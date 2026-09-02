//! C9 robustness wire tests: session/steer (ack, live drain, closed/full
//! rejections, cancel-beats-steer drop notices, kill-resume replay), the
//! capability advertisement + -32601 fallback, the byte-identical mid-turn
//! session/prompt -32602, and the set_model-then-prompt actionable error.
//! No FLUX key, no network, no child process.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{
    ModelError, ModelEvent, ModelRequest, ModelResponse, ReasoningEffort, ToolCall, Usage,
};
use nano_protocol::acp::AvailableModel;
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── channel-backed streams (same pattern as acp_live.rs) ───────────────

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

// ── scripted model ──────────────────────────────────────────────────────

#[derive(Debug)]
enum Step {
    Respond(ModelResponse),
    Fail(ModelError),
    /// Park (async) until released — the mid-turn window.
    WaitForRelease(ModelResponse),
}

#[derive(Debug, Clone)]
struct MockDriver {
    script: Arc<Mutex<VecDeque<Step>>>,
    calls: Arc<AtomicU64>,
    release: tokio::sync::watch::Receiver<bool>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        let step = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock driver script exhausted");
        match step {
            Step::Respond(response) => Ok(response),
            Step::Fail(err) => Err(err),
            Step::WaitForRelease(response) => {
                let mut release = self.release.clone();
                while !*release.borrow() {
                    release.changed().await.expect("release sender alive");
                }
                Ok(response)
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
struct MockTools;

#[async_trait::async_trait]
impl ToolExecutor for MockTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        ToolOutcome {
            ok: true,
            output: format!("ran {}", call.name),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

/// P5: the fail-closed default routing posture for test hosts (no Auto
/// opt-in, no configured default).
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

// ── harness ─────────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    release: tokio::sync::watch::Sender<bool>,
    driver_calls: Arc<AtomicU64>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
    sessions_dir: std::path::PathBuf,
    init_response: serde_json::Value,
}

fn temp_sessions_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nano-c9-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp sessions dir");
    dir
}

impl Harness {
    fn spawn(script: Vec<Step>) -> Self {
        Self::spawn_full(
            script,
            &temp_sessions_dir("default"),
            true,
            "mock",
            vec![AvailableModel {
                id: "mock".into(),
                name: "mock".into(),
            }],
            None,
        )
    }

    fn spawn_full(
        script: Vec<Step>,
        sessions_dir: &std::path::Path,
        new_session: bool,
        default_model: &str,
        catalog: Vec<AvailableModel>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let driver_calls = Arc::new(AtomicU64::new(0));
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            calls: driver_calls.clone(),
            release: release_rx,
            requests: model_requests.clone(),
        };
        let sessions_dir_owned = sessions_dir.to_path_buf();
        let default_model = default_model.to_string();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                // C8: binding resolution needs SOME flux credential in env
                // (never networked — the scripted driver intercepts).
                if std::env::var("FLUX_API_KEY").is_err() {
                    unsafe { std::env::set_var("FLUX_API_KEY", "sk-test-harness-never-networked") };
                }
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir_owned.parent().expect("root").join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                    policy: nano_cli::memory_policy::ResolvedMemoryPolicy::disabled(),
                };
                // P2a: lane-A vision catalog (vendored, fail-closed) + the attachment
                // store root beside the session journals (lane-B boundary use).
                let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                    .expect("vendored vision catalog parses");
                let attachment_home = sessions_dir_owned.parent().expect("root");
                let hooks = nano_hooks::HookEngine::empty();
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir_owned,
                    default_model: &default_model,
                    available_models: &catalog,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    reasoning_effort,
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
                    sandbox_probe: &sandbox_probe,
                    journal_append_failer: None,
                    router: &router,
                    memory: &memory_config,
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
                    move |_, _, _, _, _, _| {
                        (
                            MockTools,
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
            release: release_tx,
            driver_calls,
            model_requests,
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
            sessions_dir: sessions_dir.to_path_buf(),
            init_response: serde_json::Value::Null,
        };
        harness.init_response = harness.initialize();
        if new_session {
            let response = harness.request(
                "session/new",
                serde_json::json!({ "cwd": ".", "mcpServers": [] }),
            );
            harness.session_id = response["result"]["sessionId"]
                .as_str()
                .expect("sessionId")
                .to_string();
        }
        harness
    }

    fn initialize(&mut self) -> serde_json::Value {
        self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
            }),
        )
    }

    fn send(&self, value: serde_json::Value) {
        let tx = self.to_host.as_ref().expect("stdin open");
        tx.send(format!("{}\n", serde_json::to_string(&value).unwrap()))
            .expect("send to host");
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.send_request(method, params);
        let frames = self.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(id));
        frames.last().expect("response frame").clone()
    }

    fn send_request(&mut self, method: &str, params: serde_json::Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        id
    }

    fn send_prompt(&mut self, text: &str) -> u64 {
        let session_id = self.session_id.clone();
        self.send_request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }]
            }),
        )
    }

    fn send_steer(&mut self, text: &str) -> u64 {
        let session_id = self.session_id.clone();
        self.send_request(
            "session/steer",
            serde_json::json!({ "sessionId": session_id, "text": text }),
        )
    }

    fn next_frame(&self) -> serde_json::Value {
        let line = self
            .frames
            .recv_timeout(TIMEOUT)
            .expect("frame within timeout");
        serde_json::from_str(&line).expect("frame json")
    }

    fn wait_for_driver_calls(&self, n: u64) {
        let deadline = std::time::Instant::now() + TIMEOUT;
        while self.driver_calls.load(Ordering::SeqCst) < n {
            assert!(
                std::time::Instant::now() < deadline,
                "driver call count {n} not reached within timeout"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn read_until(&self, pred: impl Fn(&serde_json::Value) -> bool) -> Vec<serde_json::Value> {
        let mut collected = Vec::new();
        loop {
            let frame = self.next_frame();
            let done = pred(&frame);
            collected.push(frame);
            if done {
                return collected;
            }
        }
    }

    fn release_model(&self) {
        self.release.send(true).expect("release");
    }

    fn journal_text(&self) -> String {
        let path = self.sessions_dir.join(format!("{}.jsonl", self.session_id));
        std::fs::read_to_string(path).expect("journal")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.to_host.take());
        if let Some(handle) = self.handle.take() {
            let Ok(result) = handle.join() else {
                return;
            };
            if !std::thread::panicking() {
                assert_eq!(result.expect("serve io"), 0, "clean exit code");
            }
        }
    }
}

fn update_kind(frame: &serde_json::Value) -> &str {
    frame["params"]["update"]["sessionUpdate"]
        .as_str()
        .unwrap_or("")
}

// ── tests ───────────────────────────────────────────────────────────────

#[test]
fn steer_capability_advertised_and_unknown_method_gets_32601() {
    let mut client = Harness::spawn(vec![]);
    // The extension capability with its version marker.
    assert_eq!(
        client.init_response["result"]["agentCapabilities"]["nanoExtensions"]["session/steer"]["version"],
        1,
        "init: {}",
        client.init_response
    );
    // Unknown methods fall back to the standard JSON-RPC -32601.
    let response = client.request("session/teleport", serde_json::json!({}));
    assert_eq!(response["error"]["code"], -32601);
}

#[test]
fn mid_turn_prompt_rejection_stays_byte_identical() {
    let mut client = Harness::spawn(vec![Step::WaitForRelease(text_response("late"))]);
    client.send_prompt("first");
    client.wait_for_driver_calls(1);
    // Q1: mid-turn session/prompt rejection is byte-identical.
    let response = client.request(
        "session/prompt",
        serde_json::json!({
            "sessionId": client.session_id,
            "prompt": [{ "type": "text", "text": "second" }]
        }),
    );
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["message"], "a turn is already running");
    // Mid-turn session/new / session/compact rejections are untouched too.
    let response = client.request(
        "session/compact",
        serde_json::json!({ "sessionId": client.session_id }),
    );
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["message"], "turn in progress");
    client.release_model();
    client.read_until(|f| f["result"]["stopReason"].is_string());
}

