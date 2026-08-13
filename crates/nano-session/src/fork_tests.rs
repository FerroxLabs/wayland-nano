//! C11 §7 fork fidelity + imported-prefix replay-namespace tests.

use crate::fork::ForkError;
use crate::fork::ForkPoint;
use crate::fork::fork_journal;
use crate::op::GoalBudgets;
use crate::op::GoalOutcome;
use crate::op::GoalReason;
use crate::op::GoalStatusKind;
use crate::op::Op;
use crate::op::OpEnvelope;
use crate::op::TurnOutcome;
use crate::reader::read_journal;
use crate::replay::GoalLive;
use crate::replay::ReplayError;
use crate::replay::SessionState;
use crate::writer::JournalWriter;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "nano-c11-fork-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn env(id: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(id, "2026-08-12T00:00:00Z", op)
}

fn begin(id: &str, session: &str) -> OpEnvelope {
    env(
        id,
        Op::SessionBegin {
            session_id: session.into(),
            cwd: "C:\\repo".into(),
        },
    )
}

fn turn(id_prefix: &str, turn_id: &str, input: &str) -> Vec<OpEnvelope> {
    vec![
        env(
            &format!("{id_prefix}-tb"),
            Op::TurnBegin {
                turn_id: turn_id.into(),
                input: input.into(),
                input_blocks: Vec::new(),
            },
        ),
        env(
            &format!("{id_prefix}-at"),
            Op::AssistantText {
                turn_id: turn_id.into(),
                text: format!("reply to {input}"),
            },
        ),
        env(
            &format!("{id_prefix}-te"),
            Op::TurnEnd {
                turn_id: turn_id.into(),
                outcome: TurnOutcome::Completed,
                usage: None,
            },
        ),
    ]
}

fn goal_ops(id_prefix: &str, goal_id: &str) -> Vec<OpEnvelope> {
    vec![
        env(
            &format!("{id_prefix}-gb"),
            Op::GoalBegin {
                goal_id: goal_id.into(),
                objective: "ship the thing".into(),
                budgets: GoalBudgets {
                    token_budget: Some(1000),
                    turn_budget: None,
                    wall_clock_budget_ms: None,
                },
            },
        ),
        env(
            &format!("{id_prefix}-gs"),
            Op::GoalStatus {
                goal_id: goal_id.into(),
                status: GoalStatusKind::Active,
                reason: GoalReason::Unspecified,
            },
        ),
    ]
}

