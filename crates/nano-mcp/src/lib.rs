//! nano-mcp — MCP client (stdio, streamable HTTP).
//!
//! Rules (audit invariants):
//! - timeouts and cancellation threaded everywhere; bounded output;
//! - crash containment: a dying server never takes the runtime down;
//! - reconnect-once with honestly-documented at-least-once semantics
//!   (if the transport died after the server executed the call, the retry
//!   executes it again — the trade-off is explicit, not hidden);
//! - MCP servers get no security bypass: stdio children run under the
//!   sandbox, HTTP flows through nano-egress only.

pub mod client;
pub mod http;
pub mod oauth;
pub mod protocol;
pub mod stdio;
