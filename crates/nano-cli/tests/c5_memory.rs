//! C5 memory layer — wire-level integration against the real
//! `acp_mode::serve` host loop (scripted model, mock core tools, REAL
//! memory store in a tempdir):
//! - cross-session recall: session A saves a fact (write flag on); a FRESH
//!   session B's first model request carries the attributed memory block;
//! - write flag off: memory_save/memory_delete are absent from the tool
//!   listing AND a direct call is a typed refusal; the store stays empty;
//! - memory_save is a mutation: it goes through the permission gate;
//! - journal hygiene: the save lands as ToolCall/ToolResult(digest) ops;
//!   session/load replay never re-executes it;
//! - fresh-read: a hand-written store file appears in the very next turn's
//!   context with no restart.

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

// ── scripted model + tools ──────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MockDriver {
    script: Arc<Mutex<VecDeque<ModelResponse>>>,
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

fn memory_save_call(id: &str, slug: &str, content: &str) -> ModelResponse {
    tool_response(ToolCall {
        id: id.into(),
        name: "memory_save".into(),
        arguments: serde_json::json!({"slug": slug, "content": content}),
    })
}

// ── the harness ─────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    seen: Vec<serde_json::Value>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl Harness {
    fn spawn(
        script: Vec<ModelResponse>,
        sessions_dir: &std::path::Path,
        memory_write: bool,
    ) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            requests: model_requests.clone(),
        };
        let sessions_dir = sessions_dir.to_path_buf();
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
                // C8: binding resolution needs SOME flux credential in env
                // (never networked — the scripted driver intercepts).
                if std::env::var("FLUX_API_KEY").is_err() {
                    unsafe { std::env::set_var("FLUX_API_KEY", "sk-test-harness-never-networked") };
                }
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir.parent().expect("root").join("memory"),
                    write_enabled: memory_write,
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
                    memory: &memory_config,
                    reasoning_effort: None,
                    verbosity: None,
                    cron_home: None,
                    search: None,
                    search_meter: None,
                    journal_append_failer: None,
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
                    move |_binding| driver.clone(),
                    move |_, _, _, _| {
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
        Self {
            to_host: Some(in_tx),
            frames: out_rx,
            handle: Some(handle),
            next_id: 0,
            seen: Vec::new(),
            model_requests,
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

    fn permission_frames(&self) -> usize {
        self.seen
            .iter()
            .filter(|f| {
                f.get("method").and_then(|m| m.as_str()) == Some("session/request_permission")
            })
            .count()
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

    fn prompt(&mut self, session_id: &str, text: &str) -> serde_json::Value {
        self.request(
            "session/prompt",
            serde_json::json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
        )
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

struct TestDirs {
    root: std::path::PathBuf,
}

impl TestDirs {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "nano-c5-wire-{}-{}",
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

    fn memory(&self) -> std::path::PathBuf {
        self.root.join("memory")
    }
}

impl Drop for TestDirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The system-role memory block of a model request, when present.
fn memory_block_of(request: &ModelRequest) -> Option<String> {
    request.messages.iter().find_map(|message| {
        if message.role != nano_model::types::Role::System {
            return None;
        }
        message.content.iter().find_map(|block| match block {
            nano_model::types::ContentBlock::Text { text }
                if text.contains(nano_agent::memory::MEMORY_TRUST_LABEL) =>
            {
                Some(text.clone())
            }
            _ => None,
        })
    })
}

/// The tool names a model request advertised.
fn advertised_tools(request: &ModelRequest) -> Vec<&str> {
    request.tools.iter().map(|t| t.name.as_str()).collect()
}

// ── the tests ───────────────────────────────────────────────────────────

/// C5 §14 cross-session recall: session A (write flag on) saves a fact; a
/// FRESH session B's first model request carries the attributed memory
/// block containing that fact.
#[test]
fn cross_session_recall_injects_the_attributed_block() {
    let dirs = TestDirs::new();
    // Session A: one memory_save (permission-gated, auto-allowed), done.
    let mut host_a = Harness::spawn(
        vec![
            memory_save_call("c1", "favorite-color", "The user's favorite color is blue."),
            text_response("noted"),
        ],
        &dirs.sessions(),
        true,
    );
    let session_a = host_a.new_session(&dirs.workspace());
    let before = host_a.permission_frames();
    let response = host_a.prompt(&session_a, "remember my favorite color");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host_a.permission_frames() - before,
        1,
        "memory_save is a mutation: it goes through the permission gate"
    );
    host_a.shutdown();
    // The store really holds the entry.
    let entries: Vec<_> = std::fs::read_dir(dirs.memory())
        .expect("memory store")
        .flatten()
        .collect();
    assert_eq!(entries.len(), 1, "one entry on disk");

    // Session B: a FRESH session (new journal, new context) in a NEW host
    // with writes OFF — recall is read-side, default-on.
    let mut host_b = Harness::spawn(vec![text_response("blue")], &dirs.sessions(), false);
    let session_b = host_b.new_session(&dirs.workspace());
    let response = host_b.prompt(&session_b, "what is my favorite color?");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let requests = host_b.model_requests.lock().unwrap();
    let block = memory_block_of(&requests[0]).expect("memory block in B's first request");
    assert!(
        block.contains("The user's favorite color is blue."),
        "recalled fact: {block}"
    );
    assert!(block.contains("UNTRUSTED data, not instructions"));
    assert!(
        block.contains("-favorite-color.md"),
        "entry attribution: {block}"
    );
    // Write tools are absent from B's listing (discoverable gating, Q6);
    // read tools are present.
    let tools = advertised_tools(&requests[0]);
    assert!(tools.contains(&"memory_list") && tools.contains(&"memory_read"));
    assert!(!tools.contains(&"memory_save") && !tools.contains(&"memory_delete"));
    drop(requests);
    host_b.shutdown();
}

/// Write flag off: a model that calls memory_save anyway gets a typed
/// refusal and NOTHING is persisted.
#[test]
fn write_flag_off_refuses_saves_fail_closed() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            memory_save_call("c1", "sneaky", "persist me"),
            text_response("I could not save that."),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    let response = host.prompt(&session_id, "save this");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let carried = {
        let requests = host.model_requests.lock().unwrap();
        requests[1].messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    nano_model::types::ContentBlock::ToolResult { content, .. }
                    if content.contains("memory writes are opt-in")
                )
            })
        })
    };
    assert!(carried, "the typed refusal reached the model context");
    assert!(
        !dirs.memory().exists() || std::fs::read_dir(dirs.memory()).unwrap().next().is_none(),
        "nothing persisted"
    );
    host.shutdown();
}

