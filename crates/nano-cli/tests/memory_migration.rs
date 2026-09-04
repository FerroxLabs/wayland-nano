use nano_memory::{
    AgentScope, ConfiguredAgents, DecisionWrite, FactWrite, MemoryPolicy, MemoryProposal,
    MemoryStore, ProposalKind, RetrieveQuery, SourceTrust, rebuild_from_journals,
};
use nano_session::{JournalWriter, Op, OpEnvelope, read_journal, replay::SessionState};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

fn seed(home: &Path, name: &str, contents: &str) {
    let memory = home.join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join(name), contents).unwrap();
}

fn migrate(home: &Path, extra_env: Option<(&str, &str)>) -> std::process::Output {
    write_policy(home, true, "SessionAndProject");
    migrate_with_session(home, "migration-session", extra_env)
}

fn migrate_with_session(
    home: &Path,
    session_id: &str,
    extra_env: Option<(&str, &str)>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wayland-nano"));
    command
        .args([
            "memory",
            "migrate",
            "--project",
            "project-a",
            "--agent-id",
            "main",
            "--session-id",
            session_id,
        ])
        .env("NANO_HOME", home)
        .current_dir(home);
    if let Some((name, value)) = extra_env {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn write_policy(home: &Path, enabled: bool, write: &str) {
    std::fs::write(
        home.join("memory-policy.toml"),
        format!(
            "enabled = {enabled}\nwrite = \"{write}\"\nread_scope = \"SessionAndProject\"\nembedding_backend = \"HashedLocal\"\ndeletion = \"Never\"\nmin_tier = \"ModelInference\"\n\n[retention]\nepisodes = 10000\nfacts = 50000\nbytes = 268435456\n"
        ),
    )
    .unwrap();
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReceipt {
    v: u8,
    outcome: WireOutcome,
    ingested: usize,
    skipped: usize,
    refused: usize,
    journal_paths: Vec<PathBuf>,
    entries: Vec<WireEntryReceipt>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WireOutcome {
    Complete,
    Incomplete,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEntryReceipt {
    name: String,
    content_sha256: Option<String>,
    status: WireEntryStatus,
    error_kind: Option<WireEntryErrorKind>,
    message: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WireEntryStatus {
    Ingested,
    Skipped,
    Refused,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WireEntryErrorKind {
    InvalidTimestamp,
    NotPlainFile,
    ReadFailed,
    InvalidUtf8,
    SecretScreening,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFailure {
    v: u8,
    error_kind: WireFailureKind,
    message: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WireFailureKind {
    Usage,
    AlreadyCompleted,
    PolicyInvalid,
    PolicyDisabled,
    InvalidSessionId,
    JournalInvalid,
    RebuildFailed,
    CompletionCollision,
}

fn receipt(output: &std::process::Output) -> WireReceipt {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failure(output: &std::process::Output) -> WireFailure {
    serde_json::from_slice(&output.stderr).unwrap()
}

fn configured() -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(std::iter::empty()).unwrap()
}

fn inspect_facts(home: &Path) -> Vec<nano_memory::FactState> {
    let store = MemoryStore::open(
        home,
        &home.join("inspect.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    store.current_facts().unwrap()
}

#[test]
fn explicit_migration_is_journal_first_model_inference_and_receipted() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-01-02T03-04-05-deploy-target.md",
        "deploy target is staging",
    );

    let output = migrate(home.path(), None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let receipt = receipt(&output);
    assert_eq!(receipt.v, 1);
    assert_eq!(receipt.outcome, WireOutcome::Complete);
    assert_eq!(receipt.ingested, 1);
    assert_eq!(receipt.skipped, 0);
    assert_eq!(receipt.refused, 0);
    assert_eq!(
        receipt.entries[0].content_sha256.as_ref().unwrap().len(),
        64
    );
    assert_eq!(receipt.entries[0].status, WireEntryStatus::Ingested);
    assert!(receipt.entries[0].error_kind.is_none());
    assert!(receipt.entries[0].message.is_none());
    assert_eq!(receipt.journal_paths, [home.path().join("memory.jsonl")]);

    let journal = read_journal(&home.path().join("memory.jsonl")).unwrap();
    let write = journal
        .envelopes
        .iter()
        .find_map(|entry| match &entry.op {
            Op::MemoryWriteFact {
                fact_id,
                object,
                valid_from,
                source_trust,
                project,
                agent_id,
                session_id,
                ..
            } => Some((
                fact_id,
                object,
                valid_from,
                source_trust,
                project,
                agent_id,
                session_id,
            )),
            _ => None,
        })
        .expect("migration write");
    assert_eq!(write.1, "deploy target is staging");
    assert_eq!(write.2, "2026-01-02T03:04:05Z");
    assert_eq!(write.3, "ModelInference");
    assert_eq!(write.4, "project-a");
    assert_eq!(write.5, "main");
    assert_eq!(write.6.as_deref(), Some("migration-session"));
    assert!(journal.envelopes.iter().any(|entry| matches!(
        &entry.op,
        Op::MemoryWriteReceipt { write_id, agent_id, message }
            if write_id == write.0 && agent_id == "main" && message.contains("sha256:")
    )));

    let facts = inspect_facts(home.path());
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].source_trust, SourceTrust::ModelInference);
    assert_eq!(facts[0].project, "project-a");
    assert_eq!(facts[0].agent_id, "main");
}

#[test]
fn invalid_legacy_metadata_is_a_per_entry_refusal() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-99-99T03-04-05-invalid-date.md",
        "must not acquire authority",
    );

    let output = migrate(home.path(), None);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let receipt = receipt(&output);
    assert_eq!(receipt.outcome, WireOutcome::Incomplete);
    assert_eq!(receipt.ingested, 0);
    assert_eq!(receipt.refused, 1);
    assert_eq!(receipt.entries[0].status, WireEntryStatus::Refused);
    assert_eq!(
        receipt.entries[0].error_kind,
        Some(WireEntryErrorKind::InvalidTimestamp)
    );
    assert!(
        receipt.entries[0]
            .message
            .as_deref()
            .unwrap()
            .contains("timestamp")
    );
    assert!(inspect_facts(home.path()).is_empty());
    let journal = read_journal(&home.path().join("memory.jsonl")).unwrap();
    assert!(!journal.envelopes.iter().any(|entry| matches!(
        &entry.op,
        Op::MemoryWriteReceipt { write_id, .. } if write_id == "legacy-migration-complete"
    )));

    std::fs::rename(
        home.path()
            .join("memory/2026-99-99T03-04-05-invalid-date.md"),
        home.path().join("memory/2026-01-02T03-04-05-corrected.md"),
    )
    .unwrap();
    let corrected = migrate(home.path(), None);
    assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
    assert_eq!(inspect_facts(home.path()).len(), 1);
}

#[test]
fn filenames_outside_the_legacy_store_grammar_are_never_enumerated() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), "not-a-legacy-name.md", "must stay powerless");
    seed(
        home.path(),
        "2026-01-02T03-04-05-valid.md",
        "accepted legacy entry",
    );
    let output = migrate(home.path(), None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let receipt = receipt(&output);
    assert_eq!(receipt.entries.len(), 1);
    assert_eq!(receipt.entries[0].name, "2026-01-02T03-04-05-valid.md");
    assert_eq!(inspect_facts(home.path()).len(), 1);
}

#[test]
fn resolved_policy_must_enable_writes_before_any_migration_append() {
    for (enabled, write) in [(false, "SessionAndProject"), (true, "Off")] {
        let home = tempfile::tempdir().unwrap();
        seed(
            home.path(),
            "2026-01-02T03-04-05-disabled.md",
            "must not land",
        );
        write_policy(home.path(), enabled, write);
        let output = migrate_with_session(home.path(), "migration-session", None);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        let refusal = failure(&output);
        assert_eq!(refusal.error_kind, WireFailureKind::PolicyDisabled);
        assert!(!home.path().join("memory.jsonl").exists());
        assert!(!home.path().join("memory/memory.db").exists());
    }
}

#[test]
fn invalid_session_id_is_typed_before_append_and_corrected_retry_succeeds() {
    for invalid in ["bad/session".to_owned(), "x".repeat(129)] {
        let home = tempfile::tempdir().unwrap();
        seed(
            home.path(),
            "2026-01-02T03-04-05-session.md",
            "session validation marker",
        );
        write_policy(home.path(), true, "SessionAndProject");
        let output = migrate_with_session(home.path(), &invalid, None);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert_eq!(
            failure(&output).error_kind,
            WireFailureKind::InvalidSessionId
        );
        assert!(!home.path().join("memory.jsonl").exists());
        assert!(!home.path().join("memory/memory.db").exists());

        let corrected = migrate_with_session(home.path(), "corrected-session", None);
        assert_eq!(corrected.status.code(), Some(0), "{corrected:?}");
    }
}

#[test]
fn colliding_completion_envelope_cannot_claim_migration_closure() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-01-02T03-04-05-completion.md",
        "completion collision marker",
    );
    write_policy(home.path(), true, "SessionAndProject");
    let journal_path = home.path().join("memory.jsonl");
    let collision = OpEnvelope::new(
        "legacy-migration-complete",
        "2026-01-01T00:00:00Z",
        Op::MemoryWriteReceipt {
            write_id: "different-write".into(),
            agent_id: "main".into(),
            message: "not a completion".into(),
        },
    );
    JournalWriter::open(&journal_path)
        .unwrap()
        .append(&collision)
        .unwrap();

    let output = migrate_with_session(home.path(), "migration-session", None);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(
        failure(&output).error_kind,
        WireFailureKind::CompletionCollision
    );
    let report = read_journal(&journal_path).unwrap();
    assert_eq!(report.envelopes, [collision]);
}

#[test]
fn interruption_after_journal_append_leaves_rebuildable_authority_only() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-01-02T03-04-05-crash.md",
        "journal survives interruption",
    );

    let output = migrate(
        home.path(),
        Some(("NANO_TEST_MEMORY_MIGRATE_STOP_AFTER_JOURNAL", "1")),
    );
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(!home.path().join("memory/memory.db").exists());
    let journal_path = home.path().join("memory.jsonl");
    let journal = read_journal(&journal_path).unwrap();
    assert!(
        journal
            .envelopes
            .iter()
            .any(|entry| matches!(entry.op, Op::MemoryWriteFact { .. }))
    );
    assert!(
        journal
            .envelopes
            .iter()
            .any(|entry| matches!(entry.op, Op::MemoryWriteReceipt { .. }))
    );

    let rebuilt = home.path().join("rebuilt.db");
    rebuild_from_journals(
        &rebuilt,
        &[PathBuf::from(&journal_path)],
        MemoryPolicy::default(),
        configured(),
    )
    .unwrap();
    let store = MemoryStore::open_at(
        &rebuilt,
        &home.path().join("rebuilt-inspect.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    let facts = store.current_facts().unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].agent_id, "main");
}

#[test]
fn retry_repairs_a_write_interrupted_before_its_receipt() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-01-02T03-04-05-torn-receipt.md",
        "receipt recovery marker",
    );
    let interrupted = migrate(
        home.path(),
        Some(("NANO_TEST_MEMORY_MIGRATE_STOP_AFTER_JOURNAL", "1")),
    );
    assert_eq!(interrupted.status.code(), Some(3), "{interrupted:?}");

    let journal_path = home.path().join("memory.jsonl");
    let write = read_journal(&journal_path)
        .unwrap()
        .envelopes
        .into_iter()
        .find(|entry| matches!(entry.op, Op::MemoryWriteFact { .. }))
        .unwrap();
    let mut bytes = serde_json::to_vec(&write).unwrap();
    bytes.push(b'\n');
    std::fs::write(&journal_path, bytes).unwrap();

    let recovered = migrate(home.path(), None);
    assert_eq!(recovered.status.code(), Some(0), "{recovered:?}");
    let report = read_journal(&journal_path).unwrap();
    let fact_id = match &write.op {
        Op::MemoryWriteFact { fact_id, .. } => fact_id,
        _ => unreachable!(),
    };
    assert!(report.envelopes.iter().any(|entry| matches!(
        &entry.op,
        Op::MemoryWriteReceipt { write_id, agent_id, .. }
            if write_id == fact_id && agent_id == "main"
    )));
    assert_eq!(inspect_facts(home.path()).len(), 1);
}

