//! Filesystem policy engine: cwd resolution, precedence, read-deny matching,
//! writable-root carveouts, and metadata protection.
//!
//! Provenance: ported from Codex `codex-rs/protocol/src/permissions.rs` (engine
//! half) + `codex-rs/protocol/src/protocol.rs` (`WritableRoot`) +
//! `codex-rs/utils/absolute-path/src/lib.rs` (`canonicalize_preserving_symlinks`)
//! @ 646f7c0a. Transformations:
//! - module paths only, except the deliberate branding change below;
//! - PROTECTED_METADATA `.codex` → `.nano` (Nano owns no `.codex` dirs; the
//!   carveout exists to keep the agent from self-modifying its own tooling
//!   config — the Nano equivalent is `.nano`);
//! - `error!` diagnostics → `tracing::warn!`;
//! - named ADS spellings (`file:stream`) are normalized to the base file
//!   before candidate resolution so a deny on a file covers all its streams
//!   (NanoK3 hardening, no donor counterpart);
//! - legacy `SandboxPolicy` conversions, semantic-signature/equivalence, and
//!   `materialize_project_roots_*` intentionally NOT ported (greenfield: no
//!   legacy config exists; land with a consumer if ever needed).

use crate::abs::AbsolutePathBuf;
use crate::permissions::FileSystemAccessMode;
use crate::permissions::FileSystemPath;
use crate::permissions::FileSystemSandboxEntry;
use crate::permissions::FileSystemSandboxKind;
use crate::permissions::FileSystemSandboxPolicy;
use crate::permissions::FileSystemSpecialPath;
use globset::GlobBuilder;
use globset::GlobMatcher;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;

const PROTECTED_METADATA_GIT_PATH_NAME: &str = ".git";
const PROTECTED_METADATA_AGENTS_PATH_NAME: &str = ".agents";
const PROTECTED_METADATA_NANO_PATH_NAME: &str = ".nano";

/// Metadata path names protected from agent writes under writable roots.
pub const PROTECTED_METADATA_PATH_NAMES: &[&str] = &[
    PROTECTED_METADATA_GIT_PATH_NAME,
    PROTECTED_METADATA_AGENTS_PATH_NAME,
    PROTECTED_METADATA_NANO_PATH_NAME,
];

/// A writable root path accompanied by a list of subpaths that should remain
/// read-only even when the root is writable. This is primarily used to ensure
/// that folders containing files that could be modified to escalate the
/// privileges of the agent (e.g. `.nano`, `.git`, notably `.git/hooks`) under
/// a writable root are not modified by the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableRoot {
    pub root: AbsolutePathBuf,

    /// By construction, these subpaths are all under `root`.
    pub read_only_subpaths: Vec<AbsolutePathBuf>,

    /// Workspace metadata path names that must not be created or replaced under
    /// `root` unless the policy grants an explicit write rule for that metadata
    /// path.
    pub protected_metadata_names: Vec<String>,
}

