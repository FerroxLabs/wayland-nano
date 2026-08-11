//! Kill-boundary and crash-window tests (the C1.3 evidence).

use crate::compact::NextTransition;
use crate::compact::actionably_equivalent;
use crate::compact::compacted_prefix;
use crate::compact::next_legal_transition;
use crate::op::Op;
use crate::op::OpEnvelope;
use crate::op::TurnOutcome;
use crate::reader::parse_journal_bytes;
use crate::replay::CompactionPhase;
use crate::replay::SessionState;
use crate::writer::JournalWriter;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

fn env(id: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(id, "2026-08-09T00:00:00Z", op)
}

fn session_ops() -> Vec<OpEnvelope> {
    vec![
        env(
            "1",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "C:\\repo".into(),
            },
        ),
        env(
            "2",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "fix the build".into(),
            },
        ),
        env(
            "3",
            Op::ToolCall {
                turn_id: "t1".into(),
                call_id: "c1".into(),
                name: "fs_edit".into(),
                args: serde_json::json!({"path": "main.rs"}),
            },
        ),
        env(
            "4",
            Op::ToolResult {
                call_id: "c1".into(),
                ok: true,
                output_digest: "d1".into(),
                changed_files: vec!["main.rs".into()],
            },
        ),
        env(
            "5",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: TurnOutcome::Completed,
            },
        ),
    ]
}

#[test]
fn append_and_replay_round_trip() {
    let state = SessionState::fold(&session_ops());
    assert_eq!(state.session_id.as_deref(), Some("s1"));
    assert!(state.open_turn.is_none());
    assert!(!state.turn_interrupted);
    assert!(state.changed_files.contains("main.rs"));
    assert_eq!(
        next_legal_transition(&state),
        NextTransition::AcceptUserInstruction
    );
}

#[test]
fn torn_tail_is_dropped_and_middle_stays_authoritative() {
    let mut bytes = Vec::new();
    for envelope in session_ops().into_iter().take(3) {
        bytes.extend(serde_json::to_vec(&envelope).unwrap());
        bytes.push(b'\n');
    }
    bytes.extend(b"{\"v\":1,\"id\":\"4\",\"ts\":\""); // crash mid-write
    let report = parse_journal_bytes(&bytes).unwrap();
    assert_eq!(report.envelopes.len(), 3);
    assert!(report.torn_tail_at.is_some());
}

#[test]
fn malformed_middle_line_is_an_integrity_error() {
    let mut bytes = serde_json::to_vec(&session_ops()[0]).unwrap();
    bytes.push(b'\n');
    bytes.extend(b"not json\n");
    bytes.extend(serde_json::to_vec(&session_ops()[1]).unwrap());
    bytes.push(b'\n');
    assert!(parse_journal_bytes(&bytes).is_err());
}

