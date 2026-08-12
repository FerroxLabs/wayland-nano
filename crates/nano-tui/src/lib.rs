//! nano-tui — the Wayland Nano terminal UI (design doc:
//! shared/reviews/panel-tui/DESIGN-DRAFT.md, panel-CERTIFIED).
//!
//! An ACP subprocess client: spawns `wayland-nano acp-host` and speaks the
//! hand-rolled JSON-RPC subset (acp_client.rs). It links no ENGINE crates;
//! the only nano link is the C7 error vocabulary + presentation table
//! (nano-session's serde-only `NanoErrorKind` / `error_codes`, sanctioned
//! by the C7 design §2.1) so typed errors render as table presentations,
//! never raw wire strings.
//! Fail-closed everywhere: default-deny approvals with explicit decisions
//! only, streaming-safe terminal-sequence sanitization on all rendered
//! text, no TUI-side credential handling.

pub mod acp_client;
pub mod app;
pub mod composer;
pub mod doctor;
pub mod event;
pub mod fake_host;
pub mod frame_requester;
pub mod modal;
pub mod render;
pub mod sanitize;
pub mod slash_commands;
pub mod status;
pub mod transcript;

#[cfg(any(windows, test))]
pub mod windows_console;
