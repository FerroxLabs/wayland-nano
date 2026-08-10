//! nano-sandbox — containment profiles (ported Codex base).
//!
//! Windows: dual-identity pattern (restricted/online + offline identity,
//! WFP block-all with loopback-proxy carve-out). Fail closed:
//! SANDBOX_UNAVAILABLE, never silent downgrade.
//!
//! Provenance: ported from OpenAI Codex `codex-rs/windows-sandbox-rs`
//! @ 646f7c0a (Apache-2.0, see ../../vendor/NOTICE). Per-file donor mapping
//! and transformations are recorded in ../../UPSTREAM.md.
// Rust 2024 surfaces this lint across ported donor code; matches the donor crate-root allow.
#![allow(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(target_os = "windows"), allow(unused))]

pub mod telemetry;

// Unix containment backends. The policy/argv builders are pure string
// construction, so they also compile in test builds on every host to
// maximize cross-platform test coverage; only the spawned helpers are
// platform-bound.
#[cfg(any(target_os = "linux", test))]
pub mod linux_bwrap;
#[cfg(any(target_os = "linux", test))]
pub mod linux_landlock;
#[cfg(any(target_os = "macos", test))]
pub mod macos_seatbelt;

/// Sandbox backend selected for the current platform.
///
/// Provenance: codex `codex-rs/sandboxing/src/manager.rs` @ 646f7c0a
/// (`SandboxType` + `get_platform_sandbox`, verbatim). Adaptation: the
/// Windows variant maps to the existing restricted-token backend in this
/// crate; `SandboxablePreference` and the SandboxManager/spawn machinery are
/// intentionally not ported — callers transform argv via the per-platform
/// modules (`macos_seatbelt`, `linux_landlock`) and spawn themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxType {
    None,
    MacosSeatbelt,
    LinuxSeccomp,
    WindowsRestrictedToken,
}

impl SandboxType {
    pub fn as_metric_tag(self) -> &'static str {
        match self {
            SandboxType::None => "none",
            SandboxType::MacosSeatbelt => "seatbelt",
            SandboxType::LinuxSeccomp => "seccomp",
            SandboxType::WindowsRestrictedToken => "windows_sandbox",
        }
    }
}

