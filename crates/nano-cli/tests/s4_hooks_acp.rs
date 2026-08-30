//! S4 lifecycle hooks on the acp-host surface (F-46) — wire-level
//! integration against the real `acp_mode::serve` host loop (scripted model,
//! recording mock tools, real journal, REAL hook engine over a hooks.toml in
//! the test nano_home):
//! - a configured PreToolUse blocking hook denies the tool call AFTER the
//!   approval gate passed, journaled as HookDecision(PreToolUse, Blocked)
//!   plus the failed ToolResult carrying `hook_blocked` — and the tool
//!   executor never runs;
//! - a gate-denied call (read_only) never reaches the hook at all (the
//!   after-approval ordering, proven negatively);
//! - notify hooks (SessionStart/UserPromptSubmit/PostToolUse) fire and
//!   journal Pass decisions while the turn and the tool complete normally;
//! - a resumed session (fresh host over the same journal) fires SessionStart
//!   "resume" and blocks identically on the next turn;
//! - a broken hooks.toml degrades to warnings + zero hooks — the host
//!   serves, turns complete, and no HookDecision lands.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::acp::AvailableModel;
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── channel-backed streams (the c2_permission_modes pattern) ────────────

struct ChannelReader {
    rx: std::sync::mpsc::Receiver<String>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = {
            let avail = self.fill_buf()?;
            let n = avail.len().min(out.len());
            out[..n].copy_from_slice(&avail[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ChannelReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        while self.pos >= self.buf.len() {
            match self.rx.recv() {
                Ok(line) => {
                    self.buf = line.into_bytes();
                    self.pos = 0;
                }
                Err(_) => return Ok(&[]),
            }
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, amt: usize) {
        self.pos += amt;
    }
}

struct ChannelWriter {
    tx: std::sync::mpsc::Sender<String>,
    buf: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            self.tx
                .send(String::from_utf8_lossy(&line).into_owned())
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

// ── scripted model + recording tools ─────────────────────────────────────

#[derive(Debug, Clone)]
struct MockDriver {
    script: Arc<Mutex<VecDeque<ModelResponse>>>,
    /// Every request the engine made — the hook-block text assertion reads
    /// the model context from here.
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ModelError::Protocol("mock driver script exhausted".into()))
    }
}

#[derive(Debug, Clone, Default)]
struct MockTools {
    /// Tool calls the engine actually DISPATCHED — a hook-blocked call must
    /// never appear here.
    executed: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for MockTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        self.executed.lock().unwrap().push(call.name.clone());
        ToolOutcome {
            ok: true,
            output: format!("ran {}", call.name),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

/// P5: the fail-closed default routing posture for test hosts.
static DEFAULT_ROUTING: nano_cli::auto_routing::RoutingConfig =
    nano_cli::auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: None,
        tools_probe: false,
    };

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::Done {
                stop_reason: "stop".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "stop".into(),
        model: None,
    }
}

fn tool_response(call: ToolCall) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::ToolCallComplete(call),
            ModelEvent::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "tool_calls".into(),
        model: None,
    }
}

fn write_call(dirs: &TestDirs) -> ToolCall {
    ToolCall {
        id: "call-fs_write".into(),
        name: "fs_write".into(),
        arguments: serde_json::json!({"path": dirs.workspace().join("inside.txt"), "content": "x"}),
    }
}

fn workspace_policy() -> nano_core::permissions::FileSystemSandboxPolicy {
    nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy()
}

/// C8: prompt re-resolves the session credential per turn; the scripted
/// driver never networks, but resolution needs SOME flux credential in the
/// process env (the c2 harness discipline).
fn ensure_test_flux_key() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var("FLUX_API_KEY").is_err() {
            unsafe { std::env::set_var("FLUX_API_KEY", "sk-test-harness-never-networked") };
        }
    });
}

// ── hooks.toml fixtures (nano_home = the TestDirs root) ─────────────────

