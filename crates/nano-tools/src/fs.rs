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

// Single definition lives in nano-core (moved for the P4 repomap, design
// §5.3); re-exported here so existing callers keep compiling unchanged.
pub use nano_core::sensitive_path::is_sensitive_path;

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
    #[error("invalid read window: {0}")]
    InvalidWindow(String),
    /// P4 (design §5.5): malformed tool arguments that are not a read
    /// window — e.g. an unparseable `path_glob` for `repo_map`. Maps to
    /// `NanoErrorKind::InvalidParams` (model-correctable).
    #[error("invalid params: {0}")]
    InvalidParams(String),
}

/// Read bounds matching the harness contract.
///
/// Paging (C3): `line_offset` is 0-based. `byte_offset_in_line` resumes
/// INSIDE line `line_offset` after an oversized-line hard cut; it is a byte
/// offset into that line's text (terminator excluded) and is only meaningful
/// together with `line_offset`.
#[derive(Debug, Clone)]
pub struct ReadBounds {
    pub line_offset: Option<usize>,
    pub max_lines: usize,
    /// Per-page byte cap applied to the SELECTED line window (never a
    /// window-position constraint).
    pub max_bytes: usize,
    pub byte_offset_in_line: Option<usize>,
}

impl Default for ReadBounds {
    fn default() -> Self {
        Self {
            line_offset: None,
            max_lines: 1000,
            max_bytes: 100 * 1024,
            byte_offset_in_line: None,
        }
    }
}

/// Advisory freshness token (C3 §2.3): (mtime secs, len). ADVISORY ONLY —
/// a coarse-mtime filesystem plus a same-length in-place edit defeats it.
/// The fs policy layer is the security boundary; this token protects against
/// ordinary edit-between-reads drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileToken {
    pub mtime_secs: i64,
    pub len: u64,
}

impl FileToken {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let mtime_secs = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(-1);
        Self {
            mtime_secs,
            len: metadata.len(),
        }
    }
}

impl std::fmt::Display for FileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "m:{}-l:{}", self.mtime_secs, self.len)
    }
}

impl std::str::FromStr for FileToken {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s.strip_prefix("m:").ok_or(())?;
        let (secs, len) = rest.split_once("-l:").ok_or(())?;
        Ok(Self {
            mtime_secs: secs.parse().map_err(|_| ())?,
            len: len.parse().map_err(|_| ())?,
        })
    }
}

/// Where a returned page leaves the reader (C3 §2.3). 0-based throughout:
/// `next_line_offset = line_offset + lines_returned`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageCursor {
    /// More whole lines follow; resume with `line_offset = next_line_offset`.
    Lines { next_line_offset: usize },
    /// A single line exceeded the remaining per-page byte budget and was
    /// hard-cut (UTF-8 boundary-safe); resume with the SAME `line_offset`
    /// plus `byte_offset_in_line`.
    LineTruncated {
        line_offset: usize,
        byte_offset_in_line: usize,
    },
    /// The read reached EOF. For an out-of-range request this is the typed
    /// out-of-range signal with the REAL total; for an in-window read it
    /// simply means "not truncated".
    Eof { total_lines: usize },
}

/// One bounded page of a file (C3 §2.5 — the read_file return type).
#[derive(Debug, Clone)]
pub struct ReadPage {
    pub content: String,
    pub cursor: PageCursor,
    /// Real file size from metadata (always exact).
    pub total_bytes: u64,
    /// Real line count, `Some` ONLY when this read reached EOF — never an
    /// estimate.
    pub total_lines: Option<usize>,
    pub file_token: FileToken,
}

impl ReadPage {
    /// True when content was left unread (a footer must be appended).
    pub fn is_truncated(&self) -> bool {
        !matches!(self.cursor, PageCursor::Eof { .. })
    }
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

