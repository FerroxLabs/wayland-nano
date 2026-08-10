//! Provenance: ported from Codex `codex-rs/sandboxing/src/seatbelt_tests.rs`
//! @ 646f7c0a. Transformations:
//! - legacy `SandboxPolicy` constructions replaced by direct
//!   `FileSystemSandboxPolicy` / `policy_engine::workspace_write_policy`
//!   construction (no legacy config layer in nano-k3);
//! - managed-network-proxy tests DROPPED with the proxy surface itself;
//!   the remaining network tests assert the Restricted/Enabled split and
//!   fail-closed restricted behavior;
//! - `.codex` -> `.nano` (recorded branding, matches policy_engine);
//! - tests that only build strings run on every host; tests bound to
//!   unix path semantics are `cfg(unix)`; tests that spawn
//!   `/usr/bin/sandbox-exec` are `cfg(target_os = "macos")`.

use super::CreateSeatbeltCommandArgsParams;
use super::MACOS_SEATBELT_BASE_POLICY;
use super::UnixDomainSocketPolicy;
#[cfg(unix)]
use super::build_seatbelt_unreadable_glob_policy;
use super::create_seatbelt_command_args;
use super::dynamic_network_policy_for_network;
use super::normalize_path_for_sandbox;
use super::seatbelt_regex_for_unreadable_glob;
use super::unix_socket_dir_params;
use super::unix_socket_policy;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::FileSystemAccessMode;
use nano_core::permissions::FileSystemPath;
use nano_core::permissions::FileSystemSandboxEntry;
use nano_core::permissions::FileSystemSandboxPolicy;
#[cfg(unix)]
use nano_core::permissions::FileSystemSpecialPath;
use nano_core::permissions::NetworkSandboxPolicy;
use nano_core::policy_engine::PROTECTED_METADATA_PATH_NAMES;
use nano_core::policy_engine::workspace_write_policy;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

/// The engine normalizes with `dunce::canonicalize`; mirror that here so
/// expectations match on every host (no `\\?\` verbatim prefixes).
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).expect("canonicalize")
}

fn absolute_path(path: &Path) -> AbsolutePathBuf {
    AbsolutePathBuf::from_absolute_path(path).expect("absolute path")
}

fn seatbelt_policy_arg(args: &[String]) -> &str {
    let policy_index = args
        .iter()
        .position(|arg| arg == "-p")
        .expect("seatbelt args should include -p");
    args.get(policy_index + 1)
        .expect("seatbelt args should include policy text")
}

