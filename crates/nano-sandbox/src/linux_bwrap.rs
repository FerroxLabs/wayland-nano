//! Linux bubblewrap (bwrap) filesystem sandbox: argv construction plus system
//! bwrap discovery and WSL1 detection.
//!
//! This module mirrors the semantics used by the macOS Seatbelt sandbox:
//! - the filesystem is read-only by default,
//! - explicit writable roots are layered on top, and
//! - sensitive subpaths such as `.git`, `.agents`, and `.nano` remain
//!   read-only even when their parent root is writable.
//!
//! The overall Linux sandbox is composed of:
//! - bubblewrap constructing the filesystem view before exec (this module
//!   builds its argv), and
//! - seccomp + `PR_SET_NO_NEW_PRIVS` applied in-process by the helper's inner
//!   `--apply-seccomp-then-exec` stage.
//!
//! Provenance: ported from Codex `codex-rs/linux-sandbox/src/bwrap.rs`
//! (argv construction) and `codex-rs/sandboxing/src/bwrap.rs` (system bwrap
//! discovery + WSL1 detection) @ 646f7c0a. Transformations:
//! - CodexErr -> anyhow;
//! - codex_protocol::protocol/permissions -> nano_core::permissions +
//!   nano_core::policy_engine (`WritableRoot`, `PROTECTED_METADATA_PATH_NAMES`);
//! - codex_utils_absolute_path -> nano_core::abs;
//! - managed-network-proxy surface DROPPED (nano-egress owns egress):
//!   `BwrapNetworkMode::ProxyOnly` is not ported; restricted network maps to
//!   `BwrapNetworkMode::Isolated`;
//! - `.codex` protected metadata name -> `.nano` (matches the ported policy
//!   engine's protected set);
//! - `which::which_in_all` -> std-only PATH walk (no new crate dependency;
//!   equivalent on Linux where PATHEXT does not apply);
//! - the bundled-bwrap fallback lives with the helper binary
//!   (`src/bin/linux_sandbox/bundled_bwrap.rs`);
//! - compiled on Linux and in test builds everywhere (argv construction is
//!   pure string building); the user-namespace probe is Linux-only, with Unix
//!   OS interfaces shimmed so non-Unix test hosts still compile.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use globset::GlobBuilder;
use globset::GlobSet;
use globset::GlobSetBuilder;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::FileSystemAccessMode;
use nano_core::permissions::FileSystemPath;
use nano_core::permissions::FileSystemSandboxKind;
use nano_core::permissions::FileSystemSandboxPolicy;
use nano_core::permissions::FileSystemSpecialPath;
use nano_core::permissions::NetworkSandboxPolicy;
use nano_core::permissions::PermissionProfile;
use nano_core::policy_engine::PROTECTED_METADATA_PATH_NAMES;
use nano_core::policy_engine::WritableRoot;

/// Linux "platform defaults" that keep common system binaries and dynamic
/// libraries readable when a split filesystem policy requests `:minimal`.
///
/// These are intentionally system-level paths only (plus Nix store roots) so
/// `include_platform_defaults` does not silently widen access to user data.
const LINUX_PLATFORM_DEFAULT_READ_ROOTS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/lib",
    "/lib64",
    "/nix/store",
    "/run/current-system/sw",
];

const MAX_UNREADABLE_GLOB_MATCHES: usize = 8192;

/// Options that control how bubblewrap is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BwrapOptions {
    /// Whether to mount a fresh `/proc` inside the sandbox.
    ///
    /// This is the secure default, but some restrictive container environments
    /// deny `--proc /proc`.
    pub mount_proc: bool,
    /// How networking should be configured inside the bubblewrap sandbox.
    pub network_mode: BwrapNetworkMode,
    /// Optional maximum depth for expanding unreadable glob patterns with ripgrep.
    ///
    /// Keep this uncapped by default so existing nested deny-read matches are
    /// masked before the sandboxed command starts.
    pub glob_scan_max_depth: Option<usize>,
}

impl Default for BwrapOptions {
    fn default() -> Self {
        Self {
            mount_proc: true,
            network_mode: BwrapNetworkMode::FullAccess,
            glob_scan_max_depth: None,
        }
    }
}

/// Network policy modes for bubblewrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BwrapNetworkMode {
    /// Keep access to the host network namespace.
    #[default]
    FullAccess,
    /// Remove access to the host network namespace.
    Isolated,
}

impl BwrapNetworkMode {
    fn should_unshare_network(self) -> bool {
        matches!(self, Self::Isolated)
    }
}

#[derive(Debug)]
pub struct BwrapArgs {
    pub args: Vec<String>,
    pub preserved_files: Vec<File>,
    pub synthetic_mount_targets: Vec<SyntheticMountTarget>,
    pub protected_create_targets: Vec<ProtectedCreateTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    dev: u64,
    ino: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        file_identity_from_metadata(metadata)
    }
}

#[cfg(unix)]
fn file_identity_from_metadata(metadata: &Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn file_identity_from_metadata(_metadata: &Metadata) -> FileIdentity {
    // Test-host compilation only: pre-existing-path identity uses dev/ino on
    // Unix, which stable std has no equivalent for on Windows. The
    // synthetic-mount paths that consult identity are exercised by `cfg(unix)`
    // tests only.
    FileIdentity { dev: 0, ino: 0 }
}

/// fd token consumed by bubblewrap's `--ro-bind-data`. The helper clears
/// CLOEXEC on preserved files before exec, so the numeric fd survives.
#[cfg(unix)]
fn preserved_fd_token(file: &File) -> String {
    use std::os::fd::AsRawFd;
    file.as_raw_fd().to_string()
}

#[cfg(not(unix))]
fn preserved_fd_token(_file: &File) -> String {
    // Test-host compilation only: `--ro-bind-data` argv is built but never
    // executed off Linux, and tests that reach this path are `cfg(unix)`.
    "0".to_string()
}

#[cfg(unix)]
fn os_string_from_bytes(bytes: Vec<u8>) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes)
}

