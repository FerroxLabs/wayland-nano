//! Provenance: ported from Codex `codex-rs/linux-sandbox/src/bwrap.rs` tests
//! and `codex-rs/sandboxing/src/bwrap_tests.rs` @ 646f7c0a. Transformations:
//! - proxy-mode tests dropped with the managed-proxy surface;
//! - `.codex` -> `.nano` in protected-metadata expectations;
//! - `FileSystemSandboxPolicy::workspace_write` ->
//!   `policy_engine::workspace_write_policy` (wayland-nano has no legacy
//!   SandboxPolicy constructors);
//! - gating: pure string/argv tests run on every host; filesystem-builder
//!   tests are `cfg(unix)` (unix path semantics, `/dev/null`); the
//!   user-namespace probe tests are `cfg(target_os = "linux")` (shell fakes).

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn default_unreadable_glob_scan_has_no_depth_cap() {
    assert_eq!(BwrapOptions::default().glob_scan_max_depth, None);
}

#[test]
fn network_mode_unshare_mapping() {
    assert!(!BwrapNetworkMode::FullAccess.should_unshare_network());
    assert!(BwrapNetworkMode::Isolated.should_unshare_network());
}

#[test]
fn full_disk_write_full_network_returns_unwrapped_command() {
    let command = vec!["/bin/true".to_string()];
    let args = create_bwrap_command_args(
        command.clone(),
        &FileSystemSandboxPolicy::unrestricted(),
        Path::new("/"),
        Path::new("/"),
        BwrapOptions {
            mount_proc: true,
            network_mode: BwrapNetworkMode::FullAccess,
            ..Default::default()
        },
    )
    .expect("create bwrap args");

    assert_eq!(args.args, command);
}

#[test]
fn full_disk_write_isolated_network_keeps_full_filesystem_but_unshares_network() {
    let command = vec!["/bin/true".to_string()];
    let args = create_bwrap_command_args(
        command,
        &FileSystemSandboxPolicy::unrestricted(),
        Path::new("/"),
        Path::new("/"),
        BwrapOptions {
            mount_proc: true,
            network_mode: BwrapNetworkMode::Isolated,
            ..Default::default()
        },
    )
    .expect("create bwrap args");

    assert_eq!(
        args.args,
        vec![
            "--new-session".to_string(),
            "--die-with-parent".to_string(),
            "--bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--bind-try".to_string(),
            "/dev/shm".to_string(),
            "/dev/shm".to_string(),
            "--unshare-user".to_string(),
            "--unshare-pid".to_string(),
            "--unshare-net".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--".to_string(),
            "/bin/true".to_string(),
        ]
    );
}

#[test]
fn detects_wsl1_proc_version_formats() {
    assert!(proc_version_indicates_wsl1(
        "Linux version 4.4.0-22621-Microsoft"
    ));
    assert!(proc_version_indicates_wsl1(
        "Linux version 5.15.0-microsoft-standard-WSL1"
    ));
    assert!(proc_version_indicates_wsl1(
        "Linux version 5.15.0-wsl-microsoft-standard-WSL1"
    ));
}

#[test]
fn does_not_treat_wsl2_or_native_linux_as_wsl1() {
    assert!(!proc_version_indicates_wsl1(
        "Linux version 6.6.87.2-microsoft-standard-WSL2"
    ));
    assert!(!proc_version_indicates_wsl1(
        "Linux version 6.6.87.2-wsl-microsoft-standard-WSL2"
    ));
    assert!(!proc_version_indicates_wsl1(
        "Linux version 4.19.104-microsoft-standard"
    ));
    assert!(!proc_version_indicates_wsl1(
        "Linux version 6.6.87.2-microsoft-standard-WSL3"
    ));
    assert!(!proc_version_indicates_wsl1("Linux version 6.8.0"));
}

#[test]
fn system_bwrap_warning_reports_missing_system_bwrap() {
    assert_eq!(
        system_bwrap_warning_for_path(/*system_bwrap_path*/ None),
        Some(MISSING_BWRAP_WARNING.to_string())
    );
}

#[test]
fn missing_bwrap_warning_uses_nano_namespace() {
    assert!(MISSING_BWRAP_WARNING.contains("Wayland Nano"));
    assert!(WSL1_BWRAP_WARNING.contains("Wayland Nano"));
}

#[cfg(target_os = "linux")]
#[test]
fn unclosed_character_classes_are_escaped_for_ripgrep() {
    let (search_root, glob) =
        split_pattern_for_ripgrep("/tmp/[*.env", Path::new("/")).expect("split pattern");

    assert_eq!(search_root.as_path(), Path::new("/tmp"));
    assert_eq!(glob, r"\[*.env");
}

#[cfg(target_os = "linux")]
#[test]
fn root_prefix_unreadable_globs_are_too_broad_for_linux_expansion() {
    assert_eq!(
        split_pattern_for_ripgrep("/**/*.env", Path::new("/tmp")),
        None
    );
}

#[cfg(target_os = "linux")]
mod unix {
    use super::*;
    use nano_core::permissions::FileSystemSandboxEntry;
    use nano_core::policy_engine::workspace_write_policy;
    use pretty_assertions::assert_eq;
    use std::fs::File;
    use tempfile::TempDir;

    const NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH: Option<usize> = None;

    fn unreadable_glob_entry(pattern: String) -> FileSystemSandboxEntry {
        FileSystemSandboxEntry {
            path: FileSystemPath::GlobPattern { pattern },
            access: FileSystemAccessMode::Deny,
            missing_path_behavior: None,
        }
    }