#[test]
fn steer_enqueues_drains_at_loop_top_and_streams_live() {
    let mut client = Harness::spawn(vec![
        Step::WaitForRelease(text_response("first answer")),
        Step::Respond(text_response("steered answer")),
    ]);
    let prompt_id = client.send_prompt("start");
    client.wait_for_driver_calls(1);
    // Mid-turn steer: ack resolves IMMEDIATELY with the position.
    let steer_id = client.send_steer("also check the tests");
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(steer_id));
    let ack = frames.last().unwrap();
    assert_eq!(ack["result"]["queued"], true, "ack: {ack}");
    assert_eq!(ack["result"]["position"], 1);
    client.release_model();
    // The turn completes (the steer kept it going past the first answer).
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    // A live user_message_chunk rendered the drained steer.
    assert!(
        frames.iter().any(|f| update_kind(f) == "user_message_chunk"
            && f["params"]["update"]["content"]["text"] == "also check the tests"),
        "drained steer streams live: {frames:?}"
    );
    // The model SAW the steer as a trailing user message on the follow-up.
    let requests = client.model_requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let last = requests[1].messages.last().expect("steer message");
    assert!(
        matches!(&last.content[0], nano_model::types::ContentBlock::Text { text } if text == "also check the tests")
    );
    // Journal-first: Op::SteerInput is durably journaled.
    assert!(client.journal_text().contains("\"steer_input\""));
}