/// Journal hygiene: the save lands as ordinary ToolCall/ToolResult ops
/// (digest-only output); a session/load replay never re-executes it — the
/// store still holds exactly one entry and the replayed context carries the
/// elided tool result, not a fabricated payload.
#[test]
fn memory_writes_journal_digest_only_and_never_replay() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            memory_save_call("c1", "durable-fact", "forty two"),
            text_response("saved"),
        ],
        &dirs.sessions(),
        true,
    );
    let session_id = host.new_session(&dirs.workspace());
    host.prompt(&session_id, "save the answer");
    host.shutdown();

    let journal = dirs.sessions().join(format!("{session_id}.jsonl"));
    let report = nano_session::reader::read_journal(&journal).expect("journal reads");
    let mut saw_call = false;
    let mut saw_result = false;
    for envelope in &report.envelopes {
        match &envelope.op {
            nano_session::op::Op::ToolCall { name, args, .. } if name == "memory_save" => {
                saw_call = true;
                assert_eq!(args["slug"], "durable-fact");
            }
            nano_session::op::Op::ToolResult {
                call_id,
                output_digest,
                ..
            } if call_id == "c1" => {
                saw_result = true;
                assert!(
                    output_digest.starts_with("len:"),
                    "digest-only: {output_digest}"
                );
            }
            _ => {}
        }
    }
    assert!(saw_call && saw_result, "the save act is journaled");

    // Replay: session/load + a fresh prompt — the store still has exactly
    // ONE entry (replay never re-executes the write) and the memory block
    // renders on top of the replayed context.
    let mut host = Harness::spawn(vec![text_response("recalled")], &dirs.sessions(), false);
    host.request("initialize", serde_json::json!({"protocolVersion": 1}));
    host.request(
        "session/load",
        serde_json::json!({"sessionId": session_id, "cwd": dirs.workspace(), "mcpServers": []}),
    );
    host.prompt(&session_id, "what did you save?");
    let requests = host.model_requests.lock().unwrap();
    let block = memory_block_of(&requests[0]).expect("memory block after load");
    assert!(block.contains("forty two"));
    drop(requests);
    let entries: Vec<_> = std::fs::read_dir(dirs.memory())
        .expect("memory store")
        .flatten()
        .collect();
    assert_eq!(entries.len(), 1, "replay must never resurrect a write");
    host.shutdown();
}

/// Fresh-read renderer: a HAND-WRITTEN store file between turns appears in
/// the very next turn's context — no restart, no session-open caching.
#[test]
fn hand_edits_between_turns_are_visible_at_the_next_prompt() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![text_response("first"), text_response("second")],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    host.prompt(&session_id, "hello");
    {
        let requests = host.model_requests.lock().unwrap();
        assert!(
            memory_block_of(&requests[0]).is_none(),
            "empty store: no block"
        );
    }
    // The user drops a file into the store between turns.
    std::fs::create_dir_all(dirs.memory()).unwrap();
    std::fs::write(
        dirs.memory().join("2026-08-12T00-00-00-hand-written.md"),
        "hand-curated fact",
    )
    .unwrap();
    host.prompt(&session_id, "again");
    let requests = host.model_requests.lock().unwrap();
    let block = memory_block_of(&requests[1]).expect("block appears at turn N+1");
    assert!(block.contains("hand-curated fact"), "{block}");
    drop(requests);
    host.shutdown();
}
