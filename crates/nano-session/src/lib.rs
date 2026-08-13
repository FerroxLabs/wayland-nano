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

pub mod attachment_store;
pub mod compact;
pub mod coordinator;
pub mod error_codes;
pub mod error_kind;
pub mod fork;
pub mod lock;
pub mod op;
pub mod reader;
pub mod redaction;
pub mod replay;
pub mod writer;

pub use attachment_store::{
    AttachmentStore, AttachmentStoreError, BlobReadError, GC_GRACE_SECS, MAX_BLOB_READ_BYTES,
    SweepReport, WriteLease, attachment_unavailable_placeholder, is_valid_digest,
};
pub use coordinator::{CompactionGuard, JournalCoordinator, hydration_carry_at};
pub use error_kind::NanoErrorKind;
pub use fork::{ForkError, ForkOutcome, ForkPoint, fork_journal};
pub use lock::{FileLock, LockError, LockMode};
pub use op::{
    CompactionCancelReason, DIGEST_HEX_CHARS, ESTIMATION_METHOD_VERSION, GoalBudgets, GoalOutcome,
    GoalReason, GoalStatusKind, GrantEndpoint, GrantMethod, HydrationCarryEntry, HydrationEntry,
    ImageRef, InputBlock, MAX_AS_ORIGIN_CHARS, MAX_ELICITATION_REQUEST_ID_CHARS,
    MAX_GOAL_OBJECTIVE_LEN, MAX_GOAL_SUMMARY_LEN, MAX_GRANT_ENDPOINTS, MAX_HYDRATION_ENTRIES,
    MAX_HYDRATION_TOOL_NAME_CHARS, MAX_HYDRATION_TOOL_NAMES, MAX_ISSUER_CHARS, MAX_RECENT_DIGESTS,
    McpElicitationAction, Op, OpEnvelope, SCHEMA_VERSION, TurnOutcome, TurnUsage, UsageSource,
    is_canonical_digest, validate_elicitation, validate_hydration_batch,
    validate_hydration_carry_entry, validate_hydration_entry, validate_oauth_grant,
};
pub use reader::{JournalReport, read_journal};
pub use redaction::{RedactionError, SecretKind, scan_for_secrets};
pub use replay::{CompactionPhase, GoalLive, McpOauthGrantState, ReplayError, SessionState};
pub use writer::JournalWriter;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "p3_tests.rs"]
mod p3_tests;

#[cfg(test)]
#[path = "fork_tests.rs"]
mod fork_tests;