fn seatbelt_protected_metadata_name_requirements(root: &Path) -> String {
    let mut root = root.to_string_lossy().to_string();
    while root.len() > 1 && root.ends_with('/') {
        root.pop();
    }
    let root = regex_lite::escape(&root);
    PROTECTED_METADATA_PATH_NAMES
        .iter()
        .map(|name| {
            let name = regex_lite::escape(name);
            if root == "/" {
                format!(r#"(require-not (regex #"^/{name}(/.*)?$"))"#)
            } else {
                format!(r#"(require-not (regex #"^{root}/{name}(/.*)?$"))"#)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn seatbelt_args_for(
    command: Vec<String>,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    network_sandbox_policy: NetworkSandboxPolicy,
    sandbox_policy_cwd: &Path,
    extra_allow_unix_sockets: &[AbsolutePathBuf],
) -> Vec<String> {
    create_seatbelt_command_args(CreateSeatbeltCommandArgsParams {
        command,
        file_system_sandbox_policy,
        network_sandbox_policy,
        sandbox_policy_cwd,
        extra_allow_unix_sockets,
    })
    .expect("seatbelt args")
}

#[test]
fn base_policy_allows_node_cpu_sysctls() {
    assert!(
        MACOS_SEATBELT_BASE_POLICY.contains("(sysctl-name \"machdep.cpu.brand_string\")"),
        "base policy must allow CPU brand lookup for os.cpus()"
    );
    assert!(
        MACOS_SEATBELT_BASE_POLICY.contains("(sysctl-name \"hw.model\")"),
        "base policy must allow hardware model lookup for os.cpus()"
    );
}

#[test]
fn base_policy_allows_kmp_registration_shm_read_create_and_unlink() {
    let expected = r##"(allow ipc-posix-shm-read-data
  ipc-posix-shm-write-create
  ipc-posix-shm-write-unlink
  (ipc-posix-name-regex #"^/__KMP_REGISTERED_LIB_[0-9]+$"))"##;

    assert!(
        MACOS_SEATBELT_BASE_POLICY.contains(expected),
        "base policy must allow only KMP registration shm read/create/unlink:\n{MACOS_SEATBELT_BASE_POLICY}"
    );
}

#[test]
fn dynamic_network_policy_allows_tls_without_darwin_user_cache_write() {
    let policy = dynamic_network_policy_for_network(
        NetworkSandboxPolicy::Enabled,
        &UnixDomainSocketPolicy::default(),
    );

    assert!(
        policy.contains("(global-name \"com.apple.trustd.agent\")"),
        "policy should keep trustd agent access for TLS certificate verification:\n{policy}"
    );
    assert!(
        !policy.contains("DARWIN_USER_CACHE_DIR"),
        "network policy should not grant broad user cache writes:\n{policy}"
    );
}

#[test]
fn dynamic_network_policy_restricted_without_unix_sockets_fails_closed() {
    let policy = dynamic_network_policy_for_network(
        NetworkSandboxPolicy::Restricted,
        &UnixDomainSocketPolicy::default(),
    );

    assert_eq!(
        policy, "",
        "restricted network with no unix-socket allowance must not emit any network rules"
    );
}

#[test]
fn dynamic_network_policy_enabled_grants_full_network() {
    let policy = dynamic_network_policy_for_network(
        NetworkSandboxPolicy::Enabled,
        &UnixDomainSocketPolicy::default(),
    );

    assert!(
        policy.contains("(allow network-outbound)\n"),
        "policy should preserve full outbound network access:\n{policy}"
    );
    assert!(
        policy.contains("(allow network-inbound)\n"),
        "policy should preserve full inbound network access:\n{policy}"
    );
}

#[test]
fn create_seatbelt_args_allows_all_unix_sockets_when_enabled() {
    let policy = dynamic_network_policy_for_network(
        NetworkSandboxPolicy::Restricted,
        &UnixDomainSocketPolicy::AllowAll,
    );

    assert!(
        policy.contains("(allow system-socket (socket-domain AF_UNIX))"),
        "policy should allow AF_UNIX socket creation when unix sockets are enabled:\n{policy}"
    );
    assert!(
        policy.contains("(allow network-bind (local unix-socket))"),
        "policy should allow binding unix sockets when enabled:\n{policy}"
    );
    assert!(
        policy.contains("(allow network-outbound (remote unix-socket))"),
        "policy should allow connecting to unix sockets when enabled:\n{policy}"
    );
    assert!(
        !policy.contains("(allow network* (subpath"),
        "policy should no longer use the generic subpath unix-socket rules:\n{policy}"
    );
}

#[test]
fn unreadable_globstar_slash_matches_zero_or_more_directories() {
    let regex = seatbelt_regex_for_unreadable_glob("/tmp/repo/**/*.env");
    assert_eq!(regex.as_deref(), Some(r"^/tmp/repo/(.*/)?[^/]*\.env$"));
    let regex = regex_lite::Regex::new(regex.as_deref().expect("glob should compile"))
        .expect("regex should compile");

    assert!(regex.is_match("/tmp/repo/.env"));
    assert!(regex.is_match("/tmp/repo/app/.env"));
    assert!(regex.is_match("/tmp/repo/app/config.env"));
    assert!(!regex.is_match("/tmp/repo/app/config.toml"));
}

#[test]
fn unreadable_globs_use_git_style_component_matching() {
    let regex = seatbelt_regex_for_unreadable_glob("/tmp/repo/*/file[0-9]?.txt");
    assert_eq!(
        regex.as_deref(),
        Some(r"^/tmp/repo/[^/]*/file[0-9][^/]\.txt$")
    );
    let regex = regex_lite::Regex::new(regex.as_deref().expect("glob should compile"))
        .expect("regex should compile");

    assert!(regex.is_match("/tmp/repo/app/file42.txt"));
    assert!(!regex.is_match("/tmp/repo/app/nested/file42.txt"));
    assert!(!regex.is_match("/tmp/repo/app/file4.txt"));
    assert!(!regex.is_match("/tmp/repo/app/fileab.txt"));
}

#[test]
fn unreadable_globs_treat_unclosed_character_classes_as_literals() {
    let regex = seatbelt_regex_for_unreadable_glob("/tmp/repo/[*.env");
    assert_eq!(regex.as_deref(), Some(r"^/tmp/repo/\[[^/]*\.env$"));
    let regex = regex_lite::Regex::new(regex.as_deref().expect("glob should compile"))
        .expect("regex should compile");

    assert!(regex.is_match("/tmp/repo/[local.env"));
    assert!(regex.is_match("/tmp/repo/[.env"));
    assert!(!regex.is_match("/tmp/repo/local.env"));
}

#[test]
fn normalize_path_for_sandbox_rejects_relative_paths() {
    assert_eq!(normalize_path_for_sandbox(Path::new("relative.sock")), None);
}

#[test]
fn seatbelt_args_read_only_policy_keeps_preferences_read_access() {
    let cwd = std::env::temp_dir();
    let args = seatbelt_args_for(
        vec!["echo".to_string(), "ok".to_string()],
        &FileSystemSandboxPolicy::read_only(),
        NetworkSandboxPolicy::Restricted,
        cwd.as_path(),
        &[],
    );
    let policy = seatbelt_policy_arg(&args);
    assert!(policy.contains("(allow user-preference-read)"));
    assert!(!policy.contains("(allow user-preference-write)"));
}

#[test]
fn explicit_unreadable_paths_are_excluded_from_readable_roots() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("nano-readable");
    let unreadable = root.join("private");
    fs::create_dir_all(&unreadable).expect("create unreadable dir");
    let file_system_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: absolute_path(root.as_path()),
            },
            access: FileSystemAccessMode::Read,
            missing_path_behavior: None,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: absolute_path(unreadable.as_path()),
            },
            access: FileSystemAccessMode::Deny,
            missing_path_behavior: None,
        },
    ]);

    let args = seatbelt_args_for(
        vec!["/bin/true".to_string()],
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
        tmp.path(),
        &[],
    );

    let policy = seatbelt_policy_arg(&args);
    let readable_roots = file_system_policy.get_readable_roots_with_cwd(tmp.path());
    let readable_root = readable_roots.first().expect("expected readable root");
    let unreadable_roots = file_system_policy.get_unreadable_roots_with_cwd(tmp.path());
    let unreadable_root = unreadable_roots.first().expect("expected unreadable root");
    assert!(
        policy.contains("(require-not (literal (param \"READABLE_ROOT_0_EXCLUDED_0\")))"),
        "expected exact read carveout in policy:\n{policy}"
    );
    assert!(
        policy.contains("(require-not (subpath (param \"READABLE_ROOT_0_EXCLUDED_0\")))"),
        "expected read carveout in policy:\n{policy}"
    );
    assert!(
        args.iter()
            .any(|arg| arg == &format!("-DREADABLE_ROOT_0={readable_root}")),
        "expected readable root parameter in args: {args:#?}"
    );
    assert!(
        args.iter()
            .any(|arg| arg == &format!("-DREADABLE_ROOT_0_EXCLUDED_0={unreadable_root}")),
        "expected read carveout parameter in args: {args:#?}"
    );
}

