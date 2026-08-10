//! nanok3-acp-profile — agent-path perf numbers for docs/metrics/C2-metrics.md.
//!
//! Measures (no live model calls unless explicitly keyed — see the last item):
//! - acp-host spawn→ready latency: process spawn to the first `initialize`
//!   response (the agent's first sign of life on the ACP wire);
//! - initialize handshake latency: warm-process `initialize` round trips;
//! - fixture-replayed turn frame throughput: scripted turn events pushed
//!   through the NDJSON codec + host-loop framing into a sink;
//! - idle RSS: resident set of a spawned `acp-host` after `initialize`,
//!   settled, no turn running;
//! - active RSS: this process's peak working set while a fixture turn storm
//!   is replayed through the real host loop (the codec/framing path is the
//!   resident code; a spawned `acp-host` cannot run turns without a live
//!   model, so the storm runs in-process);
//! - task wall time: one full fixture turn end-to-end (message frame in →
//!   terminal `stream_end` out) through `run_host_loop`;
//! - active-agent RSS (LIVE, opt-in): when `FLUX_TEST_KEY` (or
//!   `FLUX_API_KEY`) is present in the environment, the profiler spawns the
//!   real `acp-host` child with that key, drives a live multi-tool turn
//!   through it (fs_read ×3 + fs_write, permissions auto-approved), and
//!   measures the CHILD's peak `WorkingSet64` externally while the turn
//!   runs. Without a key this metric prints `skipped` and the bin still
//!   exits 0, so CI is unaffected.
//!
//! The `nanok3` binary is located next to this binary in the target dir, or
//! via NANOK3_EXE. For the non-live metrics FLUX_API_KEY is set to a
//! placeholder: `acp-host` refuses to start without one, but
//! `initialize`/`session/new` never touch the network, and no prompt is
//! ever sent on that path.
//!
//! RSS is read externally (never self-reported by the measured child): on
//! Windows via a `Get-Process` PowerShell one-liner, on unix via `ps`. This
//! is a dev-profiling bin, not shipped runtime, so a shell-out is fine.
//!
//! `--json <path>` additionally writes a machine-readable report (used by
//! the advisory CI perf step in `.github/workflows/gate.yml`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn nanok3_exe() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("NANOK3_EXE") {
        return std::path::PathBuf::from(path);
    }
    let here = std::env::current_exe().expect("current exe");
    here.parent()
        .expect("target dir")
        .join(format!("nanok3{}", std::env::consts::EXE_SUFFIX))
}

struct AcpChild {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl AcpChild {
    fn spawn(workspace: &std::path::Path) -> (Self, Instant) {
        Self::spawn_with_key(workspace, "nanok3-perf-placeholder-key")
    }

    fn spawn_with_key(workspace: &std::path::Path, flux_key: &str) -> (Self, Instant) {
        let started = Instant::now();
        let mut child = Command::new(nanok3_exe())
            .arg("acp-host")
            .current_dir(workspace)
            .env("FLUX_API_KEY", flux_key)
            .env("NANOK3_HOME", workspace.join("nano-home"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nanok3 acp-host");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        (
            Self {
                child,
                stdin,
                stdout,
            },
            started,
        )
    }

    fn round_trip(
        &mut self,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).expect("write");
        self.stdin.flush().expect("flush");
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read");
            assert!(n > 0, "engine closed stdout");
            let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return frame;
            }
        }
    }

    /// Live `session/prompt`: reads until the prompt response, answering
    /// permission requests "allow once" inline. Returns the response plus
    /// every frame seen (canary substrate — checked for the key by the
    /// caller).
    fn prompt_live(
        &mut self,
        id: u64,
        session_id: &str,
        text: &str,
    ) -> (serde_json::Value, Vec<serde_json::Value>) {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }],
            },
        });
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).expect("write");
        self.stdin.flush().expect("flush");
        let mut frames = Vec::new();
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read");
            assert!(n > 0, "engine closed stdout mid-turn");
            let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
            let is_permission =
                frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission");
            frames.push(frame.clone());
            if is_permission {
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": frame["id"].clone(),
                    "result": { "outcome": { "outcome": "selected", "optionId": "allow" } }
                });
                writeln!(self.stdin, "{}", serde_json::to_string(&reply).unwrap()).expect("write");
                self.stdin.flush().expect("flush");
                continue;
            }
            if frame.get("method").is_none() && frame.get("id").and_then(|v| v.as_u64()) == Some(id)
            {
                return (frame, frames);
            }
        }
    }
}

