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

/// Goal lifecycle states (C11, kimi-minimal — Q7 RULED). `complete` is the
/// only success terminal; a budget trip is `blocked` with a machine-readable
/// reason, never a separate status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatusKind {
    Active,
    Paused,
    Blocked,
    Complete,
    /// A status written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Bounded, machine-readable reasons for a goal transition (C11). Same rule
/// as [`CompactionCancelReason`]: never free text — a reason field must never
/// become a secondary persistence channel for content that failed a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalReason {
    /// No reason applies (e.g. activation, completion).
    #[default]
    Unspecified,
    BudgetTokens,
    BudgetTurns,
    BudgetWallclock,
    ModelDeclared,
    Error,
    Cancelled,
    /// A reason written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Terminal goal outcomes journaled by `GoalEnd` (C11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalOutcome {
    Complete,
    Blocked,
    /// An outcome written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

// ── P1 economy (panel-certified design: P1-search-economy-design.md) ─────
// JOURNAL-MIGRATION REVIEW FLAG (RC2 coordinated review): the THREE P1
// journal additions — (a) `usage` on `Op::TurnEnd`, (b) `Op::BudgetGrant`,
// (c) `Op::ChildUsageRollup` — ride the coordinated RC2 journal-migration
// review together (design §3.3/§6): no piecemeal schema erosion. All three
// are numbers/bounded-enums-only payloads (digest-only invariant untouched),
// all serde-defaulted so pre-P1 journals replay unchanged, and old readers
// skip them via the `Unknown`-op forward tolerance.

/// Where a usage figure came from (P1 §3.5 provenance): provider-reported
/// wire usage vs the fail-closed conservative estimate charged when the wire
/// reports nothing. Bounded enum, `#[serde(other)]`-tolerant like
/// [`GoalReason`] — never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The provider reported these tokens on the wire.
    #[default]
    ProviderReported,
    /// The wire reported nothing; the meter charged the §3.5 conservative
    /// estimate (never zero — an under-reporting provider must not become a
    /// budget bypass).
    Estimated,
    /// A source written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Stable id of the §3.5 conservative-estimation formula (Q4 RULED): bump
/// when the formula changes so old journals stay re-derivable under the
/// rules that produced them.
pub const ESTIMATION_METHOD_VERSION: u32 = 1;

/// The turn-scoped usage record (P1 §3.4–3.5): the SUM across EVERY
/// `record_usage` call in the turn — explicitly NOT the last response's
/// usage — plus cost and estimation provenance. NUMBERS and bounded enums
/// only, no content: the digest-only journal invariant is untouched. All
/// fields serde-defaulted; the whole record is optional on `TurnEnd` and
/// omitted when a turn recorded no usage at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// Meter-computed cost in integer microcents (the budget authority;
    /// provider-reported `cost_usd` is observability only and is NEVER
    /// journaled here). Meaningful only when `priced` is true.
    #[serde(default)]
    pub microcents: u64,
    /// Cost-truth honesty flag (P1 §3.1): true = every contributing record
    /// had a real metered (or known-free) price; false = unpriceable, NEVER
    /// render as $0. Defaults true for an empty sum (nothing unpriced yet).
    #[serde(default = "default_priced")]
    pub priced: bool,
    /// Provenance (P1 §3.5): `estimated` when ANY contributing record took
    /// the conservative missing-usage charge.
    #[serde(default)]
    pub usage_source: UsageSource,
    /// The formula id that produced estimated charges ([`ESTIMATION_METHOD_VERSION`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimation_method_version: Option<u32>,
    /// Sum of the request-side input estimates used by estimated charges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_estimate_input: Option<u64>,
    /// Sum of the request-side output estimates (the reserved allowance)
    /// used by estimated charges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_estimate_output: Option<u64>,
    /// Sum of the conservative charges actually applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_estimate: Option<u64>,
}

fn default_priced() -> bool {
    true
}

impl Default for TurnUsage {
    /// An empty sum is "nothing unpriced yet" (`priced: true`); the first
    /// unpriceable record flips it false permanently.
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            reasoning_tokens: 0,
            microcents: 0,
            priced: true,
            usage_source: UsageSource::default(),
            estimation_method_version: None,
            request_estimate_input: None,
            request_estimate_output: None,
            applied_estimate: None,
        }
    }
}