/// Write `<root>/hooks.toml` and load the engine from it, with the unix
/// 0600/ownership discipline the loader enforces.
fn engine_from(root: &std::path::Path, toml: &str) -> nano_hooks::HookEngine {
    let path = root.join("hooks.toml");
    std::fs::write(&path, toml).expect("hooks.toml");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("hooks.toml 0600");
    }
    nano_hooks::HookEngine::load(root)
}

/// A PreToolUse blocking hook on fs_write (exit 2 = blocked on both `cmd`
/// and `sh`) plus a passing SessionStart notify hook.
const BLOCKING_HOOKS: &str = r#"
[[hooks.PreToolUse]]
matcher = "fs_write"
hooks = [{ command = "exit 2" }]

[[hooks.SessionStart]]
hooks = [{ command = "exit 0" }]
"#;

/// Notify-only hooks: SessionStart, UserPromptSubmit, PostToolUse all pass.
const NOTIFY_HOOKS: &str = r#"
[[hooks.SessionStart]]
hooks = [{ command = "exit 0" }]

[[hooks.UserPromptSubmit]]
hooks = [{ command = "exit 0" }]

[[hooks.PostToolUse]]
hooks = [{ command = "exit 0" }]
"#;

// ── the harness ─────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    seen: Vec<serde_json::Value>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
    executed: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    fn spawn(script: Vec<ModelResponse>, dirs: &TestDirs, hooks: nano_hooks::HookEngine) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            requests: model_requests.clone(),
        };
        let tools = MockTools::default();
        let executed = tools.executed.clone();
        let sessions_dir = dirs.sessions();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let catalog = vec![AvailableModel {
                    id: "mock".into(),
                    name: "Mock".into(),
                }];
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                ensure_test_flux_key();
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir.parent().expect("root").join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                };
                let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                    .expect("vendored vision catalog parses");
                let attachment_home = sessions_dir.parent().expect("root");
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir,
                    default_model: "mock",
                    available_models: &catalog,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    sandbox_probe: &sandbox_probe,
                    router: &router,
                    journal_append_failer: None,
                    memory: &memory_config,
                    reasoning_effort: None,
                    verbosity: None,
                    cron_home: None,
                    search: None,
                    search_meter: None,
                    pricing: None,
                    budget_cap: None,
                    vision_catalog: &vision_catalog,
                    attachment_home,
                    hooks: &hooks,
                    routing: &DEFAULT_ROUTING,
                };
                acp_mode::serve_legacy_debug(
                    ChannelReader {
                        rx: in_rx,
                        buf: Vec::new(),
                        pos: 0,
                    },
                    ChannelWriter {
                        tx: out_tx,
                        buf: Vec::new(),
                    },
                    &config,
                    move |_| driver.clone(),
                    move |_, _, _, _, _, _| (tools.clone(), workspace_policy()),
                )
                .await
            })
        });
        Self {
            to_host: Some(in_tx),
            frames: out_rx,
            handle: Some(handle),
            next_id: 0,
            seen: Vec::new(),
            model_requests,
            executed,
        }
    }

    fn send(&mut self, frame: serde_json::Value) {
        self.to_host
            .as_ref()
            .expect("stdin open")
            .send(format!("{}\n", serde_json::to_string(&frame).unwrap()))
            .expect("send to host");
    }

    fn next_frame(&mut self) -> serde_json::Value {
        let line = self
            .frames
            .recv_timeout(TIMEOUT)
            .expect("frame within timeout");
        let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
        self.seen.push(frame.clone());
        frame
    }

    /// Send a request; collect host frames until its response arrives,
    /// auto-answering any session/request_permission with `allow`.
    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        loop {
            let frame = self.next_frame();
            if frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission") {
                let permission_id = frame["id"].as_u64().expect("permission id");
                self.send(serde_json::json!({
                    "jsonrpc": "2.0", "id": permission_id,
                    "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}
                }));
                continue;
            }
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return frame;
            }
        }
    }

    fn new_session(&mut self, cwd: &std::path::Path) -> String {
        self.request("initialize", serde_json::json!({"protocolVersion": 1}));
        let response = self.request(
            "session/new",
            serde_json::json!({"cwd": cwd, "mcpServers": []}),
        );
        response["result"]["sessionId"]
            .as_str()
            .expect("sessionId")
            .to_string()
    }

    fn load_session(&mut self, session_id: &str, cwd: &std::path::Path) -> serde_json::Value {
        self.request("initialize", serde_json::json!({"protocolVersion": 1}));
        self.request(
            "session/load",
            serde_json::json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []}),
        )
    }

    fn set_mode(&mut self, session_id: &str, mode: &str) -> serde_json::Value {
        self.request(
            "session/set_mode",
            serde_json::json!({"sessionId": session_id, "modeId": mode}),
        )
    }

    fn prompt(&mut self, session_id: &str, text: &str) -> serde_json::Value {
        self.request(
            "session/prompt",
            serde_json::json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        )
    }

    /// The model saw a tool result carrying `needle` (the block/denial text
    /// assertion reads the model context, never self-report).
    fn model_saw_tool_result(&self, needle: &str) -> bool {
        let requests = self.model_requests.lock().unwrap();
        requests.iter().any(|request| {
            request.messages.iter().any(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        nano_model::types::ContentBlock::ToolResult { content, .. }
                        if content.contains(needle)
                    )
                })
            })
        })
    }

    fn executed_tools(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }

    fn shutdown(mut self) {
        drop(self.to_host.take());
        if let Some(handle) = self.handle.take() {
            assert_eq!(
                handle.join().expect("host thread").expect("serve io"),
                0,
                "clean exit"
            );
        }
    }
}

