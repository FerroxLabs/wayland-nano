//! C1 §9 integration tests: auto-compaction through the ACP host with a
//! scripted model, manual session/compact, kill-resume fidelity over the
//! compacted prefix, commit-order fault injection, and forged-op replay
//! tolerance. No FLUX key, no network, no child process.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{
    ContentBlock, Message, ModelError, ModelEvent, ModelRequest, ModelResponse, Role, ToolCall,
    Usage,
};
use nano_protocol::acp::AvailableModel;
use nano_session::op::{CompactionCancelReason, Op, OpEnvelope};
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── channel-backed streams (same pattern as acp_live.rs) ────────────────

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

// ── scripted model: recognizes the summarization call ────────────────────

#[derive(Debug)]
enum Step {
    Respond(ModelResponse),
    /// Park (async) until the test releases, then respond. While parked the
    /// turn is OBSERVABLY in flight (the host's select loop is free) — this
    /// is what makes a mid-turn rejection deterministic instead of a race
    /// against the reader thread (same pattern as acp_live.rs).
    WaitForRelease(ModelResponse),
}

#[derive(Debug, Clone)]
struct MockDriver {
    script: Arc<Mutex<VecDeque<Step>>>,
    /// Summarization-call responses (last message is the SUMMARIZATION_PROMPT).
    summaries: Arc<Mutex<VecDeque<Result<ModelResponse, ModelError>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    calls: Arc<AtomicU64>,
    release: tokio::sync::watch::Receiver<bool>,
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(request.clone());
        let is_summary = request
            .messages
            .last()
            .and_then(|m| m.content.first())
            .is_some_and(|b| matches!(b, ContentBlock::Text { text } if text == nano_agent::compact::SUMMARIZATION_PROMPT));
        if is_summary {
            return self
                .summaries
                .lock()
                .unwrap()
                .pop_front()
                .expect("summary script exhausted");
        }
        let step = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock driver script exhausted");
        match step {
            Step::Respond(response) => Ok(response),
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
struct MockTools {
    calls: Arc<Mutex<Vec<ToolCall>>>,
}

#[async_trait::async_trait]
impl ToolExecutor for MockTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        self.calls.lock().unwrap().push(call.clone());
        ToolOutcome {
            ok: true,
            output: format!("ran {}", call.name),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

fn text_response(text: &str, input_tokens: u64) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::Done {
                stop_reason: "stop".into(),
            },
        ],
        usage: Usage {
            input_tokens,
            ..Default::default()
        },
        stop_reason: "stop".into(),
    }
}

fn tool_response(call: ToolCall, input_tokens: u64) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::ToolCallComplete(call),
            ModelEvent::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        usage: Usage {
            input_tokens,
            ..Default::default()
        },
        stop_reason: "tool_calls".into(),
    }
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({"path": "x.txt"}),
    }
}

// ── harness ──────────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
    driver_calls: Arc<AtomicU64>,
    release_tx: tokio::sync::watch::Sender<bool>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
}

fn temp_sessions_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nano-c1-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp sessions dir");
    dir
}