impl WritableRoot {
    pub fn is_path_writable(&self, path: &Path) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }
        for subpath in &self.read_only_subpaths {
            if path.starts_with(subpath) {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedFileSystemEntry {
    path: AbsolutePathBuf,
    access: FileSystemAccessMode,
}

/// Returns the logical absolute path when canonicalization would rewrite
/// through a nested symlink; otherwise the canonical target.
///
/// Provenance: codex-utils-absolute-path `canonicalize_preserving_symlinks`.
pub fn canonicalize_preserving_symlinks(path: &Path) -> std::io::Result<PathBuf> {
    let logical = AbsolutePathBuf::from_absolute_path(path)?.into_path_buf();
    let preserve_logical_path = should_preserve_logical_path(&logical);
    match dunce::canonicalize(path) {
        Ok(canonical) if preserve_logical_path && canonical != logical => Ok(logical),
        Ok(canonical) => Ok(canonical),
        Err(_) => Ok(logical),
    }
}

fn should_preserve_logical_path(logical: &Path) -> bool {
    logical.ancestors().any(|ancestor| {
        let Ok(metadata) = std::fs::symlink_metadata(ancestor) else {
            return false;
        };
        metadata.file_type().is_symlink() && ancestor.parent().and_then(Path::parent).is_some()
    })
}

impl FileSystemSandboxPolicy {
    pub fn resolve_access_with_cwd(&self, path: &Path, cwd: &Path) -> FileSystemAccessMode {
        match self.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => {
                return FileSystemAccessMode::Write;
            }
            FileSystemSandboxKind::Restricted => {}
        }

        let Some(path) = resolve_candidate_path(path, cwd) else {
            return FileSystemAccessMode::Deny;
        };

        self.resolved_entries_with_cwd(cwd)
            .into_iter()
            .filter(|entry| path.as_path().starts_with(entry.path.as_path()))
            .max_by_key(resolved_entry_precedence)
            .map(|entry| entry.access)
            .unwrap_or(FileSystemAccessMode::Deny)
    }

    pub fn can_read_path_with_cwd(&self, path: &Path, cwd: &Path) -> bool {
        self.resolve_access_with_cwd(path, cwd).can_read()
    }

    pub fn can_write_path_with_cwd(&self, path: &Path, cwd: &Path) -> bool {
        if !self.resolve_access_with_cwd(path, cwd).can_write() {
            return false;
        }
        if self.has_full_disk_write_access() {
            return true;
        }
        if self.is_write_link_escape(path, cwd) {
            return false;
        }
        !self.is_metadata_write_denied(path, cwd)
    }

    /// Fail-closed reparse-point check for writes: resolves the nearest
    /// existing ancestor of the target (writes often create new files, so the
    /// target itself may not exist) so an NTFS junction or symlinked
    /// directory inside a writable root cannot redirect the write outside
    /// every writable entry. Policy entries are resolved the same way so
    /// both sides of the prefix check compare in canonical spelling.
    fn is_write_link_escape(&self, path: &Path, cwd: &Path) -> bool {
        let Some(path) = resolve_candidate_path(path, cwd) else {
            return true;
        };
        let effective_path = canonicalize_write_target(&path);
        if effective_path == path {
            return false;
        }
        !self
            .resolved_entries_with_cwd(cwd)
            .into_iter()
            .map(|entry| ResolvedFileSystemEntry {
                path: canonicalize_write_target(&entry.path),
                access: entry.access,
            })
            .filter(|entry| effective_path.as_path().starts_with(entry.path.as_path()))
            .max_by_key(resolved_entry_precedence)
            .map(|entry| entry.access)
            .unwrap_or(FileSystemAccessMode::Deny)
            .can_write()
    }

    fn is_metadata_write_denied(&self, path: &Path, cwd: &Path) -> bool {
        if !matches!(self.kind, FileSystemSandboxKind::Restricted) {
            return false;
        }

        let Some(target) = resolve_candidate_path(path, cwd) else {
            return true;
        };
        let Some((protected_metadata_path, _)) =
            metadata_child_of_writable_root(self, target.as_path(), cwd)
        else {
            return false;
        };

        !has_explicit_write_entry_for_metadata_path(
            self,
            &protected_metadata_path,
            target.as_path(),
            cwd,
        )
    }

    /// Returns true when filesystem reads are unrestricted.
    pub fn has_full_disk_read_access(&self) -> bool {
        match self.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => true,
            FileSystemSandboxKind::Restricted => {
                self.has_root_access(FileSystemAccessMode::can_read)
                    && !self.has_denied_read_restrictions()
            }
        }
    }

    /// Returns true when filesystem writes are unrestricted.
    pub fn has_full_disk_write_access(&self) -> bool {
        match self.kind {
            FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => true,
            FileSystemSandboxKind::Restricted => {
                self.has_root_access(FileSystemAccessMode::can_write)
                    && !self.has_write_narrowing_entries()
            }
        }
    }

    /// Returns true when platform-default readable roots should be included.
    pub fn include_platform_defaults(&self) -> bool {
        !self.has_full_disk_read_access()
            && matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                matches!(
                    &entry.path,
                    FileSystemPath::Special { value }
                        if matches!(value, FileSystemSpecialPath::Minimal)
                            && entry.access.can_read()
                )
            })
    }

    /// Returns true when a restricted policy contains any entry that really
    /// reduces a broader `:root = write` grant.
    fn has_write_narrowing_entries(&self) -> bool {
        matches!(self.kind, FileSystemSandboxKind::Restricted)
            && self.entries.iter().any(|entry| {
                if entry.access.can_write() {
                    return false;
                }

                match &entry.path {
                    FileSystemPath::Path { .. } => !self.has_same_target_write_override(entry),
                    FileSystemPath::GlobPattern { .. } => true,
                    FileSystemPath::Special { value } => match value {
                        FileSystemSpecialPath::Root => entry.access == FileSystemAccessMode::Deny,
                        FileSystemSpecialPath::SlashTmp if !cfg!(unix) => false,
                        FileSystemSpecialPath::Minimal | FileSystemSpecialPath::Unknown { .. } => {
                            false
                        }
                        _ => !self.has_same_target_write_override(entry),
                    },
                }
            })
    }

    /// Returns true when a higher-priority `write` entry targets the same
    /// location as `entry`, so `entry` cannot narrow effective write access.
    fn has_same_target_write_override(&self, entry: &FileSystemSandboxEntry) -> bool {
        self.entries.iter().any(|candidate| {
            candidate.access.can_write()
                && candidate.access > entry.access
                && file_system_paths_share_target(&candidate.path, &entry.path)
        })
    }

    /// Returns the explicit readable roots resolved against the provided cwd.
    pub fn get_readable_roots_with_cwd(&self, cwd: &Path) -> Vec<AbsolutePathBuf> {
        if self.has_full_disk_read_access() {
            return Vec::new();
        }

        dedup_absolute_paths(
            self.resolved_entries_with_cwd(cwd)
                .into_iter()
                .filter(|entry| entry.access.can_read())
                .filter(|entry| self.can_read_path_with_cwd(entry.path.as_path(), cwd))
                .map(|entry| entry.path)
                .collect(),
            /*normalize_effective_paths*/ true,
        )
    }

    /// Returns the writable roots together with read-only carveouts resolved
    /// against the provided cwd.
    ///
    /// Carveout order is deterministic and independent of the cwd's
    /// raw-vs-canonical spelling: explicit entries are matched against the
    /// on-disk defaults after `normalize_effective_absolute_path`, so hosts
    /// whose cwd is not in canonical form (macOS `/var` -> `/private/var`,
    /// Windows 8.3 short names) emit the same order as canonical hosts.
    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
        if self.has_full_disk_write_access() {
            return Vec::new();
        }

        let resolved_entries = self.resolved_entries_with_cwd(cwd);
        let writable_entries: Vec<AbsolutePathBuf> = resolved_entries
            .iter()
            .filter(|entry| entry.access.can_write())
            .filter(|entry| self.can_write_path_with_cwd(entry.path.as_path(), cwd))
            .map(|entry| entry.path.clone())
            .collect();

        dedup_absolute_paths(
            writable_entries.clone(),
            /*normalize_effective_paths*/ true,
        )
        .into_iter()
        .map(|root| {
            let preserve_raw_carveout_paths = root.as_path().parent().is_some();
            let raw_writable_roots: Vec<&AbsolutePathBuf> = writable_entries
                .iter()
                .filter(|path| normalize_effective_absolute_path((*path).clone()) == root)
                .collect();
            let protected_metadata_names =
                protected_metadata_names_for_writable_root(self, &root, &raw_writable_roots, cwd);
            let protect_missing_dot_nano = AbsolutePathBuf::from_absolute_path(cwd)
                .ok()
                .is_some_and(|cwd| normalize_effective_absolute_path(cwd) == root);
            let mut read_only_subpaths: Vec<AbsolutePathBuf> =
                default_read_only_subpaths_for_writable_root(&root, protect_missing_dot_nano)
                    .into_iter()
                    .filter(|path| !has_explicit_resolved_path_entry(&resolved_entries, path))
                    .collect();
            // Narrower explicit non-write entries carve out broader writable
            // roots; literal in-root paths (incl. symlinks) are preserved so
            // downstream sandboxes can mask the symlink inode itself.
            read_only_subpaths.extend(
                resolved_entries
                    .iter()
                    .filter(|entry| !entry.access.can_write())
                    .filter(|entry| !self.can_write_path_with_cwd(entry.path.as_path(), cwd))
                    .filter_map(|entry| {
                        let effective_path = normalize_effective_absolute_path(entry.path.clone());
                        let raw_carveout_path = if preserve_raw_carveout_paths {
                            if entry.path == root {
                                None
                            } else if entry.path.as_path().starts_with(root.as_path()) {
                                Some(entry.path.clone())
                            } else {
                                raw_writable_roots.iter().find_map(|raw_root| {
                                    let suffix = entry
                                        .path
                                        .as_path()
                                        .strip_prefix(raw_root.as_path())
                                        .ok()?;
                                    if suffix.as_os_str().is_empty() {
                                        return None;
                                    }
                                    Some(root.join(suffix))
                                })
                            }
                        } else {
                            None
                        };

                        if let Some(raw_carveout_path) = raw_carveout_path {
                            return Some(raw_carveout_path);
                        }

                        if effective_path == root
                            || !effective_path.as_path().starts_with(root.as_path())
                        {
                            return None;
                        }

                        Some(effective_path)
                    }),
            );
            WritableRoot {
                protected_metadata_names,
                root,
                read_only_subpaths: dedup_absolute_paths(
                    read_only_subpaths,
                    /*normalize_effective_paths*/ false,
                ),
            }
        })
        .collect()
    }

    /// Returns explicit unreadable roots resolved against the provided cwd.
    pub fn get_unreadable_roots_with_cwd(&self, cwd: &Path) -> Vec<AbsolutePathBuf> {
        if !matches!(self.kind, FileSystemSandboxKind::Restricted) {
            return Vec::new();
        }

        let root = AbsolutePathBuf::from_absolute_path(cwd)
            .ok()
            .map(|cwd| absolute_root_path_for_cwd(&cwd));

        dedup_absolute_paths(
            self.resolved_entries_with_cwd(cwd)
                .iter()
                .filter(|entry| entry.access == FileSystemAccessMode::Deny)
                .filter(|entry| !self.can_read_path_with_cwd(entry.path.as_path(), cwd))
                // Restricted policies already deny reads outside explicit allow
                // roots, so materializing the filesystem root here would erase
                // narrower readable carveouts.
                .filter(|entry| root.as_ref() != Some(&entry.path))
                .map(|entry| entry.path.clone())
                .collect(),
            /*normalize_effective_paths*/ true,
        )
    }

    /// Returns unreadable glob patterns resolved against the provided cwd.
    pub fn get_unreadable_globs_with_cwd(&self, cwd: &Path) -> Vec<String> {
        if !matches!(self.kind, FileSystemSandboxKind::Restricted) {
            return Vec::new();
        }

        let mut patterns = self
            .entries
            .iter()
            .filter(|entry| entry.access == FileSystemAccessMode::Deny)
            .filter_map(|entry| match &entry.path {
                FileSystemPath::GlobPattern { pattern } => {
                    Some(AbsolutePathBuf::resolve_path_against_base(pattern, cwd))
                }
                FileSystemPath::Path { .. } | FileSystemPath::Special { .. } => None,
            })
            .map(|pattern| pattern.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        patterns.sort();
        patterns.dedup();
        patterns
    }

    fn resolved_entries_with_cwd(&self, cwd: &Path) -> Vec<ResolvedFileSystemEntry> {
        let cwd_absolute = AbsolutePathBuf::from_absolute_path(cwd).ok();
        self.entries
            .iter()
            .filter_map(|entry| {
                resolve_entry_path(&entry.path, cwd_absolute.as_ref()).map(|path| {
                    ResolvedFileSystemEntry {
                        path,
                        access: entry.access,
                    }
                })
            })
            .collect()
    }
}