impl TurnUsage {
    /// Total tokens counted against the session cap (input + output, the C11
    /// goal-budget accounting rule; cached input is part of the provider's
    /// reported input total, never double-counted).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// True when nothing was recorded (the `TurnEnd.usage` field stays
    /// omitted — new journals stay byte-minimal).
    pub fn is_zero(&self) -> bool {
        self.total_tokens() == 0
            && self.cached_input_tokens == 0
            && self.reasoning_tokens == 0
            && self.applied_estimate.unwrap_or(0) == 0
    }

    /// Fold one provider-reported response's usage into the sum.
    #[allow(clippy::too_many_arguments)]
    pub fn add_provider_reported(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: u64,
        microcents: u64,
        priced: bool,
    ) {
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.cached_input_tokens = self.cached_input_tokens.saturating_add(cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(reasoning_tokens);
        self.microcents = self.microcents.saturating_add(microcents);
        self.priced &= priced;
    }

    /// Fold one §3.5 conservative (missing-usage) charge into the sum, WITH
    /// auditable provenance.
    #[allow(clippy::too_many_arguments)]
    pub fn add_estimated(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        microcents: u64,
        priced: bool,
        method_version: u32,
        applied_estimate: u64,
    ) {
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.microcents = self.microcents.saturating_add(microcents);
        self.priced &= priced;
        self.usage_source = UsageSource::Estimated;
        self.estimation_method_version = Some(method_version);
        self.request_estimate_input = Some(
            self.request_estimate_input
                .unwrap_or(0)
                .saturating_add(input_tokens),
        );
        self.request_estimate_output = Some(
            self.request_estimate_output
                .unwrap_or(0)
                .saturating_add(output_tokens),
        );
        self.applied_estimate = Some(
            self.applied_estimate
                .unwrap_or(0)
                .saturating_add(applied_estimate),
        );
    }

    /// Fold another recorded sum into this one (replay reconstruction,
    /// orphan-child rollups). Provenance merges conservatively: any
    /// estimated contribution marks the result estimated.
    pub fn add_sum(&mut self, other: &TurnUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.microcents = self.microcents.saturating_add(other.microcents);
        self.priced &= other.priced;
        if other.usage_source == UsageSource::Estimated {
            self.usage_source = UsageSource::Estimated;
        }
        if other.estimation_method_version.is_some() {
            self.estimation_method_version = other.estimation_method_version;
        }
        let merge = |a: &mut Option<u64>, b: Option<u64>| {
            if let Some(b) = b {
                *a = Some(a.unwrap_or(0).saturating_add(b));
            }
        };
        merge(
            &mut self.request_estimate_input,
            other.request_estimate_input,
        );
        merge(
            &mut self.request_estimate_output,
            other.request_estimate_output,
        );
        merge(&mut self.applied_estimate, other.applied_estimate);
    }
}

/// Goal budget limits (C11, kimi `GoalBudgetLimits` shape): all optional,
/// any combination. Enforcement lives in the goal driver (nano-agent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBudgets {
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub turn_budget: Option<u64>,
    #[serde(default)]
    pub wall_clock_budget_ms: Option<u64>,
}

