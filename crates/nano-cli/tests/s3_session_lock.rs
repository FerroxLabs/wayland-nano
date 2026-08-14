//! F-P4-3: single-writer session ownership, proven on the ACP wire.
//!
//! Opening a session for writing (session/new, session/load) takes a
//! lifetime OS lock on the journal; a second host's session/load of the
//! same session is a typed `session_busy` refusal — never the silent
//! double-load the F-P4-3 live probe demonstrated. The harness is the
//! acp_live.rs pattern (scripted client drives `acp_mode::serve`
//! in-process, no FLUX key, no network); the cross-process legs spawn the
//! test binary itself as a lock-holder fixture (the session_browser
//! `lock_holder_fixture` pattern) so a REAL second process — including a
//! kill -9'd one — is the contender.

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

const TIMEOUT: Duration = Duration::from_secs(15);

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

// ── scripted model + inert tools ───────────────────────────────────────

#[derive(Debug)]
enum Step {
    Respond(ModelResponse),
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

static DEFAULT_ROUTING: nano_cli::auto_routing::RoutingConfig =
    nano_cli::auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: None,
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

/// The scripted driver never reaches the network, but provider resolution
/// needs SOME flux credential in the process env (acp_live.rs precedent).
fn ensure_test_flux_key() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if std::env::var("FLUX_API_KEY").is_err() {
            unsafe { std::env::set_var("FLUX_API_KEY", "sk-test-harness-never-networked") };
        }
    });
}

// ── fake client harness ────────────────────────────────────────────────

struct Harness {
    to_host: Option<std::sync::mpsc::Sender<String>>,
    frames: std::sync::mpsc::Receiver<String>,
    release: tokio::sync::watch::Sender<bool>,
    driver_calls: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
    next_id: u64,
    session_id: String,
}

fn temp_sessions_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nano-s3-lock-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp sessions dir");
    dir
}

impl Harness {
    /// A fresh serve instance over `sessions_dir`; `new_session` mirrors the
    /// client flow (fresh conversation: session/new; resuming host: straight
    /// to session/load).
    fn spawn(script: Vec<Step>, sessions_dir: &std::path::Path, new_session: bool) -> Self {
        let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
        let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let driver_calls = Arc::new(AtomicU64::new(0));
        let driver = MockDriver {
            script: Arc::new(Mutex::new(script.into())),
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
                let sandbox_probe = || true;
                let router = nano_cli::provider_router::ProviderRouter::default();
                ensure_test_flux_key();
                let memory_config = acp_mode::MemoryHostConfig {
                    dir: sessions_dir.parent().expect("root").join("memory"),
                    write_enabled: false,
                    block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                };
                let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                    .expect("vendored vision catalog parses");
                let attachment_home = sessions_dir.parent().expect("root");
                let catalog = [AvailableModel {
                    id: "mock".into(),
                    name: "mock".into(),
                }];
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
                    routing: &DEFAULT_ROUTING,
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
            handle: Some(handle),
            next_id: 1,
            session_id: String::new(),
        };
        let init = harness.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], 1, "init: {init}");
        if new_session {
            let response = harness.request(
                "session/new",
                serde_json::json!({ "cwd": ".", "mcpServers": [] }),
            );
            harness.session_id = response["result"]["sessionId"]
                .as_str()
                .unwrap_or_else(|| panic!("session/new failed: {response}"))
                .to_string();
        }
        harness
    }

    fn send(&self, value: serde_json::Value) {
        let tx = self.to_host.as_ref().expect("stdin open");
        tx.send(format!("{}\n", serde_json::to_string(&value).unwrap()))
            .expect("send to host");
    }

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

    fn load_session(&mut self, session_id: &str) -> serde_json::Value {
        self.request(
            "session/load",
            serde_json::json!({
                "sessionId": session_id,
                "cwd": ".",
                "mcpServers": []
            }),
        )
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

    /// Blocks until the mock driver has been called `n` times — the engine
    /// really is parked inside the nth model call (mid-turn).
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

    fn release_model(&self) {
        self.release.send(true).expect("release");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.to_host.take()); // stdin EOF → clean exit
        if let Some(handle) = self.handle.take() {
            let Ok(result) = handle.join() else {
                return; // the test's own assertion reports the panic
            };
            if !std::thread::panicking() {
                assert_eq!(result.expect("serve io"), 0, "clean exit code");
            }
        }
    }
}