#[cfg(not(unix))]
fn os_string_from_bytes(bytes: Vec<u8>) -> OsString {
    // Test-host compilation only: ripgrep output parsing is lossy here, but
    // the tests that exercise it are `cfg(unix)`.
    OsString::from(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    // Test-host compilation only: executable bits are a Unix concept.
    path.is_file()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntheticMountTargetKind {
    EmptyFile,
    EmptyDirectory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticMountTarget {
    path: PathBuf,
    kind: SyntheticMountTargetKind,
    // If an empty metadata path was already present, remember its inode so
    // cleanup does not delete a real pre-existing file or directory.
    pre_existing_path: Option<FileIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectedCreateTarget {
    path: PathBuf,
}

impl ProtectedCreateTarget {
    pub fn missing(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SyntheticMountTarget {
    pub fn missing(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyFile,
            pre_existing_path: None,
        }
    }

    pub fn missing_empty_directory(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyDirectory,
            pre_existing_path: None,
        }
    }

    pub fn existing_empty_file(path: &Path, metadata: &Metadata) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyFile,
            pre_existing_path: Some(FileIdentity::from_metadata(metadata)),
        }
    }

    fn existing_empty_directory(path: &Path, metadata: &Metadata) -> Self {
        Self {
            path: path.to_path_buf(),
            kind: SyntheticMountTargetKind::EmptyDirectory,
            pre_existing_path: Some(FileIdentity::from_metadata(metadata)),
        }
    }

    pub fn preserves_pre_existing_path(&self) -> bool {
        self.pre_existing_path.is_some()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> SyntheticMountTargetKind {
        self.kind
    }

    pub fn should_remove_after_bwrap(&self, metadata: &Metadata) -> bool {
        match self.kind {
            SyntheticMountTargetKind::EmptyFile => {
                if !metadata.file_type().is_file() || metadata.len() != 0 {
                    return false;
                }
            }
            SyntheticMountTargetKind::EmptyDirectory => {
                if !metadata.file_type().is_dir() {
                    return false;
                }
            }
        }

        match self.pre_existing_path {
            Some(pre_existing_path) => pre_existing_path != FileIdentity::from_metadata(metadata),
            None => true,
        }
    }
}

/// Whether `name` is a protected workspace metadata path name (`.git`,
/// `.agents`, `.nano`).
///
/// Provenance: codex_protocol::permissions::is_protected_metadata_name,
/// re-expressed over the ported policy engine's protected set.
fn is_protected_metadata_name(name: &std::ffi::OsStr) -> bool {
    PROTECTED_METADATA_PATH_NAMES
        .iter()
        .any(|metadata_name| name == std::ffi::OsStr::new(metadata_name))
}

/// Wrap a command with bubblewrap so the filesystem is read-only by default,
/// with explicit writable roots and read-only subpaths layered afterward.
///
/// When the policy grants full disk write access and full network access, this
/// returns `command` unchanged so we avoid unnecessary sandboxing overhead.
/// If network isolation is requested, we still wrap with bubblewrap so network
/// namespace restrictions apply while preserving full filesystem access.
pub fn create_bwrap_command_args(
    command: Vec<String>,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    sandbox_policy_cwd: &Path,
    command_cwd: &Path,
    options: BwrapOptions,
) -> Result<BwrapArgs> {
    let unreadable_globs =
        file_system_sandbox_policy.get_unreadable_globs_with_cwd(sandbox_policy_cwd);
    // Full disk write normally skips bwrap, but unreadable glob patterns still
    // need concrete bwrap masks for the matches expanded below.
    if file_system_sandbox_policy.has_full_disk_write_access() && unreadable_globs.is_empty() {
        return if options.network_mode == BwrapNetworkMode::FullAccess {
            Ok(BwrapArgs {
                args: command,
                preserved_files: Vec::new(),
                synthetic_mount_targets: Vec::new(),
                protected_create_targets: Vec::new(),
            })
        } else {
            Ok(create_bwrap_flags_full_filesystem(command, options))
        };
    }

    create_bwrap_flags(
        command,
        file_system_sandbox_policy,
        sandbox_policy_cwd,
        command_cwd,
        options,
    )
}

fn create_bwrap_flags_full_filesystem(command: Vec<String>, options: BwrapOptions) -> BwrapArgs {
    let mut args = vec![
        "--new-session".to_string(),
        "--die-with-parent".to_string(),
        "--bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        // Preserve nodev on the root bind while exposing only standard devices.
        "--dev".to_string(),
        "/dev".to_string(),
        // Restore shared memory without exposing other host devices.
        "--bind-try".to_string(),
        "/dev/shm".to_string(),
        "/dev/shm".to_string(),
        // Always enter a fresh user namespace so root inside a container does
        // not need ambient CAP_SYS_ADMIN to create the remaining namespaces.
        "--unshare-user".to_string(),
        "--unshare-pid".to_string(),
    ];
    if options.network_mode.should_unshare_network() {
        args.push("--unshare-net".to_string());
    }
    if options.mount_proc {
        args.push("--proc".to_string());
        args.push("/proc".to_string());
    }
    args.push("--".to_string());
    args.extend(command);
    BwrapArgs {
        args,
        preserved_files: Vec::new(),
        synthetic_mount_targets: Vec::new(),
        protected_create_targets: Vec::new(),
    }
}

/// Build the bubblewrap flags (everything after `argv[0]`).
fn create_bwrap_flags(
    command: Vec<String>,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    sandbox_policy_cwd: &Path,
    command_cwd: &Path,
    options: BwrapOptions,
) -> Result<BwrapArgs> {
    let BwrapArgs {
        args: filesystem_args,
        preserved_files,
        synthetic_mount_targets,
        protected_create_targets,
    } = create_filesystem_args(
        file_system_sandbox_policy,
        sandbox_policy_cwd,
        options
            .glob_scan_max_depth
            .or(file_system_sandbox_policy.glob_scan_max_depth),
    )?;
    let normalized_command_cwd = normalize_command_cwd_for_bwrap(command_cwd);
    let mut args = Vec::new();
    args.push("--new-session".to_string());
    args.push("--die-with-parent".to_string());
    args.extend(filesystem_args);
    // Request a user namespace explicitly rather than relying on bubblewrap's
    // auto-enable behavior, which is skipped when the caller runs as uid 0.
    args.push("--unshare-user".to_string());
    args.push("--unshare-pid".to_string());
    if options.network_mode.should_unshare_network() {
        args.push("--unshare-net".to_string());
    }
    // Mount a fresh /proc unless the caller explicitly disables it.
    if options.mount_proc {
        args.push("--proc".to_string());
        args.push("/proc".to_string());
    }
    if normalized_command_cwd.as_path() != command_cwd {
        // Bubblewrap otherwise inherits the helper's logical cwd, which can be
        // a symlink alias that disappears once the sandbox only mounts
        // canonical roots. Enter the canonical command cwd explicitly so
        // relative paths stay aligned with the mounted filesystem view.
        args.push("--chdir".to_string());
        args.push(path_to_string(normalized_command_cwd.as_path()));
    }
    args.push("--".to_string());
    args.extend(command);
    Ok(BwrapArgs {
        args,
        preserved_files,
        synthetic_mount_targets,
        protected_create_targets,
    })
}

/// Build the bubblewrap filesystem mounts for a given filesystem policy.
///
/// The mount order is important:
/// 1. Full-read policies, and restricted policies that explicitly read `/`,
///    use `--ro-bind / /`; other restricted-read policies start from
///    `--tmpfs /` and layer scoped `--ro-bind` mounts.
/// 2. `--dev /dev` mounts a minimal writable `/dev` with standard device nodes
///    (including `/dev/urandom`) even under a read-only root.
/// 3. Unreadable ancestors of writable roots are masked before their child
///    mounts are rebound so nested writable carveouts can be reopened safely.
/// 4. `--bind <root> <root>` re-enables writes for allowed roots, including
///    writable subpaths under `/dev` (for example, `/dev/shm`).
/// 5. `--ro-bind <subpath> <subpath>` re-applies read-only protections under
///    those writable roots so protected subpaths win.
/// 6. Nested unreadable carveouts under a writable root are masked after that
///    root is bound, and unrelated unreadable roots are masked afterward.
fn create_filesystem_args(
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    cwd: &Path,
    glob_scan_max_depth: Option<usize>,
) -> Result<BwrapArgs> {
    let unreadable_globs = file_system_sandbox_policy.get_unreadable_globs_with_cwd(cwd);
    // Bubblewrap requires bind mount targets to exist. Skip missing writable
    // roots so mixed-platform configs can keep harmless paths for other
    // environments without breaking Linux command startup.
    let mut writable_roots = file_system_sandbox_policy
        .get_writable_roots_with_cwd(cwd)
        .into_iter()
        .filter(|writable_root| writable_root.root.as_path().exists())
        .collect::<Vec<_>>();
    if writable_roots.is_empty()
        && file_system_sandbox_policy.has_full_disk_write_access()
        && !unreadable_globs.is_empty()
    {
        writable_roots.push(WritableRoot {
            root: AbsolutePathBuf::from_absolute_path("/")?,
            read_only_subpaths: Vec::new(),
            protected_metadata_names: Vec::new(),
        });
    }
    let missing_auto_metadata_read_only_project_root_subpaths: HashSet<PathBuf> =
        file_system_sandbox_policy
            .entries
            .iter()
            .filter(|entry| entry.access == FileSystemAccessMode::Read)
            .filter_map(|entry| {
                let subpath = match &entry.path {
                    FileSystemPath::Special {
                        value:
                            FileSystemSpecialPath::ProjectRoots {
                                subpath: Some(subpath),
                            },
                    } => subpath,
                    _ => return None,
                };
                // Automatic repo-metadata read masks are skipped here so the
                // metadata handling below can apply the root-scoped
                // protection consistently for `.git`, `.agents`, and `.nano`.
                // User-authored `read` rules for other subpaths and `none`
                // rules should keep their normal bwrap behavior, which can mask
                // the first missing component to prevent creation under writable
                // roots.
                let project_subpath = Path::new(subpath);
                if project_subpath != Path::new(".git")
                    && project_subpath != Path::new(".agents")
                    && project_subpath != Path::new(".nano")
                {
                    return None;
                }
                let resolved = AbsolutePathBuf::resolve_path_against_base(subpath, cwd);
                (!resolved.as_path().exists()).then(|| resolved.into_path_buf())
            })
            .collect();
    let mut unreadable_roots = file_system_sandbox_policy
        .get_unreadable_roots_with_cwd(cwd)
        .into_iter()
        .map(AbsolutePathBuf::into_path_buf)
        .collect::<Vec<_>>();
    // Bubblewrap can only mask concrete paths. Expand unreadable glob patterns
    // to the existing matches we can see before constructing the mount overlay;
    // core tool helpers still evaluate the original patterns directly at read time.
    unreadable_roots.extend(
        expand_unreadable_globs_with_ripgrep(&unreadable_globs, cwd, glob_scan_max_depth)?
            .into_iter()
            .map(AbsolutePathBuf::into_path_buf),
    );
    unreadable_roots.sort();
    unreadable_roots.dedup();

    let args = if file_system_sandbox_policy.has_full_disk_read_access() {
        // Read-only root, then mount a minimal device tree.
        // In bubblewrap (`bubblewrap.c`, `SETUP_MOUNT_DEV`), `--dev /dev`
        // creates the standard minimal nodes: null, zero, full, random,
        // urandom, and tty. `/dev` must be mounted before writable roots so
        // explicit `/dev/*` writable binds remain visible.
        vec![
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
        ]
    } else {
        // Start from an empty filesystem and add only the approved readable
        // roots plus a minimal `/dev`.
        let mut args = vec![
            "--tmpfs".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
        ];

        let mut readable_roots: BTreeSet<PathBuf> = file_system_sandbox_policy
            .get_readable_roots_with_cwd(cwd)
            .into_iter()
            .map(AbsolutePathBuf::into_path_buf)
            .collect();
        if file_system_sandbox_policy.include_platform_defaults() {
            readable_roots.extend(
                LINUX_PLATFORM_DEFAULT_READ_ROOTS
                    .iter()
                    .map(|path| PathBuf::from(*path))
                    .filter(|path| path.exists()),
            );
        }

        // A restricted policy can still explicitly request `/`, which is
        // the broad read baseline. Explicit unreadable carveouts are
        // re-applied later.
        if readable_roots.iter().any(|root| root == Path::new("/")) {
            args = vec![
                "--ro-bind".to_string(),
                "/".to_string(),
                "/".to_string(),
                "--dev".to_string(),
                "/dev".to_string(),
            ];
        } else {
            for root in readable_roots {
                if !root.exists() {
                    continue;
                }
                // Writable roots are rebound by real target below; mirror that
                // for their restricted-read bootstrap mount. Plain read-only
                // roots must stay logical because callers may execute those
                // paths inside bwrap, such as Bazel runfiles helper binaries.
                let mount_root = if writable_roots
                    .iter()
                    .any(|writable_root| root.starts_with(writable_root.root.as_path()))
                {
                    canonical_target_if_symlinked_path(&root).unwrap_or(root)
                } else {
                    root
                };
                args.push("--ro-bind".to_string());
                args.push(path_to_string(&mount_root));
                args.push(path_to_string(&mount_root));
            }
        }

        args
    };
    let mut bwrap_args = BwrapArgs {
        args,
        preserved_files: Vec::new(),
        synthetic_mount_targets: Vec::new(),
        protected_create_targets: Vec::new(),
    };
    let mut allowed_write_paths = Vec::with_capacity(writable_roots.len());
    for writable_root in &writable_roots {
        let root = writable_root.root.as_path();
        allowed_write_paths.push(root.to_path_buf());
        if let Some(target) = canonical_target_if_symlinked_path(root) {
            allowed_write_paths.push(target);
        }
    }
    let unreadable_paths: HashSet<PathBuf> = unreadable_roots.iter().cloned().collect();
    let mut sorted_writable_roots = writable_roots;
    sorted_writable_roots.sort_by_key(|writable_root| path_depth(writable_root.root.as_path()));
    // Mask only the unreadable ancestors that sit outside every writable root.
    // Unreadable paths nested under a broader writable root are applied after
    // that broader root is bound, then reopened by any deeper writable child.
    let mut unreadable_ancestors_of_writable_roots: Vec<PathBuf> = unreadable_roots
        .iter()
        .filter(|path| {
            let unreadable_root = path.as_path();
            !allowed_write_paths
                .iter()
                .any(|root| unreadable_root.starts_with(root))
                && allowed_write_paths
                    .iter()
                    .any(|root| root.starts_with(unreadable_root))
        })
        .cloned()
        .collect();
    unreadable_ancestors_of_writable_roots.sort_by_key(|path| path_depth(path));

    for unreadable_root in &unreadable_ancestors_of_writable_roots {
        append_unreadable_root_args(&mut bwrap_args, unreadable_root, &allowed_write_paths)?;
    }

    for writable_root in &sorted_writable_roots {
        let root = writable_root.root.as_path();
        let symlink_target = canonical_target_if_symlinked_path(root);
        // If a denied ancestor was already masked, recreate any missing mount
        // target parents before binding the narrower writable descendant.
        if let Some(masking_root) = unreadable_roots
            .iter()
            .map(PathBuf::as_path)
            .filter(|unreadable_root| root.starts_with(unreadable_root))
            .max_by_key(|unreadable_root| path_depth(unreadable_root))
        {
            append_mount_target_parent_dir_args(&mut bwrap_args.args, root, masking_root);
        }

        let mount_root = symlink_target.as_deref().unwrap_or(root);
        bwrap_args.args.push("--bind".to_string());
        bwrap_args.args.push(path_to_string(mount_root));
        bwrap_args.args.push(path_to_string(mount_root));

        let mut read_only_subpaths: Vec<PathBuf> = writable_root
            .read_only_subpaths
            .iter()
            .map(|path| path.as_path().to_path_buf())
            .filter(|path| !unreadable_paths.contains(path))
            .filter(|path| !missing_auto_metadata_read_only_project_root_subpaths.contains(path))
            .collect();
        let protected_metadata_names = writable_root.protected_metadata_names.clone();
        append_metadata_path_masks_for_writable_root(
            &mut read_only_subpaths,
            root,
            mount_root,
            &protected_metadata_names,
        );
        if let Some(target) = &symlink_target {
            read_only_subpaths = remap_paths_for_symlink_target(read_only_subpaths, root, target);
        }
        append_protected_create_targets_for_writable_root(
            &mut bwrap_args,
            &protected_metadata_names,
            root,
            symlink_target.as_deref(),
            &read_only_subpaths,
        );
        read_only_subpaths.sort_by_key(|path| path_depth(path));
        for subpath in read_only_subpaths {
            append_read_only_subpath_args(&mut bwrap_args, &subpath, &allowed_write_paths)?;
        }
        let mut nested_unreadable_roots: Vec<PathBuf> = unreadable_roots
            .iter()
            .filter(|path| path.starts_with(root))
            .cloned()
            .collect();
        if let Some(target) = &symlink_target {
            nested_unreadable_roots =
                remap_paths_for_symlink_target(nested_unreadable_roots, root, target);
        }
        nested_unreadable_roots.sort_by_key(|path| path_depth(path));
        for unreadable_root in nested_unreadable_roots {
            append_unreadable_root_args(&mut bwrap_args, &unreadable_root, &allowed_write_paths)?;
        }
    }

    let mut rootless_unreadable_roots: Vec<PathBuf> = unreadable_roots
        .iter()
        .filter(|path| {
            let unreadable_root = path.as_path();
            !allowed_write_paths
                .iter()
                .any(|root| unreadable_root.starts_with(root) || root.starts_with(unreadable_root))
        })
        .cloned()
        .collect();
    rootless_unreadable_roots.sort_by_key(|path| path_depth(path));
    for unreadable_root in rootless_unreadable_roots {
        append_unreadable_root_args(&mut bwrap_args, &unreadable_root, &allowed_write_paths)?;
    }

    Ok(bwrap_args)
}

fn append_protected_create_targets_for_writable_root(
    bwrap_args: &mut BwrapArgs,
    protected_metadata_names: &[String],
    root: &Path,
    symlink_target: Option<&Path>,
    read_only_subpaths: &[PathBuf],
) {
    for name in protected_metadata_names {
        let mut path = root.join(name);
        if let Some(target) = symlink_target
            && let Ok(relative_path) = path.strip_prefix(root)
        {
            path = target.join(relative_path);
        }
        if read_only_subpaths.iter().any(|subpath| subpath == &path) || path.exists() {
            continue;
        }
        bwrap_args
            .protected_create_targets
            .push(ProtectedCreateTarget::missing(&path));
    }
}

fn append_metadata_path_masks_for_writable_root(
    read_only_subpaths: &mut Vec<PathBuf>,
    root: &Path,
    mount_root: &Path,
    protected_metadata_names: &[String],
) {
    for name in protected_metadata_names {
        let path = root.join(name);
        if should_leave_missing_git_for_parent_repo_discovery(mount_root, name) {
            continue;
        }
        if !read_only_subpaths.iter().any(|subpath| subpath == &path) {
            read_only_subpaths.push(path);
        }
    }
}

fn should_leave_missing_git_for_parent_repo_discovery(mount_root: &Path, name: &str) -> bool {
    let path = mount_root.join(name);
    name == ".git"
        && matches!(
            path.symlink_metadata(),
            Err(err) if err.kind() == io::ErrorKind::NotFound
        )
        && mount_root
            .ancestors()
            .skip(1)
            .any(ancestor_has_git_metadata)
}

fn ancestor_has_git_metadata(ancestor: &Path) -> bool {
    let git_path = ancestor.join(".git");
    let Ok(metadata) = git_path.symlink_metadata() else {
        return false;
    };
    if metadata.is_dir() {
        return git_path.join("HEAD").symlink_metadata().is_ok();
    }
    if metadata.is_file() {
        return fs::read_to_string(git_path)
            .is_ok_and(|contents| contents.trim_start().starts_with("gitdir:"));
    }
    false
}

fn expand_unreadable_globs_with_ripgrep(
    patterns: &[String],
    cwd: &Path,
    max_depth: Option<usize>,
) -> Result<Vec<AbsolutePathBuf>> {
    if patterns.is_empty() || max_depth == Some(0) {
        return Ok(Vec::new());
    }

    // Group each pattern by the static path prefix before its first glob
    // metacharacter. That keeps scans narrow, avoids searching from `/`, and
    // lets one `rg --files` call handle all patterns under the same root.
    let mut patterns_by_search_root: BTreeMap<AbsolutePathBuf, Vec<String>> = BTreeMap::new();
    for pattern in patterns {
        if let Some((search_root, glob)) = split_pattern_for_ripgrep(pattern, cwd)
            && search_root.as_path().is_dir()
        {
            patterns_by_search_root
                .entry(search_root)
                .or_default()
                .push(glob);
        }
    }

    // Record both the logical match and any canonical symlink target. The bwrap
    // overlay needs the resolved target to prevent a readable symlink path from
    // bypassing an unreadable glob match.
    let mut expanded_paths = BTreeSet::new();
    for (search_root, globs) in patterns_by_search_root {
        for path in ripgrep_files(search_root.as_path(), &globs, max_depth)? {
            if let Some(target) = canonical_target_if_symlinked_path(path.as_path()) {
                expanded_paths.insert(AbsolutePathBuf::from_absolute_path_checked(target)?);
            }
            expanded_paths.insert(path);
            if expanded_paths.len() > MAX_UNREADABLE_GLOB_MATCHES {
                anyhow::bail!(
                    "unreadable glob expansion for {} matched more than {MAX_UNREADABLE_GLOB_MATCHES} paths",
                    search_root.display()
                );
            }
        }
    }

    Ok(expanded_paths.into_iter().collect())
}

fn split_pattern_for_ripgrep(pattern: &str, cwd: &Path) -> Option<(AbsolutePathBuf, String)> {
    // Resolve relative patterns once, then split at the first glob
    // metacharacter. The prefix becomes the search root and the suffix stays as
    // the ripgrep glob. Root-level glob scans are intentionally skipped because
    // they are too broad for startup-time sandbox construction.
    let absolute_pattern = AbsolutePathBuf::resolve_path_against_base(pattern, cwd);
    let pattern = absolute_pattern.to_string_lossy();
    let first_glob_index = pattern
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '*' | '?' | '[' | ']').then_some(index))?;
    let static_prefix = &pattern[..first_glob_index];
    if static_prefix.is_empty() || static_prefix == "/" {
        return None;
    }
    let search_root_end = if static_prefix.ends_with('/') {
        static_prefix.len() - 1
    } else {
        static_prefix.rfind('/').unwrap_or(0)
    };
    let search_root = if search_root_end == 0 {
        PathBuf::from("/")
    } else {
        PathBuf::from(&pattern[..search_root_end])
    };
    let search_root = AbsolutePathBuf::from_absolute_path_checked(search_root).ok()?;
    let glob = escape_unclosed_glob_classes(&pattern[search_root_end + 1..]);
    (!glob.is_empty()).then_some((search_root, glob))
}

fn escape_unclosed_glob_classes(glob: &str) -> String {
    // The filesystem policy accepts an unclosed `[` as a literal. Ripgrep treats
    // that as invalid glob syntax, so escape only the unclosed class opener.
    let mut escaped = String::with_capacity(glob.len());
    let mut chars = glob.chars();

    while let Some(ch) = chars.next() {
        if ch != '[' {
            escaped.push(ch);
            continue;
        }

        let mut class = String::new();
        let mut closed = false;
        for class_ch in chars.by_ref() {
            if class_ch == ']' {
                closed = true;
                break;
            }
            class.push(class_ch);
        }

        if closed {
            escaped.push('[');
            escaped.push_str(&class);
            escaped.push(']');
        } else {
            escaped.push_str(r"\[");
            escaped.push_str(&class);
        }
    }

    escaped
}

fn ripgrep_files(
    search_root: &Path,
    globs: &[String],
    max_depth: Option<usize>,
) -> Result<Vec<AbsolutePathBuf>> {
    // Use `rg --files` rather than shell expansion so dotfiles and ignored files
    // are still considered. A status 1 with no stderr is ripgrep's "no matches"
    // case, not a sandbox construction error.
    let mut command = Command::new("rg");
    command
        .arg("--files")
        .arg("--hidden")
        .arg("--no-ignore")
        .arg("--null");
    if let Some(max_depth) = max_depth {
        command.arg("--max-depth").arg(max_depth.to_string());
    }
    for glob in globs {
        command.arg("--glob").arg(glob);
    }
    command.arg("--").arg(search_root);

    /*
     * Prefer ripgrep for unreadable glob expansion because it is fast and
     * already implements the file-walking semantics we want here: include
     * dotfiles, ignore ignore files, and do not recurse through symlinked
     * directories. If `rg` is not installed in the runtime environment, fall
     * back to the internal globset walker so sandbox construction still masks
     * matching paths. Other ripgrep failures stay fatal so deny-read does not
     * silently weaken.
     */
    let output = match command.output() {
        Ok(output) => output,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return glob_files(search_root, globs, max_depth);
        }
        Err(err) => return Err(err.into()),
    };
    if !output.status.success() {
        if output.status.code() == Some(1) && output.stderr.is_empty() {
            return Ok(Vec::new());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "ripgrep unreadable glob scan failed for {}: {stderr}",
            search_root.display()
        );
    }

    let paths = output
        .stdout
        .split(|byte| *byte == b'\0')
        .filter(|path| !path.is_empty())
        .map(|path| {
            let path = PathBuf::from(os_string_from_bytes(path.to_vec()));
            if path.is_absolute() {
                path
            } else {
                search_root.join(path)
            }
        })
        .map(AbsolutePathBuf::from_absolute_path_checked)
        .collect::<io::Result<Vec<_>>>()?;
    Ok(paths)
}

