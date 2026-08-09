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
pub mod op;
pub mod reader;
pub mod replay;
pub mod writer;

pub use op::{Op, OpEnvelope, TurnOutcome, SCHEMA_VERSION};
pub use reader::{JournalReport, read_journal};
pub use replay::{CompactionPhase, SessionState};
pub use writer::JournalWriter;

#[cfg(test)]
mod tests;
