//! Turn engine: the agent state machine over a model driver and tool calls,
//! with journal recording and loop protection wired in.
//!
//! States (every one testable): RECEIVE → UNDERSTAND → PLAN → ACT → OBSERVE
//! → CONTINUE/REPLAN → VERIFY → COMPLETE. No cognitive theatre.

use crate::compact::{AutoCompactGuard, CompactionConfig, TokenTracker, compact_messages};
use crate::loop_protection::{
    BudgetTracker, NoProgressTracker, ProgressAction, ProgressSignals, RepeatAction, RepeatBreaker,
    ToolCallKey, TurnBudget,
};
use crate::steer::SteerHandle;
use nano_model::auth::RefreshOutcome;
use nano_model::types::{
    CallHooks, Message, ModelError, ModelEvent, ModelObservation, ModelRequest, ModelResponse,
    ToolCall,
};
use nano_session::NanoErrorKind;
use nano_session::op::{Op, OpEnvelope, TurnUsage};
use std::fmt::Debug;

/// The default output cap on a model request (P1 §4.2: the reservation
/// clamp replaces this hardcoded value at the single build site below when
/// the session meter is wired).
const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4096;

/// P1 §3.4 / r3 codex-F1: drain externally-attributed usage (the search
/// lane's grounding round trips feed the OWNING turn's accumulator through
/// this cell BEFORE terminal journaling) into the turn-scoped sum, so live
/// meter == journaled sum == replay reconstruction, searches included.
fn drain_extra_usage(
    extra: &Option<std::sync::Arc<std::sync::Mutex<TurnUsage>>>,
    turn_usage: &mut TurnUsage,
    recorded: &mut bool,
) {
    let Some(cell) = extra else { return };
    let drained = std::mem::take(&mut *cell.lock().unwrap_or_else(|p| p.into_inner()));
    if !drained.is_zero() {
        turn_usage.add_sum(&drained);
        *recorded = true;
    }
}

/// P1 §3.4: build the `TurnEnd` op carrying the turn-scoped usage SUM
/// across every `record_usage` call in the turn (explicitly NOT the last
/// response's usage) — partial usage for EVERY terminal outcome, omitted
/// when nothing was recorded so new journals stay byte-minimal.
fn turn_end_op(
    turn_id: &str,
    outcome: nano_session::op::TurnOutcome,
    turn_usage: &mut TurnUsage,
    recorded: &mut bool,
    extra: &Option<std::sync::Arc<std::sync::Mutex<TurnUsage>>>,
) -> Op {
    drain_extra_usage(extra, turn_usage, recorded);
    Op::TurnEnd {
        turn_id: turn_id.into(),
        outcome,
        usage: if *recorded {
            Some(turn_usage.clone())
        } else {
            None
        },
    }
}

/// The model boundary the engine drives. FluxCompletionsClient implements
/// this in production; tests script a mock.
#[async_trait::async_trait]
pub trait ModelDriver: Debug + Send + Sync {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;

    /// The hooked variant (C9): cancel-selectable reconnect sleeps and the
    /// typed observation channel. Drivers that cannot consume hooks fall
    /// back to the plain call — hooks are then inert, never misrouted.
    async fn complete_observed(
        &self,
        request: &ModelRequest,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let _ = hooks;
        self.complete(request).await
    }
}

/// One tool invocation the engine can perform (fs/shell/etc. register here).
/// Async (C3/C4 Q1, Option A — mirrors the ModelDriver precedent above):
/// web_fetch awaits the egress pipeline; fs/shell arms stay synchronous
/// internally. No ambient-runtime bridge.
#[async_trait::async_trait]
pub trait ToolExecutor: Debug + Send + Sync {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome;

    /// The cancel-aware variant (P1 §2.2, r2 codex-F3 — the ModelDriver
    /// `complete_observed` precedent): executors with cancel-selectable
    /// in-flight I/O (web_search's grounding send/body-read) override this
    /// to honor the flag mid-call; every other executor falls back to the
    /// plain call — the flag is then checked at the turn loop's boundaries
    /// exactly as before, never misrouted.
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> ToolOutcome {
        let _ = cancel;
        self.execute(call).await
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub ok: bool,
    pub output: String,
    pub progress: ProgressSignals,
    /// C7: the typed error classification, set at the same site that
    /// stringifies the error (the variant is still in scope there — no
    /// information is lost before the handoff). `None` on success and for
    /// executors that predate typing.
    pub error_kind: Option<NanoErrorKind>,
}

/// A turn-fatal failure with its typed classification (C7 design §2/D6).
/// `detail` is logs/model-side ONLY — it may carry provider text and never
/// reaches a UI-bound frame (design §7). The wire carries `kind` plus the
/// closed typed extras.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedError {
    pub kind: NanoErrorKind,
    pub detail: String,
    /// Provider HTTP status (bounded code) when the failure carries one.
    pub status: Option<u16>,
    pub retry_after_ms: Option<u64>,
    /// Egress-redacted host (redacted by construction in nano-egress).
    pub host: Option<String>,
}

impl TypedError {
    pub fn new(kind: NanoErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            status: None,
            retry_after_ms: None,
            host: None,
        }
    }
}

/// An engine stop with its typed classification (C7 design §2/D6): kinds
/// `user_cancelled` / `budget_exhausted` / `no_progress` /
/// `repeat_force_stop`, assigned at the construction site instead of
/// parsing a payload string downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopInfo {
    pub kind: NanoErrorKind,
    /// Logs/model-side only (same rule as [`TypedError::detail`]).
    pub detail: String,
}

