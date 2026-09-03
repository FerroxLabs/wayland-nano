use nano_memory::*;
use nano_session::{JournalWriter, Op, OpEnvelope};

fn configured() -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(["agent-a".to_owned(), "agent-b".to_owned()]).unwrap()
}

fn fact(id: &str, object: &str) -> FactWrite {
    FactWrite {
        id: id.into(),
        subject: id.into(),
        predicate: "value".into(),
        object: object.into(),
        confidence: 1.0,
        source_episode: None,
        valid_from: "1".into(),
        valid_to: None,
        source_trust: SourceTrust::User,
        project: "project-a".into(),
        agent_id: "agent-a".into(),
    }
}

fn query(store: &MemoryStore, text: &str) -> Vec<String> {
    store
        .retrieve(&RetrieveQuery {
            text: text.into(),
            project: "project-a".into(),
            agent_id: "agent-a".into(),
            agent_scope: AgentScope::Own,
            limit: 10,
            token_budget: 1_000,
            min_tier: SourceTrust::ModelInference,
        })
        .unwrap()
        .into_iter()
        .map(|hit| hit.id)
        .collect()
}

fn append_fact(writer: &mut JournalWriter, id: &str, agent_id: &str, session_id: Option<&str>) {
    writer
        .append(&OpEnvelope::new(
            format!("op-{id}"),
            "1",
            Op::MemoryWriteFact {
                fact_id: id.into(),
                subject: id.into(),
                predicate: "value".into(),
                object: "content".into(),
                confidence_micros: 1_000_000,
                source_episode: None,
                valid_from: "1".into(),
                valid_to: None,
                source_trust: "User".into(),
                project: "project-a".into(),
                agent_id: agent_id.into(),
                session_id: session_id.map(str::to_owned),
                resolver_outcome: "coexist".into(),
            },
        ))
        .unwrap();
}

#[test]
fn recovery_rejects_invalid_partition_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("invalid.jsonl");
    let mut writer = JournalWriter::open(&journal).unwrap();
    append_fact(&mut writer, "bad", "agent\nspoof", None);
    drop(writer);

    let error = rebuild_from_journals(
        &temp.path().join("memory.db"),
        &[journal],
        MemoryPolicy::default(),
        configured(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        MemoryError::InvalidValue {
            field: "agent_id",
            ..
        }
    ));
}

#[test]
fn recovery_rejects_an_unconfigured_agent() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("unconfigured.jsonl");
    let mut writer = JournalWriter::open(&journal).unwrap();
    append_fact(&mut writer, "foreign", "agent-z", None);
    drop(writer);

    let error = rebuild_from_journals(
        &temp.path().join("memory.db"),
        &[journal],
        MemoryPolicy::default(),
        configured(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        MemoryError::UnconfiguredAgent(ref id) if id == "agent-z"
    ));
}

#[test]
fn recovery_does_not_apply_receipt_from_a_different_agent() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("mismatch.jsonl");
    let mut writer = JournalWriter::open(&journal).unwrap();
    writer
        .append(&OpEnvelope::new(
            "write",
            "1",
            Op::MemoryWriteFact {
                fact_id: "proposed".into(),
                subject: "proposed".into(),
                predicate: "value".into(),
                object: "content".into(),
                confidence_micros: 900_000,
                source_episode: None,
                valid_from: "1".into(),
                valid_to: None,
                source_trust: "ModelInference".into(),
                project: "project-a".into(),
                agent_id: "agent-a".into(),
                session_id: None,
                resolver_outcome: "coexist".into(),
            },
        ))
        .unwrap();
    writer
        .append(&OpEnvelope::new(
            "receipt",
            "2",
            Op::MemoryWriteReceipt {
                write_id: "proposed".into(),
                agent_id: "agent-b".into(),
                message: "wrong principal".into(),
            },
        ))
        .unwrap();
    drop(writer);

    let db = temp.path().join("memory.db");
    rebuild_from_journals(&db, &[journal], MemoryPolicy::default(), configured()).unwrap();
    let store = MemoryStore::open_at(
        &db,
        &temp.path().join("inspect.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    assert!(store.current_facts().unwrap().is_empty());
}

#[test]
fn rebuild_contention_preserves_original_database() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("memory.db");
    let target_journal = temp.path().join("target.jsonl");
    let mut target = MemoryStore::open_at(
        &db,
        &target_journal,
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    target.write_fact(fact("original", "keep-me")).unwrap();
    let before = std::fs::read(&db).unwrap();

    let source = temp.path().join("source.jsonl");
    let mut writer = JournalWriter::open(&source).unwrap();
    append_fact(&mut writer, "replacement", "agent-a", None);
    drop(writer);
    let error =
        rebuild_from_journals(&db, &[source], MemoryPolicy::default(), configured()).unwrap_err();
    assert!(matches!(error, MemoryError::Contention(_)));
    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert_eq!(target.current_facts().unwrap()[0].id, "original");
}

#[test]
fn rebuild_atomically_replaces_target_and_cleans_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("memory.db");
    {
        let mut old = MemoryStore::open_at(
            &db,
            &temp.path().join("old.jsonl"),
            MemoryPolicy::default(),
            "main",
            configured(),
        )
        .unwrap();
        old.write_fact(fact("old", "obsolete")).unwrap();
    }
    let source_db = temp.path().join("source.db");
    let source_journal = temp.path().join("source.jsonl");
    {
        let mut source = MemoryStore::open_at(
            &source_db,
            &source_journal,
            MemoryPolicy::default(),
            "main",
            configured(),
        )
        .unwrap();
        source.write_fact(fact("new", "replacement")).unwrap();
    }

    rebuild_from_journals(
        &db,
        std::slice::from_ref(&source_journal),
        MemoryPolicy::default(),
        configured(),
    )
    .unwrap();
    let rebuilt = MemoryStore::open_at(
        &db,
        &temp.path().join("inspect.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    assert_eq!(rebuilt.current_facts().unwrap()[0].id, "new");
    let siblings = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("rebuild-"))
        .collect::<Vec<_>>();
    assert!(siblings.is_empty(), "rebuild siblings remain: {siblings:?}");
}

#[test]
fn session_only_policy_isolates_narrow_reads() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("memory.db");
    for (session, id, content) in [
        ("session-a", "fact-a", "alpha continuity"),
        ("session-b", "fact-b", "beta continuity"),
    ] {
        let policy = MemoryPolicy {
            write: WriteScope::SessionOnly,
            read_scope: ReadScope::Session,
            session_id: Some(session.into()),
            ..MemoryPolicy::default()
        };
        let mut store = MemoryStore::open_at(
            &db,
            &temp.path().join(format!("{session}.jsonl")),
            policy,
            "main",
            configured(),
        )
        .unwrap();
        store.write_fact(fact(id, content)).unwrap();
    }

    let policy = MemoryPolicy {
        write: WriteScope::SessionOnly,
        read_scope: ReadScope::Session,
        session_id: Some("session-a".into()),
        ..MemoryPolicy::default()
    };
    let store = MemoryStore::open_at(
        &db,
        &temp.path().join("read.jsonl"),
        policy,
        "main",
        configured(),
    )
    .unwrap();
    assert_eq!(query(&store, "continuity"), vec!["fact-a"]);
}

