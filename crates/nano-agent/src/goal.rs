//! Goal mode (C11 §4): one long-running objective per session, journaled
//! transitions, budgets, and a trusted completion channel.
//!
//! Invariants:
//! - transitions are JOURNALED ops via trusted paths only — the
//!   `goal_complete` tool handler (host-side code), budget trips, engine
//!   errors, or explicit commands. Model prose claiming completion has zero
//!   protocol effect (the forge test pins this).
//! - journal-first everywhere: `GoalStatus`/`GoalEnd` are appended and
//!   flushed BEFORE the in-memory effect.
//! - budget enforcement rides the turn-loop seam: a GOAL-LEVEL accumulator
//!   (cumulative input+output tokens over goal turns, a turn counter, a
//!   wall-clock deadline via [`Clock`]) checked between turns — never a
//!   mid-batch abort.
//! - bounded enums and capped strings: objective ≤ 4000 chars, summary ≤
//!   2000 chars, reasons are the `GoalReason` enum.

use crate::clock::Clock;
use nano_session::GoalBudgets;
use nano_session::GoalOutcome;
use nano_session::GoalReason;
use nano_session::GoalStatusKind;
use nano_session::JournalCoordinator;
use nano_session::MAX_GOAL_OBJECTIVE_LEN;
use nano_session::MAX_GOAL_SUMMARY_LEN;
use nano_session::Op;
use nano_session::OpEnvelope;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

/// Shared control cell between the goal driver and the `goal_complete` tool
/// handler. The tool handler journals (journal-first) and THEN marks the
/// completion here; the driver observes it after the turn.
#[derive(Debug)]
pub struct GoalControl {
    goal_id: String,
    completed_summary: Mutex<Option<String>>,
}

impl GoalControl {
    pub fn new(goal_id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            goal_id: goal_id.into(),
            completed_summary: Mutex::new(None),
        })
    }

    pub fn goal_id(&self) -> &str {
        &self.goal_id
    }

    pub fn completed_summary(&self) -> Option<String> {
        self.completed_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn mark_complete(&self, summary: String) {
        *self
            .completed_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(summary);
    }
}

/// Why a goal run ended. Exit-code mapping lives in exec (§2.2): Complete →
/// 0; Blocked → 3; Paused → 6; EngineError / TurnBudgetTrip → 1 (turn-level
/// failure class wins over the goal's journaled blocked record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalDriveOutcome {
    Complete {
        summary: String,
    },
    /// Terminal blocked journaled from a trusted source (budget trip,
    /// engine error, explicit cancel).
    Blocked {
        reason: GoalReason,
    },
    /// The goal paused (cancel/kill normalization); resumable.
    Paused,
    /// Model/runtime failure inside a turn — the goal is journaled
    /// `blocked(error)`, but the failure class is the turn's.
    EngineError,
    /// A TURN-level `TurnBudget` trip (the per-turn seam every turn has) —
    /// distinct from the goal-level accumulator; exec maps this to exit 1.
    TurnBudgetTrip,
}

/// What one goal turn produced, handed back to the driver.
#[derive(Debug)]
pub struct GoalTurnOutcome {
    /// Engine state label: "complete" | "failed" | "stopped".
    pub stop: GoalTurnStop,
    /// Server-reported usage for the turn (input+output accumulates into
    /// the GOAL-level token budget).
    pub usage: nano_model::types::Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalTurnStop {
    Complete,
    Failed,
    /// Turn-level budget trip (TurnBudget at the loop seam).
    TurnBudget,
    /// Cancelled by the caller.
    Cancelled,
}

/// Journals a goal transition pair, journal-first: `GoalStatus` then (for
/// terminals) `GoalEnd`, each appended and synced before the next effect.
pub fn journal_goal_transition(
    journal: &JournalCoordinator,
    session_id: &str,
    sequence: &AtomicU64,
    goal_id: &str,
    status: GoalStatusKind,
    reason: GoalReason,
    terminal: Option<GoalOutcome>,
) -> std::io::Result<()> {
    // Nanos + sequence: ids must never collide across processes/resumes —
    // the journal writer dedupes repeated ids, so a collision would
    // silently DROP the transition.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let next = || {
        format!(
            "{}-goal-{}-{}",
            session_id,
            nanos,
            sequence.fetch_add(1, Ordering::SeqCst)
        )
    };
    // P3 §3.3: the coordinator's append is fsync-durable per call, so the
    // explicit sync steps fold into it.
    journal.append(&OpEnvelope::new(
        next(),
        "now",
        Op::GoalStatus {
            goal_id: goal_id.to_string(),
            status,
            reason,
        },
    ))?;
    if let Some(outcome) = terminal {
        journal.append(&OpEnvelope::new(
            next(),
            "now",
            Op::GoalEnd {
                goal_id: goal_id.to_string(),
                outcome,
            },
        ))?;
    }
    Ok(())
}

