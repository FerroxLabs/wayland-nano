use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::types::{ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
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

#[test]
fn authenticated_entrypoints_use_one_seam_and_keep_legacy_tools_denied() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let acp = std::fs::read_to_string(root.join("src/acp_mode.rs")).unwrap();
    let host = std::fs::read_to_string(root.join("src/host_mode.rs")).unwrap();
    let exec = std::fs::read_to_string(root.join("src/exec_run.rs")).unwrap();
    let seam = std::fs::read_to_string(root.join("src/memory_seam.rs")).unwrap();

    assert!(acp.contains("start_for_activation"));
    assert!(host.contains("start_for_activation"));
    assert!(exec.contains("start_for_activation"));
    for legacy in ["memory_list", "memory_read", "memory_save", "memory_delete"] {
        assert!(seam.contains(legacy));
    }
    assert!(seam.contains("NanoErrorKind::UnknownTool"));
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

#[derive(Debug)]
enum EphemeralModel {
    Text,
    PersistentTool,
}

#[async_trait::async_trait]
impl ModelDriver for EphemeralModel {
    async fn complete(
        &self,
        request: &ModelRequest,
    ) -> Result<ModelResponse, nano_model::types::ModelError> {
        assert!(request.tools.is_empty());
        let events = match self {
            Self::Text => vec![
                ModelEvent::TextDelta("ephemeral answer".into()),
                ModelEvent::Done {
                    stop_reason: "end_turn".into(),
                },
            ],
            Self::PersistentTool => vec![ModelEvent::ToolCallComplete(call(
                "memory_save",
                serde_json::json!({"text":"SECRET-CANARY"}),
            ))],
        };
        Ok(ModelResponse {
            events,
            usage: Usage::default(),
            stop_reason: "end_turn".into(),
            model: None,
        })
    }
}

#[tokio::test]
async fn nonpersistent_prompt_is_in_memory_and_persistent_tool_is_typed_refused() {
    let session_id = format!("volatile-{}-1", std::process::id());
    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/new\",\"params\":{{\"cwd\":\".\"}}}}\n{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/prompt\",\"params\":{{\"sessionId\":\"{session_id}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"hello\"}}]}}}}\n"
    );
    let mut output = Vec::new();
    nano_cli::acp_mode::serve_nonpersistent(
        std::io::Cursor::new(input),
        &mut output,
        "flux-auto",
        &[],
        &EphemeralModel::Text,
    )
    .await
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("ephemeral answer"));
    assert!(output.contains("end_turn"));
    assert!(!output.contains("SECRET-CANARY"));

    let mut refused = Vec::new();
    nano_cli::acp_mode::serve_nonpersistent(
        std::io::Cursor::new(format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/new\",\"params\":{{}}}}\n{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/prompt\",\"params\":{{\"sessionId\":\"{session_id}\",\"prompt\":[{{\"type\":\"text\",\"text\":\"persist\"}}]}}}}\n"
        )),
        &mut refused,
        "flux-auto",
        &[],
        &EphemeralModel::PersistentTool,
    )
    .await
    .unwrap();
    let refused = String::from_utf8(refused).unwrap();
    assert!(refused.contains("unknown_tool"));
    assert!(refused.contains("exposes no tools or persistent effects"));
    assert!(!refused.contains("SECRET-CANARY"));
}

#[test]
fn explicit_nonpersistent_process_launches_without_carrier_and_writes_no_state() {
    let home = tempfile::tempdir().unwrap();
    seed_legacy_state(home.path());
    let before = inventory(home.path());
    let key_home = tempfile::tempdir().unwrap();
    let key = key_home.path().join("flux.key");
    std::fs::write(&key, "test-only-flux-key").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(["acp-host", "--nonpersistent"])
        .env("NANO_HOME", home.path())
        .env("FLUX_API_KEY_FILE", &key)
        .env("NANO_MEMORY_WRITE", "true")
        .env("NANO_CRON_ENABLED", "true")
        .env("NANO_HOOKS_ENABLED", "true")
        .current_dir(home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut frame = String::new();
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{}}}}"
    )
    .unwrap();
    stdin.flush().unwrap();
    std::io::BufRead::read_line(&mut stdout, &mut frame).unwrap();
    assert!(frame.contains("wayland-nano-nonpersistent"));
    frame.clear();
    writeln!(
        stdin,
        "{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/new\",\"params\":{{\"cwd\":\".\"}}}}"
    )
    .unwrap();
    stdin.flush().unwrap();
    std::io::BufRead::read_line(&mut stdout, &mut frame).unwrap();
    let created: serde_json::Value = serde_json::from_str(&frame).unwrap();
    let session_id = created["result"]["sessionId"].as_str().unwrap();
    assert!(session_id.starts_with("volatile-"));
    let prompt = serde_json::json!({
        "jsonrpc":"2.0", "id":3, "method":"session/prompt",
        "params":{"sessionId":session_id,"prompt":[]}
    });
    writeln!(stdin, "{}", serde_json::to_string(&prompt).unwrap()).unwrap();
    stdin.flush().unwrap();
    frame.clear();
    std::io::BufRead::read_line(&mut stdout, &mut frame).unwrap();
    assert!(frame.contains("accepts text prompts only"));
    writeln!(stdin, "{{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"session/load\",\"params\":{{\"sessionId\":\"main\"}}}}")
        .unwrap();
    stdin.flush().unwrap();
    frame.clear();
    std::io::BufRead::read_line(&mut stdout, &mut frame).unwrap();
    assert!(frame.contains("sessions cannot be loaded"));
    drop(stdin);
    let mut remainder = String::new();
    std::io::Read::read_to_string(&mut stdout, &mut remainder).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(remainder.is_empty());
    assert_eq!(inventory(home.path()), before);
}
