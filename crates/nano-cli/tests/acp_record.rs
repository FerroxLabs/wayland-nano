//! ACP transcript recorder (design doc §6, corpus binding C2 — pick (b)
//! primary): records REAL `acp_mode::serve` host-loop transcripts into
//! `crates/nano-tui/tests/fixtures/` for the nano-tui L2/L3 fake host.
//!
//! The wire-shaping code here is the production host loop (serve), the
//! production frame builders (agent_message_chunk, tool_call_*,
//! request_permission_request, replay_frames) and the production session
//! journal; only the model is scripted (MockDriver) so recording runs
//! offline with no Flux key. Transcripts therefore cannot drift from the
//! real wire by belief — and this test is the drift lint: without
//! NANO_RECORD_ACP=1 it re-records in memory and FAILS on any byte
//! difference against the checked-in fixture. Re-record deliberately with:
//!
//!   NANO_RECORD_ACP=1 cargo test -p nano-cli --test acp_record
//!
//! Determinism: the recorded session id is normalized to
//! "wayland-nano-session-recorded" (the real id embeds nanoseconds);
//! everything else (request ids, permission ids, tool output, digests) is
//! deterministic across runs by construction. A new nondeterminism source
//! breaks this test loudly — that is the point.

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
/// The normalized session id every recorded fixture carries.
const RECORDED_SESSION_ID: &str = "wayland-nano-session-recorded";
/// Fixture root (relative to the nano-cli crate).
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../nano-tui/tests/fixtures");

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

// ── scripted model + tools ─────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MockDriver {
    script: Arc<Mutex<VecDeque<ModelResponse>>>,
    calls: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl ModelDriver for MockDriver {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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

fn text_response(chunks: &[&str]) -> ModelResponse {
    let mut events: Vec<ModelEvent> = chunks
        .iter()
        .map(|c| ModelEvent::TextDelta((*c).to_string()))
        .collect();
    events.push(ModelEvent::Done {
        stop_reason: "stop".into(),
    });
    ModelResponse {
        events,
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

// ── the recorder ───────────────────────────────────────────────────────

struct Recorder {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    /// The recorded transcript: (direction, frame) in wire order.
    recorded: Vec<(char, serde_json::Value)>,
}

fn catalog() -> Vec<AvailableModel> {
    vec![
        AvailableModel {
            id: "flux-auto".into(),
            name: "Flux Auto".into(),
        },
        AvailableModel {
            id: "flux-fast".into(),
            name: "Flux Fast".into(),
        },
    ]
}

/// The standard agent fs posture (C2): the gate's advisory containment
/// oracle and the executor must share this exact policy shape.
fn workspace_policy() -> nano_core::permissions::FileSystemSandboxPolicy {
    nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy()
}

impl Recorder {
    fn spawn(script: Vec<ModelResponse>, sessions_dir: &std::path::Path) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
            calls: Arc::new(AtomicU64::new(0)),
        };
        let sessions_dir = sessions_dir.to_path_buf();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(async move {
                // C2: the gate's sandbox probe is injectable; recordings pin
                // the default-mode wire, where the probe is never consulted.
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
                let config = acp_mode::ServeConfig {
                    sessions_dir: &sessions_dir,
                    default_model: "flux-auto",
                    available_models: &catalog(),
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
                    // C2: make_tools returns the executor AND its exact fs
                    // policy (the gate's advisory oracle shares provenance).
                    move |_, _, _, _| (MockTools, workspace_policy()),
                )
                .await
            })
        });
        Self {
            to_host: Some(in_tx),
            frames: out_rx,
            handle: Some(handle),
            next_id: 1,
            recorded: Vec::new(),
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn send(&mut self, frame: serde_json::Value) {
        self.recorded.push(('>', frame.clone()));
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
        self.recorded.push(('<', frame.clone()));
        frame
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.alloc();
        self.send(serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }));
        loop {
            let frame = self.next_frame();
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return frame;
            }
        }
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