/// A temp dir holding the sessions root (= nano_home) and a workspace.
struct TestDirs {
    root: std::path::PathBuf,
}

impl TestDirs {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "nano-s4-hooks-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(root.join("sessions")).expect("sessions dir");
        std::fs::create_dir_all(root.join("workspace")).expect("workspace dir");
        Self { root }
    }

    fn sessions(&self) -> std::path::PathBuf {
        self.root.join("sessions")
    }

    fn workspace(&self) -> std::path::PathBuf {
        self.root.join("workspace")
    }
}

impl Drop for TestDirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ── journal oracles (external state, never self-report) ─────────────────

type HookDecisionRow = (
    nano_session::op::HookEvent,
    nano_session::op::HookOutcome,
    Option<String>,
);

fn hook_decisions(dirs: &TestDirs, session_id: &str) -> Vec<HookDecisionRow> {
    let journal = dirs.sessions().join(format!("{session_id}.jsonl"));
    let report = nano_session::reader::read_journal(&journal).expect("journal reads");
    report
        .envelopes
        .iter()
        .filter_map(|e| match &e.op {
            nano_session::op::Op::HookDecision {
                event,
                outcome,
                matcher_input,
                ..
            } => Some((*event, *outcome, matcher_input.clone())),
            _ => None,
        })
        .collect()
}

type ToolResultRow = (bool, Option<nano_session::NanoErrorKind>);

fn tool_results(dirs: &TestDirs, session_id: &str) -> Vec<ToolResultRow> {
    let journal = dirs.sessions().join(format!("{session_id}.jsonl"));
    let report = nano_session::reader::read_journal(&journal).expect("journal reads");
    report
        .envelopes
        .iter()
        .filter_map(|e| match &e.op {
            nano_session::op::Op::ToolResult { ok, error_kind, .. } => Some((*ok, *error_kind)),
            _ => None,
        })
        .collect()
}

// ── the battery ─────────────────────────────────────────────────────────