impl Drop for AcpChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn stats(label: &str, mut samples: Vec<f64>) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let median = samples[samples.len() / 2];
    let min = samples[0];
    let max = samples[samples.len() - 1];
    println!(
        "{label:<34} mean {mean:>8.2} ms | median {median:>8.2} | min {min:>8.2} | max {max:>8.2} (n={})",
        samples.len()
    );
}

/// Sorted-sample summary for the JSON report.
fn summary(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    serde_json::json!({
        "mean": mean,
        "median": samples[samples.len() / 2],
        "min": samples[0],
        "max": samples[samples.len() - 1],
        "n": samples.len(),
    })
}

/// Current resident set (bytes) of `pid`, read externally. Windows:
/// `Get-Process` working set; unix: `ps` RSS (KiB). Dev-profiling only.
fn rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid}).WorkingSet64"),
        ])
        .output()
        .ok()?;
    #[cfg(not(windows))]
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let value: u64 = text.trim().parse().ok()?;
    #[cfg(windows)]
    return Some(value);
    #[cfg(not(windows))]
    return Some(value * 1024);
}

/// Peak working set (bytes) of `pid`. Windows: `PeakWorkingSet64`; Linux:
/// VmHWM. `None` where the platform exposes neither (e.g. macOS) — the
/// caller falls back to the current RSS and says so in the report.
fn peak_rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {pid}).PeakWorkingSet64"),
            ])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let kb: u64 = status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))?
            .trim()
            .strip_suffix("kB")?
            .trim()
            .parse()
            .ok()?;
        Some(kb * 1024)
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

/// Spawn→ready: fresh process, spawn-to-first-`initialize`-response.
fn spawn_to_ready(workspace: &std::path::Path, iterations: usize) -> Vec<f64> {
    let mut samples = Vec::new();
    for _ in 0..iterations {
        let (mut acp, started) = AcpChild::spawn(workspace);
        let response = acp.round_trip(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
            }),
        );
        assert_eq!(response["result"]["protocolVersion"], 1, "init: {response}");
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    samples
}

/// Warm-process initialize handshake round trips.
fn warm_initialize(workspace: &std::path::Path, iterations: usize) -> Vec<f64> {
    let (mut acp, _) = AcpChild::spawn(workspace);
    // Warm-up handshake (also covers the spawn, excluded from samples).
    acp.round_trip(
        1,
        "initialize",
        serde_json::json!({ "protocolVersion": 1, "clientCapabilities": {} }),
    );
    let mut samples = Vec::new();
    for id in 2..(iterations as u64 + 2) {
        let started = Instant::now();
        let response = acp.round_trip(
            id,
            "initialize",
            serde_json::json!({ "protocolVersion": 1, "clientCapabilities": {} }),
        );
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(response["result"]["agentInfo"]["name"], "nanok3");
    }
    samples
}