/// The typed session_busy refusal, asserted on the exact wire shape.
fn assert_session_busy(response: &serde_json::Value) {
    assert_eq!(
        response["error"]["code"], -32602,
        "contention must be a typed -32602 refusal: {response}"
    );
    assert_eq!(
        response["error"]["data"]["nanoError"]["kind"], "session_busy",
        "the typed kind rides data.nanoError: {response}"
    );
}

// ── tests ──────────────────────────────────────────────────────────────

/// The F-P4-3 repro, inverted: host A holds the session MID-TURN (the model
/// call is parked), host B's session/load is a typed session_busy refusal —
/// before the fix this load succeeded (captures/leg6b-live.json). After
/// host A closes cleanly, host B loads fine.
#[test]
fn second_host_load_during_mid_turn_is_typed_session_busy() {
    let sessions_dir = temp_sessions_dir("midturn");
    let mut host_a = Harness::spawn(
        vec![Step::WaitForRelease(text_response("host A answer"))],
        &sessions_dir,
        true,
    );
    let session_id = host_a.session_id.clone();
    let prompt_id = host_a.send_prompt(&session_id, "hold this turn open");
    host_a.wait_for_driver_calls(1); // host A is genuinely mid-turn now

    // Host B: a second host over the same sessions dir. Read-only listing
    // works under contention and reports the row live...
    let list = nano_cli::session_browser::list_sessions(&sessions_dir).expect("list");
    let row = list
        .sessions
        .iter()
        .find(|s| s.session_id == session_id)
        .expect("owned session listed");
    assert_eq!(
        row.status,
        nano_cli::session_browser::SessionStatus::Live,
        "browser reports the owned session live: {row:?}"
    );
    // ...but host B's WRITE open is the typed refusal, never a double-load.
    let mut host_b = Harness::spawn(Vec::new(), &sessions_dir, false);
    let refused = host_b.load_session(&session_id);
    assert_session_busy(&refused);
    // Host B stays usable: an unknown id still earns session_not_found.
    let missing = host_b.load_session("wayland-nano-session-never-existed");
    assert_eq!(
        missing["error"]["data"]["nanoError"]["kind"],
        "session_not_found"
    );

    // Host A finishes its turn and closes; host B can then take ownership.
    host_a.release_model();
    let frames = host_a.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    drop(host_a);
    let loaded = host_b.load_session(&session_id);
    assert!(
        loaded.get("error").is_none() && loaded.get("result").is_some(),
        "load after the owner closed must succeed: {loaded}"
    );
    drop(host_b);
    let _ = std::fs::remove_dir_all(&sessions_dir);
}