pub fn get_platform_sandbox(windows_sandbox_enabled: bool) -> Option<SandboxType> {
    if cfg!(target_os = "macos") {
        Some(SandboxType::MacosSeatbelt)
    } else if cfg!(target_os = "linux") {
        Some(SandboxType::LinuxSeccomp)
    } else if cfg!(target_os = "windows") {
        if windows_sandbox_enabled {
            Some(SandboxType::WindowsRestrictedToken)
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
pub mod acl;
#[cfg(target_os = "windows")]
pub mod allow;
#[cfg(target_os = "windows")]
pub mod audit;
#[cfg(target_os = "windows")]
pub mod cap;
#[cfg(target_os = "windows")]
pub mod capture;
#[cfg(target_os = "windows")]
pub mod deny_read_acl;
#[cfg(target_os = "windows")]
pub mod deny_read_resolver;
#[cfg(target_os = "windows")]
pub mod deny_read_state;
#[cfg(target_os = "windows")]
pub mod desktop;
#[cfg(target_os = "windows")]
pub mod dpapi;
#[cfg(target_os = "windows")]
pub mod elevated;
#[cfg(target_os = "windows")]
pub mod elevated_impl;
#[cfg(target_os = "windows")]
pub mod env;
#[cfg(target_os = "windows")]
pub mod gather;
#[cfg(target_os = "windows")]
pub mod helper_materialization;
#[cfg(target_os = "windows")]
pub mod hide_users;
#[cfg(target_os = "windows")]
pub mod identity;
#[cfg(target_os = "windows")]
pub mod job;
#[cfg(target_os = "windows")]
pub mod logging;
#[cfg(target_os = "windows")]
mod path_normalization;
#[cfg(target_os = "windows")]
pub mod proc_thread_attr;
#[cfg(target_os = "windows")]
pub mod process;
#[cfg(target_os = "windows")]
pub mod resolved_permissions;
#[cfg(target_os = "windows")]
pub mod sandbox_utils;
#[cfg(target_os = "windows")]
pub mod setup_error;
#[cfg(target_os = "windows")]
pub mod setup_exec;
#[cfg(target_os = "windows")]
pub mod setup_types;
#[cfg(target_os = "windows")]
pub mod spawn_prep;
#[cfg(target_os = "windows")]
pub mod spawn_types;
#[cfg(target_os = "windows")]
mod ssh_config_dependencies;
#[cfg(target_os = "windows")]
pub mod stdio_bridge;
#[cfg(target_os = "windows")]
pub mod token;
#[cfg(target_os = "windows")]
pub mod unified_exec;
#[cfg(target_os = "windows")]
pub mod wfp;
#[cfg(target_os = "windows")]
pub mod wfp_setup;
#[cfg(target_os = "windows")]
pub mod winutil;
#[cfg(target_os = "windows")]
pub mod workspace_acl;
#[cfg(target_os = "windows")]
pub mod wrapper;

#[cfg(target_os = "windows")]
pub use path_normalization::canonical_path_key;
#[cfg(target_os = "windows")]
pub use token::world_sid;

/// Base directory for sandbox-owned state under a Nano home.
///
/// Interim home: lands in this crate root until the `setup` module is ported
/// (donor location: `setup.rs`). Provenance: Codex
/// `windows-sandbox-rs/src/setup.rs` @ 646f7c0a (`sandbox_dir`).
#[cfg(target_os = "windows")]
pub fn sandbox_dir(nano_home: &std::path::Path) -> std::path::PathBuf {
    nano_home.join(".sandbox")
}

#[cfg(target_os = "windows")]
pub use setup_types::{
    sandbox_bin_dir, sandbox_secrets_dir, sandbox_users_path, setup_marker_path,
};

#[cfg(target_os = "windows")]
pub use nano_core::permissions::WindowsSandboxProxySettingsMode;

#[cfg(target_os = "windows")]
pub use elevated::ipc_framed;
#[cfg(target_os = "windows")]
pub use elevated::runner_client;
#[cfg(target_os = "windows")]
pub use elevated::runner_pipe;

/// Cancellation hook used by Windows sandbox capture backends.
///
/// Provenance: codex `windows-sandbox-rs/src/lib.rs` @ 646f7c0a (verbatim).
#[derive(Clone)]
pub struct WindowsSandboxCancellationToken {
    is_cancelled: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
}

impl WindowsSandboxCancellationToken {
    /// Creates a token backed by a cancellation predicate.
    pub fn new(is_cancelled: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            is_cancelled: std::sync::Arc::new(is_cancelled),
        }
    }

    /// Returns whether the caller has requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        (self.is_cancelled)()
    }
}

impl std::fmt::Debug for WindowsSandboxCancellationToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsSandboxCancellationToken")
            .finish_non_exhaustive()
    }
}

