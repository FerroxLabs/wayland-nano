//! nano-mcp — MCP client (stdio, streamable HTTP).
//!
//! Rules (audit invariants):
//! - full-duplex dispatcher (P3 §2): reader/handler/writer threads, a
//!   supervisor as sole owner of child-kill and joins, state-only
//!   idempotent poison, bounded queues and bounded lines, absolute
//!   once-only-extensible deadlines;
//! - timeouts and cancellation threaded everywhere; bounded output;
//! - crash containment: a dying server never takes the runtime down, and a
//!   poisoned connection fails every subsequent call with the same typed
//!   error (reconnect policy is a registry decision, §15);
//! - MCP servers get no security bypass: HTTP flows through nano-egress
//!   only. Stdio children CURRENTLY spawn via raw std::process; the
//!   contained spawn (`spawn_process_with_pipes_contained`, job-object
//!   teardown) is the containment lane's seam at `StdioTransport::spawn`,
//!   and the stdio-MCP capability flag stays FALSE until the §13 leg-1b
//!   direct-descendant kill proofs pass (P3 §2.6, D13).

pub mod client;
pub mod dispatcher;
pub mod http;
pub mod protocol;
pub mod stdio;
