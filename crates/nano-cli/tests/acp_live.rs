//! ACP live-I/O tests: a scripted fake client drives `acp_mode::serve`
//! in-process over channel-backed streams with a scripted model driver and
//! recording tools — no FLUX key, no network, no child process.
//!
//! Proves the three reworked behaviors:
//! 1. tool_call frames stream DURING the turn (the model is still parked when
//!    the first frame arrives — after-the-fact replay cannot pass this);
//! 2. session/cancel mid-turn is heard while the model call is in flight and
//!    the pending session/prompt is answered with stopReason "cancelled";
//! 3. mutating tools raise session/request_permission and honor the client's
//!    decision — deny (and malformed responses, fail-closed) means the tool
//!    never executes, while reads run without asking.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{
    ContentBlock, ModelError, ModelEvent, ModelRequest, ModelResponse, Role, ToolCall, Usage,
};
use nano_session::{JournalWriter, Op, OpEnvelope};
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── channel-backed streams ─────────────────────────────────────────────

/// Test → host stdin. Each sent string is one or more NDJSON lines.
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
                Err(_) => return Ok(&[]), // test hung up: EOF
            }
        }
        Ok(&self.buf[self.pos..])
    }

    fn consume(&mut self, amt: usize) {
        self.pos += amt;
    }
}

/// Host stdout → test. Emits one channel message per complete NDJSON line.
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

// ── scripted model + recording tools ───────────────────────────────────

#[derive(Debug)]
enum Step {
    Respond(ModelResponse),
    /// Park (async) until the test releases, then respond. While parked the
    /// host's select loop is free — this is what lets a mid-turn cancel land.
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

impl ToolExecutor for MockTools {
    fn execute(&self, call: &ToolCall) -> ToolOutcome {
        self.calls.lock().unwrap().push(call.clone());
        ToolOutcome {
            ok: true,
            output: format!("ran {}", call.name),
            progress: ProgressSignals::default(),
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

fn tool_call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args,
    }
}

// ── fake client harness ────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    release: tokio::sync::watch::Sender<bool>,
    driver_calls: Arc<AtomicU64>,
    tool_calls: Arc<Mutex<Vec<ToolCall>>>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
    /// The initialize response, kept so tests can assert advertised caps.
    init_response: serde_json::Value,
}

/// A unique temp dir per harness for the ACP session journals.
fn temp_sessions_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nano-acp-live-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp sessions dir");
    dir
}

impl Harness {
    fn spawn(script: Vec<Step>) -> Self {
        Self::spawn_with(script, &temp_sessions_dir("default"), true)
    }

    /// A fresh serve instance over the SAME sessions dir — the stand-in for a
    /// process restart. `new_session` mirrors the client's flow: a fresh
    /// conversation calls session/new; a resuming one goes straight to
    /// session/load (Desktop's AcpConnection.resumeSession path).
    fn spawn_with(script: Vec<Step>, sessions_dir: &std::path::Path, new_session: bool) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let driver_calls = Arc::new(AtomicU64::new(0));
        let tools = MockTools::default();
        let tool_calls = tools.calls.clone();
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            calls: driver_calls.clone(),
            release: release_rx,
            requests: model_requests.clone(),
        };
        let sessions_dir = sessions_dir.to_path_buf();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
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
                    &sessions_dir,
                    move || driver.clone(),
                    move |_| tools.clone(),
                    "mock",
                )
                .await
            })
        });
        let mut harness = Self {
            to_host: Some(in_tx),
            frames: out_rx,
            release: release_tx,
            driver_calls,
            tool_calls,
            model_requests,
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
            init_response: serde_json::Value::Null,
        };
        harness.init_response = harness.initialize();
        if new_session {
            harness.session_id = harness.new_session();
        }
        harness
    }

    fn initialize(&mut self) -> serde_json::Value {
        let init = self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], 1, "init: {init}");
        init
    }

    fn new_session(&mut self) -> String {
        let created = self.request(
            "session/new",
            serde_json::json!({ "cwd": ".", "mcpServers": [] }),
        );
        created["result"]["sessionId"]
            .as_str()
            .expect("sessionId")
            .to_string()
    }

    fn send(&self, value: serde_json::Value) {
        let tx = self.to_host.as_ref().expect("stdin open");
        tx.send(format!("{}\n", serde_json::to_string(&value).unwrap()))
            .expect("send to host");
    }

    /// Sends a request and reads frames until the response with its id
    /// arrives (interleaved notifications are fine). Returns the response.
    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        let frames = self.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(id));
        frames.last().expect("response frame").clone()
    }

    fn send_prompt(&mut self, session_id: &str, text: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }]
            },
        }));
        id
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
    /// (not on a frame's arrival) is what makes the mid-turn cancel tests
    /// deterministic.
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

    /// Reads frames until `pred` matches; the matching frame is the last
    /// element of the returned vec.
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

    fn executed_tool_names(&self) -> Vec<String> {
        self.tool_calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.name.clone())
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.to_host.take()); // stdin EOF → clean exit
        if let Some(handle) = self.handle.take() {
            let Ok(result) = handle.join() else {
                // Host thread panicked; the test's own assertion reports it.
                // Never panic here — a second panic during unwind aborts.
                return;
            };
            if !std::thread::panicking() {
                assert_eq!(result.expect("serve io"), 0, "clean exit code");
            }
        }
    }
}

