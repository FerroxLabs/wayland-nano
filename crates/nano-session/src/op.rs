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
        /// Typed error classification (C7): the KIND only, never raw error
        /// text — the digest-only journal invariant holds, and the
        /// presentation string is re-derivable from the error-code table on
        /// replay. Serde-defaulted: journals written before the field
        /// existed replay unchanged; omitted from serialization when absent
        /// so new journals stay byte-minimal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_kind: Option<crate::error_kind::NanoErrorKind>,
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
    /// An ACCEPTED todo-list replacement (C10 §2), journaled by the `todo`
    /// tool under journal-first, accepted-only ordering: the item set
    /// validates first, this op lands durably, and only then does the
    /// session's todo cell mutate. CONTENT, not posture: replay folds it
    /// into session state (last-write-wins) so a resumed session restores
    /// the list. Payload policy is the AssistantText class — model-authored,
    /// user-visible, journaled verbatim (a model that writes a secret into a
    /// todo item persists it in plaintext, the same risk class as an
    /// assistant message; documented, user-visible, not a new channel).
    TodoSet {
        items: Vec<TodoItem>,
    },
    /// An ACCEPTED plan-posture transition (C10 §3), journaled by the single
    /// `set_plan_posture` transition every entry/exit path converges on.
    /// Audit history ONLY — the pack-wide rule is "content replays, postures
    /// don't" (C2 Q5 precedent): replay IGNORES this op for activation and
    /// session/load NEVER restores the posture; a resumed session starts
    /// with plan mode off. Older builds read this as `Unknown` and skip.
    PlanSet {
        active: bool,
    },
    /// An ACCEPTED permission-mode change (C2), journaled by the
    /// session/set_mode handler under journal-first, accepted-only ordering:
    /// the id validates first, this op lands durably, and only then does the
    /// session's mode cell mutate. Audit history ONLY — replay treats it as
    /// context-neutral and session/load NEVER restores the mode: every
    /// session starts in `default` and elevated autonomy always requires a
    /// fresh, explicit grant. Older builds read this as `Unknown` and skip.
    ModeSet {
        /// The PermissionMode wire id ("read_only" | "default" | "full_auto").
        mode: String,
    },
    /// A drained mid-turn steer (C9): journaled DURABLY at drain time,
    /// BEFORE the in-memory history mutation, so the journal records what
    /// the model actually saw, in order. Enqueued-but-undrained steers are
    /// never journaled. User text is journaled verbatim (same rule as
    /// `TurnBegin.input`); replay folds it as a user message exactly like
    /// the TurnBegin input fold, so kill-resume reconstructs steer-adjusted
    /// context byte-identically.
    SteerInput {
        turn_id: String,
        text: String,
    },
    /// The one allowed structured-output re-ask (C9 §4.3): the LITERAL
    /// feedback text the model saw, journaled at the moment the feedback
    /// message enters history (journal-first, fail-closed on append
    /// failure). Replay folds it as a user message, so kill-resume
    /// reconstructs the re-asked context byte-identically regardless of
    /// template wording changes across versions. A re-ask is a new
    /// journaled sampling step with its own budget accounting, NOT a retry.
    SchemaReask {
        turn_id: String,
        feedback: String,
    },
    /// Forward tolerance: any Op type this build does not know. Skipped on
    /// replay; the raw line stays in the journal for future readers.
    #[serde(other)]
    Unknown,
}

/// One todo-list entry (C10 §2). The status vocabulary adopts the
/// wcore/codex set (`pending`/`in_progress`/`completed`/`cancelled`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
    /// A status written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

impl TodoStatus {
    /// The wire/model-facing id.
    pub fn id(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
            TodoStatus::Unknown => "unknown",
        }
    }

    /// Parse a model-supplied status string. Unknown strings are `None` —
    /// the todo tool turns that into a typed validation error (fail-closed),
    /// never a silent coercion.
    pub fn parse(id: &str) -> Option<TodoStatus> {
        match id {
            "pending" => Some(TodoStatus::Pending),
            "in_progress" => Some(TodoStatus::InProgress),
            "completed" => Some(TodoStatus::Completed),
            "cancelled" => Some(TodoStatus::Cancelled),
            _ => None,
        }
    }

    /// Counts toward the open work the status line reports.
    pub fn is_open(self) -> bool {
        matches!(self, TodoStatus::Pending | TodoStatus::InProgress)
    }
}
