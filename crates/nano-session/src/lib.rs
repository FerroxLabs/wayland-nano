//! nano-session — append-only Op journal, replay, migrations, compaction.
//!
//! Model (Kimi `wire.jsonl` invariants, original Nano implementation):
//! - append-only typed Op journal, one JSON envelope per line;
//! - reducer-fold replay reconstructs executable session state;
//! - versioned envelopes; unknown Op types are skipped (journals survive
//!   feature addition/removal in both directions);
//! - torn-tail tolerance: a crash-truncated final line is dropped, earlier
//!   records stay authoritative; malformed middle records are integrity errors;
//! - stranded `running` phases (mid-turn, mid-compaction) reset to safe states
//!   on restore — no duplicate side effects after a crash;
//! - compaction is a set of replayable Ops, never a remote operation.

pub mod compact;
pub mod error_codes;
pub mod error_kind;
pub mod fork;
pub mod lock;
pub mod op;
pub mod reader;
pub mod redaction;
pub mod replay;
pub mod writer;

pub use error_kind::NanoErrorKind;
pub use fork::{ForkError, ForkOutcome, ForkPoint, fork_journal};
pub use lock::{FileLock, LockError};
pub use op::{
    CompactionCancelReason, ESTIMATION_METHOD_VERSION, GoalBudgets, GoalOutcome, GoalReason,
    GoalStatusKind, MAX_GOAL_OBJECTIVE_LEN, MAX_GOAL_SUMMARY_LEN, Op, OpEnvelope, SCHEMA_VERSION,
    TurnOutcome, TurnUsage, UsageSource,
};
pub use reader::{JournalReport, read_journal};
pub use redaction::{RedactionError, SecretKind, scan_for_secrets};
pub use replay::{CompactionPhase, GoalLive, ReplayError, SessionState};
pub use writer::JournalWriter;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "fork_tests.rs"]
mod fork_tests;
