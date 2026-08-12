//! C10 agent-UX pack — ACP end-to-end tests: scripted fake client driving
//! `acp_mode::serve` in-process (the acp_live.rs harness pattern), no FLUX
//! key, no network, no child process.
//!
//! Proves, over the real wire + journal:
//! 1. plan posture: tool-driven entry journals PlanSet and the GATE denies
//!    a workspace write with NO permission prompt (categorical), while the
//!    plan file itself auto-approves; ACP set_mode "plan" produces the
//!    identical journal + cell state (entry-path equivalence); a
//!    privilege-mode set exits the posture (PlanSet false);
//! 2. todo: write journals TodoSet (journal-first), a kill + session/load
//!    restores the list into the rebuilt context (bounded restore block);
//! 3. ask_user: the question rides session/request_permission with minted
//!    opt_{i} ids + Dismiss; a selected answer resolves to the LABEL as the
//!    tool result; a dismiss fails closed to a typed error.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::acp::AvailableModel;
use nano_session::op::Op;
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

// ── channel-backed streams (acp_live.rs pattern) ───────────────────────

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

// ── scripted model + recording tools ───────────────────────────────────

#[derive(Debug, Clone)]
struct MockDriver {
    script: Arc<Mutex<VecDeque<ModelResponse>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("mock driver script exhausted"))
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

// ── harness ────────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    tool_calls: Arc<Mutex<Vec<ToolCall>>>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
    /// The driver's remaining script: push more responses between turns
    /// (e.g. once the session-dependent plan path is known).
    script: Arc<Mutex<VecDeque<ModelResponse>>>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
    sessions_dir: std::path::PathBuf,
}

fn temp_sessions_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nano-acp-c10-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp sessions dir");
    dir
}

