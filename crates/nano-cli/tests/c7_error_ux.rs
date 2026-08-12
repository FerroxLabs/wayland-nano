//! C7 §8 wire-level tests: typed errors end-to-end through the REAL
//! `acp_mode::serve` host loop (streams injected, scripted model/tools — no
//! Flux key, no network, no child process).
//!
//! Covers, per the certified design:
//! - every ModelError variant → a JSON-RPC error RESPONSE (not a
//!   stopReason:"error" result) with the table's code + data.nanoError.kind
//!   and NO provider free-text (canary + escape payloads);
//! - tool failures → tool_call_update failed cards with the `content`
//!   presentation and `_meta.nanoError`;
//! - gate denial → ToolCall journaled+framed BEFORE the prompt, a failed
//!   ToolResult with approval_denied, and the turn continues;
//! - journal-append failure mid-turn → fail-closed journal_unavailable and
//!   no unjournaled live frame;
//! - engine stops (repeat force-stop, budget) are typed errors — never
//!   mislabeled stopReason:"cancelled";
//! - partial-stream-then-fail keeps the streamed text, error appended after;
//! - replay compat: old journals (no error_kind) load unchanged, new
//!   journals replay failed cards with their kind, and resume context shows
//!   `<presentation> [output elided]`.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::acp::AvailableModel;
use nano_session::NanoErrorKind;
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── channel-backed streams (same pattern as c1_compaction.rs) ───────────

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

// ── scripted model + tools ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MockDriver {
    /// Exact sequence of complete() outcomes, in call order (compaction
    /// summarization calls consume entries too).
    script: Arc<Mutex<VecDeque<Result<ModelResponse, ModelError>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        // Yield so the serve loop's biased select can process inbound
        // frames (e.g. a second prompt) BETWEEN steps — without this the
        // scripted turn completes in one poll and mid-turn request tests
        // race.
        tokio::task::yield_now().await;
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock driver script exhausted")
    }
}

#[derive(Debug, Clone)]
struct MockTools {
    outcomes: Arc<Mutex<VecDeque<ToolOutcome>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for MockTools {
    async fn execute(&self, _call: &ToolCall) -> ToolOutcome {
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock tools script exhausted")
    }
}

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
    }
}

/// A response carrying BOTH text and a tool call (the partial-stream case:
/// text is journaled + the loop continues to the next model call).
fn text_and_tool_response(text: &str, call: ToolCall) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::ToolCallComplete(call),
            ModelEvent::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "tool_calls".into(),
    }
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

fn ok_outcome() -> ToolOutcome {
    ToolOutcome {
        ok: true,
        output: "ok".into(),
        progress: ProgressSignals {
            new_information: true,
            ..Default::default()
        },
        error_kind: None,
    }
}

fn fail_outcome(kind: NanoErrorKind) -> ToolOutcome {
    ToolOutcome {
        ok: false,
        output: format!("mock failure for {kind:?}"),
        progress: ProgressSignals::default(),
        error_kind: Some(kind),
    }
}

// ── harness ──────────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
    sessions_dir: std::path::PathBuf,
}

fn temp_sessions_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nano-c7-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp sessions dir");
    dir
}