    fn default_policy_with_unreadable_glob(pattern: String) -> FileSystemSandboxPolicy {
        let mut policy = FileSystemSandboxPolicy::default();
        policy.entries.push(unreadable_glob_entry(pattern));
        policy
    }

    #[test]
    fn full_disk_write_with_unreadable_glob_still_wraps_and_masks_match() {
        if !ripgrep_available() {
            return;
        }

        let temp_dir = TempDir::new().expect("temp dir");
        let root_env = temp_dir.path().join(".env");
        std::fs::write(&root_env, "secret").expect("write env");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            unreadable_glob_entry(format!("{}/**/*.env", temp_dir.path().display())),
        ]);
        let command = vec!["/bin/true".to_string()];

        let args = create_bwrap_command_args(
            command.clone(),
            &policy,
            temp_dir.path(),
            temp_dir.path(),
            BwrapOptions::default(),
        )
        .expect("create bwrap args");

        assert_ne!(
            args.args, command,
            "full-write policy with unreadable globs must still use bwrap"
        );
        assert_file_masked(&args.args, &root_env);
    }

    #[test]
    fn restricted_policy_chdirs_to_canonical_command_cwd() {
        let temp_dir = TempDir::new().expect("temp dir");
        let real_root = temp_dir.path().join("real");
        let real_subdir = real_root.join("subdir");
        let link_root = temp_dir.path().join("link");
        std::fs::create_dir_all(&real_subdir).expect("create real subdir");
        std::os::unix::fs::symlink(&real_root, &link_root).expect("create symlinked root");

        let sandbox_policy_cwd = AbsolutePathBuf::from_absolute_path(&link_root)
            .expect("absolute symlinked root")
            .to_path_buf();
        let command_cwd = link_root.join("subdir");
        let canonical_command_cwd = real_subdir
            .canonicalize()
            .expect("canonicalize command cwd");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Minimal,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
        ]);

        let args = create_bwrap_command_args(
            vec!["/bin/true".to_string()],
            &policy,
            sandbox_policy_cwd.as_path(),
            &command_cwd,
            BwrapOptions::default(),
        )
        .expect("create bwrap args");
        let canonical_sandbox_cwd = path_to_string(
            &real_root
                .canonicalize()
                .expect("canonicalize sandbox policy cwd"),
        );
        let canonical_command_cwd = path_to_string(&canonical_command_cwd);
        let link_sandbox_cwd = path_to_string(&link_root);
        let link_command_cwd = path_to_string(&command_cwd);

        assert!(
            args.args
                .windows(2)
                .any(|window| { window == ["--chdir", canonical_command_cwd.as_str()] })
        );
        assert!(args.args.windows(3).any(|window| {
            window
                == [
                    "--ro-bind",
                    canonical_sandbox_cwd.as_str(),
                    canonical_sandbox_cwd.as_str(),
                ]
        }));
        assert!(
            !args
                .args
                .windows(2)
                .any(|window| { window == ["--chdir", link_command_cwd.as_str()] })
        );
        assert!(!args.args.windows(3).any(|window| {
            window
                == [
                    "--ro-bind",
                    link_sandbox_cwd.as_str(),
                    link_sandbox_cwd.as_str(),
                ]
        }));
    }

    #[test]
    fn symlinked_writable_roots_bind_real_target_and_remap_carveouts() {
        let temp_dir = TempDir::new().expect("temp dir");
        let real_root = temp_dir.path().join("real");
        let link_root = temp_dir.path().join("link");
        let blocked = real_root.join("blocked");
        std::fs::create_dir_all(&blocked).expect("create blocked dir");
        std::os::unix::fs::symlink(&real_root, &link_root).expect("create symlinked root");

        let link_root =
            AbsolutePathBuf::from_absolute_path(&link_root).expect("absolute symlinked root");
        let link_blocked = link_root.join("blocked");
        let real_root_str = path_to_string(&real_root);
        let real_blocked_str = path_to_string(&blocked);
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: link_root },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: link_blocked },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");

        assert!(args.args.windows(3).any(|window| {
            window == ["--bind", real_root_str.as_str(), real_root_str.as_str()]
        }));
        assert!(args.args.windows(6).any(|window| {
            window
                == [
                    "--perms",
                    "000",
                    "--tmpfs",
                    real_blocked_str.as_str(),
                    "--remount-ro",
                    real_blocked_str.as_str(),
                ]
        }));
    }

    #[test]
    fn writable_roots_under_symlinked_ancestors_bind_real_target() {
        let temp_dir = TempDir::new().expect("temp dir");
        let logical_home = temp_dir.path().join("home");
        let real_nano = temp_dir.path().join("real-nano");
        let logical_nano = logical_home.join(".nano");
        let real_memories = real_nano.join("memories");
        let logical_memories = logical_nano.join("memories");
        std::fs::create_dir_all(&logical_home).expect("create logical home");
        std::fs::create_dir_all(&real_memories).expect("create memories dir");
        std::os::unix::fs::symlink(&real_nano, &logical_nano).expect("create symlinked nano home");

        let logical_memories_root =
            AbsolutePathBuf::from_absolute_path(&logical_memories).expect("absolute memories");
        let real_memories_str = path_to_string(&real_memories);
        let logical_memories_str = path_to_string(&logical_memories);
        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: logical_memories_root,
            },
            access: FileSystemAccessMode::Write,
            missing_path_behavior: None,
        }]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");

        assert!(args.args.windows(3).any(|window| {
            window
                == [
                    "--bind",
                    real_memories_str.as_str(),
                    real_memories_str.as_str(),
                ]
        }));
        assert!(!args.args.windows(3).any(|window| {
            window
                == [
                    "--bind",
                    logical_memories_str.as_str(),
                    logical_memories_str.as_str(),
                ]
        }));
    }

    #[test]
    fn protected_symlinked_directory_subpaths_fail_closed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().join("root");
        let agents_target = root.join("agents-target");
        let agents_link = root.join(".agents");
        std::fs::create_dir_all(&agents_target).expect("create agents target");
        std::os::unix::fs::symlink(&agents_target, &agents_link).expect("create symlinked .agents");

        let root = AbsolutePathBuf::from_absolute_path(&root).expect("absolute root");
        let agents_link_str = path_to_string(&agents_link);
        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Path { path: root },
            access: FileSystemAccessMode::Write,
            missing_path_behavior: None,
        }]);

        let err =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect_err("protected symlinked subpath should fail closed");
        let message = err.to_string();

        assert!(
            message.contains("cannot enforce sandbox read-only path"),
            "{message}"
        );
        assert!(message.contains(&agents_link_str), "{message}");
    }

    #[test]
    fn symlinked_writable_roots_nested_symlink_escape_paths_fail_closed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let real_root = temp_dir.path().join("real");
        let link_root = temp_dir.path().join("link");
        let outside = temp_dir.path().join("outside-private");
        let linked_private = real_root.join("linked-private");
        std::fs::create_dir_all(&real_root).expect("create real root");
        std::fs::create_dir_all(&outside).expect("create outside dir");
        std::os::unix::fs::symlink(&real_root, &link_root).expect("create symlinked root");
        std::os::unix::fs::symlink(&outside, &linked_private)
            .expect("create nested escape symlink");

        let link_root =
            AbsolutePathBuf::from_absolute_path(&link_root).expect("absolute symlinked root");
        let link_private = link_root.join("linked-private");
        let real_linked_private_str = path_to_string(&linked_private);
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: link_root },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: link_private },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]);

        let err =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect_err("deny-read path crossing writable symlink should fail closed");
        let message = err.to_string();

        assert!(
            message.contains("cannot enforce sandbox deny-read path"),
            "{message}"
        );
        assert!(message.contains(&real_linked_private_str), "{message}");
    }

    #[test]
    fn missing_read_only_subpath_uses_empty_file_bind_data() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().join("workspace");
        let blocked = workspace.join("blocked");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let workspace_root =
            AbsolutePathBuf::from_absolute_path(&workspace).expect("absolute workspace");
        let blocked_root = AbsolutePathBuf::from_absolute_path(&blocked).expect("absolute blocked");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: workspace_root,
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: blocked_root },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");

        assert_empty_file_bound_without_perms(&args.args, &blocked);
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".git"));
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".agents"));
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".nano"));
        assert_eq!(args.preserved_files.len(), 1);
        assert_eq!(
            synthetic_mount_target_paths(&args),
            vec![
                blocked.clone(),
                workspace.join(".git"),
                workspace.join(".agents"),
                workspace.join(".nano"),
            ]
        );
        assert!(
            !blocked.exists(),
            "missing path mask should not materialize host-side metadata paths at arg construction time",
        );
    }

    #[test]
    fn transient_empty_preserved_file_uses_empty_file_bind_data() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().join("workspace");
        let dot_git = workspace.join(".git");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        File::create(&dot_git).expect("create empty .git file");

        let workspace_root =
            AbsolutePathBuf::from_absolute_path(&workspace).expect("absolute workspace");
        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: workspace_root,
            },
            access: FileSystemAccessMode::Write,
            missing_path_behavior: None,
        }]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let dot_git_str = path_to_string(&dot_git);

        assert_empty_file_bound_without_perms(&args.args, &dot_git);
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".agents"));
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".nano"));
        assert_eq!(
            synthetic_mount_target_paths(&args),
            vec![
                dot_git.clone(),
                workspace.join(".agents"),
                workspace.join(".nano"),
            ]
        );
        assert!(
            !args
                .args
                .windows(3)
                .any(|window| window == ["--ro-bind", dot_git_str.as_str(), dot_git_str.as_str()]),
            "transient empty preserved file should not be treated as a stable bind source",
        );
        let metadata = std::fs::symlink_metadata(&dot_git).expect("stat .git");
        assert!(
            !args.synthetic_mount_targets[0].should_remove_after_bwrap(&metadata),
            "pre-existing empty preserved files must not be cleaned up as synthetic targets",
        );
    }

    #[test]
    fn missing_child_git_under_parent_repo_uses_protected_create_target() {
        let temp_dir = TempDir::new().expect("temp dir");
        let repo = temp_dir.path().join("repo");
        let workspace = repo.join("workspace");
        let dot_git = workspace.join(".git");
        std::fs::create_dir_all(repo.join(".git")).expect("create parent .git");
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        std::fs::create_dir_all(&workspace).expect("create workspace");

        let workspace_root =
            AbsolutePathBuf::from_absolute_path(&workspace).expect("absolute workspace");
        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: workspace_root,
            },
            access: FileSystemAccessMode::Write,
            missing_path_behavior: None,
        }]);

        let args = create_filesystem_args(&policy, &workspace, NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
            .expect("filesystem args");
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".agents"));
        assert_empty_directory_mounted_read_only(&args.args, &workspace.join(".nano"));
        let dot_git_str = path_to_string(&dot_git);
        assert!(
            !args
                .args
                .windows(4)
                .any(|window| window == ["--perms", "555", "--tmpfs", dot_git_str.as_str()]),
            "missing child .git should not shadow parent repo discovery",
        );
        assert!(
            !synthetic_mount_target_paths(&args).contains(&dot_git),
            "missing child .git should not be a transient mount target",
        );
        assert_eq!(
            protected_create_target_paths(&args),
            vec![dot_git],
            "missing child .git should fail through protected create cleanup",
        );
    }

    #[test]
    fn ignores_missing_writable_roots() {
        let temp_dir = TempDir::new().expect("temp dir");
        let existing_root = temp_dir.path().join("existing");
        let missing_root = temp_dir.path().join("missing");
        std::fs::create_dir(&existing_root).expect("create existing root");

        let policy = workspace_write_policy(
            &[
                AbsolutePathBuf::from_absolute_path(&existing_root).expect("absolute existing"),
                AbsolutePathBuf::from_absolute_path(&missing_root).expect("absolute missing"),
            ],
            /*exclude_tmpdir_env_var*/ true,
            /*exclude_slash_tmp*/ true,
        );

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let existing_root = path_to_string(&existing_root);
        let missing_root = path_to_string(&missing_root);

        assert!(
            args.args.windows(3).any(|window| {
                window == ["--bind", existing_root.as_str(), existing_root.as_str()]
            }),
            "existing writable root should be rebound writable",
        );
        assert!(
            !args.args.iter().any(|arg| arg == &missing_root),
            "missing writable root should be skipped",
        );
    }

    #[test]
    fn missing_project_root_metadata_carveouts_use_metadata_path_masks() {
        let temp_dir = TempDir::new().expect("temp dir");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some(".git".into())),
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some(".agents".into())),
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some(".nano".into())),
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let dot_git = path_to_string(&temp_dir.path().join(".git"));
        let dot_agents = path_to_string(&temp_dir.path().join(".agents"));
        let dot_nano = path_to_string(&temp_dir.path().join(".nano"));

        assert_empty_directory_mounted_read_only(&args.args, Path::new(&dot_git));
        assert_empty_directory_mounted_read_only(&args.args, Path::new(&dot_agents));
        assert_empty_directory_mounted_read_only(&args.args, Path::new(&dot_nano));
        assert!(args.preserved_files.is_empty());
        let synthetic_targets = synthetic_mount_target_paths(&args);
        assert!(synthetic_targets.contains(&PathBuf::from(&dot_git)));
        assert!(synthetic_targets.contains(&PathBuf::from(&dot_agents)));
        assert!(synthetic_targets.contains(&PathBuf::from(&dot_nano)));
        assert_eq!(
            protected_create_target_paths(&args),
            Vec::<PathBuf>::new(),
            "missing protected metadata paths should fail at creation time through read-only mounts",
        );
    }

    #[test]
    fn missing_user_project_root_subpath_rules_are_still_enforced() {
        let temp_dir = TempDir::new().expect("temp dir");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some(".vscode".into())),
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some(".secrets".into())),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let dot_vscode = path_to_string(&temp_dir.path().join(".vscode"));
        let dot_secrets = path_to_string(&temp_dir.path().join(".secrets"));

        assert_empty_file_bound_without_perms(&args.args, Path::new(&dot_vscode));
        assert_empty_file_bound_without_perms(&args.args, Path::new(&dot_secrets));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mounts_dev_before_writable_dev_binds() {
        let sandbox_policy = workspace_write_policy(
            &[AbsolutePathBuf::from_absolute_path("/dev").expect("/dev path")],
            /*exclude_tmpdir_env_var*/ true,
            /*exclude_slash_tmp*/ true,
        );

        let args = create_filesystem_args(
            &sandbox_policy,
            Path::new("/"),
            NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH,
        )
        .expect("bwrap fs args");
        assert!(args.preserved_files.is_empty());
        assert_eq!(
            synthetic_mount_target_paths(&args),
            vec![
                PathBuf::from("/.git"),
                PathBuf::from("/.agents"),
                PathBuf::from("/.nano"),
                PathBuf::from("/dev/.git"),
                PathBuf::from("/dev/.agents"),
                PathBuf::from("/dev/.nano"),
            ]
        );
        assert_eq!(
            args.args,
            vec![
                // Start from a read-only view of the full filesystem.
                "--ro-bind".to_string(),
                "/".to_string(),
                "/".to_string(),
                // Recreate a writable /dev inside the sandbox.
                "--dev".to_string(),
                "/dev".to_string(),
                // Make the writable root itself writable again.
                "--bind".to_string(),
                "/".to_string(),
                "/".to_string(),
                // Mask the default metadata path names under the writable root.
                // Because the root is `/` in this test, these carveout paths
                // appear directly below `/`.
                "--perms".to_string(),
                "555".to_string(),
                "--tmpfs".to_string(),
                "/.git".to_string(),
                "--remount-ro".to_string(),
                "/.git".to_string(),
                "--perms".to_string(),
                "555".to_string(),
                "--tmpfs".to_string(),
                "/.agents".to_string(),
                "--remount-ro".to_string(),
                "/.agents".to_string(),
                "--perms".to_string(),
                "555".to_string(),
                "--tmpfs".to_string(),
                "/.nano".to_string(),
                "--remount-ro".to_string(),
                "/.nano".to_string(),
                // Rebind /dev after the root bind so device nodes remain
                // writable/usable inside the writable root.
                "--bind".to_string(),
                "/dev".to_string(),
                "/dev".to_string(),
                // Then mask the metadata names that would otherwise be
                // creatable below the writable /dev bind.
                "--perms".to_string(),
                "555".to_string(),
                "--tmpfs".to_string(),
                "/dev/.git".to_string(),
                "--remount-ro".to_string(),
                "/dev/.git".to_string(),
                "--perms".to_string(),
                "555".to_string(),
                "--tmpfs".to_string(),
                "/dev/.agents".to_string(),
                "--remount-ro".to_string(),
                "/dev/.agents".to_string(),
                "--perms".to_string(),
                "555".to_string(),
                "--tmpfs".to_string(),
                "/dev/.nano".to_string(),
                "--remount-ro".to_string(),
                "/dev/.nano".to_string(),
            ]
        );
    }

    #[test]
    fn restricted_read_only_uses_scoped_read_roots_instead_of_erroring() {
        let temp_dir = TempDir::new().expect("temp dir");
        let readable_root = temp_dir.path().join("readable");
        std::fs::create_dir(&readable_root).expect("create readable root");

        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(&readable_root)
                    .expect("absolute readable root"),
            },
            access: FileSystemAccessMode::Read,
            missing_path_behavior: None,
        }]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");

        assert_eq!(args.args[0..4], ["--tmpfs", "/", "--dev", "/dev"]);

        let readable_root_str = path_to_string(&readable_root);
        assert!(args.args.windows(3).any(|window| {
            window
                == [
                    "--ro-bind",
                    readable_root_str.as_str(),
                    readable_root_str.as_str(),
                ]
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn restricted_read_only_with_platform_defaults_includes_usr_when_present() {
        let temp_dir = TempDir::new().expect("temp dir");
        let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Minimal,
            },
            access: FileSystemAccessMode::Read,
            missing_path_behavior: None,
        }]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");

        assert!(
            args.args
                .starts_with(&["--tmpfs".to_string(), "/".to_string()])
        );

        if Path::new("/usr").exists() {
            assert!(
                args.args
                    .windows(3)
                    .any(|window| window == ["--ro-bind", "/usr", "/usr"])
            );
        }
    }

    #[test]
    fn split_policy_reapplies_unreadable_carveouts_after_writable_binds() {
        let temp_dir = TempDir::new().expect("temp dir");
        let writable_root = temp_dir.path().join("workspace");
        let blocked = writable_root.join("blocked");
        std::fs::create_dir_all(&blocked).expect("create blocked dir");
        let writable_root =
            AbsolutePathBuf::from_absolute_path(&writable_root).expect("absolute writable root");
        let blocked = AbsolutePathBuf::from_absolute_path(&blocked).expect("absolute blocked dir");
        let writable_root_str = path_to_string(writable_root.as_path());
        let blocked_str = path_to_string(blocked.as_path());
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: writable_root,
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: blocked },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");

        assert!(args.args.windows(3).any(|window| {
            window
                == [
                    "--bind",
                    writable_root_str.as_str(),
                    writable_root_str.as_str(),
                ]
        }));
        let blocked_mask_index = args
            .args
            .windows(6)
            .position(|window| {
                window
                    == [
                        "--perms",
                        "000",
                        "--tmpfs",
                        blocked_str.as_str(),
                        "--remount-ro",
                        blocked_str.as_str(),
                    ]
            })
            .expect("blocked directory should be remounted unreadable");

        let writable_root_bind_index = args
            .args
            .windows(3)
            .position(|window| {
                window
                    == [
                        "--bind",
                        writable_root_str.as_str(),
                        writable_root_str.as_str(),
                    ]
            })
            .expect("writable root should be rebound writable");

        assert!(
            writable_root_bind_index < blocked_mask_index,
            "expected unreadable carveout to be re-applied after writable bind: {:#?}",
            args.args
        );
    }

    #[test]
    fn split_policy_reenables_nested_writable_subpaths_after_read_only_parent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let writable_root = temp_dir.path().join("workspace");
        let docs = writable_root.join("docs");
        let docs_public = docs.join("public");
        std::fs::create_dir_all(&docs_public).expect("create docs/public");
        let writable_root =
            AbsolutePathBuf::from_absolute_path(&writable_root).expect("absolute writable root");
        let docs = AbsolutePathBuf::from_absolute_path(&docs).expect("absolute docs");
        let docs_public =
            AbsolutePathBuf::from_absolute_path(&docs_public).expect("absolute docs/public");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: writable_root,
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: docs.clone() },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: docs_public.clone(),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let docs_str = path_to_string(docs.as_path());
        let docs_public_str = path_to_string(docs_public.as_path());
        let docs_ro_index = args
            .args
            .windows(3)
            .position(|window| window == ["--ro-bind", docs_str.as_str(), docs_str.as_str()])
            .expect("docs should be remounted read-only");
        let docs_public_rw_index = args
            .args
            .windows(3)
            .position(|window| {
                window == ["--bind", docs_public_str.as_str(), docs_public_str.as_str()]
            })
            .expect("docs/public should be rebound writable");

        assert!(
            docs_ro_index < docs_public_rw_index,
            "expected read-only parent remount before nested writable bind: {:#?}",
            args.args
        );
    }

    #[test]
    fn split_policy_reenables_writable_subpaths_after_unreadable_parent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let blocked = temp_dir.path().join("blocked");
        let allowed = blocked.join("allowed");
        std::fs::create_dir_all(&allowed).expect("create blocked/allowed");
        let blocked = AbsolutePathBuf::from_absolute_path(&blocked).expect("absolute blocked");
        let allowed = AbsolutePathBuf::from_absolute_path(&allowed).expect("absolute allowed");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: blocked.clone(),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: allowed.clone(),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let blocked_str = path_to_string(blocked.as_path());
        let allowed_str = path_to_string(allowed.as_path());
        let blocked_none_index = args
            .args
            .windows(4)
            .position(|window| window == ["--perms", "111", "--tmpfs", blocked_str.as_str()])
            .expect("blocked should be masked first");
        let allowed_dir_index = args
            .args
            .windows(2)
            .position(|window| window == ["--dir", allowed_str.as_str()])
            .expect("allowed mount target should be recreated");
        let blocked_remount_ro_index = args
            .args
            .windows(2)
            .position(|window| window == ["--remount-ro", blocked_str.as_str()])
            .expect("blocked directory should be remounted read-only");
        let allowed_bind_index = args
            .args
            .windows(3)
            .position(|window| window == ["--bind", allowed_str.as_str(), allowed_str.as_str()])
            .expect("allowed path should be rebound writable");

        assert!(
            blocked_none_index < allowed_dir_index
                && allowed_dir_index < blocked_remount_ro_index
                && blocked_remount_ro_index < allowed_bind_index,
            "expected writable child target recreation before remounting and rebinding under unreadable parent: {:#?}",
            args.args
        );
    }

    #[test]
    fn split_policy_reenables_writable_files_after_unreadable_parent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let blocked = temp_dir.path().join("blocked");
        let allowed_dir = blocked.join("allowed");
        let allowed_file = allowed_dir.join("note.txt");
        std::fs::create_dir_all(&allowed_dir).expect("create blocked/allowed");
        std::fs::write(&allowed_file, "ok").expect("create note");
        let blocked = AbsolutePathBuf::from_absolute_path(&blocked).expect("absolute blocked");
        let allowed_dir =
            AbsolutePathBuf::from_absolute_path(&allowed_dir).expect("absolute allowed dir");
        let allowed_file =
            AbsolutePathBuf::from_absolute_path(&allowed_file).expect("absolute allowed file");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: blocked.clone(),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: allowed_file.clone(),
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let blocked_str = path_to_string(blocked.as_path());
        let allowed_dir_str = path_to_string(allowed_dir.as_path());
        let allowed_file_str = path_to_string(allowed_file.as_path());

        assert!(
            args.args
                .windows(2)
                .any(|window| window == ["--dir", allowed_dir_str.as_str()]),
            "expected ancestor directory to be recreated: {:#?}",
            args.args
        );
        assert!(
            !args
                .args
                .windows(2)
                .any(|window| window == ["--dir", allowed_file_str.as_str()]),
            "writable file target should not be converted into a directory: {:#?}",
            args.args
        );
        let blocked_none_index = args
            .args
            .windows(4)
            .position(|window| window == ["--perms", "111", "--tmpfs", blocked_str.as_str()])
            .expect("blocked should be masked first");
        let allowed_bind_index = args
            .args
            .windows(3)
            .position(|window| {
                window
                    == [
                        "--bind",
                        allowed_file_str.as_str(),
                        allowed_file_str.as_str(),
                    ]
            })
            .expect("allowed file should be rebound writable");

        assert!(
            blocked_none_index < allowed_bind_index,
            "expected unreadable parent mask before rebinding writable file child: {:#?}",
            args.args
        );
    }

    #[test]
    fn split_policy_reenables_nested_writable_roots_after_unreadable_parent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let writable_root = temp_dir.path().join("workspace");
        let blocked = writable_root.join("blocked");
        let allowed = blocked.join("allowed");
        std::fs::create_dir_all(&allowed).expect("create blocked/allowed dir");
        let writable_root =
            AbsolutePathBuf::from_absolute_path(&writable_root).expect("absolute writable root");
        let blocked = AbsolutePathBuf::from_absolute_path(&blocked).expect("absolute blocked dir");
        let allowed = AbsolutePathBuf::from_absolute_path(&allowed).expect("absolute allowed dir");
        let blocked_str = path_to_string(blocked.as_path());
        let allowed_str = path_to_string(allowed.as_path());
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: writable_root,
                },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: blocked },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path { path: allowed },
                access: FileSystemAccessMode::Write,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let blocked_none_index = args
            .args
            .windows(4)
            .position(|window| window == ["--perms", "111", "--tmpfs", blocked_str.as_str()])
            .expect("blocked should be masked first");
        let allowed_dir_index = args
            .args
            .windows(2)
            .position(|window| window == ["--dir", allowed_str.as_str()])
            .expect("allowed mount target should be recreated");
        let allowed_bind_index = args
            .args
            .windows(3)
            .position(|window| window == ["--bind", allowed_str.as_str(), allowed_str.as_str()])
            .expect("allowed path should be rebound writable");

        assert!(
            blocked_none_index < allowed_dir_index && allowed_dir_index < allowed_bind_index,
            "expected unreadable parent mask before recreating and rebinding writable child: {:#?}",
            args.args
        );
    }

    #[test]
    fn split_policy_masks_root_read_directory_carveouts() {
        let temp_dir = TempDir::new().expect("temp dir");
        let blocked = temp_dir.path().join("blocked");
        std::fs::create_dir_all(&blocked).expect("create blocked dir");
        let blocked = AbsolutePathBuf::from_absolute_path(&blocked).expect("absolute blocked dir");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: blocked.clone(),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let blocked_str = path_to_string(blocked.as_path());

        assert!(
            args.args
                .windows(3)
                .any(|window| window == ["--ro-bind", "/", "/"])
        );
        assert!(
            args.args
                .windows(4)
                .any(|window| { window == ["--perms", "000", "--tmpfs", blocked_str.as_str()] })
        );
        assert!(
            args.args
                .windows(2)
                .any(|window| window == ["--remount-ro", blocked_str.as_str()])
        );
    }

    #[test]
    fn split_policy_masks_root_read_file_carveouts() {
        let temp_dir = TempDir::new().expect("temp dir");
        let blocked_file = temp_dir.path().join("blocked.txt");
        std::fs::write(&blocked_file, "secret").expect("create blocked file");
        let blocked_file =
            AbsolutePathBuf::from_absolute_path(&blocked_file).expect("absolute blocked file");
        let policy = FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
                missing_path_behavior: None,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: blocked_file.clone(),
                },
                access: FileSystemAccessMode::Deny,
                missing_path_behavior: None,
            },
        ]);

        let args =
            create_filesystem_args(&policy, temp_dir.path(), NO_UNREADABLE_GLOB_SCAN_MAX_DEPTH)
                .expect("filesystem args");
        let blocked_file_str = path_to_string(blocked_file.as_path());

        assert_eq!(args.preserved_files.len(), 1);
        assert!(args.synthetic_mount_targets.is_empty());
        assert!(args.args.windows(5).any(|window| {
            window[0] == "--perms"
                && window[1] == "000"
                && window[2] == "--ro-bind-data"
                && window[4] == blocked_file_str
        }));
    }

    #[test]
    fn unreadable_globs_expand_existing_matches_with_configured_depth() {
        if !ripgrep_available() {
            return;
        }

        let temp_dir = TempDir::new().expect("temp dir");
        let root_env = temp_dir.path().join(".env");
        let nested_env = temp_dir.path().join("app").join(".env");
        let too_deep_env = temp_dir.path().join("app").join("deep").join(".env");
        std::fs::create_dir_all(too_deep_env.parent().expect("parent")).expect("create parent");
        std::fs::write(temp_dir.path().join(".gitignore"), ".env\n").expect("write gitignore");
        std::fs::write(&root_env, "secret").expect("write root env");
        std::fs::write(&nested_env, "secret").expect("write nested env");
        std::fs::write(&too_deep_env, "secret").expect("write deep env");
        let policy =
            default_policy_with_unreadable_glob(format!("{}/**/*.env", temp_dir.path().display()));

        let args =
            create_filesystem_args(&policy, temp_dir.path(), Some(2)).expect("filesystem args");

        assert_file_masked(&args.args, &root_env);
        assert_file_masked(&args.args, &nested_env);
        assert!(
            !args
                .args
                .iter()
                .any(|arg| arg == &path_to_string(&too_deep_env)),
            "max depth should keep deeper matches out of bwrap args: {:#?}",
            args.args
        );
    }

    #[test]
    fn unreadable_globs_add_canonical_targets_for_symlink_matches() {
        if !ripgrep_available() {
            return;
        }

        let temp_dir = TempDir::new().expect("temp dir");
        let real_root = temp_dir.path().join("real");
        let link_root = temp_dir.path().join("link");
        let real_secret = real_root.join("secret.env");
        std::fs::create_dir_all(&real_root).expect("create real root");
        std::fs::write(&real_secret, "secret").expect("write real secret");
        std::os::unix::fs::symlink(&real_root, &link_root).expect("create symlink");
        let policy =
            default_policy_with_unreadable_glob(format!("{}/**/*.env", link_root.display()));

        let args =
            create_filesystem_args(&policy, temp_dir.path(), Some(2)).expect("filesystem args");

        assert_file_masked(&args.args, &real_secret);
    }

    #[test]
    fn finds_first_executable_bwrap_in_joined_search_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cwd = temp_dir.path().join("cwd");
        let first_dir = temp_dir.path().join("first");
        let second_dir = temp_dir.path().join("second");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        std::fs::create_dir_all(&first_dir).expect("create first dir");
        std::fs::create_dir_all(&second_dir).expect("create second dir");
        std::fs::write(first_dir.join("bwrap"), "not executable")
            .expect("write non-executable bwrap");
        let expected_bwrap = write_named_fake_bwrap_in(&second_dir);

        assert_eq!(
            find_system_bwrap_in_search_paths([first_dir, second_dir], &cwd),
            Some(expected_bwrap)
        );
    }

    #[test]
    fn skips_workspace_local_bwrap_in_joined_search_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let cwd = temp_dir.path().join("cwd");
        let trusted_dir = temp_dir.path().join("trusted");
        std::fs::create_dir_all(&cwd).expect("create cwd");
        std::fs::create_dir_all(&trusted_dir).expect("create trusted dir");
        let _workspace_bwrap = write_named_fake_bwrap_in(&cwd);
        let expected_bwrap = write_named_fake_bwrap_in(&trusted_dir);

        assert_eq!(
            find_system_bwrap_in_search_paths([cwd.clone(), trusted_dir], &cwd),
            Some(expected_bwrap)
        );
    }

    #[test]
    fn root_cwd_does_not_hide_system_bwrap_candidates() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let bin_dir = temp_dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        let expected_bwrap = write_named_fake_bwrap_in(&bin_dir);

        assert_eq!(
            find_system_bwrap_in_search_paths([bin_dir], Path::new("/")),
            Some(expected_bwrap)
        );
    }

    fn write_named_fake_bwrap_in(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("bwrap");
        std::fs::write(&path, "#!/bin/sh\n").expect("write fake bwrap");
        let permissions = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod fake bwrap");
        std::fs::canonicalize(path).expect("canonicalize fake bwrap")
    }

    fn ripgrep_available() -> bool {
        Command::new("rg")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// Assert that `path` is masked due to a bwrap arg sequence like:
    ///
    /// `bwrap ... --perms 000 --ro-bind-data FD PATH`
    fn assert_file_masked(args: &[String], path: &Path) {
        let path = path_to_string(path);
        assert!(
            args.windows(5).any(|window| {
                window[0] == "--perms"
                    && window[1] == "000"
                    && window[2] == "--ro-bind-data"
                    && window[4] == path
            }),
            "expected file mask for {path}: {args:#?}"
        );
    }

    /// Assert that `path` is backed by an fd-supplied empty file without
    /// changing the next mount operation's permissions.
    fn assert_empty_file_bound_without_perms(args: &[String], path: &Path) {
        let path = path_to_string(path);
        assert!(
            args.windows(3)
                .any(|window| { window[0] == "--ro-bind-data" && window[2] == path }),
            "expected empty file bind for {path}: {args:#?}"
        );
        assert!(
            !args.windows(5).any(|window| {
                window[0] == "--perms"
                    && window[1] == "000"
                    && window[2] == "--ro-bind-data"
                    && window[4] == path
            }),
            "missing path bind should not set explicit file perms for {path}: {args:#?}"
        );
    }

    fn assert_empty_directory_mounted_read_only(args: &[String], path: &Path) {
        let path = path_to_string(path);
        assert!(
            args.windows(4)
                .any(|window| window == ["--perms", "555", "--tmpfs", path.as_str()]),
            "expected empty directory mount for {path}: {args:#?}"
        );
        assert!(
            args.windows(2)
                .any(|window| window == ["--remount-ro", path.as_str()]),
            "expected read-only remount for {path}: {args:#?}"
        );
    }

    fn synthetic_mount_target_paths(args: &BwrapArgs) -> Vec<PathBuf> {
        args.synthetic_mount_targets
            .iter()
            .map(|target| target.path().to_path_buf())
            .collect()
    }

    fn protected_create_target_paths(args: &BwrapArgs) -> Vec<PathBuf> {
        args.protected_create_targets
            .iter()
            .map(|target| target.path().to_path_buf())
            .collect()
    }
}

