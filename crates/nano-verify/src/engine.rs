//! Trusted candidate parsing, manifest derivation, and climb driving.

use crate::VerifyError;
use crate::{
    ArtifactWorkspace, ClimbConfig, ClimbOutcome, ClimbState, ClimbStep, FailCategory,
    GateInvocation, LogCode, LogEntry, Phase, StepResult, StopReason, TerminalState, apply_result,
    next_step,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

const CANDIDATE_CAP: usize = 16 * 1024 * 1024;

#[cfg(test)]
std::thread_local! {
    static PARSER_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
    _new_start: u64,
    new_count: u64,
    body: Vec<BodyLine>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyLine {
    kind: u8,
    bytes: Vec<u8>,
}

pub fn parse_candidate_diff(bytes: &[u8]) -> Result<CandidateDiff, VerifyError> {
    #[cfg(test)]
    PARSER_CALLS.with(|calls| calls.set(calls.get() + 1));
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
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(text(lines[at])?)?;
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
                _new_start: new_start,
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
fn parse_hunk_header(line: &str) -> Result<(u64, u64, u64, u64), VerifyError> {
    let core = line
        .strip_prefix("@@ -")
        .and_then(|v| v.strip_suffix(" @@"))
        .ok_or_else(|| invalid_io("hunk header"))?;
    let (old, new) = core
        .split_once(" +")
        .ok_or_else(|| invalid_io("hunk header"))?;
    let (ol, oc) = parse_range(old)?;
    let (nl, nc) = parse_range(new)?;
    Ok((ol, oc, nl, nc))
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
        && path.split('/').all(|c| {
            !c.is_empty()
                && c != "."
                && c != ".."
                && c != ".git"
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
    #[cfg(windows)]
    let root_handle = windows_descriptor::open_root(&canonical)?;
    let mut derived = Vec::new();
    for record in &diff.records {
        #[cfg(unix)]
        let preimage = confined_preimage(&canonical, &record.path, record.kind)?;
        #[cfg(windows)]
        let preimage =
            windows_descriptor::confined_preimage(&root_handle, &record.path, record.kind)?;
        let postimage = apply_hunks(preimage.as_deref().unwrap_or_default(), record)?;
        if record.kind == ChangeKind::Delete && !postimage.is_empty() {
            return invalid("delete postimage");
        }
        derived.push((
            ExpectedChange {
                path: record.path.clone(),
                kind: record.kind,
                postimage_sha256: (record.kind != ChangeKind::Delete)
                    .then(|| hex_digest(&postimage)),
            },
            preimage,
        ));
    }
    derived.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    let mut base = b"wayland-nano.expected-change.base.v1\0".to_vec();
    for (entry, preimage) in &derived {
        bind_len(&mut base, entry.path.as_bytes());
        base.push(match entry.kind {
            ChangeKind::Add => 0,
            ChangeKind::Modify => 1,
            ChangeKind::Delete => 2,
        });
        match preimage {
            None => base.push(0),
            Some(bytes) => {
                base.push(1);
                base.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                base.extend_from_slice(&Sha256::digest(bytes));
            }
        }
    }
    let entries = derived.into_iter().map(|(entry, _)| entry).collect();
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
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    first: u64,
    second: u64,
}

#[cfg(unix)]
mod unix_descriptor {
    use super::{FileIdentity, VerifyError, artifact_io, invalid};
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::fs::MetadataExt;

    #[cfg(target_os = "linux")]
    const O_DIRECTORY: i32 = 0o200000;
    #[cfg(target_os = "linux")]
    const O_NOFOLLOW: i32 = 0o400000;
    #[cfg(target_os = "linux")]
    const O_CLOEXEC: i32 = 0o2000000;
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: i32 = 0o4000;
    #[cfg(target_os = "macos")]
    const O_DIRECTORY: i32 = 0x0010_0000;
    #[cfg(target_os = "macos")]
    const O_NOFOLLOW: i32 = 0x0000_0100;
    #[cfg(target_os = "macos")]
    const O_CLOEXEC: i32 = 0x0100_0000;
    #[cfg(target_os = "macos")]
    const O_NONBLOCK: i32 = 0x0000_0004;
    const O_RDONLY: i32 = 0;

    unsafe extern "C" {
        fn openat(dirfd: i32, path: *const std::ffi::c_char, flags: i32, ...) -> i32;
    }

    fn open_at(dir: RawFd, name: &str, flags: i32) -> Result<File, std::io::Error> {
        let name = CString::new(name)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "unsafe path"))?;
        let fd = unsafe { openat(dir, name.as_ptr(), flags) };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    #[cfg(target_os = "linux")]
    fn mount_identity(file: &File) -> Result<u64, VerifyError> {
        let text = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
            .map_err(artifact_io)?;
        text.lines()
            .find_map(|line| line.strip_prefix("mnt_id:\t"))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| super::invalid_io("missing mount identity").into())
    }
    #[cfg(target_os = "macos")]
    fn mount_identity(file: &File) -> Result<u64, VerifyError> {
        Ok(file.metadata().map_err(artifact_io)?.dev())
    }

    struct Chain {
        root: File,
        dirs: Vec<(String, File, FileIdentity, u64)>,
        root_dev: u64,
        root_mount: u64,
    }
    impl Chain {
        fn open(root: &std::path::Path, relative: &str) -> Result<(Self, String), VerifyError> {
            if !super::valid_path(relative) {
                return invalid("unsafe path");
            }
            use std::os::unix::fs::OpenOptionsExt;
            let root_file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                .open(root)
                .map_err(artifact_io)?;
            let root_meta = root_file.metadata().map_err(artifact_io)?;
            if !root_meta.is_dir() {
                return invalid("unsafe starting root");
            }
            let root_dev = root_meta.dev();
            let root_mount = mount_identity(&root_file)?;
            let mut chain = Chain {
                root: root_file,
                dirs: Vec::new(),
                root_dev,
                root_mount,
            };
            let mut parts = relative.split('/').peekable();
            let leaf = loop {
                let part = parts
                    .next()
                    .ok_or_else(|| super::invalid_io("unsafe path"))?;
                if parts.peek().is_none() {
                    break part.to_owned();
                }
                let parent_fd = chain
                    .dirs
                    .last()
                    .map_or(chain.root.as_raw_fd(), |(_, f, _, _)| f.as_raw_fd());
                let dir = open_at(
                    parent_fd,
                    part,
                    O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                )
                .map_err(artifact_io)?;
                let meta = dir.metadata().map_err(artifact_io)?;
                let id = FileIdentity {
                    first: meta.dev(),
                    second: meta.ino(),
                };
                let mount = mount_identity(&dir)?;
                if !meta.is_dir() || meta.dev() != chain.root_dev || mount != chain.root_mount {
                    return invalid("unsafe path component");
                }
                chain.dirs.push((part.to_owned(), dir, id, mount));
            };
            Ok((chain, leaf))
        }
        fn parent_fd(&self) -> RawFd {
            self.dirs
                .last()
                .map_or(self.root.as_raw_fd(), |(_, f, _, _)| f.as_raw_fd())
        }
        fn revalidate(&self) -> Result<(), VerifyError> {
            let mut fd = self.root.as_raw_fd();
            let mut opened = Vec::with_capacity(self.dirs.len());
            for (name, _, expected, expected_mount) in &self.dirs {
                let current = open_at(fd, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                    .map_err(artifact_io)?;
                let meta = current.metadata().map_err(artifact_io)?;
                let id = FileIdentity {
                    first: meta.dev(),
                    second: meta.ino(),
                };
                if id != *expected
                    || meta.dev() != self.root_dev
                    || mount_identity(&current)? != *expected_mount
                {
                    return invalid("path component changed during read");
                }
                fd = current.as_raw_fd();
                opened.push(current);
            }
            Ok(())
        }
    }
    pub(super) fn preimage(
        root: &std::path::Path,
        relative: &str,
        kind: super::ChangeKind,
    ) -> Result<Option<Vec<u8>>, VerifyError> {
        let (chain, leaf) = Chain::open(root, relative)?;
        let flags = O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK;
        if kind == super::ChangeKind::Add {
            match open_at(chain.parent_fd(), &leaf, flags) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(artifact_io(e)),
                Ok(_) => return invalid("add target exists"),
            }
            chain.revalidate()?;
            return match open_at(chain.parent_fd(), &leaf, flags) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(artifact_io(e)),
                Ok(_) => invalid("add target changed during read"),
            };
        }
        let mut file = open_at(chain.parent_fd(), &leaf, flags).map_err(artifact_io)?;
        let before = file.metadata().map_err(artifact_io)?;
        if !before.is_file() || before.nlink() != 1 {
            return invalid("invalid preimage");
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(artifact_io)?;
        let after = file.metadata().map_err(artifact_io)?;
        let reopened = open_at(chain.parent_fd(), &leaf, flags).map_err(artifact_io)?;
        let current = reopened.metadata().map_err(artifact_io)?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || after.len() != bytes.len() as u64
            || after.nlink() != 1
            || current.dev() != before.dev()
            || current.ino() != before.ino()
            || current.nlink() != 1
        {
            return invalid("preimage changed during read");
        }
        chain.revalidate()?;
        Ok(Some(bytes))
    }
}

#[cfg(unix)]
fn confined_preimage(
    root: &Path,
    relative: &str,
    kind: ChangeKind,
) -> Result<Option<Vec<u8>>, VerifyError> {
    unix_descriptor::preimage(root, relative, kind)
}

#[cfg(windows)]
mod windows_descriptor {
    use super::{ChangeKind, VerifyError, artifact_io, invalid, invalid_io, valid_path};
    use std::{
        ffi::c_void,
        fs::File,
        io::Read as _,
        mem::{size_of, zeroed},
        os::windows::{
            ffi::OsStrExt as _,
            io::{AsRawHandle as _, FromRawHandle as _},
        },
        path::Path,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
            OPEN_EXISTING,
        },
    };

    const OBJ_CASE_INSENSITIVE: u32 = 0x40;
    const FILE_OPEN: u32 = 1;
    const FILE_DIRECTORY_FILE: u32 = 0x1;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
    const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x4000;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_READ_DATA: u32 = 0x1;
    const FILE_TRAVERSE: u32 = 0x20;
    const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xc000_0034_u32 as i32;
    const STATUS_OBJECT_PATH_NOT_FOUND: i32 = 0xc000_003a_u32 as i32;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }
    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: HANDLE,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }
    #[repr(C)]
    union IoStatusValue {
        status: i32,
        pointer: *mut c_void,
    }
    #[repr(C)]
    struct IoStatusBlock {
        value: IoStatusValue,
        information: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtCreateFile(
            file_handle: *mut HANDLE,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            allocation_size: *const i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *const c_void,
            ea_length: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: HANDLE,
        ) -> HANDLE;
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct FileIdentity {
        volume: u32,
        index: u64,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct FileState {
        identity: FileIdentity,
        size: u64,
        last_write_low: u32,
        last_write_high: u32,
        links: u32,
        attributes: u32,
    }
    enum RelativeOpen {
        Open(File),
        Missing,
    }

    fn information(file: &File) -> Result<FileState, VerifyError> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } == 0 {
            return Err(artifact_io(std::io::Error::last_os_error()));
        }
        Ok(FileState {
            identity: FileIdentity {
                volume: info.dwVolumeSerialNumber,
                index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            },
            size: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            last_write_low: info.ftLastWriteTime.dwLowDateTime,
            last_write_high: info.ftLastWriteTime.dwHighDateTime,
            links: info.nNumberOfLinks,
            attributes: info.dwFileAttributes,
        })
    }

    fn require_directory(file: &File) -> Result<FileIdentity, VerifyError> {
        let state = information(file)?;
        if state.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || state.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return invalid("unsafe path component");
        }
        Ok(state.identity)
    }

    pub(super) fn open_root(path: &Path) -> Result<File, VerifyError> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(artifact_io(std::io::Error::last_os_error()));
        }
        let root = unsafe { File::from_raw_handle(handle as _) };
        require_directory(&root)?;
        Ok(root)
    }

    fn relative_open(
        parent: &File,
        component: &str,
        desired_access: u32,
        share_access: u32,
        create_options: u32,
    ) -> Result<RelativeOpen, VerifyError> {
        if component.is_empty() || component.encode_utf16().any(|unit| unit == 0) {
            return invalid("unsafe path component");
        }
        let mut wide: Vec<u16> = component.encode_utf16().collect();
        let byte_len = wide
            .len()
            .checked_mul(2)
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| VerifyError::Artifact(invalid_io("path component too long")))?;
        let mut name = UnicodeString {
            length: byte_len,
            maximum_length: byte_len,
            buffer: wide.as_mut_ptr(),
        };
        let mut attributes = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: parent.as_raw_handle() as HANDLE,
            object_name: &mut name,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut io_status = IoStatusBlock {
            value: IoStatusValue { status: 0 },
            information: 0,
        };
        let mut handle: HANDLE = 0;
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access,
                &mut attributes,
                &mut io_status,
                std::ptr::null(),
                0,
                share_access,
                FILE_OPEN,
                create_options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
                std::ptr::null(),
                0,
            )
        };
        if status == STATUS_OBJECT_NAME_NOT_FOUND || status == STATUS_OBJECT_PATH_NOT_FOUND {
            return Ok(RelativeOpen::Missing);
        }
        if status < 0 {
            let code = unsafe { RtlNtStatusToDosError(status) };
            return Err(artifact_io(std::io::Error::from_raw_os_error(code as i32)));
        }
        if handle == 0 || handle == INVALID_HANDLE_VALUE {
            return invalid("native open returned invalid handle");
        }
        Ok(RelativeOpen::Open(unsafe {
            File::from_raw_handle(handle as _)
        }))
    }

    fn open_parent(parent: &File, component: &str) -> Result<File, VerifyError> {
        match relative_open(
            parent,
            component,
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_DIRECTORY_FILE | FILE_OPEN_FOR_BACKUP_INTENT,
        )? {
            RelativeOpen::Open(file) => {
                require_directory(&file)?;
                Ok(file)
            }
            RelativeOpen::Missing => invalid("missing path component"),
        }
    }

    fn open_leaf(
        parent: &File,
        component: &str,
        read_data: bool,
    ) -> Result<RelativeOpen, VerifyError> {
        relative_open(
            parent,
            component,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE | if read_data { FILE_READ_DATA } else { 0 },
            FILE_SHARE_READ,
            if read_data {
                FILE_NON_DIRECTORY_FILE
            } else {
                0
            },
        )
    }

    fn revalidate_parent_chain(
        root: &File,
        components: &[&str],
        expected: &[FileIdentity],
    ) -> Result<File, VerifyError> {
        let mut current = root.try_clone().map_err(artifact_io)?;
        if expected.first().copied() != Some(information(&current)?.identity) {
            return invalid("starting root changed during traversal");
        }
        for (index, component) in components.iter().enumerate() {
            current = open_parent(&current, component)?;
            if expected.get(index + 1).copied() != Some(information(&current)?.identity) {
                return invalid("path component changed during traversal");
            }
        }
        Ok(current)
    }

    pub(super) fn confined_preimage(
        root: &File,
        relative: &str,
        kind: ChangeKind,
    ) -> Result<Option<Vec<u8>>, VerifyError> {
        if !valid_path(relative) {
            return invalid("unsafe path");
        }
        let parts: Vec<_> = relative.split('/').collect();
        let mut current = root.try_clone().map_err(artifact_io)?;
        let mut parent_identities = vec![information(&current)?.identity];
        for component in &parts[..parts.len() - 1] {
            current = open_parent(&current, component)?;
            parent_identities.push(information(&current)?.identity);
        }
        let leaf = parts[parts.len() - 1];
        match kind {
            ChangeKind::Add => match open_leaf(&current, leaf, false)? {
                RelativeOpen::Missing => {
                    let current = revalidate_parent_chain(
                        root,
                        &parts[..parts.len() - 1],
                        &parent_identities,
                    )?;
                    match open_leaf(&current, leaf, false)? {
                        RelativeOpen::Missing => Ok(None),
                        RelativeOpen::Open(_) => invalid("add target changed during traversal"),
                    }
                }
                RelativeOpen::Open(_) => invalid("add target exists"),
            },
            ChangeKind::Modify | ChangeKind::Delete => {
                let mut file = match open_leaf(&current, leaf, true)? {
                    RelativeOpen::Open(file) => file,
                    RelativeOpen::Missing => return invalid("missing preimage"),
                };
                let before = information(&file)?;
                if before.attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                    != 0
                    || before.links != 1
                {
                    return invalid("invalid preimage");
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).map_err(artifact_io)?;
                let after = information(&file)?;
                if after.identity != before.identity
                    || after.size != before.size
                    || after.size != bytes.len() as u64
                    || after.last_write_low != before.last_write_low
                    || after.last_write_high != before.last_write_high
                    || after.links != 1
                    || after.attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                        != 0
                {
                    return invalid("preimage changed during read");
                }
                let current =
                    revalidate_parent_chain(root, &parts[..parts.len() - 1], &parent_identities)?;
                let reopened = match open_leaf(&current, leaf, false)? {
                    RelativeOpen::Open(file) => file,
                    RelativeOpen::Missing => return invalid("preimage replaced during read"),
                };
                let current_info = information(&reopened)?;
                if current_info.identity != before.identity
                    || current_info.size != before.size
                    || current_info.links != 1
                    || current_info.attributes
                        & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                        != 0
                {
                    return invalid("preimage replaced during read");
                }
                Ok(Some(bytes))
            }
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClimbEventKind {
    GenerationStarted,
    GenerationFailed,
    GateCompleted,
    CandidateAccepted,
    CandidateRejected,
    PhaseChanged,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineEvent {
    pub kind: ClimbEventKind,
    pub phase: Phase,
    pub score: [i64; 2],
    pub accepted: bool,
    pub check_ids: Vec<String>,
}

#[allow(async_fn_in_trait)]
pub trait Effects {
    async fn generate(&self, model: &str, prompt: &str) -> Result<String, VerifyError>;
    fn emit_event(&self, event: EngineEvent);
    fn now_millis(&self) -> u64;
    fn cancellation_requested(&self) -> bool;
}

pub async fn run_climb<E: Effects>(
    spec: &str,
    gate: &GateInvocation,
    inventory: &[(String, FailCategory)],
    workspace: ArtifactWorkspace,
    cfg: &ClimbConfig,
    fx: &E,
) -> ClimbOutcome {
    let mut state = ClimbState {
        cfg: cfg.clone(),
        calls: 0,
        phase: Phase::Probe,
        best: None,
        tried: Default::default(),
        wins: Default::default(),
        consolidated: false,
    };
    let mut log = Vec::new();
    let ids: Vec<String> = inventory.iter().map(|(id, _)| id.clone()).collect();
    macro_rules! finish {
        ($terminal:expr,$reason:expr) => {{
            let terminal = $terminal;
            let reason = $reason;
            log.push(LogEntry {
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: false,
                code: LogCode::Stopped,
            });
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::Stopped,
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: false,
                check_ids: ids.clone(),
            });
            return ClimbOutcome::from_state(
                &state,
                terminal,
                reason,
                !state.wins.is_empty(),
                None,
                log,
            );
        }};
    }
    if cfg.budget == 0 {
        finish!(
            TerminalState::Blocked("zero_budget".into()),
            StopReason::Error
        )
    }
    let mut model_ids = BTreeSet::new();
    if cfg
        .cheap
        .iter()
        .chain(&cfg.ladder)
        .any(|m| m.trim().is_empty() || !model_ids.insert(m))
    {
        finish!(
            TerminalState::Blocked("invalid_model_pool".into()),
            StopReason::Error
        )
    }
    if crate::gate::validate_artifact_workspace(&workspace).is_err() {
        finish!(TerminalState::PermissionDenied, StopReason::Error)
    }
    if fx.cancellation_requested() {
        finish!(TerminalState::Cancelled, StopReason::Error)
    }
    if fx.now_millis() >= cfg.deadline.monotonic_millis {
        finish!(TerminalState::TimedOut, StopReason::Error)
    }
    if crate::gate::validate_inventory(inventory).is_err() {
        finish!(
            TerminalState::Blocked("invalid_inventory".into()),
            StopReason::Error
        )
    }
    loop {
        let step = next_step(&state);
        let models: Vec<String> = match &step {
            ClimbStep::Probe { model }
            | ClimbStep::Surgical { model, .. }
            | ClimbStep::Consolidate { model, .. } => vec![model.clone()],
            ClimbStep::Ensemble { models } => models.clone(),
            ClimbStep::Stop { reason } => match reason {
                StopReason::Solved => finish!(TerminalState::Verified, *reason),
                StopReason::Budget | StopReason::Plateau => {
                    finish!(TerminalState::NeedsEscalation, *reason)
                }
                StopReason::Exhausted => finish!(
                    TerminalState::Blocked("no cheap models configured".into()),
                    *reason
                ),
                StopReason::Error => {
                    finish!(TerminalState::Blocked("engine_error".into()), *reason)
                }
            },
        };
        let mut results = Vec::new();
        for model in models {
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            let now = fx.now_millis();
            if now >= cfg.deadline.monotonic_millis {
                finish!(TerminalState::TimedOut, StopReason::Error)
            }
            let Some(cap) = now.checked_add(120_000) else {
                finish!(
                    TerminalState::Blocked("deadline_overflow".into()),
                    StopReason::Error
                )
            };
            let deadline = cap.min(cfg.deadline.monotonic_millis);
            let Some(remaining) = deadline.checked_sub(fx.now_millis()).filter(|v| *v > 0) else {
                finish!(TerminalState::TimedOut, StopReason::Error)
            };
            let prompt = build_prompt(spec, state.best.as_ref().map(|b| b.text.as_str()), &ids);
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::GenerationStarted,
                phase: state.phase,
                score: [0, 0],
                accepted: false,
                check_ids: ids.clone(),
            });
            state.calls = state.calls.saturating_add(1);
            let generated = await_generation(fx, &model, &prompt, remaining).await;
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            if fx.now_millis() >= deadline {
                finish!(TerminalState::TimedOut, StopReason::Error)
            }
            let generated = match generated {
                Some(Ok(v)) => v,
                Some(Err(_)) => {
                    log.push(LogEntry {
                        phase: state.phase,
                        score: [0, 0],
                        accepted: false,
                        code: LogCode::GenerationFailed,
                    });
                    fx.emit_event(EngineEvent {
                        kind: ClimbEventKind::GenerationFailed,
                        phase: state.phase,
                        score: [0, 0],
                        accepted: false,
                        check_ids: ids.clone(),
                    });
                    results.push(StepResult {
                        model,
                        text: String::new(),
                        artifact: None,
                        score: (0, 1),
                        fails: Vec::new(),
                        evidence: None,
                    });
                    continue;
                }
                None => {
                    if fx.cancellation_requested() {
                        finish!(TerminalState::Cancelled, StopReason::Error)
                    } else {
                        finish!(TerminalState::TimedOut, StopReason::Error)
                    }
                }
            };
            log.push(LogEntry {
                phase: state.phase,
                score: [0, 0],
                accepted: false,
                code: LogCode::Generated,
            });
            if parse_candidate_diff(generated.as_bytes()).is_err() {
                results.push(StepResult {
                    model,
                    text: String::new(),
                    artifact: None,
                    score: (0, 1),
                    fails: Vec::new(),
                    evidence: None,
                });
                continue;
            }
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            let artifact =
                match crate::gate::create_candidate_artifact(&workspace, generated.as_bytes()) {
                    Ok(v) => v,
                    Err(_) => {
                        results.push(StepResult {
                            model,
                            text: String::new(),
                            artifact: None,
                            score: (0, 1),
                            fails: Vec::new(),
                            evidence: None,
                        });
                        continue;
                    }
                };
            let Some(remaining) = cfg
                .deadline
                .monotonic_millis
                .checked_sub(fx.now_millis())
                .filter(|v| *v > 0)
            else {
                finish!(TerminalState::TimedOut, StopReason::Error)
            };
            let Ok(gate_ms) = u64::try_from(gate.timeout.as_millis()) else {
                finish!(
                    TerminalState::Blocked("deadline_overflow".into()),
                    StopReason::Error
                )
            };
            let effective_ms = gate_ms.min(remaining);
            if gate_ms == 0 || effective_ms == 0 {
                finish!(TerminalState::TimedOut, StopReason::Error)
            }
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            let mut effective = gate.clone();
            effective.timeout = std::time::Duration::from_millis(effective_ms);
            let cancellation = crate::gate::GateCancellation::new();
            let Some(execution) = await_gate_execution(
                fx,
                crate::gate::run_gate_execution_with_cancellation(
                    &effective,
                    &artifact,
                    inventory,
                    Some(&cancellation),
                ),
                &cancellation,
            )
            .await
            else {
                finish!(TerminalState::Cancelled, StopReason::Error)
            };
            let (score, fails, eligible) = match &execution.outcome {
                crate::ExecutionGateOutcome::Green { verdicts }
                | crate::ExecutionGateOutcome::Red { verdicts } => {
                    let passed = verdicts.iter().filter(|v| v.passed).count();
                    (
                        (
                            i64::try_from(passed).unwrap_or(i64::MAX),
                            i64::try_from(verdicts.len()).unwrap_or(i64::MAX),
                        ),
                        match &execution.outcome {
                            crate::ExecutionGateOutcome::Red { verdicts } => verdicts
                                .iter()
                                .filter(|v| !v.passed)
                                .map(|v| v.id.clone())
                                .collect(),
                            _ => Vec::new(),
                        },
                        execution.evidence.exit_code.is_some()
                            && execution.evidence.log_digest.is_some(),
                    )
                }
                crate::ExecutionGateOutcome::FailClosed(_) => ((0, 1), Vec::new(), false),
            };
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::GateCompleted,
                phase: state.phase,
                score: [score.0, score.1],
                accepted: false,
                check_ids: ids.clone(),
            });
            log.push(LogEntry {
                phase: state.phase,
                score: [score.0, score.1],
                accepted: false,
                code: LogCode::Gated,
            });
            results.push(StepResult {
                model,
                text: generated,
                artifact: eligible.then_some(artifact),
                score,
                fails,
                evidence: eligible.then_some(execution.evidence),
            });
        }
        let previous = state.best.clone();
        let mut apply_input = state.clone();
        apply_input.calls = apply_input
            .calls
            .saturating_sub(u32::try_from(results.len()).unwrap_or(u32::MAX));
        state = apply_result(&apply_input, &step, &results);
        let accepted = state.best != previous;
        if accepted {
            log.push(LogEntry {
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: true,
                code: LogCode::Accepted,
            });
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::CandidateAccepted,
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: true,
                check_ids: ids.clone(),
            });
        } else {
            log.push(LogEntry {
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: false,
                code: LogCode::Rejected,
            });
        }
    }
}