/// Cap on a goal objective (kimi `MAX_GOAL_OBJECTIVE_LENGTH`).
pub const MAX_GOAL_OBJECTIVE_LEN: usize = 4000;
/// Cap on a `goal_complete` summary, schema-validated like any other argument.
pub const MAX_GOAL_SUMMARY_LEN: usize = 2000;

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
        /// P1 §3.4 — JOURNAL-MIGRATION REVIEW FLAG (addition (a) of the RC2
        /// coordinated review trio). The turn-scoped usage SUM across every
        /// `record_usage` call in the turn (explicitly NOT the last
        /// response's usage), with §3.5 estimation provenance — numbers and
        /// bounded enums only, so the digest-only invariant is untouched.
        /// Journaled for EVERY terminal outcome (Completed/Cancelled/Failed):
        /// a cancelled stream that consumed tokens journals those tokens.
        /// Serde-defaulted so pre-P1 journals replay unchanged (the
        /// `ToolResult.error_kind` pattern); omitted when the turn recorded
        /// no usage, keeping new journals byte-minimal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TurnUsage>,
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
    /// Fork lineage marker (C11): the second envelope of every forked child
    /// journal, right after the child genesis `SessionBegin`. It opens the
    /// imported region — the next `imported_ops` envelopes are the parent's
    /// journal through the fork point, copied byte-verbatim. Replay folds the
    /// imported region BY CLASS: context/turn-structure ops fold normally,
    /// control ops (goal/cron) are suppressed from live state into an
    /// audit-only namespace. A declared `imported_ops` count that overruns
    /// the actual stream is a typed replay error (fail-closed).
    ForkedFrom {
        parent_session_id: String,
        /// Id of the last imported parent envelope (the fork point).
        parent_op_id: String,
        /// The turn the fork was taken at (`None` = fork at end).
        at_turn: Option<String>,
        /// SHA-256 (hex) of the parent journal before and after the copy —
        /// the byte-identical-parent proof, asserted equal by the fork.
        parent_digest_before: String,
        parent_digest_after: String,
        /// Number of envelopes in the imported region that follows.
        imported_ops: u64,
    },
    /// Goal activation (C11). One current goal per session; the writer side
    /// rejects a second non-terminal goal and objectives over
    /// [`MAX_GOAL_OBJECTIVE_LEN`].
    GoalBegin {
        goal_id: String,
        objective: String,
        budgets: GoalBudgets,
    },
    /// Every goal transition (C11). Reasons are the bounded [`GoalReason`]
    /// enum, never free text.
    GoalStatus {
        goal_id: String,
        status: GoalStatusKind,
        #[serde(default)]
        reason: GoalReason,
    },
    /// Goal terminal record (C11): `complete`, or terminal `blocked`.
    GoalEnd {
        goal_id: String,
        outcome: GoalOutcome,
    },
    /// Cron fire reservation (C11 §5.4): journaled and flushed BEFORE any
    /// prompt injection or model call — the durable act of firing. Keyed by
    /// `occurrence_id` (`{job_id}:{scheduled_fire_time}` RFC3339 UTC, minute
    /// resolution), stable across restarts/jitter/coalescing, so a durable
    /// reservation can never double-fire. `mode_at_fire` records the capped
    /// `min(session_mode, default)` derivation for after-the-fact audit.
    CronFired {
        job_id: String,
        session_id: String,
        turn_id: String,
        occurrence_id: String,
        /// Permission-mode wire id the fired turn ran under.
        mode_at_fire: String,
        /// Number of missed occurrences this fire coalesces (0 = on time).
        #[serde(default)]
        coalesced: u32,
    },
    /// An ACCEPTED `/budget continue` grant (P1 §4.3) — JOURNAL-MIGRATION
    /// REVIEW FLAG (addition (b) of the RC2 coordinated review trio).
    /// Journaled DURABLY at acceptance under journal-first, accepted-only
    /// ordering (the C10 `TodoSet` pattern): the command validates, this op
    /// lands, and only then does the session's effective-limit cell mutate;
    /// an append failure leaves the limit unchanged. Numbers and ids only.
    /// Replay folds it into budget state, so a kill-resumed session
    /// reconstructs the exact effective limit (replay-deterministic, never
    /// session-volatile).
    BudgetGrant {
        grant_id: String,
        tokens: u64,
        after_limit: u64,
    },
    /// A C6 child task's usage folded into the PARENT journal at the child's
    /// terminal state (P1 §3.3) — JOURNAL-MIGRATION REVIEW FLAG (addition
    /// (c) of the RC2 coordinated review trio). Journal-first at the
    /// reconciliation boundary: this op lands DURABLY BEFORE the child's
    /// terminal result becomes visible to the parent session
    /// (`task_result`/`task_apply`); append failure fails closed (the
    /// completion is held, never reported while its usage is unjournaled).
    /// Numbers and stable ids only. Replay folds it into meter totals keyed
    /// by `task_id`; envelope-id dedup makes a retried append idempotent,
    /// and the op's presence is the orphan-fold dedup marker at resume.
    ChildUsageRollup {
        task_id: String,
        /// The child's single turn id (`{task_id}-turn-1`, tasks.rs).
        child_turn_id: String,
        outcome: TurnOutcome,
        /// The child's turn-scoped usage sum (§3.4) with §3.5 provenance.
        usage: TurnUsage,
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
