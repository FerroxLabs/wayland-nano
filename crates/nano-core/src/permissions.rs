//! Permission model types: filesystem/network sandbox policy vocabulary.
//!
//! Provenance: extracted from Codex `codex-rs/protocol/src/permissions.rs` and
//! `codex-rs/protocol/src/models.rs` @ 646f7c0a, per B-VND-01 "EXTRACT-TYPES —
//! lift only what consumers name". This file carries the *type layer* and the
//! simple predicates. The *behavioral layer* (cwd root resolution, narrowing
//! analysis, ReadDenyMatcher) lands with its consumers
//! (`nano-sandbox::resolved_permissions` / `deny_read_resolver`).
//! Transformations: JsonSchema/TS/strum derives dropped (codex tooling);
//! serde shape preserved byte-for-byte (config compatibility).

use crate::abs::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkSandboxPolicy {
    #[default]
    Restricted,
    Enabled,
}

impl NetworkSandboxPolicy {
    pub fn is_enabled(self) -> bool {
        matches!(self, NetworkSandboxPolicy::Enabled)
    }
}

/// Access mode for a filesystem entry.
///
/// When two equally specific entries target the same path, we compare these by
/// conflict precedence rather than by capability breadth: `deny` beats
/// `write`, and `write` beats `read`.
#[derive(
    Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum FileSystemAccessMode {
    Read,
    Write,
    /// `none` is a legacy input alias retained temporarily for compatibility.
    #[serde(alias = "none")]
    Deny,
}

impl FileSystemAccessMode {
    pub fn can_read(self) -> bool {
        !matches!(self, FileSystemAccessMode::Deny)
    }

