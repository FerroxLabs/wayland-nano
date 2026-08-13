//! P3 §3.3/§12 [r4 codex new-1]: the JournalCoordinator routing rules.
//! (i) EVERY live-session journal append routes through a JournalCoordinator
//! — a call-site/architecture test asserts no `JournalWriter::open` remains
//! in the session-runtime modules; (ii) a failed append INSIDE the
//! compaction critical section aborts the compaction (nothing published).

use nano_session::op::Op;
use nano_session::op::OpEnvelope;

/// The session-runtime modules whose journal appends must ALL route through
/// `JournalCoordinator` (the §3.3 rule-(i) call-site test). Excluded by
/// design: `doctor.rs` (offline journal REPAIR — it operates on corrupt
/// files where no coordinator can open), `session_cmds.rs` already routes
/// through a coordinator, the nano-session library internals (writer,
/// reader, fork — the coordinator's own implementation layer), and tests.
#[test]
fn no_direct_journal_writer_opens_in_session_runtime() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let agent = root.join("../nano-agent");
    let files = [
        root.join("src/acp_mode.rs"),
        root.join("src/session_tools.rs"),
        root.join("src/exec_run.rs"),
        root.join("src/exec_mode.rs"),
        root.join("src/cron_fire.rs"),
        root.join("src/host_mode.rs"),
        root.join("src/mcp_specs.rs"),
        agent.join("src/turn.rs"),
        agent.join("src/compact.rs"),
        agent.join("src/mcp.rs"),
        agent.join("src/mcp_session_tools.rs"),
        agent.join("src/elicitation.rs"),
        agent.join("src/tasks.rs"),
        agent.join("src/bootstrap.rs"),
        agent.join("src/goal.rs"),
    ];
    for file in &files {
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|err| panic!("{}: {err}", file.display()));
        assert!(
            !source.contains("JournalWriter::open"),
            "{} opens a JournalWriter directly — route the append through JournalCoordinator (P3 §3.3 rule i)",
            file.display()
        );
    }
}

/// §3.3 rule (iv) + poisoning: a panic inside the compaction critical
/// section poisons the coordinator's one mutex — every later append and
/// section entry fails typed (never a torn cut, never a silent continue),
/// while the on-disk journal stays valid and the session can continue
/// uncompacted under a fresh coordinator.
#[test]
fn failed_section_poisons_fail_closed_and_session_continues() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let coordinator = nano_session::JournalCoordinator::open(&path).unwrap();
    coordinator
        .append(&OpEnvelope::new(
            "1",
            "now",
            Op::SessionBegin {
                session_id: "s".into(),
                cwd: "/tmp".into(),
            },
        ))
        .unwrap();

    // A crash mid-section: the guard is held when the thread panics.
    let _ = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let _guard = coordinator.compaction().unwrap();
                panic!("simulated mid-compaction crash");
            })
            .join()
    });

    // Poisoned: no ordinary append and no new section succeeds — fail
    // closed, typed, nothing published.
    assert!(
        coordinator
            .append(&OpEnvelope::new(
                "2",
                "now",
                Op::ModeSet {
                    mode: "default".into(),
                },
            ))
            .is_err()
    );
    assert!(coordinator.compaction().is_err());

    // The journal on disk is untouched by the aborted section, and a fresh
    // coordinator continues the session uncompacted.
    let report = nano_session::read_journal(&path).unwrap();
    assert_eq!(report.envelopes.len(), 1);
    assert!(
        report
            .envelopes
            .iter()
            .all(|e| !matches!(e.op, Op::CompactionComplete { .. })),
        "nothing was published from the aborted section"
    );
    let fresh = nano_session::JournalCoordinator::open(&path).unwrap();
    fresh
        .append(&OpEnvelope::new(
            "2",
            "now",
            Op::ModeSet {
                mode: "default".into(),
            },
        ))
        .unwrap();
    assert_eq!(
        nano_session::read_journal(&path).unwrap().envelopes.len(),
        2
    );
}

/// §3.3 carry equivalence, end to end through the coordinator: hydrate →
/// compact (covered hydration op) → the journaled carry restores the exact
/// at-watermark state on replay, and a SECOND compaction feeds carry into
/// carry (carry(W) ≡ fold(replay input at W)).
#[test]
fn compaction_carry_round_trip_through_coordinator() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let coordinator = nano_session::JournalCoordinator::open(&path).unwrap();
    let digest = format!("{:064x}", 7);
    coordinator
        .append(&OpEnvelope::new(
            "1",
            "now",
            Op::SessionBegin {
                session_id: "s".into(),
                cwd: "/tmp".into(),
            },
        ))
        .unwrap();
    coordinator
        .append(&OpEnvelope::new(
            "2",
            "now",
            Op::McpToolHydration {
                hydration_id: "h1".into(),
                entries: vec![nano_session::HydrationEntry {
                    server_id: "fs".into(),
                    tool_names: vec!["read".into()],
                    tools_digest: digest.clone(),
                }],
            },
        ))
        .unwrap();

    // The compaction critical section: watermark + carry + durable Complete.
    {
        let mut guard = coordinator.compaction().unwrap();
        let snapshot = guard.snapshot().unwrap();
        let covers: Vec<String> = snapshot.iter().map(|e| e.id.clone()).collect();
        let carry = nano_session::hydration_carry_at(&snapshot);
        guard
            .append_complete(&OpEnvelope::new(
                "3",
                "now",
                Op::CompactionComplete {
                    compaction_id: "c1".into(),
                    summary: "s".into(),
                    covers_op_ids: covers,
                    changed_files: vec![],
                    image_influenced: false,
                    mcp_hydration: carry,
                },
            ))
            .unwrap();
    }

    // Replay from disk: the hydration state survives the covered prefix via
    // the carry, exactly once.
    let report = nano_session::read_journal(&path).unwrap();
    let replay_input = nano_session::compact::compacted_prefix(&report.envelopes);
    let state =
        nano_session::SessionState::fold(&replay_input.into_iter().cloned().collect::<Vec<_>>());
    assert_eq!(
        state.mcp_hydrated.get("fs").map(|s| s.len()),
        Some(1),
        "hydrated set restored from the carry"
    );
    assert_eq!(state.mcp_tools_digest.get("fs"), Some(&digest));
    assert_eq!(
        state.mcp_recent_digests.get("fs"),
        Some(&vec![digest.clone()]),
        "the churn window is carried"
    );
}
