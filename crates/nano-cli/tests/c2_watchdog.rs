//! C2.3 Desktop-style watchdog harness — the live long-turn frame-cadence
//! oracle.
//!
//! Scorecard (`shared/SCORECARD.md` C2.3): "Event stream: turn-scoped frame
//! every <600s outside tool windows", oracle "Desktop-style watchdog
//! harness". This test mirrors Desktop's ACTUAL timeout semantics rather
//! than the looser scorecard bound:
//!
//! - Desktop's per-`session/prompt` watchdog is `acp.promptTimeout`, default
//!   **300 s** (`desktop/src/process/agent/acp/AcpConnection.ts`:
//!   `promptTimeoutMs = 300000`; `setPromptTimeout` clamps to >= 30 s; the
//!   settings UI clamps 30..3600 s). The budget applies to the
//!   `session/prompt` request (other requests get 60 s) and is PAUSED while
//!   a client-side permission request is open
//!   (`pauseRequestTimeout`/`resumeRequestTimeout`) — the scorecard's
//!   "outside tool windows" carve-out. On expiry Desktop sends
//!   `session/cancel` and rejects the turn (fail-closed).
//! - This harness therefore asserts every ACTIVE inter-frame gap (frame
//!   arrival to frame arrival, minus any open permission window) stays under
//!   Desktop's 300 s default — strictly stronger than the scorecard's 600 s
//!   — and that the total active prompt time (prompt send → response, minus
//!   permission windows) stays under the same 300 s budget Desktop enforces.
//!
//! LIVE-GATED: self-skips without `FLUX_TEST_KEY` (mirrors `acp_slice.rs`).
//!
//! Flow: spawn the REAL `nanok3 acp-host`, initialize → session/new against
//! a fresh temp workspace seeded with three small source files, then run a
//! genuinely LONG turn (three fs_read round trips plus eight fs_write
//! round trips, one per essay part — tens of seconds of model time across
//! many model round trips). Every `session/update` frame's arrival is
//! timestamped on the reader thread; permission requests are answered
//! "allow once" inline and their open windows are subtracted from the gap
//! accounting, exactly like Desktop's pause/resume. Asserts: turn completes
//! `end_turn`, turn wall is at least 30 s (the scenario is really a long
//! turn), every active inter-frame gap is under 300 s, total active prompt
//! time is under the same 300 s budget Desktop enforces, and the stream is
//! well-ordered (monotonic arrivals, `tool_call` before its
//! `tool_call_update`, the prompt response is the final frame).
//!
//! Run manifest: JSON written to `<workspace>/c2-watchdog-manifest.json`
//! (workspace = `$TEMP/nanok3-c2-watchdog-<pid>/`, kept after the run; path
//! printed to stderr). It records per-frame arrival offsets and active gaps,
//! the max gap, and the bound. Measured numbers are pasted in
//! `shared/reviews/C2/trackb-claim.md` §C2.3.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Desktop's `acp.promptTimeout` default (300 s), the per-`session/prompt`
/// watchdog budget — stricter than the scorecard's <600 s bound.
const DESKTOP_PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

/// Desktop's timeout for non-prompt requests (`AcpConnection.ts`: 60 s).
const DESKTOP_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Scenario-validity floor: the turn must genuinely be a long turn.
const MIN_TURN_WALL: Duration = Duration::from_secs(30);

const PROMPT: &str = "Multi-part writing task over three short source files. \
Step 1: use fs_read on alpha.txt, then beta.txt, then gamma.txt (one call each). \
Step 2: write an 8-part essay about how the three files' themes relate to the craft of reliable \
software: use fs_write EIGHT separate times to create part1.txt, part2.txt, part3.txt, part4.txt, \
part5.txt, part6.txt, part7.txt, and part8.txt. Each partN.txt must contain a heading line \
\"Part N\" followed by one section of AT LEAST 150 words. Make exactly one fs_write call per \
part; never combine two parts into one call. \
Step 3, only after all eight parts are written: reply with DONE.";