#[test]
fn create_seatbelt_args_allowlists_explicit_unix_socket_paths() {
    let cwd = TempDir::new().expect("temp cwd");
    let file_system_policy = FileSystemSandboxPolicy::read_only();
    let socket_path = absolute_path(cwd.path().join("nano-browser-use").as_path());
    let extra_allow_unix_sockets = vec![socket_path];
    let args = seatbelt_args_for(
        vec!["/usr/bin/true".to_string()],
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
        cwd.path(),
        &extra_allow_unix_sockets,
    );
    let policy = seatbelt_policy_arg(&args);

    assert!(
        policy.contains("(allow system-socket (socket-domain AF_UNIX))"),
        "policy should allow AF_UNIX when explicit socket paths are requested:\n{policy}"
    );
    assert!(
        policy.contains(
            "(allow network-outbound (remote unix-socket (subpath (param \"UNIX_SOCKET_PATH_0\"))))"
        ),
        "policy should allow outbound AF_UNIX traffic for explicit socket paths:\n{policy}"
    );
    let expected_socket_root = normalize_path_for_sandbox(cwd.path().join("nano-browser-use").as_path())
        .expect("socket root should normalize")
        .to_string_lossy()
        .into_owned();
    assert!(
        args.iter()
            .any(|arg| arg == &format!("-DUNIX_SOCKET_PATH_0={expected_socket_root}")),
        "seatbelt args should pass the configured socket root as a sandbox param: {args:?}"
    );
}

#[test]
fn create_seatbelt_args_preserves_full_network_with_explicit_unix_socket_paths() {
    let cwd = TempDir::new().expect("temp cwd");
    let file_system_policy = FileSystemSandboxPolicy::read_only();
    let extra_allow_unix_sockets =
        vec![absolute_path(cwd.path().join("nano-browser-use").as_path())];
    let args = seatbelt_args_for(
        vec!["/usr/bin/true".to_string()],
        &file_system_policy,
        NetworkSandboxPolicy::Enabled,
        cwd.path(),
        &extra_allow_unix_sockets,
    );
    let policy = seatbelt_policy_arg(&args);

    assert!(
        policy.contains("(allow network-outbound)\n"),
        "policy should preserve full outbound network access:\n{policy}"
    );
    assert!(
        policy.contains("(allow network-inbound)\n"),
        "policy should preserve full inbound network access:\n{policy}"
    );
    assert!(
        policy.contains(
            "(allow network-outbound (remote unix-socket (subpath (param \"UNIX_SOCKET_PATH_0\"))))"
        ),
        "policy should still allow outbound AF_UNIX traffic for explicit socket paths:\n{policy}"
    );
}