fn resolve_file_system_path(
    path: &FileSystemPath,
    cwd: Option<&AbsolutePathBuf>,
) -> Option<AbsolutePathBuf> {
    match path {
        FileSystemPath::Path { path } => Some(path.clone()),
        FileSystemPath::GlobPattern { .. } => None,
        FileSystemPath::Special { value } => resolve_file_system_special_path(value, cwd),
    }
}

fn resolve_entry_path(
    path: &FileSystemPath,
    cwd: Option<&AbsolutePathBuf>,
) -> Option<AbsolutePathBuf> {
    match path {
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        } => cwd.map(absolute_root_path_for_cwd),
        _ => resolve_file_system_path(path, cwd),
    }
}

fn resolve_candidate_path(path: &Path, cwd: &Path) -> Option<AbsolutePathBuf> {
    let path = strip_named_stream_suffix(path);
    let path = path.as_path();
    if path.is_absolute() {
        AbsolutePathBuf::from_absolute_path(path).ok()
    } else {
        Some(AbsolutePathBuf::from_absolute_path(cwd).ok()?.join(path))
    }
}

/// Strip an NTFS alternate-data-stream suffix (`:name`) from the final path
/// component so the policy decision is made on the BASE FILE for all of its
/// streams: a named stream of a denied file is denied along with the file.
/// Drive-letter colons live in the path prefix, not the final component, and
/// a colon at the start of the final component has no base name to keep, so
/// neither is treated as a stream suffix. Windows-only: `:` is an ordinary
/// filename character elsewhere.
fn strip_named_stream_suffix(path: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(name) = path.file_name().and_then(|name| name.to_str())
        && let Some(colon) = name.find(':')
        && colon > 0
    {
        return path.with_file_name(&name[..colon]);
    }
    path.to_path_buf()
}

