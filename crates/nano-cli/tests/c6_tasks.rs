//! C6 background tasks — wire-level integration against the real
//! `acp_mode::serve` host loop (routed scripted model shared by parent and
//! child turns, real workspace copy, real child journals):
//! - spawn → poll → result → apply lifecycle over ordinary ACP frames;
//! - the child gate: children can write to their COPY but get an immediate
//!   typed denial for task_spawn (depth) and interactive-only tools;
//! - journal separation: the parent journal carries only task_* ops; each
//!   child journal replays standalone;
//! - parent cancel cascades to children;
//! - wire regression: no sub-agent frames, only ordinary tool_call frames.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::acp::AvailableModel;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

// ── channel-backed streams (the c2/c5 pattern) ──────────────────────────

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

// ── routed scripted model + mock core tools ─────────────────────────────

/// Routes on the LAST user text (parent and child turns share one driver).
/// Supports `$TASK_ID` substitution in scripted tool-call arguments from a
/// prior "spawned task-..." tool result in the request context.
#[derive(Debug, Clone)]
struct RoutedDriver {
    routes: Arc<Mutex<HashMap<String, VecDeque<ModelResponse>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    /// Calls whose route key matches block until this fires (slow children).
    block_key: Option<String>,
    release: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl RoutedDriver {
    fn last_user_text(request: &ModelRequest) -> String {
        request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == nano_model::types::Role::User)
            .and_then(|m| {
                m.content.iter().find_map(|b| match b {
                    nano_model::types::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default()
    }

    /// The task id a prior task_spawn result announced — searched across ALL
    /// recorded requests (later turns rebuild context from the journal,
    /// where tool payloads are elided by design).
    fn spawned_task_id(&self) -> Option<String> {
        self.requests.lock().unwrap().iter().find_map(|r| {
            r.messages.iter().find_map(|m| {
                m.content.iter().find_map(|b| match b {
                    nano_model::types::ContentBlock::ToolResult { content, .. } => content
                        .strip_prefix("spawned ")
                        .map(|rest| rest.trim().to_string()),
                    _ => None,
                })
            })
        })
    }
}

#[async_trait::async_trait]
impl ModelDriver for RoutedDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        let key = Self::last_user_text(request);
        if let (Some(block), Some(release)) = (&self.block_key, &self.release)
            && *block == key
        {
            while !release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let mut response = self
            .routes
            .lock()
            .unwrap()
            .get_mut(&key)
            .and_then(|q| q.pop_front())
            .ok_or_else(|| ModelError::Protocol(format!("no scripted response for {key:?}")))?;
        // $TASK_ID substitution from the spawn tool result in context.
        if let Some(task_id) = self.spawned_task_id() {
            for event in &mut response.events {
                if let ModelEvent::ToolCallComplete(call) = event {
                    call.arguments = serde_json::from_str(
                        &call.arguments.to_string().replace("$TASK_ID", &task_id),
                    )
                    .expect("substituted args parse");
                }
            }
        }
        Ok(response)
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

fn route(text: &str, responses: Vec<ModelResponse>) -> (String, VecDeque<ModelResponse>) {
    (text.to_string(), responses.into())
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
    fn spawn(driver: RoutedDriver, sessions_dir: &std::path::Path) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let model_requests = driver.requests.clone();
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

    /// The tool-result texts the model has seen (context inspection).
    fn tool_results_seen(&self) -> Vec<String> {
        self.model_requests
            .lock()
            .unwrap()
            .iter()
            .flat_map(|r| {
                r.messages.iter().flat_map(|m| {
                    m.content.iter().filter_map(|b| match b {
                        nano_model::types::ContentBlock::ToolResult { content, .. } => {
                            Some(content.clone())
                        }
                        _ => None,
                    })
                })
            })
            .collect()
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
            "nano-c6-wire-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(root.join("sessions")).expect("sessions dir");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        std::fs::write(workspace.join("seed.txt"), "seed").unwrap();
        Self { root }
    }

    fn sessions(&self) -> std::path::PathBuf {
        self.root.join("sessions")
    }

    fn workspace(&self) -> std::path::PathBuf {
        self.root.join("workspace")
    }

    fn tasks(&self) -> std::path::PathBuf {
        self.root.join("tasks")
    }

    /// Wait until some task dir contains a report.md (child completed).
    fn wait_for_report(&self) -> std::path::PathBuf {
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            if let Ok(entries) = std::fs::read_dir(self.tasks()) {
                for entry in entries.flatten() {
                    let report = entry.path().join("report.md");
                    if report.exists() {
                        return entry.path();
                    }
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no child report landed"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for TestDirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ── the tests ───────────────────────────────────────────────────────────

/// The full lifecycle: spawn (permission-gated) → child writes into its
/// COPY → result poll carries the report + changed_files → apply copies the
/// recorded file back into the parent workspace. Plus: journal separation
/// and the ordinary-frames-only wire regression.
#[test]
fn task_lifecycle_over_the_wire() {
    let dirs = TestDirs::new();
    let driver = RoutedDriver {
        routes: Arc::new(Mutex::new(HashMap::from([
            route(
                "spawn a task",
                vec![
                    tool_response(ToolCall {
                        id: "s1".into(),
                        name: "task_spawn".into(),
                        arguments: serde_json::json!({"prompt": "child job", "label": "demo"}),
                    }),
                    text_response("spawned it"),
                ],
            ),
            route(
                "child job",
                vec![
                    tool_response(ToolCall {
                        id: "w1".into(),
                        name: "fs_write".into(),
                        arguments: serde_json::json!({"path": "note.txt", "content": "from the child"}),
                    }),
                    text_response("child wrote note.txt"),
                ],
            ),
            route(
                "collect",
                vec![
                    tool_response(ToolCall {
                        id: "r1".into(),
                        name: "task_result".into(),
                        arguments: serde_json::json!({"task_id": "$TASK_ID"}),
                    }),
                    text_response("got the report"),
                ],
            ),
            route(
                "apply it",
                vec![
                    tool_response(ToolCall {
                        id: "a1".into(),
                        name: "task_apply".into(),
                        arguments: serde_json::json!({"task_id": "$TASK_ID"}),
                    }),
                    text_response("applied"),
                ],
            ),
        ]))),
        requests: Arc::new(Mutex::new(Vec::new())),
        block_key: None,
        release: None,
    };
    let mut host = Harness::spawn(driver, &dirs.sessions());
    let session_id = host.new_session(&dirs.workspace());

    // Turn 1: spawn (the gate prompts; the harness allows).
    let response = host.prompt(&session_id, "spawn a task");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let task_dir = dirs.wait_for_report();

    // Turn 2: result poll carries the child's report + changed_files.
    let response = host.prompt(&session_id, "collect");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    let seen = host.tool_results_seen();
    let result_text = seen
        .iter()
        .find(|t| t.contains("child wrote note.txt"))
        .expect("child report reached the parent model");
    assert!(
        result_text.contains("changed_files: note.txt"),
        "{result_text}"
    );

    // Turn 3: apply copies the recorded file back.
    let response = host.prompt(&session_id, "apply it");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        std::fs::read_to_string(dirs.workspace().join("note.txt")).unwrap(),
        "from the child",
        "task_apply copied the recorded file into the parent workspace"
    );

    // Journal separation: the parent journal carries ONLY task_* tool ops;
    // the child journal replays standalone with its own turn.
    let journal = dirs.sessions().join(format!("{session_id}.jsonl"));
    let report = nano_session::reader::read_journal(&journal).expect("parent journal reads");
    for envelope in &report.envelopes {
        if let nano_session::op::Op::ToolCall { name, .. } = &envelope.op {
            assert!(
                name.starts_with("task_"),
                "child ops must never enter the parent journal, saw {name}"
            );
        }
    }
    let child_journal = task_dir.join("journal.jsonl");
    let child = nano_session::reader::read_journal(&child_journal).expect("child journal reads");
    assert!(
        child.envelopes.iter().any(
            |e| matches!(&e.op, nano_session::op::Op::ToolCall { name, .. } if name == "fs_write")
        ),
        "the child's own journal carries its write"
    );
    assert!(
        child
            .envelopes
            .iter()
            .any(|e| matches!(e.op, nano_session::op::Op::TurnEnd { .. })),
        "child turn completed"
    );

    // Wire regression: every session/update was an ordinary kind.
    for frame in &host.seen {
        if let Some(kind) = frame
            .get("params")
            .and_then(|p| p.get("update"))
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(|k| k.as_str())
        {
            assert!(
                matches!(
                    kind,
                    "tool_call" | "tool_call_update" | "agent_message_chunk" | "user_message_chunk"
                ),
                "no sub-agent frames on the wire, saw {kind}"
            );
        }
    }
    host.shutdown();
}

/// Depth limit over the wire: a child asked to call task_spawn gets an
/// immediate typed denial (the family is absent from its tool definitions
/// and its gate denies it), completes its turn, and spawns NOTHING.
#[test]
fn child_cannot_spawn_a_grandchild() {
    let dirs = TestDirs::new();
    let driver = RoutedDriver {
        routes: Arc::new(Mutex::new(HashMap::from([
            route(
                "spawn",
                vec![
                    tool_response(ToolCall {
                        id: "s1".into(),
                        name: "task_spawn".into(),
                        arguments: serde_json::json!({"prompt": "recurse please"}),
                    }),
                    text_response("spawned"),
                ],
            ),
            route(
                "recurse please",
                vec![
                    tool_response(ToolCall {
                        id: "deep".into(),
                        name: "task_spawn".into(),
                        arguments: serde_json::json!({"prompt": "grandchild"}),
                    }),
                    text_response("I could not spawn"),
                ],
            ),
        ]))),
        requests: Arc::new(Mutex::new(Vec::new())),
        block_key: None,
        release: None,
    };
    let mut host = Harness::spawn(driver, &dirs.sessions());
    let session_id = host.new_session(&dirs.workspace());
    host.prompt(&session_id, "spawn");
    let task_dir = dirs.wait_for_report();
    host.shutdown();

    // Exactly ONE task dir exists (no grandchild), and the child's journal
    // carries the task_spawn call as a FAILED result — C7's certified
    // journal-the-denial rule supersedes the original deny-before-journal
    // assertion: the denial is now an audit record, never silent. The
    // invariant that matters — no grandchild spawn — is unchanged.
    let task_count = std::fs::read_dir(dirs.tasks()).unwrap().count();
    assert_eq!(task_count, 1, "no grandchild task");
    let child = nano_session::reader::read_journal(&task_dir.join("journal.jsonl")).unwrap();
    let spawn_call = child.envelopes.iter().find_map(|e| match &e.op {
        nano_session::op::Op::ToolCall { call_id, name, .. } if name == "task_spawn" => {
            Some(call_id.clone())
        }
        _ => None,
    });
    let spawn_call = spawn_call.expect("the denied task_spawn is journaled (C7 denial audit)");
    let denied = child.envelopes.iter().any(|e| match &e.op {
        nano_session::op::Op::ToolResult { call_id, ok, .. } => call_id == &spawn_call && !ok,
        _ => false,
    });
    assert!(denied, "the journaled task_spawn has a failed result");
    let report = std::fs::read_to_string(task_dir.join("report.md")).unwrap();
    assert!(report.contains("I could not spawn"), "{report}");
}

/// Parent cancel cascades: a session/cancel mid-turn flags the running
/// child too; the child ends cancelled (its own flag, never shared).
#[test]
fn parent_cancel_cascades_to_children() {
    let dirs = TestDirs::new();
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let driver = RoutedDriver {
        routes: Arc::new(Mutex::new(HashMap::from([
            route(
                "spawn slow",
                vec![
                    tool_response(ToolCall {
                        id: "s1".into(),
                        name: "task_spawn".into(),
                        arguments: serde_json::json!({"prompt": "slow child"}),
                    }),
                    text_response("spawned"),
                ],
            ),
            route("slow child", vec![text_response("never reached")]),
            route("check", vec![text_response("checked")]),
        ]))),
        requests: Arc::new(Mutex::new(Vec::new())),
        block_key: Some("slow child".to_string()),
        release: Some(release.clone()),
    };
    let mut host = Harness::spawn(driver, &dirs.sessions());
    let session_id = host.new_session(&dirs.workspace());
    host.prompt(&session_id, "spawn slow");
    // Wait for the child thread to be inside its (blocked) model call, then
    // cancel the session and release the child: it must end CANCELLED.
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !dirs.tasks().exists()
        || std::fs::read_dir(dirs.tasks())
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
    {
        assert!(std::time::Instant::now() < deadline, "child never spawned");
        std::thread::sleep(Duration::from_millis(25));
    }
    std::thread::sleep(Duration::from_millis(200)); // let the child start its turn
    host.send(serde_json::json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session_id}}));
    release.store(true, Ordering::SeqCst);
    let task_dir = dirs.wait_for_report();
    let report = std::fs::read_to_string(task_dir.join("report.md")).unwrap();
    assert!(
        report.contains("state: cancelled"),
        "cascaded cancel ended the child: {report}"
    );
    host.prompt(&session_id, "check"); // host still healthy after the cascade
    host.shutdown();
}