fn is_tool_call(frame: &serde_json::Value) -> bool {
    frame["method"] == "session/update" && frame["params"]["update"]["sessionUpdate"] == "tool_call"
}

fn is_permission_request(frame: &serde_json::Value) -> bool {
    frame["method"] == "session/request_permission"
}

// ── tests ──────────────────────────────────────────────────────────────

#[test]
fn tool_frames_stream_during_turn_not_after() {
    // Step 2 parks until released: any frame seen before the release could
    // only have been written live. Batch replay would deadlock this test
    // into its timeout.
    let mut client = Harness::spawn(vec![
        Step::Respond(tool_response(tool_call(
            "c1",
            "fs_read",
            serde_json::json!({"path": "note.txt"}),
        ))),
        Step::WaitForRelease(text_response("all done")),
    ]);
    let session_id = client.session_id.clone();
    let prompt_id = client.send_prompt(&session_id, "read the note");

    // The tool_call frame arrives while the model is still parked on step 2.
    let frames = client.read_until(is_tool_call);
    assert!(
        frames
            .iter()
            .all(|f| f.get("id").and_then(|v| v.as_u64()) != Some(prompt_id)),
        "no prompt response may precede the live tool frame: {frames:?}"
    );
    let tool = frames.last().unwrap();
    assert_eq!(tool["params"]["update"]["toolCallId"], "c1");
    assert_eq!(tool["params"]["update"]["status"], "in_progress");

    // Unpark the model; the turn completes with the text chunk streamed
    // before the final prompt response.
    client.release_model();
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    let response = frames.last().unwrap();
    assert_eq!(
        response["result"]["stopReason"], "end_turn",
        "response: {response}"
    );
    assert!(
        frames.iter().any(
            |f| f["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && f["params"]["update"]["status"] == "completed"
        ),
        "tool completion must stream: {frames:?}"
    );
    assert!(
        frames.iter().any(
            |f| f["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                && f["params"]["update"]["content"]["text"] == "all done"
        ),
        "final text must stream: {frames:?}"
    );
}

#[test]
fn cancel_mid_turn_answers_cancelled_and_stops_stream() {
    let mut client = Harness::spawn(vec![
        Step::Respond(tool_response(tool_call(
            "c1",
            "fs_read",
            serde_json::json!({"path": "a.txt"}),
        ))),
        Step::WaitForRelease(tool_response(tool_call(
            "c2",
            "fs_read",
            serde_json::json!({"path": "b.txt"}),
        ))),
        Step::Respond(text_response("never reached")),
    ]);
    let session_id = client.session_id.clone();
    let prompt_id = client.send_prompt(&session_id, "read everything");

    // First tool frame landed; wait until the model is REALLY parked inside
    // the second call (a frame's arrival alone does not prove the engine has
    // looped around to it — journaling I/O can shift that timing).
    client.read_until(is_tool_call);
    client.wait_for_driver_calls(2);

    // Cancel while the model call is in flight, then let it return.
    client.send(serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": { "sessionId": session_id },
    }));
    client.release_model();

    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    let response = frames.last().unwrap();
    assert_eq!(
        response["result"]["stopReason"], "cancelled",
        "cancelled prompt must answer stopReason cancelled: {response}"
    );
    assert_eq!(
        client.driver_calls.load(Ordering::SeqCst),
        2,
        "the third scripted model call must never happen after cancel"
    );
    assert!(
        client
            .frames
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "no frames may follow the cancelled response"
    );
}