/// Returns true when two config paths refer to the same exact target before
/// any prefix matching is applied (narrower than full resolution by design —
/// used only by `has_write_narrowing_entries`).
fn file_system_paths_share_target(left: &FileSystemPath, right: &FileSystemPath) -> bool {
    match (left, right) {
        (FileSystemPath::Path { path: left }, FileSystemPath::Path { path: right }) => {
            left == right
        }
        (FileSystemPath::Special { value: left }, FileSystemPath::Special { value: right }) => {
            special_paths_share_target(left, right)
        }
        (FileSystemPath::Path { path }, FileSystemPath::Special { value })
        | (FileSystemPath::Special { value }, FileSystemPath::Path { path }) => {
            special_path_matches_absolute_path(value, path)
        }
        (
            FileSystemPath::GlobPattern { pattern: left },
            FileSystemPath::GlobPattern { pattern: right },
        ) => left == right,
        (FileSystemPath::GlobPattern { .. }, _) | (_, FileSystemPath::GlobPattern { .. }) => false,
    }
}

fn special_paths_share_target(left: &FileSystemSpecialPath, right: &FileSystemSpecialPath) -> bool {
    match (left, right) {
        (FileSystemSpecialPath::Root, FileSystemSpecialPath::Root)
        | (FileSystemSpecialPath::Minimal, FileSystemSpecialPath::Minimal)
        | (FileSystemSpecialPath::Tmpdir, FileSystemSpecialPath::Tmpdir)
        | (FileSystemSpecialPath::SlashTmp, FileSystemSpecialPath::SlashTmp) => true,
        (
            FileSystemSpecialPath::ProjectRoots { subpath: left },
            FileSystemSpecialPath::ProjectRoots { subpath: right },
        ) => left == right,
        (
            FileSystemSpecialPath::Unknown {
                path: left,
                subpath: left_subpath,
            },
            FileSystemSpecialPath::Unknown {
                path: right,
                subpath: right_subpath,
            },
        ) => left == right && left_subpath == right_subpath,
        _ => false,
    }
}

/// Matches cwd-independent special paths against absolute `Path` entries when
/// they name the same location (only `/` and `/tmp` are stable without a cwd).
fn special_path_matches_absolute_path(
    value: &FileSystemSpecialPath,
    path: &AbsolutePathBuf,
) -> bool {
    match value {
        FileSystemSpecialPath::Root => path.as_path().parent().is_none(),
        FileSystemSpecialPath::SlashTmp => path.as_path() == Path::new("/tmp"),
        _ => false,
    }
}

/// Orders resolved entries so the most specific path wins first, then applies
/// the access tie-breaker from [`FileSystemAccessMode`].
fn resolved_entry_precedence(entry: &ResolvedFileSystemEntry) -> (usize, FileSystemAccessMode) {
    let specificity = entry.path.as_path().components().count();
    (specificity, entry.access)
}