#[test]
fn unix_socket_policy_non_empty_output_is_newline_terminated() {
    let tmp = TempDir::new().expect("tempdir");
    let allowlist_policy = unix_socket_policy(&UnixDomainSocketPolicy::Restricted {
        allowed: vec![absolute_path(tmp.path().join("example.sock").as_path())],
    });
    assert!(
        allowlist_policy.ends_with('\n'),
        "allowlist unix socket policy should end with a newline:\n{allowlist_policy}"
    );

    let allow_all_policy = unix_socket_policy(&UnixDomainSocketPolicy::AllowAll);
    assert!(
        allow_all_policy.ends_with('\n'),
        "allow-all unix socket policy should end with a newline:\n{allow_all_policy}"
    );
}

#[test]
fn unix_socket_dir_params_use_stable_param_names() {
    let tmp = TempDir::new().expect("tempdir");
    let params = unix_socket_dir_params(&UnixDomainSocketPolicy::Restricted {
        allowed: vec![
            absolute_path(tmp.path().join("b.sock").as_path()),
            absolute_path(tmp.path().join("a.sock").as_path()),
            absolute_path(tmp.path().join("a.sock").as_path()),
        ],
    });

    assert_eq!(
        params,
        vec![
            (
                "UNIX_SOCKET_PATH_0".to_string(),
                tmp.path().join("a.sock")
            ),
            (
                "UNIX_SOCKET_PATH_1".to_string(),
                tmp.path().join("b.sock")
            ),
        ]
    );
}

#[test]
fn create_seatbelt_args_block_first_time_dot_nano_creation_with_metadata_name_regex() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_root = tmp.path().join("repo");
    fs::create_dir_all(&repo_root).expect("create repo root");

    std::process::Command::new("git")
        .arg("init")
        .arg(".")
        .current_dir(&repo_root)
        .output()
        .expect("git init .");

    let file_system_policy = workspace_write_policy(
        &[absolute_path(repo_root.as_path())],
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );

    let args = seatbelt_args_for(
        vec!["/bin/true".to_string()],
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
        repo_root.as_path(),
        &[],
    );

    let policy_text = seatbelt_policy_arg(&args);
    assert!(
        policy_text.contains(&seatbelt_protected_metadata_name_requirements(&canonical(
            repo_root.as_path()
        ))),
        "expected metadata protection regex requirements in policy:\n{policy_text}"
    );
}