    /// Reads one bounded page of a file (C3: line-slice BEFORE byte-cap —
    /// the byte cap is a per-page cap, never a window-position constraint).
    ///
    /// Streaming windowed reader: peak memory is one page plus a bounded
    /// probe, never the whole file — the line skip consumes fixed buffers
    /// and every line read is take()-limited to the remaining byte budget.
    /// Line-skip is O(offset) per page (accepted v1 cost; a byte_offset
    /// fast-path cursor is the designated escape hatch).
    pub fn read_file(&self, path: &Path, bounds: &ReadBounds) -> Result<ReadPage, ToolError> {
        use std::io::{BufRead, Read};
        self.check_read(path)?;
        let file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let total_bytes = metadata.len();
        let file_token = FileToken::from_metadata(&metadata);
        let line_offset = bounds.line_offset.unwrap_or(0);
        let byte_offset_in_line = bounds.byte_offset_in_line.unwrap_or(0);
        let mut reader = std::io::BufReader::new(file);

        // Skip to the window start by discard, in fixed buffers (a line can
        // be gigabytes long; it must never be held whole). Reaching EOF here
        // proves the offset out of range and yields the REAL total — a typed
        // EOF page, never a fabricated "last page".
        let mut skipped = 0usize;
        let mut partial = false; // consumed bytes not yet terminated by '\n'
        while skipped < line_offset {
            let (found, consumed, hit_eof, ends_newline) = {
                let buf = reader.fill_buf()?;
                if buf.is_empty() {
                    (0usize, 0usize, true, false)
                } else {
                    let need = line_offset - skipped;
                    let mut found = 0usize;
                    let mut consumed = buf.len();
                    for (i, &b) in buf.iter().enumerate() {
                        if b == b'\n' {
                            found += 1;
                            if found == need {
                                consumed = i + 1;
                                break;
                            }
                        }
                    }
                    (found, consumed, false, buf[consumed - 1] == b'\n')
                }
            };
            if hit_eof {
                // A final line without a terminator still counts as a line.
                let total = skipped + usize::from(partial);
                return Ok(ReadPage {
                    content: String::new(),
                    cursor: PageCursor::Eof { total_lines: total },
                    total_bytes,
                    total_lines: Some(total),
                    file_token,
                });
            }
            reader.consume(consumed);
            skipped += found;
            partial = !ends_newline;
        }

        let mut content = String::new();
        let mut lines_returned = 0usize;
        let cursor;
        let mut total_lines = None;
        let mut raw: Vec<u8> = Vec::new();
        loop {
            if lines_returned >= bounds.max_lines || content.len() >= bounds.max_bytes {
                // Window closed: one buffered-byte probe decides
                // has_more / total_lines.
                if reader.fill_buf()?.is_empty() {
                    let total = line_offset + lines_returned;
                    total_lines = Some(total);
                    cursor = PageCursor::Eof { total_lines: total };
                } else {
                    cursor = PageCursor::Lines {
                        next_line_offset: line_offset + lines_returned,
                    };
                }
                break;
            }
            let line_index = line_offset + lines_returned;
            // Intra-line byte resume applies only to the first delivered line.
            let start = if lines_returned == 0 {
                byte_offset_in_line
            } else {
                0
            };
            let sep = usize::from(!content.is_empty());
            // Loop head guarantees content.len() < max_bytes, so no underflow.
            let avail = bounds.max_bytes - content.len() - sep;
            // Probe at most start + avail + 2 bytes: the deliverable fragment
            // plus its terminator plus one byte of continuation evidence.
            let limit = (start + avail + 2) as u64;
            raw.clear();
            let n = reader.by_ref().take(limit).read_until(b'\n', &mut raw)?;
            if n == 0 {
                let total = line_offset + lines_returned;
                total_lines = Some(total);
                cursor = PageCursor::Eof { total_lines: total };
                break;
            }
            let complete = raw.last() == Some(&b'\n') || raw.len() < limit as usize;
            if !complete {
                // The take() limit can split a multibyte char: drop only a
                // TRAILING incomplete char (genuine invalid bytes stay and
                // get the legacy lossy replacement below).
                if let Err(e) = std::str::from_utf8(&raw) {
                    if e.error_len().is_none() {
                        raw.truncate(e.valid_up_to());
                    }
                }
            }
            let line_text = match raw.strip_suffix(b"\n") {
                Some(s) => {
                    let s = s.strip_suffix(b"\r").unwrap_or(s);
                    String::from_utf8_lossy(s)
                }
                None => String::from_utf8_lossy(&raw),
            };
            let line_text = line_text.as_ref();
            if start > line_text.len() {
                return Err(ToolError::InvalidWindow(format!(
                    "byte_offset_in_line {start} is past the end of line {line_index}"
                )));
            }
            let Some(frag) = line_text.get(start..) else {
                return Err(ToolError::InvalidWindow(format!(
                    "byte_offset_in_line {start} is not a char boundary inside line {line_index}"
                )));
            };
            if complete && frag.len() <= avail {
                if sep == 1 {
                    content.push('\n');
                }
                content.push_str(frag);
                lines_returned += 1;
                continue;
            }
            // Oversized for the remaining budget: hard-cut, UTF-8
            // boundary-safe, with an intra-line byte cursor for resume.
            let mut cut = avail.min(frag.len());
            while cut > 0 && !frag.is_char_boundary(cut) {
                cut -= 1;
            }
            if cut == 0 && !content.is_empty() {
                // Page filled exactly at a line boundary: clean line cursor.
                cursor = PageCursor::Lines {
                    next_line_offset: line_index,
                };
                break;
            }
            if cut == 0 {
                // Progress guarantee (max_bytes smaller than one char):
                // always deliver at least one character.
                cut = frag.chars().next().map(char::len_utf8).unwrap_or(0);
            }
            if sep == 1 {
                content.push('\n');
            }
            content.push_str(&frag[..cut]);
            cursor = PageCursor::LineTruncated {
                line_offset: line_index,
                byte_offset_in_line: start + cut,
            };
            break;
        }
        Ok(ReadPage {
            content,
            cursor,
            total_bytes,
            total_lines,
            file_token,
        })
    }

