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
use nano_model::types::{Message, ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall};
use nano_session::op::{Op, OpEnvelope};
use std::fmt::Debug;

/// The model boundary the engine drives. FluxCompletionsClient implements
/// this in production; tests script a mock.
#[async_trait::async_trait]
pub trait ModelDriver: Debug + Send + Sync {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;
}

/// One tool invocation the engine can perform (fs/shell/etc. register here).
/// Async (C3/C4 Q1, Option A — mirrors the ModelDriver precedent above):
/// web_fetch awaits the egress pipeline; fs/shell arms stay synchronous
/// internally. No ambient-runtime bridge.
#[async_trait::async_trait]
pub trait ToolExecutor: Debug + Send + Sync {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub ok: bool,
    pub output: String,
    pub progress: ProgressSignals,
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
    Failed(String),
    Stopped(String),
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
        sink: &mut dyn FnMut(&OpEnvelope) -> bool,
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
        sink: &mut dyn FnMut(&OpEnvelope) -> bool,
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
        sink: Option<&mut dyn FnMut(&OpEnvelope) -> bool>,
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
        // C1 context management: server-anchored token accounting, the
        // consecutive-ineffective-compaction guard, and the one-shot reactive
        // overflow retry.
        let mut tokens = TokenTracker::default();
        let mut compact_guard = AutoCompactGuard::default();
        let mut compaction_counter = 0u32;
        let mut reactive_compaction_used = false;

        transition(&mut state, &mut history, TurnState::Understand);
        loop {
            if cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
                state = TurnState::Stopped("cancelled by caller".into());
                emit(
                    &mut ops,
                    Op::TurnEnd {
                        turn_id: turn_id.into(),
                        outcome: nano_session::op::TurnOutcome::Cancelled,
                    },
                );
                break;
            }
            if let Err(exhausted) = budget_tracker.check(&self.budget) {
                state = TurnState::Stopped(format!("budget exhausted: {exhausted:?}"));
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
                        state = TurnState::Failed(format!("auto-compaction failed: {err}"));
                        emit(
                            &mut ops,
                            Op::TurnEnd {
                                turn_id: turn_id.into(),
                                outcome: nano_session::op::TurnOutcome::Failed,
                            },
                        );
                        break;
                    }
                    // Post-compaction recompute: drop the stale server sample;
                    // the next real sample re-baselines.
                    tokens.reset();
                    if tokens.estimate(&messages) >= config.auto_compact_limit
                        && compact_guard.record_ineffective()
                    {
                        state = TurnState::Failed(
                            "auto-compaction loop guard: two consecutive compactions failed to bring the context under the limit"
                                .into(),
                        );
                        emit(
                            &mut ops,
                            Op::TurnEnd {
                                turn_id: turn_id.into(),
                                outcome: nano_session::op::TurnOutcome::Failed,
                            },
                        );
                        break;
                    }
                }
            }

            let request = ModelRequest {
                model: self.model_name.clone(),
                messages: messages.clone(),
                tools: self.tool_definitions.clone(),
                max_tokens: Some(4096),
                stream: false,
                ..Default::default()
            };
            let response = match self.model.complete(&request).await {
                Ok(r) => r,
                // [R1] Reactive overflow: the tail heuristic undershot and the
                // model rejected the request — route into the SAME compaction
                // path exactly once instead of failing the turn. A second
                // overflow after that compaction falls through to the generic
                // failure below.
                Err(ModelError::ContextOverflow(_))
                    if self.compaction.is_some() && !reactive_compaction_used =>
                {
                    reactive_compaction_used = true;
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
                        state = TurnState::Failed(format!("reactive compaction failed: {err}"));
                        emit(
                            &mut ops,
                            Op::TurnEnd {
                                turn_id: turn_id.into(),
                                outcome: nano_session::op::TurnOutcome::Failed,
                            },
                        );
                        break;
                    }
                    tokens.reset();
                    continue; // retry the model call once, post-compaction
                }
                Err(err) => {
                    state = TurnState::Failed(format!("model call failed: {err}"));
                    break;
                }
            };
            // The server sample is authoritative (C1 §2): the LAST REQUEST's
            // input_tokens, covering the messages as they stood at the call.
            tokens.record_usage(&response.usage, messages.len());

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
                // No more actions: verify then complete.
                transition(&mut state, &mut history, TurnState::Verify);
                emit(
                    &mut ops,
                    Op::TurnEnd {
                        turn_id: turn_id.into(),
                        outcome: nano_session::op::TurnOutcome::Completed,
                    },
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
                        state = TurnState::Stopped(reason);
                        break;
                    }
                }
                if let Some(gate) = self.approval {
                    if gate.approve(call) == ApprovalDecision::Deny {
                        // C2: a mode-categorical denial names the mode so the
                        // model stops retrying variants of the forbidden call.
                        let text = match gate.denial_reason() {
                            Some(reason) => format!("denied by approval gate: {reason}"),
                            None => "denied by approval gate".to_string(),
                        };
                        messages.push(Message::tool_result(&call.id, text, true));
                        continue;
                    }
                }
                emit(
                    &mut ops,
                    Op::ToolCall {
                        turn_id: turn_id.into(),
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        args: call.arguments.clone(),
                    },
                );
                let outcome = self.tools.execute(call).await;
                step_progress.files_changed |= outcome.progress.files_changed;
                step_progress.process_outcome_changed |= outcome.progress.process_outcome_changed;
                step_progress.new_information |= outcome.progress.new_information;
                emit(
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
                    },
                );
                messages.push(Message {
                    role: nano_model::types::Role::Tool,
                    content: vec![nano_model::types::ContentBlock::ToolResult {
                        tool_use_id: call.id.clone(),
                        content: outcome.output.clone(),
                        is_error: !outcome.ok,
                    }],
                });
            }
            if matches!(state, TurnState::Stopped(_)) {
                emit(
                    &mut ops,
                    Op::TurnEnd {
                        turn_id: turn_id.into(),
                        outcome: nano_session::op::TurnOutcome::Failed,
                    },
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
                    state =
                        TurnState::Stopped("no observable progress for 6 consecutive steps".into());
                    emit(
                        &mut ops,
                        Op::TurnEnd {
                            turn_id: turn_id.into(),
                            outcome: nano_session::op::TurnOutcome::Failed,
                        },
                    );
                    break;
                }
            }
        }

        TurnResult {
            state,
            history,
            steps: budget_tracker.steps_count(),
            tool_calls: budget_tracker.tool_calls_count(),
            final_text,
            ops,
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