fn absolute_root_path_for_cwd(cwd: &AbsolutePathBuf) -> AbsolutePathBuf {
    let root = cwd
        .as_path()
        .ancestors()
        .last()
        .unwrap_or_else(|| panic!("cwd must have a filesystem root"));
    AbsolutePathBuf::from_absolute_path(root)
        .unwrap_or_else(|err| panic!("cwd root must be an absolute path: {err}"))
}

fn normalized_and_canonical_candidates(path: &Path) -> Vec<PathBuf> {
    // Compare the lexical absolute form plus the canonical target when it
    // exists. Missing paths still need the lexical candidate so future-created
    // denied paths remain blocked by direct tool checks. Named ADS spellings
    // are reduced to the base file first so spelled and resolved forms agree
    // and a deny on a file covers all of its streams.
    let mut candidates = Vec::new();
    let path = strip_named_stream_suffix(path);
    let path = path.as_path();

    if let Ok(normalized) = AbsolutePathBuf::from_absolute_path(path) {
        push_unique(&mut candidates, normalized.to_path_buf());
    } else {
        push_unique(&mut candidates, path.to_path_buf());
    }

    if let Ok(canonical) = path.canonicalize()
        && let Ok(canonical_absolute) = AbsolutePathBuf::from_absolute_path(canonical)
    {
        push_unique(&mut candidates, canonical_absolute.to_path_buf());
    }

    candidates
}

fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn build_glob_matcher(pattern: &str) -> Result<GlobMatcher, String> {
    // Keep `*` and `?` within a single path component and preserve an unclosed
    // `[` as a literal so matcher behavior stays aligned with config parsing.
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .allow_unclosed_class(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|err| err.to_string())
}

fn resolve_file_system_special_path(
    value: &FileSystemSpecialPath,
    cwd: Option<&AbsolutePathBuf>,
) -> Option<AbsolutePathBuf> {
    match value {
        FileSystemSpecialPath::Root
        | FileSystemSpecialPath::Minimal
        | FileSystemSpecialPath::Unknown { .. } => None,
        FileSystemSpecialPath::ProjectRoots { subpath } => {
            let cwd = cwd?;
            match subpath.as_ref() {
                Some(subpath) => Some(AbsolutePathBuf::resolve_path_against_base(
                    subpath,
                    cwd.as_path(),
                )),
                None => Some(cwd.clone()),
            }
        }
        FileSystemSpecialPath::Tmpdir => {
            let tmpdir = std::env::var_os("TMPDIR")?;
            if tmpdir.is_empty() {
                None
            } else {
                let tmpdir = AbsolutePathBuf::from_absolute_path(PathBuf::from(tmpdir)).ok()?;
                Some(tmpdir)
            }
        }
        FileSystemSpecialPath::SlashTmp => {
            if !cfg!(unix) {
                return None;
            }
            #[allow(clippy::expect_used)]
            let slash_tmp = AbsolutePathBuf::from_absolute_path("/tmp").expect("/tmp is absolute");
            if !slash_tmp.as_path().is_dir() {
                return None;
            }
            Some(slash_tmp)
        }
    }
}

fn dedup_absolute_paths(
    paths: Vec<AbsolutePathBuf>,
    normalize_effective_paths: bool,
) -> Vec<AbsolutePathBuf> {
    let mut deduped = Vec::with_capacity(paths.len());
    let mut seen = HashSet::new();
    for path in paths {
        let dedup_path = if normalize_effective_paths {
            normalize_effective_absolute_path(path)
        } else {
            path
        };
        if seen.insert(dedup_path.to_path_buf()) {
            deduped.push(dedup_path);
        }
    }
    deduped
}

/// Fully resolves the nearest existing ancestor of `path` (the target itself
/// may not exist yet for writes of new files) and re-appends the remaining
/// components, so reparse points (NTFS junctions, symlinked directories)
/// anywhere in the chain are resolved before policy prefix matching.
///
/// Unlike [`normalize_effective_absolute_path`] this never preserves logical
/// spellings through symlinks: write targets must be checked where the write
/// would actually land. `dunce::canonicalize` keeps the result free of `\\?\`
/// verbatim prefixes so it compares cleanly against lexical paths.
fn canonicalize_write_target(path: &AbsolutePathBuf) -> AbsolutePathBuf {
    let raw_path = path.to_path_buf();
    for ancestor in raw_path.ancestors() {
        if std::fs::symlink_metadata(ancestor).is_err() {
            continue;
        }
        let Ok(canonical_ancestor) = dunce::canonicalize(ancestor) else {
            continue;
        };
        let Ok(suffix) = raw_path.strip_prefix(ancestor) else {
            continue;
        };
        if let Ok(canonical_path) =
            AbsolutePathBuf::from_absolute_path(canonical_ancestor.join(suffix))
        {
            return canonical_path;
        }
    }
    path.clone()
}