impl Harness {
    fn spawn(
        script: Vec<ModelResponse>,
        sessions_dir: &std::path::Path,
        resume: Option<&str>,
    ) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let tools = MockTools::default();
        let tool_calls = tools.calls.clone();
        let model_requests = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::from(script)));
        let driver = MockDriver {
            script: script.clone(),
            requests: model_requests.clone(),
        };
        let sessions_dir = sessions_dir.to_path_buf();
        let sessions_dir_for_thread = sessions_dir.clone();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                let sandbox_probe = || true;
                let catalog = vec![AvailableModel {
                    id: "mock".into(),
                    name: "mock".into(),
                }];
                let router = nano_cli::provider_router::ProviderRouter::default();
                // C8: per-turn binding resolution needs SOME flux credential
                // in env (never networked — the scripted driver intercepts).
                ensure_test_flux_key();
                // C5: memory store for this harness (writes off).
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir_for_thread
                        .parent()
                        .expect("root")
                        .join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                };
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir_for_thread,
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
            tool_calls,
            model_requests,
            script,
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
            sessions_dir,
        };
        let init = harness.request("initialize", serde_json::json!({"protocolVersion": 1}));
        assert_eq!(init["result"]["protocolVersion"], 1);
        match resume {
            Some(id) => {
                let loaded = harness.request(
                    "session/load",
                    serde_json::json!({"sessionId": id, "cwd": ".", "mcpServers": []}),
                );
                assert!(
                    loaded.get("error").is_none(),
                    "session/load failed: {loaded}"
                );
                harness.session_id = id.to_string();
            }
            None => {
                let new = harness.request(
                    "session/new",
                    serde_json::json!({"cwd": ".", "mcpServers": []}),
                );
                harness.session_id = new["result"]["sessionId"]
                    .as_str()
                    .expect("sessionId")
                    .to_string();
            }
        }
        harness
    }

    fn push_script(&self, responses: Vec<ModelResponse>) {
        self.script.lock().unwrap().extend(responses);
    }

    fn send(&self, value: serde_json::Value) {
        self.to_host
            .as_ref()
            .expect("stdin open")
            .send(format!("{}\n", serde_json::to_string(&value).unwrap()))
            .expect("send to host");
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        let frames = self.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(id));
        frames.last().expect("response frame").clone()
    }

    fn read_until(&mut self, pred: impl Fn(&serde_json::Value) -> bool) -> Vec<serde_json::Value> {
        let mut frames = Vec::new();
        let deadline = std::time::Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(remaining > Duration::ZERO, "timed out waiting for a frame");
            let line = self
                .frames
                .recv_timeout(remaining)
                .expect("host frame timeout");
            let frame: serde_json::Value = serde_json::from_str(&line).expect("json frame");
            let done = pred(&frame);
            frames.push(frame);
            if done {
                return frames;
            }
        }
    }

    /// Send session/prompt and return its request id (the response arrives
    /// when the turn completes).
    fn send_prompt(&mut self, text: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": self.session_id, "prompt": [{"type": "text", "text": text}]},
        }));
        id
    }

    /// Read until the prompt response for `id` arrives; return every frame.
    fn finish_prompt(&mut self, id: u64) -> Vec<serde_json::Value> {
        self.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(id))
    }

    fn set_mode(&mut self, mode: &str) -> serde_json::Value {
        self.request(
            "session/set_mode",
            serde_json::json!({"sessionId": self.session_id, "modeId": mode}),
        )
    }

    fn plan_file(&self) -> std::path::PathBuf {
        self.sessions_dir
            .join(format!("{}.plan.md", self.session_id))
    }

    fn journal_ops(&self) -> Vec<Op> {
        let journal = self.sessions_dir.join(format!("{}.jsonl", self.session_id));
        nano_session::reader::read_journal(&journal)
            .expect("journal readable")
            .envelopes
            .into_iter()
            .map(|e| e.op)
            .collect()
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

// ── 1. plan posture ────────────────────────────────────────────────────

/// Tool-driven entry: enter_plan_mode flips the posture (journaled PlanSet)
/// and the GATE then denies a workspace fs_write with NO permission prompt
/// (categorical), while the plan file itself auto-approves. ACP set_mode
/// "plan" produces the identical journal + cell state (entry-path
/// equivalence). A privilege-mode set exits the posture (PlanSet false).
#[test]
fn plan_posture_enforced_at_the_gate_end_to_end() {
    let sessions_dir = temp_sessions_dir("plan");
    let script = vec![
        // Turn 1: enter plan mode, then try a workspace write (denied at
        // the gate), then answer.
        tool_response(tool_call("c1", "enter_plan_mode", serde_json::json!({}))),
        tool_response(tool_call(
            "c2",
            "fs_write",
            serde_json::json!({"path": "src/main.rs", "content": "pwn"}),
        )),
        text_response("workspace write denied as expected"),
    ];
    let mut harness = Harness::spawn(script, &sessions_dir, None);

    // Turn 1: tool-driven entry + denied workspace write.
    let prompt = harness.send_prompt("plan the change");
    let frames = harness.finish_prompt(prompt);
    let executed = harness.tool_calls.lock().unwrap().clone();
    assert!(
        executed.iter().all(|c| c.name != "enter_plan_mode"),
        "session tool must not reach the base executor: {executed:?}"
    );
    assert!(
        executed.iter().all(|c| c.name != "fs_write"),
        "denied write must not execute: {executed:?}"
    );
    assert!(
        !frames
            .iter()
            .any(|f| f.get("method").and_then(|m| m.as_str())
                == Some("session/request_permission")),
        "a plan-posture denial must never prompt"
    );
    assert!(
        harness
            .journal_ops()
            .iter()
            .any(|op| matches!(op, Op::PlanSet { active: true })),
        "PlanSet(true) journaled"
    );
    // The denial names the posture, so the model learns WHY.
    let denial_seen = harness
        .model_requests
        .lock()
        .unwrap()
        .iter()
        .any(|req| format!("{:?}", req.messages).contains("plan mode is active"));
    assert!(denial_seen, "denial reason reached the model");

    // Still under the posture: the plan file itself auto-approves (no
    // prompt, no denial) — the ONLY write exception, and it is a
    // nano_home-containment check, never a workspace exception.
    let plan_file = harness.plan_file();
    harness.tool_calls.lock().unwrap().clear();
    harness.push_script(vec![
        tool_response(tool_call(
            "c3",
            "fs_write",
            serde_json::json!({"path": plan_file, "content": "the plan"}),
        )),
        text_response("plan written"),
    ]);
    let prompt = harness.send_prompt("write the plan file");
    let frames = harness.finish_prompt(prompt);
    let executed = harness.tool_calls.lock().unwrap().clone();
    assert_eq!(executed.len(), 1, "plan-file write executed: {executed:?}");
    assert_eq!(executed[0].name, "fs_write");
    assert!(
        !frames
            .iter()
            .any(|f| f.get("method").and_then(|m| m.as_str())
                == Some("session/request_permission")),
        "plan-file writes auto-approve under the posture"
    );

    // Exit by privilege-mode set: journals PlanSet(false), re-advertises the
    // underlying mode.
    let exit = harness.set_mode("default");
    assert_eq!(exit["result"]["modes"]["currentModeId"], "default");
    assert!(
        harness
            .journal_ops()
            .iter()
            .any(|op| matches!(op, Op::PlanSet { active: false })),
        "privilege-mode set cleared the posture (PlanSet false)"
    );

    // Entry-path equivalence: the ACP entry journals the same PlanSet(true)
    // through the same transition, reports "plan" as currentModeId while
    // active, and the ack carries the plan file path (Q5 discoverability).
    let enter = harness.set_mode("plan");
    assert_eq!(enter["result"]["modes"]["currentModeId"], "plan");
    assert!(
        enter["result"]["planFile"]
            .as_str()
            .expect("planFile in ack")
            .ends_with(&format!("{}.plan.md", harness.session_id))
    );
    // And with the posture active via the ACP path, the workspace write is
    // again denied at the gate with no prompt — identical enforcement.
    harness.tool_calls.lock().unwrap().clear();
    harness.push_script(vec![
        tool_response(tool_call(
            "c4",
            "fs_edit",
            serde_json::json!({"path": "src/main.rs", "old_string": "a", "new_string": "b"}),
        )),
        text_response("denied again"),
    ]);
    let prompt = harness.send_prompt("now edit the workspace");
    let frames = harness.finish_prompt(prompt);
    assert!(
        harness.tool_calls.lock().unwrap().is_empty(),
        "denied edit must not execute"
    );
    assert!(
        !frames
            .iter()
            .any(|f| f.get("method").and_then(|m| m.as_str())
                == Some("session/request_permission")),
        "categorical denial, no prompt"
    );
}

/// Kill-resume does NOT restore the posture: a session that died with plan
/// active comes back with writes governed by the plain privilege mode
/// (default: the host prompts — the write is no longer categorically
/// denied).
#[test]
fn kill_resume_does_not_restore_the_posture() {
    let sessions_dir = temp_sessions_dir("plan-resume");
    let script = vec![
        tool_response(tool_call("c1", "enter_plan_mode", serde_json::json!({}))),
        text_response("planning"),
    ];
    let session_id = {
        let mut harness = Harness::spawn(script, &sessions_dir, None);
        let prompt = harness.send_prompt("plan");
        harness.finish_prompt(prompt);
        assert!(
            harness
                .journal_ops()
                .iter()
                .any(|op| matches!(op, Op::PlanSet { active: true }))
        );
        harness.session_id.clone()
    };
    // Resume: posture is off (never restored), so a workspace write falls
    // to the default-mode host PROMPT — which we approve.
    let mut harness = Harness::spawn(vec![], &sessions_dir, Some(&session_id));
    harness.push_script(vec![
        tool_response(tool_call(
            "c5",
            "fs_write",
            serde_json::json!({"path": "src/main.rs", "content": "x"}),
        )),
        text_response("done"),
    ]);
    let prompt = harness.send_prompt("write it");
    // The turn blocks on the permission request: answer allow.
    let frames = harness.read_until(|f| {
        f.get("method").and_then(|m| m.as_str()) == Some("session/request_permission")
    });
    let permission = frames.last().expect("permission frame");
    let permission_id = permission["id"].as_u64().expect("id");
    harness.send(serde_json::json!({
        "jsonrpc": "2.0", "id": permission_id,
        "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}
    }));
    harness.finish_prompt(prompt);
    assert!(
        harness
            .tool_calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.name == "fs_write"),
        "post-resume the write prompts (posture NOT restored) and executes on allow"
    );
}