impl Harness {
    fn spawn(
        script: Vec<Step>,
        summaries: Vec<Result<ModelResponse, ModelError>>,
        sessions_dir: &std::path::Path,
        new_session: bool,
    ) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let driver_calls = Arc::new(AtomicU64::new(0));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            summaries: Arc::new(Mutex::new(summaries.into())),
            requests: model_requests.clone(),
            calls: driver_calls.clone(),
            release: release_rx,
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
                    name: "mock".into(),
                }];
                // C2: default-mode tests never consult the sandbox probe.
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                ensure_test_flux_key();
                // C5: memory store for this harness (writes off unless a
                // test opts in).
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir.parent().expect("root").join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                };
                // P2a: lane-A vision catalog (vendored, fail-closed) + the attachment
                // store root beside the session journals (lane-B boundary use).
                let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                    .expect("vendored vision catalog parses");
                let attachment_home = sessions_dir.parent().expect("root");
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir,
                    default_model: "mock",
                    available_models: &catalog,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: Some(1_000),
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
                    move |_, _, _, _, _| {
                        (
                            MockTools::default(),
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
            model_requests,
            driver_calls: driver_calls.clone(),
            release_tx,
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
        };
        harness.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } }
            }),
        );
        if new_session {
            let created = harness.request(
                "session/new",
                serde_json::json!({ "cwd": std::env::temp_dir().to_string_lossy(), "mcpServers": [] }),
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

    /// Blocks until the mock driver has been called `n` times — i.e. the
    /// engine really is parked inside the nth model call. Gating on this
    /// (not on a frame's arrival) is what makes the mid-turn tests
    /// deterministic (same pattern as acp_live.rs).
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

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        loop {
            let frame = self.next_frame();
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return frame;
            }
        }
    }

    /// Drains frames until the response for `id`, collecting session/update
    /// notifications seen on the way.
    fn request_collecting(
        &mut self,
        method: &str,
        params: serde_json::Value,
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
            if frame.get("method").and_then(|m| m.as_str()) == Some("session/update") {
                updates.push(frame);
            }
        }
    }

    fn prompt(&mut self, text: &str) -> serde_json::Value {
        self.request(
            "session/prompt",
            serde_json::json!({
                "sessionId": self.session_id,
                "prompt": [{ "type": "text", "text": text }]
            }),
        )
    }

    fn journal(&self, sessions_dir: &std::path::Path) -> Vec<OpEnvelope> {
        let path = sessions_dir.join(format!("{}.jsonl", self.session_id));
        nano_session::read_journal(&path)
            .expect("journal readable")
            .envelopes
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

// ── tests ────────────────────────────────────────────────────────────────

/// Long session crossing the threshold: compaction fires at the loop top,
/// both UIs get begin/complete notices, the journal carries the full
/// Begin/Complete pair, and the continuation runs on the compacted context.
#[test]
fn auto_compaction_fires_and_is_journaled_and_announced() {
    let dir = temp_sessions_dir("auto");
    let mut harness = Harness::spawn(
        vec![
            Step::Respond(tool_response(tool_call("c1", "fs_read"), 950)),
            Step::Respond(text_response("all done", 100)),
        ],
        vec![Ok(text_response("user asked to read x.txt; read it", 0))],
        &dir,
        true,
    );
    let (answer, updates) = harness.request_collecting(
        "session/prompt",
        serde_json::json!({
            "sessionId": harness.session_id,
            "prompt": [{ "type": "text", "text": "read x.txt" }]
        }),
    );
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    let notices: Vec<&str> = updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "compaction")
        .map(|u| u["params"]["update"]["status"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(notices, vec!["begin", "complete"], "notices: {updates:?}");

    let journal = harness.journal(&dir);
    let mut saw_begin = false;
    let mut saw_complete = false;
    for envelope in &journal {
        match &envelope.op {
            Op::CompactionBegin { .. } => {
                saw_begin = true;
                assert!(!saw_complete, "Begin precedes Complete");
            }
            Op::CompactionComplete { summary, .. } => {
                saw_complete = true;
                assert_eq!(summary, "user asked to read x.txt; read it");
            }
            _ => {}
        }
    }
    assert!(saw_begin && saw_complete);

    // The continuation request ran on the compacted context: its final
    // message is the marked summary.
    let requests = harness.model_requests.lock().unwrap();
    let continuation = &requests[2];
    let ContentBlock::Text { text } = &continuation.messages.last().unwrap().content[0] else {
        panic!("summary message")
    };
    assert!(text.starts_with(nano_agent::compact::SUMMARY_PREFIX));
}

/// Manual /compact: same pipeline, journaled identically, notices emitted,
/// and the next prompt starts from the compacted context.
#[test]
fn manual_compact_via_session_compact() {
    let dir = temp_sessions_dir("manual");
    let mut harness = Harness::spawn(
        vec![
            Step::Respond(text_response("first answer", 100)),
            Step::Respond(text_response("second answer", 100)),
        ],
        vec![Ok(text_response("the manual summary", 0))],
        &dir,
        true,
    );
    let answer = harness.prompt("hello there");
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");

    let (compact, updates) = harness.request_collecting(
        "session/compact",
        serde_json::json!({ "sessionId": harness.session_id }),
    );
    assert_eq!(compact["result"]["compacted"], true, "{compact}");
    let notices: Vec<&str> = updates
        .iter()
        .filter(|u| u["params"]["update"]["sessionUpdate"] == "compaction")
        .map(|u| u["params"]["update"]["status"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(notices, vec!["begin", "complete"]);

    let journal = harness.journal(&dir);
    assert!(
        journal
            .iter()
            .any(|e| matches!(&e.op, Op::CompactionComplete { summary, .. } if summary == "the manual summary"))
    );

    // The next prompt's context IS the compacted history.
    let answer2 = harness.prompt("continue");
    assert_eq!(answer2["result"]["stopReason"], "end_turn", "{answer2}");
    let requests = harness.model_requests.lock().unwrap();
    let last = requests.last().unwrap();
    let ContentBlock::Text { text } = &last.messages[0].content[0] else {
        panic!("user message")
    };
    assert_eq!(text, "hello there");
    let ContentBlock::Text { text: summary } = &last.messages[1].content[0] else {
        panic!("summary message")
    };
    assert!(summary.contains("the manual summary"));
}

/// /compact while a turn runs is rejected — the sync gate: no approval can
/// be pending at the compaction seam.
#[test]
fn compact_during_turn_is_rejected() {
    let dir = temp_sessions_dir("busy");
    // Deterministic mid-turn rejection: the scripted driver's single
    // response is PARKED on the release latch, so the turn is observably in
    // flight (driver called, not returned) when the compact arrives —
    // asserting "turn in progress" against a turn that may have already
    // completed would race the reader thread on contended runners (the
    // ubuntu-22.04 CI leg lost that race: the instant mock turn finished
    // before the compact frame was processed, the compact arm ran, and the
    // empty summary script panicked).
    let mut harness = Harness::spawn(
        vec![Step::WaitForRelease(text_response("slow answer", 100))],
        vec![],
        &dir,
        true,
    );
    let prompt_id = harness.next_id;
    harness.next_id += 1;
    harness.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": prompt_id, "method": "session/prompt",
        "params": { "sessionId": harness.session_id,
                    "prompt": [{ "type": "text", "text": "hi" }] }
    }));
    // The turn is REALLY in flight — parked inside the model call.
    harness.wait_for_driver_calls(1);
    let compact_id = harness.next_id;
    harness.next_id += 1;
    harness.send(&serde_json::json!({
        "jsonrpc": "2.0", "id": compact_id, "method": "session/compact",
        "params": { "sessionId": harness.session_id }
    }));
    // The compact response must be the typed rejection while the turn is
    // parked.
    let compact_response = loop {
        let frame = harness.next_frame();
        if frame.get("id").and_then(|v| v.as_u64()) == Some(compact_id) {
            break frame;
        }
    };
    assert_eq!(
        compact_response["error"]["code"], -32602,
        "{compact_response}"
    );
    assert!(
        compact_response["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("turn in progress")
    );
    // Release the parked model; the turn completes.
    harness
        .release_tx
        .send(true)
        .expect("release the parked turn");
    let prompt_response = loop {
        let frame = harness.next_frame();
        if frame.get("id").and_then(|v| v.as_u64()) == Some(prompt_id) {
            break frame;
        }
    };
    assert_eq!(prompt_response["result"]["stopReason"], "end_turn");
}

/// Kill-resume fidelity, scoped per §6: kill after a compacting turn; the
/// resumed session's context matches the live post-compaction context over
/// the COMPACTED PREFIX (through the summary message), byte-for-byte.
#[test]
fn kill_resume_fidelity_over_compacted_prefix() {
    let dir = temp_sessions_dir("resume");
    let summary_text = "user wanted x.txt read; it was read";
    let live_context;
    let session_id;
    {
        let mut harness = Harness::spawn(
            vec![
                Step::Respond(tool_response(tool_call("c1", "fs_read"), 950)),
                Step::Respond(text_response("done", 100)),
            ],
            vec![Ok(text_response(summary_text, 0))],
            &dir,
            true,
        );
        harness.prompt("read x.txt");
        session_id = harness.session_id.clone();
        // The live post-compaction context over the compacted prefix: the
        // canonical builder applied to the pre-compaction user messages.
        live_context = nano_agent::compact::build_compacted_history(
            vec![Message::user("read x.txt")],
            summary_text,
        );
        // The "kill": drop the harness (host shuts down), keep the journal.
    }
    // Resume in a fresh host over the same sessions dir.
    let mut harness = Harness::spawn(
        vec![Step::Respond(text_response("resumed answer", 50))],
        vec![],
        &dir,
        false,
    );
    let loaded = harness.request(
        "session/load",
        serde_json::json!({
            "sessionId": session_id,
            "cwd": std::env::temp_dir().to_string_lossy(),
            "mcpServers": []
        }),
    );
    assert!(loaded.get("result").is_some(), "{loaded}");
    harness.prompt("what did we do?");
    let requests = harness.model_requests.lock().unwrap();
    let resumed_request = requests.last().unwrap();
    // The compacted prefix (everything through the summary message) must be
    // byte-identical to the live post-compaction context.
    let prefix_len = live_context.len();
    assert!(
        resumed_request.messages.len() > prefix_len,
        "prefix + post-compaction messages + new input"
    );
    assert_eq!(
        &resumed_request.messages[..prefix_len],
        live_context.as_slice(),
        "compacted prefix is byte-identical live vs resumed"
    );
    // And the load replay announced the historical compaction.
    // (Notice frames were drained by `request`; re-check via the journal.)
    let journal = nano_session::read_journal(&dir.join(format!("{session_id}.jsonl")))
        .unwrap()
        .envelopes;
    assert!(
        journal
            .iter()
            .any(|e| matches!(&e.op, Op::CompactionComplete { .. }))
    );
}

/// Commit-order fault injection: a journal whose tail is a stranded
/// CompactionBegin (crash after Begin, before Complete/Cancel) replays
/// UN-compacted — the safe direction.
#[test]
fn crash_after_begin_replays_uncompacted() {
    let envelopes = vec![
        OpEnvelope::new(
            "1",
            "now",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "real question".into(),
                input_blocks: Vec::new(),
            },
        ),
        OpEnvelope::new(
            "2",
            "now",
            Op::AssistantText {
                turn_id: "t1".into(),
                text: "real answer".into(),
            },
        ),
        OpEnvelope::new(
            "3",
            "now",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: nano_session::TurnOutcome::Completed,
                usage: None,
            },
        ),
        OpEnvelope::new(
            "4",
            "now",
            Op::CompactionBegin {
                compaction_id: "k1".into(),
            },
        ),
    ];
    let messages = acp_mode::messages_from_envelopes(&envelopes);
    assert_eq!(
        messages,
        vec![
            Message::user("real question"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "real answer".into()
                }],
            },
        ],
        "stranded Begin: full history survives"
    );
}