#[test]
fn permission_round_trip_denies_write_and_skips_reads() {
    let mut client = Harness::spawn(vec![
        // Turn 1: a write (must ask), then a read (must not ask), then text.
        Step::Respond(tool_response(tool_call(
            "c1",
            "fs_write",
            serde_json::json!({"path": "out.txt", "content": "x"}),
        ))),
        Step::Respond(tool_response(tool_call(
            "c2",
            "fs_read",
            serde_json::json!({"path": "note.txt"}),
        ))),
        Step::Respond(text_response("turn one done")),
        // Turn 2: an edit (must ask) answered with a malformed response.
        Step::Respond(tool_response(tool_call(
            "c3",
            "fs_edit",
            serde_json::json!({"path": "out.txt"}),
        ))),
        Step::Respond(text_response("turn two done")),
    ]);
    let session_id = client.session_id.clone();
    let prompt_id = client.send_prompt(&session_id, "write then read");

    // The write blocks on a permission request whose shape Desktop expects.
    let frames = client.read_until(is_permission_request);
    let request = frames.last().unwrap();
    let permission_id = request["id"].as_u64().expect("numeric request id");
    assert_eq!(request["params"]["sessionId"].as_str().unwrap(), session_id);
    assert_eq!(request["params"]["toolCall"]["toolCallId"], "c1");
    assert_eq!(request["params"]["toolCall"]["title"], "fs_write");
    let options = request["params"]["options"].as_array().expect("options");
    assert!(
        options
            .iter()
            .any(|o| o["optionId"] == "allow" && o["kind"] == "allow_once")
            && options
                .iter()
                .any(|o| o["optionId"] == "deny" && o["kind"] == "reject_once"),
        "options: {options:?}"
    );
    assert!(
        client.tool_calls.lock().unwrap().is_empty(),
        "fs_write must not execute before the client answers"
    );

    // Client denies — the exact shape Desktop sends for a user reject of the
    // "deny" option (AcpConnection maps it to outcome selected + optionId).
    client.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": permission_id,
        "result": { "outcome": { "outcome": "selected", "optionId": "deny" } },
    }));

    // The turn continues: the read runs WITHOUT a new permission request,
    // and the prompt completes end_turn.
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(
        frames.iter().filter(|f| is_permission_request(f)).count(),
        0,
        "reads must not raise permission requests: {frames:?}"
    );
    assert!(
        frames
            .iter()
            .any(|f| is_tool_call(f) && f["params"]["update"]["toolCallId"] == "c2"),
        "the read must still run: {frames:?}"
    );
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    assert_eq!(
        client.executed_tool_names(),
        vec!["fs_read".to_string()],
        "denied fs_write must never execute; fs_read must"
    );

    // Turn 2: a malformed permission response denies by default.
    let prompt_id = client.send_prompt(&session_id, "edit the file");
    let frames = client.read_until(is_permission_request);
    let permission_id = frames.last().unwrap()["id"].as_u64().unwrap();
    client.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": permission_id,
        "result": { "bogus": true },
    }));
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    assert_eq!(
        client.executed_tool_names(),
        vec!["fs_read".to_string()],
        "malformed permission response must deny (fail-closed): fs_edit never runs"
    );
}

// ── session/load resume ────────────────────────────────────────────────