impl Harness {
    fn spawn(
        script: Vec<Result<ModelResponse, ModelError>>,
        outcomes: Vec<ToolOutcome>,
        sessions_dir: &std::path::Path,
        new_session: bool,
        journal_failer: Option<Arc<AtomicBool>>,
    ) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            requests: requests.clone(),
        };
        let tools = MockTools {
            outcomes: Arc::new(Mutex::new(outcomes.into())),
        };
        let sessions_dir_owned = sessions_dir.to_path_buf();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let catalog = vec![AvailableModel {
                    id: "mock".into(),
                    name: "mock".into(),
                }];
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                ensure_test_flux_key();
                let failer_closure = journal_failer.map(|flag| move || flag.load(Ordering::SeqCst));
                let failer_ref: Option<&(dyn Fn() -> bool + Send + Sync)> = failer_closure
                    .as_ref()
                    .map(|f| f as &(dyn Fn() -> bool + Send + Sync));
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir_owned.parent().expect("root").join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                };
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir_owned,
                    default_model: "mock",
                    available_models: &catalog,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    sandbox_probe: &sandbox_probe,
                    router: &router,
                    journal_append_failer: failer_ref,
                    memory: &memory_config,
                    reasoning_effort: None,
                    verbosity: None,
                    cron_home: None,
                    pricing: None,
                    budget_cap: None,
                };
                acp_mode::serve(
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
                    move |_, _, _, _| {
                        (
                            tools.clone(),
                            nano_core::permissions::PermissionProfile::workspace_write()
                                .file_system_sandbox_policy(),
                        )
                    },
                )
                .await
            })
        });
        let mut harness = Self {
            to_host: Some(in_tx),
            frames: out_rx,
            requests,
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
            sessions_dir: sessions_dir.to_path_buf(),
        };
        harness.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } }
            }),
            "allow",
        );
        if new_session {
            let created = harness.request(
                "session/new",
                serde_json::json!({ "cwd": std::env::temp_dir().to_string_lossy(), "mcpServers": [] }),
                "allow",
            );
            harness.session_id = created["result"]["sessionId"]
                .as_str()
                .expect("sessionId")
                .to_string();
        }
        harness
    }

    fn send(&mut self, frame: &serde_json::Value) {
        let tx = self.to_host.as_ref().expect("stdin open");
        tx.send(serde_json::to_string(frame).unwrap() + "\n")
            .expect("send");
    }

    fn next_frame(&self) -> serde_json::Value {
        let line = self
            .frames
            .recv_timeout(TIMEOUT)
            .expect("frame within timeout");
        serde_json::from_str(&line).expect("frame json")
    }

    /// Send a request; collect session/update notifications until its
    /// response arrives, answering any session/request_permission with
    /// `permission_answer`.
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
        permission_answer: &str,
    ) -> serde_json::Value {
        self.request_collecting(method, params, permission_answer).0
    }

    fn request_collecting(
        &mut self,
        method: &str,
        params: serde_json::Value,
        permission_answer: &str,
    ) -> (serde_json::Value, Vec<serde_json::Value>) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        let mut updates = Vec::new();
        loop {
            let frame = self.next_frame();
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return (frame, updates);
            }
            if frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission") {
                let permission_id = frame["id"].as_u64().expect("permission id");
                // C7/D4 journal-first proof: by the time the permission
                // prompt arrives, the ToolCall op is ALREADY durable.
                if let Some(journal) = self.journal_ops() {
                    assert!(
                        journal.iter().any(|op| op.contains("\"tool_call\"")),
                        "permission prompt arrived before the ToolCall was journaled"
                    );
                }
                self.send(&serde_json::json!({
                    "jsonrpc": "2.0", "id": permission_id,
                    "result": {"outcome": {"outcome": "selected", "optionId": permission_answer}}
                }));
                continue;
            }
            if frame.get("method").and_then(|m| m.as_str()) == Some("session/update") {
                updates.push(frame);
            }
        }
    }

    fn prompt(
        &mut self,
        text: &str,
        permission_answer: &str,
    ) -> (serde_json::Value, Vec<serde_json::Value>) {
        self.request_collecting(
            "session/prompt",
            serde_json::json!({
                "sessionId": self.session_id,
                "prompt": [{ "type": "text", "text": text }]
            }),
            permission_answer,
        )
    }

    /// The journaled ops as raw JSON strings (None when the journal does
    /// not exist yet).
    fn journal_ops(&self) -> Option<Vec<String>> {
        let path = self.sessions_dir.join(format!("{}.jsonl", self.session_id));
        let bytes = std::fs::read_to_string(path).ok()?;
        Some(bytes.lines().map(|l| l.to_string()).collect())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.to_host.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Every UI-bound frame line of a finished prompt, serialized — the canary
/// assertion scans this.
fn updates_blob(updates: &[serde_json::Value]) -> String {
    updates
        .iter()
        .map(|u| serde_json::to_string(u).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

// ── model family ─────────────────────────────────────────────────────────

/// C7 §8: every ModelError variant surfaces as a typed error RESPONSE with
/// the table's code/kind/retryable — and provider free-text (canary +
/// escape-laden) never appears in ANY UI-bound frame.
#[test]
fn model_errors_surface_as_typed_error_responses_without_provider_text() {
    let canary = "CANARY-provider-prose-\u{1b}[31m";
    let cases: Vec<(ModelError, &str, bool)> = vec![
        (
            ModelError::Auth {
                message: format!("bad key: {canary}"),
                status: Some(401),
            },
            "model_auth",
            false,
        ),
        (
            ModelError::RateLimited {
                retry_after_ms: Some(1500),
            },
            "model_rate_limited",
            true,
        ),
        (
            ModelError::Entitlement(format!("plan: {canary}")),
            "model_entitlement",
            false,
        ),
        (
            ModelError::Server {
                status: 503,
                message: format!("provider body: {canary}"),
            },
            "model_server_5xx",
            true,
        ),
        (
            ModelError::Server {
                status: 422,
                message: format!("provider body: {canary}"),
            },
            "model_server_4xx",
            false,
        ),
        (
            ModelError::Transport {
                phase: nano_model::types::TransportPhase::Connect,
                message: format!("reset: {canary}"),
            },
            "model_transport",
            true,
        ),
        (
            ModelError::Protocol(format!("bad json: {canary}")),
            "model_protocol",
            false,
        ),
        (
            ModelError::Egress(nano_egress::client::EgressError::Denied {
                method: "POST".into(),
                host: "api.example.com".into(),
                digest: "x".into(),
            }),
            "egress_denied",
            false,
        ),
    ];
    for (err, expected_kind, expected_retryable) in cases {
        let dir = temp_sessions_dir("model");
        let mut harness = Harness::spawn(vec![Err(err)], vec![], &dir, true, None);
        let (response, updates) = harness.prompt("hi", "allow");
        assert!(
            response.get("result").is_none(),
            "turn-fatal failures are error responses, not results: {response}"
        );
        assert_eq!(response["error"]["code"], -32603, "{expected_kind}");
        let nano = &response["error"]["data"]["nanoError"];
        assert_eq!(nano["kind"], expected_kind);
        assert_eq!(nano["retryable"], expected_retryable);
        // The wire message is the static table presentation.
        let message = response["error"]["message"].as_str().unwrap();
        assert!(
            !message.contains("CANARY"),
            "provider text leaked: {message}"
        );
        // The typed extras for the families that carry them.
        match expected_kind {
            "model_rate_limited" => assert_eq!(nano["retry_after_ms"], 1500),
            "model_server_5xx" => assert_eq!(nano["status"], 503),
            "model_server_4xx" => assert_eq!(nano["status"], 422),
            "egress_denied" => assert_eq!(nano["host"], "api.example.com"),
            _ => {}
        }
        // Canary discipline: no provider text in ANY UI-bound frame.
        let blob = format!(
            "{}\n{}",
            updates_blob(&updates),
            serde_json::to_string(&response).unwrap()
        );
        assert!(!blob.contains("CANARY"), "canary leaked: {blob}");
        assert!(!blob.contains('\u{1b}'), "escape leaked: {blob}");
    }
}

/// The second context overflow (after one reactive compaction) surfaces as
/// model_context_overflow; a FAILED reactive compaction surfaces as
/// compaction_failed.
#[test]
fn overflow_and_compaction_failures_are_distinct_kinds() {
    // (a) overflow → compaction succeeds → overflow again → terminal.
    let dir = temp_sessions_dir("overflow");
    let mut harness = Harness::spawn(
        vec![
            Err(ModelError::ContextOverflow("too long".into())),
            Ok(text_response("summary")),
            Err(ModelError::ContextOverflow("still too long".into())),
        ],
        vec![],
        &dir,
        true,
        None,
    );
    let (response, _) = harness.prompt("big", "allow");
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "model_context_overflow",
        "{response}"
    );

    // (b) overflow → the compaction call itself fails → compaction_failed.
    let dir = temp_sessions_dir("compactfail");
    let mut harness = Harness::spawn(
        vec![
            Err(ModelError::ContextOverflow("too long".into())),
            Err(ModelError::Server {
                status: 500,
                message: "summary model died".into(),
            }),
        ],
        vec![],
        &dir,
        true,
        None,
    );
    let (response, _) = harness.prompt("big", "allow");
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "compaction_failed",
        "{response}"
    );
}

// ── tool family ──────────────────────────────────────────────────────────

/// C7 §8/D3: failed tool calls carry status:"failed", the ACP-spec content
/// presentation, and _meta.nanoError — and rawOutput keeps the digest.
#[test]
fn tool_failures_carry_typed_cards() {
    let kinds = [
        (NanoErrorKind::FsEdit, "fs_edit"),
        (NanoErrorKind::ShellSpawn, "shell_spawn"),
        (NanoErrorKind::SandboxUnavailable, "sandbox_unavailable"),
        (NanoErrorKind::McpTimeout, "mcp_timeout"),
        (NanoErrorKind::UnknownTool, "unknown_tool"),
    ];
    for (kind, expected) in kinds {
        let dir = temp_sessions_dir("toolfail");
        let mut harness = Harness::spawn(
            vec![
                Ok(tool_response(call(
                    "c1",
                    "fs_edit",
                    serde_json::json!({"path": "a", "old_string": "x", "new_string": "y"}),
                ))),
                Ok(text_response("done")),
            ],
            vec![fail_outcome(kind)],
            &dir,
            true,
            None,
        );
        let (response, updates) = harness.prompt("edit", "allow");
        assert_eq!(response["result"]["stopReason"], "end_turn", "{response}");
        let card = updates
            .iter()
            .find(|u| u["params"]["update"]["sessionUpdate"] == "tool_call_update")
            .expect("tool_call_update frame");
        let update = &card["params"]["update"];
        assert_eq!(update["status"], "failed");
        assert_eq!(update["_meta"]["nanoError"]["kind"], expected);
        let presentation = update["content"][0]["content"]["text"]
            .as_str()
            .expect("content presentation");
        assert_eq!(
            presentation,
            nano_protocol::acp::error_presentation(kind),
            "{expected}"
        );
        assert!(
            update["rawOutput"].as_str().unwrap().starts_with("len:"),
            "rawOutput keeps the digest: {update}"
        );
        // The mock failure detail string never reaches the frame.
        assert!(
            !serde_json::to_string(&card)
                .unwrap()
                .contains("mock failure")
        );
    }
}

/// The REAL executor's policy denials map through the variant mappers: a
/// sensitive-path read is fs_sensitive_denied on the wire.
#[test]
fn real_executor_denial_maps_through() {
    let dir = temp_sessions_dir("realdeny");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
    let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
    let sessions_dir = dir.clone();
    let workspace_clone = workspace.clone();
    let handle = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        runtime.block_on(async move {
            let catalog = vec![AvailableModel {
                id: "mock".into(),
                name: "mock".into(),
            }];
            let sandbox_probe = || true;
            let router = nano_cli::provider_router::ProviderRouter::default();
            ensure_test_flux_key();
            let memory_config = acp_mode::MemoryHostConfig {
                dir: sessions_dir.parent().expect("root").join("memory"),
                write_enabled: false,
                block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
            };
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
                pricing: None,
                budget_cap: None,
            };
            let driver = MockDriver {
                script: Arc::new(Mutex::new(
                    vec![
                        Ok(tool_response(call(
                            "c1",
                            "fs_read",
                            serde_json::json!({"path": ".env"}),
                        ))),
                        Ok(text_response("cannot read that")),
                    ]
                    .into(),
                )),
                requests: Arc::new(Mutex::new(Vec::new())),
            };
            let home = home.clone();
            acp_mode::serve(
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
                move |_, _, _, _| {
                    let policy = nano_core::permissions::PermissionProfile::workspace_write()
                        .file_system_sandbox_policy();
                    let fs = nano_tools::fs::FsTools::new(policy.clone(), &workspace_clone);
                    let shell = nano_tools::shell::ShellTool::new(&home, &workspace_clone);
                    (
                        nano_agent::wiring::RealToolExecutor::new(fs, shell, &workspace_clone),
                        policy,
                    )
                },
            )
            .await
        })
    });
    let mut harness = Harness {
        to_host: Some(in_tx),
        frames: out_rx,
        requests: Arc::new(Mutex::new(Vec::new())),
        handle: Some(handle),
        next_id: 1,
        session_id: String::new(),
        sessions_dir: dir.clone(),
    };
    harness.request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } }
        }),
        "allow",
    );
    let created = harness.request(
        "session/new",
        serde_json::json!({ "cwd": workspace.to_string_lossy(), "mcpServers": [] }),
        "allow",
    );
    harness.session_id = created["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let (response, updates) = harness.prompt("read the env file", "allow");
    assert_eq!(response["result"]["stopReason"], "end_turn", "{response}");
    let card = updates
        .iter()
        .find(|u| u["params"]["update"]["sessionUpdate"] == "tool_call_update")
        .expect("tool_call_update frame");
    assert_eq!(
        card["params"]["update"]["_meta"]["nanoError"]["kind"], "fs_sensitive_denied",
        "{card}"
    );
    assert_eq!(
        card["params"]["update"]["content"][0]["content"]["text"],
        "Denied by policy — Path is outside the allowed set; ask the user"
    );
}

