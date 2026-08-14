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

/// Resolves the pid-file pids to HOST pids. On Linux the contained tree runs
/// in a bwrap pid namespace, so the fake server can only record
/// namespace-local pids; the host-side /proc maps them back (NSpid's last
/// value is the namespace-local pid). The server is identified by its
/// environ — FAKE_TREE_PID_FILE points at this test's unique file, which
/// keeps parallel tests in one cargo process (each running its own contained
/// tree, each with the same low namespace pids) unambiguous — and the
/// descendant as its direct child with the recorded namespace pid. Where
/// /proc is absent (macOS) the recorded pids are already host-real.
fn resolve_tree_pids(pid_file: &Path) -> (u32, u32) {
    let (server, descendant) = parse_pids(pid_file);
    #[cfg(target_os = "linux")]
    {
        let host_server = linux_host_server_pid(pid_file, server)
            .unwrap_or_else(|| panic!("no host pid for contained server {server}"));
        let host_descendant =
            linux_host_descendant_pid(host_server, descendant).unwrap_or_else(|| {
                panic!("no host pid for descendant {descendant} under {host_server}")
            });
        return (host_server, host_descendant);
    }
    #[allow(unreachable_code)]
    (server, descendant)
}

/// Numeric /proc entries as (host pid, path) pairs.
#[cfg(target_os = "linux")]
fn proc_entries() -> Vec<(u32, PathBuf)> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            Some((pid, entry.path()))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn nspid_last(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:"))
        .and_then(|value| value.split_whitespace().last())
        .and_then(|value| value.parse().ok())
}

#[cfg(target_os = "linux")]
fn linux_host_server_pid(pid_file: &Path, ns_pid: u32) -> Option<u32> {
    let needle = format!("FAKE_TREE_PID_FILE={}", pid_file.to_string_lossy());
    proc_entries().into_iter().find_map(|(pid, path)| {
        let environ = std::fs::read(path.join("environ")).ok()?;
        if !environ
            .split(|byte| *byte == 0)
            .any(|var| var == needle.as_bytes())
        {
            return None;
        }
        let status = std::fs::read_to_string(path.join("status")).ok()?;
        (nspid_last(&status) == Some(ns_pid)).then_some(pid)
    })
}

#[cfg(target_os = "linux")]
fn linux_host_descendant_pid(host_server: u32, ns_pid: u32) -> Option<u32> {
    proc_entries().into_iter().find_map(|(pid, path)| {
        let status = std::fs::read_to_string(path.join("status")).ok()?;
        let ppid = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .and_then(|value| value.trim().parse::<u32>().ok());
        (ppid == Some(host_server) && nspid_last(&status) == Some(ns_pid)).then_some(pid)
    })
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
    let (server, descendant) = resolve_tree_pids(&pid_file);
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
    let (server, descendant) = resolve_tree_pids(&pid_file);
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