fn normalize_effective_absolute_path(path: AbsolutePathBuf) -> AbsolutePathBuf {
    let raw_path = path.to_path_buf();
    for ancestor in raw_path.ancestors() {
        if std::fs::symlink_metadata(ancestor).is_err() {
            continue;
        }
        let Ok(normalized_ancestor) = canonicalize_preserving_symlinks(ancestor) else {
            continue;
        };
        let Ok(suffix) = raw_path.strip_prefix(ancestor) else {
            continue;
        };
        if let Ok(normalized_path) =
            AbsolutePathBuf::from_absolute_path(normalized_ancestor.join(suffix))
        {
            return normalized_path;
        }
    }
    path
}

pub(crate) fn default_read_only_subpaths_for_writable_root(
    writable_root: &AbsolutePathBuf,
    protect_missing_dot_nano: bool,
) -> Vec<AbsolutePathBuf> {
    let mut subpaths: Vec<AbsolutePathBuf> = Vec::new();
    let top_level_git = writable_root.join(PROTECTED_METADATA_GIT_PATH_NAME);
    // Applies to typical repos (directory .git), worktrees/submodules (file
    // .git with gitdir pointer), and bare repos when the gitdir is the root.
    let top_level_git_is_file = top_level_git.as_path().is_file();
    let top_level_git_is_dir = top_level_git.as_path().is_dir();
    let should_protect_top_level = top_level_git_is_dir || top_level_git_is_file;
    if should_protect_top_level {
        if top_level_git_is_file
            && is_git_pointer_file(&top_level_git)
            && let Some(gitdir) = resolve_gitdir_from_file(&top_level_git)
        {
            subpaths.push(gitdir);
        }
        subpaths.push(top_level_git);
    }

    let top_level_agents = writable_root.join(PROTECTED_METADATA_AGENTS_PATH_NAME);
    if top_level_agents.as_path().is_dir() {
        subpaths.push(top_level_agents);
    }

    // Keep top-level project metadata under .nano read-only to the agent by
    // default. For the workspace root itself, protect it even before the
    // directory exists so first-time creation goes through approval.
    let top_level_nano = writable_root.join(PROTECTED_METADATA_NANO_PATH_NAME);
    if protect_missing_dot_nano || top_level_nano.as_path().is_dir() {
        subpaths.push(top_level_nano);
    }

    dedup_absolute_paths(subpaths, /*normalize_effective_paths*/ false)
}

fn has_explicit_resolved_path_entry(
    entries: &[ResolvedFileSystemEntry],
    path: &AbsolutePathBuf,
) -> bool {
    // Match on the normalized effective spelling, not only the raw one: the
    // writable root (and therefore the default carveouts derived from it) is
    // canonicalized, while explicit entries keep the spelling they were
    // resolved from. Those spellings diverge whenever the cwd is not in
    // canonical form (macOS `/var` -> `/private/var` TMPDIR, Windows 8.3
    // short-name %TEMP% or casing), and a raw-only equality check would miss
    // same-target entries across that split, duplicating the default
    // carveouts and making the emitted carveout ORDER host-dependent.
    let normalized_path = normalize_effective_absolute_path(path.clone());
    entries.iter().any(|entry| {
        &entry.path == path
            || normalize_effective_absolute_path(entry.path.clone()) == normalized_path
    })
}

fn metadata_path_name(name: &OsStr) -> Option<&'static str> {
    PROTECTED_METADATA_PATH_NAMES
        .iter()
        .copied()
        .find(|metadata_name| name == OsStr::new(metadata_name))
}

fn metadata_child_of_writable_root(
    policy: &FileSystemSandboxPolicy,
    target: &Path,
    cwd: &Path,
) -> Option<(AbsolutePathBuf, &'static str)> {
    policy
        .resolved_entries_with_cwd(cwd)
        .iter()
        .filter(|entry| entry.access.can_write())
        .filter_map(|entry| {
            let relative_path = target.strip_prefix(entry.path.as_path()).ok()?;
            let first_component = relative_path.components().next()?;
            let metadata_name = metadata_path_name(first_component.as_os_str())?;
            Some((entry.path.join(metadata_name), metadata_name))
        })
        .next()
}

fn protected_metadata_names_for_writable_root(
    policy: &FileSystemSandboxPolicy,
    root: &AbsolutePathBuf,
    raw_writable_roots: &[&AbsolutePathBuf],
    cwd: &Path,
) -> Vec<String> {
    let mut protected_names = Vec::new();
    for metadata_name in PROTECTED_METADATA_PATH_NAMES {
        let mut metadata_paths = vec![root.join(*metadata_name)];
        metadata_paths.extend(
            raw_writable_roots
                .iter()
                .map(|raw_root| raw_root.join(*metadata_name)),
        );

        if metadata_paths
            .iter()
            .all(|metadata_path| !policy.can_write_path_with_cwd(metadata_path.as_path(), cwd))
        {
            protected_names.push((*metadata_name).to_string());
        }
    }
    protected_names
}

fn has_explicit_write_entry_for_metadata_path(
    policy: &FileSystemSandboxPolicy,
    protected_metadata_path: &AbsolutePathBuf,
    target: &Path,
    cwd: &Path,
) -> bool {
    policy.resolved_entries_with_cwd(cwd).iter().any(|entry| {
        entry.access.can_write()
            && target.starts_with(entry.path.as_path())
            && entry
                .path
                .as_path()
                .starts_with(protected_metadata_path.as_path())
    })
}