#[test]
fn pre_tool_use_blocks_after_approval_and_journals_hook_blocked() {
    let dirs = TestDirs::new();
    let engine = engine_from(&dirs.root, BLOCKING_HOOKS);
    assert!(
        !engine.is_empty(),
        "hooks.toml must load: {:?}",
        engine.warnings()
    );
    let mut host = Harness::spawn(
        vec![
            tool_response(write_call(&dirs)),
            text_response("done one"),
            tool_response(write_call(&dirs)),
            text_response("done two"),
        ],
        &dirs,
        engine,
    );
    let session = host.new_session(&dirs.workspace());

    // Turn 1 (default mode): the gate passes the contained write, THEN the
    // PreToolUse hook blocks it.
    let response = host.prompt(&session, "write a file");
    assert_eq!(response["result"]["stopReason"], "end_turn");

    let decisions = hook_decisions(&dirs, &session);
    assert!(
        decisions.iter().any(|(event, outcome, matcher)| {
            *event == nano_session::op::HookEvent::PreToolUse
                && *outcome == nano_session::op::HookOutcome::Blocked
                && matcher.as_deref() == Some("fs_write")
        }),
        "journaled PreToolUse Blocked decision: {decisions:?}"
    );
    let results = tool_results(&dirs, &session);
    assert!(
        results
            .iter()
            .any(|(ok, kind)| { !ok && *kind == Some(nano_session::NanoErrorKind::HookBlocked) }),
        "journaled hook_blocked ToolResult: {results:?}"
    );
    assert!(
        !results.iter().any(|(ok, _)| *ok),
        "the blocked call never produced a successful result: {results:?}"
    );
    assert!(
        host.executed_tools().is_empty(),
        "the tool executor never ran the blocked call: {:?}",
        host.executed_tools()
    );
    assert!(
        host.model_saw_tool_result("blocked by lifecycle hook"),
        "the model context carries the block text so it stops retrying"
    );

    // Turn 2 (read_only): the GATE denies first — the hook must NOT fire
    // (the after-approval ordering, proven negatively).
    host.set_mode(&session, "read_only");
    let pre_tool_use_before = hook_decisions(&dirs, &session)
        .iter()
        .filter(|(event, _, _)| *event == nano_session::op::HookEvent::PreToolUse)
        .count();
    let response = host.prompt(&session, "write a file again");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let decisions = hook_decisions(&dirs, &session);
    assert_eq!(
        decisions
            .iter()
            .filter(|(event, _, _)| *event == nano_session::op::HookEvent::PreToolUse)
            .count(),
        pre_tool_use_before,
        "no NEW PreToolUse decision after the gate denial: {decisions:?}"
    );
    let results = tool_results(&dirs, &session);
    assert!(
        results.iter().any(|(ok, kind)| {
            !ok && *kind == Some(nano_session::NanoErrorKind::ApprovalDenied)
        }),
        "the gate denial is the journaled shape, not a hook block: {results:?}"
    );
    host.shutdown();
}

#[test]
fn notify_hooks_fire_and_journal_while_the_turn_completes() {
    let dirs = TestDirs::new();
    let engine = engine_from(&dirs.root, NOTIFY_HOOKS);
    assert!(
        !engine.is_empty(),
        "hooks.toml must load: {:?}",
        engine.warnings()
    );
    let mut host = Harness::spawn(
        vec![tool_response(write_call(&dirs)), text_response("done")],
        &dirs,
        engine,
    );
    let session = host.new_session(&dirs.workspace());

    // SessionStart fired at session/new with the "startup" source.
    let decisions = hook_decisions(&dirs, &session);
    assert!(
        decisions.iter().any(|(event, outcome, matcher)| {
            *event == nano_session::op::HookEvent::SessionStart
                && *outcome == nano_session::op::HookOutcome::Pass
                && matcher.as_deref() == Some("startup")
        }),
        "journaled SessionStart(startup) Pass decision: {decisions:?}"
    );

    let response = host.prompt(&session, "write a file");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let decisions = hook_decisions(&dirs, &session);
    for (expected, name) in [
        (
            nano_session::op::HookEvent::UserPromptSubmit,
            "UserPromptSubmit",
        ),
        (nano_session::op::HookEvent::PostToolUse, "PostToolUse"),
    ] {
        assert!(
            decisions.iter().any(|(event, outcome, _)| {
                *event == expected && *outcome == nano_session::op::HookOutcome::Pass
            }),
            "journaled {name} Pass decision: {decisions:?}"
        );
    }
    // Notify hooks never interrupted the happy path: the tool RAN and the
    // result is a plain success.
    assert_eq!(host.executed_tools(), vec!["fs_write".to_string()]);
    let results = tool_results(&dirs, &session);
    assert!(
        results.iter().any(|(ok, kind)| *ok && kind.is_none()),
        "successful ToolResult journaled beside the notify decisions: {results:?}"
    );
    host.shutdown();
}