#[test]
fn steer_without_a_turn_rejects_closed() {
    let mut client = Harness::spawn(vec![]);
    let response = client.request(
        "session/steer",
        serde_json::json!({ "sessionId": client.session_id, "text": "nobody home" }),
    );
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["message"], "steer queue closed");
}

#[test]
fn cancel_beats_steer_with_exactly_one_drop_notice() {
    let mut client = Harness::spawn(vec![Step::WaitForRelease(text_response("never sent"))]);
    let prompt_id = client.send_prompt("start");
    client.wait_for_driver_calls(1);
    let steer_id = client.send_steer("dropped on the floor");
    client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(steer_id));
    // Cancel: notification.
    client.send(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": client.session_id }
    }));
    // The model release uses a different channel/thread from ACP input. A
    // FIFO request after the notification is an observable ingress barrier:
    // its response proves the reader has consumed cancel (and fired the
    // shared flag) before the parked model is allowed to complete. Without
    // this barrier, a slow runner can schedule release first and turn this
    // protocol test into a harness race.
    let barrier_id = client.send_request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
        }),
    );
    let mut frames =
        client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(barrier_id));
    client.release_model();
    if !frames
        .iter()
        .any(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id))
    {
        frames
            .extend(client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id)));
    }
    let prompt_response = frames
        .iter()
        .find(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id))
        .expect("prompt response retained across ingress barrier");
    assert_eq!(prompt_response["result"]["stopReason"], "cancelled");
    // Exactly one steer_dropped notice, carrying the steer REQUEST id and
    // the digest — never the text (it was never model-visible).
    let drops: Vec<&serde_json::Value> = frames
        .iter()
        .filter(|f| update_kind(f) == "steer_dropped")
        .collect();
    assert_eq!(drops.len(), 1, "one notice per dropped steer: {frames:?}");
    assert_eq!(
        drops[0]["params"]["update"]["requestId"],
        serde_json::Value::String(steer_id.to_string())
    );
    assert!(
        !frames.iter().any(|f| update_kind(f) == "user_message_chunk"
            && f["params"]["update"]["content"]["text"] == "dropped on the floor"),
        "an undrained steer never renders as model-visible: {frames:?}"
    );
    // Kill-resume can never resurrect it: nothing was journaled.
    assert!(!client.journal_text().contains("\"steer_input\""));
}

#[test]
fn multi_steer_drains_as_separate_user_messages() {
    let mut client = Harness::spawn(vec![
        Step::WaitForRelease(text_response("first")),
        Step::Respond(text_response("done")),
    ]);
    let prompt_id = client.send_prompt("start");
    client.wait_for_driver_calls(1);
    let steer_a = client.send_steer("steer one");
    let steer_b = client.send_steer("steer two");
    client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(steer_b));
    let _ = steer_a;
    client.release_model();
    client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    let requests = client.model_requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let messages = &requests[1].messages;
    let tail: Vec<&str> = messages
        .iter()
        .rev()
        .take(2)
        .map(|m| match &m.content[0] {
            nano_model::types::ContentBlock::Text { text } => text.as_str(),
            _ => panic!("user text message expected"),
        })
        .collect();
    // Separate user items, verbatim, in FIFO order.
    assert_eq!(tail, vec!["steer two", "steer one"]);
}

#[test]
fn kill_resume_replays_the_drained_steer() {
    let sessions_dir = temp_sessions_dir("resume");
    let session_id;
    {
        let mut client = Harness::spawn_full(
            vec![
                Step::WaitForRelease(text_response("first")),
                Step::Respond(text_response("second")),
            ],
            &sessions_dir,
            true,
            "mock",
            vec![AvailableModel {
                id: "mock".into(),
                name: "mock".into(),
            }],
            None,
        );
        let prompt_id = client.send_prompt("start");
        client.wait_for_driver_calls(1);
        let steer_id = client.send_steer("remember this steer");
        client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(steer_id));
        client.release_model();
        client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
        session_id = client.session_id.clone();
    }
    // Fresh serve over the SAME sessions dir = process restart; the load
    // replay must carry the drained steer as a user chunk.
    let mut client = Harness::spawn_full(
        vec![],
        &sessions_dir,
        false,
        "mock",
        vec![AvailableModel {
            id: "mock".into(),
            name: "mock".into(),
        }],
        None,
    );
    let load_id = client.send_request(
        "session/load",
        serde_json::json!({ "sessionId": session_id, "cwd": ".", "mcpServers": [] }),
    );
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(load_id));
    assert!(
        frames.iter().any(|f| update_kind(f) == "user_message_chunk"
            && f["params"]["update"]["content"]["text"] == "remember this steer"),
        "kill-resume replays the drained steer: {frames:?}"
    );
}

