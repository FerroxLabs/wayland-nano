use nano_session::op::{Op, OpEnvelope};
use nano_session::replay::SessionState;

fn envelope(id: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(id, "2026-08-30T00:00:00Z", op)
}

#[test]
fn legacy_cron_and_memory_vocabulary_remains_replayable_but_grants_no_authority() {
    let legacy = vec![
        envelope(
            "session",
            Op::SessionBegin {
                session_id: "legacy-session".into(),
                cwd: ".".into(),
            },
        ),
        envelope(
            "cron-created",
            Op::CronCreated {
                job_id: "legacy-job".into(),
                session_id: "legacy-session".into(),
                schedule: "* * * * *".into(),
                prompt: "legacy prompt".into(),
                created_at: "2026-08-30T00:00:00Z".into(),
            },
        ),
        envelope(
            "memory-fact",
            Op::MemoryWriteFact {
                fact_id: "legacy-fact".into(),
                subject: "subject".into(),
                predicate: "predicate".into(),
                object: "object".into(),
                confidence_micros: 1_000_000,
                source_episode: None,
                valid_from: "2026-08-30T00:00:00Z".into(),
                valid_to: None,
                source_trust: "user".into(),
                project: "project-a".into(),
                agent_id: "main".into(),
                session_id: Some("legacy-session".into()),
                resolver_outcome: "accepted".into(),
            },
        ),
    ];

    let bytes = legacy
        .iter()
        .flat_map(|entry| {
            let mut line = serde_json::to_vec(entry).unwrap();
            line.push(b'\n');
            line
        })
        .collect::<Vec<_>>();
    let parsed = nano_session::reader::parse_journal_bytes(&bytes).unwrap();
    assert_eq!(
        parsed.envelopes, legacy,
        "legacy bytes remain replay-readable"
    );

    let mut first = SessionState::new();
    let mut second = SessionState::new();
    for entry in &parsed.envelopes {
        first.apply(entry);
        second.apply(entry);
    }
    assert_eq!(
        format!("{first:#?}"),
        format!("{second:#?}"),
        "legacy replay remains deterministic"
    );
    assert!(first.cron_jobs.contains_key("legacy-job"));

    // Authority state is owned by nano-activation, not SessionState. Prove
    // the replayed legacy carrier itself has no field that could be
    // reinterpreted as enrollment, grant, admission, or receipt authority.
    let text = String::from_utf8(bytes).unwrap();
    for forbidden in [
        "issuer_id",
        "principal_id",
        "activation_id",
        "receipt_id",
        "grant_epoch",
    ] {
        assert!(
            !text.contains(forbidden),
            "legacy replay minted {forbidden}"
        );
    }
}