fn is_git_pointer_file(path: &AbsolutePathBuf) -> bool {
    path.as_path().is_file()
        && path.as_path().file_name() == Some(OsStr::new(PROTECTED_METADATA_GIT_PATH_NAME))
}

fn resolve_gitdir_from_file(dot_git: &AbsolutePathBuf) -> Option<AbsolutePathBuf> {
    let contents = match std::fs::read_to_string(dot_git.as_path()) {
        Ok(contents) => contents,
        Err(err) => {
            tracing::warn!(
                "Failed to read {path} for gitdir pointer: {err}",
                path = dot_git.as_path().display()
            );
            return None;
        }
    };

    let trimmed = contents.trim();
    let (_, gitdir_raw) = match trimmed.split_once(':') {
        Some((prefix, gitdir_raw)) if prefix.trim() == "gitdir" => (prefix, gitdir_raw),
        Some(_) => {
            tracing::warn!(
                "Expected {path} to contain a gitdir pointer, but it did not match `gitdir: <path>`.",
                path = dot_git.as_path().display()
            );
            return None;
        }
        None => {
            tracing::warn!(
                "Expected {path} to contain a gitdir pointer, but it is empty.",
                path = dot_git.as_path().display()
            );
            return None;
        }
    };

    let gitdir_raw = gitdir_raw.trim();
    if gitdir_raw.is_empty() {
        return None;
    }
    let base = dot_git.parent()?;
    Some(AbsolutePathBuf::resolve_path_against_base(
        gitdir_raw,
        base.as_path(),
    ))
}

/// Runtime matcher for read-deny entries in a filesystem sandbox policy.
pub struct ReadDenyMatcher {
    denied_candidates: Vec<Vec<PathBuf>>,
    deny_read_matchers: Vec<GlobMatcher>,
    invalid_pattern: bool,
}

impl ReadDenyMatcher {
    /// Builds a matcher from exact deny-read roots and deny-read glob entries.
    ///
    /// Returns `None` when the policy has no deny-read restrictions, so callers
    /// can skip read-deny checks without allocating matcher state. The `cwd`
    /// resolves cwd-relative policy paths and special paths before matching.
    pub fn new(file_system_sandbox_policy: &FileSystemSandboxPolicy, cwd: &Path) -> Option<Self> {
        match Self::build(
            file_system_sandbox_policy,
            cwd,
            InvalidDenyReadGlobBehavior::FailClosed,
        ) {
            Ok(matcher) => matcher,
            Err(_) => unreachable!("fail-closed glob handling does not return errors"),
        }
    }

    /// Builds a matcher for callers that must reject malformed glob patterns.
    pub fn try_new(
        file_system_sandbox_policy: &FileSystemSandboxPolicy,
        cwd: &Path,
    ) -> Result<Option<Self>, String> {
        Self::build(
            file_system_sandbox_policy,
            cwd,
            InvalidDenyReadGlobBehavior::ReturnError,
        )
    }

    fn build(
        file_system_sandbox_policy: &FileSystemSandboxPolicy,
        cwd: &Path,
        invalid_glob_behavior: InvalidDenyReadGlobBehavior,
    ) -> Result<Option<Self>, String> {
        if !file_system_sandbox_policy.has_denied_read_restrictions() {
            return Ok(None);
        }

        // Exact roots are stored as all meaningful path spellings we can derive
        // cheaply. This lets direct tool checks catch both a symlink path and
        // its canonical target without changing the policy entries themselves.
        let denied_candidates = file_system_sandbox_policy
            .get_unreadable_roots_with_cwd(cwd)
            .into_iter()
            .map(|path| normalized_and_canonical_candidates(path.as_path()))
            .collect();
        // Pattern entries stay as policy-level globs. They are matched at read
        // time here instead of being snapshotted to startup filesystem state.
        let mut invalid_pattern = false;
        let mut deny_read_matchers = Vec::new();
        for pattern in file_system_sandbox_policy.get_unreadable_globs_with_cwd(cwd) {
            match build_glob_matcher(&pattern) {
                Ok(matcher) => deny_read_matchers.push(matcher),
                Err(err) => match invalid_glob_behavior {
                    InvalidDenyReadGlobBehavior::FailClosed => invalid_pattern = true,
                    InvalidDenyReadGlobBehavior::ReturnError => {
                        return Err(format!("invalid deny-read glob pattern `{pattern}`: {err}"));
                    }
                },
            }
        }
        Ok(Some(Self {
            denied_candidates,
            deny_read_matchers,
            invalid_pattern,
        }))
    }

    /// Returns whether `path` is denied by the policy used to build this matcher.
    pub fn is_read_denied(&self, path: &Path) -> bool {
        if self.invalid_pattern {
            // Direct tool reads fail closed on malformed deny patterns. Silent
            // allow would turn a config typo into a policy bypass.
            return true;
        }

        let path_candidates = normalized_and_canonical_candidates(path);
        if self.denied_candidates.iter().any(|denied_candidates| {
            path_candidates.iter().any(|candidate| {
                denied_candidates.iter().any(|denied_candidate| {
                    candidate == denied_candidate || candidate.starts_with(denied_candidate)
                })
            })
        }) {
            return true;
        }

        self.deny_read_matchers.iter().any(|matcher| {
            path_candidates
                .iter()
                .any(|candidate| matcher.is_match(candidate))
        })
    }
}