#[test]
fn hooks_fire_identically_on_a_resumed_session() {
    let dirs = TestDirs::new();
    let session = {
        let engine = engine_from(&dirs.root, BLOCKING_HOOKS);
        let mut host = Harness::spawn(
            vec![tool_response(write_call(&dirs)), text_response("done one")],
            &dirs,
            engine,
        );
        let session = host.new_session(&dirs.workspace());
        let response = host.prompt(&session, "write a file");
        assert_eq!(response["result"]["stopReason"], "end_turn");
        host.shutdown();
        session
    };

    // A FRESH host over the same journal (the crash/restart shape): reload
    // the engine from the same hooks.toml, resume, and prompt again.
    let engine = engine_from(&dirs.root, BLOCKING_HOOKS);
    let mut host = Harness::spawn(
        vec![tool_response(write_call(&dirs)), text_response("done two")],
        &dirs,
        engine,
    );
    let response = host.load_session(&session, &dirs.workspace());
    assert!(response.get("result").is_some(), "resume: {response}");
    let response = host.prompt(&session, "write a file");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    host.shutdown();

    let decisions = hook_decisions(&dirs, &session);
    // SessionStart fired on BOTH the startup and the resume.
    for source in ["startup", "resume"] {
        assert!(
            decisions.iter().any(|(event, outcome, matcher)| {
                *event == nano_session::op::HookEvent::SessionStart
                    && *outcome == nano_session::op::HookOutcome::Pass
                    && matcher.as_deref() == Some(source)
            }),
            "journaled SessionStart({source}) Pass decision: {decisions:?}"
        );
    }
    // The blocking hook fired identically on both turns.
    assert_eq!(
        decisions
            .iter()
            .filter(|(event, outcome, _)| {
                *event == nano_session::op::HookEvent::PreToolUse
                    && *outcome == nano_session::op::HookOutcome::Blocked
            })
            .count(),
        2,
        "one PreToolUse Blocked decision per turn: {decisions:?}"
    );
    let results = tool_results(&dirs, &session);
    assert_eq!(
        results
            .iter()
            .filter(|(ok, kind)| { !ok && *kind == Some(nano_session::NanoErrorKind::HookBlocked) })
            .count(),
        2,
        "one hook_blocked ToolResult per turn: {results:?}"
    );
    assert!(
        !results.iter().any(|(ok, _)| *ok),
        "no blocked call ever executed: {results:?}"
    );
}

#[test]
fn broken_hooks_toml_degrades_to_warnings_not_death() {
    let dirs = TestDirs::new();
    let engine = engine_from(&dirs.root, "this is not [[valid toml");
    assert!(engine.is_empty(), "a broken config loads ZERO hooks");
    assert_eq!(engine.warnings().len(), 1, "exactly one loud warning");
    let mut host = Harness::spawn(
        vec![tool_response(write_call(&dirs)), text_response("done")],
        &dirs,
        engine,
    );
    let session = host.new_session(&dirs.workspace());
    let response = host.prompt(&session, "write a file");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    // Zero hooks = the pre-S4 behavior exactly: the tool runs, the turn
    // completes, and no HookDecision lands in the journal.
    assert_eq!(host.executed_tools(), vec!["fs_write".to_string()]);
    assert!(
        hook_decisions(&dirs, &session).is_empty(),
        "no hook decisions with a degraded engine"
    );
    host.shutdown();
}
