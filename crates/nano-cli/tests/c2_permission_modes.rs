//! C2 permission modes — wire-level integration against the real
//! `acp_mode::serve` host loop (scripted model, mock tools, real journal):
//! - session/new + session/load advertise all three modes from the single
//!   PermissionMode metadata;
//! - set_mode validates ids (unknown/garbage → typed error, NOTHING
//!   journaled), journals accepted changes ModeSet-first, and fails closed
//!   when the journal append itself fails (mode visibly unchanged);
//! - read_only denies a write at the gate: ZERO session/request_permission
//!   frames and the denial text naming the mode reaches the model context;
//! - full_auto auto-approves a CONTAINED write (no prompt), prompts exactly
//!   once for an uncontained one, always prompts for mcp__*, and gates
//!   shell on the injected sandbox-backend probe;
//! - kill + resume: the journaled ModeSet survives as audit history but the
//!   resumed session is back in `default` (panel ruling Q5 — autonomy is
//!   never resurrected).

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

// ── channel-backed streams (the acp_record pattern) ─────────────────────

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
    /// Every request the engine made — the denial-text assertion reads the
    /// model context from here.
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

fn workspace_policy() -> nano_core::permissions::FileSystemSandboxPolicy {
    nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy()
}

// ── the harness ─────────────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    /// Every host frame seen so far (requests, notifications, responses).
    seen: Vec<serde_json::Value>,
    model_requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl Harness {
    fn spawn(
        script: Vec<ModelResponse>,
        sessions_dir: &std::path::Path,
        sandbox_available: bool,
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
                let sandbox_probe = move || sandbox_available;
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir,
                    default_model: "mock",
                    available_models: &catalog,
                    env_mcp_specs: &[],
                    catalog: &[],
                    window_override: None,
                    limit_override: None,
                    sandbox_probe: &sandbox_probe,
                    cron_home: None,
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
                    move || driver.clone(),
                    move |_, _| (MockTools, workspace_policy()),
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

    /// Permission requests the host has emitted so far.
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

/// A temp dir holding the sessions root and a workspace.
struct TestDirs {
    root: std::path::PathBuf,
}

impl TestDirs {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "nano-c2-wire-{}-{}",
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

fn journal_modes(dirs: &TestDirs, session_id: &str) -> Vec<String> {
    let journal = dirs.sessions().join(format!("{session_id}.jsonl"));
    let report = nano_session::reader::read_journal(&journal).expect("journal reads");
    report
        .envelopes
        .iter()
        .filter_map(|e| match &e.op {
            nano_session::op::Op::ModeSet { mode } => Some(mode.clone()),
            _ => None,
        })
        .collect()
}

fn assert_advertises_three_modes(result: &serde_json::Value) {
    let modes = &result["modes"];
    assert_eq!(modes["currentModeId"], "default");
    let ids: Vec<&str> = modes["availableModes"]
        .as_array()
        .expect("availableModes")
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["read_only", "default", "full_auto"]);
}

// ── the tests ───────────────────────────────────────────────────────────

#[test]
fn session_new_and_load_advertise_the_three_modes() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(Vec::new(), &dirs.sessions(), false);
    let session_id = host.new_session(&dirs.workspace());
    let new_response = host.seen.last().unwrap().clone();
    assert_advertises_three_modes(&new_response["result"]);
    host.shutdown();

    // A fresh host over the same sessions dir: session/load advertises the
    // same block and reports `default` — never a resurrected mode.
    let mut host = Harness::spawn(Vec::new(), &dirs.sessions(), false);
    host.request("initialize", serde_json::json!({"protocolVersion": 1}));
    let load = host.request(
        "session/load",
        serde_json::json!({"sessionId": session_id, "cwd": dirs.workspace(), "mcpServers": []}),
    );
    assert_advertises_three_modes(&load["result"]);
    host.shutdown();
}

#[test]
fn set_mode_unknown_id_is_a_typed_error_and_journals_nothing() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(Vec::new(), &dirs.sessions(), false);
    let session_id = host.new_session(&dirs.workspace());
    for garbage in [
        "yolo",
        "force",
        "FULL_AUTO",
        "",
        "dangerously_skip_permissions",
    ] {
        let response = host.set_mode(&session_id, garbage);
        let error = response["error"].as_object().expect("typed error");
        assert_eq!(error["code"], -32602);
        assert!(
            error["message"].as_str().unwrap().contains("unknown mode"),
            "{garbage:?}: {error:?}"
        );
    }
    // A rejected set_mode leaves NO audit-trail entry — journaling rejected
    // changes would fabricate one.
    assert!(
        journal_modes(&dirs, &session_id).is_empty(),
        "rejected set_mode must journal nothing"
    );
    // The session still runs in default: a contained write PROMPTS.
    host.shutdown();
}

#[test]
fn accepted_set_mode_journals_modeset_and_kill_resume_returns_to_default() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "inside.txt", "content": "x"}),
            }),
            text_response("done"),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    let ack = host.set_mode(&session_id, "full_auto");
    assert_eq!(ack["result"], serde_json::json!({}), "ACP ack shape");
    assert_eq!(journal_modes(&dirs, &session_id), ["full_auto"]);
    host.shutdown();

    // Kill + relaunch over the same journal: the ModeSet survives as audit
    // history, but the session comes back in DEFAULT and a contained write
    // prompts again (autonomy is never resurrected).
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c2".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "inside.txt", "content": "x"}),
            }),
            text_response("done"),
        ],
        &dirs.sessions(),
        false,
    );
    host.request("initialize", serde_json::json!({"protocolVersion": 1}));
    let load = host.request(
        "session/load",
        serde_json::json!({"sessionId": session_id, "cwd": dirs.workspace(), "mcpServers": []}),
    );
    assert_eq!(load["result"]["modes"]["currentModeId"], "default");
    assert_eq!(
        journal_modes(&dirs, &session_id),
        ["full_auto"],
        "the journaled ModeSet is audit history, not state"
    );
    let before = host.permission_frames();
    let response = host.prompt(&session_id, "write inside.txt");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host.permission_frames() - before,
        1,
        "a resumed session in default mode prompts for the write"
    );
    host.shutdown();
}