/// Crash after a durable Complete: the load path reconstructs the compacted
/// context (user messages + marked summary; assistant/tool wholesale-gone).
#[test]
fn crash_after_complete_replays_compacted() {
    let envelopes = vec![
        OpEnvelope::new(
            "1",
            "now",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "real question".into(),
                input_blocks: Vec::new(),
            },
        ),
        OpEnvelope::new(
            "2",
            "now",
            Op::AssistantText {
                turn_id: "t1".into(),
                text: "real answer".into(),
            },
        ),
        OpEnvelope::new(
            "3",
            "now",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: nano_session::TurnOutcome::Completed,
                usage: None,
            },
        ),
        OpEnvelope::new(
            "4",
            "now",
            Op::CompactionBegin {
                compaction_id: "k1".into(),
            },
        ),
        OpEnvelope::new(
            "5",
            "now",
            Op::CompactionComplete {
                compaction_id: "k1".into(),
                summary: "durable summary".into(),
                covers_op_ids: vec!["1".into(), "2".into(), "3".into()],
                changed_files: vec![],
                image_influenced: false,
                mcp_hydration: None,
            },
        ),
    ];
    let messages = acp_mode::messages_from_envelopes(&envelopes);
    let live = nano_agent::compact::build_compacted_history(
        vec![
            Message::user("real question"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "real answer".into(),
                }],
            },
        ],
        "durable summary",
    );
    assert_eq!(messages, live, "replay == live by construction");
}