fn glob_files(
    search_root: &Path,
    globs: &[String],
    max_depth: Option<usize>,
) -> Result<Vec<AbsolutePathBuf>> {
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        let glob = GlobBuilder::new(glob)
            .literal_separator(true)
            .allow_unclosed_class(true)
            .build()
            .map_err(|err| {
                anyhow::anyhow!(
                    "unreadable glob pattern is invalid for {}: {err}",
                    search_root.display()
                )
            })?;
        builder.add(glob);
    }
    let glob_set = builder.build().map_err(|err| {
        anyhow::anyhow!(
            "unreadable glob matcher failed for {}: {err}",
            search_root.display()
        )
    })?;

    let mut paths = Vec::new();
    collect_glob_files(search_root, search_root, &glob_set, max_depth, &mut paths)?;
    Ok(paths)
}

fn collect_glob_files(
    search_root: &Path,
    dir: &Path,
    glob_set: &GlobSet,
    remaining_depth: Option<usize>,
    paths: &mut Vec<AbsolutePathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let relative = path.strip_prefix(search_root).unwrap_or(path.as_path());

        if (file_type.is_file() || file_type.is_symlink()) && glob_set.is_match(relative) {
            paths.push(AbsolutePathBuf::from_absolute_path_checked(&path)?);
        }

        if !file_type.is_dir() {
            continue;
        }
        let remaining_depth = match remaining_depth {
            Some(0 | 1) => continue,
            Some(depth) => Some(depth - 1),
            None => None,
        };
        collect_glob_files(search_root, &path, glob_set, remaining_depth, paths)?;
    }
    Ok(())
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