// Donor-parity public surface used by the helper bins and callers.
// (setup:: -> setup_types/setup_exec/gather; windows_impl:: -> capture::;
//  conpty exports omitted — D8.)
#[cfg(target_os = "windows")]
pub use acl::{
    add_deny_read_ace, add_deny_write_ace, allow_null_device, ensure_allow_mask_aces,
    ensure_allow_mask_aces_with_inheritance, ensure_allow_write_aces, fetch_dacl_handle,
    path_mask_allows, path_write_aces_need_refresh,
};
#[cfg(target_os = "windows")]
pub use audit::apply_world_writable_scan_and_denies_for_permissions;
#[cfg(target_os = "windows")]
pub use cap::{
    load_or_create_cap_sids, workspace_cap_sid_for_cwd, workspace_write_cap_sid_for_root,
    workspace_write_root_contains_path, workspace_write_root_overlaps_path,
};
#[cfg(target_os = "windows")]
pub use capture::{
    CaptureResult, run_windows_sandbox_capture,
    run_windows_sandbox_capture_with_filesystem_overrides, run_windows_sandbox_legacy_preflight,
};
#[cfg(target_os = "windows")]
pub use deny_read_acl::{apply_deny_read_acls, plan_deny_read_acl_paths};
#[cfg(target_os = "windows")]
pub use deny_read_resolver::resolve_windows_deny_read_paths;
#[cfg(target_os = "windows")]
pub use deny_read_state::sync_persistent_deny_read_acls;
#[cfg(target_os = "windows")]
pub use desktop::LaunchDesktop;
#[cfg(target_os = "windows")]
pub use dpapi::{protect as dpapi_protect, unprotect as dpapi_unprotect};
#[cfg(target_os = "windows")]
pub use elevated_impl::ElevatedSandboxProfileCaptureRequest;
#[cfg(target_os = "windows")]
pub use helper_materialization::{resolve_current_exe_for_launch, resolve_exe_for_launch};
#[cfg(target_os = "windows")]
pub use hide_users::{hide_current_user_profile_dir, hide_newly_created_users};
#[cfg(target_os = "windows")]
pub use identity::{require_logon_sandbox_creds, sandbox_setup_is_complete};
#[cfg(target_os = "windows")]
pub use ipc_framed::{
    ErrorPayload, ErrorStage, ExitPayload, FramedMessage, IPC_PROTOCOL_VERSION, Message,
    OutputPayload, OutputStream, ResizePayload, SpawnReady, SpawnRequest, decode_bytes,
    encode_bytes, read_frame, write_frame,
};
#[cfg(target_os = "windows")]
pub use logging::{
    current_log_file_path, current_log_file_path_for_nano_home, log_file_path_for_utc_date,
    log_note, log_writer,
};
#[cfg(target_os = "windows")]
pub use path_normalization::canonicalize_path;
#[cfg(target_os = "windows")]
pub use process::{
    ConsoleMode, PipeSpawnHandles, StderrMode, StdinMode, create_process_as_user, read_handle_loop,
    spawn_process_with_pipes,
};
#[cfg(target_os = "windows")]
pub use resolved_permissions::{
    ResolvedWindowsSandboxPermissions, WindowsSandboxTokenMode, token_mode_for_permission_profile,
};
#[cfg(target_os = "windows")]
pub use setup_error::{
    SetupErrorCode, SetupErrorReport, SetupFailure, extract_failure as extract_setup_failure,
    sanitize_setup_metric_tag_value, setup_error_path, write_setup_error_report,
};
#[cfg(target_os = "windows")]
pub use setup_exec::{
    SandboxSetupRequest, run_elevated_provisioning_setup, run_elevated_setup, run_setup_refresh,
    run_setup_refresh_with_extra_read_roots,
};
#[cfg(target_os = "windows")]
pub use setup_types::{SETUP_VERSION, SetupRootOverrides};
#[cfg(target_os = "windows")]
pub use stdio_bridge::forward_sandbox_session_stdio;
#[cfg(target_os = "windows")]
pub use telemetry::{MetricsSink, TelemetrySettings};
#[cfg(target_os = "windows")]
pub use token::{
    LocalSid, convert_string_sid_to_sid, create_readonly_token_with_cap_from,
    create_readonly_token_with_caps_and_user_from, create_readonly_token_with_caps_from,
    create_workspace_write_token_with_caps_and_user_from,
    create_workspace_write_token_with_caps_from, get_current_token_for_restriction,
};
#[cfg(target_os = "windows")]
pub use unified_exec::backends::elevated::spawn_windows_sandbox_session_elevated_for_permission_profile;
#[cfg(target_os = "windows")]
pub use unified_exec::backends::legacy::spawn_windows_sandbox_session_legacy;
#[cfg(target_os = "windows")]
pub use unified_exec::{WindowsSandboxSessionRequest, spawn_windows_sandbox_session_for_level};
#[cfg(target_os = "windows")]
pub use wfp::install_wfp_filters_for_account;
#[cfg(target_os = "windows")]
pub use wfp_setup::install_wfp_filters;
#[cfg(target_os = "windows")]
pub use wfp_setup::uninstall_wfp_filters;
#[cfg(target_os = "windows")]
pub use winutil::{string_from_sid_bytes, to_wide};
