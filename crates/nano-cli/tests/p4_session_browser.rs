use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelRequest, ModelResponse, ToolCall};
use nano_protocol::acp::AvailableModel;
use nano_session::{Op, OpEnvelope};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct UnusedDriver;

#[async_trait::async_trait]
impl ModelDriver for UnusedDriver {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Protocol(
            "session-browser harness unexpectedly reached the model".into(),
        ))
    }
}

#[derive(Clone, Debug)]
struct UnusedTools;

#[async_trait::async_trait]
impl ToolExecutor for UnusedTools {
    async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: "session-browser harness unexpectedly reached a tool".into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

static DEFAULT_ROUTING: nano_cli::auto_routing::RoutingConfig =
    nano_cli::auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: None,
        tools_probe: false,
    };

fn debug_exchange(home: &std::path::Path, frames: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let input = frames
        .iter()
        .map(|frame| format!("{frame}\n"))
        .collect::<String>();
    let output = SharedWriter::default();
    let captured = output.clone();
    let sessions_dir = home.join("sessions");
    let memory = acp_mode::MemoryHostConfig {
        dir: home.join("memory"),
        write_enabled: false,
        block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
        policy: nano_cli::memory_policy::ResolvedMemoryPolicy::disabled(),
    };
    let available_models = [AvailableModel {
        id: "mock".into(),
        name: "mock".into(),
    }];
    let sandbox_probe = || true;
    let router = nano_cli::provider_router::ProviderRouter::default();
    let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
        .expect("vendored vision catalog parses");
    let hooks = nano_hooks::HookEngine::empty();
    let config = acp_mode::ServeConfig {
        sessions_dir: &sessions_dir,
        default_model: "mock",
        available_models: &available_models,
        env_mcp_specs: &[],
        catalog: &[],
        window_override: None,
        limit_override: None,
        sandbox_probe: &sandbox_probe,
        router: &router,
        journal_append_failer: None,
        memory: &memory,
        reasoning_effort: None,
        verbosity: None,
        cron_home: None,
        search: None,
        search_meter: None,
        pricing: None,
        budget_cap: None,
        vision_catalog: &vision_catalog,
        attachment_home: home,
        hooks: &hooks,
        routing: &DEFAULT_ROUTING,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = runtime.block_on(acp_mode::serve_legacy_debug(
        std::io::Cursor::new(input),
        output,
        &config,
        |_| UnusedDriver,
        |_, _, _, _, _, _| {
            (
                UnusedTools,
                nano_core::permissions::PermissionProfile::workspace_write()
                    .file_system_sandbox_policy(),
            )
        },
    ));
    assert_eq!(result.unwrap(), 0);
    let bytes = captured.0.lock().unwrap().clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn write_journal(path: &std::path::Path, session_id: &str, cwd: &str) {
    let first = serde_json::to_string(&OpEnvelope::new(
        format!("begin-{session_id}"),
        "2026-08-13T00:00:00Z",
        Op::SessionBegin {
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
        },
    ))
    .unwrap();
    std::fs::write(path, format!("{first}\n")).unwrap();
}

#[test]
fn headless_sessions_prints_the_shared_summary_fields() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    write_journal(&sessions.join("s1.jsonl"), "s1", "C:/workspace");

    let output = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("sessions")
        .env("NANO_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let fields: Vec<&str> = stdout.trim().split('\t').collect();
    assert_eq!(fields.len(), 5, "id/cwd/mtime/size/status: {stdout}");
    assert_eq!(fields[0], "s1");
    assert_eq!(fields[1], "C:/workspace");
    assert_eq!(fields[4], "Closed");
}

#[test]
fn corrupt_session_load_keeps_existing_journal_unavailable_kind() {
    let Some(flux_key) = std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|key| !key.is_empty())
    else {
        eprintln!("FLUX_TEST_KEY not set — skipping ACP process load assertion");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("corrupt.jsonl"), "not-json\n{}\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("acp-host")
        .env("NANO_HOME", home.path())
        .env("FLUX_API_KEY", flux_key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":1,"clientCapabilities":{}}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let initialize: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert!(
        initialize.get("result").is_some(),
        "initialize: {initialize}"
    );

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"session/load",
            "params":{"sessionId":"corrupt","cwd":".","mcpServers":[]}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let load: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        load["error"]["data"]["nanoError"]["kind"],
        "journal_unavailable"
    );

    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// RC2 wiring (P4 session browser §4): `_wayland/session/list` is served
/// with or without an active session (the listing is global), advertised
/// in nanoExtensions, and io failures map to the typed
/// journal_unavailable kind.
#[test]
fn session_list_extension_round_trip_without_an_active_session() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    write_journal(&sessions.join("s1.jsonl"), "s1", "C:/workspace");

    let responses = debug_exchange(
        home.path(),
        &[
            serde_json::json!({
                "jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":1,"clientCapabilities":{}}
            }),
            serde_json::json!({
                "jsonrpc":"2.0","id":2,"method":"_wayland/session/list","params":{}
            }),
        ],
    );
    let initialize = &responses[0];
    assert_eq!(
        initialize["result"]["agentCapabilities"]["nanoExtensions"]["_wayland/session/list"]["version"],
        1,
        "advertised: {initialize}"
    );

    // No session/new — the listing is global and must still answer.
    let list = &responses[1];
    assert_eq!(list["result"]["sessions"][0]["sessionId"], "s1", "{list}");
    assert_eq!(list["result"]["sessions"][0]["status"], "closed", "{list}");
    assert!(
        list["result"]["liveStatusCaveat"]
            .as_str()
            .unwrap()
            .contains("point-in-time"),
        "{list}"
    );

    // io failure (sessions dir replaced by a regular file) ⇒ typed
    // journal_unavailable, never a panic or an empty ok.
    std::fs::remove_dir_all(&sessions).unwrap();
    std::fs::write(&sessions, "not a dir").unwrap();
    let responses = debug_exchange(
        home.path(),
        &[serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"_wayland/session/list","params":{}
        })],
    );
    let list = &responses[0];
    assert_eq!(
        list["error"]["data"]["nanoError"]["kind"], "journal_unavailable",
        "{list}"
    );
}
