//! S9 CUA journal tests (design S9-BROWSER-CUA-DESIGN.md §4): the
//! `CuaAction`/`CuaResult` op pair, the §4.2 ambiguous-tail replay rule, the
//! digest-only invariant, and the additive-variant discipline (pre-S9
//! journals replay byte-identical; `SCHEMA_VERSION` stays 1).

use crate::error_kind::NanoErrorKind;
use crate::op::{CuaOutcome, Op, OpEnvelope, SCHEMA_VERSION, TurnOutcome};
use crate::replay::SessionState;

fn env(id: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(id, "now", op)
}

fn cua_action(call_id: &str) -> Op {
    Op::CuaAction {
        turn_id: "t1".into(),
        call_id: call_id.into(),
        op_kind: "left_click".into(),
        args_digest: "a".repeat(64),
        frontmost_app: Some("notepad.exe".into()),
        pre_shot: Some("b".repeat(64)),
    }
}

fn cua_result(call_id: &str, outcome: CuaOutcome) -> Op {
    Op::CuaResult {
        call_id: call_id.into(),
        outcome,
        post_shot: Some("c".repeat(64)),
        error_kind: None,
    }
}

#[test]
fn cua_ops_round_trip_and_stay_byte_minimal() {
    let action = cua_action("c1");
    let json = serde_json::to_string(&action).expect("serialize");
    assert!(json.contains(r#""type":"cua_action""#), "{json}");
    assert_eq!(serde_json::from_str::<Op>(&json).expect("parse"), action);

    // Optional fields omitted when None (new journals stay byte-minimal) and
    // defaulted on parse (forward tolerance for leaner frames).
    let bare = Op::CuaAction {
        turn_id: "t1".into(),
        call_id: "c1".into(),
        op_kind: "type".into(),
        args_digest: "d".repeat(64),
        frontmost_app: None,
        pre_shot: None,
    };
    let json = serde_json::to_string(&bare).expect("serialize");
    assert!(!json.contains("frontmost_app"), "{json}");
    assert!(!json.contains("pre_shot"), "{json}");
    assert_eq!(serde_json::from_str::<Op>(&json).expect("parse"), bare);

    let result = cua_result("c1", CuaOutcome::Completed);
    let json = serde_json::to_string(&result).expect("serialize");
    assert!(json.contains(r#""type":"cua_result""#), "{json}");
    assert!(json.contains(r#""outcome":"completed""#), "{json}");
    assert!(!json.contains("error_kind"), "{json}");
    assert_eq!(serde_json::from_str::<Op>(&json).expect("parse"), result);

    // A denied result carries the closed-vocabulary kind, never raw text.
    let denied = Op::CuaResult {
        call_id: "c1".into(),
        outcome: CuaOutcome::Denied,
        post_shot: None,
        error_kind: Some(NanoErrorKind::CuaPolicyDenied),
    };
    let json = serde_json::to_string(&denied).expect("serialize");
    assert!(
        json.contains(r#""error_kind":"cua_policy_denied""#),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Op>(&json).expect("parse"), denied);
}

#[test]
fn cua_outcome_is_forward_tolerant() {
    for (wire, expected) in [
        ("completed", CuaOutcome::Completed),
        ("denied", CuaOutcome::Denied),
        ("cancelled", CuaOutcome::Cancelled),
        ("failed", CuaOutcome::Failed),
    ] {
        let op: Op = serde_json::from_str(&format!(
            r#"{{"type":"cua_result","call_id":"c","outcome":"{wire}"}}"#
        ))
        .expect("parse");
        let Op::CuaResult { outcome, .. } = op else {
            panic!("wrong op")
        };
        assert_eq!(outcome, expected);
    }
    // An outcome from a NEWER build folds to Unknown, never fails the read.
    let op: Op =
        serde_json::from_str(r#"{"type":"cua_result","call_id":"c","outcome":"teleported"}"#)
            .expect("forward tolerance");
    let Op::CuaResult { outcome, .. } = op else {
        panic!("wrong op")
    };
    assert_eq!(outcome, CuaOutcome::Unknown);
}

/// Digest-only invariant scan (S9 §4.1): no serialized CUA op carries raw
/// coordinates, typed text, key spellings, or image bytes — digests and
/// bounded ids/enums only.
#[test]
fn cua_ops_carry_no_raw_payload_fields() {
    for op in [
        cua_action("c1"),
        cua_result("c1", CuaOutcome::Failed),
        Op::CuaResult {
            call_id: "c1".into(),
            outcome: CuaOutcome::Denied,
            post_shot: None,
            error_kind: Some(NanoErrorKind::CuaFocusLost),
        },
    ] {
        let value = serde_json::to_value(&op).expect("serialize");
        let object = value.as_object().expect("op is an object");
        for forbidden in [
            "x", "y", "dx", "dy", "text", "keys", "args", "data", "data_b64", "bytes", "image",
        ] {
            assert!(
                !object.contains_key(forbidden),
                "journal op must not carry raw payload field {forbidden:?}: {object:?}"
            );
        }
    }
}

/// Pre-S9 journals replay byte-identical (the additive-variant discipline,
/// p3_tests precedent): pre-S9 op lines parse, RE-SERIALIZE byte-identically,
/// and fold with NO CUA state.
#[test]
fn pre_s9_journal_lines_round_trip_byte_identical() {
    let lines = [
        r#"{"type":"session_begin","session_id":"s1","cwd":"/w"}"#,
        r#"{"type":"turn_begin","turn_id":"t1","input":"hi"}"#,
        r#"{"type":"tool_call","turn_id":"t1","call_id":"c1","name":"shell","args":{"command":"ls"}}"#,
        r#"{"type":"tool_result","call_id":"c1","ok":true,"output_digest":"len:2","changed_files":[]}"#,
        r#"{"type":"turn_end","turn_id":"t1","outcome":"completed"}"#,
    ];
    let mut envelopes = Vec::new();
    for line in lines {
        let op: Op = serde_json::from_str(line).expect("pre-S9 op parses");
        assert_eq!(
            serde_json::to_string(&op).expect("reserialize"),
            line,
            "pre-S9 op reserializes byte-identically"
        );
        envelopes.push(env(&format!("e{}", envelopes.len()), op));
    }
    let state = SessionState::fold(&envelopes);
    assert!(state.interrupted_cua.is_empty());
    assert!(!state.turn_interrupted);
    assert!(state.open_turn.is_none());
}

/// §4.2: a CuaAction WITHOUT its paired CuaResult at the journal tail (a
/// kill between the action append and the dispatch return) is ambiguous —
/// replay marks it interrupted. A paired action is replay-neutral history.
#[test]
fn unpaired_cua_action_marks_the_tail_interrupted() {
    let mut envelopes = vec![
        env(
            "e0",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "/w".into(),
            },
        ),
        env(
            "e1",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "click the button".into(),
                input_blocks: vec![],
            },
        ),
        env("e2", cua_action("c1")),
        // Kill here: no CuaResult, no TurnEnd.
    ];
    let state = SessionState::fold(&envelopes);
    assert!(state.turn_interrupted, "stranded turn is interrupted");
    assert_eq!(state.interrupted_cua.len(), 1);
    assert_eq!(state.interrupted_cua[0].call_id, "c1");
    assert_eq!(state.interrupted_cua[0].op_kind, "left_click");
    assert_eq!(state.interrupted_cua[0].turn_id, "t1");

    // The paired journal (result + turn end present) folds clean.
    envelopes.push(env("e3", cua_result("c1", CuaOutcome::Completed)));
    envelopes.push(env(
        "e4",
        Op::TurnEnd {
            turn_id: "t1".into(),
            outcome: TurnOutcome::Completed,
            usage: None,
        },
    ));
    let state = SessionState::fold(&envelopes);
    assert!(!state.turn_interrupted);
    assert!(
        state.interrupted_cua.is_empty(),
        "a paired action is history, not an interruption"
    );

    // A denied action (pair present, outcome denied) is equally clean.
    let denied = vec![
        env(
            "e0",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "/w".into(),
            },
        ),
        env(
            "e1",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "type the password".into(),
                input_blocks: vec![],
            },
        ),
        env("e2", cua_action("c1")),
        env(
            "e3",
            Op::CuaResult {
                call_id: "c1".into(),
                outcome: CuaOutcome::Denied,
                post_shot: None,
                error_kind: Some(NanoErrorKind::CuaPolicyDenied),
            },
        ),
        env(
            "e4",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: TurnOutcome::Completed,
                usage: None,
            },
        ),
    ];
    let state = SessionState::fold(&denied);
    assert!(state.interrupted_cua.is_empty());
}

/// The digest-only pair does NOT feed the open-tool-call surface: replay
/// treats both ops context-neutrally (no OpenToolCall entries, no changed
/// files, no context payloads).
#[test]
fn cua_ops_are_context_neutral_on_replay() {
    let envelopes = vec![
        env(
            "e0",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "/w".into(),
            },
        ),
        env(
            "e1",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "screenshot".into(),
                input_blocks: vec![],
            },
        ),
        env("e2", cua_action("c1")),
        env("e3", cua_result("c1", CuaOutcome::Completed)),
        env(
            "e4",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: TurnOutcome::Completed,
                usage: None,
            },
        ),
    ];
    let state = SessionState::fold(&envelopes);
    assert!(state.open_tool_calls.is_empty());
    assert!(state.changed_files.is_empty());
    assert!(state.interrupted_cua.is_empty());
}

/// Envelope-id idempotence covers the CUA pair: a retried append after a
/// crash-uncertain write never double-marks an interruption.
#[test]
fn cua_fold_is_idempotent_on_duplicate_envelopes() {
    let envelopes = vec![
        env(
            "e0",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "/w".into(),
            },
        ),
        env("e1", cua_action("c1")),
        env("e1", cua_action("c1")), // retried append, same id
        env("e2", cua_result("c1", CuaOutcome::Completed)),
        env("e2", cua_result("c1", CuaOutcome::Completed)),
    ];
    let state = SessionState::fold(&envelopes);
    assert!(state.interrupted_cua.is_empty());
}

/// SCHEMA_VERSION stays 1: the additive S9 variants ride the Unknown
/// forward-tolerance, no envelope change (the p3 pin, extended).
#[test]
fn s9_schema_version_stays_one() {
    assert_eq!(SCHEMA_VERSION, 1);
}