fn canonical_target_if_symlinked_path(path: &Path) -> Option<PathBuf> {
    // Return the fully resolved target only when some path component is a
    // symlink. Callers use this to bind/mask the real filesystem location while
    // leaving ordinary paths in their logical form.
    let mut current = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => return None,
        };
        if metadata.file_type().is_symlink() {
            let target = fs::canonicalize(path).ok()?;
            if target.as_path() == path {
                return None;
            }
            return Some(target);
        }
    }
    None
}

fn remap_paths_for_symlink_target(paths: Vec<PathBuf>, root: &Path, target: &Path) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| {
            if let Ok(relative) = path.strip_prefix(root) {
                target.join(relative)
            } else {
                path
            }
        })
        .collect()
}

fn normalize_command_cwd_for_bwrap(command_cwd: &Path) -> PathBuf {
    command_cwd
        .canonicalize()
        .unwrap_or_else(|_| command_cwd.to_path_buf())
}

fn append_mount_target_parent_dir_args(args: &mut Vec<String>, mount_target: &Path, anchor: &Path) {
    let mount_target_dir = if mount_target.is_dir() {
        mount_target
    } else if let Some(parent) = mount_target.parent() {
        parent
    } else {
        return;
    };
    let mut mount_target_dirs: Vec<PathBuf> = mount_target_dir
        .ancestors()
        .take_while(|path| *path != anchor)
        .map(Path::to_path_buf)
        .collect();
    mount_target_dirs.reverse();
    for mount_target_dir in mount_target_dirs {
        args.push("--dir".to_string());
        args.push(path_to_string(&mount_target_dir));
    }
}

