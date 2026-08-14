//! Unix F-P3-2 process-inventory proofs. These tests are platform-gated, not
//! silently skipped: on Linux/macOS they require the selected sandbox backend
//! and helper to exist, and fail typed when it does not.

#![cfg(unix)]

use nano_mcp::client::McpClient;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const FAKE: &str = env!("CARGO_BIN_EXE_wayland-nano-mcp-fake-server");

fn proof_dir(label: &str) -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("nano-mcp-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn parse_pids(path: &Path) -> (u32, u32) {
    let pids = std::fs::read_to_string(path).unwrap();
    let parse = |key: &str| {
        pids.lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("pid file must contain {key}: {pids:?}"))
    };
    (parse("server="), parse("descendant="))
}

/// External process inventory oracle. `ps` owns the observation; neither the
/// MCP child nor its descendant self-reports liveness.
fn alive(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps process inventory");
    if !output.status.success() {
        return false;
    }
    let state = String::from_utf8_lossy(&output.stdout);
    !state.trim().is_empty() && !state.trim_start().starts_with('Z')
}

fn wait_until(deadline: Duration, pred: impl Fn() -> bool, what: &str) {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn connect_tree(pid_file: &Path) -> McpClient {
    McpClient::connect(
        FAKE,
        &["tree".to_string()],
        &[(
            "FAKE_TREE_PID_FILE".to_string(),
            pid_file.to_string_lossy().into_owned(),
        )],
    )
    .expect("contained unix connect")
}

#[test]
fn supervisor_close_kills_server_and_descendant() {
    let dir = proof_dir("close-tree");
    let pid_file = dir.join("pids.txt");
    let client = connect_tree(&pid_file);
    wait_until(Duration::from_secs(10), || pid_file.exists(), "pid file");
    let (server, descendant) = parse_pids(&pid_file);
    assert!(alive(server));
    assert!(alive(descendant));

    client.close();

    wait_until(Duration::from_secs(10), || !alive(server), "server death");
    wait_until(
        Duration::from_secs(10),
        || !alive(descendant),
        "descendant death",
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[ignore = "subprocess harness; driven by host_sigkill_reaps_server_tree"]
fn host_death_harness() {
    let pid_file = PathBuf::from(std::env::var_os("NANO_MCP_HOST_DEATH_PID_FILE").unwrap());
    let ready_file = PathBuf::from(std::env::var_os("NANO_MCP_HOST_DEATH_READY_FILE").unwrap());
    let _client = connect_tree(&pid_file);
    wait_until(Duration::from_secs(10), || pid_file.exists(), "pid file");
    std::fs::write(ready_file, b"ready").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[test]
fn host_sigkill_reaps_server_tree() {
    let dir = proof_dir("host-death-tree");
    let pid_file = dir.join("pids.txt");
    let ready_file = dir.join("ready");
    let mut host = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "host_death_harness", "--ignored", "--nocapture"])
        .env("NANO_MCP_HOST_DEATH_PID_FILE", &pid_file)
        .env("NANO_MCP_HOST_DEATH_READY_FILE", &ready_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn host harness");
    wait_until(
        Duration::from_secs(15),
        || ready_file.exists(),
        "host ready",
    );
    let (server, descendant) = parse_pids(&pid_file);
    assert!(alive(server));
    assert!(alive(descendant));

    unsafe {
        libc::kill(host.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = host.wait();

    wait_until(
        Duration::from_secs(10),
        || !alive(server),
        "server death after host SIGKILL",
    );
    wait_until(
        Duration::from_secs(10),
        || !alive(descendant),
        "descendant death after host SIGKILL",
    );
    let _ = std::fs::remove_dir_all(dir);
}