/// Normalize nondeterministic values (the generated session id; the temp
/// sessions dir — C10: it surfaces in the set_mode plan ack's planFile).
fn normalize(
    recorded: &[(char, serde_json::Value)],
    session_id: &str,
    sessions_dir: &std::path::Path,
) -> String {
    // In the serialized line the dir's backslashes are JSON-escaped.
    let dir = sessions_dir.display().to_string();
    let dir_escaped = dir.replace('\\', "\\\\");
    let mut out = String::new();
    for (dir, frame) in recorded {
        let line = serde_json::to_string(&serde_json::json!({"dir": dir, "frame": frame}))
            .expect("serialize");
        out.push_str(
            &line
                .replace(session_id, RECORDED_SESSION_ID)
                .replace(&dir_escaped, "<sessions>")
                // The placeholder eats the dir but not the separator that
                // followed it: a Windows recording leaves `<sessions>\\…`
                // where unix leaves `<sessions>/…`. Normalize so the checked
                // -in fixture replays byte-identically on every CI leg.
                .replace("<sessions>\\\\", "<sessions>/"),
        );
        out.push('\n');
    }
    out
}

fn write_or_verify(name: &str, content: &str) {
    let path = std::path::Path::new(FIXTURE_DIR).join(name);
    if std::env::var_os("NANO_RECORD_ACP").is_some() {
        std::fs::write(&path, content).expect("write fixture");
        eprintln!("recorded {}", path.display());
        return;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "fixture {} missing ({e}) — record it with NANO_RECORD_ACP=1",
            path.display()
        )
    });
    if existing != content {
        // Locate the first differing line for a useful failure message.
        let old: Vec<&str> = existing.lines().collect();
        let new: Vec<&str> = content.lines().collect();
        let diverged = (0..old.len().max(new.len())).find(|&i| old.get(i) != new.get(i));
        panic!(
            "fixture {} drifted from the real host loop (first divergence at line {:?})\n\
             checked-in: {:?}\n\
             re-recorded: {:?}\n\
             If the wire change is intentional, re-record with NANO_RECORD_ACP=1.",
            path.display(),
            diverged.map(|i| i + 1),
            diverged.and_then(|i| old.get(i)),
            diverged.and_then(|i| new.get(i)),
        );
    }
}

