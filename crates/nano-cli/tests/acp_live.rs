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
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
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
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
}

impl Harness {
    fn spawn(script: Vec<Step>) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let driver_calls = Arc::new(AtomicU64::new(0));
        let tools = MockTools::default();
        let tool_calls = tools.calls.clone();
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            calls: driver_calls.clone(),
            release: release_rx,
        };
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
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
        };
        harness.session_id = harness.handshake();
        harness
    }

    /// initialize + session/new, returning the session id.
    fn handshake(&mut self) -> String {
        let init = self.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], 1, "init: {init}");
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

    // First tool frame landed; the model is parked on step 2.
    client.read_until(is_tool_call);

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