fn append_read_only_subpath_args(
    bwrap_args: &mut BwrapArgs,
    subpath: &Path,
    allowed_write_paths: &[PathBuf],
) -> Result<()> {
    if let Some(symlink) = first_writable_symlink_component_in_path(subpath, allowed_write_paths) {
        /*
         * A read-only carveout under a writable symlink cannot be made reliable
         * with bwrap path arguments. Binding the symlink's current target would
         * only protect a startup-time snapshot; the sandboxed process could
         * replace the writable symlink before it reads through the logical path.
         */
        anyhow::bail!(
            "cannot enforce sandbox read-only path {} because it crosses writable symlink {}",
            subpath.display(),
            symlink.display()
        );
    }

    if let Some(metadata) = transient_empty_metadata_path(subpath)
        && is_within_allowed_write_paths(subpath, allowed_write_paths)
    {
        // Another concurrent bwrap setup can leave an empty mount target at
        // a missing metadata path. Treat it like the missing case instead of
        // binding that transient host path as the stable source.
        match metadata {
            EmptyProtectedMetadataPath::File(metadata) => {
                append_existing_empty_file_bind_data_args(bwrap_args, subpath, &metadata)?;
            }
            EmptyProtectedMetadataPath::Directory(metadata) => {
                append_existing_empty_directory_args(bwrap_args, subpath, &metadata);
            }
        }
        return Ok(());
    }

    if !subpath.exists() {
        if let Some(first_missing_component) = find_first_non_existent_component(subpath)
            && is_within_allowed_write_paths(&first_missing_component, allowed_write_paths)
        {
            append_missing_read_only_subpath_args(bwrap_args, &first_missing_component)?;
        }
        return Ok(());
    }

    if is_within_allowed_write_paths(subpath, allowed_write_paths) {
        bwrap_args.args.push("--ro-bind".to_string());
        bwrap_args.args.push(path_to_string(subpath));
        bwrap_args.args.push(path_to_string(subpath));
    }
    Ok(())
}