#[test]
fn unknown_ops_are_skipped_without_failing_replay() {
    let mut lines = serde_json::to_string(&session_ops()[0]).unwrap();
    lines.push('\n');
    lines.push_str(r#"{"v":1,"id":"x1","ts":"t","op":{"type":"future_feature","payload":42}}"#);
    lines.push('\n');
    lines.push_str(&serde_json::to_string(&session_ops()[1]).unwrap());
    lines.push('\n');
    let report = parse_journal_bytes(lines.as_bytes()).unwrap();
    assert_eq!(report.envelopes.len(), 3);
    let state = SessionState::fold(&report.envelopes);
    assert_eq!(state.open_turn.unwrap().turn_id, "t1");
    assert!(state.turn_interrupted);
}

#[test]
fn duplicate_ids_never_double_apply() {
    let mut ops = session_ops();
    let dup = env(
        "4",
        Op::ToolResult {
            call_id: "c1".into(),
            ok: true,
            output_digest: "d1".into(),
            changed_files: vec!["other.rs".into()],
        },
    );
    ops.insert(4, dup);
    let state = SessionState::fold(&ops);
    assert!(!state.changed_files.contains("other.rs"));
}

#[test]
fn stranded_compaction_running_resets_to_idle() {
    let mut ops = session_ops();
    ops.push(env(
        "6",
        Op::CompactionBegin {
            compaction_id: "k1".into(),
        },
    ));
    let state = SessionState::fold(&ops);
    assert_eq!(state.compaction, Some(CompactionPhase::Idle));
}

#[test]
fn crash_mid_turn_marks_interrupted_without_duplicate_effects() {
    let ops = &session_ops()[..4]; // cut before TurnEnd
    let state = SessionState::fold(ops);
    assert!(state.turn_interrupted);
    assert_eq!(state.open_turn.as_ref().unwrap().input, "fix the build");
    assert!(state.open_tool_calls.is_empty(), "c1 returned before crash");
    assert!(state.changed_files.contains("main.rs"));
    assert_eq!(
        next_legal_transition(&state),
        NextTransition::ResolveInterruptedTurn
    );
}

#[test]
fn open_tool_call_survives_to_resume_surface() {
    let ops = &session_ops()[..3]; // cut before ToolResult
    let state = SessionState::fold(ops);
    assert_eq!(state.open_tool_calls.len(), 1);
    assert_eq!(state.open_tool_calls[0].call_id, "c1");
    assert_eq!(
        next_legal_transition(&state),
        NextTransition::ResolveInterruptedTurn
    );
}

#[test]
fn compaction_replay_is_actionably_equivalent() {
    let full = session_ops();
    let mut compacted = full.clone();
    compacted.push(env(
        "6",
        Op::CompactionBegin {
            compaction_id: "k1".into(),
        },
    ));
    compacted.push(env(
        "7",
        Op::CompactionComplete {
            compaction_id: "k1".into(),
            summary: "turn t1 fixed main.rs".into(),
            covers_op_ids: vec!["2".into(), "3".into(), "4".into(), "5".into()],
            changed_files: vec!["main.rs".into()],
        },
    ));

    let full_state = SessionState::fold(&full);
    let prefix = compacted_prefix(&compacted);
    let compacted_state = SessionState::fold(&prefix.into_iter().cloned().collect::<Vec<_>>());

    assert!(actionably_equivalent(&full_state, &compacted_state));
    assert_eq!(
        compacted_state.last_compaction_summary.as_deref(),
        Some("turn t1 fixed main.rs")
    );
}

#[test]
fn writer_is_idempotent_across_reopen() {
    let dir = std::env::temp_dir().join(format!("nano-journal-{}", std::process::id()));
    let path = PathBuf::from(&dir).join("wire.jsonl");
    {
        let mut writer = JournalWriter::open(&path).unwrap();
        assert!(writer.append(&session_ops()[0]).unwrap());
        assert!(
            !writer.append(&session_ops()[0]).unwrap(),
            "duplicate id no-op"
        );
    }
    {
        let mut writer = JournalWriter::open(&path).unwrap();
        assert!(
            !writer.append(&session_ops()[0]).unwrap(),
            "id survives reopen"
        );
        assert!(writer.append(&session_ops()[1]).unwrap());
    }
    let report = crate::reader::read_journal(&path).unwrap();
    assert_eq!(report.envelopes.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nano-journal-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn journal_prefix(take: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for envelope in session_ops().into_iter().take(take) {
        bytes.extend(serde_json::to_vec(&envelope).unwrap());
        bytes.push(b'\n');
    }
    bytes
}

#[test]
fn writer_open_truncates_torn_tail_so_retry_stays_recoverable() {
    let dir = temp_dir("torn-retry");
    let path = dir.join("wire.jsonl");
    let mut bytes = journal_prefix(3);
    bytes.extend(b"{\"v\":1,\"id\":\"4\",\"ts\":\""); // crash mid-write of id "4"
    std::fs::write(&path, &bytes).unwrap();

    let mut writer = JournalWriter::open(&path).unwrap();
    assert!(
        writer.append(&session_ops()[3]).unwrap(),
        "torn record never committed, so the retry is a fresh append"
    );
    drop(writer);

    let report = crate::reader::read_journal(&path).unwrap();
    assert!(report.torn_tail_at.is_none(), "torn tail must be gone");
    assert_eq!(report.envelopes, session_ops()[..4]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writer_open_truncation_removes_only_torn_bytes() {
    let dir = temp_dir("torn-prefix");
    let path = dir.join("wire.jsonl");
    let prefix = journal_prefix(3);
    let mut bytes = prefix.clone();
    bytes.extend(b"{\"v\":1,\"id\":\"4\",\"ts\":\"");
    std::fs::write(&path, &bytes).unwrap();

    let writer = JournalWriter::open(&path).unwrap();
    drop(writer);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        prefix,
        "open must keep every complete record byte-for-byte"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writer_retry_of_committed_record_after_truncate_still_noops() {
    // id "4" committed fully; the crash tore the NEXT write (id "5"). After
    // truncation, retrying id "4" must dedupe and retrying id "5" must land.
    let dir = temp_dir("torn-dedupe");
    let path = dir.join("wire.jsonl");
    let mut bytes = journal_prefix(4);
    bytes.extend(b"{\"v\":1,\"id\":\"5\",\"ts\":\"");
    std::fs::write(&path, &bytes).unwrap();

    let mut writer = JournalWriter::open(&path).unwrap();
    assert!(
        !writer.append(&session_ops()[3]).unwrap(),
        "committed record must not double-append"
    );
    assert!(
        writer.append(&session_ops()[4]).unwrap(),
        "torn record retry must append"
    );
    drop(writer);

    let report = crate::reader::read_journal(&path).unwrap();
    assert_eq!(report.envelopes, session_ops());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writer_never_truncates_integrity_broken_middle() {
    let dir = temp_dir("middle-corrupt");
    let path = dir.join("wire.jsonl");
    let mut bytes = journal_prefix(2);
    bytes.extend(b"not json\n");
    bytes.extend(serde_json::to_vec(&session_ops()[2]).unwrap());
    bytes.push(b'\n');
    std::fs::write(&path, &bytes).unwrap();

    assert!(crate::reader::read_journal(&path).is_err());
    let mut writer = JournalWriter::open(&path).unwrap();
    writer.append(&session_ops()[3]).unwrap();
    drop(writer);

    let after = std::fs::read(&path).unwrap();
    assert!(
        after.starts_with(&bytes),
        "an integrity-broken journal must never lose a byte"
    );
    assert!(
        crate::reader::read_journal(&path).is_err(),
        "restore must stay fail-loud"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