impl StopInfo {
    pub fn new(kind: NanoErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnState {
    Receive,
    Understand,
    Plan,
    Act,
    Observe,
    Replan,
    Verify,
    Complete,
    Failed(TypedError),
    Stopped(StopInfo),
}

impl TurnState {
    pub fn label(&self) -> String {
        match self {
            TurnState::Receive => "RECEIVE",
            TurnState::Understand => "UNDERSTAND",
            TurnState::Plan => "PLAN",
            TurnState::Act => "ACT",
            TurnState::Observe => "OBSERVE",
            TurnState::Replan => "REPLAN",
            TurnState::Verify => "VERIFY",
            TurnState::Complete => "COMPLETE",
            TurnState::Failed(_) => "FAILED",
            TurnState::Stopped(_) => "STOPPED",
        }
        .to_string()
    }
}

#[derive(Debug)]
pub struct TurnResult {
    pub state: TurnState,
    /// Ordered state transitions (the plan: every state is testable).
    pub history: Vec<TurnState>,
    pub steps: u32,
    pub tool_calls: u32,
    pub final_text: String,
    pub ops: Vec<OpEnvelope>,
    /// Server-reported usage of the LAST model response in the turn (C11:
    /// feeds the goal-level token accumulator and exec's turn_completed
    /// event). Zero when no model call succeeded.
    pub usage: nano_model::types::Usage,
    /// P1 §3.4: the turn-scoped usage SUM across EVERY `record_usage` call
    /// in the turn (explicitly NOT `usage` above, which is last-response
    /// only) — the payload journaled on `TurnEnd` and rolled up by C6
    /// parents. `None` when nothing was recorded.
    pub turn_usage: Option<TurnUsage>,
}

pub struct TurnEngine<'a> {
    pub model: &'a dyn ModelDriver,
    pub tools: &'a dyn ToolExecutor,
    pub budget: TurnBudget,
    pub model_name: String,
    /// Tool definitions advertised to the model (its tool-call surface).
    pub tool_definitions: Vec<nano_model::types::ToolDefinition>,
    /// Approval gate consulted before every side-effecting tool execution.
    pub approval: Option<&'a dyn ApprovalGate>,
    /// Context-management settings (C1): window + 90% auto-compact limit for
    /// the active model. `None` disables auto-compaction (unit tests); the
    /// production host always resolves a config.
    pub compaction: Option<CompactionConfig>,
    /// The C9 robustness seams (steer queue, 401 refresh, observations,
    /// model params). `Default` = every seam off (pre-C9 behavior).
    pub robustness: TurnRobustness<'a>,
}

/// The C9 robustness seams, bundled so hosts opt in wholesale. All-off is
/// exactly the pre-C9 engine.
#[derive(Default)]
pub struct TurnRobustness<'a> {
    /// Mid-turn steer queue (§3): drained at loop top only, journal-first,
    /// closed by the engine exactly once at turn end.
    pub steer: Option<SteerHandle>,
    /// The 401 recovery seam (§2.4, Q5): the credential provider's refresh
    /// trait; the one-shot state machine lives here in the turn. `None` =
    /// static-key behavior: a 401 takes zero retries.
    pub auth_refresh: Option<&'a dyn nano_model::auth::AuthRefresh>,
    /// The host's typed observation sink (reconnect banners, inert-param
    /// notices, rate-limit snapshots).
    pub observer: Option<&'a (dyn Fn(ModelObservation) + Send + Sync)>,
    /// Cross-surface model params (§4): run through the Q3 capability
    /// ladder inside the surface adapter.
    pub reasoning_effort: Option<nano_model::types::ReasoningEffort>,
    pub verbosity: Option<nano_model::types::Verbosity>,
    pub output_schema: Option<serde_json::Value>,
    /// P1 §3.2/§4.2: the session cost meter handle (Arc-shared, the same
    /// ownership pattern as the steer queue). When present, the meter
    /// records EVERY response's usage and the engine takes an ATOMIC output
    /// reservation before every request (the clamp at the single
    /// `ModelRequest` build site); `None` = the pre-P1 posture (no metering,
    /// no reservation, no clamp) — tests and C6's pre-C9 posture keep
    /// compiling unchanged.
    pub meter: Option<crate::cost::CostMeter>,
    /// P1 r3 codex-F1: externally-attributed usage (the search lane's Flux
    /// grounding round trips) routed into THIS turn's accumulator before
    /// terminal journaling, so live meter == journaled sum == replay.
    /// `None` = no external attribution.
    pub extra_usage: Option<std::sync::Arc<std::sync::Mutex<TurnUsage>>>,
}

/// Decides whether a tool call may execute. Production prompts via the host;
/// tests and headless flows use policy-driven implementations.
pub trait ApprovalGate: Debug + Send + Sync {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision;
    /// Why the gate is currently denying, when a categorical rule (C2
    /// permission modes) rather than a host decision produced the denial —
    /// e.g. `"session is in read_only mode"`. The engine appends it to the
    /// denial tool-result so the model learns WHY and stops retrying
    /// variants instead of looping. Default: no reason (the plain
    /// "denied by approval gate" text stands).
    fn denial_reason(&self) -> Option<&'static str> {
        None
    }
    /// Structured mid-turn question channel (C10 §5): `ask_user` calls and
    /// the plan-exit approval round-trip route here — the ONE question
    /// channel, reused, never parallel machinery. The argument is the raw
    /// tool call; the implementation mints wire option ids from
    /// `arguments.options[].label` and resolves the response back to the
    /// selected LABEL. Default: `Unavailable` — hosts that cannot answer
    /// (headless, or a gate that never learned questions) map to the typed
    /// "questions unavailable in this host" tool error, never a block.
    fn ask(&self, _call: &ToolCall) -> AskOutcome {
        AskOutcome::Unavailable
    }
}