fn append_empty_file_bind_data_args(bwrap_args: &mut BwrapArgs, path: &Path) -> Result<()> {
    if bwrap_args.preserved_files.is_empty() {
        bwrap_args.preserved_files.push(File::open("/dev/null")?);
    }
    let null_fd = preserved_fd_token(&bwrap_args.preserved_files[0]);
    bwrap_args.args.push("--ro-bind-data".to_string());
    bwrap_args.args.push(null_fd);
    bwrap_args.args.push(path_to_string(path));
    Ok(())
}

fn append_empty_directory_args(bwrap_args: &mut BwrapArgs, path: &Path) {
    bwrap_args.args.push("--perms".to_string());
    bwrap_args.args.push("555".to_string());
    bwrap_args.args.push("--tmpfs".to_string());
    bwrap_args.args.push(path_to_string(path));
    bwrap_args.args.push("--remount-ro".to_string());
    bwrap_args.args.push(path_to_string(path));
}

fn append_missing_read_only_subpath_args(bwrap_args: &mut BwrapArgs, path: &Path) -> Result<()> {
    if path.file_name().is_some_and(is_protected_metadata_name) {
        append_empty_directory_args(bwrap_args, path);
        bwrap_args
            .synthetic_mount_targets
            .push(SyntheticMountTarget::missing_empty_directory(path));
        return Ok(());
    }

    append_missing_empty_file_bind_data_args(bwrap_args, path)
}

fn append_missing_empty_file_bind_data_args(bwrap_args: &mut BwrapArgs, path: &Path) -> Result<()> {
    append_empty_file_bind_data_args(bwrap_args, path)?;
    bwrap_args
        .synthetic_mount_targets
        .push(SyntheticMountTarget::missing(path));
    Ok(())
}

fn append_existing_empty_file_bind_data_args(
    bwrap_args: &mut BwrapArgs,
    path: &Path,
    metadata: &Metadata,
) -> Result<()> {
    append_empty_file_bind_data_args(bwrap_args, path)?;
    bwrap_args
        .synthetic_mount_targets
        .push(SyntheticMountTarget::existing_empty_file(path, metadata));
    Ok(())
}

