//! Filesystem tools: read/write/edit with policy enforcement.
//!
//! Rules (architecture invariants):
//! - every access checks the policy first — read against ReadDenyMatcher,
//!   write against can_write_path_with_cwd; denial is typed, never silent;
//! - sensitive-file defaults deny .env / private keys / credential stores
//!   unless the caller explicitly overrides;
//! - reads are bounded (line offset + line/byte caps) so a huge file cannot
//!   flood the context;
//! - edits are exact replacements; zero matches or multiple unexpected
//!   matches are typed errors, never guesses.

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::FileSystemSandboxPolicy;
use nano_core::policy_engine::ReadDenyMatcher;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("read denied by policy: {0}")]
    ReadDenied(String),
    #[error("write denied by policy: {0}")]
    WriteDenied(String),
    #[error("sensitive file denied (explicit override required): {0}")]
    SensitiveDenied(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("edit failed: {0}")]
    Edit(String),
}

/// Read bounds matching the harness contract.
#[derive(Debug, Clone)]
pub struct ReadBounds {
    pub line_offset: Option<usize>,
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl Default for ReadBounds {
    fn default() -> Self {
        Self {
            line_offset: None,
            max_lines: 1000,
            max_bytes: 100 * 1024,
        }
    }
}

const SENSITIVE_BASENAMES: &[&str] = &[".env", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];
const SENSITIVE_EXTENSIONS: &[&str] = &[".pem", ".key", ".pfx", ".p12", ".kdbx"];

pub fn is_sensitive_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if SENSITIVE_BASENAMES.iter().any(|b| name.eq_ignore_ascii_case(b)) {
        return true;
    }
    if name.starts_with(".env.") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    SENSITIVE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

#[derive(Debug)]
pub struct FsTools {
    policy: FileSystemSandboxPolicy,
    cwd: PathBuf,
    allow_sensitive: bool,
}

impl FsTools {
    pub fn new(policy: FileSystemSandboxPolicy, cwd: &Path) -> Self {
        Self {
            policy,
            cwd: cwd.to_path_buf(),
            allow_sensitive: false,
        }
    }

    pub fn with_sensitive_override(mut self) -> Self {
        self.allow_sensitive = true;
        self
    }

    fn check_read(&self, path: &Path) -> Result<(), ToolError> {
        if !self.allow_sensitive && is_sensitive_path(path) {
            return Err(ToolError::SensitiveDenied(path.display().to_string()));
        }
        if let Some(matcher) = ReadDenyMatcher::new(&self.policy, &self.cwd) {
            if matcher.is_read_denied(path) {
                return Err(ToolError::ReadDenied(path.display().to_string()));
            }
        }
        Ok(())
    }

    fn check_write(&self, path: &Path) -> Result<(), ToolError> {
        if !self.allow_sensitive && is_sensitive_path(path) {
            return Err(ToolError::SensitiveDenied(path.display().to_string()));
        }
        if !self.policy.can_write_path_with_cwd(path, &self.cwd) {
            return Err(ToolError::WriteDenied(path.display().to_string()));
        }
        Ok(())
    }

    /// Reads a file within bounds. Returns (content, truncated).
    pub fn read_file(&self, path: &Path, bounds: &ReadBounds) -> Result<(String, bool), ToolError> {
        self.check_read(path)?;
        let raw = std::fs::read(path)?;
        let text_full = String::from_utf8_lossy(&raw);
        let (text, byte_truncated) = if text_full.len() > bounds.max_bytes {
            let mut cut = bounds.max_bytes.min(text_full.len());
            while cut > 0 && !text_full.is_char_boundary(cut) {
                cut -= 1;
            }
            (text_full[..cut].to_string(), true)
        } else {
            (text_full.into_owned(), false)
        };
        let text = text.as_str();
        let lines: Vec<&str> = text.lines().collect();
        let start = bounds.line_offset.unwrap_or(0).min(lines.len());
        let selected = &lines[start..];
        let (selected, line_truncated) = if selected.len() > bounds.max_lines {
            (&selected[..bounds.max_lines], true)
        } else {
            (selected, false)
        };
        Ok((selected.join("\n"), byte_truncated || line_truncated))
    }

