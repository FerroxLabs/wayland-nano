use nano_memory::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    version: String,
    facts: Vec<FactWrite>,
    decisions: Vec<DecisionWrite>,
    queries: Vec<Query>,
}
#[derive(Deserialize)]
struct Query {
    text: String,
    project: String,
    agent_id: String,
    relevant_ids: Vec<String>,
}

fn load() -> Fixture {
    serde_json::from_str(include_str!(
        "../../../gates/fixtures/memory-retrieval-recall-v1/fixture.json"
    ))
    .unwrap()
}
fn ingest(store: &mut MemoryStore, fixture: &Fixture) {
    for d in &fixture.decisions {
        store.write_decision(d.clone()).unwrap();
    }
    for f in &fixture.facts {
        if f.source_trust == SourceTrust::ModelInference {
            store
                .commit_proposal(MemoryProposal {
                    kind: ProposalKind::Fact(f.clone()),
                })
                .unwrap();
        } else {
            store.write_fact(f.clone()).unwrap();
        }
    }
}

#[test]
fn memory_retrieval_recall_v1_bar() {
    let fixture = load();
    assert_eq!(fixture.version, "memory-retrieval-recall-v1");
    assert_eq!(fixture.facts.len(), 50);
    assert_eq!(fixture.decisions.len(), 10);
    assert_eq!(fixture.queries.len(), 20);
    let temp = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &temp.path().join("session.jsonl"),
        MemoryPolicy::default(),
    )
    .unwrap();
    ingest(&mut store, &fixture);
    let mut found = 0usize;
    let mut labeled = 0usize;
    for q in fixture.queries {
        let hits = store
            .retrieve(&RetrieveQuery {
                text: q.text,
                project: q.project.clone(),
                agent_id: q.agent_id.clone(),
                agent_scope: AgentScope::Own,
                limit: 10,
                token_budget: 4096,
                min_tier: SourceTrust::ModelInference,
            })
            .unwrap();
        assert!(
            hits.iter()
                .all(|h| h.project == q.project && h.agent_id == q.agent_id),
            "partition leak"
        );
        labeled += q.relevant_ids.len();
        found += q
            .relevant_ids
            .iter()
            .filter(|id| hits.iter().any(|h| &h.id == *id))
            .count();
    }
    let recall = found as f64 / labeled as f64;
    println!("memory-retrieval-recall-v1 recall@10={recall:.3}; cross-project=0; cross-agent=0");
    assert!(recall >= 0.90, "recall@10={recall:.3}");
}

#[test]
fn explicit_agent_scope_never_widens_project() {
    let fixture = load();
    let temp = tempfile::tempdir().unwrap();
    let mut store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &temp.path().join("session.jsonl"),
        MemoryPolicy::default(),
    )
    .unwrap();
    ingest(&mut store, &fixture);
    let result = store.retrieve(&RetrieveQuery {
        text: "database engine SQLite".into(),
        project: "project-a".into(),
        agent_id: "bot-a".into(),
        agent_scope: AgentScope::Explicit(vec!["bot-a".into(), "bot-b".into()]),
        limit: 10,
        token_budget: 4096,
        min_tier: SourceTrust::ModelInference,
    });
    assert!(matches!(
        result,
        Err(MemoryError::InvalidValue {
            field: "agent_scope",
            ..
        })
    ));
}