#[test]
fn completed_migration_refuses_rerun_and_post_migration_edits_are_invisible() {
    let home = tempfile::tempdir().unwrap();
    let name = "2026-01-02T03-04-05-stable.md";
    seed(home.path(), name, "original authority");
    assert_eq!(migrate(home.path(), None).status.code(), Some(0));
    let before = inspect_facts(home.path());

    std::fs::write(home.path().join("memory").join(name), "hand edited poison").unwrap();
    seed(
        home.path(),
        "2026-01-03T03-04-05-new-poison.md",
        "new poison",
    );
    let rerun = migrate(home.path(), None);
    assert_eq!(rerun.status.code(), Some(3), "{rerun:?}");
    let refusal = failure(&rerun);
    assert_eq!(refusal.v, 1);
    assert_eq!(refusal.error_kind, WireFailureKind::AlreadyCompleted);
    assert!(refusal.message.contains("already completed"));

    let after = inspect_facts(home.path());
    assert_eq!(after, before);
    assert_eq!(after.len(), 1);
}

#[test]
fn migrated_model_inference_cannot_remain_current_beside_higher_tier_truth() {
    let home = tempfile::tempdir().unwrap();
    let name = "2026-01-02T03-04-05-protected.md";
    seed(home.path(), name, "untrusted replacement");
    {
        let mut store = MemoryStore::open(
            home.path(),
            &home.path().join("memory.jsonl"),
            MemoryPolicy::default(),
            "main",
            configured(),
        )
        .unwrap();
        store
            .write_fact(FactWrite {
                id: "trusted-anchor".into(),
                subject: "legacy-memory-entry".into(),
                predicate: name.trim_end_matches(".md").into(),
                object: "trusted original".into(),
                confidence: 0.1,
                source_episode: None,
                valid_from: "2025-01-01T00:00:00Z".into(),
                valid_to: None,
                source_trust: SourceTrust::User,
                project: "project-a".into(),
                agent_id: "main".into(),
            })
            .unwrap();
    }

    let output = migrate(home.path(), None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let current = inspect_facts(home.path());
    assert_eq!(
        current
            .iter()
            .map(|fact| fact.id.as_str())
            .collect::<Vec<_>>(),
        ["trusted-anchor"]
    );
}

#[test]
fn valid_legacy_session_memory_op_is_readable_but_not_dedicated_authority() {
    let home = tempfile::tempdir().unwrap();
    let legacy_journal = home.path().join("sessions/legacy.jsonl");
    let mut writer = JournalWriter::open(&legacy_journal).unwrap();
    writer
        .append(&OpEnvelope::new(
            "legacy-memory",
            "2026-01-01T00:00:00Z",
            Op::MemoryWriteFact {
                fact_id: "legacy-authority".into(),
                subject: "legacy".into(),
                predicate: "authority".into(),
                object: "must not surface".into(),
                confidence_micros: 1_000_000,
                source_episode: None,
                valid_from: "2026-01-01T00:00:00Z".into(),
                valid_to: None,
                source_trust: "User".into(),
                project: "project-a".into(),
                agent_id: "main".into(),
                session_id: Some("legacy-session".into()),
                resolver_outcome: "coexist".into(),
            },
        ))
        .unwrap();
    drop(writer);

    let parsed = read_journal(&legacy_journal).unwrap();
    assert_eq!(parsed.envelopes.len(), 1);
    let mut folded = SessionState::new();
    folded.apply(&parsed.envelopes[0]);

    let dedicated = home.path().join("memory.jsonl");
    let rebuilt = home.path().join("dedicated-rebuild.db");
    rebuild_from_journals(
        &rebuilt,
        &[dedicated],
        MemoryPolicy::default(),
        configured(),
    )
    .unwrap();
    let store = MemoryStore::open_at(
        &rebuilt,
        &home.path().join("inspect-legacy.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    assert!(store.current_facts().unwrap().is_empty());
}

#[test]
fn migration_refuses_unconfigured_agent_before_writing_state() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-01-02T03-04-05-unconfigured.md",
        "must not land",
    );
    write_policy(home.path(), true, "SessionAndProject");
    let output = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args([
            "memory",
            "migrate",
            "--project",
            "project-a",
            "--agent-id",
            "bot-z",
            "--session-id",
            "migration-session",
        ])
        .env("NANO_HOME", home.path())
        .current_dir(home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let refusal = failure(&output);
    assert_eq!(refusal.error_kind, WireFailureKind::PolicyInvalid);
    assert!(refusal.message.contains("unconfigured memory agent"));
    assert!(!home.path().join("memory.jsonl").exists());
    assert!(!home.path().join("memory/memory.db").exists());
}

#[test]
fn migration_refuses_a_preexisting_fact_id_with_different_payload() {
    let home = tempfile::tempdir().unwrap();
    let name = "2026-01-02T03-04-05-collision.md";
    let contents = b"expected legacy bytes";
    seed(home.path(), name, std::str::from_utf8(contents).unwrap());
    let mut identity = Sha256::new();
    identity.update(name.as_bytes());
    identity.update([0]);
    identity.update(contents);
    let fact_id = format!("legacy-{:x}", identity.finalize());
    let journal_path = home.path().join("memory.jsonl");
    let mut writer = JournalWriter::open(&journal_path).unwrap();
    writer
        .append(&OpEnvelope::new(
            format!("legacy-migration-write-{fact_id}"),
            "2026-01-02T03:04:05Z",
            Op::MemoryWriteFact {
                fact_id: fact_id.clone(),
                subject: "legacy-memory-entry".into(),
                predicate: name.trim_end_matches(".md").into(),
                object: "substituted journal payload".into(),
                confidence_micros: 1_000_000,
                source_episode: None,
                valid_from: "2026-01-02T03:04:05Z".into(),
                valid_to: None,
                source_trust: "ModelInference".into(),
                project: "project-a".into(),
                agent_id: "main".into(),
                session_id: Some("migration-session".into()),
                resolver_outcome: "coexist".into(),
            },
        ))
        .unwrap();
    drop(writer);

    let output = migrate(home.path(), None);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let refusal = failure(&output);
    assert_eq!(refusal.error_kind, WireFailureKind::JournalInvalid);
    assert!(refusal.message.contains("conflicting authoritative write"));
    let report = read_journal(&journal_path).unwrap();
    assert!(!report.envelopes.iter().any(|entry| matches!(
        &entry.op,
        Op::MemoryWriteReceipt { write_id, .. } if write_id == &fact_id
    )));
    assert!(!home.path().join("memory/memory.db").exists());
}

fn fact_id_for(name: &str, contents: &[u8]) -> String {
    let mut identity = Sha256::new();
    identity.update(name.as_bytes());
    identity.update([0]);
    identity.update(contents);
    format!("legacy-{:x}", identity.finalize())
}

#[test]
fn unreceipted_forged_supersede_is_refused_without_appending_authority() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), true, "SessionAndProject");
    let name = "2026-01-02T03-04-05-forged-outcome.md";
    let contents = b"untrusted replacement";
    seed(home.path(), name, std::str::from_utf8(contents).unwrap());
    let journal_path = home.path().join("memory.jsonl");
    {
        let mut store = MemoryStore::open(
            home.path(),
            &journal_path,
            MemoryPolicy::default(),
            "main",
            configured(),
        )
        .unwrap();
        store
            .write_fact(FactWrite {
                id: "user-anchor".into(),
                subject: "legacy-memory-entry".into(),
                predicate: name.trim_end_matches(".md").into(),
                object: "trusted truth".into(),
                confidence: 0.1,
                source_episode: None,
                valid_from: "2025-01-01T00:00:00Z".into(),
                valid_to: None,
                source_trust: SourceTrust::User,
                project: "project-a".into(),
                agent_id: "main".into(),
            })
            .unwrap();
    }
    let fact_id = fact_id_for(name, contents);
    JournalWriter::open(&journal_path)
        .unwrap()
        .append(&OpEnvelope::new(
            format!("legacy-migration-write-{fact_id}"),
            "2026-01-02T03:04:05Z",
            Op::MemoryWriteFact {
                fact_id: fact_id.clone(),
                subject: "legacy-memory-entry".into(),
                predicate: name.trim_end_matches(".md").into(),
                object: std::str::from_utf8(contents).unwrap().into(),
                confidence_micros: 1_000_000,
                source_episode: None,
                valid_from: "2026-01-02T03:04:05Z".into(),
                valid_to: None,
                source_trust: "ModelInference".into(),
                project: "project-a".into(),
                agent_id: "main".into(),
                session_id: Some("migration-session".into()),
                resolver_outcome: "supersede".into(),
            },
        ))
        .unwrap();
    let before = std::fs::read(&journal_path).unwrap();

    let output = migrate_with_session(home.path(), "migration-session", None);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(failure(&output).error_kind, WireFailureKind::JournalInvalid);
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);
}