// ── approval denial (D4) ─────────────────────────────────────────────────

/// C7 §8/D4: a gate denial journals the ToolCall (with args — honest
/// transcript) BEFORE the permission prompt, then a failed ToolResult with
/// approval_denied; the live frames follow the same order; the turn
/// continues to end_turn.
#[test]
fn gate_denial_is_journaled_then_framed_and_the_turn_continues() {
    let dir = temp_sessions_dir("deny");
    let mut harness = Harness::spawn(
        vec![
            Ok(tool_response(call(
                "c1",
                "fs_write",
                serde_json::json!({"path": "note.txt", "content": "hello"}),
            ))),
            Ok(text_response("understood, not writing")),
        ],
        vec![ok_outcome()],
        &dir,
        true,
        None,
    );
    let (response, updates) = harness.prompt("write note", "deny");
    assert_eq!(
        response["result"]["stopReason"], "end_turn",
        "a denial is not turn-fatal: {response}"
    );

    // Live frame order: tool_call (in_progress) → failed tool_call_update.
    let kinds: Vec<&str> = updates
        .iter()
        .filter_map(|u| u["params"]["update"]["sessionUpdate"].as_str())
        .collect();
    let call_pos = kinds
        .iter()
        .position(|k| *k == "tool_call")
        .expect("tool_call frame");
    let update_pos = kinds
        .iter()
        .position(|k| *k == "tool_call_update")
        .expect("tool_call_update frame");
    assert!(call_pos < update_pos, "order: {kinds:?}");
    let update = &updates[update_pos]["params"]["update"];
    assert_eq!(update["status"], "failed");
    assert_eq!(update["_meta"]["nanoError"]["kind"], "approval_denied");
    assert_eq!(update["content"][0]["content"]["text"], "Denied by user");

    // The journal holds ToolCall (full args) THEN the failed ToolResult
    // carrying the kind (never the raw text).
    let journal = harness.journal_ops().expect("journal");
    let call_line = journal
        .iter()
        .position(|l| l.contains("\"tool_call\""))
        .expect("journaled tool_call");
    let result_line = journal
        .iter()
        .position(|l| l.contains("\"tool_result\""))
        .expect("journaled tool_result");
    assert!(call_line < result_line);
    assert!(journal[call_line].contains("note.txt"), "args journaled");
    assert!(journal[result_line].contains("\"error_kind\":\"approval_denied\""));
    assert!(!journal[result_line].contains("denied by approval gate"));
}

