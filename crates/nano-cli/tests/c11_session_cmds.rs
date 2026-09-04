//! C11: `session fork` / `goal …` command-core tests — the shared library
//! path the CLI mirrors AND the ACP `_wayland/*` adapters both call.

use nano_cli::session_cmds::{
    goal_set_core, goal_status_core, goal_transition_core, session_fork_core,
    session_fork_core_with_binding,
};
use nano_session::op::Op;
use nano_session::writer::JournalWriter;
use nano_session::{GoalBudgets, OpEnvelope, SessionState};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "nano-c11-cmds-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_session(sessions: &std::path::Path, id: &str) {
    let mut writer = JournalWriter::open(&sessions.join(format!("{id}.jsonl"))).unwrap();
    writer
        .append(&OpEnvelope::new(
            format!("{id}-begin-1"),
            "now",
            Op::SessionBegin {
                session_id: id.into(),
                cwd: "C:\\repo".into(),
            },
        ))
        .unwrap();
    writer
        .append(&OpEnvelope::new(
            format!("{id}-turn-1-1"),
            "now",
            Op::TurnBegin {
                turn_id: format!("{id}-turn-1"),
                input: "work".into(),
                input_blocks: Vec::new(),
            },
        ))
        .unwrap();
    writer
        .append(&OpEnvelope::new(
            format!("{id}-turn-1-2"),
            "now",
            Op::TurnEnd {
                turn_id: format!("{id}-turn-1"),
                outcome: nano_session::TurnOutcome::Completed,
                usage: None,
            },
        ))
        .unwrap();
    writer.sync().unwrap();
}

#[test]
fn session_fork_core_returns_digests_and_loadable_child() {
    let dir = tmpdir("fork");
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    make_session(&sessions, "s1");

    let result = session_fork_core(&sessions, "s1", None, false).unwrap();
    assert_eq!(
        result["parent_digest_before"], result["parent_digest_after"],
        "byte-identical parent proof"
    );
    let child_id = result["child_session_id"].as_str().unwrap().to_string();
    // The child loads as an ordinary session with its own identity.
    let report = nano_session::read_journal(&sessions.join(format!("{child_id}.jsonl"))).unwrap();
    let state = SessionState::fold_strict(&report.envelopes).unwrap();
    assert_eq!(state.session_id.as_deref(), Some(child_id.as_str()));

    // Fork-at-turn works; unknown turn is a typed error.
    let at = session_fork_core(&sessions, "s1", Some("s1-turn-1".into()), false).unwrap();
    assert_eq!(at["imported_ops"], 3); // begin + TurnBegin + TurnEnd
    assert!(session_fork_core(&sessions, "s1", Some("nope".into()), false).is_err());
    assert!(session_fork_core(&sessions, "no-such", None, false).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_child_binding_removes_the_fork_journal() {
    let dir = tmpdir("fork-bind-fail");
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    make_session(&sessions, "s1");

    let error = session_fork_core_with_binding(&sessions, "s1", None, false, |_| {
        Err("injected child-binding append failure".to_string())
    })
    .unwrap_err();
    assert!(error.contains("injected child-binding append failure"));
    let journals = std::fs::read_dir(&sessions)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    assert_eq!(journals.len(), 1, "failed binding must leave no child journal");
    assert_eq!(journals[0].file_name().to_string_lossy(), "s1.jsonl");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn goal_lifecycle_core_round_trip() {
    let dir = tmpdir("goal");
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    make_session(&sessions, "s1");

    let set = goal_set_core(&sessions, "s1", "ship it", &GoalBudgets::default()).unwrap();
    let goal_id = set["goal_id"].as_str().unwrap().to_string();
    assert_eq!(set["status"], "active");

    // A second goal while one is non-terminal: typed error (one current
    // goal per session).
    assert!(goal_set_core(&sessions, "s1", "another", &GoalBudgets::default()).is_err());

    // Status reads the folded state — with the replay normalization
    // (kill-resume rule): a goal whose last state is `active` reads
    // `paused` on any fresh load, never silently resumable.
    let status = goal_status_core(&sessions, "s1").unwrap();
    assert_eq!(status["goal_id"], goal_id);
    assert_eq!(status["status"], "paused");

    // Pause → resume → cancel.
    assert_eq!(
        goal_transition_core(&sessions, "s1", "pause").unwrap()["status"],
        "paused"
    );
    assert_eq!(
        goal_transition_core(&sessions, "s1", "resume").unwrap()["status"],
        "active"
    );
    assert_eq!(
        goal_transition_core(&sessions, "s1", "cancel").unwrap()["status"],
        "blocked"
    );
    // Terminal now: further transitions and new statuses are typed errors /
    // terminal state.
    assert!(goal_transition_core(&sessions, "s1", "pause").is_err());
    let status = goal_status_core(&sessions, "s1").unwrap();
    assert_eq!(status["status"], "blocked");

    // A fresh goal can begin after the terminal one.
    assert!(goal_set_core(&sessions, "s1", "second wind", &GoalBudgets::default()).is_ok());

    // Objective cap: 4001 chars rejected.
    let long = "x".repeat(4001);
    assert!(goal_set_core(&sessions, "s1", &long, &GoalBudgets::default()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
