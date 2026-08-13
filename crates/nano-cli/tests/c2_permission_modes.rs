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
                    move |_, _, _, _, _| (MockTools, workspace_policy()),
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

/// F-C2-1 driver: start a two-write turn, and when the FIRST write's
/// permission prompt parks the turn, send `set_mode` mid-park (WITHOUT
/// answering), give the reader thread a beat to relay, then answer allow.
/// Returns once the prompt AND the queued set_mode have both been answered.
/// `permission_frames` afterwards tells whether the SECOND write prompted.
fn park_then_flip(
    host: &mut Harness,
    session_id: &str,
    mode: &str,
) -> (serde_json::Value, serde_json::Value) {
    host.next_id += 1;
    let prompt_id = host.next_id;
    host.send(serde_json::json!({
        "jsonrpc": "2.0", "id": prompt_id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": "write two files"}]},
    }));
    host.next_id += 1;
    let mode_id = host.next_id;
    let mut prompt_response = None;
    let mut mode_response = None;
    let mut flipped = false;
    while prompt_response.is_none() || mode_response.is_none() {
        let frame = host.next_frame();
        if frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission") {
            let permission_id = frame["id"].as_u64().expect("permission id");
            if !flipped {
                flipped = true;
                // The turn is PARKED inside the gate right now: flip the mode
                // mid-park, wait for the reader-thread relay (the journal
                // cannot show it — ModeSet is journaled by the main loop
                // after the park — so the wait is time-based), then answer.
                host.send(serde_json::json!({
                    "jsonrpc": "2.0", "id": mode_id, "method": "session/set_mode",
                    "params": {"sessionId": session_id, "modeId": mode},
                }));
                std::thread::sleep(Duration::from_millis(300));
            } else {
                // A SECOND prompt means the flip did NOT reach the running
                // turn's gate (the de-escalation leg asserts this arm never
                // fires; the escalation leg asserts it does). Answer it so
                // the turn can finish.
            }
            host.send(serde_json::json!({
                "jsonrpc": "2.0", "id": permission_id,
                "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}
            }));
            continue;
        }
        match frame.get("id").and_then(|v| v.as_u64()) {
            Some(id) if id == prompt_id => prompt_response = Some(frame),
            Some(id) if id == mode_id => mode_response = Some(frame),
            _ => {}
        }
    }
    // The queued set_mode is journaled by the main loop exactly once — and
    // by now it has been (its response arrived).
    (
        prompt_response.expect("prompt ack"),
        mode_response.expect("mode ack"),
    )
}

/// F-C2-1 (de-escalation relayed mid-park): flipping default→read_only
/// while the turn is PARKED at the first write's permission prompt tightens
/// the running turn — the SECOND write is denied at the gate with NO
/// permission frame and a mode-naming denial in the model's context. The
/// ModeSet is journaled (by the main loop, once the park ends) exactly once.
#[test]
fn mid_park_de_escalation_tightens_the_running_turn() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "a.txt", "content": "x"}),
            }),
            tool_response(ToolCall {
                id: "c2".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "b.txt", "content": "y"}),
            }),
            text_response("done"),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());

    let (prompt_response, mode_response) = park_then_flip(&mut host, &session_id, "read_only");
    assert_eq!(prompt_response["result"]["stopReason"], "end_turn");
    assert_eq!(
        mode_response["result"]["modes"]["currentModeId"], "read_only",
        "the ack carries the modes block (C10 shape)"
    );
    assert_eq!(
        host.permission_frames(),
        1,
        "the relayed de-escalation denied the second write WITHOUT a prompt"
    );
    let carried = {
        let requests = host.model_requests.lock().unwrap();
        requests.iter().any(|request| {
            request.messages.iter().any(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        nano_model::types::ContentBlock::ToolResult { content, .. }
                        if content.contains("denied by approval gate: session is in read_only mode")
                    )
                })
            })
        })
    };
    assert!(carried, "the second write's denial must name the mode");
    assert_eq!(
        journal_modes(&dirs, &session_id),
        ["read_only"],
        "the main loop journaled the accepted set_mode exactly once"
    );
    host.shutdown();
}

/// F-C2-1 (escalation still deferred): flipping default→full_auto mid-park
/// relays NOTHING — the second write STILL prompts (the running turn keeps
/// its captured ceiling), and the escalation takes effect only via the main
/// loop's journal-first path for the NEXT turn.
#[test]
fn mid_park_escalation_is_still_deferred() {
    let dirs = TestDirs::new();
    let mut host = Harness::spawn(
        vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "a.txt", "content": "x"}),
            }),
            tool_response(ToolCall {
                id: "c2".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "b.txt", "content": "y"}),
            }),
            text_response("done"),
            // The post-escalation turn: a third contained write, then text.
            tool_response(ToolCall {
                id: "c3".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "c.txt", "content": "z"}),
            }),
            text_response("done again"),
        ],
        &dirs.sessions(),
        false,
    );
    let session_id = host.new_session(&dirs.workspace());

    let (prompt_response, mode_response) = park_then_flip(&mut host, &session_id, "full_auto");
    assert_eq!(prompt_response["result"]["stopReason"], "end_turn");
    assert_eq!(
        mode_response["result"]["modes"]["currentModeId"], "full_auto",
        "the ack carries the modes block (C10 shape)"
    );
    assert_eq!(
        host.permission_frames(),
        2,
        "escalation must NOT affect the running turn: both writes prompted"
    );
    assert_eq!(journal_modes(&dirs, &session_id), ["full_auto"]);

    // The escalation applies from the NEXT turn: a third write under the now
    // journaled full_auto mode auto-approves without a prompt.
    let before = host.permission_frames();
    let response = host.prompt(&session_id, "write c.txt");
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        host.permission_frames() - before,
        0,
        "the NEXT turn captures full_auto"
    );
    host.shutdown();
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
    // C10 §3 (Q1 RULED): "plan" is the fourth advertised id — a projection
    // of the orthogonal posture, NOT a privilege mode.
    assert_eq!(ids, ["read_only", "default", "full_auto", "plan"]);
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
    // C10: the ack carries the modes block with the re-advertised id.
    assert_eq!(ack["result"]["modes"]["currentModeId"], "full_auto");
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
        host.set_mode(&session_id, "read_only")["result"]["modes"]["currentModeId"],
        "read_only"
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
        host.set_mode(&session_id, "full_auto")["result"]["modes"]["currentModeId"],
        "full_auto"
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