#[test]
fn authoritative_envelope_id_collision_is_refused_before_receipt() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), true, "SessionAndProject");
    let name = "2026-01-02T03-04-05-op-id.md";
    let contents = b"expected migration";
    seed(home.path(), name, std::str::from_utf8(contents).unwrap());
    let fact_id = fact_id_for(name, contents);
    let journal_path = home.path().join("memory.jsonl");
    let collision = OpEnvelope::new(
        format!("legacy-migration-write-{fact_id}"),
        "2026-01-01T00:00:00Z",
        Op::MemoryWriteReceipt {
            write_id: "unrelated".into(),
            agent_id: "main".into(),
            message: "unrelated op using reserved id".into(),
        },
    );
    JournalWriter::open(&journal_path)
        .unwrap()
        .append(&collision)
        .unwrap();
    let before = std::fs::read(&journal_path).unwrap();

    let output = migrate_with_session(home.path(), "migration-session", None);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(failure(&output).error_kind, WireFailureKind::JournalInvalid);
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);
}

#[test]
fn mixed_partial_retry_has_exact_counts_and_no_duplicate_authority() {
    let home = tempfile::tempdir().unwrap();
    seed(
        home.path(),
        "2026-01-02T03-04-05-valid-first.md",
        "first valid entry",
    );
    seed(
        home.path(),
        "2026-99-99T03-04-05-fix-me.md",
        "second corrected entry",
    );
    let first = migrate(home.path(), None);
    assert_eq!(first.status.code(), Some(3), "{first:?}");
    let first_receipt = receipt(&first);
    assert_eq!(first_receipt.ingested, 1);
    assert_eq!(first_receipt.skipped, 0);
    assert_eq!(first_receipt.refused, 1);

    std::fs::rename(
        home.path().join("memory/2026-99-99T03-04-05-fix-me.md"),
        home.path().join("memory/2026-01-03T03-04-05-fixed.md"),
    )
    .unwrap();
    let second = migrate(home.path(), None);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    let second_receipt = receipt(&second);
    assert_eq!(second_receipt.ingested, 1);
    assert_eq!(second_receipt.skipped, 1);
    assert_eq!(second_receipt.refused, 0);

    let rows = read_journal(&home.path().join("memory.jsonl"))
        .unwrap()
        .envelopes;
    let writes = rows
        .iter()
        .filter(|row| matches!(row.op, Op::MemoryWriteFact { .. }))
        .count();
    let entry_receipts = rows
        .iter()
        .filter(|row| {
            matches!(&row.op, Op::MemoryWriteReceipt { write_id, message, .. }
                if write_id.starts_with("legacy-") && message.contains("sha256:"))
        })
        .count();
    assert_eq!(writes, 2);
    assert_eq!(entry_receipts, 2);
}

