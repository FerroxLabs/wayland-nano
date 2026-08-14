//! S8 wave-end audit fix: installed plugins must ACTIVATE (they previously
//! shipped inert — `plugin_mcp_specs`/`plugin_skill_roots` had no
//! production consumer). These tests install a fixture MCP plugin through
//! the real CLI surface (`plugin registry add` / `plugin install`), resolve
//! its specs through the production activation adapter, register them
//! through the SAME registry path the hosts use for config-file servers,
//! and prove the plugin's tool advertises and answers (loopback fake stdio
//! server; the marker file is the external oracle). The exec leg proves the
//! wired bootstrap end to end: the plugin tool reaches the model's
//! advertised surface and the exec gate treats it exactly like any
//! configured mcp__ server (would-prompt ⇒ auto-deny, fail closed).

use crate::exec_mode::{ExecParams, ExecRouting};
use crate::exec_run::run_exec_with;
use crate::plugin_cmds::{plugin_mcp_specs, run as plugin_run};
use nano_agent::loop_protection::ProgressSignals;
use nano_agent::mcp::{McpToolExecutor, SpecSource};
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::permission_mode::PermissionMode;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const IMPLICIT_ROUTING: ExecRouting = ExecRouting {
    mode: nano_session::RoutingMode::ImplicitAliasPassthrough,
    reference: String::new(),
    tools_probe: false,
};

/// A fake MCP stdio server (same JSON-RPC line discipline as nano-mcp's own
/// fixtures): answers the handshake, lists exactly one tool (`ping`), and
/// records every tools/call to the FAKE_MARKER_FILE (the external oracle).
#[cfg(windows)]
const FAKE_SCRIPT: &str = r#"
$reader = [System.Console]::In
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { break }
    if ($line -match '"method"\s*:\s*"initialize"') {
        $id = ($line | ConvertFrom-Json).id
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$id,`"result`":{`"protocolVersion`":`"2025-03-26`",`"capabilities`":{},`"serverInfo`":{`"name`":`"plugfake`",`"version`":`"0`"}}}")
    } elseif ($line -match '"method"\s*:\s*"tools/list"') {
        $id = ($line | ConvertFrom-Json).id
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$id,`"result`":{`"tools`":[{`"name`":`"ping`",`"description`":`"plugin pong`",`"inputSchema`":{`"type`":`"object`",`"properties`":{}}}]}}")
    } elseif ($line -match '"method"\s*:\s*"tools/call"') {
        $id = ($line | ConvertFrom-Json).id
        Add-Content -Path $env:FAKE_MARKER_FILE -Value "tools/call"
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$id,`"result`":{`"content`":`"pong`",`"isError`":false}}")
    }
}
"#;

#[cfg(unix)]
const FAKE_SCRIPT: &str = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *'"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"plugfake","version":"0"}}}
' "$id" ;;
        *'"tools/list"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"ping","description":"plugin pong","inputSchema":{"type":"object","properties":{}}}]}}
' "$id" ;;
        *'"tools/call"'*)
            printf 'tools/call\n' >> "$FAKE_MARKER_FILE"
            printf '{"jsonrpc":"2.0","id":%s,"result":{"content":"pong","isError":false}}
' "$id" ;;
    esac
done
"#;

/// Never routed to: the assertions target the plugin's mcp__ tools.
#[derive(Debug)]
struct NoopTools;

#[async_trait::async_trait]
impl ToolExecutor for NoopTools {
    async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: "should not route here".into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

/// A scripted model that also RECORDS every request (the advertised tool
/// surface is the assertion target). Clones share queue + log.
#[derive(Debug, Default, Clone)]
struct RecordingModel {
    responses: Arc<Mutex<std::collections::VecDeque<Result<ModelResponse, ModelError>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModel {
    fn with(responses: Vec<Result<ModelResponse, ModelError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for RecordingModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text_response("(no more scripted responses)"))
    }
}

fn text_response(text: &str) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![ModelEvent::TextDelta(text.to_string())],
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        stop_reason: "end_turn".into(),
        model: None,
    })
}

fn tool_response(call: ToolCall) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![ModelEvent::ToolCallComplete(call)],
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        stop_reason: "tool_use".into(),
        model: None,
    })
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    marker: PathBuf,
}