/// The goal driver (§4.4): while the goal is active, run a turn; if no
/// completion was declared during it, inject a continuation prompt
/// (objective + completion criterion + budget remaining) and run the next
/// turn, until a terminal state or a budget trip. Budget checks happen at
/// the loop top (between turns), never mid-batch.
///
/// `run_turn` is the host's turn closure: it takes the prompt text and runs
/// ONE ordinary turn (all tool execution stays inside the existing turn
/// loop with its sandbox, policy engine, and C2 gate — the driver only
/// injects prompts).
#[allow(clippy::too_many_arguments)]
pub async fn drive_goal<F, Fut>(
    journal: Arc<JournalCoordinator>,
    journal_sequence: &AtomicU64,
    session_id: &str,
    goal_id: &str,
    objective: &str,
    budgets: &GoalBudgets,
    control: Arc<GoalControl>,
    clock: &dyn Clock,
    mut run_turn: F,
) -> GoalDriveOutcome
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = GoalTurnOutcome>,
{
    let started_ms = clock.now_ms();
    let mut tokens_used: u64 = 0;
    let mut turns_done: u64 = 0;
    let mut prompt = objective.to_string();

    loop {
        // ── Loop-top budget seam (beside C1's compaction trigger) ──
        let trip = if budgets
            .token_budget
            .is_some_and(|budget| tokens_used >= budget)
        {
            Some(GoalReason::BudgetTokens)
        } else if budgets
            .turn_budget
            .is_some_and(|budget| turns_done >= budget)
        {
            Some(GoalReason::BudgetTurns)
        } else if budgets
            .wall_clock_budget_ms
            .is_some_and(|budget| clock.now_ms().saturating_sub(started_ms) >= budget)
        {
            Some(GoalReason::BudgetWallclock)
        } else {
            None
        };
        if let Some(reason) = trip {
            let _ = journal_goal_transition(
                &journal,
                session_id,
                journal_sequence,
                goal_id,
                GoalStatusKind::Blocked,
                reason,
                Some(GoalOutcome::Blocked),
            );
            return GoalDriveOutcome::Blocked { reason };
        }

        let outcome = run_turn(prompt).await;
        turns_done += 1;
        tokens_used = tokens_used.saturating_add(
            outcome
                .usage
                .input_tokens
                .saturating_add(outcome.usage.output_tokens),
        );

        // Trusted completion: the goal_complete tool handler journaled
        // GoalStatus{complete}+GoalEnd{complete} journal-first during the
        // turn; the driver just observes and stops.
        if let Some(summary) = control.completed_summary() {
            return GoalDriveOutcome::Complete { summary };
        }

        match outcome.stop {
            GoalTurnStop::Complete => {}
            GoalTurnStop::Cancelled => {
                let _ = journal_goal_transition(
                    &journal,
                    session_id,
                    journal_sequence,
                    goal_id,
                    GoalStatusKind::Paused,
                    GoalReason::Cancelled,
                    None,
                );
                return GoalDriveOutcome::Paused;
            }
            GoalTurnStop::TurnBudget => {
                let _ = journal_goal_transition(
                    &journal,
                    session_id,
                    journal_sequence,
                    goal_id,
                    GoalStatusKind::Blocked,
                    GoalReason::Error,
                    Some(GoalOutcome::Blocked),
                );
                return GoalDriveOutcome::TurnBudgetTrip;
            }
            GoalTurnStop::Failed => {
                let _ = journal_goal_transition(
                    &journal,
                    session_id,
                    journal_sequence,
                    goal_id,
                    GoalStatusKind::Blocked,
                    GoalReason::Error,
                    Some(GoalOutcome::Blocked),
                );
                return GoalDriveOutcome::EngineError;
            }
        }

        // Continuation driver (kimi pattern): objective + completion
        // criterion + budget remaining.
        prompt = continuation_prompt(
            objective,
            budgets,
            tokens_used,
            turns_done,
            started_ms,
            clock,
        );
    }
}

fn continuation_prompt(
    objective: &str,
    budgets: &GoalBudgets,
    tokens_used: u64,
    turns_done: u64,
    started_ms: u64,
    clock: &dyn Clock,
) -> String {
    let mut remaining = Vec::new();
    if let Some(budget) = budgets.token_budget {
        remaining.push(format!(
            "tokens remaining: {}",
            budget.saturating_sub(tokens_used)
        ));
    }
    if let Some(budget) = budgets.turn_budget {
        remaining.push(format!(
            "turns remaining: {}",
            budget.saturating_sub(turns_done)
        ));
    }
    if let Some(budget) = budgets.wall_clock_budget_ms {
        remaining.push(format!(
            "wall-clock remaining: {}ms",
            budget.saturating_sub(clock.now_ms().saturating_sub(started_ms))
        ));
    }
    format!(
        "Continue working on the goal. Objective: {objective}\n\
         Completion criterion: when the objective is fully achieved, call the \
         goal_complete tool with a summary. Do NOT just say it is done — only \
         the goal_complete tool ends the goal.\n\
         Budget status: {}.",
        if remaining.is_empty() {
            "unbounded".to_string()
        } else {
            remaining.join(", ")
        }
    )
}

