use nano_memory::{ConfiguredAgents, MemoryPolicy, MemoryStore};
use nano_session::{Op, OpEnvelope, read_journal};

fn configured() -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(std::iter::empty()).unwrap()
}

#[test]
fn attributed_policy_record_round_trips_and_legacy_shape_stays_readable() {
    let op = nano_cli::memory_seam::policy_audit_op(
        &MemoryPolicy::default(),
        "project-a",
        "main",
        "session-real",
    );
    let encoded = serde_json::to_string(&OpEnvelope::new("policy-1", "now", op)).unwrap();
    let decoded: OpEnvelope = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(decoded.op, Op::MemoryPolicyResolved {
        project: Some(ref project), agent_id: Some(ref agent), session_id: Some(ref session), ..
    } if project == "project-a" && agent == "main" && session == "session-real"));
    let legacy = r#"{"v":1,"id":"legacy","ts":"now","op":{"type":"memory_policy_resolved","enabled":true,"write":"SessionAndProject","read_scope":"SessionAndProject","episode_cap":1,"fact_cap":1,"byte_cap":1,"deletion":"Never","min_tier":"ModelInference"}}"#;
    let decoded: OpEnvelope = serde_json::from_str(legacy).unwrap();
    assert!(matches!(
        decoded.op,
        Op::MemoryPolicyResolved {
            project: None,
            agent_id: None,
            session_id: None,
            ..
        }
    ));
}

#[test]
fn store_open_validates_but_does_not_emit_a_duplicate_policy_audit() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("memory.jsonl");
    let _store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &journal,
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    assert!(
        read_journal(&journal)
            .unwrap()
            .envelopes
            .iter()
            .all(|row| !matches!(row.op, Op::MemoryPolicyResolved { .. }))
    );
}

#[test]
fn memory_seam_definitions_expose_only_recall_and_mediated_propose() {
    let names = nano_cli::memory_seam::tool_definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["memory_recall", "memory_propose"]);
    for legacy in ["memory_list", "memory_read", "memory_save", "memory_delete"] {
        assert!(!names.iter().any(|name| name == legacy));
    }
}