/// Replays `turns` scripted fixture turns through the real NDJSON host loop
/// (encode + frame + flush) into a counting sink, timing the loop. Returns
/// (frames, bytes, wall time).
fn run_fixture(turns: usize, events_per_turn: usize) -> (u64, u64, std::time::Duration) {
    struct CountingSink {
        frames: u64,
        bytes: u64,
    }
    impl Write for CountingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.frames += 1;
            self.bytes += buf.len() as u64;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    use nano_protocol::messages::{Event, ToolRequestBody};

    let mut input = String::new();
    for turn in 0..turns {
        input.push_str(&format!(
            "{{\"type\":\"message\",\"msg_id\":\"m{turn}\",\"content\":\"fixture replay\"}}\n"
        ));
    }

    let run_turn = move |msg_id: String, _content: String| async move {
        let mut events = Vec::new();
        for index in 0..events_per_turn {
            events.push(match index % 4 {
                0 => Event::TextDelta {
                    msg_id: msg_id.clone(),
                    text: "delta: the quick brown fox jumps over the lazy dog".into(),
                },
                1 => Event::ToolRunning {
                    call_id: format!("call-{index}"),
                    msg_id: msg_id.clone(),
                    tool_name: "fs_read".into(),
                },
                2 => Event::ToolRequest {
                    call_id: format!("call-{index}"),
                    msg_id: msg_id.clone(),
                    tool: ToolRequestBody {
                        name: "fs_read".into(),
                        args: serde_json::json!({"path": "src/main.rs"}),
                        category: Some("fs".into()),
                        description: None,
                    },
                },
                _ => Event::ToolResult {
                    call_id: format!("call-{index}"),
                    msg_id: msg_id.clone(),
                    output: "sha256:0123456789abcdef".into(),
                    output_type: Some("text".into()),
                    status: "success".into(),
                    tool_name: "fs_read".into(),
                    metadata: None,
                },
            });
        }
        (
            events,
            Option::<nano_model::types::Usage>::None,
            "stop".into(),
        )
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        let mut reader = std::io::Cursor::new(input.into_bytes());
        let mut sink = CountingSink {
            frames: 0,
            bytes: 0,
        };
        let config = nano_protocol::host::HostConfig {
            runtime_version: "profile".into(),
            session_id: "s-profile".into(),
            capabilities: nano_protocol::profile::v1_capabilities(),
        };
        let started = Instant::now();
        let exit = nano_protocol::host::run_host_loop(&mut reader, &mut sink, &config, run_turn)
            .await
            .expect("host loop");
        let elapsed = started.elapsed();
        assert_eq!(exit, nano_protocol::host::HostExit::StdinClosed);
        (sink.frames, sink.bytes, elapsed)
    })
}

/// Fixture-replayed turn throughput.
fn frame_throughput(turns: usize, events_per_turn: usize) -> (u64, f64) {
    let (frames, bytes, elapsed) = run_fixture(turns, events_per_turn);
    println!(
        "{:<34} {} frames ({} bytes) in {:.2} ms",
        "fixture turn stream",
        frames,
        bytes,
        elapsed.as_secs_f64() * 1000.0
    );
    (frames, elapsed.as_secs_f64())
}

/// Task wall time: one full fixture turn end-to-end — message frame in to
/// terminal stream_end out — per sample.
fn fixture_turn_wall(events_per_turn: usize, iterations: usize) -> Vec<f64> {
    let mut samples = Vec::new();
    for _ in 0..iterations {
        let (_, _, elapsed) = run_fixture(1, events_per_turn);
        samples.push(elapsed.as_secs_f64() * 1000.0);
    }
    samples
}