#[test]
fn sigkill_after_migration_journal_sync_rebuilds_like_control() {
    let control = tempfile::tempdir().unwrap();
    let killed = tempfile::tempdir().unwrap();
    let name = "2026-01-02T03-04-05-real-kill.md";
    let contents = "real kill migration marker";
    seed(control.path(), name, contents);
    seed(killed.path(), name, contents);
    let control_output = migrate(control.path(), None);
    assert_eq!(control_output.status.code(), Some(0), "{control_output:?}");
    let expected = inspect_facts(control.path());

    write_policy(killed.path(), true, "SessionAndProject");
    let marker = killed.path().join("migration-journal-synced.marker");
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args([
            "memory",
            "migrate",
            "--project",
            "project-a",
            "--agent-id",
            "main",
            "--session-id",
            "migration-session",
        ])
        .env("NANO_HOME", killed.path())
        .env("NANO_TEST_MEMORY_MIGRATE_KILL_MARKER", &marker)
        .current_dir(killed.path())
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
        "child reached migration journal fault point"
    );
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(!killed.path().join("memory/memory.db").exists());

    let journal = killed.path().join("memory.jsonl");
    rebuild_from_journals(
        &killed.path().join("memory/memory.db"),
        std::slice::from_ref(&journal),
        MemoryPolicy::default(),
        configured(),
    )
    .unwrap();
    assert_eq!(inspect_facts(killed.path()), expected);
    assert_eq!(
        journal_receipts(&journal),
        journal_receipts(&control.path().join("memory.jsonl"))
    );
}

