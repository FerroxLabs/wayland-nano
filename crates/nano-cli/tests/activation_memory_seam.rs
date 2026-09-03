use nano_memory::{ConfiguredAgents, MemoryPolicy, MemoryStore};
use nano_session::{Op, OpEnvelope, read_journal};
use std::sync::Arc;

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

#[test]
fn every_runtime_entrypoint_uses_the_ordered_fail_closed_bootstrap() {
    for entrypoint in ["acp-new", "acp-load", "exec-fresh", "exec-resume", "protocol-host"] {
        let temp = tempfile::tempdir().unwrap();
        let journal_path = temp.path().join(format!("{entrypoint}.jsonl"));
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal_path).unwrap());
        let resolved = nano_cli::memory_policy::ResolvedMemoryPolicy::disabled();
        let result = nano_cli::memory_seam::bootstrap_entrypoint_memory(
            temp.path(),
            entrypoint,
            "workspace",
            &format!("{entrypoint}-begin"),
            "project-a",
            "main",
            nano_activation::admission::AdmittedStrategy::Fresh,
            nano_activation::admission::AdmittedFallback::None,
            &resolved,
            coordinator.clone(),
            || Ok(()),
        )
        .unwrap();
        assert!(result.is_none());
        coordinator
            .append(&OpEnvelope::new(
                format!("{entrypoint}-effect"),
                "now",
                Op::MemoryWriteReceipt {
                    write_id: format!("{entrypoint}-effect"),
                    committed: false,
                    agent_id: "main".into(),
                },
            ))
            .unwrap();
        let rows = read_journal(&journal_path).unwrap().envelopes;
        assert!(matches!(rows[0].op, Op::SessionBegin { ref session_id, .. } if session_id == entrypoint));
        assert!(matches!(rows[1].op, Op::MemoryPolicyResolved {
            project: Some(ref project), agent_id: Some(ref agent), session_id: Some(ref session), ..
        } if project == "project-a" && agent == "main" && session == entrypoint));
        assert!(matches!(rows[2].op, Op::MemoryWriteReceipt { .. }));
        assert_eq!(rows.iter().filter(|row| matches!(row.op, Op::MemoryPolicyResolved { .. })).count(), 1);
    }
}

#[test]
fn every_runtime_entrypoint_refuses_policy_append_failure_before_effect() {
    for entrypoint in ["acp-new", "acp-load", "exec-fresh", "exec-resume", "protocol-host"] {
        let temp = tempfile::tempdir().unwrap();
        let journal_path = temp.path().join(format!("{entrypoint}.jsonl"));
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal_path).unwrap());
        let resolved = nano_cli::memory_policy::ResolvedMemoryPolicy::disabled();
        let driver_calls = std::cell::Cell::new(0_u32);
        let error = nano_cli::memory_seam::bootstrap_entrypoint_memory(
            temp.path(),
            entrypoint,
            "workspace",
            &format!("{entrypoint}-begin"),
            "project-a",
            "main",
            nano_activation::admission::AdmittedStrategy::Fresh,
            nano_activation::admission::AdmittedFallback::None,
            &resolved,
            coordinator,
            || {
                std::fs::remove_file(&journal_path)?;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, nano_session::NanoErrorKind::JournalUnavailable);
        assert_eq!(driver_calls.get(), 0);
        assert!(!temp.path().join("memory.db").exists());
        assert!(!journal_path.exists());
    }
}
