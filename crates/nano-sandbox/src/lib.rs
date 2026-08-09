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

#[cfg(target_os = "windows")]
pub mod acl;
#[cfg(target_os = "windows")]
pub mod allow;
#[cfg(target_os = "windows")]
pub mod audit;
#[cfg(target_os = "windows")]
pub mod cap;
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
pub mod env;
#[cfg(target_os = "windows")]
pub mod gather;
#[cfg(target_os = "windows")]
pub mod helper_materialization;
#[cfg(target_os = "windows")]
pub mod identity;
#[cfg(target_os = "windows")]
pub mod job;
#[cfg(target_os = "windows")]
pub mod process;
#[cfg(target_os = "windows")]
pub mod resolved_permissions;
#[cfg(target_os = "windows")]
pub mod setup_error;
#[cfg(target_os = "windows")]
pub mod sandbox_utils;
#[cfg(target_os = "windows")]
pub mod setup_exec;
#[cfg(target_os = "windows")]
pub mod spawn_prep;
#[cfg(target_os = "windows")]
pub mod spawn_types;
#[cfg(target_os = "windows")]
pub mod stdio_bridge;
#[cfg(target_os = "windows")]
pub mod setup_types;
#[cfg(target_os = "windows")]
mod ssh_config_dependencies;
#[cfg(target_os = "windows")]
pub mod logging;
#[cfg(target_os = "windows")]
pub mod proc_thread_attr;
#[cfg(target_os = "windows")]
mod path_normalization;
#[cfg(target_os = "windows")]
pub mod token;
#[cfg(target_os = "windows")]
pub mod wfp;
#[cfg(target_os = "windows")]
pub mod workspace_acl;
#[cfg(target_os = "windows")]
pub mod winutil;

#[cfg(target_os = "windows")]
pub use path_normalization::{canonical_path_key, canonicalize_path};
#[cfg(target_os = "windows")]
pub use token::{LocalSid, world_sid};

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
pub use setup_types::{sandbox_bin_dir, sandbox_secrets_dir, setup_marker_path, sandbox_users_path};

#[cfg(target_os = "windows")]
pub use nano_core::permissions::WindowsSandboxProxySettingsMode;