// ── journal fail-closed (D4 hardening) ───────────────────────────────────

/// C7 §8: a journal-append failure mid-turn fails the turn closed with
/// journal_unavailable, and the unjournaled op NEVER becomes a live frame.
#[test]
fn journal_append_failure_fails_closed() {
    let dir = temp_sessions_dir("journalfail");
    let failer = Arc::new(AtomicBool::new(true)); // every append fails
    let mut harness = Harness::spawn(
        vec![Ok(tool_response(call(
            "c1",
            "fs_read",
            serde_json::json!({"path": "a.txt"}),
        )))],
        vec![ok_outcome()],
        &dir,
        true,
        Some(failer),
    );
    let (response, updates) = harness.prompt("read", "allow");
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "journal_unavailable",
        "{response}"
    );
    assert_eq!(response["error"]["data"]["nanoError"]["retryable"], false);
    assert!(
        updates.is_empty(),
        "no unjournaled frame may reach the client: {updates:?}"
    );
}

// ── engine stops are errors, never cancels (§1.1a regression) ────────────

/// Repeat force-stop and budget exhaustion answer with TYPED error
/// responses — stopReason:"cancelled" is reserved for genuine cancels
/// (covered by acp_live's cancel test).
#[test]
fn engine_stops_are_typed_errors_not_cancels() {
    // (a) repeat force-stop: the same call 12+ times.
    let identical: Vec<Result<ModelResponse, ModelError>> = (0..15)
        .map(|_| {
            Ok(tool_response(call(
                "c1",
                "fs_read",
                serde_json::json!({"path": "same.txt"}),
            )))
        })
        .collect();
    let dir = temp_sessions_dir("forcestop");
    let mut harness = Harness::spawn(identical, vec![ok_outcome(); 15], &dir, true, None);
    let (response, _) = harness.prompt("loop", "allow");
    assert!(
        response.get("result").is_none(),
        "force-stop must not answer a result: {response}"
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "repeat_force_stop",
        "{response}"
    );

    // (b) budget exhaustion: 60 DISTINCT calls (no repeat trip), each
    // scoring progress (no no-progress trip) → 50-step budget trips.
    let distinct: Vec<Result<ModelResponse, ModelError>> = (0..60)
        .map(|i| {
            Ok(tool_response(call(
                "c1",
                "fs_read",
                serde_json::json!({"path": format!("file-{i}.txt")}),
            )))
        })
        .collect();
    let dir = temp_sessions_dir("budget");
    let mut harness = Harness::spawn(distinct, vec![ok_outcome(); 60], &dir, true, None);
    let (response, _) = harness.prompt("many reads", "allow");
    assert!(
        response.get("result").is_none(),
        "budget stop must not answer a result: {response}"
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "budget_exhausted",
        "{response}"
    );
}

