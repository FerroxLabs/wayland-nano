//! Turn engine: the agent state machine over a model driver and tool calls,
//! with journal recording and loop protection wired in.
//!
//! States (every one testable): RECEIVE → UNDERSTAND → PLAN → ACT → OBSERVE
//! → CONTINUE/REPLAN → VERIFY → COMPLETE. No cognitive theatre.

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
pub trait ToolExecutor: Debug + Send + Sync {
    fn execute(&self, call: &ToolCall) -> ToolOutcome;
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
}

/// Decides whether a tool call may execute. Production prompts via the host;
/// tests and headless flows use policy-driven implementations.
pub trait ApprovalGate: Debug + Send + Sync {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision;
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
        self.run_turn_inner(turn_id, input, context, None).await
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
        self.run_turn_inner(turn_id, input, None, cancel).await
    }

    async fn run_turn_inner(
        &self,
        turn_id: &str,
        input: &str,
        context: Option<Message>,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> TurnResult {
        let mut ops: Vec<OpEnvelope> = Vec::new();
        let mut next_id = 0u32;
        let mut emit = |ops: &mut Vec<OpEnvelope>, op: Op| {
            next_id += 1;
            ops.push(OpEnvelope::new(format!("{turn_id}-{next_id}"), "now", op));
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

        let mut messages = Vec::new();
        if let Some(context) = context {
            messages.push(context);
        }
        messages.push(Message::user(input));
        let mut final_text = String::new();

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
                Err(err) => {
                    state = TurnState::Failed(format!("model call failed: {err}"));
                    break;
                }
            };

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
            for call in &tool_calls {
                budget_tracker.record_tool_call();
                let key = ToolCallKey::new(&call.name, &call.arguments);
                match protection.check(&key) {
                    RepeatAction::Allow => {}
                    RepeatAction::Remind(reminder) => {
                        messages.push(Message::system(reminder));
                        continue;
                    }
                    RepeatAction::ForceStop(reason) => {
                        state = TurnState::Stopped(reason);
                        break;
                    }
                }
                if let Some(gate) = self.approval {
                    if gate.approve(call) == ApprovalDecision::Deny {
                        messages.push(Message {
                            role: nano_model::types::Role::Tool,
                            content: vec![nano_model::types::ContentBlock::ToolResult {
                                tool_use_id: call.id.clone(),
                                content: "denied by approval gate".into(),
                                is_error: true,
                            }],
                        });
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
                let outcome = self.tools.execute(call);
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
