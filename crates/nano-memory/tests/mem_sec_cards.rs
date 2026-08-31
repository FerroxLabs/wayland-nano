use nano_memory::*;
use nano_session::{Op, read_journal};
use rusqlite::Connection;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixture {
    facts: Vec<FactWrite>,
    #[serde(default)]
    episodes: Vec<EpisodeWrite>,
    #[serde(default)]
    queries: Vec<Query>,
    #[serde(default)]
    config_cases: Vec<ParseCase>,
    #[serde(default)]
    journal_cases: Vec<ParseCase>,
    #[serde(default)]
    configured_agents: Vec<String>,
    #[serde(default)]
    unconfigured_agent_id: String,
}

#[derive(Debug, Deserialize)]
struct Query {
    text: String,
    project: String,
    agent_id: String,
}

#[derive(Debug, Deserialize)]
struct ParseCase {
    field: String,
    value: String,
}

fn load(number: u8) -> Fixture {
    let bytes = match number {
        1 => include_str!("../../../gates/fixtures/mem-sec/mem-sec-1/fixture.json"),
        2 => include_str!("../../../gates/fixtures/mem-sec/mem-sec-2/fixture.json"),
        3 => include_str!("../../../gates/fixtures/mem-sec/mem-sec-3/fixture.json"),
        4 => include_str!("../../../gates/fixtures/mem-sec/mem-sec-4/fixture.json"),
        5 => include_str!("../../../gates/fixtures/mem-sec/mem-sec-5/fixture.json"),
        6 => include_str!("../../../gates/fixtures/mem-sec/mem-sec-6/fixture.json"),
        _ => unreachable!(),
    };
    serde_json::from_str(bytes).expect("sealed mem-sec fixture must parse")
}

fn configured(ids: &[&str]) -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(ids.iter().map(|id| (*id).to_owned())).unwrap()
}

fn query(fixture: &Fixture) -> RetrieveQuery {
    let query = &fixture.queries[0];
    RetrieveQuery {
        text: query.text.clone(),
        project: query.project.clone(),
        agent_id: query.agent_id.clone(),
        agent_scope: AgentScope::Own,
        limit: 100,
        token_budget: 16_384,
        min_tier: SourceTrust::ModelInference,
    }
}

fn open_store(
    temp: &tempfile::TempDir,
    active_agent: &str,
    agents: &[&str],
) -> Result<MemoryStore, String> {
    MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &temp.path().join("memory.jsonl"),
        MemoryPolicy::default(),
        active_agent,
        configured(agents),
    )
    .map_err(|error| error.to_string())
}

