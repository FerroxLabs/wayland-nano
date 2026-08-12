//! C11 §7 goal-mode tests: budgets (token/turn/wall-clock via TestClock),
//! journal-first completion, the forge test (model TEXT claiming completion
//! is INERT), caps, kill-resume normalization, turn-level disambiguation.

use crate::clock::TestClock;
use crate::goal::{
    GoalControl, GoalDriveOutcome, GoalToolExecutor, GoalTurnOutcome, GoalTurnStop, drive_goal,
    validate_objective,
};
use crate::loop_protection::ProgressSignals;
use crate::turn::{ToolExecutor, ToolOutcome};
use nano_model::types::{ToolCall, Usage};
use nano_session::op::Op;
use nano_session::reader::read_journal;
use nano_session::writer::JournalWriter;
use nano_session::{GoalBudgets, GoalOutcome, GoalReason, GoalStatusKind, SessionState};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "nano-c11-goal-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn goal_ops_in(dir: &std::path::Path) -> Vec<Op> {
    read_journal(&dir.join("s.jsonl"))
        .unwrap()
        .envelopes
        .into_iter()
        .map(|e| e.op)
        .collect()
}

fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        ..Default::default()
    }
}

fn driver_config() -> (Arc<Mutex<JournalWriter>>, AtomicU64, std::path::PathBuf) {
    let dir = tmpdir("drive");
    let journal = Arc::new(Mutex::new(
        JournalWriter::open(&dir.join("s.jsonl")).unwrap(),
    ));
    (journal, AtomicU64::new(1), dir)
}

/// Journals the GoalBegin the exec/host layer writes before the driver runs
/// (drive_goal drives an ACTIVATED goal; activation is the caller's op).
fn journal_goal_begin(journal: &Arc<Mutex<JournalWriter>>, goal_id: &str) {
    journal
        .lock()
        .unwrap()
        .append(&nano_session::OpEnvelope::new(
            format!("s-goalbegin-{goal_id}"),
            "now",
            Op::GoalBegin {
                goal_id: goal_id.into(),
                objective: "obj".into(),
                budgets: GoalBudgets::default(),
            },
        ))
        .unwrap();
}

