//! Provenance: ported from Codex `codex-rs/sandboxing/src/landlock_tests.rs`
//! @ 646f7c0a. Transformations: proxy-flag tests dropped with the
//! managed-proxy surface; remaining tests target the permission-profile
//! builder directly.

use super::*;
use pretty_assertions::assert_eq;

#[test]
fn legacy_landlock_flag_is_included_when_requested() {
    let command = vec!["/bin/true".to_string()];
    let command_cwd = Path::new("/tmp/link");
    let cwd = Path::new("/tmp");
    let permission_profile = PermissionProfile::read_only();

    let default_args = create_linux_sandbox_command_args_for_permission_profile(
        command.clone(),
        command_cwd,
        &permission_profile,
        cwd,
        /*use_legacy_landlock*/ false,
    );
    assert_eq!(
        default_args.contains(&"--use-legacy-landlock".to_string()),
        false
    );

    let legacy_landlock = create_linux_sandbox_command_args_for_permission_profile(
        command,
        command_cwd,
        &permission_profile,
        cwd,
        /*use_legacy_landlock*/ true,
    );
    assert_eq!(
        legacy_landlock.contains(&"--use-legacy-landlock".to_string()),
        true
    );
}

#[test]
fn permission_profile_flag_is_included() {
    let command = vec!["/bin/true".to_string()];
    let command_cwd = Path::new("/tmp/link");
    let cwd = Path::new("/tmp");
    let permission_profile = PermissionProfile::read_only();

    let args = create_linux_sandbox_command_args_for_permission_profile(
        command,
        command_cwd,
        &permission_profile,
        cwd,
        /*use_legacy_landlock*/ true,
    );

    assert_eq!(
        args.windows(2)
            .any(|window| { window[0] == "--permission-profile" && !window[1].is_empty() }),
        true
    );
    assert_eq!(
        args.windows(2)
            .any(|window| window[0] == "--command-cwd" && window[1] == "/tmp/link"),
        true
    );
}

#[test]
fn permission_profile_json_round_trips_through_helper_cli_contract() {
    // The `--permission-profile` payload is the CLI contract with the
    // nanok3-linux-sandbox helper: it must deserialize back into the same
    // profile.
    let permission_profile = PermissionProfile::workspace_write();
    let args = create_linux_sandbox_command_args_for_permission_profile(
        vec!["/bin/true".to_string()],
        Path::new("/tmp"),
        &permission_profile,
        Path::new("/tmp"),
        /*use_legacy_landlock*/ true,
    );

    let json = args
        .windows(2)
        .find_map(|window| (window[0] == "--permission-profile").then(|| window[1].clone()))
        .expect("permission profile flag");
    let parsed: PermissionProfile =
        serde_json::from_str(&json).expect("permission profile JSON should round-trip");
    assert_eq!(parsed, permission_profile);
}

#[test]
fn helper_arg0_uses_nanok3_namespace() {
    assert_eq!(NANOK3_LINUX_SANDBOX_ARG0, "nanok3-linux-sandbox");
}
