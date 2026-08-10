//! nanok3-acp-profile — agent-path perf numbers for docs/metrics/C2-metrics.md.
//!
//! Measures (no live model calls anywhere):
//! - acp-host spawn→ready latency: process spawn to the first `initialize`
//!   response (the agent's first sign of life on the ACP wire);
//! - initialize handshake latency: warm-process `initialize` round trips;
//! - fixture-replayed turn frame throughput: scripted turn events pushed
//!   through the NDJSON codec + host-loop framing into a sink.
//!
//! The `nanok3` binary is located next to this binary in the target dir, or
//! via NANOK3_EXE. FLUX_API_KEY is set to a placeholder: `acp-host` refuses
//! to start without one, but `initialize`/`session/new` never touch the
//! network, and no prompt is ever sent.

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

    fn round_trip(&mut self, id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
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

/// Fixture-replayed turn throughput: scripted turn events through the real
/// NDJSON host loop (encode + frame + flush) into a counting sink.
fn frame_throughput(turns: usize, events_per_turn: usize) -> (u64, f64) {
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
        (events, Option::<nano_model::types::Usage>::None, "stop".into())
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        let mut reader = std::io::Cursor::new(input.into_bytes());
        let mut sink = CountingSink { frames: 0, bytes: 0 };
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
        println!(
            "{:<34} {} frames ({} bytes) in {:.2} ms",
            "fixture turn stream",
            sink.frames,
            sink.bytes,
            elapsed.as_secs_f64() * 1000.0
        );
        (sink.frames, elapsed.as_secs_f64())
    })
}

fn main() {
    let workspace = std::env::temp_dir().join(format!("nanok3-acp-prof-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("workspace");

    println!("=== acp profile: agent path (no live model calls) ===");
    println!("binary: {}", nanok3_exe().display());

    stats("spawn→ready (cold process)", spawn_to_ready(&workspace, 10));
    stats("initialize handshake (warm)", warm_initialize(&workspace, 50));

    let (frames, seconds) = frame_throughput(50, 100);
    println!(
        "{:<34} {:>8.0} frames/sec",
        "codec frame throughput",
        frames as f64 / seconds
    );

    let _ = std::fs::remove_dir_all(&workspace);
}