/// Builds a local-dir registry carrying one MCP plugin whose stdio server
/// is the fake above, then installs it through the REAL CLI surface
/// (`plugin registry add` + `plugin install --yes`).
fn install_mcp_plugin(tag: &str) -> Fixture {
    // The unix contained spawn (seatbelt/bwrap workspace-write) may only
    // write under the host cwd — fixture scratch anchors under target/,
    // never the OS temp dir (the mcp_tests/exec reap-test precedent).
    let scratch = std::env::current_dir().expect("cwd").join("target");
    std::fs::create_dir_all(&scratch).expect("fixture scratch root");
    let dir = tempfile::Builder::new()
        .prefix(&format!("nano-cli-plugin-activation-{tag}-"))
        .tempdir_in(&scratch)
        .expect("fixture dir");
    let registry = dir.path().join("registry");
    let home = dir.path().join("home");
    let marker = dir.path().join("marker.log");
    std::fs::create_dir_all(registry.join("p")).unwrap();
    std::fs::write(
        registry.join("marketplace.json"),
        r#"{"name":"r","version":1,"plugins":[{"name":"p","version":null,"description":null,"source":{"kind":"path","path":"p"}}]}"#,
    )
    .unwrap();
    #[cfg(windows)]
    let (command, args) = (
        "powershell.exe".to_string(),
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            FAKE_SCRIPT.to_string(),
        ],
    );
    #[cfg(unix)]
    let (command, args) = (
        "sh".to_string(),
        vec!["-c".to_string(), FAKE_SCRIPT.to_string()],
    );
    let plugin = serde_json::json!({
        "name": "p",
        "version": null,
        "description": null,
        "kind": "mcp_server",
        "mcp_server": {
            "name": "plugfake",
            "command": command,
            "args": args,
            "env": { "FAKE_MARKER_FILE": marker.display().to_string() },
        },
    });
    std::fs::write(
        registry.join("p/plugin.json"),
        serde_json::to_vec_pretty(&plugin).unwrap(),
    )
    .unwrap();
    let mut out = Vec::new();
    let add = vec![
        "registry".into(),
        "add".into(),
        "r".into(),
        "local-dir".into(),
        registry.display().to_string(),
    ];
    assert_eq!(plugin_run(&home, &add, &mut out), 0, "registry add");
    let install = vec!["install".into(), "p@r".into(), "--yes".into()];
    assert_eq!(plugin_run(&home, &install, &mut out), 0, "install");
    Fixture {
        _dir: dir,
        home,
        marker,
    }
}

/// The activation core: an installed MCP plugin resolves through the
/// production adapter (Result now — a corrupt store is a typed error, never
/// a silent zero), registers through the SAME registry path as config-file
/// servers, and its tool answers a real call.
#[test]
fn installed_mcp_plugin_registers_and_calls() {
    let fixture = install_mcp_plugin("call");
    let specs = plugin_mcp_specs(&fixture.home).expect("store resolves");
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "plugfake");
    assert_eq!(specs[0].source, SpecSource::Marketplace("r/p".to_string()));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // The SAME registry entry point the host/exec/acp bootstraps use.
        let registry = crate::mcp_specs::register_all_with(specs, None);
        let definitions = registry.tool_definitions();
        assert!(
            definitions.iter().any(|d| d.name == "mcp__plugfake__ping"),
            "plugin tool registered, got {definitions:?}"
        );
        let shared = Arc::new(Mutex::new(registry));
        let executor = McpToolExecutor::from_shared(shared, &NoopTools);
        let outcome = executor
            .execute(&ToolCall {
                id: "c1".into(),
                name: "mcp__plugfake__ping".into(),
                arguments: serde_json::json!({}),
            })
            .await;
        assert!(outcome.ok, "plugin tool callable: {}", outcome.output);
    });
    assert_eq!(
        std::fs::read_to_string(&fixture.marker)
            .expect("marker")
            .trim(),
        "tools/call",
        "the fake server observed the call (external oracle)"
    );
}

/// The exec bootstrap leg: specs resolved from the installed-plugin store
/// (exactly the production `exec_run::run` resolution) reach the run's
/// registry — the plugin tool is ADVERTISED to the model — and the exec
/// gate treats it exactly like any configured mcp__ server (would-prompt ⇒
/// auto-deny on the non-interactive surface, fail closed).
#[test]
fn exec_advertises_and_gates_installed_plugin_tools() {
    let fixture = install_mcp_plugin("exec");
    let specs = plugin_mcp_specs(&fixture.home).expect("store resolves");
    assert_eq!(specs.len(), 1, "the installed plugin resolves");
    let sessions = fixture.home.join("sessions");
    let workspace = fixture.home.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let model = RecordingModel::with(vec![
        tool_response(ToolCall {
            id: "call-1".into(),
            name: "mcp__plugfake__ping".into(),
            arguments: serde_json::json!({}),
        }),
        text_response("done"),
    ]);
    let requests = model.requests.clone();
    let ladder = model.clone();
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = SharedWriter(shared.clone());
    let params = ExecParams {
        prompt: "ping the plugin".into(),
        mode: PermissionMode::Default,
        resume: None,
        output_last_message: None,
        goal: None,
        model: None,
        auto: false,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let exit = runtime.block_on(run_exec_with(
        &sessions,
        &fixture.home,
        &workspace,
        &params,
        "fake-model",
        move || model.clone(),
        move || ladder.clone(),
        |_, _| {
            (
                NoopTools,
                nano_core::permissions::PermissionProfile::workspace_write()
                    .file_system_sandbox_policy(),
            )
        },
        false,
        false,
        &specs,
        &IMPLICIT_ROUTING,
        writer,
    ));
    assert_eq!(exit, 0, "the scripted turn completes (denial is a result)");

    // Advertised: the plugin tool reached the model's tool surface.
    let advertised = requests.lock().unwrap()[0]
        .tools
        .iter()
        .map(|d| d.name.clone())
        .collect::<Vec<_>>();
    assert!(
        advertised.iter().any(|n| n == "mcp__plugfake__ping"),
        "plugin tool advertised, got {advertised:?}"
    );

    // Gated: identical posture to any configured mcp__ server — exec can
    // never prompt, so the call auto-denies with a named event.
    let text = String::from_utf8(shared.lock().unwrap().clone()).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "approval_denied" && e["tool"] == "mcp__plugfake__ping"),
        "plugin mcp__ call auto-denied like any configured server: {text}"
    );
}