// ── 2. todo ────────────────────────────────────────────────────────────

/// todo write is journaled journal-first (TodoSet lands; the tool result is
/// the full list + counts) and a kill + session/load restores the list into
/// the rebuilt context as the bounded, delimited restore block (Q2).
#[test]
fn todo_journaled_and_restored_into_context_on_resume() {
    let sessions_dir = temp_sessions_dir("todo");
    let todos = serde_json::json!([
        {"id": "t1", "content": "first task", "status": "completed"},
        {"id": "t2", "content": "second task", "status": "in_progress"}
    ]);
    let script = vec![
        tool_response(tool_call("c1", "todo", serde_json::json!({"todos": todos}))),
        text_response("list set"),
    ];
    let session_id = {
        let mut harness = Harness::spawn(script, &sessions_dir, None);
        let prompt = harness.send_prompt("track this");
        harness.finish_prompt(prompt);
        // Journaled as content (TodoSet), never reaching the base executor.
        assert!(
            harness
                .journal_ops()
                .iter()
                .any(|op| matches!(op, Op::TodoSet { items } if items.len() == 2)),
            "TodoSet journaled"
        );
        assert!(harness.tool_calls.lock().unwrap().is_empty());
        // The tool result carries the full list with counts.
        let saw_counts = harness.model_requests.lock().unwrap().iter().any(|req| {
            let text = format!("{:?}", req.messages);
            text.contains("2 item(s)") && text.contains("1 in_progress")
        });
        assert!(saw_counts, "full list + counts returned to the model");
        harness.session_id.clone()
    };
    // Resume: the restored list is re-injected as the bounded, delimited
    // block — the model sees it on the very next request.
    let mut harness = Harness::spawn(vec![], &sessions_dir, Some(&session_id));
    harness.push_script(vec![text_response("resumed")]);
    let prompt = harness.send_prompt("continue");
    harness.finish_prompt(prompt);
    let requests = harness.model_requests.lock().unwrap();
    let first = requests.first().expect("a model request");
    let text = format!("{:?}", first.messages);
    assert!(
        text.contains("[Restored session todo list"),
        "restore block injected: {text}"
    );
    assert!(text.contains("second task"), "list content restored");
    assert!(text.contains("[End of restored todo list]"));
}