    pub fn can_write(self) -> bool {
        matches!(self, FileSystemAccessMode::Write)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileSystemSpecialPath {
    Root,
    Minimal,
    #[serde(alias = "current_working_directory")]
    ProjectRoots {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
    Tmpdir,
    SlashTmp,
    /// WARNING: `:special_path` tokens are part of config compatibility.
    /// Do not make older runtimes reject newly introduced tokens. Unknown
    /// values must stay representable so newer config degrades to
    /// warn-and-ignore instead of failing to load.
    Unknown {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
}

impl FileSystemSpecialPath {
    pub fn project_roots(subpath: Option<String>) -> Self {
        Self::ProjectRoots { subpath }
    }

    pub fn unknown(path: impl Into<String>, subpath: Option<String>) -> Self {
        Self::Unknown {
            path: path.into(),
            subpath,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileSystemSandboxEntry {
    pub path: FileSystemPath,
    pub access: FileSystemAccessMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_path_behavior: Option<FileSystemSandboxEntryMissingPathBehavior>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSystemSandboxEntryMissingPathBehavior {
    Skip,
}

impl FileSystemSandboxEntry {
    pub fn new(path: FileSystemPath, access: FileSystemAccessMode) -> Self {
        Self {
            path,
            access,
            missing_path_behavior: None,
        }
    }

    pub fn skip_missing_path(path: FileSystemPath, access: FileSystemAccessMode) -> Self {
        Self {
            path,
            access,
            missing_path_behavior: Some(FileSystemSandboxEntryMissingPathBehavior::Skip),
        }
    }

    pub fn skips_missing_path(&self) -> bool {
        self.missing_path_behavior == Some(FileSystemSandboxEntryMissingPathBehavior::Skip)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileSystemSandboxKind {
    #[default]
    Restricted,
    Unrestricted,
    ExternalSandbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSystemSandboxPolicy {
    pub kind: FileSystemSandboxKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob_scan_max_depth: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<FileSystemSandboxEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemPath {
    Path { path: AbsolutePathBuf },
    /// A git-style glob pattern. Pattern entries currently support
    /// FileSystemAccessMode::Deny only.
    GlobPattern { pattern: String },
    Special { value: FileSystemSpecialPath },
}

fn read_only_file_system_entries() -> Vec<FileSystemSandboxEntry> {
    vec![FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        FileSystemAccessMode::Read,
    )]
}

impl Default for FileSystemSandboxPolicy {
    fn default() -> Self {
        Self::read_only()
    }
}

impl FileSystemSandboxPolicy {
    pub fn read_only() -> Self {
        Self::restricted(read_only_file_system_entries())
    }

    pub fn unrestricted() -> Self {
        Self {
            kind: FileSystemSandboxKind::Unrestricted,
            glob_scan_max_depth: None,
            entries: Vec::new(),
        }
    }

    pub fn external_sandbox() -> Self {
        Self {
            kind: FileSystemSandboxKind::ExternalSandbox,
            glob_scan_max_depth: None,
            entries: Vec::new(),
        }
    }

    pub fn restricted(entries: Vec<FileSystemSandboxEntry>) -> Self {
        Self {
            kind: FileSystemSandboxKind::Restricted,
            glob_scan_max_depth: None,
            entries,
        }
    }

    /// Removes entries that should be skipped when their paths are missing.
    pub fn remove_skip_missing_path_entries(&mut self) {
        self.entries.retain(|entry| !entry.skips_missing_path());
    }

    pub(crate) fn has_root_access(&self, predicate: impl Fn(FileSystemAccessMode) -> bool) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                matches!(
                    &entry.path,
                    FileSystemPath::Special { value }
                        if matches!(value, FileSystemSpecialPath::Root) && predicate(entry.access)
                )
            })
    }

    pub fn has_denied_read_restrictions(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                entry.access == FileSystemAccessMode::Deny
                    && !matches!(
                        &entry.path,
                        FileSystemPath::Special {
                            value: FileSystemSpecialPath::SlashTmp,
                        } if !cfg!(unix)
                    )
            })
    }
}

/// Filesystem permissions for profiles where Nano owns sandbox construction.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ManagedFileSystemPermissions {
    /// Apply a managed filesystem sandbox from the listed entries.
    #[serde(rename_all = "snake_case")]
    Restricted {
        entries: Vec<FileSystemSandboxEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glob_scan_max_depth: Option<std::num::NonZeroUsize>,
    },
    /// Apply a managed sandbox that allows all filesystem access.
    Unrestricted,
}

/// Canonical active runtime permissions for a conversation, turn, or command.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionProfile {
    /// Nano owns sandbox construction for this profile.
    #[serde(rename_all = "snake_case")]
    Managed {
        file_system: ManagedFileSystemPermissions,
        network: NetworkSandboxPolicy,
    },
    /// Do not apply an outer sandbox.
    Disabled,
    /// Filesystem isolation is enforced by an external caller.
    #[serde(rename_all = "snake_case")]
    External { network: NetworkSandboxPolicy },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_default_has_root_read_and_no_write() {
        let policy = FileSystemSandboxPolicy::default();
        assert!(policy.has_root_access(FileSystemAccessMode::can_read));
        assert!(!policy.has_root_access(FileSystemAccessMode::can_write));
        assert!(!policy.has_denied_read_restrictions());
    }

    #[test]
    fn deny_entry_marks_restrictions() {
        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry::new(
            FileSystemPath::GlobPattern {
                pattern: "**/.env".into(),
            },
            FileSystemAccessMode::Deny,
        )]);
        assert!(policy.has_denied_read_restrictions());
    }

    #[test]
    fn access_mode_precedence_ordering() {
        assert!(FileSystemAccessMode::Deny > FileSystemAccessMode::Write);
        assert!(FileSystemAccessMode::Write > FileSystemAccessMode::Read);
    }

    #[test]
    fn serde_shape_matches_donor() {
        // Donor compatibility: kebab-case kind, snake_case tagged paths.
        let policy = FileSystemSandboxPolicy::read_only();
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("\"kind\":\"restricted\""));
        assert!(json.contains("\"type\":\"special\""));
        assert!(json.contains("\"kind\":\"root\""));
    }
}