/// Forged/malformed compaction ops: replay must tolerate them — no panics,
/// no history loss beyond the builder's own rules.
#[test]
fn forged_compaction_ops_are_replay_tolerant() {
    let turn = vec![
        OpEnvelope::new(
            "1",
            "now",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "keep me".into(),
                input_blocks: Vec::new(),
            },
        ),
        OpEnvelope::new(
            "2",
            "now",
            Op::TurnEnd {
                turn_id: "t1".into(),
                outcome: nano_session::TurnOutcome::Completed,
                usage: None,
            },
        ),
    ];
    // Complete with no Begin, empty summary.
    let mut forged = turn.clone();
    forged.push(OpEnvelope::new(
        "3",
        "now",
        Op::CompactionComplete {
            compaction_id: "forged".into(),
            summary: String::new(),
            covers_op_ids: vec![],
            changed_files: vec![],
            image_influenced: false,
            mcp_hydration: None,
        },
    ));
    let messages = acp_mode::messages_from_envelopes(&forged);
    assert_eq!(messages[0], Message::user("keep me"), "no history loss");
    // Complete as the VERY FIRST op (empty accumulation).
    let first = vec![OpEnvelope::new(
        "9",
        "now",
        Op::CompactionComplete {
            compaction_id: "f2".into(),
            summary: "out of nowhere".into(),
            covers_op_ids: vec!["not-real".into()],
            changed_files: vec![],
            image_influenced: false,
            mcp_hydration: None,
        },
    )];
    let messages = acp_mode::messages_from_envelopes(&first);
    assert_eq!(messages.len(), 1, "summary message only; no panic");
    // Cancel with unknown fields / a Complete carrying hostile unicode.
    let mut hostile = turn.clone();
    hostile.push(OpEnvelope::new(
        "4",
        "now",
        Op::CompactionComplete {
            compaction_id: "f3".into(),
            summary: "\u{1b}[2J\u{1b}[H wipe \u{202e} rtl".into(),
            covers_op_ids: vec![],
            changed_files: vec![],
            image_influenced: false,
            mcp_hydration: None,
        },
    ));
    let messages = acp_mode::messages_from_envelopes(&hostile);
    assert_eq!(messages[0], Message::user("keep me"));
    assert_eq!(messages.len(), 2);
}