// ── mid-stream failure (D1) ──────────────────────────────────────────────

/// C7 §8 partial-stream-then-fail: streamed text from earlier steps STAYS
/// (agent_message_chunk commits before the error), the typed error response
/// is appended after it.
#[test]
fn partial_stream_then_fail_keeps_content_then_errors() {
    let dir = temp_sessions_dir("midstream");
    let mut harness = Harness::spawn(
        vec![
            Ok(text_and_tool_response(
                "partial answer",
                call("c1", "fs_read", serde_json::json!({"path": "a.txt"})),
            )),
            Err(ModelError::Server {
                status: 503,
                message: "edge saturated".into(),
            }),
        ],
        vec![ok_outcome()],
        &dir,
        true,
        None,
    );
    let (response, updates) = harness.prompt("go", "allow");
    let chunk_pos = updates
        .iter()
        .position(|u| {
            u["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                && u["params"]["update"]["content"]["text"] == "partial answer"
        })
        .expect("partial content frame");
    let _ = chunk_pos;
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "model_server_5xx",
        "{response}"
    );
    assert_eq!(response["error"]["data"]["nanoError"]["status"], 503);
}

// ── request-level kinds ──────────────────────────────────────────────────

/// C7 §3: the proven request-level errors keep their numeric codes AND
/// messages (Desktop greps model_not_found), gaining typed kinds in data.
#[test]
fn request_level_errors_gain_kinds_without_changing_codes_or_messages() {
    let dir = temp_sessions_dir("reqlevel");
    let mut harness = Harness::spawn(vec![], vec![], &dir, true, None);

    // model_not_found: -32602, message prefix unchanged, kind added.
    let response = harness.request(
        "session/set_model",
        serde_json::json!({"sessionId": harness.session_id, "modelId": "no-such-model"}),
        "allow",
    );
    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("model_not_found: no-such-model"),
        "{response}"
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"],
        "model_not_found"
    );

    // turn_in_progress: a second prompt while a turn runs.
    // (The running turn loops on distinct reads; answered below.)
    let slow_dir = temp_sessions_dir("inprogress");
    let mut slow_script: Vec<Result<ModelResponse, ModelError>> = (0..30)
        .map(|i| {
            Ok(tool_response(call(
                "c1",
                "fs_read",
                serde_json::json!({"path": format!("f{i}.txt")}),
            )))
        })
        .collect();
    // The turn eventually completes on its own (text response, no calls).
    slow_script.push(Ok(text_response("finished the long turn")));
    let mut slow = Harness::spawn(slow_script, vec![ok_outcome(); 30], &slow_dir, true, None);
    let prompt_id = slow.next_id;
    slow.next_id += 1;
    slow.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": prompt_id, "method": "session/prompt",
        "params": {"sessionId": slow.session_id, "prompt": [{"type":"text","text":"run"}]}
    }));
    // Wait until the turn is actually running (first tool frame).
    loop {
        let frame = slow.next_frame();
        if frame["params"]["update"]["sessionUpdate"] == "tool_call" {
            break;
        }
    }
    let second = slow.request(
        "session/prompt",
        serde_json::json!({"sessionId": slow.session_id, "prompt": [{"type":"text","text":"again"}]}),
        "allow",
    );
    assert_eq!(second["error"]["code"], -32602);
    assert_eq!(
        second["error"]["data"]["nanoError"]["kind"], "turn_in_progress",
        "{second}"
    );
    // Let the first turn drain so the harness drops cleanly.
    loop {
        let frame = slow.next_frame();
        if frame.get("id").and_then(|v| v.as_u64()) == Some(prompt_id) {
            break;
        }
        if frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission") {
            panic!("fs_read must not prompt");
        }
    }

    // session_not_found: -32602 + kind, message unchanged.
    let missing = harness.request(
        "session/load",
        serde_json::json!({"sessionId": "wayland-nano-no-such-session", "cwd": "/x", "mcpServers": []}),
        "allow",
    );
    assert_eq!(missing["error"]["code"], -32602);
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("session not found:"),
        "{missing}"
    );
    assert_eq!(
        missing["error"]["data"]["nanoError"]["kind"],
        "session_not_found"
    );
}

