use nano_agent::turn::{ToolExecutor, ToolOutcome};
use nano_model::types::ToolCall;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug)]
struct NoInner;

#[async_trait::async_trait]
impl ToolExecutor for NoInner {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: format!("inner saw {}", call.name),
            progress: Default::default(),
            error_kind: None,
        }
    }
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "forced".into(),
        name: name.into(),
        arguments,
    }
}

fn seed_legacy_state(home: &Path) {
    for (path, contents) in [
        ("memory/2026-01-01T00-00-00-canary.md", "legacy fs memory"),
        ("t2/canary.bin", "legacy t2 memory"),
        ("cron/jobs.json", r#"[{"job_id":"due-canary"}]"#),
        ("hooks.toml", "[hooks]\ncanary = true\n"),
        ("fire-canary", "must not fire"),
    ] {
        let path = home.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

fn inventory(home: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u64, Option<std::time::SystemTime>)> {
    let mut result = BTreeMap::new();
    let mut pending = vec![home.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(path);
            } else {
                result.insert(
                    path.strip_prefix(home).unwrap().to_path_buf(),
                    (
                        std::fs::read(&path).unwrap(),
                        metadata.len(),
                        metadata.modified().ok(),
                    ),
                );
            }
        }
    }
    result
}

#[test]
fn unauthenticated_process_paths_leave_seeded_legacy_state_unchanged() {
    for (label, args, input) in [
        ("exec", vec!["exec", "hello"], None),
        ("protocol-host", vec!["protocol-host"], None),
        (
            "acp-host",
            vec!["acp-host"],
            Some(
                br#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"."}}
"#
                .as_slice(),
            ),
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        seed_legacy_state(home.path());
        let before = inventory(home.path());
        let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
            .args(args)
            .env("NANO_HOME", home.path())
            .env("NANO_MEMORY_WRITE", "true")
            .current_dir(home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child.stdin.take().unwrap().write_all(input).unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{label}: {output:?}");
        assert_eq!(
            inventory(home.path()),
            before,
            "{label} changed legacy state"
        );
    }
}

#[tokio::test]
async fn forced_memory_and_cron_calls_are_typed_denials_with_zero_state_change() {
    let home = tempfile::tempdir().unwrap();
    seed_legacy_state(home.path());
    let journal_path = home.path().join("sessions/quarantine.jsonl");
    std::fs::create_dir_all(journal_path.parent().unwrap()).unwrap();
    let coordinator = nano_session::JournalCoordinator::open(&journal_path).unwrap();
    let store = nano_agent::cron::JsonCronStore::new(home.path());
    let before = inventory(home.path());

    let memory = nano_agent::memory::MemoryToolExecutor::quarantined(&NoInner);
    for name in ["memory_list", "memory_read", "memory_save", "memory_delete"] {
        let outcome = memory.execute(&call(name, serde_json::json!({}))).await;
        assert!(!outcome.ok, "{name}");
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::UnknownTool)
        );
    }

    let cron = nano_agent::cron::CronjobExecutor::quarantined(
        &NoInner,
        &store,
        "quarantine".into(),
        &coordinator,
    );
    let outcome = cron
        .execute(&call(
            "cronjob",
            serde_json::json!({"action":"create","schedule":"* * * * *","prompt":"touch canary"}),
        ))
        .await;
    assert!(!outcome.ok);
    assert_eq!(
        outcome.error_kind,
        Some(nano_session::NanoErrorKind::UnknownTool)
    );
    assert_eq!(inventory(home.path()), before);
}

#[test]
fn production_wiring_omits_legacy_surfaces_and_blocks_environment_reenablement() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let acp = std::fs::read_to_string(root.join("src/acp_mode.rs")).unwrap();
    let host = std::fs::read_to_string(root.join("src/host_mode.rs")).unwrap();
    let exec = std::fs::read_to_string(root.join("src/exec_run.rs")).unwrap();
    let fire = std::fs::read_to_string(root.join("src/cron_fire.rs")).unwrap();
    assert!(acp.contains("phase2_persistence_quarantined = activation.is_some()"));
    assert!(acp.contains("MemoryToolExecutor::quarantined"));
    assert!(acp.contains("CronjobExecutor::quarantined"));
    assert!(host.contains("MemoryToolExecutor::quarantined"));
    assert!(!host.contains("memory_tool_definitions(memory_write)"));
    assert!(exec.contains("CronjobExecutor::quarantined"));
    assert!(!exec.contains("vec![nano_agent::cron::cronjob_tool_definition()]"));
    assert!(fire.contains("const fn phase2_cron_quarantined() -> bool"));
}