/// The pending-assistant flush before the builder call (claude r2 build
/// brief): a CompactionComplete arriving while assistant blocks are still
/// buffered must not strand them into the builder's input — they are
/// flushed into `messages` first, and the builder then drops them by rule 3.
#[test]
fn replay_arm_flushes_pending_assistant_before_compaction() {
    let envelopes = vec![
        OpEnvelope::new(
            "1",
            "now",
            Op::TurnBegin {
                turn_id: "t1".into(),
                input: "q".into(),
                input_blocks: Vec::new(),
            },
        ),
        OpEnvelope::new(
            "2",
            "now",
            Op::AssistantText {
                turn_id: "t1".into(),
                text: "in-flight text".into(),
            },
        ),
        // No TurnEnd: the assistant block is still pending in the fold's
        // buffer when the compaction lands.
        OpEnvelope::new(
            "3",
            "now",
            Op::CompactionComplete {
                compaction_id: "k1".into(),
                summary: "s".into(),
                covers_op_ids: vec![],
                changed_files: vec![],
                image_influenced: false,
                mcp_hydration: None,
            },
        ),
    ];
    let messages = acp_mode::messages_from_envelopes(&envelopes);
    // The pending assistant text was flushed into the fold and then dropped
    // by the builder's wholesale-removal rule — the output is exactly the
    // user message + summary, not a corrupted partial fold.
    assert_eq!(
        messages,
        nano_agent::compact::build_compacted_history(vec![Message::user("q")], "s")
    );
}

/// A canary-bearing summary on the manual path: the gate rejects, the
/// journal carries CompactionCancel with a bounded reason, nothing
/// sensitive is persisted, and the session context is retained.
#[test]
fn canary_summary_on_manual_compact_fails_closed() {
    let dir = temp_sessions_dir("canary");
    let mut harness = Harness::spawn(
        vec![
            Step::Respond(text_response("first answer", 100)),
            Step::Respond(text_response("second answer", 100)),
        ],
        vec![Ok(text_response(
            "notes with wayland-nano-canary-a1b2c3d4 inside",
            0,
        ))],
        &dir,
        true,
    );
    harness.prompt("hello");
    let (compact, _updates) = harness.request_collecting(
        "session/compact",
        serde_json::json!({ "sessionId": harness.session_id }),
    );
    assert_eq!(compact["error"]["code"], -32603, "{compact}");
    assert!(
        compact["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("secret scan"),
        "{compact}"
    );
    let journal = harness.journal(&dir);
    let cancel = journal.iter().find_map(|e| match &e.op {
        Op::CompactionCancel { reason, .. } => Some(*reason),
        _ => None,
    });
    assert_eq!(cancel, Some(CompactionCancelReason::RedactionHit));
    assert!(
        !journal
            .iter()
            .any(|e| matches!(&e.op, Op::CompactionComplete { .. })),
        "nothing persisted"
    );
    // The canary string itself never reached the journal.
    let raw = std::fs::read_to_string(dir.join(format!("{}.jsonl", harness.session_id))).unwrap();
    assert!(!raw.contains("wayland-nano-canary"), "journal is clean");
    // The context is retained: the next prompt still carries the full
    // history (user + assistant + new input).
    harness.prompt("again");
    let requests = harness.model_requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert!(
        last.messages.iter().any(|m| m.role == Role::Assistant),
        "original history retained: {last:?}"
    );
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