#[derive(Deserialize)]
struct RecallFixture {
    facts: Vec<FactWrite>,
    decisions: Vec<DecisionWrite>,
    queries: Vec<RecallQuery>,
}

#[derive(Deserialize)]
struct RecallQuery {
    text: String,
    project: String,
    agent_id: String,
}

fn load_recall_fixture() -> RecallFixture {
    serde_json::from_str(include_str!(
        "../../../gates/fixtures/memory-retrieval-recall-v1/fixture.json"
    ))
    .unwrap()
}

fn fixture_query_ids(store: &MemoryStore, fixture: &RecallFixture) -> Vec<Vec<String>> {
    fixture
        .queries
        .iter()
        .map(|query| {
            store
                .retrieve(&RetrieveQuery {
                    text: query.text.clone(),
                    project: query.project.clone(),
                    agent_id: query.agent_id.clone(),
                    agent_scope: AgentScope::Own,
                    limit: 10,
                    token_budget: 4_096,
                    min_tier: SourceTrust::ModelInference,
                })
                .unwrap()
                .into_iter()
                .map(|hit| hit.id)
                .collect()
        })
        .collect()
}

fn journal_receipts(path: &Path) -> Vec<(String, String, String)> {
    read_journal(path)
        .unwrap()
        .envelopes
        .into_iter()
        .filter_map(|entry| match entry.op {
            Op::MemoryWriteReceipt {
                write_id,
                agent_id,
                message,
            } => Some((write_id, agent_id, message)),
            _ => None,
        })
        .collect()
}