/// The honesty gate for `loadSession: true`: a turn journaled by one serve
/// instance is restored by a NEW serve instance over the same sessions dir
/// (the process boundary), the replay arrives in order with final statuses,
/// and the follow-up prompt's model request provably carries the prior
/// turn's context — resume, not just replay.
#[test]
fn session_load_restores_context_across_serve_instances() {
    let sessions_dir = temp_sessions_dir("resume");
    let session_id;
    // First "process": one turn (read + answer), then stdin EOF ends it.
    {
        let mut client = Harness::spawn_with(
            vec![
                Step::Respond(tool_response(tool_call(
                    "c1",
                    "fs_read",
                    serde_json::json!({"path": "note.txt"}),
                ))),
                Step::Respond(text_response("the note says hello")),
            ],
            &sessions_dir,
            true,
        );
        session_id = client.session_id.clone();
        assert_ne!(
            session_id,
            format!("wayland-nano-session-{}", std::process::id()),
            "session ids must not be pid-based"
        );
        let prompt_id = client.send_prompt(&session_id, "what does the note say?");
        let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
        assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    } // drop: EOF → clean exit; the journal is all that survives

    // Second "process": same sessions dir, no session/new — straight to load,
    // exactly Desktop's resume path (AcpConnection.resumeSession).
    let mut client = Harness::spawn_with(
        vec![Step::Respond(text_response("you quoted it earlier"))],
        &sessions_dir,
        false,
    );
    // The capability this whole test justifies advertising.
    assert_eq!(
        client.init_response["result"]["agentCapabilities"]["loadSession"], true,
        "loadSession must only be advertised once resume is proven: {}",
        client.init_response
    );

    // Unknown id: typed error, no panic, host stays usable afterwards.
    let missing = client.request(
        "session/load",
        serde_json::json!({
            "sessionId": "wayland-nano-session-never-existed",
            "cwd": ".",
            "mcpServers": []
        }),
    );
    assert_eq!(
        missing["error"]["code"], -32602,
        "unknown session must be a typed error Desktop can fall back on: {missing}"
    );

    // Known id: replay notifications arrive IN JOURNAL ORDER, before the
    // load response resolves.
    let load_id = client.next_id;
    client.next_id += 1;
    client.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": load_id,
        "method": "session/load",
        "params": { "sessionId": session_id, "cwd": ".", "mcpServers": [] },
    }));
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(load_id));
    let response = frames.last().unwrap();
    assert!(
        response.get("error").is_none() && response.get("result").is_some(),
        "load of a known session must succeed: {response}"
    );
    let replay = &frames[..frames.len() - 1];
    let kinds: Vec<&str> = replay
        .iter()
        .map(|f| {
            f["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap_or("not-a-session-update")
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "user_message_chunk",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk"
        ],
        "replay frames, in order: {frames:?}"
    );
    assert_eq!(
        replay[0]["params"]["update"]["content"]["text"],
        "what does the note say?"
    );
    assert_eq!(replay[1]["params"]["update"]["toolCallId"], "c1");
    assert_eq!(replay[1]["params"]["update"]["status"], "completed");
    assert_eq!(replay[2]["params"]["update"]["toolCallId"], "c1");
    assert_eq!(replay[2]["params"]["update"]["status"], "completed");
    assert_eq!(
        replay[3]["params"]["update"]["content"]["text"],
        "the note says hello"
    );

    // The decisive assertion: the NEXT prompt's model request must contain
    // the prior turn's context — user text, assistant text, tool use and a
    // (digest-elided) tool result — proving resume, not just replay.
    let prompt_id = client.send_prompt(&session_id, "what did it say again?");
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    let requests = client.model_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "one scripted model call for the follow-up"
    );
    let messages = &requests[0].messages;
    assert!(
        messages.iter().any(|m| matches!(&m.role, Role::User)
            && m.content.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text == "what does the note say?")
            )),
        "prior user turn must be in the model request: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| matches!(&m.role, Role::Assistant)
            && m.content.iter().any(
                |b| matches!(b, ContentBlock::Text { text } if text == "the note says hello")
            )),
        "prior assistant text must be in the model request: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { id, name, .. } if id == "c1" && name == "fs_read"))),
        "prior tool use must be in the model request: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| matches!(&m.role, Role::Tool)
            && m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, is_error, .. } if tool_use_id == "c1" && !is_error))),
        "prior tool result must be in the model request: {messages:?}"
    );
    let last = messages.last().expect("at least the new prompt");
    assert!(
        matches!(&last.role, Role::User)
            && matches!(&last.content[0], ContentBlock::Text { text } if text == "what did it say again?"),
        "the new prompt must be the final message: {last:?}"
    );
    drop(requests);

    drop(client); // EOF → clean exit before the temp dir goes away
    let _ = std::fs::remove_dir_all(&sessions_dir);
}