#[test]
fn project_writes_remain_visible_after_the_originating_session() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("memory.db");
    let write_policy = MemoryPolicy {
        write: WriteScope::SessionAndProject,
        read_scope: ReadScope::Session,
        session_id: Some("session-a".into()),
        ..MemoryPolicy::default()
    };
    {
        let mut store = MemoryStore::open_at(
            &db,
            &temp.path().join("write.jsonl"),
            write_policy,
            "main",
            configured(),
        )
        .unwrap();
        store
            .write_fact(fact("project-fact", "persistent project marker"))
            .unwrap();
    }

    let read_policy = MemoryPolicy {
        session_id: Some("session-b".into()),
        ..MemoryPolicy::default()
    };
    let store = MemoryStore::open_at(
        &db,
        &temp.path().join("read.jsonl"),
        read_policy,
        "main",
        configured(),
    )
    .unwrap();
    assert_eq!(query(&store, "persistent marker"), vec!["project-fact"]);
}

#[test]
fn retention_policy_rebuild_is_query_equivalent() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("live.db");
    let journal = temp.path().join("session.jsonl");
    let policy = MemoryPolicy {
        retention: RetentionCaps {
            facts: 1,
            ..RetentionCaps::default()
        },
        ..MemoryPolicy::default()
    };
    let expected;
    {
        let mut store =
            MemoryStore::open_at(&db, &journal, policy.clone(), "main", configured()).unwrap();
        store.write_fact(fact("low", "discarded marker")).unwrap();
        store.write_fact(fact("high", "retained marker")).unwrap();
        expected = store.current_facts().unwrap();
    }

    let rebuilt_db = temp.path().join("rebuilt.db");
    rebuild_from_journals(
        &rebuilt_db,
        std::slice::from_ref(&journal),
        policy.clone(),
        configured(),
    )
    .unwrap();
    let rebuilt = MemoryStore::open_at(
        &rebuilt_db,
        &temp.path().join("inspect.jsonl"),
        policy,
        "main",
        configured(),
    )
    .unwrap();
    assert_eq!(rebuilt.current_facts().unwrap(), expected);
    assert_eq!(query(&rebuilt, "marker"), vec![expected[0].id.clone()]);
}

#[test]
fn local_temp_paths_are_accepted_and_network_forms_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    MemoryStore::open_at(
        &temp.path().join("local.db"),
        &temp.path().join("local.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();

    #[cfg(windows)]
    for path in [
        std::path::PathBuf::from(r"\\server\share\memory.db"),
        std::path::PathBuf::from(r"\\?\UNC\server\share\memory.db"),
    ] {
        let error = match MemoryStore::open_at(
            &path,
            &temp.path().join("network.jsonl"),
            MemoryPolicy::default(),
            "main",
            configured(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("network path unexpectedly accepted: {}", path.display()),
        };
        assert!(matches!(error, MemoryError::NetworkFilesystem));
    }
}