    /// Re-stats the file for the advisory freshness token (C3 §2.3). The
    /// executor compares this against a model-supplied token before paging.
    pub fn stat_token(&self, path: &Path) -> Result<FileToken, ToolError> {
        self.check_read(path)?;
        Ok(FileToken::from_metadata(&std::fs::metadata(path)?))
    }

    /// Writes a file (creating parents). Policy-checked.
    pub fn write_file(&self, path: &Path, content: &str) -> Result<(), ToolError> {
        self.write_file_with_diff(path, content).map(|_| ())
    }

    /// C10 §6: write_file that additionally returns the before/after text
    /// pair for the human-facing diff. The read-before-overwrite runs AFTER
    /// the policy check (authorization), immediately before the mutation; a
    /// failed or absent read is NON-FATAL — `old_text: None` covers both
    /// "new file" (whole-file add) and "unreadable prior content" (the
    /// caller cannot distinguish, and must not fail the write over it).
    pub fn write_file_with_diff(&self, path: &Path, content: &str) -> Result<WriteDiff, ToolError> {
        self.check_write(path)?;
        let old_text = std::fs::read_to_string(path).ok();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(WriteDiff {
            replacements: 0,
            old_text,
            new_text: content.to_string(),
        })
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
        self.edit_file_with_diff(path, old_string, new_string, replace_all)
            .map(|diff| diff.replacements)
    }