/// The `goal_complete` tool definition (§4.4): advertised ONLY while a goal
/// is active. Summary is schema-capped at 2000 chars.
pub fn goal_complete_tool_definition() -> nano_model::types::ToolDefinition {
    nano_model::types::ToolDefinition {
        name: "goal_complete".into(),
        description: "Declare the active goal complete. The ONLY way to complete a goal — \
             prose claiming completion has no effect. Args: summary (max 2000 chars)."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "maxLength": MAX_GOAL_SUMMARY_LEN}
            },
            "required": ["summary"]
        }),
    }
}

/// `goal_complete` is a session-internal control channel: it writes only to
/// the session journal (journal-first), never the filesystem or the
/// network, so every approval gate auto-approves it in every mode.
pub fn is_control_tool(name: &str) -> bool {
    name == "goal_complete"
}

/// Validates a goal objective at the write path (capped strings are
/// schema-validated, not trusted).
pub fn validate_objective(objective: &str) -> Result<(), String> {
    if objective.trim().is_empty() {
        return Err("goal objective must not be empty".into());
    }
    if objective.chars().count() > MAX_GOAL_OBJECTIVE_LEN {
        return Err(format!(
            "goal objective exceeds {MAX_GOAL_OBJECTIVE_LEN} chars"
        ));
    }
    Ok(())
}

/// ToolExecutor decorator adding the trusted `goal_complete` control
/// channel. Every other tool delegates to the inner executor unchanged.
pub struct GoalToolExecutor<'a, T: crate::turn::ToolExecutor> {
    inner: &'a T,
    /// `None` when the session has no active goal: the tool definition is
    /// not advertised then, and a stray call is a typed tool error (never a
    /// forged transition).
    control: Option<Arc<GoalControl>>,
    journal: Arc<JournalCoordinator>,
    session_id: String,
    journal_sequence: Arc<AtomicU64>,
}

impl<T: crate::turn::ToolExecutor> std::fmt::Debug for GoalToolExecutor<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalToolExecutor")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl<'a, T: crate::turn::ToolExecutor> GoalToolExecutor<'a, T> {
    pub fn new(
        inner: &'a T,
        control: Option<Arc<GoalControl>>,
        journal: Arc<JournalCoordinator>,
        session_id: String,
        journal_sequence: Arc<AtomicU64>,
    ) -> Self {
        Self {
            inner,
            control,
            journal,
            session_id,
            journal_sequence,
        }
    }
}

#[async_trait::async_trait]
impl<T: crate::turn::ToolExecutor> crate::turn::ToolExecutor for GoalToolExecutor<'_, T> {
    async fn execute(&self, call: &nano_model::types::ToolCall) -> crate::turn::ToolOutcome {
        if call.name != "goal_complete" {
            return self.inner.execute(call).await;
        }
        let fail0 = |message: &str| crate::turn::ToolOutcome {
            ok: false,
            output: message.to_string(),
            progress: crate::loop_protection::ProgressSignals::default(),
            error_kind: None,
        };
        let Some(control) = &self.control else {
            return fail0("no active goal in this session");
        };
        let summary = call
            .arguments
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let fail = |message: String| crate::turn::ToolOutcome {
            ok: false,
            output: message,
            progress: crate::loop_protection::ProgressSignals::default(),
            error_kind: None,
        };
        if summary.trim().is_empty() {
            return fail("goal_complete requires a non-empty summary".into());
        }
        if summary.chars().count() > MAX_GOAL_SUMMARY_LEN {
            return fail(format!(
                "goal_complete summary exceeds {MAX_GOAL_SUMMARY_LEN} chars"
            ));
        }
        // Journal-first: the durable transition leads the in-memory effect.
        // A journal failure means the control cell is NOT marked — the goal
        // stays active, fail-closed.
        if let Err(err) = journal_goal_transition(
            &self.journal,
            &self.session_id,
            &self.journal_sequence,
            control.goal_id(),
            GoalStatusKind::Complete,
            GoalReason::Unspecified,
            Some(GoalOutcome::Complete),
        ) {
            return fail(format!("cannot journal goal completion: {err}"));
        }
        control.mark_complete(summary.to_string());
        crate::turn::ToolOutcome {
            ok: true,
            output: "goal recorded complete".into(),
            progress: crate::loop_protection::ProgressSignals::default(),
            error_kind: None,
        }
    }

    fn take_image_result(&self, call_id: &str) -> Option<crate::turn::LiveImageToolResult> {
        self.inner.take_image_result(call_id)
    }

    fn image_results_backed(&self) -> bool {
        self.inner.image_results_backed()
    }
}
