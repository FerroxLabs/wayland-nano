//! Op envelope and Op vocabulary.
//!
//! Design notes:
//! - `#[serde(other)] Unknown` gives forward tolerance: a journal written by a
//!   newer Nano (with Ops this build does not know) still loads; unknown Ops
//!   are preserved in the raw file but skipped during replay.
//! - `id` gives idempotence: replay dedupes repeated ids, so a retried append
//!   after a crash-uncertain write cannot double-apply effects.

use serde::Deserialize;
use serde::Serialize;

/// Current journal schema version. Bump only for breaking envelope changes;
/// additive Op variants ride the same version (unknown-skip covers old readers).
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpEnvelope {
    pub v: u32,
    pub id: String,
    pub ts: String,
    pub op: Op,
}

impl OpEnvelope {
    pub fn new(id: impl Into<String>, ts: impl Into<String>, op: Op) -> Self {
        Self {
            v: SCHEMA_VERSION,
            id: id.into(),
            ts: ts.into(),
            op,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed,
    /// Written only by replay/restore, never by a live writer: marks a turn
    /// that was in progress when the journal tail was cut.
    Interrupted,
}

/// Bounded, non-sensitive reasons a compaction attempt is abandoned (C1).
/// Free-text reasons are deliberately impossible: the text that tripped the
/// gate is exactly what must never reach the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionCancelReason {
    /// Journaled by a build that predates the reason field.
    #[default]
    Unspecified,
    /// The summary tripped the pre-persistence secret scan.
    RedactionHit,
    /// The secret scanner itself errored (fail-closed, nothing persisted).
    RedactorError,
    /// The summarization model call failed.
    ModelFailed,
    /// The compaction call overflowed even after pair-preserving escalation.
    OverflowEscalationExhausted,
    /// The journal append/flush failed, so the in-memory swap never happened.
    JournalWriteFailed,
    /// A reason written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    SessionBegin {
        session_id: String,
        cwd: String,
    },
    TurnBegin {
        turn_id: String,
        input: String,
    },
    ToolCall {
        turn_id: String,
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        /// Digest of the output, not the output itself — journals never carry
        /// secret payloads by default.
        output_digest: String,
        changed_files: Vec<String>,
    },
    /// Assistant-visible reply text for a model step. Unlike tool output this
    /// is content the agent itself produced for the user (it is streamed to
    /// the client live anyway), so journaling it carries no payload the user
    /// has not already seen; it is what lets a restored session rebuild the
    /// assistant side of the conversation.
    AssistantText {
        turn_id: String,
        text: String,
    },
    TurnEnd {
        turn_id: String,
        outcome: TurnOutcome,
    },
    CompactionBegin {
        compaction_id: String,
    },
    CompactionComplete {
        compaction_id: String,
        summary: String,
        /// Op ids this summary replaces for replay purposes.
        covers_op_ids: Vec<String>,
        /// Durable-effect inventory at compaction time. The summary replaces
        /// the *transcript*; effects must survive or replay diverges.
        changed_files: Vec<String>,
    },
    CompactionCancel {
        compaction_id: String,
        /// Why the compaction was abandoned. Bounded enum, never free text:
        /// a cancel reason must never become a secondary persistence channel
        /// for the sensitive content that just failed the redaction gate.
        /// Defaults for journals written before the field existed.
        #[serde(default)]
        reason: CompactionCancelReason,
    },
    /// Forward tolerance: any Op type this build does not know. Skipped on
    /// replay; the raw line stays in the journal for future readers.
    #[serde(other)]
    Unknown,
}
