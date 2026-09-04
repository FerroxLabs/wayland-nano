use nano_memory::{
    ConfiguredAgents, MemoryPolicy, MemoryStore, SourceTrust, rebuild_from_journals,
};
use nano_session::{Op, read_journal};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn seed(home: &Path, name: &str, contents: &str) {
    let memory = home.join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join(name), contents).unwrap();
}

fn migrate(home: &Path, extra_env: Option<(&str, &str)>) -> std::process::Output {
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
            "migration-session",
        ])
        .env("NANO_HOME", home)
        .current_dir(home);
    if let Some((name, value)) = extra_env {
        command.env(name, value);
    }
    command.output().unwrap()
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
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["ingested"], 1);
    assert_eq!(receipt["skipped"], 0);
    assert_eq!(receipt["refused"], 0);
    assert_eq!(
        receipt["entries"][0]["content_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        receipt["journal_paths"],
        serde_json::json!([home.path().join("memory.jsonl")])
    );

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
        "not-a-timestamp.md",
        "must not acquire authority",
    );

    let output = migrate(home.path(), None);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["ingested"], 0);
    assert_eq!(receipt["refused"], 1);
    assert_eq!(receipt["entries"][0]["status"], "refused");
    assert!(
        receipt["entries"][0]["error"]
            .as_str()
            .unwrap()
            .contains("timestamp")
    );
    assert!(inspect_facts(home.path()).is_empty());
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
    assert!(String::from_utf8_lossy(&rerun.stderr).contains("already completed"));

    let after = inspect_facts(home.path());
    assert_eq!(after, before);
    assert_eq!(after.len(), 1);
}
