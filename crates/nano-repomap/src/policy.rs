//! Read-policy enforcement for the repomap walker (P4 design §5.3).
//!
//! Three checks, enforced at EVERY directory entry and BEFORE any read:
//!
//! 1. **Symlink/junction escape** — structural: the walker (`walk.rs`)
//!    never follows links, so a junction pointing outside the workspace
//!    is never traversed. Not a predicate; nothing here can forget it.
//! 2. **Denied-read** — `ReadDenyMatcher::is_read_denied` (fail-closed on
//!    malformed globs) against lexical + canonicalized candidates.
//! 3. **Sensitive paths** — `nano_core::sensitive_path::is_sensitive_path`
//!    (`.env*`, key basenames, `.pem/.key/.pfx/.p12/.kdbx`) — excluded
//!    even though extraction would only leak names; the path itself is
//!    signal.
//!
//! [`repomap_path_allowed`] is the one named seam composing checks 2+3 so
//! the panel can attack a single function. Skipped paths are COUNTED by
//! the walker (`IndexStats::skipped_denied`), never enumerated in results.

use std::path::Path;

use nano_core::permissions::FileSystemSandboxPolicy;
use nano_core::policy_engine::ReadDenyMatcher;
use nano_core::sensitive_path::is_sensitive_path;

/// The read-policy view the walker enforces. Built once per `RepoMap`
/// from the session's sandbox policy + cwd (the same construction the
/// fs/search tools use).
pub struct ReadPolicy {
    matcher: Option<ReadDenyMatcher>,
    allow_sensitive: bool,
}

impl ReadPolicy {
    pub fn new(policy: &FileSystemSandboxPolicy, cwd: &Path) -> Self {
        Self {
            matcher: ReadDenyMatcher::new(policy, cwd),
            allow_sensitive: false,
        }
    }

    /// Test/operator override mirroring `FsTools::with_sensitive_override`.
    /// The default posture denies sensitive paths.
    pub fn with_sensitive_override(mut self) -> Self {
        self.allow_sensitive = true;
        self
    }
}

/// The §5.3 seam: is `path` indexable under `policy`? Fail-closed —
/// a denied or sensitive path NEVER appears in the index or in results.
pub fn repomap_path_allowed(path: &Path, policy: &ReadPolicy) -> bool {
    if !policy.allow_sensitive && is_sensitive_path(path) {
        return false;
    }
    if let Some(matcher) = &policy.matcher
        && matcher.is_read_denied(path)
    {
        return false;
    }
    true
}

/// Convenience for callers that hold the raw sandbox policy rather than
/// a prebuilt [`ReadPolicy`] (query-time spot checks).
pub fn repomap_path_allowed_with(
    path: &Path,
    policy: &FileSystemSandboxPolicy,
    cwd: &Path,
    allow_sensitive: bool,
) -> bool {
    let mut p = ReadPolicy::new(policy, cwd);
    if allow_sensitive {
        p = p.with_sensitive_override();
    }
    repomap_path_allowed(path, &p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_core::abs::AbsolutePathBuf;
    use nano_core::permissions::{
        FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, FileSystemSpecialPath,
    };

    fn workspace_policy(ws: &Path) -> FileSystemSandboxPolicy {
        FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
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
    fn sensitive_path_denied_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let policy = ReadPolicy::new(&workspace_policy(ws), ws);
        assert!(!repomap_path_allowed(&ws.join(".env"), &policy));
        assert!(repomap_path_allowed(&ws.join("main.rs"), &policy));
    }

    #[test]
    fn denied_read_path_never_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        let secret = ws.join("secrets");
        std::fs::create_dir_all(&secret).unwrap();
        let mut entries = vec![FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        )];
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&secret).unwrap(),
            },
            FileSystemAccessMode::Deny,
        ));
        let policy = ReadPolicy::new(&FileSystemSandboxPolicy::restricted(entries), ws);
        assert!(!repomap_path_allowed(&secret.join("hidden.rs"), &policy));
        assert!(!repomap_path_allowed(&secret, &policy));
        assert!(repomap_path_allowed(&ws.join("visible.rs"), &policy));
    }
}
