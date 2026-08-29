use nano_memory::*;
use nano_session::{JournalWriter, Op, OpEnvelope};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct Fixture {
    facts: Vec<FactWrite>,
    decisions: Vec<DecisionWrite>,
    queries: Vec<Query>,
}
#[derive(Deserialize)]
struct Query {
    text: String,
    project: String,
    agent_id: String,
}
fn load() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../gates/fixtures/memory-retrieval-recall-v1/fixture.json"
    ))
    .unwrap()
}
fn write_fact(store: &mut MemoryStore, f: FactWrite) {
    if f.source_trust == SourceTrust::ModelInference {
        store
            .commit_proposal(MemoryProposal {
                kind: ProposalKind::Fact(f),
            })
            .unwrap();
    } else {
        store.write_fact(f).unwrap();
    }
}
fn ingest(store: &mut MemoryStore, fixture: &Fixture) {
    for d in &fixture.decisions {
        store.write_decision(d.clone()).unwrap();
    }
    for f in &fixture.facts {
        write_fact(store, f.clone());
    }
}
fn query_ids(store: &MemoryStore, fixture: &Fixture) -> Vec<Vec<String>> {
    fixture
        .queries
        .iter()
        .map(|q| {
            store
                .retrieve(&RetrieveQuery {
                    text: q.text.clone(),
                    project: q.project.clone(),
                    agent_id: q.agent_id.clone(),
                    agent_scope: AgentScope::Own,
                    limit: 10,
                    token_budget: 4096,
                    min_tier: SourceTrust::ModelInference,
                })
                .unwrap()
                .into_iter()
                .map(|h| h.id)
                .collect()
        })
        .collect()
}

#[test]
fn durability_kill_mid_write() {
    if let (Ok(db), Ok(journal), Ok(marker)) = (
        std::env::var("NANO_MEMORY_KILL_DB"),
        std::env::var("NANO_MEMORY_KILL_JOURNAL"),
        std::env::var("NANO_MEMORY_KILL_MARKER"),
    ) {
        let fixture = load();
        let mut store =
            MemoryStore::open_at(Path::new(&db), Path::new(&journal), MemoryPolicy::default())
                .unwrap();
        for d in fixture.decisions {
            store.write_decision(d).unwrap();
        }
        let mut facts = fixture.facts;
        let last = facts.pop().unwrap();
        for f in facts {
            write_fact(&mut store, f);
        }
        store
            .write_fact_with_fault_injection(last, || {
                std::fs::write(marker, b"journal-synced").unwrap();
                loop {
                    std::thread::park();
                }
            })
            .unwrap();
        unreachable!();
    }
    let temp = tempfile::tempdir().unwrap();
    let fixture = load();
    let control_db = temp.path().join("control.db");
    let control_journal = temp.path().join("control.jsonl");
    let mut control =
        MemoryStore::open_at(&control_db, &control_journal, MemoryPolicy::default()).unwrap();
    ingest(&mut control, &fixture);
    let expected_queries = query_ids(&control, &fixture);
    let expected_facts = control.current_facts().unwrap();
    drop(control);
    let killed_db = temp.path().join("killed.db");
    let killed_journal = temp.path().join("killed.jsonl");
    let marker = temp.path().join("journal-synced.marker");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "durability_kill_mid_write", "--nocapture"])
        .env("NANO_MEMORY_KILL_DB", &killed_db)
        .env("NANO_MEMORY_KILL_JOURNAL", &killed_journal)
        .env("NANO_MEMORY_KILL_MARKER", &marker)
        .spawn()
        .unwrap();
    for _ in 0..1500 {
        if marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        marker.exists(),
        "child reached the journal-before-DB fault point"
    );
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success(), "fault child must terminate abruptly");
    rebuild_from_journals(
        &killed_db,
        std::slice::from_ref(&killed_journal),
        MemoryPolicy::default(),
    )
    .unwrap();
    let rebuilt = MemoryStore::open_at(
        &killed_db,
        &temp.path().join("rebuilt-open.jsonl"),
        MemoryPolicy::default(),
    )
    .unwrap();
    assert_eq!(rebuilt.current_facts().unwrap(), expected_facts);
    assert_eq!(query_ids(&rebuilt, &fixture), expected_queries);
    println!("kill-mid-write rebuild query-equivalent; agent_id and current facts identical");
}

#[test]
fn reopen_keeps_journal_ids_collision_free() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("memory.db");
    let journal = temp.path().join("session.jsonl");
    let fixture = load();
    {
        let mut store = MemoryStore::open_at(&db, &journal, MemoryPolicy::default()).unwrap();
        write_fact(&mut store, fixture.facts[0].clone());
    }
    {
        let mut store = MemoryStore::open_at(&db, &journal, MemoryPolicy::default()).unwrap();
        write_fact(&mut store, fixture.facts[1].clone());
    }
    std::fs::remove_file(&db).unwrap();
    rebuild_from_journals(&db, std::slice::from_ref(&journal), MemoryPolicy::default()).unwrap();
    let rebuilt = MemoryStore::open_at(
        &db,
        &temp.path().join("inspect.jsonl"),
        MemoryPolicy::default(),
    )
    .unwrap();
    assert_eq!(rebuilt.current_facts().unwrap().len(), 2);
}

#[test]
fn rebuild_ignores_unreceipted_model_write() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("session.jsonl");
    let mut writer = JournalWriter::open(&journal).unwrap();
    writer
        .append(&OpEnvelope::new(
            "memory-1",
            "1",
            Op::MemoryWriteFact {
                fact_id: "stranded".into(),
                subject: "s".into(),
                predicate: "p".into(),
                object: "o".into(),
                confidence_micros: 900_000,
                source_episode: None,
                valid_from: "1".into(),
                valid_to: None,
                source_trust: "ModelInference".into(),
                project: "p".into(),
                agent_id: "main".into(),
                session_id: None,
                resolver_outcome: "coexist".into(),
            },
        ))
        .unwrap();
    drop(writer);
    let db = temp.path().join("memory.db");
    rebuild_from_journals(&db, &[journal], MemoryPolicy::default()).unwrap();
    let store = MemoryStore::open_at(
        &db,
        &temp.path().join("inspect.jsonl"),
        MemoryPolicy::default(),
    )
    .unwrap();
    assert!(store.current_facts().unwrap().is_empty());
}