    /// Writes a file (creating parents). Policy-checked.
    pub fn write_file(&self, path: &Path, content: &str) -> Result<(), ToolError> {
        self.check_write(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Exact-replacement edit. Fails typed on zero or multiple matches
    /// (multiple matches require replace_all).
    pub fn edit_file(
        &self,
        path: &Path,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<usize, ToolError> {
        self.check_write(path)?;
        let content = std::fs::read_to_string(path)?;
        let matches: Vec<_> = content.match_indices(old_string).collect();
        match (matches.len(), replace_all) {
            (0, _) => Err(ToolError::Edit(format!(
                "old_string not found in {}",
                path.display()
            ))),
            (1, _) => {
                let updated = content.replacen(old_string, new_string, 1);
                std::fs::write(path, updated)?;
                Ok(1)
            }
            (n, true) => {
                let updated = content.replace(old_string, new_string);
                std::fs::write(path, updated)?;
                Ok(n)
            }
            (n, false) => Err(ToolError::Edit(format!(
                "old_string is ambiguous ({n} matches) — pass replace_all or add context"
            ))),
        }
    }
}

/// Convenience: resolve a possibly-relative path against the policy cwd.
pub fn resolve_against_cwd(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        AbsolutePathBuf::from_absolute_path(cwd)
            .map(|base| base.join(path).into_path_buf())
            .unwrap_or_else(|_| cwd.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_core::permissions::{
        FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry,
    };

    fn workspace_policy(workspace: &Path) -> FileSystemSandboxPolicy {
        FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: nano_core::permissions::FileSystemSpecialPath::Root,
                },
                FileSystemAccessMode::Read,
            ),
            FileSystemSandboxEntry::new(
                FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(workspace).unwrap(),
                },
                FileSystemAccessMode::Write,
            ),
        ])
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        (tmp, ws)
    }

    #[test]
    fn read_write_edit_round_trip_in_workspace() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("note.txt");
        tools.write_file(&file, "hello world").unwrap();
        let (content, truncated) = tools.read_file(&file, &ReadBounds::default()).unwrap();
        assert_eq!(content, "hello world");
        assert!(!truncated);
        let edits = tools.edit_file(&file, "world", "workspace", false).unwrap();
        assert_eq!(edits, 1);
        let (content, _) = tools.read_file(&file, &ReadBounds::default()).unwrap();
        assert_eq!(content, "hello workspace");
    }

    #[test]
    fn write_outside_workspace_is_denied() {
        let (tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let outside = tmp.path().join("outside.txt");
        let err = tools.write_file(&outside, "x").expect_err("must deny");
        assert!(matches!(err, ToolError::WriteDenied(_)));
        assert!(!outside.exists());
    }

    #[test]
    fn sensitive_files_denied_without_override() {
        let (_tmp, ws) = fixture();
        let env_file = ws.join(".env");
        std::fs::write(&env_file, "SECRET=1").unwrap();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let err = tools
            .read_file(&env_file, &ReadBounds::default())
            .expect_err("must deny");
        assert!(matches!(err, ToolError::SensitiveDenied(_)));

        let tools_open = FsTools::new(workspace_policy(&ws), &ws).with_sensitive_override();
        let (content, _) = tools_open
            .read_file(&env_file, &ReadBounds::default())
            .expect("override allows");
        assert_eq!(content, "SECRET=1");
    }

    #[test]
    fn sensitive_detection_covers_key_material() {
        assert!(is_sensitive_path(Path::new("/repo/.env")));
        assert!(is_sensitive_path(Path::new("/repo/.env.production")));
        assert!(is_sensitive_path(Path::new("/home/u/.ssh/id_rsa")));
        assert!(is_sensitive_path(Path::new("/certs/server.pem")));
        assert!(!is_sensitive_path(Path::new("/repo/notes.txt")));
        assert!(!is_sensitive_path(Path::new("/repo/environment.rs")));
    }

    #[test]
    fn read_bounds_truncate_lines_and_bytes() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("big.txt");
        let body = (1..=2000).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        tools.write_file(&file, &body).unwrap();
        let (content, truncated) = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(1995),
                    max_lines: 10,
                    max_bytes: 100 * 1024,
                },
            )
            .unwrap();
        assert!(content.contains("line 1996"));
        assert!(!truncated);
        let (content, truncated) = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: None,
                    max_lines: 5,
                    max_bytes: 1024,
                },
            )
            .unwrap();
        assert_eq!(content.lines().count(), 5);
        assert!(truncated);
    }

    #[test]
    fn edit_zero_and_ambiguous_matches_are_typed() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("dup.txt");
        tools.write_file(&file, "foo bar foo").unwrap();
        let err = tools
            .edit_file(&file, "missing", "x", false)
            .expect_err("zero match");
        assert!(matches!(err, ToolError::Edit(_)));
        let err = tools.edit_file(&file, "foo", "x", false).expect_err("ambiguous");
        assert!(matches!(err, ToolError::Edit(_)));
        let n = tools.edit_file(&file, "foo", "x", true).unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn read_denied_path_is_blocked_by_matcher() {
        let (_tmp, ws) = fixture();
        let secret_dir = ws.join("secrets");
        std::fs::create_dir_all(&secret_dir).unwrap();
        let secret = secret_dir.join("data.txt");
        std::fs::write(&secret, "hidden").unwrap();
        let mut entries = vec![FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: nano_core::permissions::FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        )];
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&secret_dir).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ));
        let policy = FileSystemSandboxPolicy::restricted(entries);
        let tools = FsTools::new(policy, &ws);
        let err = tools
            .read_file(&secret, &ReadBounds::default())
            .expect_err("must deny");
        assert!(matches!(err, ToolError::ReadDenied(_)));
    }
}
