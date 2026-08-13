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
//!   only. Stdio children spawn CONTAINED on Windows (F-P3-2, §2.6):
//!   `spawn_process_with_pipes_contained` (NO-BREAKAWAY job object,
//!   KILL_ON_JOB_CLOSE) with job-object teardown in the supervisor; unix
//!   keeps the std spawn for v1 (recorded deviation, `stdio.rs`). The
//!   stdio-MCP capability flag stays FALSE until the §13 leg-1b
//!   direct-descendant kill proofs pass (P3 §2.6, D13).

pub mod client;
pub mod dispatcher;
pub mod http;
pub mod oauth;
pub mod protocol;
pub mod stdio;