    /// C10 §6: edit_file that additionally returns the whole-file
    /// before/after text pair (the one structured diff representation).
    pub fn edit_file_with_diff(
        &self,
        path: &Path,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<WriteDiff, ToolError> {
        self.check_write(path)?;
        let content = std::fs::read_to_string(path)?;
        let matches: Vec<_> = content.match_indices(old_string).collect();
        let (replacements, updated) = match (matches.len(), replace_all) {
            (0, _) => {
                return Err(ToolError::Edit(format!(
                    "old_string not found in {}",
                    path.display()
                )));
            }
            (1, _) => (1, content.replacen(old_string, new_string, 1)),
            (n, true) => (n, content.replace(old_string, new_string)),
            (n, false) => {
                return Err(ToolError::Edit(format!(
                    "old_string is ambiguous ({n} matches) — pass replace_all or add context"
                )));
            }
        };
        std::fs::write(path, &updated)?;
        Ok(WriteDiff {
            replacements,
            old_text: Some(content),
            new_text: updated,
        })
    }
}

/// The before/after text pair a mutating fs call produces (C10 §6).
/// `replacements` is the edit count for `edit_file` (unused by `write_file`,
/// which reports 0 — a write replaces the whole file by definition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteDiff {
    pub replacements: usize,
    /// None = no readable prior content (whole-file add, or the
    /// non-fatal read-before-overwrite failure case).
    pub old_text: Option<String>,
    pub new_text: String,
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
    use nano_core::permissions::{FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry};

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
        let page = tools.read_file(&file, &ReadBounds::default()).unwrap();
        assert_eq!(page.content, "hello world");
        assert!(!page.is_truncated());
        let edits = tools.edit_file(&file, "world", "workspace", false).unwrap();
        assert_eq!(edits, 1);
        let page = tools.read_file(&file, &ReadBounds::default()).unwrap();
        assert_eq!(page.content, "hello workspace");
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
        let page = tools_open
            .read_file(&env_file, &ReadBounds::default())
            .expect("override allows");
        assert_eq!(page.content, "SECRET=1");
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
        let body = (1..=2000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        tools.write_file(&file, &body).unwrap();
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(1995),
                    max_lines: 10,
                    max_bytes: 100 * 1024,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(page.content.contains("line 1996"));
        assert!(!page.is_truncated());
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: None,
                    max_lines: 5,
                    max_bytes: 1024,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.content.lines().count(), 5);
        assert!(page.is_truncated());
        assert_eq!(
            page.cursor,
            PageCursor::Lines {
                next_line_offset: 5
            }
        );
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
        let err = tools
            .edit_file(&file, "foo", "x", false)
            .expect_err("ambiguous");
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

    // --- C3 paged-read battery ------------------------------------------------

