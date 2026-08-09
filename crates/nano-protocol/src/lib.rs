//! nano-protocol — Desktop wire protocol (versioned, NDJSON stdio).
//!
//! Contract (from the audited Desktop integration surface):
//! - NDJSON over stdin/stdout; the engine emits `ready` FIRST — before any
//!   other event — carrying version, session_id, and capabilities;
//! - turn-scoped frames flow during turns (a host-side watchdog kills quiet
//!   turns after ~10 idle minutes — every model step must frame);
//! - malformed input never kills the engine: it gets an `error` frame;
//! - clean shutdown on stdin close; `ping` → `pong` (heartbeats do NOT count
//!   as turn progress).

pub mod codec;
pub mod corpus;
pub mod host;
pub mod messages;
pub mod profile;
