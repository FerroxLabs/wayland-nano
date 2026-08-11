//! Linux sandbox argv builder: transforms a command + permission profile
//! into a `wayland-nano-linux-sandbox` helper invocation. The caller spawns the
//! helper binary with the returned argv.
//!
//! Provenance: ported from Codex `codex-rs/sandboxing/src/landlock.rs`
//! @ 646f7c0a. Transformations:
//! - codex_protocol::models::PermissionProfile -> nano_core::permissions::PermissionProfile
//!   (nano-k3's profile is an enum; its serde shape is the CLI contract);
//! - `codex-linux-sandbox` -> `wayland-nano-linux-sandbox` (wayland-nano-* namespacing);
//! - managed-network-proxy surface DROPPED (nano-egress owns egress):
//!   `allow_network_for_proxy` and the `--allow-network-for-proxy` flag are
//!   not ported; proxy-only networking required bubblewrap, which is also
//!   not ported in this pass (see `src/bin/linux_sandbox/`);
//! - compiled on Linux and in test builds everywhere (argv construction is
//!   pure string building).

use nano_core::permissions::PermissionProfile;
use std::path::Path;

/// Basename used when a Wayland Nano executable self-invokes as the Linux sandbox
/// helper.
pub const NANO_LINUX_SANDBOX_ARG0: &str = "wayland-nano-linux-sandbox";

/// Converts the permission profile into the CLI invocation for
/// `wayland-nano-linux-sandbox`.
///
/// The helper performs the actual sandboxing (legacy Landlock + seccomp)
/// after parsing these arguments. The profile JSON flag is emitted before
/// helper feature flags so the argv order matches the helper's CLI shape.
pub fn create_linux_sandbox_command_args_for_permission_profile(
    command: Vec<String>,
    command_cwd: &Path,
    permission_profile: &PermissionProfile,
    sandbox_policy_cwd: &Path,
    use_legacy_landlock: bool,
) -> Vec<String> {
    let permission_profile_json = serde_json::to_string(permission_profile)
        .unwrap_or_else(|err| panic!("failed to serialize permission profile: {err}"));
    let sandbox_policy_cwd = sandbox_policy_cwd
        .to_str()
        .unwrap_or_else(|| panic!("cwd must be valid UTF-8"))
        .to_string();
    let command_cwd = command_cwd
        .to_str()
        .unwrap_or_else(|| panic!("command cwd must be valid UTF-8"))
        .to_string();

    let mut linux_cmd: Vec<String> = vec![
        "--sandbox-policy-cwd".to_string(),
        sandbox_policy_cwd,
        "--command-cwd".to_string(),
        command_cwd,
        "--permission-profile".to_string(),
        permission_profile_json,
    ];
    if use_legacy_landlock {
        linux_cmd.push("--use-legacy-landlock".to_string());
    }
    // Separator so that command arguments starting with `-` are not parsed as
    // options of the helper itself.
    linux_cmd.push("--".to_string());
    // Append the original tool command.
    linux_cmd.extend(command);
    linux_cmd
}

#[cfg(test)]
#[path = "linux_landlock_tests.rs"]
mod tests;