/// Fixture child process: own the journal named by NANO_OWNERSHIP_HOLD_PATH
/// and hold it until killed — the stand-in for a second host process,
/// including one that dies without any cleanup (kill -9).
#[test]
fn ownership_holder_fixture() {
    let Some(path) = std::env::var_os("NANO_OWNERSHIP_HOLD_PATH") else {
        return;
    };
    let ready = std::path::PathBuf::from(
        std::env::var_os("NANO_OWNERSHIP_READY_PATH").expect("ready path"),
    );
    let _ownership = nano_agent::bootstrap::session_guard_registry()
        .try_own(std::path::Path::new(&path))
        .expect("fixture owns the journal");
    std::fs::write(&ready, b"ready").unwrap();
    // Bounded lifetime: if the parent dies first, exit rather than holding
    // a temp-file lock forever.
    for _ in 0..2400 {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The full cross-process matrix over the REAL wire:
/// 1. host A creates a session and exits cleanly (lock released);
/// 2. a second PROCESS takes ownership (the mid-turn holder stand-in) —
///    host B's session/load is typed session_busy;
/// 3. the holder is KILLED (no clean close, no Drop) — the OS releases the
///    handle lock and host B's retry loads the session cleanly: the
///    crash-recovery UX case (a killed host must not brick its session).
#[test]
fn cross_process_holder_blocks_load_and_holder_death_releases_it() {
    let sessions_dir = temp_sessions_dir("xproc");
    let session_id = {
        let host_a = Harness::spawn(Vec::new(), &sessions_dir, true);
        host_a.session_id.clone()
    }; // host A exits cleanly here — the journal is all that survives
    let journal = sessions_dir.join(format!("{session_id}.jsonl"));
    let ready = sessions_dir.join("holder-ready");

    let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ownership_holder_fixture", "--nocapture"])
        .env("NANO_OWNERSHIP_HOLD_PATH", &journal)
        .env("NANO_OWNERSHIP_READY_PATH", &ready)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !ready.exists() {
        assert!(holder.try_wait().unwrap().is_none(), "fixture exited early");
        assert!(
            std::time::Instant::now() < deadline,
            "fixture did not acquire ownership"
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    // Leg 2: host B's write open against the live cross-process holder.
    let mut host_b = Harness::spawn(
        vec![Step::Respond(text_response("host B answer"))],
        &sessions_dir,
        false,
    );
    let refused = host_b.load_session(&session_id);
    assert_session_busy(&refused);

    // Leg 3: kill -9 the holder; host B's retry must load cleanly and the
    // session must remain fully usable (a follow-up turn runs).
    holder.kill().unwrap();
    holder.wait().unwrap();
    let loaded = host_b.load_session(&session_id);
    assert!(
        loaded.get("error").is_none() && loaded.get("result").is_some(),
        "load after the holder was killed must succeed: {loaded}"
    );
    let prompt_id = host_b.send_prompt(&session_id, "are you alive?");
    let frames = host_b.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    drop(host_b);
    let _ = std::fs::remove_dir_all(&sessions_dir);
}

/// Same-host reload of the host's OWN active session (no turn running):
/// the host releases its ownership and reacquires — a `/resume` of the
/// current session must not deadlock or false-positive busy against itself.
#[test]
fn same_host_reload_of_own_session_reacquires() {
    let sessions_dir = temp_sessions_dir("selfreload");
    let mut host = Harness::spawn(
        vec![
            Step::Respond(text_response("first answer")),
            Step::Respond(text_response("after reload")),
        ],
        &sessions_dir,
        true,
    );
    let session_id = host.session_id.clone();
    let prompt_id = host.send_prompt(&session_id, "hello");
    let frames = host.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");

    let reloaded = host.load_session(&session_id);
    assert!(
        reloaded.get("error").is_none() && reloaded.get("result").is_some(),
        "same-host reload of its own session must succeed: {reloaded}"
    );
    let prompt_id = host.send_prompt(&session_id, "still there?");
    let frames = host.read_until(|f| f.get("id").and_then(|v| v.as_u64()) == Some(prompt_id));
    assert_eq!(frames.last().unwrap()["result"]["stopReason"], "end_turn");
    drop(host);
    let _ = std::fs::remove_dir_all(&sessions_dir);
}

/// In-process contention between two live serve instances (two hosts in
/// one process share the guard registry): host B's load of host A's
/// session is typed busy even with no turn running.
#[test]
fn two_live_hosts_in_one_process_still_refuse_double_load() {
    let sessions_dir = temp_sessions_dir("oneproc");
    let host_a = Harness::spawn(Vec::new(), &sessions_dir, true);
    let session_id = host_a.session_id.clone();
    let mut host_b = Harness::spawn(Vec::new(), &sessions_dir, false);
    let refused = host_b.load_session(&session_id);
    assert_session_busy(&refused);
    // And a fresh session/new in host B is unaffected.
    let created = host_b.request(
        "session/new",
        serde_json::json!({ "cwd": ".", "mcpServers": [] }),
    );
    assert!(
        created.get("result").is_some(),
        "host B's own session/new must succeed: {created}"
    );
    drop(host_b);
    drop(host_a);
    let _ = std::fs::remove_dir_all(&sessions_dir);
}