#[test]
fn set_model_to_non_reasoning_then_prompt_is_actionable() {
    // Sticky effort configured, then the user switches mid-session to a
    // non-reasoning model: the failure names the setting to clear.
    let mut client = Harness::spawn_full(
        vec![Step::Respond(text_response("ok"))],
        &temp_sessions_dir("setmodel"),
        true,
        "flux-reasoning",
        vec![
            AvailableModel {
                id: "flux-reasoning".into(),
                name: "flux-reasoning".into(),
            },
            AvailableModel {
                id: "flux-fast".into(),
                name: "flux-fast".into(),
            },
        ],
        Some(ReasoningEffort::High),
    );
    let response = client.request(
        "session/set_model",
        serde_json::json!({ "sessionId": client.session_id, "modelId": "flux-fast" }),
    );
    assert!(response.get("result").is_some(), "set_model ok: {response}");
    // NOTE: the mock driver accepts every request, so the ladder's
    // known-unsupported rejection must happen at request-BUILD — but the
    // mock bypasses the Flux client. The engine-level guarantee is pinned
    // in nano-model's no-network tests; here we pin that the turn still
    // completes against a driver that does not implement the ladder, and
    // that the request CARRIES the sticky effort for the real client to
    // judge.
    let prompt_id = client.send_prompt("hello");
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert!(frames.last().unwrap().get("result").is_some());
    let requests = client.model_requests.lock().unwrap();
    assert_eq!(
        requests[0].reasoning_effort,
        Some(ReasoningEffort::High),
        "the sticky effort rides the request for the surface to judge"
    );
    assert_eq!(requests[0].model, "flux-fast");
}

/// The engine-level actionable error, end to end (re-pinned to the C7 wire
/// shape, which supersedes this branch's original trailing-chunk form): a
/// driver that mimics the real client's pre-network rejection (typed
/// UnsupportedParam) answers the prompt with a typed ERROR RESPONSE —
/// model_unsupported_param with the static table presentation, never an
/// opaque stopReason or a free-text chunk. The verbatim setting name stays
/// in the journaled failure detail (design §7: only static presentations
/// ride UI-bound frames).
#[test]
fn unsupported_param_failure_reaches_the_user_verbatim() {
    let mut client = Harness::spawn(vec![Step::Fail(ModelError::UnsupportedParam {
        param: "reasoning_effort".into(),
        surface: "flux-completions".into(),
        message: "model 'flux-fast' has no reasoning tier; clear `reasoning_effort` in config (NANO_REASONING_EFFORT) or switch back to a reasoning model".into(),
    })]);
    let prompt_id = client.send_prompt("hello");
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    let response = frames.last().unwrap();
    assert!(
        response.get("result").is_none(),
        "turn-fatal failures are error responses, not results: {response}"
    );
    let nano = &response["error"]["data"]["nanoError"];
    assert_eq!(nano["kind"], "model_unsupported_param");
    assert_eq!(nano["retryable"], false);
    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("Parameter unsupported on this model"),
        "static table presentation: {message}"
    );
}

#[test]
fn replay_fold_treats_steers_and_reasks_as_user_messages() {
    // Unit-level kill-resume fidelity: the context rebuild folds
    // SteerInput and SchemaReask exactly like the TurnBegin input fold.
    let envelopes = vec![
        nano_session::op::OpEnvelope::new(
            "t-1",
            "now",
            nano_session::op::Op::TurnBegin {
                turn_id: "t".into(),
                input: "original prompt".into(),
                input_blocks: Vec::new(),
            },
        ),
        nano_session::op::OpEnvelope::new(
            "t-2",
            "now",
            nano_session::op::Op::AssistantText {
                turn_id: "t".into(),
                text: "first answer".into(),
            },
        ),
        nano_session::op::OpEnvelope::new(
            "t-3",
            "now",
            nano_session::op::Op::SteerInput {
                turn_id: "t".into(),
                text: "steer text".into(),
            },
        ),
        nano_session::op::OpEnvelope::new(
            "t-4",
            "now",
            nano_session::op::Op::SchemaReask {
                turn_id: "t".into(),
                feedback: "literal feedback".into(),
            },
        ),
    ];
    let messages = acp_mode::messages_from_envelopes(&envelopes);
    let texts: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == nano_model::types::Role::User)
        .filter_map(|m| match &m.content[0] {
            nano_model::types::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec!["original prompt", "steer text", "literal feedback"]
    );
}