#[cfg(target_os = "linux")]
mod linux_probe {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::time::Duration;
    use std::time::Instant;

    #[test]
    fn system_bwrap_warning_reports_user_namespace_failures() {
        for failure in USER_NAMESPACE_FAILURES {
            let fake_bwrap = write_fake_bwrap(&format!(
                r#"#!/bin/sh
echo '{failure}' >&2
exit 1
"#
            ));
            let fake_bwrap_path: &Path = fake_bwrap.as_ref();

            assert_eq!(
                system_bwrap_warning_for_path(Some(fake_bwrap_path)),
                Some(USER_NAMESPACE_WARNING.to_string()),
                "{failure}",
            );
        }
    }

    #[test]
    fn system_bwrap_warning_skips_unrelated_bwrap_failures() {
        let fake_bwrap = write_fake_bwrap(
            r#"#!/bin/sh
echo 'bwrap: Unknown option --argv0' >&2
exit 1
"#,
        );
        let fake_bwrap_path: &Path = fake_bwrap.as_ref();

        assert_eq!(system_bwrap_warning_for_path(Some(fake_bwrap_path)), None);
    }

    #[test]
    fn system_bwrap_probe_times_out_without_reporting_a_warning() {
        let fake_bwrap = write_fake_bwrap(
            r#"#!/bin/sh
sleep 1
exit 0
"#,
        );
        let fake_bwrap_path: &Path = fake_bwrap.as_ref();
        let started_at = Instant::now();

        assert!(system_bwrap_has_user_namespace_access(
            fake_bwrap_path,
            Duration::from_millis(10),
        ));
        assert!(started_at.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn system_bwrap_probe_does_not_wait_for_descendants_holding_stderr_open() {
        let fake_bwrap = write_fake_bwrap(
            r#"#!/bin/sh
echo 'No permissions to create a new namespace' >&2
sleep 1 &
exit 1
"#,
        );
        let fake_bwrap_path: &Path = fake_bwrap.as_ref();
        let started_at = Instant::now();

        assert!(!system_bwrap_has_user_namespace_access(
            fake_bwrap_path,
            Duration::from_millis(100),
        ));
        assert!(started_at.elapsed() < Duration::from_millis(500));
    }

    fn write_fake_bwrap(contents: &str) -> tempfile::TempPath {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::NamedTempFile;

        // Bazel can mount the OS temp directory `noexec`, so prefer the current
        // working directory for fake executables and fall back to the default
        // temp dir outside that environment.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let temp_file = NamedTempFile::new_in(cwd)
            .ok()
            .unwrap_or_else(|| NamedTempFile::new().expect("temp file"));
        // Linux rejects exec-ing a file that is still open for writing.
        let path = temp_file.into_temp_path();
        std::fs::write(&path, contents).expect("write fake bwrap");
        let permissions = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod fake bwrap");
        path
    }
}