fn append_existing_empty_directory_args(
    bwrap_args: &mut BwrapArgs,
    path: &Path,
    metadata: &Metadata,
) {
    append_empty_directory_args(bwrap_args, path);
    bwrap_args
        .synthetic_mount_targets
        .push(SyntheticMountTarget::existing_empty_directory(
            path, metadata,
        ));
}

fn append_unreadable_root_args(
    bwrap_args: &mut BwrapArgs,
    unreadable_root: &Path,
    allowed_write_paths: &[PathBuf],
) -> Result<()> {
    if let Some(symlink) =
        first_writable_symlink_component_in_path(unreadable_root, allowed_write_paths)
    {
        /*
         * Deny-read masks must fail closed when the protected path crosses a
         * symlink that remains writable to the sandboxed process. Resolving and
         * masking the symlink's current target is a TOCTTOU snapshot: bwrap would
         * protect the old target while the logical path could later point
         * somewhere else.
         */
        anyhow::bail!(
            "cannot enforce sandbox deny-read path {} because it crosses writable symlink {}",
            unreadable_root.display(),
            symlink.display()
        );
    }

    if !unreadable_root.exists() {
        if let Some(first_missing_component) = find_first_non_existent_component(unreadable_root)
            && is_within_allowed_write_paths(&first_missing_component, allowed_write_paths)
        {
            append_missing_empty_file_bind_data_args(bwrap_args, &first_missing_component)?;
        }
        return Ok(());
    }

    append_existing_unreadable_path_args(bwrap_args, unreadable_root, allowed_write_paths)
}

fn append_existing_unreadable_path_args(
    bwrap_args: &mut BwrapArgs,
    unreadable_root: &Path,
    allowed_write_paths: &[PathBuf],
) -> Result<()> {
    if unreadable_root.is_dir() {
        let mut writable_descendants: Vec<&Path> = allowed_write_paths
            .iter()
            .map(PathBuf::as_path)
            .filter(|path| *path != unreadable_root && path.starts_with(unreadable_root))
            .collect();
        bwrap_args.args.push("--perms".to_string());
        // Execute-only perms let the process traverse into explicitly
        // re-opened writable descendants while still hiding the denied
        // directory contents. Plain denied directories with no writable child
        // mounts stay at `000`.
        bwrap_args.args.push(if writable_descendants.is_empty() {
            "000".to_string()
        } else {
            "111".to_string()
        });
        bwrap_args.args.push("--tmpfs".to_string());
        bwrap_args.args.push(path_to_string(unreadable_root));
        // Recreate any writable descendants inside the tmpfs before remounting
        // the denied parent read-only. Otherwise bubblewrap cannot mkdir the
        // nested mount targets after the parent has been frozen.
        writable_descendants.sort_by_key(|path| path_depth(path));
        for writable_descendant in writable_descendants {
            append_mount_target_parent_dir_args(
                &mut bwrap_args.args,
                writable_descendant,
                unreadable_root,
            );
        }
        bwrap_args.args.push("--remount-ro".to_string());
        bwrap_args.args.push(path_to_string(unreadable_root));
        return Ok(());
    }

    bwrap_args.args.push("--perms".to_string());
    bwrap_args.args.push("000".to_string());
    append_empty_file_bind_data_args(bwrap_args, unreadable_root)
}

/// Returns true when `path` is under any allowed writable root.
fn is_within_allowed_write_paths(path: &Path, allowed_write_paths: &[PathBuf]) -> bool {
    allowed_write_paths
        .iter()
        .any(|root| path.starts_with(root))
}

enum EmptyProtectedMetadataPath {
    File(Metadata),
    Directory(Metadata),
}

fn transient_empty_metadata_path(path: &Path) -> Option<EmptyProtectedMetadataPath> {
    if !path.file_name().is_some_and(is_protected_metadata_name) {
        return None;
    }

    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_file() && metadata.len() == 0 {
        return Some(EmptyProtectedMetadataPath::File(metadata));
    }

    if metadata.file_type().is_dir() && directory_is_empty(path) {
        return Some(EmptyProtectedMetadataPath::Directory(metadata));
    }

    None
}

fn directory_is_empty(path: &Path) -> bool {
    let Ok(mut entries) = fs::read_dir(path) else {
        return false;
    };
    entries.next().is_none()
}

fn first_writable_symlink_component_in_path(
    target_path: &Path,
    allowed_write_paths: &[PathBuf],
) -> Option<PathBuf> {
    /*
     * Walk the logical path and report the first symlink component that lives
     * under a writable root. These symlinks are mutable from inside the sandbox,
     * so any mount or mask based on their resolved target would be racing a path
     * the sandboxed process can change.
     */
    let mut current = PathBuf::new();

    for component in target_path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(_) => break,
        };

        if metadata.file_type().is_symlink()
            && is_within_allowed_write_paths(&current, allowed_write_paths)
        {
            return Some(current);
        }
    }

    None
}