#[derive(Clone, Copy)]
enum InvalidDenyReadGlobBehavior {
    FailClosed,
    ReturnError,
}

/// Filesystem policy matching `WorkspaceWrite` semantics without requiring a
/// legacy policy first.
///
/// Provenance: codex `permissions.rs` `FileSystemSandboxPolicy::workspace_write`
/// + append-default helpers (`.codex` default subpath → `.nano`).
pub fn workspace_write_policy(
    writable_roots: &[AbsolutePathBuf],
    exclude_tmpdir_env_var: bool,
    exclude_slash_tmp: bool,
) -> FileSystemSandboxPolicy {
    let mut entries = vec![FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::Root,
        },
        FileSystemAccessMode::Read,
    )];

    entries.push(FileSystemSandboxEntry::new(
        FileSystemPath::Special {
            value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
        },
        FileSystemAccessMode::Write,
    ));
    if !exclude_slash_tmp {
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::SlashTmp,
            },
            FileSystemAccessMode::Write,
        ));
    }
    if !exclude_tmpdir_env_var {
        entries.push(FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Tmpdir,
            },
            FileSystemAccessMode::Write,
        ));
    }
    entries.extend(writable_roots.iter().cloned().map(|path| {
        FileSystemSandboxEntry::new(FileSystemPath::Path { path }, FileSystemAccessMode::Write)
    }));

    append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".git");
    append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".agents");
    append_default_read_only_project_root_subpath_if_no_explicit_rule(&mut entries, ".nano");
    for writable_root in writable_roots {
        for protected_path in default_read_only_subpaths_for_writable_root(
            writable_root,
            /*protect_missing*/ false,
        ) {
            append_default_read_only_path_if_no_explicit_rule(&mut entries, protected_path);
        }
    }

    FileSystemSandboxPolicy::restricted(entries)
}

fn append_default_read_only_project_root_subpath_if_no_explicit_rule(
    entries: &mut Vec<FileSystemSandboxEntry>,
    subpath: impl Into<String>,
) {
    append_default_read_only_entry_if_no_explicit_rule(
        entries,
        FileSystemPath::Special {
            value: FileSystemSpecialPath::project_roots(Some(subpath.into())),
        },
    );
}

fn append_default_read_only_path_if_no_explicit_rule(
    entries: &mut Vec<FileSystemSandboxEntry>,
    path: AbsolutePathBuf,
) {
    append_default_read_only_entry_if_no_explicit_rule(entries, FileSystemPath::Path { path });
}

fn append_default_read_only_entry_if_no_explicit_rule(
    entries: &mut Vec<FileSystemSandboxEntry>,
    path: FileSystemPath,
) {
    if entries
        .iter()
        .any(|entry| file_system_paths_share_target(&entry.path, &path))
    {
        return;
    }

    entries.push(FileSystemSandboxEntry::skip_missing_path(
        path,
        FileSystemAccessMode::Read,
    ));
}

impl FileSystemSandboxPolicy {
    /// Replaces symbolic `:workspace_roots` entries with concrete entries for
    /// each workspace root.
    pub fn materialize_project_roots_with_workspace_roots(
        mut self,
        workspace_roots: &[AbsolutePathBuf],
    ) -> Self {
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            match entry.path {
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::ProjectRoots { subpath },
                } => {
                    entries.extend(workspace_roots.iter().map(|root| FileSystemSandboxEntry {
                        path: FileSystemPath::Path {
                            path: match subpath.as_ref() {
                                Some(subpath) => AbsolutePathBuf::resolve_path_against_base(
                                    subpath,
                                    root.as_path(),
                                ),
                                None => root.clone(),
                            },
                        },
                        access: entry.access,
                        missing_path_behavior: entry.missing_path_behavior,
                    }));
                }
                FileSystemPath::GlobPattern { pattern } => {
                    if let Some(subpath) =
                        crate::permissions::parse_project_roots_glob_pattern(&pattern)
                    {
                        entries.extend(workspace_roots.iter().map(|root| FileSystemSandboxEntry {
                            path: FileSystemPath::GlobPattern {
                                pattern: crate::permissions::resolve_project_roots_glob_pattern(
                                    subpath, root,
                                ),
                            },
                            access: entry.access,
                            missing_path_behavior: entry.missing_path_behavior,
                        }));
                    } else {
                        entries.push(FileSystemSandboxEntry {
                            path: FileSystemPath::GlobPattern { pattern },
                            access: entry.access,
                            missing_path_behavior: entry.missing_path_behavior,
                        });
                    }
                }
                FileSystemPath::Path { path } => {
                    entries.push(FileSystemSandboxEntry {
                        path: FileSystemPath::Path { path },
                        access: entry.access,
                        missing_path_behavior: entry.missing_path_behavior,
                    });
                }
                FileSystemPath::Special { value } => {
                    entries.push(FileSystemSandboxEntry {
                        path: FileSystemPath::Special { value },
                        access: entry.access,
                        missing_path_behavior: entry.missing_path_behavior,
                    });
                }
            }
        }
        self.entries = entries;
        self
    }
}