fn ms_01() -> Result<(), String> {
    let fixture = load(1);
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut store = open_store(&temp, "bot-a", &["bot-a"])?;
    store
        .write_fact(fixture.facts[0].clone())
        .map_err(|error| error.to_string())?;
    let poisoned = FactWrite {
        id: "ms1-f2".into(),
        subject: "deploy_target".into(),
        predicate: "equals".into(),
        object: "prod".into(),
        confidence: 1.0,
        source_episode: None,
        valid_from: "2026-08-31T00:00:00Z".into(),
        valid_to: None,
        source_trust: SourceTrust::ToolOutput,
        project: "project-a".into(),
        agent_id: "bot-a".into(),
    };
    let resolution = store
        .write_fact(poisoned)
        .map_err(|error| error.to_string())?;
    if resolution != ContradictionResolution::KeepExisting {
        return Err(format!("lower-tier resolution was {resolution:?}"));
    }
    let current = store.current_facts().map_err(|error| error.to_string())?;
    if current
        .iter()
        .map(|fact| fact.id.as_str())
        .collect::<Vec<_>>()
        != ["ms1-f1"]
    {
        return Err("F1 did not remain the only current fact".into());
    }
    let evidence = store
        .retrieve_with_evidence(&query(&fixture))
        .map_err(|error| error.to_string())?;
    if evidence.fts_hits == 0 || evidence.knn_hits == 0 {
        return Err("retrieval checkpoint was empty".into());
    }
    if evidence.assembled.first().map(|hit| hit.id.as_str()) != Some("ms1-f1") {
        return Err("F1 did not rank first".into());
    }
    drop(store);
    let db = Connection::open(temp.path().join("memory.db")).map_err(|error| error.to_string())?;
    let losing_valid_to: Option<String> = db
        .query_row("SELECT valid_to FROM facts WHERE id='ms1-f2'", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    if losing_valid_to.is_none() {
        return Err("F2 was not stored as non-current history".into());
    }
    Ok(())
}

fn ms_02() -> Result<(), String> {
    let fixture = load(2);
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut store = open_store(&temp, "bot-a", &["bot-a"])?;
    store
        .write_fact(fixture.facts[0].clone())
        .map_err(|error| error.to_string())?;
    let control = FactWrite {
        id: "ms2-g2".into(),
        subject: "release_channel".into(),
        predicate: "equals".into(),
        object: "candidate".into(),
        confidence: 0.7,
        source_episode: None,
        valid_from: "2026-08-31T00:00:00Z".into(),
        valid_to: None,
        source_trust: SourceTrust::ToolOutput,
        project: "project-a".into(),
        agent_id: "bot-a".into(),
    };
    let resolution = store
        .write_fact(control)
        .map_err(|error| error.to_string())?;
    if resolution != ContradictionResolution::Supersede {
        return Err(format!("same-tier resolution was {resolution:?}"));
    }
    let current = store.current_facts().map_err(|error| error.to_string())?;
    if current
        .iter()
        .map(|fact| fact.id.as_str())
        .collect::<Vec<_>>()
        != ["ms2-g2"]
    {
        return Err("G2 did not supersede G1".into());
    }
    Ok(())
}

fn ingest_facts(store: &mut MemoryStore, fixture: &Fixture) -> Result<(), String> {
    for fact in &fixture.facts {
        store
            .write_fact(fact.clone())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn assert_partitioned(
    evidence: &RetrievalEvidence,
    project: &str,
    agent_id: &str,
) -> Result<(), String> {
    if evidence.fts_hits == 0 || evidence.knn_hits == 0 {
        return Err("partition checkpoint was empty".into());
    }
    if evidence
        .assembled
        .iter()
        .any(|hit| hit.project != project || hit.agent_id != agent_id)
    {
        return Err("assembled output leaked a foreign partition".into());
    }
    Ok(())
}

fn ms_03() -> Result<(), String> {
    let fixture = load(3);
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut store = open_store(&temp, "bot-a", &["bot-a"])?;
    ingest_facts(&mut store, &fixture)?;
    let evidence = store
        .retrieve_with_evidence(&query(&fixture))
        .map_err(|error| error.to_string())?;
    assert_partitioned(&evidence, "project-b", "bot-a")
}

fn ms_04() -> Result<(), String> {
    let fixture = load(4);
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let journal = temp.path().join("memory.jsonl");
    let db = temp.path().join("memory.db");
    let mut store = open_store(&temp, "bot-a", &["bot-a"])?;
    store
        .write_episode(fixture.episodes[0].clone())
        .map_err(|error| error.to_string())?;
    for fact in [
        fixture.facts[0].clone(),
        FactWrite {
            id: "ms4-extracted".into(),
            subject: "deployment_region".into(),
            predicate: "equals".into(),
            object: "ap-southeast-1".into(),
            confidence: 0.9,
            source_episode: Some("ms4-episode".into()),
            valid_from: "2026-08-31T00:00:00Z".into(),
            valid_to: None,
            source_trust: SourceTrust::ToolOutput,
            project: "project-a".into(),
            agent_id: "bot-a".into(),
        },
    ] {
        store
            .commit_proposal(MemoryProposal {
                kind: ProposalKind::Fact(fact),
            })
            .map_err(|error| error.to_string())?;
    }
    let live = store.current_facts().map_err(|error| error.to_string())?;
    if live
        .iter()
        .any(|fact| fact.source_trust != SourceTrust::ModelInference || fact.agent_id != "bot-a")
    {
        return Err("live extraction tier or attribution was laundered".into());
    }
    drop(store);
    let report = read_journal(&journal).map_err(|error| error.to_string())?;
    let journaled = report
        .envelopes
        .iter()
        .filter_map(|envelope| match &envelope.op {
            Op::MemoryWriteFact {
                fact_id,
                source_trust,
                agent_id,
                ..
            } if fact_id == "ms4-h1" || fact_id == "ms4-extracted" => {
                Some((source_trust.as_str(), agent_id.as_str()))
            }
            _ => None,
        });
    if journaled.count() != 2
        || report
            .envelopes
            .iter()
            .filter_map(|envelope| match &envelope.op {
                Op::MemoryWriteFact {
                    fact_id,
                    source_trust,
                    agent_id,
                    ..
                } if fact_id == "ms4-h1" || fact_id == "ms4-extracted" => {
                    Some((source_trust.as_str(), agent_id.as_str()))
                }
                _ => None,
            })
            .any(|pair| pair != ("ModelInference", "bot-a"))
    {
        return Err("journal tier or attribution changed".into());
    }
    std::fs::remove_file(&db).map_err(|error| error.to_string())?;
    rebuild_from_journals(&db, std::slice::from_ref(&journal), MemoryPolicy::default())
        .map_err(|error| error.to_string())?;
    let rebuilt = MemoryStore::open_at(
        &db,
        &temp.path().join("inspect.jsonl"),
        MemoryPolicy::default(),
        "bot-a",
        configured(&["bot-a"]),
    )
    .map_err(|error| error.to_string())?;
    let rebuilt_facts = rebuilt.current_facts().map_err(|error| error.to_string())?;
    if rebuilt_facts.len() != 2
        || rebuilt_facts.iter().any(|fact| {
            fact.source_trust != SourceTrust::ModelInference || fact.agent_id != "bot-a"
        })
    {
        return Err("rebuild tier or attribution changed".into());
    }
    Ok(())
}

fn assert_parse_error(case: &ParseCase) -> Result<(), String> {
    let result = match case.field.as_str() {
        "read_scope" => ReadScope::parse(&case.value).map(|_| ()),
        "source_trust" => SourceTrust::parse(&case.value).map(|_| ()),
        "agent_scope" => AgentScope::parse(&case.value).map(|_| ()),
        other => return Err(format!("unexpected parse fixture field {other}")),
    };
    match result {
        Err(MemoryError::InvalidValue { field, .. }) if field == case.field => Ok(()),
        Err(error) => Err(format!("wrong parse error for {}: {error}", case.field)),
        Ok(()) => Err(format!("{} was silently accepted", case.field)),
    }
}

fn ms_05() -> Result<(), String> {
    let fixture = load(5);
    for case in fixture.config_cases.iter().chain(&fixture.journal_cases) {
        assert_parse_error(case)?;
    }
    let configured_agents = ConfiguredAgents::try_from_ids(fixture.configured_agents.clone())
        .map_err(|e| e.to_string())?;
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let open_error = match MemoryStore::open_at(
        &temp.path().join("rejected.db"),
        &temp.path().join("rejected.jsonl"),
        MemoryPolicy::default(),
        &fixture.unconfigured_agent_id,
        configured_agents.clone(),
    ) {
        Ok(_) => return Err("unconfigured active agent opened the store".into()),
        Err(error) => error,
    };
    if !matches!(open_error, MemoryError::UnconfiguredAgent(ref id) if id == &fixture.unconfigured_agent_id)
    {
        return Err(format!("wrong store-open refusal: {open_error}"));
    }
    let mut store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &temp.path().join("memory.jsonl"),
        MemoryPolicy::default(),
        "bot-a",
        configured_agents,
    )
    .map_err(|error| error.to_string())?;
    let write_error = store
        .write_fact(FactWrite {
            id: "ms5-unconfigured".into(),
            subject: "namespace".into(),
            predicate: "belongs_to".into(),
            object: "bot-z".into(),
            confidence: 1.0,
            source_episode: None,
            valid_from: "2026-08-31T00:00:00Z".into(),
            valid_to: None,
            source_trust: SourceTrust::User,
            project: "project-a".into(),
            agent_id: fixture.unconfigured_agent_id.clone(),
        })
        .unwrap_err();
    if !matches!(write_error, MemoryError::UnconfiguredAgent(ref id) if id == &fixture.unconfigured_agent_id)
    {
        return Err(format!("wrong write refusal: {write_error}"));
    }
    Ok(())
}

fn ms_06() -> Result<(), String> {
    let fixture = load(6);
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut store = open_store(&temp, "bot-b", &["bot-a", "bot-b"])?;
    ingest_facts(&mut store, &fixture)?;
    let evidence = store
        .retrieve_with_evidence(&query(&fixture))
        .map_err(|error| error.to_string())?;
    assert_partitioned(&evidence, "project-shared", "bot-b")
}

#[test]
fn mem_sec_1() {
    ms_01().unwrap();
}

#[test]
fn mem_sec_2() {
    ms_02().unwrap();
}

#[test]
fn mem_sec_3() {
    ms_03().unwrap();
}

#[test]
fn mem_sec_4() {
    ms_04().unwrap();
}

#[test]
fn mem_sec_5() {
    ms_05().unwrap();
}

#[test]
fn mem_sec_6() {
    ms_06().unwrap();
}

#[test]
fn mem_sec_gate_summary() {
    type Check = (fn() -> Result<(), String>, &'static str, &'static str);
    let checks: [Check; 6] = [
        (ms_01, "MS-01", "security"),
        (ms_02, "MS-02", "relation"),
        (ms_03, "MS-03", "security"),
        (ms_04, "MS-04", "security"),
        (ms_05, "MS-05", "security"),
        (ms_06, "MS-06", "security"),
    ];
    let mut passed = 0;
    let mut failures = Vec::new();
    for (run, id, category) in checks {
        match run() {
            Ok(()) => passed += 1,
            Err(error) => {
                eprintln!("{id}: {error}");
                failures.push((id, category));
            }
        }
    }
    println!();
    for (id, category) in &failures {
        println!("FAIL {id} {category}");
    }
    println!("gate: {passed}/6");
    assert!(failures.is_empty(), "mem-sec gate failed");
}