fn write_parent(dir: &std::path::Path, name: &str, envelopes: &[OpEnvelope]) -> PathBuf {
    let path = dir.join(name);
    let mut writer = JournalWriter::open(&path).unwrap();
    for envelope in envelopes {
        writer.append(envelope).unwrap();
    }
    writer.sync().unwrap();
    path
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The core fidelity proof: parent digest before == after, child prefix is
/// byte-verbatim, child loads with the CHILD id, and the imported parent
/// SessionBegin is inert.
#[test]
fn fork_at_end_byte_identical_parent_and_child_identity() {
    let dir = tmpdir("end");
    let mut envelopes = vec![begin("p-begin-1", "parent")];
    envelopes.extend(turn("p-1", "parent-turn-1", "first"));
    envelopes.extend(turn("p-2", "parent-turn-2", "second"));
    let parent = write_parent(&dir, "parent.jsonl", &envelopes);
    let parent_bytes_before = std::fs::read(&parent).unwrap();

    let child = dir.join("child.jsonl");
    let outcome = fork_journal(&parent, &child, "child", &ForkPoint::End).unwrap();

    assert_eq!(outcome.parent_digest_before, outcome.parent_digest_after);
    assert_eq!(
        outcome.parent_digest_before,
        sha256_hex(&parent_bytes_before)
    );
    assert_eq!(
        std::fs::read(&parent).unwrap(),
        parent_bytes_before,
        "parent bytes untouched"
    );
    assert_eq!(outcome.imported_ops, envelopes.len() as u64);
    assert!(!outcome.closed_parent_goal);

    // Child regions: genesis at position 0, lineage, then BYTE-VERBATIM prefix.
    let child_bytes = std::fs::read(&child).unwrap();
    let child_text = String::from_utf8(child_bytes).unwrap();
    let mut lines = child_text.lines();
    let genesis: OpEnvelope = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert!(matches!(&genesis.op, Op::SessionBegin { session_id, .. } if session_id == "child"));
    let lineage: OpEnvelope = serde_json::from_str(lines.next().unwrap()).unwrap();
    match &lineage.op {
        Op::ForkedFrom {
            parent_session_id,
            imported_ops,
            parent_digest_before,
            parent_digest_after,
            ..
        } => {
            assert_eq!(parent_session_id, "parent");
            assert_eq!(*imported_ops, envelopes.len() as u64);
            assert_eq!(parent_digest_before, parent_digest_after);
        }
        other => panic!("expected ForkedFrom, got {other:?}"),
    }
    let imported_text: String = lines.collect::<Vec<_>>().join("\n") + "\n";
    assert_eq!(
        imported_text,
        String::from_utf8(parent_bytes_before).unwrap(),
        "imported prefix is byte-verbatim"
    );

    // Replay: identity from genesis ONLY (imported parent begin is inert).
    let report = read_journal(&child).unwrap();
    let state = SessionState::fold_strict(&report.envelopes).unwrap();
    assert_eq!(state.session_id.as_deref(), Some("child"));
    assert!(state.integrity_error.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Fork-at-turn: child replay == parent replay truncated at that turn's
/// TurnEnd, and the fork is an exact boundary.
#[test]
fn fork_at_turn_truncates_at_turn_end() {
    let dir = tmpdir("turn");
    let mut envelopes = vec![begin("p-begin-1", "parent")];
    envelopes.extend(turn("p-1", "parent-turn-1", "first"));
    envelopes.extend(turn("p-2", "parent-turn-2", "second"));
    let parent = write_parent(&dir, "parent.jsonl", &envelopes);

    let child = dir.join("child.jsonl");
    let outcome = fork_journal(
        &parent,
        &child,
        "child",
        &ForkPoint::Turn("parent-turn-1".into()),
    )
    .unwrap();
    assert_eq!(outcome.imported_ops, 4); // begin + turn-1's 3 envelopes
    assert_eq!(outcome.parent_op_id, "p-1-te");
    match read_journal(&child)
        .unwrap()
        .envelopes
        .get(1)
        .map(|e| &e.op)
    {
        Some(Op::ForkedFrom { at_turn, .. }) => {
            assert_eq!(at_turn.as_deref(), Some("parent-turn-1"))
        }
        other => panic!("expected ForkedFrom, got {other:?}"),
    }
    // The imported region folds to exactly the parent's state at that point.
    let state = SessionState::fold_strict(&read_journal(&child).unwrap().envelopes).unwrap();
    let parent_prefix = SessionState::fold(&envelopes[..4]);
    assert_eq!(state.changed_files, parent_prefix.changed_files);
    assert!(state.open_turn.is_none());

    // Unknown turn / crashed turn are typed errors, never silent truncation.
    let crashed = vec![
        begin("q-begin-1", "q"),
        env(
            "q-tb",
            Op::TurnBegin {
                turn_id: "q-turn-1".into(),
                input: "died mid-turn".into(),
                input_blocks: Vec::new(),
            },
        ),
    ];
    let crashed_parent = write_parent(&dir, "crashed.jsonl", &crashed);
    let err = fork_journal(
        &crashed_parent,
        &dir.join("c2.jsonl"),
        "c2",
        &ForkPoint::Turn("q-turn-1".into()),
    )
    .unwrap_err();
    assert!(
        matches!(err, ForkError::CrashedTurn { ref turn_id } if turn_id == "q-turn-1"),
        "{err:?}"
    );
    let err = fork_journal(
        &parent,
        &dir.join("c3.jsonl"),
        "c3",
        &ForkPoint::Turn("no-such-turn".into()),
    )
    .unwrap_err();
    assert!(matches!(err, ForkError::TurnNotFound { .. }), "{err:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Busy parent (lock held by another writer) → typed busy, no child written.
#[test]
fn fork_under_held_lock_is_typed_busy() {
    let dir = tmpdir("busy");
    let parent = write_parent(&dir, "parent.jsonl", &[begin("p-begin-1", "parent")]);
    let _held = crate::lock::FileLock::try_acquire(&parent).unwrap();
    let err =
        fork_journal(&parent, &dir.join("child.jsonl"), "child", &ForkPoint::End).unwrap_err();
    assert!(matches!(err, ForkError::Busy), "{err:?}");
    assert!(!dir.join("child.jsonl").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fork of a session with an ACTIVE goal: the child's live goal state is
/// EMPTY at EVERY fold step (asserted per-step), imported goal ops stay
/// reader-visible in the audit namespace, the close-out ops reference the
/// parent goal and fold as inert no-ops, and the parent's goal is intact.
#[test]
fn fork_with_active_goal_suppresses_control_ops_at_every_step() {
    let dir = tmpdir("goal");
    let mut envelopes = vec![begin("p-begin-1", "parent")];
    envelopes.extend(goal_ops("p-g", "goal-parent"));
    envelopes.extend(turn("p-1", "parent-turn-1", "work"));
    let parent = write_parent(&dir, "parent.jsonl", &envelopes);

    let child = dir.join("child.jsonl");
    let outcome = fork_journal(&parent, &child, "child", &ForkPoint::End).unwrap();
    assert!(outcome.closed_parent_goal);

    // Parent goal intact: parent journal unchanged, still folds to the goal.
    let parent_state = SessionState::fold(&read_journal(&parent).unwrap().envelopes);
    assert_eq!(
        parent_state.goal.as_ref().map(|g| g.goal_id.as_str()),
        Some("goal-parent")
    );

    let report = read_journal(&child).unwrap();
    // Per-step assertion: at NO fold step does the child hold live goal
    // state for the parent's goal.
    let mut stepper = SessionState::new();
    for (index, envelope) in report.envelopes.iter().enumerate() {
        stepper.apply(envelope);
        assert!(
            stepper.goal.is_none(),
            "step {index}: live goal must stay empty (op: {:?})",
            envelope.op
        );
    }
    let state = SessionState::fold_strict(&report.envelopes).unwrap();
    assert!(state.goal.is_none());

    // Audit namespace: the imported GoalBegin/GoalStatus AND the close-out
    // GoalStatus/GoalEnd (parent goal id — inert no-ops) are all visible.
    let suppressed: Vec<&Op> = state.suppressed_control_ops.iter().map(|e| &e.op).collect();
    assert!(
        suppressed
            .iter()
            .any(|op| matches!(op, Op::GoalBegin { goal_id, .. } if goal_id == "goal-parent")),
        "imported GoalBegin visible in audit namespace: {suppressed:?}"
    );
    assert!(
        suppressed
            .iter()
            .any(|op| matches!(op, Op::GoalEnd { goal_id, .. } if goal_id == "goal-parent")),
        "close-out GoalEnd visible in audit namespace: {suppressed:?}"
    );
    // The close-out ops exist in the raw journal (the durable record).
    assert!(report.envelopes.iter().any(|e| matches!(
        &e.op,
        Op::GoalStatus { goal_id, status: GoalStatusKind::Blocked, reason: GoalReason::Cancelled }
            if goal_id == "goal-parent"
    )));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A declared imported_ops count that overruns the stream is a typed replay
/// error (fail-closed), never a silent short fold.
#[test]
fn imported_region_overrun_is_typed_replay_error() {
    let envelopes = vec![
        begin("c-begin-1", "child"),
        env(
            "c-fork-1",
            Op::ForkedFrom {
                parent_session_id: "parent".into(),
                parent_op_id: "p-9".into(),
                at_turn: None,
                parent_digest_before: "a".into(),
                parent_digest_after: "a".into(),
                imported_ops: 5,
            },
        ),
        env(
            "p-1",
            Op::TurnBegin {
                turn_id: "t".into(),
                input: "x".into(),
                input_blocks: Vec::new(),
            },
        ),
    ];
    let state = SessionState::fold(&envelopes);
    assert_eq!(
        state.integrity_error,
        Some(ReplayError::ImportedRegionOverrun {
            declared: 5,
            actual: 1
        })
    );
    let err = SessionState::fold_strict(&envelopes).unwrap_err();
    assert!(matches!(err, ReplayError::ImportedRegionOverrun { .. }));
}

/// Op-id namespacing: child-authored op ids never collide with imported
/// parent op ids (the child namespace is the child id).
#[test]
fn child_authored_op_ids_never_collide_with_imported() {
    let dir = tmpdir("ids");
    let mut envelopes = vec![begin("p-begin-1", "parent")];
    envelopes.extend(turn("p-1", "parent-turn-1", "work"));
    let parent = write_parent(&dir, "parent.jsonl", &envelopes);
    let child = dir.join("child.jsonl");
    fork_journal(&parent, &child, "child", &ForkPoint::End).unwrap();

    // The child appends its own ops under its own namespace.
    let mut writer = JournalWriter::open(&child).unwrap();
    writer
        .append(&env(
            "child-turn-1-tb",
            Op::TurnBegin {
                turn_id: "child-turn-1".into(),
                input: "new work".into(),
                input_blocks: Vec::new(),
            },
        ))
        .unwrap();
    writer
        .append(&env(
            "child-turn-1-te",
            Op::TurnEnd {
                turn_id: "child-turn-1".into(),
                outcome: TurnOutcome::Completed,
                usage: None,
            },
        ))
        .unwrap();

    let report = read_journal(&child).unwrap();
    let mut seen = std::collections::HashSet::new();
    for envelope in &report.envelopes {
        assert!(seen.insert(&envelope.id), "duplicate id: {}", envelope.id);
    }
    let state = SessionState::fold_strict(&report.envelopes).unwrap();
    assert_eq!(state.session_id.as_deref(), Some("child"));
    assert!(state.open_turn.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Old-reader tolerance: ForkedFrom/goal/cron ops deserialize as `Unknown`
/// when the variants are unknown — pinned here by asserting the NEW ops
/// round-trip through the current schema and that `Unknown` skips fold
/// (the forward-compat budget is the variant tag only).
#[test]
fn new_ops_round_trip_and_unknown_skips() {
    let ops = vec![
        Op::ForkedFrom {
            parent_session_id: "p".into(),
            parent_op_id: "p-1".into(),
            at_turn: None,
            parent_digest_before: "a".into(),
            parent_digest_after: "a".into(),
            imported_ops: 0,
        },
        Op::GoalBegin {
            goal_id: "g".into(),
            objective: "o".into(),
            budgets: GoalBudgets::default(),
        },
        Op::GoalStatus {
            goal_id: "g".into(),
            status: GoalStatusKind::Blocked,
            reason: GoalReason::BudgetTokens,
        },
        Op::GoalEnd {
            goal_id: "g".into(),
            outcome: GoalOutcome::Blocked,
        },
        Op::CronFired {
            job_id: "j".into(),
            session_id: "s".into(),
            turn_id: "t".into(),
            occurrence_id: "j:2026-08-12T10:00:00Z".into(),
            mode_at_fire: "default".into(),
            coalesced: 3,
        },
    ];
    for op in ops {
        let envelope = env("x-1", op.clone());
        let json = serde_json::to_string(&envelope).unwrap();
        let back: OpEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.op, op);
    }
    // An unknown variant tag folds as a skip, never a failure.
    let unknown: OpEnvelope =
        serde_json::from_str(r#"{"v":1,"id":"u-1","ts":"t","op":{"type":"future_op"}}"#).unwrap();
    assert!(matches!(unknown.op, Op::Unknown));
    let state = SessionState::fold(&[unknown]);
    assert!(state.session_id.is_none());
}

/// Replay suppression of imported cron ops: imported CronFired never touches
/// child scheduler state (occurrence set / last_fired) but stays visible.
#[test]
fn imported_cron_fired_never_touches_child_scheduler_state() {
    let envelopes = vec![
        begin("c-begin-1", "child"),
        env(
            "c-fork-1",
            Op::ForkedFrom {
                parent_session_id: "parent".into(),
                parent_op_id: "p-2".into(),
                at_turn: None,
                parent_digest_before: "a".into(),
                parent_digest_after: "a".into(),
                imported_ops: 2,
            },
        ),
        env(
            "p-1",
            Op::CronFired {
                job_id: "job".into(),
                session_id: "parent".into(),
                turn_id: "parent-turn-1".into(),
                occurrence_id: "job:2026-08-12T09:00:00Z".into(),
                mode_at_fire: "default".into(),
                coalesced: 0,
            },
        ),
        env(
            "p-2",
            Op::TurnBegin {
                turn_id: "parent-turn-1".into(),
                input: "scheduled".into(),
                input_blocks: Vec::new(),
            },
        ),
        // Child-authored CronFired AFTER the imported region: folds live.
        env(
            "c-cron-1",
            Op::CronFired {
                job_id: "job".into(),
                session_id: "child".into(),
                turn_id: "child-turn-1".into(),
                occurrence_id: "job:2026-08-12T10:00:00Z".into(),
                mode_at_fire: "default".into(),
                coalesced: 0,
            },
        ),
    ];
    let state = SessionState::fold_strict(&envelopes).unwrap();
    assert!(
        !state
            .cron_fired_occurrences
            .contains("job:2026-08-12T09:00:00Z"),
        "imported occurrence suppressed from live scheduler state"
    );
    assert!(
        state
            .cron_fired_occurrences
            .contains("job:2026-08-12T10:00:00Z")
    );
    assert_eq!(
        state.cron_last_fired.get("job").map(String::as_str),
        Some("2026-08-12T10:00:00Z")
    );
    assert!(
        state
            .suppressed_control_ops
            .iter()
            .any(|e| matches!(&e.op, Op::CronFired { occurrence_id, .. } if occurrence_id == "job:2026-08-12T09:00:00Z")),
        "imported CronFired stays reader-visible in the audit namespace"
    );
}

/// Goal replay: active normalizes to paused at the tail (kill mid-goal →
/// paused on load); a fork-of-a-goal-less session replays unchanged.
#[test]
fn goal_active_normalizes_to_paused_and_goal_less_fork_is_clean() {
    let mut envelopes = vec![begin("p-begin-1", "parent")];
    envelopes.extend(goal_ops("p-g", "goal-1"));
    let state = SessionState::fold(&envelopes);
    assert_eq!(
        state.goal,
        Some(GoalLive {
            goal_id: "goal-1".into(),
            status: GoalStatusKind::Paused,
            reason: GoalReason::Unspecified,
            objective: "ship the thing".into(),
            budgets: GoalBudgets {
                token_budget: Some(1000),
                turn_budget: None,
                wall_clock_budget_ms: None,
            },
        })
    );

    // Goal-less fork: no close-out ops, no suppressed control ops.
    let dir = tmpdir("goalless");
    let parent = write_parent(&dir, "p.jsonl", &[begin("p-begin-1", "parent")]);
    let child = dir.join("c.jsonl");
    let outcome = fork_journal(&parent, &child, "child", &ForkPoint::End).unwrap();
    assert!(!outcome.closed_parent_goal);
    let state = SessionState::fold_strict(&read_journal(&child).unwrap().envelopes).unwrap();
    assert!(state.goal.is_none());
    assert!(state.suppressed_control_ops.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fork of a COMPACTED journal (C1 §6 scoping): compaction ops cross the
/// fork unsuppressed (they are context-class ops) and the child replay
/// matches the parent's post-compaction state over the compacted prefix.
#[test]
fn fork_of_compacted_journal_replays_identically() {
    let dir = tmpdir("compacted");
    let mut envelopes = vec![begin("p-begin-1", "parent")];
    envelopes.extend(turn("p-1", "parent-turn-1", "first"));
    envelopes.push(env(
        "p-cb",
        Op::CompactionBegin {
            compaction_id: "c1".into(),
        },
    ));
    envelopes.push(env(
        "p-cc",
        Op::CompactionComplete {
            compaction_id: "c1".into(),
            summary: "summary of the prefix".into(),
            covers_op_ids: vec!["p-begin-1".into(), "p-1-tb".into()],
            changed_files: vec!["main.rs".into()],
            image_influenced: false,
        },
    ));
    envelopes.extend(turn("p-2", "parent-turn-2", "second"));
    let parent = write_parent(&dir, "parent.jsonl", &envelopes);

    let child = dir.join("child.jsonl");
    fork_journal(&parent, &child, "child", &ForkPoint::End).unwrap();

    let child_state = SessionState::fold_strict(&read_journal(&child).unwrap().envelopes).unwrap();
    let parent_state = SessionState::fold(&envelopes);
    // Compaction replay equivalence across the fork: same summary, same
    // durable effects, compaction phase idle — and NOT suppressed (the
    // audit namespace must not contain the context-class ops).
    assert_eq!(
        child_state.last_compaction_summary,
        parent_state.last_compaction_summary
    );
    assert_eq!(child_state.changed_files, parent_state.changed_files);
    assert!(child_state.changed_files.contains("main.rs"));
    assert!(
        !child_state.suppressed_control_ops.iter().any(|e| matches!(
            &e.op,
            Op::CompactionBegin { .. } | Op::CompactionComplete { .. }
        )),
        "compaction ops are context-class: they fold, never suppress"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