/// The live-wire diff sink type (C10 §6): called with (tool call id, diff)
/// when a fs_write/fs_edit succeeds. Live-wire-only, never journaled.
pub type DiffHook = std::sync::Arc<dyn Fn(&str, &FileDiff) + Send + Sync>;

/// A structured before/after text pair for one file mutation (C10 §6): the
/// ONE diff representation end-to-end (no unified-diff string anywhere).
/// Emitted on the live wire as an ACP `diff` content block for human review;
/// NEVER journaled (diffs can carry secret-bearing file content and the
/// journal is digest-only) and NEVER fed back to the model (the model-facing
/// outcome string stays terse). `old_text: None` = whole-file add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: std::path::PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

impl FileDiff {
    /// Per-side cap (C10 §6): 32k chars, deterministic head+tail truncation
    /// with an explicit elision marker, so a huge write cannot flood the
    /// wire frame.
    pub const MAX_SIDE_CHARS: usize = 32 * 1024;

    /// Builds a diff with each side capped to [`Self::MAX_SIDE_CHARS`].
    pub fn capped(path: std::path::PathBuf, old_text: Option<String>, new_text: String) -> Self {
        Self {
            path,
            old_text: old_text.map(|t| cap_diff_side(&t)),
            new_text: cap_diff_side(&new_text),
        }
    }
}

/// Head+tail truncation: over the cap, keep the first and last halves with a
/// deterministic elision marker between. Char-based, so the cut never splits
/// a UTF-8 sequence (C1's truncation rule).
fn cap_diff_side(text: &str) -> String {
    let total = text.chars().count();
    if total <= FileDiff::MAX_SIDE_CHARS {
        return text.to_string();
    }
    let half = FileDiff::MAX_SIDE_CHARS / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = {
        let skip = total - half;
        text.chars().skip(skip).collect()
    };
    format!("{head}\n…[elided {} chars]…\n{tail}", total - 2 * half)
}

/// Outcome of a structured mid-turn question (C10 §5). Every exit is
/// fail-closed to a typed tool error except an explicit answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskOutcome {
    /// The user picked an option; carries its LABEL (the wire only carried
    /// the minted id; the gate resolved it through its id→label map).
    Answered(String),
    /// Dismissed, cancelled, timed out, disconnected, or a malformed
    /// response — the string is the bounded typed reason.
    Denied(String),
    /// This host has no question channel at all (call-time failure).
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

#[derive(Debug)]
pub struct ApproveAll;
impl ApprovalGate for ApproveAll {
    fn approve(&self, _call: &ToolCall) -> ApprovalDecision {
        ApprovalDecision::Approve
    }
}

impl<'a> TurnEngine<'a> {
    pub async fn run_turn(&self, turn_id: &str, input: &str) -> TurnResult {
        self.run_turn_cancellable(turn_id, input, None).await
    }

    /// Runs a turn with a prepended context message (e.g. skill activation).
    pub async fn run_turn_with_context(
        &self,
        turn_id: &str,
        input: &str,
        context: Option<Message>,
    ) -> TurnResult {
        self.run_turn_inner(turn_id, input, context.into_iter().collect(), None, None)
            .await
    }

    /// Runs a turn with SEVERAL prepended context messages (skill
    /// activation, AGENTS.md, restored session blocks — C10; the C5 memory
    /// block), in order, before the user input.
    pub async fn run_turn_with_context_messages(
        &self,
        turn_id: &str,
        input: &str,
        context: Vec<Message>,
    ) -> TurnResult {
        self.run_turn_inner(turn_id, input, context, None, None)
            .await
    }