/// Find the first missing path component while walking `target_path`.
///
/// Mounting `/dev/null` on the first missing component prevents the sandboxed
/// process from creating the protected path hierarchy.
fn find_first_non_existent_component(target_path: &Path) -> Option<PathBuf> {
    let mut current = PathBuf::new();

    for component in target_path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {
                current.push(Path::new("/"));
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::Normal(part) => current.push(part),
            Component::Prefix(_) => continue,
        }

        if !current.exists() {
            return Some(current);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// System bwrap discovery + WSL1 detection (donor: codex-rs/sandboxing/src/bwrap.rs)
// ---------------------------------------------------------------------------

const SYSTEM_BWRAP_PROGRAM: &str = "bwrap";
const MISSING_BWRAP_WARNING: &str = concat!(
    "Wayland Nano could not find bubblewrap on PATH. ",
    "Install bubblewrap with your OS package manager. ",
    "Wayland Nano will use the bundled bubblewrap in the meantime.",
);
const USER_NAMESPACE_WARNING: &str =
    "Wayland Nano's Linux sandbox uses bubblewrap and needs access to create user namespaces.";
pub const WSL1_BWRAP_WARNING: &str = concat!(
    "Wayland Nano's Linux sandbox uses bubblewrap, which is not supported on WSL1 ",
    "because WSL1 cannot create the required user namespaces. ",
    "Use WSL2 for sandboxed shell commands."
);
#[cfg(target_os = "linux")]
const USER_NAMESPACE_FAILURES: [&str; 4] = [
    "loopback: Failed RTM_NEWADDR",
    "loopback: Failed RTM_NEWLINK",
    "setting up uid map: Permission denied",
    "No permissions to create a new namespace",
];
const SYSTEM_BWRAP_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(target_os = "linux")]
const SYSTEM_BWRAP_PROBE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
#[cfg(target_os = "linux")]
const SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES: u64 = 64 * 1024;

/// Returns a user-facing warning when the bubblewrap-based Linux sandbox is
/// degraded or unavailable for the given permission profile.
pub fn system_bwrap_warning(permission_profile: &PermissionProfile) -> Option<String> {
    if !should_warn_about_system_bwrap(permission_profile) {
        return None;
    }

    let system_bwrap_path = find_system_bwrap_in_path();
    system_bwrap_warning_for_path(system_bwrap_path.as_deref())
}

fn should_warn_about_system_bwrap(permission_profile: &PermissionProfile) -> bool {
    let (file_system_policy, network_policy) = permission_profile.to_runtime_permissions();
    should_require_platform_sandbox(&file_system_policy, network_policy)
}

/// Provenance: codex `sandboxing/src/policy_transforms.rs`
/// `should_require_platform_sandbox` @ 646f7c0a, with the managed-network
/// parameter dropped (nano-egress owns egress; the donor call chain for this
/// warning always passes `false`).
fn should_require_platform_sandbox(
    file_system_policy: &FileSystemSandboxPolicy,
    network_policy: NetworkSandboxPolicy,
) -> bool {
    if !network_policy.is_enabled() {
        return !matches!(
            file_system_policy.kind,
            FileSystemSandboxKind::ExternalSandbox
        );
    }

    match file_system_policy.kind {
        FileSystemSandboxKind::Restricted => !file_system_policy.has_full_disk_write_access(),
        FileSystemSandboxKind::Unrestricted | FileSystemSandboxKind::ExternalSandbox => false,
    }
}

fn system_bwrap_warning_for_path(system_bwrap_path: Option<&Path>) -> Option<String> {
    if is_wsl1() {
        return Some(WSL1_BWRAP_WARNING.to_string());
    }

    let Some(system_bwrap_path) = system_bwrap_path else {
        return Some(MISSING_BWRAP_WARNING.to_string());
    };

    if !system_bwrap_has_user_namespace_access(system_bwrap_path, SYSTEM_BWRAP_PROBE_TIMEOUT) {
        return Some(USER_NAMESPACE_WARNING.to_string());
    }

    None
}

#[cfg(target_os = "linux")]
fn system_bwrap_has_user_namespace_access(
    system_bwrap_path: &Path,
    timeout: std::time::Duration,
) -> bool {
    use std::io::ErrorKind;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::process::Output;
    use std::process::Stdio;

    let mut child = match Command::new(system_bwrap_path)
        .args([
            "--unshare-user",
            "--unshare-net",
            "--ro-bind",
            "/",
            "/",
            "/bin/true",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return true,
    };

    let mut stderr = child.stderr.take();
    if let Some(stderr_pipe) = stderr.as_ref() {
        let fd = stderr_pipe.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            stderr = None;
        }
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut stderr_bytes = Vec::new();
    let mut exited_status = None;
    loop {
        let stderr_closed = stderr.as_mut().is_none_or(|stderr| {
            while stderr_bytes.len() < SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES as usize {
                let remaining = SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES as usize - stderr_bytes.len();
                let mut buffer = [0_u8; 4096];
                let read_limit = remaining.min(buffer.len());
                match stderr.read(&mut buffer[..read_limit]) {
                    Ok(0) => return true,
                    Ok(read) => stderr_bytes.extend_from_slice(&buffer[..read]),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => return false,
                    Err(_) => return true,
                }
            }
            true
        });

        if exited_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exited_status = Some(status),
                Ok(None) => {}
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return true;
                }
            }
        }

        if let Some(status) = exited_status {
            if status.success() {
                return true;
            }
            let output = Output {
                status,
                stdout: Vec::new(),
                stderr: stderr_bytes.clone(),
            };
            if is_user_namespace_failure(&output) {
                return false;
            }
            if stderr_closed {
                let output = Output {
                    status,
                    stdout: Vec::new(),
                    stderr: stderr_bytes,
                };
                return !is_user_namespace_failure(&output);
            }
        }

        let now = std::time::Instant::now();
        if now >= deadline {
            if exited_status.is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return true;
        }
        std::thread::sleep(SYSTEM_BWRAP_PROBE_POLL_INTERVAL.min(deadline - now));
    }
}

#[cfg(not(target_os = "linux"))]
fn system_bwrap_has_user_namespace_access(
    _system_bwrap_path: &Path,
    _timeout: std::time::Duration,
) -> bool {
    // Test-host compilation only: Linux user namespaces do not exist off
    // Linux, so the probe is treated as passed. The probe tests are
    // `cfg(target_os = "linux")`.
    true
}

pub fn is_wsl1() -> bool {
    std::fs::read_to_string("/proc/version")
        .is_ok_and(|proc_version| proc_version_indicates_wsl1(&proc_version))
}

fn proc_version_indicates_wsl1(proc_version: &str) -> bool {
    let proc_version = proc_version.to_ascii_lowercase();
    let mut remaining = proc_version.as_str();
    while let Some(marker) = remaining.find("wsl") {
        let version_start = marker + "wsl".len();
        let version_digits: String = remaining[version_start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(version) = version_digits.parse::<u32>() {
            return version == 1;
        }
        remaining = &remaining[version_start..];
    }

    proc_version.contains("microsoft") && !proc_version.contains("microsoft-standard")
}

#[cfg(target_os = "linux")]
fn is_user_namespace_failure(output: &std::process::Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    USER_NAMESPACE_FAILURES
        .iter()
        .any(|failure| stderr.contains(failure))
}

/// Locates a `bwrap` executable on PATH, skipping workspace-local candidates
/// under the current directory so a sandboxed workspace cannot shadow the
/// system binary.
pub fn find_system_bwrap_in_path() -> Option<PathBuf> {
    let search_path = std::env::var_os("PATH")?;
    let cwd = std::env::current_dir().ok()?;
    find_system_bwrap_in_search_paths(std::env::split_paths(&search_path), &cwd)
}

fn find_system_bwrap_in_search_paths(
    search_paths: impl IntoIterator<Item = PathBuf>,
    cwd: &Path,
) -> Option<PathBuf> {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let cwd_is_root = cwd.parent().is_none();
    search_paths
        .into_iter()
        .map(|dir| dir.join(SYSTEM_BWRAP_PROGRAM))
        .filter(|path| is_executable_file(path))
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .find(|path| cwd_is_root || !path.starts_with(&cwd))
}

#[cfg(test)]
#[path = "linux_bwrap_tests.rs"]
mod tests;