#[test]
fn create_seatbelt_args_with_read_only_git_and_nano_subpaths() {
    // Create a temporary workspace with two writable roots: one containing
    // top-level workspace metadata paths and one without them.
    let tmp = TempDir::new().expect("tempdir");
    let PopulatedTmp {
        vulnerable_root: _,
        vulnerable_root_canonical,
        dot_git_canonical,
        dot_agents_canonical: _,
        dot_nano_canonical,
        empty_root,
        empty_root_canonical,
    } = populate_tmpdir(tmp.path());
    let cwd = tmp.path().join("cwd");
    fs::create_dir_all(&cwd).expect("create cwd");

    // Build a policy that only includes the two test roots as writable and
    // does not automatically include defaults TMPDIR or /tmp.
    let file_system_policy = workspace_write_policy(
        &[
            absolute_path(vulnerable_root_canonical.as_path()),
            absolute_path(empty_root.as_path()),
        ],
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );

    // Create the Seatbelt command to wrap a shell command that tries to
    // write to .nano/config.toml in the vulnerable root.
    let shell_command: Vec<String> = [
        "bash",
        "-c",
        "echo 'sandbox_mode = \"danger-full-access\"' > \"$1\"",
        "bash",
        dot_nano_canonical
            .join("config.toml")
            .to_string_lossy()
            .as_ref(),
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect();
    let args = seatbelt_args_for(
        shell_command.clone(),
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
        &cwd,
        &[],
    );

    let policy_text = seatbelt_policy_arg(&args);
    assert!(
        policy_text.contains("(require-all (subpath (param \"WRITABLE_ROOT_0\"))"),
        "expected cwd writable root to carry protected carveouts:\n{policy_text}",
    );
    assert!(
        policy_text.contains("WRITABLE_ROOT_0_EXCLUDED_0"),
        "expected cwd metadata carveouts in policy:\n{policy_text}",
    );
    assert!(
        policy_text.contains("WRITABLE_ROOT_0_EXCLUDED_1")
            && policy_text.contains("WRITABLE_ROOT_0_EXCLUDED_2"),
        "expected symbolic cwd .git/.agents carveouts in policy:\n{policy_text}",
    );
    assert!(
        policy_text.contains("WRITABLE_ROOT_1_EXCLUDED_0")
            && policy_text.contains("WRITABLE_ROOT_1_EXCLUDED_1"),
        "expected explicit writable root .git/.nano carveouts in policy:\n{policy_text}",
    );
    assert!(
        policy_text.contains(&seatbelt_protected_metadata_name_requirements(&canonical(&cwd))),
        "expected cwd metadata protection regex requirements in policy:\n{policy_text}",
    );
    assert!(
        policy_text.contains(&seatbelt_protected_metadata_name_requirements(
            &vulnerable_root_canonical
        )),
        "expected populated root metadata protection regex requirements in policy:\n{policy_text}",
    );
    assert!(
        policy_text
            .contains(&seatbelt_protected_metadata_name_requirements(&empty_root_canonical)),
        "expected empty root metadata protection regex requirements in policy:\n{policy_text}",
    );

    let expected_definitions = [
        format!("-DWRITABLE_ROOT_0={}", canonical(&cwd).to_string_lossy()),
        format!(
            "-DWRITABLE_ROOT_0_EXCLUDED_0={}",
            canonical(&cwd).join(".git").display()
        ),
        format!(
            "-DWRITABLE_ROOT_0_EXCLUDED_1={}",
            canonical(&cwd).join(".agents").display()
        ),
        format!(
            "-DWRITABLE_ROOT_0_EXCLUDED_2={}",
            canonical(&cwd).join(".nano").display()
        ),
        format!(
            "-DWRITABLE_ROOT_1={}",
            vulnerable_root_canonical.to_string_lossy()
        ),
        format!(
            "-DWRITABLE_ROOT_1_EXCLUDED_0={}",
            dot_git_canonical.to_string_lossy()
        ),
        format!(
            "-DWRITABLE_ROOT_1_EXCLUDED_1={}",
            dot_nano_canonical.to_string_lossy()
        ),
        format!(
            "-DWRITABLE_ROOT_2={}",
            empty_root_canonical.to_string_lossy()
        ),
    ];
    let writable_definitions: Vec<String> = args
        .iter()
        .filter(|arg| arg.starts_with("-DWRITABLE_ROOT_"))
        .cloned()
        .collect();
    assert_eq!(
        writable_definitions, expected_definitions,
        "unexpected writable-root parameter definitions in {args:#?}"
    );
    let command_index = args
        .iter()
        .position(|arg| arg == "--")
        .expect("seatbelt args should include command separator");
    assert_eq!(args[command_index + 1..], shell_command);
}

#[cfg(unix)]
#[test]
fn explicit_unreadable_paths_are_excluded_from_full_disk_read_and_write_access() {
    let unreadable = absolute_path(Path::new("/tmp/nano-unreadable"));
    let file_system_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Write,
            missing_path_behavior: None,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: unreadable.clone(),
            },
            access: FileSystemAccessMode::Deny,
            missing_path_behavior: None,
        },
    ]);

    let args = seatbelt_args_for(
        vec!["/bin/true".to_string()],
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
        Path::new("/"),
        &[],
    );

    let policy = seatbelt_policy_arg(&args);
    let unreadable_roots = file_system_policy.get_unreadable_roots_with_cwd(Path::new("/"));
    let unreadable_root = unreadable_roots.first().expect("expected unreadable root");
    assert!(
        policy.contains("(require-not (literal (param \"READABLE_ROOT_0_EXCLUDED_0\")))"),
        "expected exact read carveout in policy:\n{policy}"
    );
    assert!(
        policy.contains("(require-not (subpath (param \"READABLE_ROOT_0_EXCLUDED_0\")))"),
        "expected read carveout in policy:\n{policy}"
    );
    assert!(
        policy.contains("(require-not (literal (param \"WRITABLE_ROOT_0_EXCLUDED_0\")))"),
        "expected exact write carveout in policy:\n{policy}"
    );
    assert!(
        policy.contains("(require-not (subpath (param \"WRITABLE_ROOT_0_EXCLUDED_0\")))"),
        "expected write carveout in policy:\n{policy}"
    );
    assert!(
        policy.contains(&seatbelt_protected_metadata_name_requirements(Path::new(
            "/"
        ))),
        "expected metadata protection regex deny requirements in policy:\n{policy}"
    );
    assert!(
        args.iter()
            .any(|arg| arg == &format!("-DREADABLE_ROOT_0_EXCLUDED_0={unreadable_root}")),
        "expected read carveout parameter in args: {args:#?}"
    );
    let writable_definitions: Vec<String> = args
        .iter()
        .filter(|arg| arg.starts_with("-DWRITABLE_ROOT_"))
        .cloned()
        .collect();
    assert_eq!(
        writable_definitions,
        vec![
            "-DWRITABLE_ROOT_0=/".to_string(),
            "-DWRITABLE_ROOT_0_EXCLUDED_0=/.nano".to_string(),
            format!("-DWRITABLE_ROOT_0_EXCLUDED_1={unreadable_root}"),
        ],
        "unexpected write carveout parameters in args: {args:#?}"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_glob_policy_includes_canonicalized_static_prefix() {
    use std::os::unix::fs::symlink;

    let temp_dir = TempDir::new().expect("temp dir");
    let real_root = temp_dir.path().join("real-root");
    let link_root = temp_dir.path().join("link-root");
    fs::create_dir(&real_root).expect("create real root");
    symlink(&real_root, &link_root).expect("create symlinked root");

    let pattern = format!("{}/**/*.env", link_root.display());
    let canonical_pattern = format!("{}/**/*.env", canonical(real_root.as_path()).display());
    let expected_regex = seatbelt_regex_for_unreadable_glob(&canonical_pattern)
        .expect("canonical glob should compile");
    let mut policy = FileSystemSandboxPolicy::default();
    policy.entries.push(FileSystemSandboxEntry {
        path: FileSystemPath::GlobPattern { pattern },
        access: FileSystemAccessMode::Deny,
        missing_path_behavior: None,
    });

    let seatbelt_policy = build_seatbelt_unreadable_glob_policy(&policy, temp_dir.path());

    assert!(
        seatbelt_policy.contains(&format!(r#"(deny file-read* (regex #"{expected_regex}"))"#)),
        "expected canonicalized glob regex in policy:\n{seatbelt_policy}"
    );
}

#[cfg(unix)]
#[test]
fn create_seatbelt_args_for_cwd_as_git_repo() {
    // Create a temporary workspace with one writable root (the cwd) and use
    // the default writable temp roots, verifying protected metadata checks
    // for each root.
    let tmp = TempDir::new().expect("tempdir");
    let PopulatedTmp {
        vulnerable_root,
        vulnerable_root_canonical,
        dot_git_canonical,
        dot_nano_canonical,
        ..
    } = populate_tmpdir(tmp.path());

    let file_system_policy =
        workspace_write_policy(&[], /*exclude_tmpdir_env_var*/ false, /*exclude_slash_tmp*/ false);

    let shell_command: Vec<String> = [
        "bash",
        "-c",
        "echo 'sandbox_mode = \"danger-full-access\"' > \"$1\"",
        "bash",
        dot_nano_canonical
            .join("config.toml")
            .to_string_lossy()
            .as_ref(),
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect();
    let args = seatbelt_args_for(
        shell_command.clone(),
        &file_system_policy,
        NetworkSandboxPolicy::Restricted,
        vulnerable_root.as_path(),
        &[],
    );

    let slash_tmp = canonical(Path::new("/tmp"));
    let policy_text = seatbelt_policy_arg(&args);
    assert!(
        policy_text.contains(&seatbelt_protected_metadata_name_requirements(
            &vulnerable_root_canonical
        )),
        "expected cwd metadata protection regex requirements in policy:\n{policy_text}",
    );
    assert!(
        policy_text.contains(&seatbelt_protected_metadata_name_requirements(&slash_tmp)),
        "expected /tmp metadata protection regex requirements in policy:\n{policy_text}",
    );
    if let Some(tmpdir_env_var) = std::env::var("TMPDIR")
        .ok()
        .map(PathBuf::from)
        .map(|p| canonical(p.as_path()))
    {
        assert!(
            policy_text.contains(&seatbelt_protected_metadata_name_requirements(
                &tmpdir_env_var
            )),
            "expected TMPDIR metadata protection regex requirements in policy:\n{policy_text}",
        );
    }

    let expected_root = format!(
        "-DWRITABLE_ROOT_0={}",
        vulnerable_root_canonical.to_string_lossy()
    );
    assert!(
        args.contains(&expected_root),
        "missing {expected_root}: {args:#?}"
    );
    let expected_dot_git = format!(
        "-DWRITABLE_ROOT_0_EXCLUDED_0={}",
        dot_git_canonical.to_string_lossy()
    );
    assert!(
        args.contains(&expected_dot_git),
        "missing {expected_dot_git}: {args:#?}"
    );
    let expected_dot_nano = format!(
        "-DWRITABLE_ROOT_0_EXCLUDED_2={}",
        dot_nano_canonical.to_string_lossy()
    );
    assert!(
        args.contains(&expected_dot_nano),
        "missing {expected_dot_nano}: {args:#?}"
    );
    let expected_slash_tmp = format!("-DWRITABLE_ROOT_1={}", slash_tmp.to_string_lossy());
    assert!(
        args.contains(&expected_slash_tmp),
        "missing {expected_slash_tmp}: {args:#?}"
    );
    let command_index = args
        .iter()
        .position(|arg| arg == "--")
        .expect("seatbelt args should include command separator");
    assert_eq!(args[command_index + 1..], shell_command);
}

// Some fields are only read by unix/macos-gated tests.
#[cfg_attr(not(unix), allow(dead_code))]
struct PopulatedTmp {
    /// Path containing protected metadata subfolders.
    /// For the purposes of this test, we consider this a "vulnerable" root
    /// because a bad actor could write to .git/hooks/pre-commit so an
    /// unsuspecting user would run code as privileged the next time they
    /// ran `git commit` themselves, or modify .nano/config.toml to
    /// contain `sandbox_mode = "danger-full-access"` so the agent would
    /// have full privileges the next time it ran in that repo.
    vulnerable_root: PathBuf,
    vulnerable_root_canonical: PathBuf,
    dot_git_canonical: PathBuf,
    dot_agents_canonical: PathBuf,
    dot_nano_canonical: PathBuf,

    /// Path without protected metadata subfolders.
    empty_root: PathBuf,
    /// Canonicalized version of `empty_root`.
    empty_root_canonical: PathBuf,
}

fn populate_tmpdir(tmp: &Path) -> PopulatedTmp {
    let vulnerable_root = tmp.join("vulnerable_root");
    fs::create_dir_all(&vulnerable_root).expect("create vulnerable_root");

    std::process::Command::new("git")
        .arg("init")
        .arg(".")
        .current_dir(&vulnerable_root)
        .output()
        .expect("git init .");

    fs::create_dir_all(vulnerable_root.join(".nano")).expect("create .nano");
    fs::write(
        vulnerable_root.join(".nano").join("config.toml"),
        "sandbox_mode = \"read-only\"\n",
    )
    .expect("write .nano/config.toml");

    let empty_root = tmp.join("empty_root");
    fs::create_dir_all(&empty_root).expect("create empty_root");

    // Ensure we have canonical paths for -D parameter matching.
    let vulnerable_root_canonical = canonical(vulnerable_root.as_path());
    let dot_git_canonical = vulnerable_root_canonical.join(".git");
    let dot_agents_canonical = vulnerable_root_canonical.join(".agents");
    let dot_nano_canonical = vulnerable_root_canonical.join(".nano");
    let empty_root_canonical = canonical(empty_root.as_path());
    PopulatedTmp {
        vulnerable_root,
        vulnerable_root_canonical,
        dot_git_canonical,
        dot_agents_canonical,
        dot_nano_canonical,
        empty_root,
        empty_root_canonical,
    }
}

/// Live-enforcement tests: these spawn `/usr/bin/sandbox-exec` and therefore
/// only run on macOS.
#[cfg(target_os = "macos")]
mod live {
    use super::super::MACOS_PATH_TO_SEATBELT_EXECUTABLE;
    use super::*;
    use pretty_assertions::assert_eq;
    use std::process::Command;

    fn assert_seatbelt_denied(stderr: &[u8], path: &Path) {
        let stderr = String::from_utf8_lossy(stderr);
        let expected = format!("bash: {}: Operation not permitted\n", path.display());
        assert!(
            stderr == expected
                || stderr.contains("sandbox-exec: sandbox_apply: Operation not permitted"),
            "unexpected stderr: {stderr}"
        );
    }

    #[test]
    fn seatbelt_enforces_read_only_git_and_nano_subpaths() {
        let tmp = TempDir::new().expect("tempdir");
        let PopulatedTmp {
            vulnerable_root_canonical,
            dot_git_canonical,
            dot_nano_canonical,
            empty_root,
            ..
        } = populate_tmpdir(tmp.path());
        let cwd = tmp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("create cwd");

        let file_system_policy = workspace_write_policy(
            &[
                absolute_path(vulnerable_root_canonical.as_path()),
                absolute_path(empty_root.as_path()),
            ],
            /*exclude_tmpdir_env_var*/ true,
            /*exclude_slash_tmp*/ true,
        );

        // Verify that .nano/config.toml cannot be modified under the
        // generated Seatbelt policy.
        let config_toml = dot_nano_canonical.join("config.toml");
        let shell_command: Vec<String> = [
            "bash",
            "-c",
            "echo 'sandbox_mode = \"danger-full-access\"' > \"$1\"",
            "bash",
            config_toml.to_string_lossy().as_ref(),
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        let args = seatbelt_args_for(
            shell_command,
            &file_system_policy,
            NetworkSandboxPolicy::Restricted,
            &cwd,
            &[],
        );
        let output = Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
            .args(&args)
            .current_dir(&cwd)
            .output()
            .expect("execute seatbelt command");
        assert_eq!(
            "sandbox_mode = \"read-only\"\n",
            String::from_utf8_lossy(&fs::read(&config_toml).expect("read config.toml")),
            "config.toml should contain its original contents because it should not have been modified"
        );
        assert!(
            !output.status.success(),
            "command to write {} should fail under seatbelt",
            &config_toml.display()
        );
        assert_seatbelt_denied(&output.stderr, &config_toml);

        // Create a similar Seatbelt command that tries to write to a file in
        // the .git folder, which should also be blocked.
        let pre_commit_hook = dot_git_canonical.join("hooks").join("pre-commit");
        let shell_command_git: Vec<String> = [
            "bash",
            "-c",
            "echo 'pwned!' > \"$1\"",
            "bash",
            pre_commit_hook.to_string_lossy().as_ref(),
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        let write_hooks_file_args = seatbelt_args_for(
            shell_command_git,
            &file_system_policy,
            NetworkSandboxPolicy::Restricted,
            &cwd,
            &[],
        );
        let output = Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
            .args(&write_hooks_file_args)
            .current_dir(&cwd)
            .output()
            .expect("execute seatbelt command");
        assert!(
            !fs::exists(&pre_commit_hook).expect("exists pre-commit hook"),
            "{} should not exist because it should not have been created",
            pre_commit_hook.display()
        );
        assert!(
            !output.status.success(),
            "command to write {} should fail under seatbelt",
            &pre_commit_hook.display()
        );
        assert_seatbelt_denied(&output.stderr, &pre_commit_hook);

        // Verify that writing a file to the folder containing .git and .nano
        // is allowed.
        let allowed_file = vulnerable_root_canonical.join("allowed.txt");
        let shell_command_allowed: Vec<String> = [
            "bash",
            "-c",
            "echo 'this is allowed' > \"$1\"",
            "bash",
            allowed_file.to_string_lossy().as_ref(),
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        let write_allowed_file_args = seatbelt_args_for(
            shell_command_allowed,
            &file_system_policy,
            NetworkSandboxPolicy::Restricted,
            &cwd,
            &[],
        );
        let output = Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
            .args(&write_allowed_file_args)
            .current_dir(&cwd)
            .output()
            .expect("execute seatbelt command");
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success()
            && stderr.contains("sandbox-exec: sandbox_apply: Operation not permitted")
        {
            return;
        }
        assert!(
            output.status.success(),
            "command to write {} should succeed under seatbelt",
            &allowed_file.display()
        );
        assert_eq!(
            "this is allowed\n",
            String::from_utf8_lossy(&fs::read(&allowed_file).expect("read allowed.txt")),
            "{} should contain the written text",
            allowed_file.display()
        );
    }

    #[test]
    fn create_seatbelt_args_with_read_only_git_pointer_file() {
        let tmp = TempDir::new().expect("tempdir");
        let worktree_root = tmp.path().join("worktree_root");
        fs::create_dir_all(&worktree_root).expect("create worktree_root");
        let gitdir = worktree_root.join("actual-gitdir");
        fs::create_dir_all(&gitdir).expect("create gitdir");
        let gitdir_config = gitdir.join("config");
        let gitdir_config_contents = "[core]\n";
        fs::write(&gitdir_config, gitdir_config_contents).expect("write gitdir config");

        let dot_git = worktree_root.join(".git");
        let dot_git_contents = format!("gitdir: {}\n", gitdir.to_string_lossy());
        fs::write(&dot_git, &dot_git_contents).expect("write .git pointer");

        let cwd = tmp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("create cwd");

        let file_system_policy = workspace_write_policy(
            &[absolute_path(worktree_root.as_path())],
            /*exclude_tmpdir_env_var*/ true,
            /*exclude_slash_tmp*/ true,
        );

        let shell_command: Vec<String> = [
            "bash",
            "-c",
            "echo 'pwned!' > \"$1\"",
            "bash",
            dot_git.to_string_lossy().as_ref(),
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        let args = seatbelt_args_for(
            shell_command,
            &file_system_policy,
            NetworkSandboxPolicy::Restricted,
            &cwd,
            &[],
        );

        let output = Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
            .args(&args)
            .current_dir(&cwd)
            .output()
            .expect("execute seatbelt command");

        assert_eq!(
            dot_git_contents,
            String::from_utf8_lossy(&fs::read(&dot_git).expect("read .git pointer")),
            ".git pointer file should not be modified under seatbelt"
        );
        assert!(
            !output.status.success(),
            "command to write {} should fail under seatbelt",
            dot_git.display()
        );
        assert_seatbelt_denied(&output.stderr, &dot_git);

        let shell_command_gitdir: Vec<String> = [
            "bash",
            "-c",
            "echo 'pwned!' > \"$1\"",
            "bash",
            gitdir_config.to_string_lossy().as_ref(),
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        let gitdir_args = seatbelt_args_for(
            shell_command_gitdir,
            &file_system_policy,
            NetworkSandboxPolicy::Restricted,
            &cwd,
            &[],
        );
        let output = Command::new(MACOS_PATH_TO_SEATBELT_EXECUTABLE)
            .args(&gitdir_args)
            .current_dir(&cwd)
            .output()
            .expect("execute seatbelt command");

        assert_eq!(
            gitdir_config_contents,
            String::from_utf8_lossy(&fs::read(&gitdir_config).expect("read gitdir config")),
            "gitdir config should contain its original contents because it should not have been modified"
        );
        assert!(
            !output.status.success(),
            "command to write {} should fail under seatbelt",
            gitdir_config.display()
        );
        assert_seatbelt_denied(&output.stderr, &gitdir_config);
    }
}