    /// Runs a turn, checking the cancellation flag between steps. A fired
    /// flag stops the turn at the next boundary with a typed reason — never
    /// mid-tool-execution (side effects already applied stay applied and are
    /// journaled).
    pub async fn run_turn_cancellable(
        &self,
        turn_id: &str,
        input: &str,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> TurnResult {
        self.run_turn_inner(turn_id, input, Vec::new(), cancel, None)
            .await
    }

    /// Runs a turn like [`Self::run_turn_cancellable`], additionally invoking
    /// `sink` with every op the moment it is recorded — before the turn
    /// completes. Streaming hosts (ACP) use this to forward frames live
    /// instead of replaying `result.ops` after the fact. The sink returns
    /// whether the op is DURABLY journaled: the compaction commit protocol
    /// (C1 §6) swaps in-memory history only behind a durable Complete.
    pub async fn run_turn_streaming(
        &self,
        turn_id: &str,
        input: &str,
        cancel: Option<&std::sync::atomic::AtomicBool>,
        sink: &mut (dyn FnMut(&OpEnvelope) -> bool + Send),
    ) -> TurnResult {
        self.run_turn_inner(turn_id, input, Vec::new(), cancel, Some(sink))
            .await
    }

    /// Runs a streaming turn whose model context starts with `prior` messages
    /// (e.g. a conversation rebuilt from the session journal on resume),
    /// followed by the new user input.
    pub async fn run_turn_streaming_with_context(
        &self,
        turn_id: &str,
        input: &str,
        prior: Vec<Message>,
        cancel: Option<&std::sync::atomic::AtomicBool>,
        sink: &mut (dyn FnMut(&OpEnvelope) -> bool + Send),
    ) -> TurnResult {
        self.run_turn_inner(turn_id, input, prior, cancel, Some(sink))
            .await
    }

    async fn run_turn_inner(
        &self,
        turn_id: &str,
        input: &str,
        context: Vec<Message>,
        cancel: Option<&std::sync::atomic::AtomicBool>,
        sink: Option<&mut (dyn FnMut(&OpEnvelope) -> bool + Send)>,
    ) -> TurnResult {
        let mut ops: Vec<OpEnvelope> = Vec::new();
        let mut next_id = 0u32;
        let mut sink = sink;
        // Emits one op into the turn record and through the sink. Returns
        // whether the op is durably recorded (no sink = in-memory only, e.g.
        // unit tests, which counts as recorded for protocol purposes).
        let mut emit = |ops: &mut Vec<OpEnvelope>, op: Op| -> bool {
            next_id += 1;
            ops.push(OpEnvelope::new(format!("{turn_id}-{next_id}"), "now", op));
            if let Some(sink) = sink.as_deref_mut() {
                sink(ops.last().expect("op just pushed"))
            } else {
                true
            }
        };

        emit(
            &mut ops,
            Op::TurnBegin {
                turn_id: turn_id.into(),
                input: input.into(),
            },
        );

        let mut history = vec![TurnState::Receive];
        let mut state = TurnState::Receive;
        let transition = |state: &mut TurnState, history: &mut Vec<TurnState>, next: TurnState| {
            *state = next;
            history.push(state.clone());
        };
        let mut protection = RepeatBreaker::default();
        let mut progress_tracker = NoProgressTracker::default();
        let mut budget_tracker = BudgetTracker::default();
        budget_tracker.start_turn();

        let mut messages = context;
        messages.push(Message::user(input));
        let mut final_text = String::new();
        let mut last_usage = nano_model::types::Usage::default();
        // P1 §3.4: the turn-scoped usage accumulator — sums EVERY response's
        // record beside `last_usage`; the sum (not the last response) is
        // what `TurnEnd.usage` serializes, so replay reconstruction equals
        // live accumulation for multi-step turns.
        let mut turn_usage = TurnUsage::default();
        let mut usage_recorded = false;
        // C1 context management: server-anchored token accounting, the
        // consecutive-ineffective-compaction guard, and the one-shot reactive
        // overflow retry.
        let mut tokens = TokenTracker::default();
        let mut compact_guard = AutoCompactGuard::default();
        let mut compaction_counter = 0u32;
        let mut reactive_compaction_used = false;
        // C9 per-turn one-shot states (§2.4 auth refresh, §4.3 schema
        // re-ask): both reset here, so each turn gets exactly one of each.
        let mut auth_retry_used = false;
        let mut reask_used = false;
        // C9 rate-limit coalescing: latest-wins per iteration. The Mutex
        // pair is written by the observation closure (an Fn) and flushed to
        // the host observer at loop top and turn end.
        let latest_rate_limit =
            std::sync::Mutex::new(None::<nano_model::rate_limits::RateLimitSnapshot>);
        let rate_limit_dirty = std::sync::Mutex::new(false);
        // Flush the coalesced rate-limit snapshot to the host observer.
        let flush_rate_limit = |dirty: &std::sync::Mutex<bool>,
                                latest: &std::sync::Mutex<
            Option<nano_model::rate_limits::RateLimitSnapshot>,
        >| {
            let was_dirty =
                std::mem::replace(&mut *dirty.lock().unwrap_or_else(|p| p.into_inner()), false);
            if was_dirty && let Some(host) = self.robustness.observer {
                let snapshot = latest.lock().unwrap_or_else(|p| p.into_inner()).clone();
                if let Some(snapshot) = snapshot {
                    host(ModelObservation::RateLimit(snapshot));
                }
            }
        };

        transition(&mut state, &mut history, TurnState::Understand);
        'turn: loop {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
                state = TurnState::Stopped(StopInfo::new(
                    NanoErrorKind::UserCancelled,
                    "cancelled by caller",
                ));
                emit(
                    &mut ops,
                    turn_end_op(
                        turn_id,
                        nano_session::op::TurnOutcome::Cancelled,
                        &mut turn_usage,
                        &mut usage_recorded,
                        &self.robustness.extra_usage,
                    ),
                );
                break;
            }
            if let Err(exhausted) = budget_tracker.check(&self.budget) {
                state = TurnState::Stopped(StopInfo::new(
                    NanoErrorKind::BudgetExhausted,
                    format!("budget exhausted: {exhausted:?}"),
                ));
                break;
            }
            budget_tracker.record_step();

            // ── C1 compaction seam: loop top only, pre-next-model-call ──
            // NEVER inside the tool-batch loop (a mid-batch swap would strand
            // tool_use blocks awaiting tool_results). No approval can be
            // pending here: approvals only exist inside a tool batch, and the
            // previous iteration's batch completed before this seam.
            if let Some(config) = self.compaction {
                let estimate = tokens.estimate(&messages);
                if estimate < config.auto_compact_limit {
                    // Any re-baseline below the trigger re-arms the guard.
                    compact_guard.reset();
                } else {
                    compaction_counter += 1;
                    let compaction_id = format!("{turn_id}-compact-{compaction_counter}");
                    let covers_op_ids = ops.iter().map(|e| e.id.clone()).collect();
                    let changed_files = collect_changed_files(&ops);
                    let mut journal_emit = |op: Op| emit(&mut ops, op);
                    let outcome = compact_messages(
                        self.model,
                        &self.model_name,
                        &mut messages,
                        &compaction_id,
                        covers_op_ids,
                        changed_files,
                        &mut journal_emit,
                    )
                    .await;
                    if let Err(err) = outcome {
                        state = TurnState::Failed(TypedError::new(
                            NanoErrorKind::CompactionFailed,
                            format!("auto-compaction failed: {err}"),
                        ));
                        emit(
                            &mut ops,
                            turn_end_op(
                                turn_id,
                                nano_session::op::TurnOutcome::Failed,
                                &mut turn_usage,
                                &mut usage_recorded,
                                &self.robustness.extra_usage,
                            ),
                        );
                        break;
                    }
                    // Post-compaction recompute: drop the stale server sample;
                    // the next real sample re-baselines.
                    tokens.reset();
                    if tokens.estimate(&messages) >= config.auto_compact_limit
                        && compact_guard.record_ineffective()
                    {
                        state = TurnState::Failed(TypedError::new(
                            NanoErrorKind::CompactionFailed,
                            "auto-compaction loop guard: two consecutive compactions failed to bring the context under the limit",
                        ));
                        emit(
                            &mut ops,
                            turn_end_op(
                                turn_id,
                                nano_session::op::TurnOutcome::Failed,
                                &mut turn_usage,
                                &mut usage_recorded,
                                &self.robustness.extra_usage,
                            ),
                        );
                        break;
                    }
                }
            }

            // ── C9 steer drain: loop top ONLY, after the cancel check and
            // the C1 compaction seam, before ModelRequest construction ──
            // The same adjacency-safe point C1 certified: no tool batch is
            // in flight, so a user message can never strand a tool_use.
            // Journal-first: Op::SteerInput lands DURABLY before the
            // in-memory history mutates; an append failure aborts the turn
            // fail-closed with history untouched. Undrained steers are
            // never journaled (replay can never resurrect input the model
            // never saw). Steers drained here BYPASS the proactive token
            // check for this iteration by design — C1's reactive-overflow
            // retry is the documented backstop.
            if let Some(handle) = &self.robustness.steer {
                for item in handle.drain_for(turn_id) {
                    if !emit(
                        &mut ops,
                        Op::SteerInput {
                            turn_id: turn_id.into(),
                            text: item.text.clone(),
                        },
                    ) {
                        state = TurnState::Failed(TypedError::new(
                            NanoErrorKind::JournalUnavailable,
                            "steer journal append failed (fail-closed: history unmutated)",
                        ));
                        emit(
                            &mut ops,
                            turn_end_op(
                                turn_id,
                                nano_session::op::TurnOutcome::Failed,
                                &mut turn_usage,
                                &mut usage_recorded,
                                &self.robustness.extra_usage,
                            ),
                        );
                        break 'turn;
                    }
                    // Multi-steer shape: separate user messages, one per
                    // steer, each submitter's intent verbatim. A surface
                    // whose provider rejects consecutive user roles
                    // coalesces at request-build time (per-surface,
                    // fixture-pinned) — never here, never mixed.
                    messages.push(Message::user(item.text));
                }
            }
            // Coalesced rate-limit snapshot (latest-wins per iteration).
            flush_rate_limit(&rate_limit_dirty, &latest_rate_limit);

            // ── P1 §4.2: ATOMIC output reservation + clamp — the ONE clamp
            // site (this single ModelRequest build serves parent AND child
            // engines). The reservation, not a bare allowance read, is what
            // makes the cap sound under C6 fan-out concurrency. A zero grant
            // is the §4.1 hard stop — typed, never a zero-token request.
            let mut reservation = match &self.robustness.meter {
                Some(meter) => {
                    let reservation = meter.reserve_output(u64::from(DEFAULT_MAX_OUTPUT_TOKENS));
                    if reservation.granted() == 0 {
                        let (limit, observed) = meter
                            .budget_state()
                            .map(|s| (s.limit, s.observed))
                            .unwrap_or((0, 0));
                        state = TurnState::Stopped(StopInfo::new(
                            NanoErrorKind::BudgetExceeded,
                            format!(
                                "budget_exceeded: limit={limit} observed={observed} reason=session token cap reached"
                            ),
                        ));
                        emit(
                            &mut ops,
                            turn_end_op(
                                turn_id,
                                nano_session::op::TurnOutcome::Failed,
                                &mut turn_usage,
                                &mut usage_recorded,
                                &self.robustness.extra_usage,
                            ),
                        );
                        break;
                    }
                    if reservation.granted() < reservation.requested() {
                        // The clamp is logged (typed notice), never silent.
                        if let Some(host) = self.robustness.observer {
                            host(ModelObservation::BudgetClamp {
                                requested: reservation.requested(),
                                granted: reservation.granted(),
                            });
                        }
                    }
                    Some(reservation)
                }
                None => None,
            };
            let request = ModelRequest {
                model: self.model_name.clone(),
                messages: messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: Some(
                    reservation
                        .as_ref()
                        .map(|r| r.granted().min(u64::from(u32::MAX)) as u32)
                        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
                ),
                stream: false,
                reasoning_effort: self.robustness.reasoning_effort,
                verbosity: self.robustness.verbosity,
                output_schema: self.robustness.output_schema.clone(),
                ..Default::default()
            };
            // C9 observation plumbing: rate-limit snapshots coalesce
            // latest-wins (flushed at the next loop top / turn end); every
            // other observation forwards to the host IMMEDIATELY — a
            // reconnect banner after the fact is useless.
            let observing = |observation: ModelObservation| match observation {
                ModelObservation::RateLimit(snapshot) => {
                    *latest_rate_limit.lock().unwrap_or_else(|p| p.into_inner()) = Some(snapshot);
                    *rate_limit_dirty.lock().unwrap_or_else(|p| p.into_inner()) = true;
                }
                other => {
                    if let Some(host) = self.robustness.observer {
                        host(other);
                    }
                }
            };
            let hooks = CallHooks {
                cancel,
                observer: Some(&observing),
            };
            let response = match self.model.complete_observed(&request, &hooks).await {
                // C9 §2.4 one-shot 401 seam (Q5 RULED): exactly one retry
                // of the same byte-identical request, only after a
                // successful refresh, only for HTTP 401. 403, non-HTTP auth
                // errors, a second 401, a refresh failure, and the
                // static-key (no provider) case are all terminal.
                Err(
                    err @ ModelError::Auth {
                        status: Some(401), ..
                    },
                ) if !auth_retry_used => {
                    auth_retry_used = true;
                    match self.robustness.auth_refresh {
                        Some(provider) => match provider.refresh().await {
                            RefreshOutcome::Refreshed => {
                                self.model.complete_observed(&request, &hooks).await
                            }
                            RefreshOutcome::NotRefreshable | RefreshOutcome::Failed(_) => Err(err),
                        },
                        None => Err(err),
                    }
                }
                other => other,
            };
            let response = match response {
                Ok(r) => r,
                // C9 §4.3 schema re-ask: ONE re-ask, a new journaled
                // sampling step (NOT a retry). Journal-first: the LITERAL
                // feedback text lands durable before history mutates; the
                // step budget is consumed at the next loop top. A second
                // schema failure falls through to the generic failure.
                Err(ModelError::OutputSchema(feedback))
                    if request.output_schema.is_some() && !reask_used =>
                {
                    reask_used = true;
                    // P1 §3.5: the failed sampling step's reservation
                    // settles conservatively before the re-ask reserves
                    // anew at the next loop iteration.
                    if let Some(reservation) = reservation.as_mut() {
                        let delta = reservation
                            .settle_conservative(&self.model_name, tokens.estimate(&messages));
                        turn_usage.add_sum(&delta);
                        usage_recorded = true;
                    }
                    if !emit(
                        &mut ops,
                        Op::SchemaReask {
                            turn_id: turn_id.into(),
                            feedback: feedback.clone(),
                        },
                    ) {
                        state = TurnState::Failed(TypedError::new(
                            NanoErrorKind::JournalUnavailable,
                            "schema re-ask journal append failed (fail-closed: history unmutated)",
                        ));
                        emit(
                            &mut ops,
                            turn_end_op(
                                turn_id,
                                nano_session::op::TurnOutcome::Failed,
                                &mut turn_usage,
                                &mut usage_recorded,
                                &self.robustness.extra_usage,
                            ),
                        );
                        break;
                    }
                    messages.push(Message::user(feedback));
                    continue; // the re-asked sampling step
                }
                // [R1] Reactive overflow: the tail heuristic undershot and the
                // model rejected the request — route into the SAME compaction
                // path exactly once instead of failing the turn. A second
                // overflow after that compaction falls through to the generic
                // failure below.
                Err(ModelError::ContextOverflow(_))
                    if self.compaction.is_some() && !reactive_compaction_used =>
                {
                    reactive_compaction_used = true;
                    // P1 §3.5: the rejected request settles conservatively;
                    // the post-compaction retry reserves anew.
                    if let Some(reservation) = reservation.as_mut() {
                        let delta = reservation
                            .settle_conservative(&self.model_name, tokens.estimate(&messages));
                        turn_usage.add_sum(&delta);
                        usage_recorded = true;
                    }
                    compaction_counter += 1;
                    let compaction_id = format!("{turn_id}-compact-{compaction_counter}");
                    let covers_op_ids = ops.iter().map(|e| e.id.clone()).collect();
                    let changed_files = collect_changed_files(&ops);
                    let mut journal_emit = |op: Op| emit(&mut ops, op);
                    let outcome = compact_messages(
                        self.model,
                        &self.model_name,
                        &mut messages,
                        &compaction_id,
                        covers_op_ids,
                        changed_files,
                        &mut journal_emit,
                    )
                    .await;
                    if let Err(err) = outcome {
                        state = TurnState::Failed(TypedError::new(
                            NanoErrorKind::CompactionFailed,
                            format!("reactive compaction failed: {err}"),
                        ));
                        emit(
                            &mut ops,
                            turn_end_op(
                                turn_id,
                                nano_session::op::TurnOutcome::Failed,
                                &mut turn_usage,
                                &mut usage_recorded,
                                &self.robustness.extra_usage,
                            ),
                        );
                        break;
                    }
                    tokens.reset();
                    continue; // retry the model call once, post-compaction
                }
                Err(err) => {
                    // P1 §3.5: a failed/cancelled request with no usage
                    // evidence settles conservatively (no refund).
                    if let Some(reservation) = reservation.as_mut() {
                        let delta = reservation
                            .settle_conservative(&self.model_name, tokens.estimate(&messages));
                        turn_usage.add_sum(&delta);
                        usage_recorded = true;
                    }
                    state = TurnState::Failed(crate::error_map::typed_error_of_model(&err));
                    break;
                }
            };
            // The server sample is authoritative (C1 §2): the LAST REQUEST's
            // input_tokens, covering the messages as they stood at the call.
            tokens.record_usage(&response.usage, messages.len());
            last_usage = response.usage.clone();
            // ── P1 §3.2/§3.4/§3.5: settle the reservation and feed the
            // turn-scoped accumulator with EXACTLY what the meter charged —
            // per response, per step (never the turn's last response only).
            let missing_usage = response.usage.input_tokens == 0
                && response.usage.output_tokens == 0
                && response.usage.cached_input_tokens.is_none()
                && response.usage.reasoning_tokens.is_none();
            match (&self.robustness.meter, reservation.as_mut()) {
                (Some(_), Some(reservation)) if missing_usage => {
                    // §3.5 (Q4): the wire reported nothing — the
                    // conservative charge with journaled provenance.
                    let delta = reservation
                        .settle_conservative(&self.model_name, tokens.estimate(&messages));
                    turn_usage.add_sum(&delta);
                    usage_recorded = true;
                }
                (Some(meter), Some(reservation)) => {
                    let delta = reservation.settle_success(&self.model_name, &response.usage);
                    turn_usage.add_sum(&delta);
                    usage_recorded = true;
                    // §4.1: surface the 80% crossing (typed notice,
                    // latest-wins, once per crossing).
                    if let Some(warn) = meter.take_pending_warn()
                        && let Some(host) = self.robustness.observer
                    {
                        host(ModelObservation::BudgetWarn {
                            limit: warn.limit,
                            observed: warn.observed,
                            pct_used: warn.pct_used,
                        });
                    }
                }
                (Some(meter), None) => {
                    // Defensive: a meter without a reservation cannot occur
                    // (they are created together above); record directly.
                    let delta = meter.record_usage(&self.model_name, &response.usage);
                    turn_usage.add_sum(&delta);
                    usage_recorded = true;
                }
                (None, _) => {
                    if !missing_usage {
                        // Pre-P1 posture (no meter): the journaled sum still
                        // records tokens; microcents stay 0 and unpriced —
                        // without a catalog there is no pricing authority.
                        turn_usage.add_provider_reported(
                            response.usage.input_tokens,
                            response.usage.output_tokens,
                            response.usage.cached_input_tokens.unwrap_or(0),
                            response.usage.reasoning_tokens.unwrap_or(0),
                            0,
                            false,
                        );
                        usage_recorded = true;
                    }
                }
            }

            if matches!(state, TurnState::Understand | TurnState::Replan) {
                transition(&mut state, &mut history, TurnState::Plan);
            }

            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut text_parts: Vec<String> = Vec::new();
            for event in &response.events {
                match event {
                    ModelEvent::TextDelta(text) => text_parts.push(text.clone()),
                    ModelEvent::ToolCallComplete(call) => tool_calls.push(call.clone()),
                    _ => {}
                }
            }
            if !text_parts.is_empty() {
                final_text = text_parts.join("");
            }

            // Record the assistant turn BEFORE any tool results: the
            // completions wire requires assistant(tool_calls) -> tool(result)
            // ordering, otherwise the model sees incoherent context and
            // repeats itself (the 20-read loop root cause).
            if !tool_calls.is_empty() || !text_parts.is_empty() {
                let mut assistant_content: Vec<nano_model::types::ContentBlock> = Vec::new();
                if !text_parts.is_empty() {
                    assistant_content.push(nano_model::types::ContentBlock::Text {
                        text: final_text.clone(),
                    });
                }
                for call in &tool_calls {
                    assistant_content.push(nano_model::types::ContentBlock::ToolUse {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.arguments.clone(),
                    });
                }
                messages.push(Message {
                    role: nano_model::types::Role::Assistant,
                    content: assistant_content,
                });
                // Journal the reply text so a restored session can rebuild
                // the assistant side of the transcript.
                if !text_parts.is_empty() {
                    emit(
                        &mut ops,
                        Op::AssistantText {
                            turn_id: turn_id.into(),
                            text: final_text.clone(),
                        },
                    );
                }
            }

            if tool_calls.is_empty() {
                // C9 §3.2 codex parity: pending steers mean the turn is NOT
                // done — they drain at the next loop top and the model gets
                // another sampling step. Cancel still wins: the loop-top
                // cancel check runs BEFORE the drain.
                if self
                    .robustness
                    .steer
                    .as_ref()
                    .is_some_and(|handle| handle.has_pending())
                {
                    transition(&mut state, &mut history, TurnState::Understand);
                    continue;
                }
                // No more actions: verify then complete.
                transition(&mut state, &mut history, TurnState::Verify);
                emit(
                    &mut ops,
                    turn_end_op(
                        turn_id,
                        nano_session::op::TurnOutcome::Completed,
                        &mut turn_usage,
                        &mut usage_recorded,
                        &self.robustness.extra_usage,
                    ),
                );
                transition(&mut state, &mut history, TurnState::Complete);
                break;
            }

            transition(&mut state, &mut history, TurnState::Act);
            let mut step_progress = ProgressSignals::default();
            for (index, call) in tool_calls.iter().enumerate() {
                budget_tracker.record_tool_call();
                let key = ToolCallKey::new(&call.name, &call.arguments);
                match protection.check(&key) {
                    RepeatAction::Allow => {}
                    RepeatAction::Remind(reminder) => {
                        // C1 source fix (was turn.rs:318-321): the assistant
                        // ToolUse is already journaled above, so a skipped
                        // call MUST still be paired with a synthesized
                        // tool_result — the shared encoding keeps the
                        // Completions/Anthropic surfaces from diverging.
                        messages.push(Message::tool_result(
                            &call.id,
                            "[tool call skipped: repeat-protection reminder issued]",
                            true,
                        ));
                        messages.push(Message::system(reminder));
                        continue;
                    }
                    RepeatAction::ForceStop(reason) => {
                        // C1 source fix (was turn.rs:322-325): pair the
                        // current AND every remaining call in the batch
                        // before stopping, so no ToolUse is ever stranded.
                        for skipped in &tool_calls[index..] {
                            messages.push(Message::tool_result(
                                &skipped.id,
                                "[tool call skipped: repeat-protection force stop]",
                                true,
                            ));
                        }
                        state = TurnState::Stopped(StopInfo::new(
                            NanoErrorKind::RepeatForceStop,
                            reason,
                        ));
                        break;
                    }
                }
                // C7/D4: the ToolCall op is journaled BEFORE the approval
                // gate runs, so a denial is an honest journaled + framed
                // beat instead of an invisible one. Journal-first is
                // fail-closed: a failed append fails the turn with
                // journal_unavailable rather than letting an unjournaled
                // live frame out (the sink skips the frame on append
                // failure).
                if !emit(
                    &mut ops,
                    Op::ToolCall {
                        turn_id: turn_id.into(),
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        args: call.arguments.clone(),
                    },
                ) {
                    state = TurnState::Failed(TypedError::new(
                        NanoErrorKind::JournalUnavailable,
                        "journal append failed for tool call",
                    ));
                    break;
                }
                if let Some(gate) = self.approval {
                    if gate.approve(call) == ApprovalDecision::Deny {
                        // C2: a mode-categorical denial names the mode so the
                        // model stops retrying variants of the forbidden call.
                        let text = match gate.denial_reason() {
                            Some(reason) => format!("denied by approval gate: {reason}"),
                            None => "denied by approval gate".to_string(),
                        };
                        // D4: the denial is a journaled, framed failed
                        // ToolResult (kind approval_denied) — and a denial
                        // whose journal append fails is a turn-fatal journal
                        // error, never a silently-dropped call.
                        if !emit(
                            &mut ops,
                            Op::ToolResult {
                                call_id: call.id.clone(),
                                ok: false,
                                output_digest: format!("len:{}", text.len()),
                                changed_files: vec![],
                                error_kind: Some(NanoErrorKind::ApprovalDenied),
                            },
                        ) {
                            state = TurnState::Failed(TypedError::new(
                                NanoErrorKind::JournalUnavailable,
                                "journal append failed for denial result",
                            ));
                            break;
                        }
                        messages.push(Message::tool_result(&call.id, text, true));
                        continue;
                    }
                }
                let outcome = self.tools.execute_cancellable(call, cancel).await;
                step_progress.files_changed |= outcome.progress.files_changed;
                step_progress.process_outcome_changed |= outcome.progress.process_outcome_changed;
                step_progress.new_information |= outcome.progress.new_information;
                // The journaled record carries the KIND and never the raw
                // error text (D5 — the digest-only invariant holds; the
                // presentation is re-derivable from the table on replay).
                if !emit(
                    &mut ops,
                    Op::ToolResult {
                        call_id: call.id.clone(),
                        ok: outcome.ok,
                        output_digest: format!("len:{}", outcome.output.len()),
                        changed_files: if outcome.progress.files_changed {
                            vec![call.name.clone()]
                        } else {
                            vec![]
                        },
                        error_kind: outcome.error_kind,
                    },
                ) {
                    state = TurnState::Failed(TypedError::new(
                        NanoErrorKind::JournalUnavailable,
                        "journal append failed for tool result",
                    ));
                    break;
                }
                messages.push(Message {
                    role: nano_model::types::Role::Tool,
                    content: vec![nano_model::types::ContentBlock::ToolResult {
                        tool_use_id: call.id.clone(),
                        content: outcome.output.clone(),
                        is_error: !outcome.ok,
                    }],
                });
            }
            if matches!(state, TurnState::Stopped(_) | TurnState::Failed(_)) {
                emit(
                    &mut ops,
                    turn_end_op(
                        turn_id,
                        nano_session::op::TurnOutcome::Failed,
                        &mut turn_usage,
                        &mut usage_recorded,
                        &self.robustness.extra_usage,
                    ),
                );
                break;
            }

            transition(&mut state, &mut history, TurnState::Observe);
            match progress_tracker.observe(&step_progress) {
                ProgressAction::Continue => {
                    transition(&mut state, &mut history, TurnState::Understand);
                }
                ProgressAction::Replan => {
                    transition(&mut state, &mut history, TurnState::Replan);
                    messages.push(Message::system(
                        "No observable progress in several steps. Stop and reconsider: what is the actual goal, and what is a materially different next action?",
                    ));
                }
                ProgressAction::Stop => {
                    state = TurnState::Stopped(StopInfo::new(
                        NanoErrorKind::NoProgress,
                        "no observable progress for 6 consecutive steps",
                    ));
                    emit(
                        &mut ops,
                        turn_end_op(
                            turn_id,
                            nano_session::op::TurnOutcome::Failed,
                            &mut turn_usage,
                            &mut usage_recorded,
                            &self.robustness.extra_usage,
                        ),
                    );
                    break;
                }
            }
        }

