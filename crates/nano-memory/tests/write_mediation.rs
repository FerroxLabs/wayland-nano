use nano_memory::*;
use nano_session::{Op, read_journal};

fn configured() -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(["bot-a".to_owned()]).unwrap()
}

fn fact(tier: SourceTrust) -> FactWrite {
    FactWrite {
        id: "f-model".into(),
        subject: "deploy".into(),
        predicate: "target".into(),
        object: "staging".into(),
        confidence: 0.9,
        source_episode: None,
        valid_from: "1".into(),
        valid_to: None,
        source_trust: tier,
        project: "p".into(),
        agent_id: "bot-a".into(),
    }
}

#[test]
fn model_proposes_host_commits_and_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("session.jsonl");
    let mut store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &journal,
        MemoryPolicy::default(),
        "bot-a",
        configured(),
    )
    .unwrap();
    assert!(matches!(
        store.write_fact(fact(SourceTrust::ModelInference)),
        Err(MemoryError::MediationRequired)
    ));
    assert!(matches!(
        store.write_decision(DecisionWrite {
            id: "d".into(),
            summary: "s".into(),
            why: "w".into(),
            how_to_apply: "h".into(),
            tags: vec![],
            source_episode: None,
            valid_from: "1".into(),
            valid_to: None,
            source_trust: SourceTrust::ModelInference,
            project: "p".into(),
            agent_id: "bot-a".into()
        }),
        Err(MemoryError::MediationRequired)
    ));
    assert!(matches!(
        store.write_episode(EpisodeWrite {
            id: "e".into(),
            content: "c".into(),
            source: "model".into(),
            source_product: "wayland-nano".into(),
            valid_from: "1".into(),
            valid_to: None,
            source_trust: SourceTrust::ModelInference,
            project: "p".into(),
            agent_id: "bot-a".into()
        }),
        Err(MemoryError::MediationRequired)
    ));
    assert!(matches!(
        store.write_procedure(ProcedureWrite {
            id: "r".into(),
            title: "t".into(),
            steps: "s".into(),
            created_by: "model".into(),
            valid_from: "1".into(),
            valid_to: None,
            source_trust: SourceTrust::ModelInference,
            project: "p".into(),
            agent_id: "bot-a".into()
        }),
        Err(MemoryError::MediationRequired)
    ));
    let before = store
        .retrieve(&RetrieveQuery {
            text: "deploy".into(),
            project: "p".into(),
            agent_id: "bot-a".into(),
            agent_scope: AgentScope::Own,
            limit: 10,
            token_budget: 4096,
            min_tier: SourceTrust::ModelInference,
        })
        .unwrap();
    assert!(before.is_empty());
    let receipt = store
        .commit_proposal(MemoryProposal {
            kind: ProposalKind::Fact(fact(SourceTrust::User)),
        })
        .unwrap();
    assert_eq!(receipt.message, "memory updated for bot-a");
    store
        .commit_proposal(MemoryProposal {
            kind: ProposalKind::Decision(DecisionWrite {
                id: "d-mediated".into(),
                summary: "summary".into(),
                why: "why".into(),
                how_to_apply: "apply".into(),
                tags: vec!["wayland-nano-canary-abcdefgh".into()],
                source_episode: None,
                valid_from: "1".into(),
                valid_to: None,
                source_trust: SourceTrust::User,
                project: "p".into(),
                agent_id: "bot-a".into(),
            }),
        })
        .unwrap();
    store
        .commit_proposal(MemoryProposal {
            kind: ProposalKind::Episode(EpisodeWrite {
                id: "e-mediated".into(),
                content: "episode".into(),
                source: "model".into(),
                source_product: "wayland-nano".into(),
                valid_from: "1".into(),
                valid_to: None,
                source_trust: SourceTrust::User,
                project: "p".into(),
                agent_id: "bot-a".into(),
            }),
        })
        .unwrap();
    store
        .commit_proposal(MemoryProposal {
            kind: ProposalKind::Procedure(ProcedureWrite {
                id: "r-mediated".into(),
                title: "title".into(),
                steps: "steps".into(),
                created_by: "model".into(),
                valid_from: "1".into(),
                valid_to: None,
                source_trust: SourceTrust::User,
                project: "p".into(),
                agent_id: "bot-a".into(),
            }),
        })
        .unwrap();
    let report = read_journal(&journal).unwrap();
    assert!(report.envelopes.iter().any(|e|matches!(&e.op,Op::MemoryWriteFact{source_trust,agent_id,..} if source_trust=="ModelInference"&&agent_id=="bot-a")));
    assert!(report.envelopes.iter().any(|e|matches!(&e.op,Op::MemoryWriteReceipt{message,..} if message=="memory updated for bot-a")));
    assert_eq!(
        report
            .envelopes
            .iter()
            .filter(|e| matches!(e.op, Op::MemoryWriteReceipt { .. }))
            .count(),
        4
    );
    assert!(
        !std::fs::read_to_string(&journal)
            .unwrap()
            .contains("wayland-nano-canary-abcdefgh")
    );
}