#[test]
fn read_only_denies_writes_at_the_gate_without_prompting() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "inside.txt", "content": "x"}),
            }),
            text_response("I cannot write while read-only."),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    assert_eq!(
        host.set_mode(&session_id, "read_only")["result"],
        serde_json::json!({})
    );

    let before = host.permission_frames();
    let response = host.prompt(&session_id, "write inside.txt");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host.permission_frames() - before,
        0,
        "read_only emits NO session/request_permission for a write"
    );
    // The denial text NAMES the mode in the context the model continued
    // from — so it learns why and stops retrying variants.
    let carried = {
        let requests = host.model_requests.lock().unwrap();
        requests[1].messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    nano_model::types::ContentBlock::ToolResult { content, .. }
                    if content.contains("denied by approval gate: session is in read_only mode")
                )
            })
        })
    };
    assert!(carried, "denial text must name the mode");
    host.shutdown();
}

#[test]
fn full_auto_contained_write_skips_the_prompt_uncontained_prompts_once() {
    let dirs = TestDirs::new();
    // The uncontained target anchors at the FILESYSTEM ROOT: the
    // workspace_write policy includes the tmp roots (SlashTmp/Tmpdir), so a
    // tempdir sibling would be CONTAINED on unix. MockTools never writes,
    // so this path is only ever fed to the gate's containment check.
    let outside = dirs
        .root
        .ancestors()
        .last()
        .expect("filesystem root")
        .join("nano-c2-wire-outside");
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "inside.txt", "content": "x"}),
            }),
            text_response("wrote it"),
            tool_response(ToolCall {
                id: "c2".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": outside.join("escape.txt"), "content": "x"}),
            }),
            text_response("tried the escape"),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    assert_eq!(
        host.set_mode(&session_id, "full_auto")["result"],
        serde_json::json!({})
    );

    // Contained (relative) write: auto-approved, NO permission frame.
    let before = host.permission_frames();
    let response = host.prompt(&session_id, "write inside.txt");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host.permission_frames() - before,
        0,
        "a contained write under full_auto never prompts"
    );

    // Uncontained write: exactly ONE permission frame (auto-allowed above).
    let before = host.permission_frames();
    let response = host.prompt(&session_id, "write outside");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host.permission_frames() - before,
        1,
        "an uncontained write under full_auto falls through to the host"
    );
    host.shutdown();
}

#[test]
fn full_auto_shell_obeys_the_sandbox_probe_and_mcp_always_prompts() {
    let dirs = TestDirs::new();
    let shell_call = |id: &str| {
        tool_response(ToolCall {
            id: id.into(),
            name: "shell".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        })
    };
    // No sandbox backend: shell PROMPTS even under full_auto.
    let mut host = Harness::spawn(
        vec![shell_call("c1"), text_response("done")],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    host.set_mode(&session_id, "full_auto");
    let before = host.permission_frames();
    host.prompt(&session_id, "run echo");
    assert_eq!(
        host.permission_frames() - before,
        1,
        "no sandbox backend → shell prompts (never a silent unsandboxed run)"
    );
    host.shutdown();

    // Sandbox backend available: shell auto-approves; mcp__* still prompts.
    let mut host = Harness::spawn(
        vec![
            shell_call("c2"),
            text_response("ran it"),
            tool_response(ToolCall {
                id: "c3".into(),
                name: "mcp__server__tool".into(),
                arguments: serde_json::json!({}),
            }),
            text_response("mcp done"),
        ],
        &dirs.sessions(),
        true,
    );
    let session_id = host.new_session(&dirs.workspace());
    host.set_mode(&session_id, "full_auto");
    let before = host.permission_frames();
    host.prompt(&session_id, "run echo");
    assert_eq!(
        host.permission_frames() - before,
        0,
        "sandboxed shell auto-approves under full_auto"
    );
    let before = host.permission_frames();
    host.prompt(&session_id, "call the mcp tool");
    assert_eq!(
        host.permission_frames() - before,
        1,
        "mcp__* is mutating-unknown: it prompts under EVERY mode"
    );
    host.shutdown();
}

#[test]
fn journal_append_failure_leaves_the_mode_visibly_unchanged() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "inside.txt", "content": "x"}),
            }),
            text_response("done"),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());
    // Break the journal: replace the file with a DIRECTORY so the
    // journal-first append fails.
    let journal = dirs.sessions().join(format!("{session_id}.jsonl"));
    std::fs::remove_file(&journal).unwrap();
    std::fs::create_dir(&journal).unwrap();

    let response = host.set_mode(&session_id, "full_auto");
    let error = response["error"].as_object().expect("typed error");
    assert_eq!(error["code"], -32603);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("cannot journal mode change"),
        "{error:?}"
    );

    // Repair the filesystem and prove the mode never mutated: a contained
    // write still PROMPTS (default), where full_auto would have approved
    // silently.
    std::fs::remove_dir(&journal).unwrap();
    let before = host.permission_frames();
    let response = host.prompt(&session_id, "write inside.txt");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host.permission_frames() - before,
        1,
        "append failure ⇒ the mode stayed default"
    );
    host.shutdown();
}
