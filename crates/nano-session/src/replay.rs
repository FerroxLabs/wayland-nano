//! Reducer-fold replay: envelopes → executable session state.
//!
//! Restore invariants (Kimi-derived):
//! - a stranded `TurnBegin` (no `TurnEnd`) marks the turn `Interrupted`: the
//!   pending user input is preserved, tool calls that already returned keep
//!   their results, and no tool call is re-executed on resume;
//! - a stranded `CompactionBegin` (no Complete/Cancel) resets to `Idle`;
//! - duplicate envelope ids never double-apply;
//! - `Unknown` ops are skipped without failing the fold;
//! - covered-by-compaction ops keep their durable effects (changed files)
//!   but drop from the pending-execution surface.

use crate::op::Op;
use crate::op::OpEnvelope;
use crate::op::TurnOutcome;
use std::collections::BTreeSet;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionPhase {
    Idle,
    Running { compaction_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenToolCall {
    pub turn_id: String,
    pub call_id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenTurn {
    pub turn_id: String,
    pub input: String,
}

#[derive(Debug, Default)]
pub struct SessionState {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    /// The in-progress (or crash-interrupted) turn, if any.
    pub open_turn: Option<OpenTurn>,
    pub turn_interrupted: bool,
    pub open_tool_calls: Vec<OpenToolCall>,
    pub changed_files: BTreeSet<String>,
    pub compaction: Option<CompactionPhase>,
    pub last_compaction_summary: Option<String>,
    seen_ids: HashSet<String>,
}

impl SessionState {
    pub fn fold(envelopes: &[OpEnvelope]) -> Self {
        let mut state = SessionState {
            compaction: Some(CompactionPhase::Idle),
            ..Default::default()
        };
        for envelope in envelopes {
            state.apply(envelope);
        }
        state.restore_invariants();
        state
    }

    fn apply(&mut self, envelope: &OpEnvelope) {
        if !self.seen_ids.insert(envelope.id.clone()) {
            return; // idempotent: duplicate ids never double-apply
        }
        match &envelope.op {
            Op::SessionBegin { session_id, cwd } => {
                self.session_id = Some(session_id.clone());
                self.cwd = Some(cwd.clone());
            }
            Op::TurnBegin { turn_id, input } => {
                self.open_turn = Some(OpenTurn {
                    turn_id: turn_id.clone(),
                    input: input.clone(),
                });
                self.turn_interrupted = false;
            }
            Op::ToolCall {
                turn_id,
                call_id,
                name,
                args,
            } => {
                self.open_tool_calls.push(OpenToolCall {
                    turn_id: turn_id.clone(),
                    call_id: call_id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                });
            }
            Op::ToolResult {
                call_id,
                changed_files,
                ..
            } => {
                self.open_tool_calls.retain(|call| &call.call_id != call_id);
                self.changed_files.extend(changed_files.iter().cloned());
            }
            Op::TurnEnd { outcome, .. } => {
                self.open_turn = None;
                self.turn_interrupted = matches!(outcome, TurnOutcome::Interrupted);
            }
            Op::CompactionBegin { compaction_id } => {
                self.compaction = Some(CompactionPhase::Running {
                    compaction_id: compaction_id.clone(),
                });
            }
            Op::CompactionComplete {
                summary,
                changed_files,
                ..
            } => {
                self.compaction = Some(CompactionPhase::Idle);
                self.last_compaction_summary = Some(summary.clone());
                self.changed_files.extend(changed_files.iter().cloned());
            }
            Op::CompactionCancel { .. } => {
                self.compaction = Some(CompactionPhase::Idle);
            }
            // Assistant text is transcript, not execution state: replay cares
            // about open work and durable effects, not the wording of replies.
            Op::AssistantText { .. } => {}
            Op::Unknown => {}
        }
    }

    /// Post-fold restore rules: anything left `running` at the tail of the
    /// journal is a crash artifact and resets to a safe state.
    fn restore_invariants(&mut self) {
        if self.open_turn.is_some() {
            self.turn_interrupted = true;
        }
        if matches!(self.compaction, Some(CompactionPhase::Running { .. })) {
            self.compaction = Some(CompactionPhase::Idle);
        }
    }
}