#[tokio::test]
async fn token_budget_trips_exit_class_blocked() {
    let (journal, sequence, dir) = driver_config();
    let clock = TestClock::new(1_000_000);
    let control = GoalControl::new("g1");
    let budgets = GoalBudgets {
        token_budget: Some(100),
        turn_budget: None,
        wall_clock_budget_ms: None,
    };
    let mut _turns = 0;
    let outcome = drive_goal(
        journal,
        &sequence,
        "s",
        "g1",
        "do the thing",
        &budgets,
        control,
        &clock,
        move |_prompt| {
            _turns += 1;
            async move {
                GoalTurnOutcome {
                    stop: GoalTurnStop::Complete,
                    usage: usage(60, 50), // 110 > 100 after turn 1
                }
            }
        },
    )
    .await;
    assert_eq!(
        outcome,
        GoalDriveOutcome::Blocked {
            reason: GoalReason::BudgetTokens
        }
    );
    let ops = goal_ops_in(&dir);
    assert!(ops.iter().any(|op| matches!(
        op,
        Op::GoalStatus {
            status: GoalStatusKind::Blocked,
            reason: GoalReason::BudgetTokens,
            ..
        }
    )));
    assert!(ops.iter().any(|op| matches!(
        op,
        Op::GoalEnd {
            outcome: GoalOutcome::Blocked,
            ..
        }
    )));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn turn_budget_and_wallclock_trip_via_testclock() {
    // Turn budget.
    let (journal, sequence, dir) = driver_config();
    let clock = TestClock::new(0);
    let budgets = GoalBudgets {
        token_budget: None,
        turn_budget: Some(2),
        wall_clock_budget_ms: None,
    };
    let outcome = drive_goal(
        journal,
        &sequence,
        "s",
        "g1",
        "obj",
        &budgets,
        GoalControl::new("g1"),
        &clock,
        |_prompt| async {
            GoalTurnOutcome {
                stop: GoalTurnStop::Complete,
                usage: usage(1, 1),
            }
        },
    )
    .await;
    assert_eq!(
        outcome,
        GoalDriveOutcome::Blocked {
            reason: GoalReason::BudgetTurns
        }
    );

    // Wall-clock: the TestClock advances past the deadline during the first
    // turn; the loop-top check trips before the second.
    let (journal, sequence, dir3) = driver_config();
    let clock2 = TestClock::new(0);
    let outcome = drive_goal(
        journal,
        &sequence,
        "s",
        "g1",
        "obj",
        &GoalBudgets {
            token_budget: None,
            turn_budget: None,
            wall_clock_budget_ms: Some(5_000),
        },
        GoalControl::new("g1"),
        &clock2,
        {
            let clock3 = &clock2;
            move |_prompt| {
                clock3.advance_ms(6_000); // wall time passes during the turn
                async move {
                    GoalTurnOutcome {
                        stop: GoalTurnStop::Complete,
                        usage: usage(1, 1),
                    }
                }
            }
        },
    )
    .await;
    assert_eq!(
        outcome,
        GoalDriveOutcome::Blocked {
            reason: GoalReason::BudgetWallclock
        }
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir3);
}

/// The forge test (§4.4/§7): model TEXT claiming completion — no tool call —
/// leaves the goal state untouched. The driver keeps driving until a REAL
/// terminal (here: the turn budget).
#[tokio::test]
async fn model_text_claiming_completion_is_inert() {
    let (journal, sequence, dir) = driver_config();
    let clock = TestClock::new(0);
    let budgets = GoalBudgets {
        token_budget: None,
        turn_budget: Some(2),
        wall_clock_budget_ms: None,
    };
    let control = GoalControl::new("g1");
    let outcome = drive_goal(
        journal,
        &sequence,
        "s",
        "g1",
        "obj",
        &budgets,
        control.clone(),
        &clock,
        |_prompt| async {
            // The model SAYS it is done, every turn, in prose only.
            GoalTurnOutcome {
                stop: GoalTurnStop::Complete,
                usage: usage(1, 1),
            }
        },
    )
    .await;
    // Prose never completed the goal: the TURN budget ended it.
    assert_eq!(
        outcome,
        GoalDriveOutcome::Blocked {
            reason: GoalReason::BudgetTurns
        }
    );
    assert!(control.completed_summary().is_none());
    let ops = goal_ops_in(&dir);
    assert!(
        !ops.iter().any(|op| matches!(
            op,
            Op::GoalEnd {
                outcome: GoalOutcome::Complete,
                ..
            }
        )),
        "no forged completion in the journal"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// goal_complete via the trusted tool path: journal-first GoalEnd{complete}
/// and the driver stops with the summary.
#[tokio::test]
async fn goal_complete_tool_is_journal_first_and_stops_driver() {
    struct CompletingTools {
        journal: Arc<Mutex<JournalWriter>>,
        sequence: Arc<AtomicU64>,
        control: Arc<GoalControl>,
        session: String,
    }
    impl std::fmt::Debug for CompletingTools {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("CompletingTools").finish_non_exhaustive()
        }
    }
    #[async_trait::async_trait]
    impl ToolExecutor for CompletingTools {
        async fn execute(&self, call: &ToolCall) -> ToolOutcome {
            let executor = GoalToolExecutor::new(
                &NoTools,
                Some(self.control.clone()),
                self.journal.clone(),
                self.session.clone(),
                self.sequence.clone(),
            );
            executor.execute(call).await
        }
    }
    #[derive(Debug)]
    struct NoTools;
    #[async_trait::async_trait]
    impl ToolExecutor for NoTools {
        async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
            ToolOutcome {
                ok: false,
                output: "no tools".into(),
                progress: ProgressSignals::default(),
            }
        }
    }

    let (journal, sequence, dir) = driver_config();
    journal_goal_begin(&journal, "g1");
    let sequence = Arc::new(sequence);
    let control = GoalControl::new("g1");
    let clock = TestClock::new(0);
    let budgets = GoalBudgets::default();
    let outcome = drive_goal(
        journal.clone(),
        &sequence,
        "s",
        "g1",
        "obj",
        &budgets,
        control.clone(),
        &clock,
        {
            let journal = journal.clone();
            let sequence = sequence.clone();
            let control = control.clone();
            move |_prompt| {
                let journal = journal.clone();
                let sequence = sequence.clone();
                let control = control.clone();
                async move {
                    // The model calls goal_complete during the turn.
                    let tools = CompletingTools {
                        journal,
                        sequence,
                        control,
                        session: "s".into(),
                    };
                    let result = tools
                        .execute(&ToolCall {
                            id: "c1".into(),
                            name: "goal_complete".into(),
                            arguments: serde_json::json!({"summary": "all done"}),
                        })
                        .await;
                    assert!(result.ok, "{:?}", result.output);
                    GoalTurnOutcome {
                        stop: GoalTurnStop::Complete,
                        usage: usage(1, 1),
                    }
                }
            }
        },
    )
    .await;
    assert_eq!(
        outcome,
        GoalDriveOutcome::Complete {
            summary: "all done".into()
        }
    );
    let ops = goal_ops_in(&dir);
    // Journal-first: GoalStatus{complete} AND GoalEnd{complete} are durable.
    let status_at = ops.iter().position(|op| {
        matches!(
            op,
            Op::GoalStatus {
                status: GoalStatusKind::Complete,
                ..
            }
        )
    });
    let end_at = ops.iter().position(|op| {
        matches!(
            op,
            Op::GoalEnd {
                outcome: GoalOutcome::Complete,
                ..
            }
        )
    });
    assert!(status_at.is_some() && end_at.is_some() && status_at < end_at);
    let state = SessionState::fold(&read_journal(&dir.join("s.jsonl")).unwrap().envelopes);
    assert_eq!(state.goal.map(|g| g.status), Some(GoalStatusKind::Complete));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Caps: objective > 4000 rejected; summary > 2000 rejected at the tool
/// boundary with NO journal writes (a rejected transition must not
/// fabricate an audit trail).
#[tokio::test]
async fn objective_and_summary_caps_are_enforced() {
    let long_objective = "x".repeat(4001);
    assert!(validate_objective(&long_objective).is_err());
    assert!(validate_objective("short").is_ok());
    assert!(validate_objective("   ").is_err());

    let (journal, sequence, dir) = driver_config();
    #[derive(Debug)]
    struct NoTools;
    #[async_trait::async_trait]
    impl ToolExecutor for NoTools {
        async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
            ToolOutcome {
                ok: false,
                output: "no tools".into(),
                progress: ProgressSignals::default(),
            }
        }
    }
    let control = GoalControl::new("g1");
    let executor = GoalToolExecutor::new(
        &NoTools,
        Some(control.clone()),
        journal,
        "s".into(),
        Arc::new(sequence),
    );
    let over = executor
        .execute(&ToolCall {
            id: "c1".into(),
            name: "goal_complete".into(),
            arguments: serde_json::json!({"summary": "y".repeat(2001)}),
        })
        .await;
    assert!(!over.ok);
    assert!(over.output.contains("2000"));
    assert!(control.completed_summary().is_none());
    let ops = goal_ops_in(&dir);
    assert!(
        !ops.iter()
            .any(|op| matches!(op, Op::GoalStatus { .. } | Op::GoalEnd { .. })),
        "a rejected summary journals nothing"
    );
    // Stray goal_complete with NO active goal: typed error, never a forged
    // transition.
    let (journal2, sequence2, dir2) = driver_config();
    let orphan = GoalToolExecutor::new(&NoTools, None, journal2, "s".into(), Arc::new(sequence2));
    let result = orphan
        .execute(&ToolCall {
            id: "c2".into(),
            name: "goal_complete".into(),
            arguments: serde_json::json!({"summary": "sneaky"}),
        })
        .await;
    assert!(!result.ok);
    assert!(result.output.contains("no active goal"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

/// Turn-level budget trip is disambiguated from goal-level trips (exit 1 vs
/// 3 at the exec seam).
#[tokio::test]
async fn turn_level_budget_trip_is_distinct_from_goal_level() {
    let (journal, sequence, dir) = driver_config();
    let clock = TestClock::new(0);
    let outcome = drive_goal(
        journal,
        &sequence,
        "s",
        "g1",
        "obj",
        &GoalBudgets::default(),
        GoalControl::new("g1"),
        &clock,
        |_prompt| async {
            GoalTurnOutcome {
                stop: GoalTurnStop::TurnBudget,
                usage: usage(1, 1),
            }
        },
    )
    .await;
    assert_eq!(outcome, GoalDriveOutcome::TurnBudgetTrip);
    // The goal's terminal record is journaled blocked(error) — the journal
    // tells the truth about the stop; the exit class stays the turn's.
    let ops = goal_ops_in(&dir);
    assert!(ops.iter().any(|op| matches!(
        op,
        Op::GoalStatus {
            status: GoalStatusKind::Blocked,
            reason: GoalReason::Error,
            ..
        }
    )));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancel mid-goal → paused (resumable), matching the replay normalization.
#[tokio::test]
async fn cancel_mid_goal_pauses() {
    let (journal, sequence, dir) = driver_config();
    journal_goal_begin(&journal, "g1");
    let clock = TestClock::new(0);
    let outcome = drive_goal(
        journal,
        &sequence,
        "s",
        "g1",
        "obj",
        &GoalBudgets::default(),
        GoalControl::new("g1"),
        &clock,
        |_prompt| async {
            GoalTurnOutcome {
                stop: GoalTurnStop::Cancelled,
                usage: usage(1, 1),
            }
        },
    )
    .await;
    assert_eq!(outcome, GoalDriveOutcome::Paused);
    let state = SessionState::fold(&read_journal(&dir.join("s.jsonl")).unwrap().envelopes);
    assert_eq!(state.goal.map(|g| g.status), Some(GoalStatusKind::Paused));
    let _ = std::fs::remove_dir_all(&dir);
}
