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
pub mod op;
pub mod reader;
pub mod redaction;
pub mod replay;
pub mod writer;

pub use error_kind::NanoErrorKind;
pub use op::{CompactionCancelReason, Op, OpEnvelope, SCHEMA_VERSION, TurnOutcome};
pub use reader::{JournalReport, read_journal};
pub use redaction::{RedactionError, SecretKind, scan_for_secrets};
pub use replay::{CompactionPhase, SessionState};
pub use writer::JournalWriter;

#[cfg(test)]
mod tests;