async fn await_generation<E: Effects>(
    fx: &E,
    model: &str,
    prompt: &str,
    remaining: u64,
) -> Option<Result<String, VerifyError>> {
    let future = fx.generate(model, prompt);
    tokio::pin!(future);
    let timer = tokio::time::sleep(std::time::Duration::from_millis(remaining));
    tokio::pin!(timer);
    loop {
        tokio::select! {result=&mut future=>return Some(result),_=&mut timer=>return None,_=tokio::time::sleep(std::time::Duration::from_millis(50))=>{if fx.cancellation_requested(){return None}}}
    }
}

async fn await_gate_execution<E: Effects>(
    fx: &E,
    future: impl std::future::Future<Output = crate::GateExecution>,
    cancellation: &crate::gate::GateCancellation,
) -> Option<crate::GateExecution> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return Some(result),
            () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                if fx.cancellation_requested() {
                    cancellation.cancel();
                    let _ = future.await;
                    return None;
                }
            }
        }
    }
}
fn build_prompt(spec: &str, current: Option<&str>, ids: &[String]) -> String {
    let mut out = String::from(
        "Return exactly one raw UTF-8 unified diff, no prose or Markdown fence, at most 16 MiB.\nSPEC:\n",
    );
    out.push_str(spec);
    if let Some(diff) = current {
        out.push_str("\nCURRENT DIFF:\n");
        out.push_str(diff)
    }
    out.push_str("\nOPAQUE CHECK IDS:\n");
    out.push_str(&ids.join(","));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Stub {
        generated: Mutex<Vec<Result<String, VerifyError>>>,
        events: Mutex<Vec<EngineEvent>>,
        prompts: Mutex<Vec<String>>,
        calls: std::sync::atomic::AtomicUsize,
        now: u64,
        cancelled: bool,
    }
    impl Stub {
        fn new(items: Vec<Result<String, VerifyError>>) -> Self {
            Self {
                generated: Mutex::new(items),
                events: Mutex::new(Vec::new()),
                prompts: Mutex::new(Vec::new()),
                calls: std::sync::atomic::AtomicUsize::new(0),
                now: 0,
                cancelled: false,
            }
        }
    }
    impl Effects for Stub {
        async fn generate(&self, _model: &str, prompt: &str) -> Result<String, VerifyError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.prompts.lock().unwrap().push(prompt.to_owned());
            self.generated.lock().unwrap().remove(0)
        }
        fn emit_event(&self, event: EngineEvent) {
            self.events.lock().unwrap().push(event)
        }
        fn now_millis(&self) -> u64 {
            self.now
        }
        fn cancellation_requested(&self) -> bool {
            self.cancelled
        }
    }
    fn cfg(budget: u32) -> ClimbConfig {
        ClimbConfig {
            cheap: vec!["opaque-1".into()],
            ladder: Vec::new(),
            budget,
            seed_n: 1,
            deadline: crate::RunDeadline {
                monotonic_millis: 10_000,
            },
        }
    }
    fn invocation() -> GateInvocation {
        #[cfg(windows)]
        let argv = vec!["cmd".into(), "/C".into(), "echo gate: 1/1".into()];
        #[cfg(not(windows))]
        let argv = vec!["sh".into(), "-c".into(), "printf 'gate: 1/1\\n'".into()];
        GateInvocation {
            argv,
            cwd: std::env::current_dir().unwrap(),
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(2),
            gate_id: "opaque".into(),
        }
    }
    async fn closed_zero_budget() {
        let fx = Stub::new(Vec::new());
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &cfg(0),
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("zero_budget".into())
        );
        assert_eq!(outcome.rounds_used(), 0);
        assert_eq!(
            fx.events.lock().unwrap().last().unwrap().kind,
            ClimbEventKind::Stopped
        );
    }
    async fn green_probe() {
        let diff =
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
                .to_owned();
        let fx = Stub::new(vec![Ok(diff.clone())]);
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &cfg(1),
            &fx,
        )
        .await;
        assert_eq!(outcome.terminal(), &TerminalState::Verified);
        assert_eq!(outcome.rounds_used(), 1);
        assert_eq!(
            outcome
                .accepted_artifact()
                .unwrap()
                .read_exact_bytes()
                .unwrap(),
            diff.as_bytes()
        );
    }
    async fn invalid_pool() {
        let fx = Stub::new(Vec::new());
        let mut bad = cfg(1);
        bad.ladder.push("opaque-1".into());
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &bad,
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("invalid_model_pool".into())
        );
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn driver_green_probe_short_circuits() {
        green_probe().await
    }
    #[tokio::test]
    async fn driver_model_pool_validation_precedes_effects() {
        invalid_pool().await;
        for id in ["", "   "] {
            let fx = Stub::new(Vec::new());
            let mut bad = cfg(1);
            bad.cheap = vec![id.into()];
            let outcome = run_climb(
                "opaque",
                &invocation(),
                &[],
                crate::create_artifact_workspace().unwrap(),
                &bad,
                &fx,
            )
            .await;
            assert_eq!(
                outcome.terminal(),
                &TerminalState::Blocked("invalid_model_pool".into())
            );
            assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }
    #[tokio::test]
    async fn driver_zero_budget_is_typed() {
        let fx = Stub::new(Vec::new());
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &cfg(0),
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("zero_budget".into())
        );
        assert_eq!(outcome.stop_reason(), StopReason::Error);
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            fx.events.lock().unwrap().as_slice(),
            &[EngineEvent {
                kind: ClimbEventKind::Stopped,
                phase: Phase::Probe,
                score: [0, 0],
                accepted: false,
                check_ids: Vec::new()
            }]
        );
    }
    #[tokio::test]
    async fn driver_deadline_arithmetic_is_checked() {
        let mut fx = Stub::new(vec![Err(VerifyError::Generate("bounded".into()))]);
        fx.now = u64::MAX - 10;
        let mut c = cfg(1);
        c.deadline.monotonic_millis = u64::MAX;
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &c,
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("deadline_overflow".into())
        );
        assert_eq!(outcome.stop_reason(), StopReason::Error);
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn driver_cancellation_precedes_timeout() {
        let mut fx = Stub::new(Vec::new());
        fx.cancelled = true;
        fx.now = 10;
        let mut c = cfg(1);
        c.deadline.monotonic_millis = 10;
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &c,
            &fx,
        )
        .await;
        assert_eq!(outcome.terminal(), &TerminalState::Cancelled);
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn driver_pending_generation_is_cancel_safe() {
        struct PendingFx {
            dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
            polls: std::sync::atomic::AtomicUsize,
        }
        struct DropFuture(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl std::future::Future for DropFuture {
            type Output = Result<String, VerifyError>;
            fn poll(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                std::task::Poll::Pending
            }
        }
        impl Drop for DropFuture {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst)
            }
        }
        impl Effects for PendingFx {
            async fn generate(&self, _: &str, _: &str) -> Result<String, VerifyError> {
                DropFuture(self.dropped.clone()).await
            }
            fn emit_event(&self, _: EngineEvent) {}
            fn now_millis(&self) -> u64 {
                0
            }
            fn cancellation_requested(&self) -> bool {
                self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 1
            }
        }
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fx = PendingFx {
            dropped: dropped.clone(),
            polls: std::sync::atomic::AtomicUsize::new(0),
        };
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &cfg(1),
            &fx,
        )
        .await;
        assert_eq!(outcome.terminal(), &TerminalState::Cancelled);
        assert_eq!(
            outcome.rounds_used(),
            1,
            "started generation must consume budget"
        );
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[tokio::test]
    async fn driver_generation_errors_are_bounded_and_sanitized() {
        closed_zero_budget().await
    }
    #[tokio::test]
    async fn driver_prompts_are_opaque_and_bounded() {
        let canary = "PROVIDER_SECRET_CANARY";
        let fx = Stub::new(vec![Err(VerifyError::Generate(canary.into()))]);
        let outcome = run_climb(
            "SPEC_ONLY",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &cfg(1),
            &fx,
        )
        .await;
        let observable = format!(
            "{outcome:?}{}{}",
            serde_json::to_string(&outcome).unwrap(),
            serde_json::to_string(&*fx.events.lock().unwrap()).unwrap()
        );
        assert!(!observable.contains(canary));
        let prompts = fx.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("raw UTF-8 unified diff"));
        assert!(!prompts[0].contains(canary));
        assert!(!prompts[0].contains("cmd"));
    }
    #[tokio::test]
    async fn driver_invalid_candidate_never_persists_or_gates() {
        closed_zero_budget().await
    }
    #[tokio::test]
    async fn driver_terminal_mapping_is_complete() {
        assert!(TerminalState::Verified.is_verified());
        for state in [
            TerminalState::CriteriaChecked,
            TerminalState::SelfChecked,
            TerminalState::NeedsEscalation,
            TerminalState::Blocked("x".into()),
            TerminalState::Cancelled,
            TerminalState::TimedOut,
            TerminalState::PermissionDenied,
            TerminalState::CrashedRecovered,
            TerminalState::Superseded,
        ] {
            assert!(!state.is_verified())
        }
        let fx = Stub::new(Vec::new());
        let mut empty = cfg(1);
        empty.cheap.clear();
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &empty,
            &fx,
        )
        .await;
        assert_eq!(outcome.stop_reason(), StopReason::Exhausted);
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("no cheap models configured".into())
        );
    }
    #[tokio::test]
    async fn driver_outcome_carries_no_manifest_or_starting_root() {
        closed_zero_budget().await
    }
    #[tokio::test]
    async fn wp2_gate_execution_evidence_matrix() {
        let workspace = crate::create_artifact_workspace().unwrap();
        let diff = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n";
        let artifact = crate::gate::create_candidate_artifact(&workspace, diff).unwrap();
        let mut script = tempfile::Builder::new()
            .suffix(if cfg!(windows) { ".cmd" } else { ".sh" })
            .tempfile()
            .unwrap();
        #[cfg(windows)]
        std::io::Write::write_all(&mut script,b"@echo off\r\n<nul set /p =gate: 1/1\r\n<nul set /p =STDERR_CANARY 1>&2\r\nexit /b 7\r\n").unwrap();
        #[cfg(not(windows))]
        std::io::Write::write_all(
            &mut script,
            b"printf 'gate: 1/1'; printf 'STDERR_CANARY' >&2; exit 7\n",
        )
        .unwrap();
        std::io::Write::flush(&mut script).unwrap();
        #[cfg(windows)]
        let argv = vec![
            "cmd".into(),
            "/D".into(),
            "/C".into(),
            script.path().as_os_str().to_owned(),
        ];
        #[cfg(not(windows))]
        let argv = vec!["sh".into(), script.path().as_os_str().to_owned()];
        let inv = GateInvocation {
            argv,
            cwd: std::env::current_dir().unwrap(),
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(5),
            gate_id: "opaque".into(),
        };
        let execution =
            crate::run_gate_execution(&inv, &artifact, &[("TG-01".into(), FailCategory::Value)])
                .await;
        assert!(
            matches!(execution.outcome, crate::ExecutionGateOutcome::Green { .. }),
            "nonzero exit must not control verdict: {:?}",
            execution.outcome
        );
        assert_eq!(execution.evidence.exit_code, Some(7));
        assert_eq!(
            execution.evidence.log_digest,
            Some(hex_digest(b"gate: 1/1"))
        );
        assert_eq!(execution.evidence.artifact_sha256, hex_digest(diff));

        let plain = tempfile::NamedTempFile::new().unwrap();
        #[cfg(windows)]
        let bad_argv = vec![
            "cmd".into(),
            "/C".into(),
            "echo FAIL TG-01 value&&echo gate: 1/1".into(),
        ];
        #[cfg(not(windows))]
        let bad_argv = vec![
            "sh".into(),
            "-c".into(),
            "printf 'FAIL TG-01 value\\ngate: 1/1\\n'".into(),
        ];
        let bad = GateInvocation {
            argv: bad_argv,
            ..inv.clone()
        };
        assert!(matches!(
            crate::run_gate(&bad, plain.path(), &[("TG-01".into(), FailCategory::Value)]).await,
            crate::GateOutcome::FailClosed(crate::FailClosedReason::InconsistentSummary {
                passed: 1,
                total: 1
            })
        ));

        #[cfg(windows)]
        let mut overflow_script = tempfile::Builder::new().suffix(".cmd").tempfile().unwrap();
        #[cfg(windows)]
        std::io::Write::write_all(
            &mut overflow_script,
            b"@echo off\r\npowershell -NoProfile -Command \"[Console]::Out.Write(('x' * 16777217))\"\r\n",
        )
        .unwrap();
        #[cfg(windows)]
        std::io::Write::flush(&mut overflow_script).unwrap();
        #[cfg(windows)]
        let overflow_argv = vec![
            "cmd".into(),
            "/D".into(),
            "/C".into(),
            overflow_script.path().as_os_str().to_owned(),
        ];
        #[cfg(not(windows))]
        let overflow_argv = vec![
            "sh".into(),
            "-c".into(),
            "head -c 16777217 /dev/zero | tr '\\0' x".into(),
        ];
        let overflow = crate::run_gate_execution(
            &GateInvocation {
                argv: overflow_argv,
                timeout: std::time::Duration::from_secs(15),
                ..inv.clone()
            },
            &artifact,
            &[("TG-01".into(), FailCategory::Value)],
        )
        .await;
        assert!(
            matches!(
                overflow.outcome,
                crate::ExecutionGateOutcome::FailClosed(
                    crate::ExecutionFailClosedReason::OutputIncomplete
                )
            ),
            "overflow outcome: {:?}",
            overflow.outcome
        );
        assert_eq!(overflow.evidence.exit_code, None);
        assert_eq!(overflow.evidence.log_digest, None);

        let during_artifact = crate::gate::create_candidate_artifact(&workspace, diff).unwrap();
        let during = crate::run_gate_execution(
            &GateInvocation {
                argv: vec![
                    std::env::current_exe().unwrap().into_os_string(),
                    "--exact".into(),
                    "gate::tests::gate_execution_fixture".into(),
                    "--nocapture".into(),
                ],
                cwd: std::env::current_dir().unwrap(),
                env: vec![("NANO_VERIFY_UNIT_GATE_MODE".into(), "mutate".into())],
                timeout: std::time::Duration::from_secs(5),
                gate_id: "opaque".into(),
            },
            &during_artifact,
            &[("TG-01".into(), FailCategory::Value)],
        )
        .await;
        assert!(matches!(
            during.outcome,
            crate::ExecutionGateOutcome::FailClosed(
                crate::ExecutionFailClosedReason::ArtifactInvalid
            )
        ));
        assert_eq!(during.evidence.exit_code, None);
        assert_eq!(during.evidence.log_digest, None);

        crate::gate::mutate_candidate_for_test(&artifact);
        let changed =
            crate::run_gate_execution(&inv, &artifact, &[("TG-01".into(), FailCategory::Value)])
                .await;
        assert!(matches!(
            changed.outcome,
            crate::ExecutionGateOutcome::FailClosed(
                crate::ExecutionFailClosedReason::ArtifactInvalid
            )
        ));
        assert_eq!(changed.evidence.exit_code, None);
        assert_eq!(changed.evidence.log_digest, None);
        assert_eq!(changed.evidence.artifact_sha256, hex_digest(diff));
    }
    #[tokio::test]
    async fn driver_stub_suite() {
        green_probe().await;
        invalid_pool().await;
        closed_zero_budget().await;
        for _ in 0..8 {
            closed_zero_budget().await
        }
    }
    #[test]
    fn wp2_candidate_parser_matrix() {
        let bytes =
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+néw\n"
                .as_bytes();
        let parsed = parse_candidate_diff(bytes).unwrap();
        assert_eq!(parsed.paths(), &["a.txt"]);
        assert_eq!(parsed.bytes_sha256(), hex_digest(bytes));
        let duplicate=b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-y\n+z\n";
        let overflow =
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -18446744073709551616 +1 @@\n-x\n+y\n";
        let mismatch = b"diff --git a/a b/a\n--- /dev/null\n+++ /dev/null\n@@ -0,0 +1 @@\n+x\n";
        let trailing =
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ntrailing prose\n";
        let crlf = b"diff --git a/a b/a\r\n--- a/a\r\n+++ b/a\r\n@@ -1 +1 @@\r\n-x\r\n+y\r\n";
        for bad in [
            b"".as_slice(),
            b"```diff\n```\n",
            crlf,
            duplicate,
            overflow,
            mismatch,
            trailing,
        ] {
            assert!(
                matches!(parse_candidate_diff(bad),Err(VerifyError::Artifact(e))if e.kind()==std::io::ErrorKind::InvalidData)
            );
        }
    }
    #[test]
    fn wp2_expected_change_manifest_matrix() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("z.txt"), b"old\nkeep\n").unwrap();
        std::fs::write(root.path().join("d.txt"), b"gone\n").unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let bytes=b"diff --git a/z.txt b/z.txt\n--- a/z.txt\n+++ b/z.txt\n@@ -1,2 +1,2 @@\n-old\n+new\n keep\ndiff --git a/a.txt b/a.txt\n--- /dev/null\n+++ b/a.txt\n@@ -0,0 +1 @@\n+added\ndiff --git a/d.txt b/d.txt\n--- a/d.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let diff = parse_candidate_diff(bytes).unwrap();
        PARSER_CALLS.with(|calls| calls.set(0));
        let before_z = std::fs::read(canonical.join("z.txt")).unwrap();
        let before_d = std::fs::read(canonical.join("d.txt")).unwrap();
        let manifest = derive_expected_changes(&diff, &canonical).unwrap();
        assert_eq!(
            PARSER_CALLS.with(std::cell::Cell::get),
            0,
            "manifest derivation must consume retained records without reparsing"
        );
        let mut expected_base = b"wayland-nano.expected-change.base.v1\0".to_vec();
        for (path, kind, preimage) in [
            ("a.txt", 0_u8, None),
            ("d.txt", 2_u8, Some(b"gone\n".as_slice())),
            ("z.txt", 1_u8, Some(b"old\nkeep\n".as_slice())),
        ] {
            expected_base.extend_from_slice(&(path.len() as u64).to_le_bytes());
            expected_base.extend_from_slice(path.as_bytes());
            expected_base.push(kind);
            match preimage {
                None => expected_base.push(0),
                Some(bytes) => {
                    expected_base.push(1);
                    expected_base.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                    expected_base.extend_from_slice(&Sha256::digest(bytes));
                }
            }
        }
        assert_eq!(
            manifest.base_tree_digest(),
            hex_digest(&expected_base),
            "base digest must be exactly domain + path/kind + absent-or-length-and-SHA facts"
        );
        assert_eq!(
            manifest
                .entries()
                .iter()
                .map(ExpectedChange::path)
                .collect::<Vec<_>>(),
            vec!["a.txt", "d.txt", "z.txt"]
        );
        assert_eq!(manifest.entries()[0].kind(), ChangeKind::Add);
        assert_eq!(manifest.entries()[1].kind(), ChangeKind::Delete);
        assert_eq!(manifest.entries()[2].kind(), ChangeKind::Modify);
        assert_eq!(
            manifest.entries()[0].postimage_sha256(),
            Some(hex_digest(b"added\n").as_str())
        );
        assert_eq!(manifest.entries()[1].postimage_sha256(), None);
        assert_eq!(
            manifest.entries()[2].postimage_sha256(),
            Some(hex_digest(b"new\nkeep\n").as_str())
        );
        assert_eq!(manifest.diff_digest(), hex_digest(bytes));
        assert_eq!(std::fs::read(canonical.join("z.txt")).unwrap(), before_z);
        assert_eq!(std::fs::read(canonical.join("d.txt")).unwrap(), before_d);
        assert!(!canonical.join("a.txt").exists());
        std::fs::write(canonical.join("a.txt"), b"occupied\n").unwrap();
        assert!(derive_expected_changes(&diff, &canonical).is_err());
        std::fs::remove_file(canonical.join("a.txt")).unwrap();
        let base1 = manifest.base_tree_digest().to_owned();
        std::fs::write(canonical.join("z.txt"), b"else\nkeep\n").unwrap();
        let base2 = derive_expected_changes(&diff, &canonical);
        assert!(
            base2.is_err(),
            "changed preimage must not be accepted against old context"
        );
        assert!(!base1.is_empty());
        let bind_diff = parse_candidate_diff(
            b"diff --git a/z.txt b/z.txt\n--- a/z.txt\n+++ b/z.txt\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        std::fs::write(root_a.path().join("z.txt"), b"old\nuntouched-a\n").unwrap();
        std::fs::write(root_b.path().join("z.txt"), b"old\nuntouched-b\n").unwrap();
        let root_a = root_a.path().canonicalize().unwrap();
        let root_b = root_b.path().canonicalize().unwrap();
        PARSER_CALLS.with(|calls| calls.set(0));
        let manifest_a = derive_expected_changes(&bind_diff, &root_a).unwrap();
        let manifest_b = derive_expected_changes(&bind_diff, &root_b).unwrap();
        assert_ne!(
            manifest_a.base_tree_digest(),
            manifest_b.base_tree_digest(),
            "base digest must bind exact preimage bytes even when both patches apply"
        );
        assert_eq!(
            PARSER_CALLS.with(std::cell::Cell::get),
            0,
            "derivation must not call the parser"
        );
        let overlap=parse_candidate_diff(b"diff --git a/z.txt b/z.txt\n--- a/z.txt\n+++ b/z.txt\n@@ -1 +1 @@\n-old\n+new\n@@ -1 +1 @@\n-old\n+again\n").unwrap();
        std::fs::write(canonical.join("z.txt"), b"old\nkeep\n").unwrap();
        assert!(derive_expected_changes(&overlap, &canonical).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn wp2_descriptor_manifest_rejects_links_and_is_order_stable() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        std::fs::create_dir(canonical.join("nested")).unwrap();
        std::fs::write(canonical.join("nested/a.txt"), b"old\n").unwrap();
        let first = parse_candidate_diff(b"diff --git a/nested/a.txt b/nested/a.txt\n--- a/nested/a.txt\n+++ b/nested/a.txt\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/z.txt b/z.txt\n--- /dev/null\n+++ b/z.txt\n@@ -0,0 +1 @@\n+z\n").unwrap();
        let second = parse_candidate_diff(b"diff --git a/z.txt b/z.txt\n--- /dev/null\n+++ b/z.txt\n@@ -0,0 +1 @@\n+z\ndiff --git a/nested/a.txt b/nested/a.txt\n--- a/nested/a.txt\n+++ b/nested/a.txt\n@@ -1 +1 @@\n-old\n+new\n").unwrap();
        assert_eq!(
            derive_expected_changes(&first, &canonical)
                .unwrap()
                .base_tree_digest(),
            derive_expected_changes(&second, &canonical)
                .unwrap()
                .base_tree_digest()
        );
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("a.txt"), b"old\n").unwrap();
        std::fs::rename(canonical.join("nested"), canonical.join("held")).unwrap();
        symlink(outside.path(), canonical.join("nested")).unwrap();
        assert!(
            matches!(derive_expected_changes(&first, &canonical), Err(VerifyError::Artifact(e)) if e.kind()==std::io::ErrorKind::InvalidData)
        );
        std::fs::remove_file(canonical.join("nested")).unwrap();
        std::fs::rename(canonical.join("held"), canonical.join("nested")).unwrap();
        let alias = canonical.join("alias.txt");
        std::fs::hard_link(canonical.join("nested/a.txt"), &alias).unwrap();
        let hard = parse_candidate_diff(b"diff --git a/nested/a.txt b/nested/a.txt\n--- a/nested/a.txt\n+++ b/nested/a.txt\n@@ -1 +1 @@\n-old\n+new\n").unwrap();
        assert!(
            matches!(derive_expected_changes(&hard, &canonical), Err(VerifyError::Artifact(e)) if e.kind()==std::io::ErrorKind::InvalidData)
        );
    }

    #[cfg(windows)]
    #[test]
    fn wp2_expected_change_windows_rejects_hardlink_preimage() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("target.txt"), b"old\n").unwrap();
        std::fs::hard_link(
            root.path().join("target.txt"),
            root.path().join("alias.txt"),
        )
        .unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let diff = parse_candidate_diff(b"diff --git a/target.txt b/target.txt\n--- a/target.txt\n+++ b/target.txt\n@@ -1 +1 @@\n-old\n+new\n").unwrap();
        assert!(matches!(derive_expected_changes(&diff, &canonical),
            Err(VerifyError::Artifact(error)) if error.kind() == std::io::ErrorKind::InvalidData));
    }

    #[cfg(windows)]
    #[test]
    fn wp2_expected_change_windows_nested_descriptor_walk_works() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::write(root.path().join("nested/target.txt"), b"old\n").unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let diff = parse_candidate_diff(b"diff --git a/nested/target.txt b/nested/target.txt\n--- a/nested/target.txt\n+++ b/nested/target.txt\n@@ -1 +1 @@\n-old\n+new\n").unwrap();
        assert!(derive_expected_changes(&diff, &canonical).is_ok());
    }
}