/// The crash branch of the replay: a journaled ToolCall whose ToolResult
/// never landed (the process died mid-call) must replay as status "failed" —
/// never "completed", never a card hanging in_progress — and the restored
/// model context must show the ToolUse WITHOUT a fabricated result. A
/// normally completed call in the SAME journal is the control: it must still
/// replay as completed.
#[test]
fn session_load_replays_interrupted_tool_call_as_failed() {
    let sessions_dir = temp_sessions_dir("interrupted");
    let session_id;
    // First "process": one clean turn (read + answer) — the control call.
    {
        let mut client = Harness::spawn_with(
            vec![
                Step::Respond(tool_response(tool_call(
                    "c1",
                    "fs_read",
                    serde_json::json!({"path": "note.txt"}),
                ))),
                Step::Respond(text_response("the note says hello")),
            ],
            &sessions_dir,
            true,
        );
        session_id = client.session_id.clone();
        let prompt_id = client.send_prompt(&session_id, "what does the note say?");
        let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
        assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    } // drop: EOF → clean exit; the journal is all that survives

    // Crash state: turn 2 began and its tool call journaled, then the process
    // died before the ToolResult landed. JournalWriter — the same append-only
    // writer the live path uses — appends exactly that tail, so the journal
    // ends with a ToolCall op and NO matching ToolResult.
    let journal = sessions_dir.join(format!("{session_id}.jsonl"));
    let mut writer = JournalWriter::open(&journal).expect("journal reopens");
    assert!(
        writer
            .append(&OpEnvelope::new(
                format!("{session_id}-crash-turnbegin-2"),
                "now",
                Op::TurnBegin {
                    turn_id: format!("{session_id}-turn-2"),
                    input: "check the note again".into(),
                },
            ))
            .expect("append crashed TurnBegin"),
        "crash TurnBegin must be a new append"
    );
    assert!(
        writer
            .append(&OpEnvelope::new(
                format!("{session_id}-crash-toolcall-c2"),
                "now",
                Op::ToolCall {
                    turn_id: format!("{session_id}-turn-2"),
                    call_id: "c2".into(),
                    name: "fs_read".into(),
                    args: serde_json::json!({"path": "note.txt"}),
                },
            ))
            .expect("append crashed ToolCall"),
        "crash ToolCall must be a new append"
    );
    drop(writer); // never a ToolResult for c2: the crash cut the journal here

    // Second "process": same sessions dir, straight to session/load.
    let mut client = Harness::spawn_with(
        vec![Step::Respond(text_response("still hello"))],
        &sessions_dir,
        false,
    );
    let load_id = client.next_id;
    client.next_id += 1;
    client.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": load_id,
        "method": "session/load",
        "params": { "sessionId": session_id, "cwd": ".", "mcpServers": [] },
    }));
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(load_id));
    let response = frames.last().unwrap();
    assert!(
        response.get("error").is_none() && response.get("result").is_some(),
        "load of a crash-cut session must succeed overall: {response}"
    );
    let replay = &frames[..frames.len() - 1];
    let kinds: Vec<&str> = replay
        .iter()
        .map(|f| {
            f["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap_or("not-a-session-update")
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "user_message_chunk",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "user_message_chunk",
            "tool_call"
        ],
        "replay frames, in order: {frames:?}"
    );
    // Control: the completed call replays as completed, digest update included.
    assert_eq!(replay[1]["params"]["update"]["toolCallId"], "c1");
    assert_eq!(replay[1]["params"]["update"]["status"], "completed");
    assert_eq!(replay[2]["params"]["update"]["toolCallId"], "c1");
    assert_eq!(replay[2]["params"]["update"]["status"], "completed");
    assert_eq!(
        replay[4]["params"]["update"]["content"]["text"],
        "check the note again"
    );
    // The interrupted call: replayed as failed, and NO completion update may
    // exist for it anywhere in the replay (no fabricated digest).
    assert_eq!(replay[5]["params"]["update"]["toolCallId"], "c2");
    assert_eq!(
        replay[5]["params"]["update"]["status"], "failed",
        "a ToolCall with no journaled ToolResult must replay as failed: {frames:?}"
    );
    assert!(
        !replay.iter().any(
            |f| f["params"]["update"]["sessionUpdate"] == "tool_call_update"
                && f["params"]["update"]["toolCallId"] == "c2"
        ),
        "the interrupted call must not gain a fabricated completion update: {frames:?}"
    );

    // The follow-up prompt's model request pins the honest context behavior:
    // the ToolUse is there, the result is simply ABSENT (no fabricated
    // payload, no failure-marker message) — the trailing assistant message
    // carries exactly the bare ToolUse block and nothing else.
    let prompt_id = client.send_prompt(&session_id, "and what was it?");
    let frames = client.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    let requests = client.model_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "one scripted model call for the follow-up"
    );
    let messages = &requests[0].messages;
    let interrupted = messages
        .iter()
        .find(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == "c2"))
        })
        .expect("the interrupted ToolUse must be in the model request");
    assert!(
        matches!(&interrupted.role, Role::Assistant)
            && interrupted.content.len() == 1
            && matches!(
                &interrupted.content[0],
                ContentBlock::ToolUse { id, name, .. } if id == "c2" && name == "fs_read"
            ),
        "the interrupted call must appear as a bare ToolUse, nothing fabricated beside it: {interrupted:?}"
    );
    assert!(
        !messages.iter().any(|m| m.content.iter().any(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "c2")
        )),
        "no tool result may be fabricated for the interrupted call: {messages:?}"
    );
    // Control in context: the completed call's (digest-elided) result IS there.
    assert!(
        messages.iter().any(|m| matches!(&m.role, Role::Tool)
            && m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, is_error, .. } if tool_use_id == "c1" && !is_error))),
        "the completed call's result must be in the model request: {messages:?}"
    );
    drop(requests);

    drop(client); // EOF → clean exit before the temp dir goes away
    let _ = std::fs::remove_dir_all(&sessions_dir);
}
