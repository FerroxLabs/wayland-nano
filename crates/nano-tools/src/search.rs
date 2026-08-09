//! Search tools: glob file matching and content search, policy-filtered.
//!
//! Rules:
//! - results pass through the same read policy as fs tools (deny-read and
//!   sensitive defaults apply before any path reaches the caller);
//! - results are bounded (max results) and sorted for determinism;
//! - searches never follow directory junctions/symlinks (escape-safe).

use crate::fs::{ReadBounds, ToolError, is_sensitive_path};
use globset::GlobBuilder;
use nano_core::permissions::FileSystemSandboxPolicy;
use nano_core::policy_engine::ReadDenyMatcher;
use std::path::Path;
use std::path::PathBuf;

pub struct SearchTools {
    policy: FileSystemSandboxPolicy,
    cwd: PathBuf,
    allow_sensitive: bool,
}

impl SearchTools {
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

    fn path_allowed(&self, path: &Path) -> bool {
        if !self.allow_sensitive && is_sensitive_path(path) {
            return false;
        }
        if let Some(matcher) = ReadDenyMatcher::new(&self.policy, &self.cwd) {
            if matcher.is_read_denied(path) {
                return false;
            }
        }
        true
    }

    /// Glob for files under `root` matching `pattern` (git-style glob).
    /// Sorted, bounded, policy-filtered. Does not follow links.
    pub fn glob_files(
        &self,
        root: &Path,
        pattern: &str,
        max_results: usize,
    ) -> Result<Vec<PathBuf>, ToolError> {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .allow_unclosed_class(true)
            .build()
            .map_err(|e| ToolError::Edit(format!("invalid glob pattern `{pattern}`: {e}")))?;
        let matcher = glob.compile_matcher();

        let mut results = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        let mut seen = std::collections::HashSet::new();
        while let Some(dir) = stack.pop() {
            if results.len() >= max_results {
                break;
            }
            let canonical = dunce::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
            if !seen.insert(canonical) {
                continue; // junction/symlink cycle guard
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if results.len() >= max_results {
                    break;
                }
                let path = entry.path();
                let Ok(ft) = entry.file_type() else {
                    continue;
                };
                if ft.is_symlink() {
                    continue; // never follow links
                }
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !self.path_allowed(&path) {
                    continue;
                }
                if matcher.is_match(&path) {
                    results.push(path);
                }
            }
        }
        results.sort();
        results.truncate(max_results);
        Ok(results)
    }

    /// Regex content search over files under `root`. Returns
    /// (path, line_number, line) matches, bounded.
    pub fn search_content(
        &self,
        root: &Path,
        pattern: &str,
        max_results: usize,
    ) -> Result<Vec<(PathBuf, usize, String)>, ToolError> {
        let regex = regex::Regex::new(pattern)
            .map_err(|e| ToolError::Edit(format!("invalid regex `{pattern}`: {e}")))?;
        let mut results = Vec::new();
        let files = self.glob_files(root, "**/*", max_results.saturating_mul(8))?;
        for file in files {
            if results.len() >= max_results {
                break;
            }
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (index, line) in content.lines().enumerate() {
                if results.len() >= max_results {
                    break;
                }
                if regex.is_match(line) {
                    results.push((file.clone(), index + 1, line.to_string()));
                }
            }
        }
        Ok(results)
    }
}

/// Re-export read bounds for callers composing search + read flows.
pub type Bounds = ReadBounds;

#[cfg(test)]
mod tests {
    use super::*;
    use nano_core::abs::AbsolutePathBuf;
    use nano_core::permissions::{
        FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry,
    };

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/main.rs"), "fn main() {}\n// TODO: wire\n").unwrap();
        std::fs::write(ws.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        std::fs::write(ws.join("README.md"), "# project\nTODO: docs\n").unwrap();
        std::fs::write(ws.join(".env"), "SECRET=1\n").unwrap();
        (tmp, ws)
    }

    fn policy(ws: &Path) -> FileSystemSandboxPolicy {
        FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: nano_core::permissions::FileSystemSpecialPath::Root,
                },
                FileSystemAccessMode::Read,
            ),
            FileSystemSandboxEntry::new(
                FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(ws).unwrap(),
                },
                FileSystemAccessMode::Write,
            ),
        ])
    }

    #[test]
    fn glob_finds_matching_files_sorted() {
        let (_tmp, ws) = fixture();
        let tools = SearchTools::new(policy(&ws), &ws);
        let results = tools.glob_files(&ws, "**/*.rs", 100).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].ends_with("lib.rs"));
        assert!(results[1].ends_with("main.rs"));
    }

    #[test]
    fn glob_excludes_sensitive_files() {
        let (_tmp, ws) = fixture();
        let tools = SearchTools::new(policy(&ws), &ws);
        let results = tools.glob_files(&ws, "**/*", 100).unwrap();
        assert!(!results.iter().any(|p| p.ends_with(".env")));
        let open = SearchTools::new(policy(&ws), &ws).with_sensitive_override();
        let results = open.glob_files(&ws, "**/*", 100).unwrap();
        assert!(results.iter().any(|p| p.ends_with(".env")));
    }

    #[test]
    fn content_search_finds_lines_with_numbers() {
        let (_tmp, ws) = fixture();
        let tools = SearchTools::new(policy(&ws), &ws);
        let results = tools.search_content(&ws, "TODO", 100).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(p, line, _)| p.ends_with("main.rs") && *line == 2));
    }

    #[test]
    fn glob_bounds_results() {
        let (_tmp, ws) = fixture();
        let tools = SearchTools::new(policy(&ws), &ws);
        let results = tools.glob_files(&ws, "**/*.rs", 1).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn denied_dir_is_invisible_to_search() {
        let (_tmp, ws) = fixture();
        let secret = ws.join("secrets");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("hidden.rs"), "fn hidden() {}\n").unwrap();
        let mut entries = vec![FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: nano_core::permissions::FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        )];
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&secret).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ));
        let tools = SearchTools::new(FileSystemSandboxPolicy::restricted(entries), &ws);
        let results = tools.glob_files(&ws, "**/*.rs", 100).unwrap();
        assert!(!results.iter().any(|p| p.starts_with(&secret)));
    }
}