    fn dense_file(workspace: &Path, name: &str, lines: usize) -> PathBuf {
        // ~100 bytes per line so line 1500 sits past the legacy 100 KB cap.
        let file = workspace.join(name);
        let body = (0..lines)
            .map(|i| format!("line-{i:05}-{}", "x".repeat(90)))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&file, &body).unwrap();
        file
    }

    /// R1 regression proof: line-slice BEFORE byte-cap. A ~300 KB file read
    /// at a line_offset past the 100 KB mark must return the correct lines —
    /// the pre-C3 code applied the byte cap to the whole file first and
    /// could never address content past ~100 KB.
    #[test]
    fn line_slice_precedes_byte_cap_orders_window_first() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = dense_file(&ws, "huge.txt", 3000);
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(1500),
                    max_lines: 3,
                    ..Default::default()
                },
            )
            .unwrap();
        let lines: Vec<&str> = page.content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("line-01500-"), "got: {}", lines[0]);
        assert!(lines[2].starts_with("line-01502-"), "got: {}", lines[2]);
        assert_eq!(
            page.cursor,
            PageCursor::Lines {
                next_line_offset: 1503
            }
        );
        assert_eq!(page.total_lines, None, "no EOF reached: no total");
    }

    /// Byte cap cuts are UTF-8 boundary-safe on multibyte content.
    #[test]
    fn byte_cap_cut_is_utf8_boundary_safe() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("utf8.txt");
        std::fs::write(&file, "éééééééééé").unwrap(); // 10 × 2 bytes
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    max_bytes: 7,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.content, "ééé"); // 6 bytes: 7 would split a char
        assert_eq!(
            page.cursor,
            PageCursor::LineTruncated {
                line_offset: 0,
                byte_offset_in_line: 6
            }
        );
        // Resume walks the rest of the line.
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(0),
                    byte_offset_in_line: Some(6),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.content, "ééééééé");
        assert_eq!(page.cursor, PageCursor::Eof { total_lines: 1 });
    }

    /// 0-based cursor math: next = offset + lines_returned, no skipped or
    /// duplicated boundary line across pages.
    #[test]
    fn cursor_math_has_no_gap_or_overlap() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("cursor.txt");
        let body = (0..10)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&file, &body).unwrap();
        let page1 = tools
            .read_file(
                &file,
                &ReadBounds {
                    max_lines: 4,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page1.content, "l0\nl1\nl2\nl3");
        let PageCursor::Lines { next_line_offset } = page1.cursor else {
            panic!("expected line cursor: {:?}", page1.cursor);
        };
        assert_eq!(next_line_offset, 4);
        let page2 = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(next_line_offset),
                    max_lines: 4,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page2.content, "l4\nl5\nl6\nl7");
    }

    /// Paged continuity: a >100 KB file read in sequential pages via the
    /// returned cursors concatenates to the original file region.
    #[test]
    fn paged_read_continuity_over_100kb() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = dense_file(&ws, "paged.txt", 3000);
        let original = std::fs::read_to_string(&file).unwrap();
        assert!(original.len() > 2 * 100 * 1024);
        let mut offset = 0usize;
        let mut assembled = String::new();
        let mut pages = 0;
        loop {
            let page = tools
                .read_file(
                    &file,
                    &ReadBounds {
                        line_offset: Some(offset),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(
                page.content.len() <= 100 * 1024,
                "page exceeds the per-page cap"
            );
            pages += 1;
            match page.cursor {
                PageCursor::Lines { next_line_offset } => {
                    if !assembled.is_empty() {
                        assembled.push('\n');
                    }
                    assembled.push_str(&page.content);
                    offset = next_line_offset;
                }
                PageCursor::Eof { total_lines } => {
                    if !page.content.is_empty() {
                        if !assembled.is_empty() {
                            assembled.push('\n');
                        }
                        assembled.push_str(&page.content);
                    }
                    assert_eq!(total_lines, 3000);
                    assert_eq!(page.total_lines, Some(3000));
                    break;
                }
                PageCursor::LineTruncated { .. } => panic!("no oversized lines here"),
            }
        }
        assert!(pages >= 3, "expected multiple pages, got {pages}");
        assert_eq!(assembled, original);
    }

    /// Oversized single line: hard-cut at the byte budget, then byte-resume
    /// walks the line to its end and continues with the line cursor — no
    /// gap, no overlap.
    #[test]
    fn oversized_line_hard_cut_and_byte_resume() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("long-line.txt");
        let long_line = "a".repeat(200 * 1024);
        std::fs::write(&file, format!("{long_line}\nnext-line")).unwrap();
        let page1 = tools.read_file(&file, &ReadBounds::default()).unwrap();
        assert_eq!(page1.content.len(), 100 * 1024);
        assert!(page1.content.bytes().all(|b| b == b'a'));
        let PageCursor::LineTruncated {
            line_offset,
            byte_offset_in_line,
        } = page1.cursor
        else {
            panic!("expected line_truncated cursor: {:?}", page1.cursor);
        };
        assert_eq!(line_offset, 0);
        assert_eq!(byte_offset_in_line, 100 * 1024);

        let page2 = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(line_offset),
                    byte_offset_in_line: Some(byte_offset_in_line),
                    ..Default::default()
                },
            )
            .unwrap();
        // The rest of the giant line is exactly one more page; the budget is
        // then exhausted, so the following line arrives via the LINE cursor.
        assert_eq!(page2.content, "a".repeat(100 * 1024));
        assert_eq!(
            page2.cursor,
            PageCursor::Lines {
                next_line_offset: 1
            }
        );
        let page3 = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page3.content, "next-line");
        assert_eq!(page3.cursor, PageCursor::Eof { total_lines: 2 });
        // Concatenation == original, no gap/overlap (page1+page2 are
        // fragments of the SAME line — no newline between them).
        assert_eq!(
            format!("{}{}\n{}", page1.content, page2.content, page3.content),
            format!("{long_line}\nnext-line")
        );
    }

    /// Out-of-range offset: typed EOF result with the REAL total, empty
    /// content, never a fabricated last page.
    #[test]
    fn out_of_range_offset_is_typed_eof() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("short.txt");
        let body = (0..100)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&file, &body).unwrap();
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(150),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.content, "");
        assert_eq!(page.cursor, PageCursor::Eof { total_lines: 100 });
        assert_eq!(page.total_lines, Some(100));
    }

    /// Freshness token: issued per page, detects an edit between reads via
    /// stat (length change — the case the advisory token CAN detect).
    #[test]
    fn freshness_token_detects_edit_between_reads() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("fresh.txt");
        std::fs::write(&file, "one\ntwo\n").unwrap();
        let page = tools.read_file(&file, &ReadBounds::default()).unwrap();
        let issued = page.file_token;
        // Round-trips through the footer string form.
        let parsed: FileToken = issued.to_string().parse().unwrap();
        assert_eq!(parsed, issued);
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        let current = tools.stat_token(&file).unwrap();
        assert_ne!(current, issued, "edit must invalidate the token");
        assert_eq!(current.len, "one\ntwo\nthree\n".len() as u64);
    }

    /// Back-compat: default single-page reads of small files are
    /// byte-identical to the pre-C3 output (trailing newline dropped, no
    /// footer signal).
    #[test]
    fn small_file_default_read_is_byte_identical_to_legacy() {
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("small.txt");
        std::fs::write(&file, "a\nb\nc\n").unwrap();
        let page = tools.read_file(&file, &ReadBounds::default()).unwrap();
        assert_eq!(page.content, "a\nb\nc");
        assert!(!page.is_truncated());
        let empty = ws.join("empty.txt");
        std::fs::write(&empty, "").unwrap();
        let page = tools.read_file(&empty, &ReadBounds::default()).unwrap();
        assert_eq!(page.content, "");
        assert!(!page.is_truncated());
    }

    /// Memory bound: a 1 GB sparse file read deep into the file completes
    /// without ever holding the file (or one giant line) in memory. Ignored
    /// by default (slow on some CI filesystems); run explicitly.
    #[test]
    #[ignore = "slow/1GB fixture: run explicitly"]
    fn huge_sparse_file_reads_with_bounded_memory() {
        use std::io::{Seek, Write};
        let (_tmp, ws) = fixture();
        let tools = FsTools::new(workspace_policy(&ws), &ws);
        let file = ws.join("sparse.bin");
        {
            let mut f = std::fs::File::create(&file).unwrap();
            // One short line every MiB so the skip phase has newlines to
            // count, in a 1 GiB sparse file.
            let mut pos = 0u64;
            while pos < (1 << 30) {
                f.seek(std::io::SeekFrom::Start(pos)).unwrap();
                f.write_all(b"mark\n").unwrap();
                pos += 1 << 20;
            }
        }
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(500),
                    max_lines: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        // Line 500 is a ~1 MiB sparse-zero line: the page hard-cuts it at
        // the byte budget (bounded memory) with an intra-line byte cursor.
        assert!(page.content.len() <= 100 * 1024);
        assert_eq!(
            page.cursor,
            PageCursor::LineTruncated {
                line_offset: 500,
                byte_offset_in_line: 100 * 1024,
            }
        );
        // And the byte resume at depth completes too.
        let page = tools
            .read_file(
                &file,
                &ReadBounds {
                    line_offset: Some(500),
                    byte_offset_in_line: Some(100 * 1024),
                    max_lines: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(page.content.len() <= 100 * 1024);
    }
}
