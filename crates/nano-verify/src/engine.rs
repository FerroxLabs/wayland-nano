//! Trusted candidate parsing, manifest derivation, and climb driving.

use crate::VerifyError;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const CANDIDATE_CAP: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateDiff {
    paths: Vec<String>,
    bytes_sha256: String,
    records: Vec<ParsedFileRecord>,
}
impl CandidateDiff {
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
    pub fn bytes_sha256(&self) -> &str {
        &self.bytes_sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Modify,
    Delete,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChange {
    path: String,
    kind: ChangeKind,
    postimage_sha256: Option<String>,
}
impl ExpectedChange {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn kind(&self) -> ChangeKind {
        self.kind
    }
    pub fn postimage_sha256(&self) -> Option<&str> {
        self.postimage_sha256.as_deref()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChangeManifest {
    entries: Vec<ExpectedChange>,
    base_tree_digest: String,
    diff_digest: String,
}
impl ExpectedChangeManifest {
    pub fn entries(&self) -> &[ExpectedChange] {
        &self.entries
    }
    pub fn base_tree_digest(&self) -> &str {
        &self.base_tree_digest
    }
    pub fn diff_digest(&self) -> &str {
        &self.diff_digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFileRecord {
    path: String,
    kind: ChangeKind,
    hunks: Vec<ParsedHunk>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHunk {
    old_start: u64,
    old_count: u64,
    new_count: u64,
    body: Vec<BodyLine>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyLine {
    kind: u8,
    bytes: Vec<u8>,
}

pub fn parse_candidate_diff(bytes: &[u8]) -> Result<CandidateDiff, VerifyError> {
    if bytes.is_empty()
        || bytes.len() > CANDIDATE_CAP
        || bytes.last() != Some(&b'\n')
        || bytes.contains(&0)
        || bytes.contains(&b'\r')
        || std::str::from_utf8(bytes).is_err()
    {
        return invalid("candidate diff encoding");
    }
    let lines: Vec<&[u8]> = bytes[..bytes.len() - 1].split(|b| *b == b'\n').collect();
    if lines.last().is_some_and(|v| v.is_empty()) {
        return invalid("blank trailing line");
    }
    let (mut at, mut records, mut paths, mut seen) =
        (0usize, Vec::new(), Vec::new(), BTreeSet::new());
    while at < lines.len() {
        let header = text(lines[at])?;
        let rest = header
            .strip_prefix("diff --git a/")
            .ok_or_else(|| invalid_io("file header"))?;
        let (path, right) = rest
            .split_once(" b/")
            .ok_or_else(|| invalid_io("file header"))?;
        if right != path || !valid_path(path) || !seen.insert(path.to_owned()) {
            return invalid("candidate path");
        }
        at += 1;
        let old = lines
            .get(at)
            .and_then(|v| text(v).ok())
            .and_then(|v| v.strip_prefix("--- "))
            .ok_or_else(|| invalid_io("old header"))?;
        at += 1;
        let new = lines
            .get(at)
            .and_then(|v| text(v).ok())
            .and_then(|v| v.strip_prefix("+++ "))
            .ok_or_else(|| invalid_io("new header"))?;
        at += 1;
        let kind = match (old, new) {
            ("/dev/null", n) if n == format!("b/{path}") => ChangeKind::Add,
            (o, "/dev/null") if o == format!("a/{path}") => ChangeKind::Delete,
            (o, n) if o == format!("a/{path}") && n == format!("b/{path}") => ChangeKind::Modify,
            _ => return invalid("candidate file pairing"),
        };
        let mut hunks = Vec::new();
        while at < lines.len() && !lines[at].starts_with(b"diff --git ") {
            let (old_start, old_count, new_count) = parse_hunk_header(text(lines[at])?)?;
            at += 1;
            let (mut body, mut old_seen, mut new_seen) = (Vec::new(), 0u64, 0u64);
            while at < lines.len()
                && !lines[at].starts_with(b"@@ ")
                && !lines[at].starts_with(b"diff --git ")
            {
                let Some((&kind_byte, data)) = lines[at].split_first() else {
                    return invalid("empty hunk line");
                };
                match kind_byte {
                    b' ' => {
                        old_seen = inc(old_seen)?;
                        new_seen = inc(new_seen)?
                    }
                    b'-' => old_seen = inc(old_seen)?,
                    b'+' => new_seen = inc(new_seen)?,
                    _ => return invalid("hunk body marker"),
                }
                let mut exact = data.to_vec();
                exact.push(b'\n');
                body.push(BodyLine {
                    kind: kind_byte,
                    bytes: exact,
                });
                at += 1;
            }
            if body.is_empty() || old_seen != old_count || new_seen != new_count {
                return invalid("hunk count");
            }
            hunks.push(ParsedHunk {
                old_start,
                old_count,
                new_count,
                body,
            });
        }
        if hunks.is_empty() {
            return invalid("record without hunk");
        }
        paths.push(path.to_owned());
        records.push(ParsedFileRecord {
            path: path.to_owned(),
            kind,
            hunks,
        });
    }
    Ok(CandidateDiff {
        paths,
        bytes_sha256: hex_digest(bytes),
        records,
    })
}

fn text(bytes: &[u8]) -> Result<&str, VerifyError> {
    std::str::from_utf8(bytes).map_err(|_| invalid_io("utf8").into())
}
fn parse_hunk_header(line: &str) -> Result<(u64, u64, u64), VerifyError> {
    let core = line
        .strip_prefix("@@ -")
        .and_then(|v| v.strip_suffix(" @@"))
        .ok_or_else(|| invalid_io("hunk header"))?;
    let (old, new) = core
        .split_once(" +")
        .ok_or_else(|| invalid_io("hunk header"))?;
    let (ol, oc) = parse_range(old)?;
    let (_, nc) = parse_range(new)?;
    Ok((ol, oc, nc))
}
fn parse_range(value: &str) -> Result<(u64, u64), VerifyError> {
    let (line, count) = value.split_once(',').map_or((value, "1"), |(a, b)| (a, b));
    if line.is_empty()
        || count.is_empty()
        || !line.bytes().all(|b| b.is_ascii_digit())
        || !count.bytes().all(|b| b.is_ascii_digit())
    {
        return invalid("hunk range");
    }
    let line: u64 = line.parse().map_err(|_| invalid_io("hunk range"))?;
    let count: u64 = count.parse().map_err(|_| invalid_io("hunk range"))?;
    if (count == 0 && line != 0) || (count > 0 && line == 0) {
        return invalid("hunk range");
    }
    Ok((line, count))
}
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.is_ascii()
        && path != ".git"
        && !path.starts_with(".git/")
        && path.split('/').all(|c| {
            !c.is_empty()
                && c != "."
                && c != ".."
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
fn inc(v: u64) -> Result<u64, VerifyError> {
    v.checked_add(1)
        .ok_or_else(|| invalid_io("hunk counter").into())
}

pub fn derive_expected_changes(
    diff: &CandidateDiff,
    starting_root: &Path,
) -> Result<ExpectedChangeManifest, VerifyError> {
    let canonical = starting_root.canonicalize().map_err(artifact_io)?;
    if !starting_root.is_absolute()
        || canonical != starting_root
        || !canonical.is_dir()
        || symlink_component(&canonical)?
    {
        return invalid("unsafe starting root");
    }
    let (mut entries, mut base) = (
        Vec::new(),
        b"wayland-nano.expected-change.base.v1\0".to_vec(),
    );
    for record in &diff.records {
        let path = confined_path(&canonical, &record.path)?;
        let preimage = match record.kind {
            ChangeKind::Add => {
                if path.exists() {
                    return invalid("add target exists");
                }
                None
            }
            ChangeKind::Modify | ChangeKind::Delete => {
                let meta = std::fs::symlink_metadata(&path).map_err(artifact_io)?;
                if !meta.file_type().is_file() || meta.file_type().is_symlink() {
                    return invalid("invalid preimage");
                }
                Some(std::fs::read(&path).map_err(artifact_io)?)
            }
        };
        bind_len(&mut base, record.path.as_bytes());
        base.push(match record.kind {
            ChangeKind::Add => 0,
            ChangeKind::Modify => 1,
            ChangeKind::Delete => 2,
        });
        match &preimage {
            None => base.push(0),
            Some(bytes) => {
                base.push(1);
                bind_len(&mut base, bytes);
                base.extend_from_slice(&Sha256::digest(bytes));
            }
        }
        let postimage = apply_hunks(preimage.as_deref().unwrap_or_default(), record)?;
        if record.kind == ChangeKind::Delete && !postimage.is_empty() {
            return invalid("delete postimage");
        }
        entries.push(ExpectedChange {
            path: record.path.clone(),
            kind: record.kind,
            postimage_sha256: (record.kind != ChangeKind::Delete).then(|| hex_digest(&postimage)),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ExpectedChangeManifest {
        entries,
        base_tree_digest: hex_digest(&base),
        diff_digest: diff.bytes_sha256.clone(),
    })
}
fn apply_hunks(preimage: &[u8], record: &ParsedFileRecord) -> Result<Vec<u8>, VerifyError> {
    let lines: Vec<&[u8]> = preimage.split_inclusive(|b| *b == b'\n').collect();
    let (mut cursor, mut output) = (0usize, Vec::new());
    for h in &record.hunks {
        let start = if h.old_count == 0 {
            usize::try_from(h.old_start).map_err(|_| invalid_io("range"))?
        } else {
            usize::try_from(h.old_start - 1).map_err(|_| invalid_io("range"))?
        };
        if start < cursor || start > lines.len() {
            return invalid("overlapping hunk");
        }
        output.extend(lines[cursor..start].iter().flat_map(|v| v.iter()).copied());
        cursor = start;
        let (mut old_seen, mut new_seen) = (0u64, 0u64);
        for body in &h.body {
            match body.kind {
                b' ' => {
                    if lines.get(cursor).copied() != Some(body.bytes.as_slice()) {
                        return invalid("context mismatch");
                    }
                    output.extend_from_slice(&body.bytes);
                    cursor += 1;
                    old_seen = inc(old_seen)?;
                    new_seen = inc(new_seen)?
                }
                b'-' => {
                    if lines.get(cursor).copied() != Some(body.bytes.as_slice()) {
                        return invalid("deletion mismatch");
                    }
                    cursor += 1;
                    old_seen = inc(old_seen)?
                }
                b'+' => {
                    output.extend_from_slice(&body.bytes);
                    new_seen = inc(new_seen)?
                }
                _ => unreachable!(),
            }
        }
        if old_seen != h.old_count || new_seen != h.new_count {
            return invalid("application count");
        }
    }
    output.extend(lines[cursor..].iter().flat_map(|v| v.iter()).copied());
    Ok(output)
}
fn confined_path(root: &Path, relative: &str) -> Result<PathBuf, VerifyError> {
    if !valid_path(relative) {
        return invalid("unsafe path");
    }
    let mut current = root.to_path_buf();
    let parts: Vec<_> = relative.split('/').collect();
    for component in &parts[..parts.len() - 1] {
        current.push(component);
        let meta = std::fs::symlink_metadata(&current).map_err(artifact_io)?;
        if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
            return invalid("unsafe path component");
        }
    }
    current.push(parts[parts.len() - 1]);
    if !current.starts_with(root) {
        return invalid("path escape");
    }
    Ok(current)
}
fn symlink_component(path: &Path) -> Result<bool, VerifyError> {
    for current in path.ancestors().filter(|entry| entry.is_absolute()) {
        if std::fs::symlink_metadata(current)
            .map_err(artifact_io)?
            .file_type()
            .is_symlink()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn bind_len(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes)
}
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn artifact_io(error: std::io::Error) -> VerifyError {
    VerifyError::Artifact(error)
}
fn invalid_io(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}
fn invalid<T>(message: &str) -> Result<T, VerifyError> {
    Err(VerifyError::Artifact(invalid_io(message)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wp2_candidate_parser_matrix() {
        let bytes =
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let parsed = parse_candidate_diff(bytes).unwrap();
        assert_eq!(parsed.paths(), &["a.txt"]);
        assert_eq!(parsed.bytes_sha256(), hex_digest(bytes));
        for bad in [b"".as_slice(), b"```diff\n```\n", b"diff --git a/a b/a\r\n"] {
            assert!(
                matches!(parse_candidate_diff(bad),Err(VerifyError::Artifact(e))if e.kind()==std::io::ErrorKind::InvalidData)
            );
        }
    }
    #[test]
    fn wp2_expected_change_manifest_matrix() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), b"old\n").unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let diff = parse_candidate_diff(
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        let before = std::fs::read(canonical.join("a.txt")).unwrap();
        let manifest = derive_expected_changes(&diff, &canonical).unwrap();
        assert_eq!(manifest.entries()[0].kind(), ChangeKind::Modify);
        assert_eq!(
            manifest.entries()[0].postimage_sha256(),
            Some(hex_digest(b"new\n").as_str())
        );
        assert_eq!(std::fs::read(canonical.join("a.txt")).unwrap(), before);
    }
}