fn key() -> Option<String> {
    std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// A `session/update` frame arrival, timestamped on the reader thread.
struct Stamp {
    at: Instant,
    kind: String,
    tool_call_id: Option<String>,
}

/// ACP client handle over a spawned `nanok3 acp-host`. Stdout is pumped on a
/// reader thread that timestamps each line the instant it arrives, so gap
/// measurement is not polluted by consumer-side work.
struct Acp {
    child: Child,
    pid: u32,
    stdin: ChildStdin,
    lines: Receiver<(Instant, String)>,
    /// Every frame received, in order (evidence + canary substrate).
    frames: Vec<serde_json::Value>,
    /// Open windows of client-side permission handling (arrival..answered),
    /// subtracted from the active-gap accounting like Desktop's
    /// pause/resume of the prompt budget.
    permission_windows: Vec<(Instant, Instant)>,
    next_id: u64,
}

impl Acp {
    fn spawn(workspace: &Path, nano_home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nanok3"))
            .arg("acp-host")
            .current_dir(workspace)
            .env(
                "FLUX_API_KEY",
                std::env::var("FLUX_TEST_KEY").unwrap_or_default(),
            )
            .env("NANOK3_HOME", nano_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nanok3 acp-host");
        let pid = child.id();
        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match stdout.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        // Timestamp at arrival on the wire, not at consumption.
                        let at = Instant::now();
                        if tx.send((at, line.trim_end().to_string())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            child,
            pid,
            stdin,
            lines: rx,
            frames: Vec::new(),
            permission_windows: Vec::new(),
            next_id: 1,
        }
    }

    fn send(&mut self, method: &str, params: serde_json::Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).expect("write");
        self.stdin.flush().expect("flush");
        id
    }

    /// Next non-permission frame within `deadline`. A deadline trip IS the
    /// watchdog firing: Desktop would cancel the prompt at that point, so
    /// the test fails. Permission requests are answered "allow once" inline
    /// and their open window is recorded for the active-gap accounting.
    fn next_frame(&mut self, deadline: Duration) -> Option<(Instant, serde_json::Value)> {
        loop {
            let (at, line) = match self.lines.recv_timeout(deadline) {
                Ok(pair) => pair,
                Err(RecvTimeoutError::Timeout) => panic!(
                    "WATCHDOG FIRED: no acp frame for {} s (Desktop acp.promptTimeout budget)",
                    deadline.as_secs()
                ),
                Err(RecvTimeoutError::Disconnected) => return None,
            };
            let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
            let is_permission =
                frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission");
            self.frames.push(frame.clone());
            if is_permission {
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": frame["id"].clone(),
                    "result": { "outcome": { "outcome": "selected", "optionId": "allow" } }
                });
                writeln!(self.stdin, "{}", serde_json::to_string(&reply).unwrap()).expect("write");
                self.stdin.flush().expect("flush");
                self.permission_windows.push((at, Instant::now()));
                continue;
            }
            return Some((at, frame));
        }
    }

    /// Request/response with Desktop's 60 s non-prompt budget: reads until
    /// the response carrying our id, returning it plus the notifications
    /// seen along the way.
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> (serde_json::Value, Vec<(Instant, serde_json::Value)>) {
        let id = self.send(method, params);
        let mut notifications = Vec::new();
        loop {
            let (at, frame) = self
                .next_frame(DESKTOP_REQUEST_TIMEOUT)
                .expect("engine closed stdout");
            if frame.get("method").is_none() && frame.get("id").and_then(|v| v.as_u64()) == Some(id)
            {
                return (frame, notifications);
            }
            notifications.push((at, frame));
        }
    }
}

impl Drop for Acp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Active elapsed time in `[a, b]`: raw interval minus any permission-window
/// overlap (Desktop pauses the prompt budget while a permission request is
/// open — the scorecard's "outside tool windows").
fn active_gap(a: Instant, b: Instant, windows: &[(Instant, Instant)]) -> Duration {
    let mut paused = Duration::ZERO;
    for &(ws, we) in windows {
        let start = ws.max(a);
        let end = we.min(b);
        if end > start {
            paused += end - start;
        }
    }
    (b - a).saturating_sub(paused)
}