/// Idle RSS: fresh acp-host, initialize, settle, then sample the child's
/// resident set. Returns the median sample in bytes.
fn idle_rss(workspace: &std::path::Path) -> Option<u64> {
    let (mut acp, _) = AcpChild::spawn(workspace);
    let response = acp.round_trip(
        1,
        "initialize",
        serde_json::json!({ "protocolVersion": 1, "clientCapabilities": {} }),
    );
    assert_eq!(response["result"]["protocolVersion"], 1, "init: {response}");
    std::thread::sleep(std::time::Duration::from_millis(300)); // settle
    let mut samples = Vec::new();
    for _ in 0..3 {
        samples.push(rss_bytes(acp.child.id())?);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    samples.sort_unstable();
    Some(samples[samples.len() / 2])
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Result of the live active-agent RSS measurement.
struct LiveAgentRss {
    bytes: u64,
    frames: usize,
    turn_wall: Duration,
}

/// Live active-agent RSS (the C2.4 "active RSS" scorecard number): spawn
/// the REAL `acp-host` child with the live key, drive a live multi-tool turn
/// (fs_read ×3 + fs_write, permissions auto-approved) through it, and
/// measure the CHILD's working set externally — a 250 ms `WorkingSet64`
/// sampler thread plus the `PeakWorkingSet64` counter after the turn.
///
/// Guarded: returns `None` (metric reported as `skipped`, exit still 0)
/// unless `FLUX_TEST_KEY` or `FLUX_API_KEY` is present in the environment.
fn live_agent_rss(workspace: &std::path::Path) -> Option<LiveAgentRss> {
    let key = std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .or_else(|| std::env::var("FLUX_API_KEY").ok().filter(|k| !k.is_empty()))?;

    let live_ws = workspace.join("live-turn");
    std::fs::create_dir_all(&live_ws).expect("live workspace");
    for (name, content) in [
        (
            "alpha.txt",
            "alpha: reliability is loud, typed, recoverable failure.\n",
        ),
        (
            "beta.txt",
            "beta: the material remembers every cut; measure twice.\n",
        ),
        (
            "gamma.txt",
            "gamma: every abstraction leaks; choose which leaks to live with.\n",
        ),
    ] {
        std::fs::write(live_ws.join(name), content).expect("seed live fixture");
    }

    let (mut acp, _) = AcpChild::spawn_with_key(&live_ws, &key);
    let init = acp.round_trip(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], 1, "init: {init}");
    let created = acp.round_trip(
        2,
        "session/new",
        serde_json::json!({ "cwd": live_ws.to_string_lossy(), "mcpServers": [] }),
    );
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // External sampler: poll the child's current working set while the turn
    // runs; the OS peak counter (read post-turn) is the authoritative peak.
    let pid = acp.child.id();
    let stop = Arc::new(AtomicBool::new(false));
    let sampled_max = Arc::new(AtomicU64::new(0));
    let sampler = {
        let stop = Arc::clone(&stop);
        let sampled_max = Arc::clone(&sampled_max);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(bytes) = rss_bytes(pid) {
                    sampled_max.fetch_max(bytes, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        })
    };

    let started = Instant::now();
    let (response, frames) = acp.prompt_live(
        3,
        &session_id,
        "Multi-tool task. Step 1: use fs_read on alpha.txt, beta.txt, and gamma.txt (one call \
         each). Step 2: use fs_write to create summary.txt containing each file's text followed by \
         a one-line note on how it relates to reliable software. Step 3: reply DONE.",
    );
    let turn_wall = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    sampler.join().expect("sampler thread");

    assert!(
        response["result"]["stopReason"].is_string(),
        "live turn must answer: {response}"
    );
    // Canary, fail-closed: the key must appear in no frame.
    let captured = serde_json::to_string(&frames).expect("frames json");
    assert!(
        !captured.contains(&key),
        "CANARY VIOLATION: credential leaked into ACP frames"
    );

    let bytes = [
        peak_rss_bytes(pid),
        Some(sampled_max.load(Ordering::Relaxed)).filter(|b| *b > 0),
        rss_bytes(pid),
    ]
    .into_iter()
    .flatten()
    .max()
    .expect("child RSS readable on every supported platform");
    Some(LiveAgentRss {
        bytes,
        frames: frames.len(),
        turn_wall,
    })
}

fn main() {
    // `--json <path>` writes the machine-readable report the advisory CI
    // perf step merges into the leg's evidence manifest.
    let args: Vec<String> = std::env::args().collect();
    let json_path = args
        .windows(2)
        .find(|w| w[0] == "--json")
        .map(|w| std::path::PathBuf::from(&w[1]));

    let workspace = std::env::temp_dir().join(format!("nanok3-acp-prof-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("workspace");

    println!("=== acp profile: agent path (fixture metrics need no live model) ===");
    println!("binary: {}", nanok3_exe().display());

    let spawn_ready = spawn_to_ready(&workspace, 10);
    stats("spawn→ready (cold process)", spawn_ready.clone());
    let initialize = warm_initialize(&workspace, 50);
    stats("initialize handshake (warm)", initialize.clone());

    let (frames, seconds) = frame_throughput(50, 100);
    let throughput = frames as f64 / seconds;
    println!(
        "{:<34} {throughput:>8.0} frames/sec",
        "codec frame throughput"
    );

    // Task wall time: full single fixture turns, end-to-end.
    let task_wall = fixture_turn_wall(100, 50);
    stats("task wall time (1 fixture turn)", task_wall.clone());

    // Idle RSS: a live initialized acp-host, no turn running.
    let idle = idle_rss(&workspace);
    match idle {
        Some(bytes) => println!(
            "{:<34} {bytes} bytes ({:.2} MiB)",
            "idle RSS (acp-host)",
            mib(bytes)
        ),
        None => println!("{:<34} unavailable on this platform", "idle RSS (acp-host)"),
    }

    // Active RSS: peak working set of THIS process across a fixture turn
    // storm through the real host loop. The storm is what a busy turn does
    // on the codec/framing path; a spawned acp-host cannot run turns without
    // a live model, so the load is generated in-process.
    let self_pid = std::process::id();
    let (storm_frames, _, _) = run_fixture(500, 100);
    let active = peak_rss_bytes(self_pid).or_else(|| rss_bytes(self_pid));
    let active_kind = if peak_rss_bytes(self_pid).is_some() {
        "peak working set"
    } else {
        "current RSS (no peak counter)"
    };
    match active {
        Some(bytes) => println!(
            "{:<34} {bytes} bytes ({:.2} MiB) across a {storm_frames}-frame storm",
            "active RSS (turn storm)",
            mib(bytes)
        ),
        None => println!(
            "{:<34} unavailable on this platform",
            "active RSS (turn storm)"
        ),
    }

    // Active-AGENT RSS (the C2.4 scorecard number): the real spawned
    // acp-host child's peak working set during a live multi-tool turn.
    // Opt-in: without FLUX_TEST_KEY/FLUX_API_KEY the metric is `skipped`
    // and the bin still exits 0 (CI path unchanged).
    let live = live_agent_rss(&workspace);
    match &live {
        Some(m) => println!(
            "{:<34} {} bytes ({:.2} MiB) child peak working set over a live multi-tool turn ({} frames, {:.1} s wall)",
            "active RSS (live acp-host)",
            m.bytes,
            mib(m.bytes),
            m.frames,
            m.turn_wall.as_secs_f64()
        ),
        None => println!(
            "{:<34} skipped (no FLUX_TEST_KEY / FLUX_API_KEY)",
            "active RSS (live acp-host)"
        ),
    }

    if let Some(path) = json_path {
        let report = serde_json::json!({
            "tool": "nanok3-acp-profile",
            "at_unix": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "spawn_ready_ms": summary(spawn_ready),
            "initialize_warm_ms": summary(initialize),
            "frame_throughput_fps": throughput,
            "task_wall_ms": summary(task_wall),
            "idle_rss_bytes": idle,
            "active_rss_bytes": active,
            "active_rss_kind": active_kind,
            "active_agent_rss_bytes": live.as_ref().map(|m| m.bytes),
            "active_agent_rss_kind": if live.is_some() {
                "live-measured spawned acp-host child, multi-tool turn"
            } else {
                "skipped-no-key"
            },
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("json report dir");
        }
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&report).expect("report json"),
        )
        .expect("write json report");
        println!("json report: {}", path.display());
    }

    let _ = std::fs::remove_dir_all(&workspace);
}
