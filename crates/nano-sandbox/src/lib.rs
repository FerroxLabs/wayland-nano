//! nano-sandbox — containment profiles (ported Codex base).
//!
//! Windows: dual-identity pattern (restricted/online + offline identity,
//! WFP block-all with loopback-proxy carve-out). Fail closed:
//! SANDBOX_UNAVAILABLE, never silent downgrade.
//!
//! Provenance: ported from OpenAI Codex `codex-rs/windows-sandbox-rs`
//! @ 646f7c0a (Apache-2.0, see ../../vendor/NOTICE). Per-file donor mapping
//! and transformations are recorded in ../../UPSTREAM.md.
#![cfg_attr(not(target_os = "windows"), allow(unused))]

pub mod telemetry;

#[cfg(target_os = "windows")]
mod path_normalization;
#[cfg(target_os = "windows")]
pub mod token;
#[cfg(target_os = "windows")]
pub mod winutil;

#[cfg(target_os = "windows")]
pub use path_normalization::{canonical_path_key, canonicalize_path};
#[cfg(target_os = "windows")]
pub use token::{LocalSid, world_sid};