// ── replay compat (D5) ───────────────────────────────────────────────────

/// C7 §8: a journal written BEFORE the error_kind field existed loads
/// unchanged — replay tolerates the missing field (serde default) and the
/// failed card replays untyped, exactly as before.
#[test]
fn old_journals_without_error_kind_load_unchanged() {
    let dir = temp_sessions_dir("oldjournal");
    let session_id = "wayland-nano-c7-oldjournal";
    let journal = dir.join(format!("{session_id}.jsonl"));
    let lines = [
        serde_json::json!({"v":1,"id":"s-1","ts":"now","op":{"type":"session_begin","session_id":session_id,"cwd":"/x"}}),
        serde_json::json!({"v":1,"id":"s-2","ts":"now","op":{"type":"turn_begin","turn_id":"t-1","input":"do it"}}),
        serde_json::json!({"v":1,"id":"s-3","ts":"now","op":{"type":"tool_call","turn_id":"t-1","call_id":"c1","name":"fs_write","args":{"path":"a"}}}),
        // The pre-C7 shape: no error_kind field at all.
        serde_json::json!({"v":1,"id":"s-4","ts":"now","op":{"type":"tool_result","call_id":"c1","ok":false,"output_digest":"len:7","changed_files":[]}}),
        serde_json::json!({"v":1,"id":"s-5","ts":"now","op":{"type":"turn_end","turn_id":"t-1","outcome":"failed"}}),
    ];
    std::fs::write(
        &journal,
        lines
            .iter()
            .map(|l| serde_json::to_string(l).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();

    let mut harness = Harness::spawn(vec![], vec![], &dir, false, None);
    let (response, updates) = harness.request_collecting(
        "session/load",
        serde_json::json!({"sessionId": session_id, "cwd": "/x", "mcpServers": []}),
        "allow",
    );
    assert!(
        response.get("error").is_none(),
        "old journal loads: {response}"
    );
    let card = updates
        .iter()
        .find(|u| {
            u["params"]["update"]["sessionUpdate"] == "tool_call"
                && u["params"]["update"]["status"] == "failed"
        })
        .expect("replayed failed card");
    assert!(
        card["params"]["update"].get("_meta").is_none(),
        "untyped legacy card stays untyped: {card}"
    );
}

/// C7 §8/D5: a NEW journal replays the failed card WITH its kind, and the
/// resumed model context shows `<presentation> [output elided]` — the model
/// still sees WHY the call failed.
#[test]
fn new_journals_replay_typed_and_resume_explains_the_failure() {
    let dir = temp_sessions_dir("newjournal");
    let session_id;
    {
        let mut harness = Harness::spawn(
            vec![
                Ok(tool_response(call(
                    "c1",
                    "fs_write",
                    serde_json::json!({"path": "note.txt", "content": "hello"}),
                ))),
                Ok(text_response("not writing then")),
            ],
            vec![ok_outcome()],
            &dir,
            true,
            None,
        );
        let (response, _) = harness.prompt("write", "deny");
        assert_eq!(response["result"]["stopReason"], "end_turn");
        session_id = harness.session_id.clone();
    }

    // Resume: a fresh host over the same journal.
    let mut harness = Harness::spawn(
        vec![Ok(text_response("resumed"))],
        vec![],
        &dir,
        false,
        None,
    );
    let (response, updates) = harness.request_collecting(
        "session/load",
        serde_json::json!({"sessionId": session_id, "cwd": std::env::temp_dir().to_string_lossy(), "mcpServers": []}),
        "allow",
    );
    assert!(response.get("error").is_none(), "{response}");
    harness.session_id = session_id.clone();
    let card = updates
        .iter()
        .find(|u| {
            u["params"]["update"]["sessionUpdate"] == "tool_call"
                && u["params"]["update"]["status"] == "failed"
        })
        .expect("replayed failed card");
    let update = &card["params"]["update"];
    assert_eq!(update["_meta"]["nanoError"]["kind"], "approval_denied");
    assert_eq!(update["content"][0]["content"]["text"], "Denied by user");

    // The next prompt's context carries the elided-but-explained result.
    let _ = harness.prompt("continue", "allow");
    let requests = harness.requests.lock().unwrap();
    let last = requests.last().expect("a model request");
    let mut blob = String::new();
    for message in &last.messages {
        for block in &message.content {
            match block {
                nano_model::types::ContentBlock::Text { text } => blob.push_str(text),
                nano_model::types::ContentBlock::ToolResult { content, .. } => {
                    blob.push_str(content)
                }
                _ => {}
            }
            blob.push('\n');
        }
    }
    assert!(
        blob.contains("Denied by user [output elided]"),
        "resume context must explain the failure: {blob}"
    );
    // And the digest-only invariant held: no raw tool output was journaled.
    let journal = std::fs::read_to_string(dir.join(format!("{session_id}.jsonl"))).unwrap();
    assert!(!journal.contains("denied by approval gate"));
}

/// C8: prompt and set_model RE-RESOLVE the session credential per turn.
/// The scripted driver never reaches the network, but the resolution needs
/// SOME flux credential in the process env (set once, never removed; every
/// harness in this file shares it).
fn ensure_test_flux_key() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var("FLUX_API_KEY").is_err() {
            unsafe { std::env::set_var("FLUX_API_KEY", "sk-test-harness-never-networked") };
        }
    });
}
