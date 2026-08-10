//! nanok3-acp-profile — agent-path perf numbers for docs/metrics/C2-metrics.md.
//!
//! Measures (no live model calls anywhere):
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
//!   terminal `stream_end` out) through `run_host_loop`.
//!
//! The `nanok3` binary is located next to this binary in the target dir, or
//! via NANOK3_EXE. FLUX_API_KEY is set to a placeholder: `acp-host` refuses
//! to start without one, but `initialize`/`session/new` never touch the
//! network, and no prompt is ever sent.
//!
//! RSS is read externally (never self-reported by the measured child): on
//! Windows via a `Get-Process` PowerShell one-liner, on unix via `ps`. This
//! is a dev-profiling bin, not shipped runtime, so a shell-out is fine.
//!
//! `--json <path>` additionally writes a machine-readable report (used by
//! the advisory CI perf step in `.github/workflows/gate.yml`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Instant;

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
        let started = Instant::now();
        let mut child = Command::new(nanok3_exe())
            .arg("acp-host")
            .current_dir(workspace)
            .env("FLUX_API_KEY", "nanok3-perf-placeholder-key")
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

    println!("=== acp profile: agent path (no live model calls) ===");
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