#[test]
fn c2_watchdog_live_long_turn_frame_cadence() {
    if key().is_none() {
        eprintln!("FLUX_TEST_KEY not set — skipping C2.3 Desktop-style watchdog");
        return;
    }

    let workspace = std::env::temp_dir().join(format!("nanok3-c2-watchdog-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("workspace");
    let nano_home = workspace.join("nano-home");
    std::fs::write(
        workspace.join("alpha.txt"),
        "On reliability: a system is reliable when its failures are loud, typed, and recoverable.\n",
    )
    .expect("seed alpha.txt");
    std::fs::write(
        workspace.join("beta.txt"),
        "On craft: the artisan measures twice because the material remembers every cut.\n",
    )
    .expect("seed beta.txt");
    std::fs::write(
        workspace.join("gamma.txt"),
        "On software: every abstraction leaks; the engineer's job is to choose which leaks to live with.\n",
    )
    .expect("seed gamma.txt");

    let mut acp = Acp::spawn(&workspace, &nano_home);
    let (init, _) = acp.request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], 1, "init: {init}");
    let (created, _) = acp.request(
        "session/new",
        serde_json::json!({ "cwd": workspace.to_string_lossy(), "mcpServers": [] }),
    );
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // ---- The long turn under the Desktop-style watchdog. ----
    let prompt_id = acp.send(
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": PROMPT }]
        }),
    );
    let prompt_sent = Instant::now();
    let mut stamps: Vec<Stamp> = Vec::new();
    let (answer, answer_at) = loop {
        let (at, frame) = acp
            .next_frame(DESKTOP_PROMPT_TIMEOUT)
            .expect("engine closed stdout mid-turn");
        if frame.get("method").is_none()
            && frame.get("id").and_then(|v| v.as_u64()) == Some(prompt_id)
        {
            break (frame, at);
        }
        if frame.get("method").and_then(|m| m.as_str()) == Some("session/update") {
            let update = &frame["params"]["update"];
            stamps.push(Stamp {
                at,
                kind: update["sessionUpdate"].as_str().unwrap_or("?").to_string(),
                tool_call_id: update["toolCallId"].as_str().map(str::to_string),
            });
        }
    };
    let turn_wall = answer_at - prompt_sent;

    // The turn completed.
    assert_eq!(
        answer["result"]["stopReason"], "end_turn",
        "long turn must complete: {answer}"
    );
    // Scenario validity: it was genuinely a long turn.
    assert!(
        turn_wall >= MIN_TURN_WALL,
        "turn wall {:?} < {:?} — the watchdog scenario requires a genuinely long turn",
        turn_wall,
        MIN_TURN_WALL
    );

    // ---- Cadence: every active inter-frame gap under Desktop's budget. ----
    assert!(
        stamps.len() >= 5,
        "a long multi-tool turn must stream frames, got {}",
        stamps.len()
    );
    let mut gaps_ms: Vec<f64> = Vec::new();
    let mut prev = prompt_sent;
    let mut max_gap = Duration::ZERO;
    for stamp in &stamps {
        let gap = active_gap(prev, stamp.at, &acp.permission_windows);
        gaps_ms.push(gap.as_secs_f64() * 1000.0);
        max_gap = max_gap.max(gap);
        prev = stamp.at;
    }
    // The response itself is also a frame: prompt-send-side cadence includes it.
    let tail_gap = active_gap(prev, answer_at, &acp.permission_windows);
    gaps_ms.push(tail_gap.as_secs_f64() * 1000.0);
    max_gap = max_gap.max(tail_gap);
    assert!(
        max_gap < DESKTOP_PROMPT_TIMEOUT,
        "active inter-frame gap {max_gap:?} exceeded Desktop's acp.promptTimeout budget {DESKTOP_PROMPT_TIMEOUT:?}"
    );
    let total_paused: Duration = acp
        .permission_windows
        .iter()
        .map(|(ws, we)| *we - *ws)
        .sum();
    let total_active = turn_wall.saturating_sub(total_paused);
    assert!(
        total_active < DESKTOP_PROMPT_TIMEOUT,
        "total active prompt time {total_active:?} exceeded Desktop's per-prompt budget (Desktop would cancel the turn)"
    );

    // ---- Well-ordered stream. ----
    for pair in stamps.windows(2) {
        assert!(
            pair[1].at >= pair[0].at,
            "frame arrivals must be monotonically ordered"
        );
    }
    let mut open_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut tool_calls = 0usize;
    let mut agent_chunks = 0usize;
    for stamp in &stamps {
        match stamp.kind.as_str() {
            "tool_call" => {
                tool_calls += 1;
                if let Some(id) = &stamp.tool_call_id {
                    open_calls.insert(id.clone());
                }
            }
            "tool_call_update" => {
                if let Some(id) = &stamp.tool_call_id {
                    assert!(
                        open_calls.contains(id),
                        "tool_call_update for {id} arrived before its tool_call"
                    );
                }
            }
            "agent_message_chunk" => agent_chunks += 1,
            _ => {}
        }
    }
    assert!(
        tool_calls >= 3,
        "multi-tool long turn expected >= 3 tool calls, got {tool_calls}"
    );
    assert!(
        agent_chunks >= 1,
        "final text must stream as agent_message_chunk"
    );
    // The prompt response is the final frame of the turn.
    let last = acp.frames.last().expect("frames");
    assert!(
        last.get("method").is_none() && last.get("id").and_then(|v| v.as_u64()) == Some(prompt_id),
        "the prompt response must be the final frame"
    );

    // Canary before any evidence leaves the process: the key is in no frame.
    let key_value = std::env::var("FLUX_TEST_KEY").unwrap_or_default();
    let captured = serde_json::to_string(&acp.frames).expect("frames json");
    assert!(
        key_value.is_empty() || !captured.contains(&key_value),
        "CANARY VIOLATION: credential leaked into ACP frames"
    );

    // ---- Run manifest. ----
    let frame_records: Vec<serde_json::Value> = stamps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            serde_json::json!({
                "seq": i,
                "kind": s.kind,
                "toolCallId": s.tool_call_id,
                "arrival_offset_ms": (s.at - prompt_sent).as_secs_f64() * 1000.0,
                "active_gap_from_prev_ms": gaps_ms[i],
            })
        })
        .collect();
    let manifest_path = workspace.join("c2-watchdog-manifest.json");
    let manifest = serde_json::json!({
        "test": "c2_watchdog_live_long_turn_frame_cadence",
        "criterion": "C2.3 event stream: turn-scoped frame every <600s outside tool windows (Desktop-style watchdog harness)",
        "desktop_semantics": {
            "reference": "desktop/src/process/agent/acp/AcpConnection.ts (promptTimeoutMs=300000; session/prompt budget; paused during permission windows)",
            "config": "acp.promptTimeout default 300s (clamp 30..3600s)",
            "bound_asserted_ms": DESKTOP_PROMPT_TIMEOUT.as_secs_f64() * 1000.0,
            "scorecard_bound_ms": 600_000,
        },
        "session_id": session_id,
        "child_pid": acp.pid,
        "workspace": workspace.to_string_lossy(),
        "turn": {
            "wall_ms": turn_wall.as_secs_f64() * 1000.0,
            "permission_window_total_ms": total_paused.as_secs_f64() * 1000.0,
            "active_total_ms": total_active.as_secs_f64() * 1000.0,
            "stop_reason": answer["result"]["stopReason"].clone(),
            "update_frames": stamps.len(),
            "tool_calls": tool_calls,
            "agent_message_chunks": agent_chunks,
        },
        "cadence": {
            "max_active_gap_ms": max_gap.as_secs_f64() * 1000.0,
            "tail_gap_to_response_ms": tail_gap.as_secs_f64() * 1000.0,
            "all_gaps_ms": gaps_ms,
        },
        "frames": frame_records,
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");
    eprintln!(
        "C2.3 watchdog: {} update frames, turn wall {:.1} s, max active gap {:.1} ms (bound {} ms) — manifest: {}",
        stamps.len(),
        turn_wall.as_secs_f64(),
        max_gap.as_secs_f64() * 1000.0,
        DESKTOP_PROMPT_TIMEOUT.as_secs_f64() * 1000.0,
        manifest_path.display()
    );
}