/// Under the plan posture, todo is REJECTED with a typed error (codex
/// plan.rs precedent) — planning uses the plan file, not the checklist.
#[test]
fn todo_rejected_under_plan_posture() {
    let sessions_dir = temp_sessions_dir("todo-plan");
    let script = vec![
        tool_response(tool_call("c1", "enter_plan_mode", serde_json::json!({}))),
        tool_response(tool_call(
            "c2",
            "todo",
            serde_json::json!({"todos": [{"id": "t", "content": "x", "status": "pending"}]}),
        )),
        text_response("rejected"),
    ];
    let mut harness = Harness::spawn(script, &sessions_dir, None);
    let prompt = harness.send_prompt("plan with todos");
    harness.finish_prompt(prompt);
    let rejection_seen = harness.model_requests.lock().unwrap().iter().any(|req| {
        format!("{:?}", req.messages).contains("todo is unavailable while plan mode is active")
    });
    assert!(rejection_seen, "typed rejection reached the model");
    assert!(
        !harness
            .journal_ops()
            .iter()
            .any(|op| matches!(op, Op::TodoSet { .. })),
        "a rejected todo write journals nothing"
    );
}

// ── 3. ask_user ────────────────────────────────────────────────────────

/// The question rides session/request_permission with minted opt_{i} ids +
/// a terminal reject-kind Dismiss; a `selected` response resolves to the
/// LABEL as the tool result (the wire carries only the id).
#[test]
fn ask_user_round_trip_resolves_the_label() {
    let sessions_dir = temp_sessions_dir("ask");
    let script = vec![
        tool_response(tool_call(
            "c1",
            "ask_user",
            serde_json::json!({
                "question": "Which approach?",
                "options": [{"label": "Refactor"}, {"label": "Rewrite"}, {"label": "Defer"}]
            }),
        )),
        text_response("user chose"),
    ];
    let mut harness = Harness::spawn(script, &sessions_dir, None);
    let prompt = harness.send_prompt("ask me something");
    let frames = harness.read_until(|f| {
        f.get("method").and_then(|m| m.as_str()) == Some("session/request_permission")
    });
    let question = frames.last().expect("question frame");
    // The question request's toolCallId equals the ask_user tool_call id
    // (Desktop's card and answer channel line up — the #504 failure mode).
    assert_eq!(question["params"]["toolCall"]["toolCallId"], "c1");
    let options = question["params"]["options"].as_array().expect("options");
    assert_eq!(options.len(), 4); // 3 minted + Dismiss
    assert_eq!(options[0]["optionId"], "opt_0");
    assert_eq!(options[1]["optionId"], "opt_1");
    assert_eq!(options[2]["name"], "Defer");
    assert_eq!(options[3]["optionId"], "reject");
    assert_eq!(options[3]["kind"], "reject_once");
    // Answer with the SECOND option; the label becomes the tool result.
    let id = question["id"].as_u64().expect("id");
    harness.send(serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {"outcome": {"outcome": "selected", "optionId": "opt_1"}}
    }));
    harness.finish_prompt(prompt);
    let answer_seen = harness
        .model_requests
        .lock()
        .unwrap()
        .iter()
        .any(|req| format!("{:?}", req.messages).contains("Rewrite"));
    assert!(answer_seen, "the selected label became the tool result");
}

