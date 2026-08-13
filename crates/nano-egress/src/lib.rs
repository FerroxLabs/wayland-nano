//! nano-egress — the single outbound-HTTP chokepoint.
//!
//! Every Nano-owned outbound request passes through this crate with a
//! fail-closed policy gate. Workspace clippy bans raw reqwest elsewhere.
//! Observability: method/host/port + path_query_sha256 only — headers and
//! bodies are never logged.

pub mod client;
pub mod grant;
pub mod policy;
pub mod redact;