        // C9: the turn owns the steer queue lifecycle — close it exactly
        // once, on ANY exit (completion, failure, cancel). Still-queued
        // steers drop WITH per-submitter notification (cancel-beats-steer:
        // a cancelled turn's pending steers are dropped here, each notified
        // exactly once). Then flush the final coalesced rate-limit snapshot.
        if let Some(handle) = &self.robustness.steer {
            handle.close();
        }
        flush_rate_limit(&rate_limit_dirty, &latest_rate_limit);
        // P1: drain any late externally-attributed usage (the grounding
        // seam) into the sum the result reports.
        drain_extra_usage(
            &self.robustness.extra_usage,
            &mut turn_usage,
            &mut usage_recorded,
        );

        TurnResult {
            state,
            history,
            steps: budget_tracker.steps_count(),
            tool_calls: budget_tracker.tool_calls_count(),
            final_text,
            ops,
            usage: last_usage,
            turn_usage: if usage_recorded {
                Some(turn_usage)
            } else {
                None
            },
        }
    }
}

/// Durable-effect inventory at compaction time (C1 §6): the union of
/// `changed_files` over journaled tool results. The summary replaces the
/// transcript; effects must survive or replay diverges.
fn collect_changed_files(ops: &[OpEnvelope]) -> Vec<String> {
    let mut files: Vec<String> = ops
        .iter()
        .filter_map(|envelope| match &envelope.op {
            Op::ToolResult { changed_files, .. } => Some(changed_files.iter().cloned()),
            _ => None,
        })
        .flatten()
        .collect();
    files.sort();
    files.dedup();
    files
}
