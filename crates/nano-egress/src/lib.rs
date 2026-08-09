//! nano-egress — the single outbound-HTTP chokepoint.
//!
//! Every Nano-owned outbound request passes through this crate with a
//! fail-closed policy gate. Workspace clippy bans raw reqwest elsewhere.
//! Observability: method/host/port + path_query_sha256 only — headers and
//! bodies are never logged.

// The only crate permitted to construct HTTP clients.
#[allow(clippy::disallowed_methods)]
pub mod client {}
