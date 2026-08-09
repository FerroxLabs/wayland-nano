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
}

impl<'a> TurnEngine<'a> {
    pub async fn run_turn(&self, turn_id: &str, input: &str) -> TurnResult {
        let mut ops: Vec<OpEnvelope> = Vec::new();
        let mut next_id = 0u32;
        let mut emit = |ops: &mut Vec<OpEnvelope>, op: Op| {
            next_id += 1;
            ops.push(OpEnvelope::new(
                format!("{turn_id}-{next_id}"),
                "now",
                op,
            ));
        };

        emit(&mut ops, Op::TurnBegin {
            turn_id: turn_id.into(),
            input: input.into(),
        });

        let mut state = TurnState::Receive;
        let mut protection = RepeatBreaker::default();
        let mut progress_tracker = NoProgressTracker::default();
        let mut budget_tracker = BudgetTracker::default();
        budget_tracker.start_turn();

        let mut messages = vec![Message::user(input)];
        let mut final_text = String::new();

        state = TurnState::Understand;
        loop {
            if let Err(exhausted) = budget_tracker.check(&self.budget) {
                state = TurnState::Stopped(format!("budget exhausted: {exhausted:?}"));
                break;
            }
            budget_tracker.record_step();

            let request = ModelRequest {
                model: self.model_name.clone(),
                messages: messages.clone(),
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
                state = TurnState::Plan;
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

            if tool_calls.is_empty() {
                // No more actions: verify then complete.
                state = TurnState::Verify;
                emit(&mut ops, Op::TurnEnd {
                    turn_id: turn_id.into(),
                    outcome: nano_session::op::TurnOutcome::Completed,
                });
                state = TurnState::Complete;
                break;
            }

            state = TurnState::Act;
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
                emit(&mut ops, Op::ToolCall {
                    turn_id: turn_id.into(),
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.arguments.clone(),
                });
                let outcome = self.tools.execute(call);
                step_progress.files_changed |= outcome.progress.files_changed;
                step_progress.process_outcome_changed |= outcome.progress.process_outcome_changed;
                step_progress.new_information |= outcome.progress.new_information;
                emit(&mut ops, Op::ToolResult {
                    call_id: call.id.clone(),
                    ok: outcome.ok,
                    output_digest: format!("len:{}", outcome.output.len()),
                    changed_files: if outcome.progress.files_changed {
                        vec![call.name.clone()]
                    } else {
                        vec![]
                    },
                });
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
                emit(&mut ops, Op::TurnEnd {
                    turn_id: turn_id.into(),
                    outcome: nano_session::op::TurnOutcome::Failed,
                });
                break;
            }

            state = TurnState::Observe;
            match progress_tracker.observe(&step_progress) {
                ProgressAction::Continue => {
                    state = TurnState::Understand;
                }
                ProgressAction::Replan => {
                    state = TurnState::Replan;
                    messages.push(Message::system(
                        "No observable progress in several steps. Stop and reconsider: what is the actual goal, and what is a materially different next action?",
                    ));
                }
                ProgressAction::Stop => {
                    state = TurnState::Stopped(
                        "no observable progress for 6 consecutive steps".into(),
                    );
                    emit(&mut ops, Op::TurnEnd {
                        turn_id: turn_id.into(),
                        outcome: nano_session::op::TurnOutcome::Failed,
                    });
                    break;
                }
            }
        }

        TurnResult {
            state,
            steps: budget_tracker.steps_count(),
            tool_calls: budget_tracker.tool_calls_count(),
            final_text,
            ops,
        }
    }
}
