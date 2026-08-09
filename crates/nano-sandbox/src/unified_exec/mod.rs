//! Unified exec session spawner for Windows sandboxing (thin orchestration).
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/unified_exec/mod.rs`
//! @ 646f7c0a. Transformations: codex_home -> nano_home; SpawnedProcess ->
//! crate::spawn_types; WindowsSandboxLevel from nano_core::permissions;
//! elevated/proxy routing fails closed typed until B-SBX-10 lands the runner
//! IPC (D8).

pub mod backends;

use crate::spawn_types::SpawnedProcess;
use anyhow::Result;
use anyhow::bail;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::PermissionProfile;
use nano_core::permissions::WindowsSandboxLevel;
use nano_core::permissions::WindowsSandboxProxySettingsMode;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

/// Explicitly advertised capability failures (never silent fallback).
#[derive(Debug, thiserror::Error)]
pub enum SandboxUnavailable {
    #[error("ConPTY interactive sessions are deferred in v1 (D8): spawn non-interactive instead")]
    ConPtyDeferred,
    #[error(
        "elevated backend not yet wired (B-SBX-10): restricted-token backend cannot enforce this request"
    )]
    ElevatedBackendPending,
}

/// Fully resolved Windows sandbox session launch request.
pub struct WindowsSandboxSessionRequest<'a> {
    pub permission_profile: &'a PermissionProfile,
    pub workspace_roots: &'a [AbsolutePathBuf],
    pub nano_home: &'a Path,
    pub command: Vec<String>,
    pub cwd: &'a Path,
    pub env_map: HashMap<String, String>,
    pub windows_sandbox_level: WindowsSandboxLevel,
    pub proxy_enforced: bool,
    pub network_proxy_restricting_sid: Option<String>,
    pub proxy_settings_mode: WindowsSandboxProxySettingsMode,
    pub timeout_ms: Option<u64>,
    pub read_roots_override: Option<&'a [PathBuf]>,
    pub read_roots_include_platform_defaults: bool,
    pub write_roots_override: Option<&'a [PathBuf]>,
    pub deny_read_paths_override: &'a [AbsolutePathBuf],
    pub deny_write_paths_override: &'a [AbsolutePathBuf],
    pub tty: bool,
    pub stdin_open: bool,
    pub use_private_desktop: bool,
}

pub async fn spawn_windows_sandbox_session_for_level(
    request: WindowsSandboxSessionRequest<'_>,
) -> Result<SpawnedProcess> {
    if request.tty {
        return Err(SandboxUnavailable::ConPtyDeferred.into());
    }
    if request.proxy_enforced
        || matches!(request.windows_sandbox_level, WindowsSandboxLevel::Elevated)
        || request.network_proxy_restricting_sid.is_some()
    {
        return backends::elevated::spawn_windows_sandbox_session_elevated_for_permission_profile(
            request.permission_profile,
            request.workspace_roots,
            request.nano_home,
            request.command,
            request.cwd,
            request.env_map,
            request.proxy_enforced,
            request.network_proxy_restricting_sid,
            request.proxy_settings_mode,
            request.timeout_ms,
            request.read_roots_override,
            request.read_roots_include_platform_defaults,
            request.write_roots_override,
            request.deny_read_paths_override,
            request.deny_write_paths_override,
            request.stdin_open,
            request.use_private_desktop,
        )
        .await;
    }
    if matches!(request.windows_sandbox_level, WindowsSandboxLevel::Disabled) {
        bail!("windows sandbox level is disabled");
    }
    backends::legacy::spawn_windows_sandbox_session_legacy(
        request.permission_profile,
        request.workspace_roots,
        request.nano_home,
        request.command,
        request.cwd,
        request.env_map,
        request.timeout_ms,
        request.deny_read_paths_override,
        request.deny_write_paths_override,
        request.stdin_open,
        request.use_private_desktop,
    )
    .await
}