#[test]
fn record_full_journey_fixture() {
    let sessions_dir = std::env::temp_dir().join(format!("nano-acp-record-{}", std::process::id()));
    std::fs::create_dir_all(&sessions_dir).expect("sessions dir");

    // Phase 1: lifecycle + streamed text turn + approval turn + set_model
    // + cancel, against one serve instance.
    let mut rec = Recorder::spawn(
        vec![
            text_response(&["Hello from ", "Wayland Nano", "."]),
            tool_response(ToolCall {
                id: "call-1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "note.txt", "content": "hello\n"}),
            }),
            text_response(&["File written."]),
        ],
        &sessions_dir,
    );

    rec.request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
            "clientInfo": { "name": "nano-tui", "version": "0.1.0" }
        }),
    );
    let session_new = rec.request(
        "session/new",
        serde_json::json!({"cwd": "/recorded-workspace", "mcpServers": []}),
    );
    let session_id = session_new["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // (a) prompt → streamed agent_message_chunk frames.
    let prompt_id = rec.alloc();
    rec.send(serde_json::json!({
        "jsonrpc": "2.0", "id": prompt_id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": "Say hello."}]}
    }));
    loop {
        let frame = rec.next_frame();
        if frame.get("id").and_then(|v| v.as_u64()) == Some(prompt_id) {
            assert_eq!(frame["result"]["stopReason"], "end_turn");
            break;
        }
    }

    // (b) prompt → tool_call → session/request_permission → allow → result.
    let prompt_id = rec.alloc();
    rec.send(serde_json::json!({
        "jsonrpc": "2.0", "id": prompt_id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": "Write note.txt."}]}
    }));
    let permission = loop {
        let frame = rec.next_frame();
        if frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission") {
            break frame;
        }
    };
    let permission_id = permission["id"].as_u64().expect("permission id");
    rec.send(serde_json::json!({
        "jsonrpc": "2.0", "id": permission_id,
        "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}
    }));
    loop {
        let frame = rec.next_frame();
        if frame.get("id").and_then(|v| v.as_u64()) == Some(prompt_id) {
            assert_eq!(frame["result"]["stopReason"], "end_turn");
            break;
        }
    }

    // (c) session/set_model.
    let set_model = rec.request(
        "session/set_model",
        serde_json::json!({"sessionId": session_id, "modelId": "flux-fast"}),
    );
    assert_eq!(set_model["result"]["models"]["currentModelId"], "flux-fast");

    // (d) session/set_mode (C2): default → full_auto → default. Each
    // accepted change journals ModeSet first; the ack carries the modes
    // block with the re-advertised currentModeId (C10 §3 Q1 — the
    // posture-projection reporting rule).
    let set_mode = rec.request(
        "session/set_mode",
        serde_json::json!({"sessionId": session_id, "modeId": "full_auto"}),
    );
    assert_eq!(set_mode["result"]["modes"]["currentModeId"], "full_auto");
    let set_mode = rec.request(
        "session/set_mode",
        serde_json::json!({"sessionId": session_id, "modeId": "default"}),
    );
    assert_eq!(set_mode["result"]["modes"]["currentModeId"], "default");

    // (d2) C10 §3: the plan-posture projection. set_mode "plan" flips the
    // posture on (journaled PlanSet), leaves the privilege mode untouched,
    // and acks currentModeId "plan" + the plan file path under nano_home.
    let set_plan = rec.request(
        "session/set_mode",
        serde_json::json!({"sessionId": session_id, "modeId": "plan"}),
    );
    assert_eq!(set_plan["result"]["modes"]["currentModeId"], "plan");
    let plan_file = set_plan["result"]["planFile"]
        .as_str()
        .expect("plan entry acks the plan file path");
    assert!(plan_file.ends_with(&format!("{session_id}.plan.md")));
    // Setting a privilege mode clears the posture and re-advertises the
    // mode — a client never reads plan→default as a privilege change.
    let set_default = rec.request(
        "session/set_mode",
        serde_json::json!({"sessionId": session_id, "modeId": "default"}),
    );
    assert_eq!(set_default["result"]["modes"]["currentModeId"], "default");

    // session/cancel shape pinned on the recording (between turns).
    rec.send(serde_json::json!({
        "jsonrpc": "2.0", "method": "session/cancel",
        "params": {"sessionId": session_id}
    }));

    let phase1_len = rec.recorded.len();
    let full_journey = normalize(&rec.recorded, &session_id, &sessions_dir);
    rec.shutdown();

    // The L3 smoke needs just the opening (initialize, session/new, first
    // streamed prompt) — a strict prefix of the same recording.
    let pty_journey: String = full_journey
        .lines()
        .take({
            // lines up to and including the first prompt's response
            let mut count = 0;
            let mut seen_prompt_response = false;
            for (i, line) in full_journey.lines().enumerate() {
                count = i + 1;
                if line.contains("\"stopReason\"") {
                    seen_prompt_response = true;
                    break;
                }
            }
            assert!(
                seen_prompt_response,
                "recording must contain a prompt response"
            );
            count
        })
        .map(|l| format!("{l}\n"))
        .collect();

    // Phase 2: (d) kill + relaunch — a FRESH serve instance over the same
    // sessions dir; session/load replays the journaled transcript.
    let mut rec2 = Recorder::spawn(Vec::new(), &sessions_dir);
    rec2.request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
            "clientInfo": { "name": "nano-tui", "version": "0.1.0" }
        }),
    );
    let load_id = rec2.alloc();
    rec2.send(serde_json::json!({
        "jsonrpc": "2.0", "id": load_id, "method": "session/load",
        "params": {"sessionId": session_id, "cwd": "/recorded-workspace", "mcpServers": []}
    }));
    loop {
        let frame = rec2.next_frame();
        if frame.get("id").and_then(|v| v.as_u64()) == Some(load_id) {
            break;
        }
    }
    let phase2 = normalize(&rec2.recorded, &session_id, &sessions_dir);
    rec2.shutdown();

    let full = format!("{full_journey}{phase2}");
    assert!(phase1_len > 0);
    write_or_verify("full_journey.ndjson", &full);
    write_or_verify("pty_journey.ndjson", &pty_journey);
    let _ = std::fs::remove_dir_all(&sessions_dir);
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