#[test]
fn actual_migration_output_is_query_equivalent_after_db_drop_and_rebuild() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), true, "SessionAndProject");
    let fixture = load_recall_fixture();
    let configured_agents =
        ConfiguredAgents::try_from_ids(["bot-a".to_owned(), "bot-b".to_owned()]).unwrap();
    std::fs::create_dir_all(home.path().join("agents")).unwrap();
    for agent in ["bot-a", "bot-b"] {
        std::fs::write(
            home.path()
                .join("agents")
                .join(format!("{agent}.agent.toml")),
            format!("id = \"{agent}\"\n"),
        )
        .unwrap();
    }
    let journal = home.path().join("memory.jsonl");
    {
        let mut store = MemoryStore::open(
            home.path(),
            &journal,
            MemoryPolicy::default(),
            "main",
            configured_agents.clone(),
        )
        .unwrap();
        for decision in &fixture.decisions {
            if decision.source_trust == SourceTrust::ModelInference {
                store
                    .commit_proposal(MemoryProposal {
                        kind: ProposalKind::Decision(decision.clone()),
                    })
                    .unwrap();
            } else {
                store.write_decision(decision.clone()).unwrap();
            }
        }
        for fact in &fixture.facts {
            if fact.source_trust == SourceTrust::ModelInference {
                store
                    .commit_proposal(MemoryProposal {
                        kind: ProposalKind::Fact(fact.clone()),
                    })
                    .unwrap();
            } else {
                store.write_fact(fact.clone()).unwrap();
            }
        }
    }
    seed(
        home.path(),
        "2026-01-02T03-04-05-migrated-roundtrip.md",
        "migrated attribution sentinel",
    );
    let migrated = migrate_with_session(home.path(), "migration-session", None);
    assert_eq!(migrated.status.code(), Some(0), "{migrated:?}");
    assert_eq!(receipt(&migrated).outcome, WireOutcome::Complete);

    let (expected_queries, expected_facts, expected_receipts);
    {
        let store = MemoryStore::open(
            home.path(),
            &home.path().join("live-inspect.jsonl"),
            MemoryPolicy::default(),
            "main",
            configured_agents.clone(),
        )
        .unwrap();
        expected_queries = fixture_query_ids(&store, &fixture);
        expected_facts = store.current_facts().unwrap();
        expected_receipts = journal_receipts(&journal);
    }
    let migrated_fact = expected_facts
        .iter()
        .find(|fact| fact.id.starts_with("legacy-"))
        .expect("actual migrated fact");
    assert_eq!(migrated_fact.source_trust, SourceTrust::ModelInference);
    assert_eq!(migrated_fact.project, "project-a");
    assert_eq!(migrated_fact.agent_id, "main");
    assert!(
        expected_receipts
            .iter()
            .any(|(write_id, agent_id, message)| {
                write_id == &migrated_fact.id && agent_id == "main" && message.contains("sha256:")
            })
    );

    let db = home.path().join("memory/memory.db");
    std::fs::remove_file(&db).unwrap();
    rebuild_from_journals(
        &db,
        std::slice::from_ref(&journal),
        MemoryPolicy::default(),
        configured_agents.clone(),
    )
    .unwrap();
    let rebuilt = MemoryStore::open(
        home.path(),
        &home.path().join("rebuilt-inspect.jsonl"),
        MemoryPolicy::default(),
        "main",
        configured_agents,
    )
    .unwrap();
    let rebuilt_facts = rebuilt.current_facts().unwrap();
    assert_eq!(rebuilt_facts, expected_facts);
    for (actual, expected) in rebuilt_facts.iter().zip(&expected_facts) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.valid_from, expected.valid_from);
        assert_eq!(actual.valid_to, expected.valid_to);
        assert_eq!(actual.source_trust, expected.source_trust);
        assert_eq!(actual.project, expected.project);
        assert_eq!(actual.agent_id, expected.agent_id);
    }
    assert_eq!(fixture_query_ids(&rebuilt, &fixture), expected_queries);
    assert_eq!(journal_receipts(&journal), expected_receipts);
}