/// Dismiss (the reject-kind terminal option) fails closed to a typed error
/// — never an empty result, never a fabricated answer.
#[test]
fn ask_user_dismiss_is_a_typed_error() {
    let sessions_dir = temp_sessions_dir("ask-dismiss");
    let script = vec![
        tool_response(tool_call(
            "c1",
            "ask_user",
            serde_json::json!({
                "question": "Proceed?",
                "options": [{"label": "Yes"}, {"label": "No"}]
            }),
        )),
        text_response("handled"),
    ];
    let mut harness = Harness::spawn(script, &sessions_dir, None);
    let prompt = harness.send_prompt("ask me");
    let frames = harness.read_until(|f| {
        f.get("method").and_then(|m| m.as_str()) == Some("session/request_permission")
    });
    let id = frames.last().expect("question")["id"].as_u64().expect("id");
    // Desktop's Dismiss mapping: a `rejected` outcome.
    harness.send(serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "result": {"outcome": {"outcome": "rejected"}}
    }));
    harness.finish_prompt(prompt);
    let denial_seen = harness.model_requests.lock().unwrap().iter().any(|req| {
        let text = format!("{:?}", req.messages);
        text.contains("dismissed") && text.contains("proceed without asking")
    });
    assert!(denial_seen, "typed dismissal reached the model");
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
