//! F-P3-2 / §2.6 proof: the contained stdio spawn kills the MCP server
//! child AND its direct descendant on graceful close — asserted via
//! EXTERNAL process inventory (OpenProcess / GetExitCodeProcess), never
//! self-report. Windows-only: the contained spawn is the Windows arm of
//! §2.6 (unix keeps the std spawn for v1 — the recorded deviation in
//! `stdio.rs`).
//!
//! Scope note (§2.6 v1 proof): this exercises the graceful-close path,
//! where the supervisor terminates the NO-BREAKAWAY job. The host-death
//! path needs no separate leg here: KILL_ON_JOB_CLOSE is a property of the
//! job object itself — the OS reaps the child and its direct descendants
//! when the last job handle dies with the host process. A test that kills
//! its own host cannot assert post-mortem from inside that host.

#![cfg(target_os = "windows")]

use nano_mcp::client::McpClient;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;

const FAKE: &str = env!("CARGO_BIN_EXE_wayland-nano-mcp-fake-server");
const STILL_ACTIVE: u32 = 259;

/// External process inventory: is `pid` a live process? A terminated
/// process OBJECT lingers while anyone holds a handle, so aliveness is the
/// exit code, not the object's existence.
fn alive(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return false;
    }
    let mut code: u32 = 0;
    let got = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    got != 0 && code == STILL_ACTIVE
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

#[test]
fn contained_close_kills_child_and_direct_descendant() {
    let dir = std::env::temp_dir().join(format!(
        "nano-mcp-tree-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pids.txt");

    // The "tree" scenario: the fake server spawns ONE direct descendant
    // (ping -t) and records both pids before answering initialize.
    let client = McpClient::connect(
        FAKE,
        &["tree".to_string()],
        &[(
            "FAKE_TREE_PID_FILE".to_string(),
            pid_file.to_string_lossy().into_owned(),
        )],
    )
    .expect("contained connect");

    wait_until(Duration::from_secs(10), || pid_file.exists(), "pid file");
    let pids = std::fs::read_to_string(&pid_file).unwrap();
    let parse = |key: &str| -> u32 {
        pids.lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("pid file must contain {key}: {pids:?}"))
    };
    let server_pid = parse("server=");
    let descendant_pid = parse("descendant=");

    // Control: both processes are LIVE before close — the test can tell
    // live from dead, so the post-close assertion is meaningful.
    assert!(alive(server_pid), "server must be alive before close");
    assert!(
        alive(descendant_pid),
        "direct descendant must be alive before close"
    );

    client.close();

    // Process teardown is asynchronous: poll for death.
    wait_until(
        Duration::from_secs(10),
        || !alive(server_pid),
        "server death after close",
    );
    wait_until(
        Duration::from_secs(10),
        || !alive(descendant_pid),
        "direct-descendant death after close",
    );
    assert!(!alive(server_pid), "server must be dead after close");
    assert!(
        !alive(descendant_pid),
        "direct descendant must be dead after close (orphan-free guarantee)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
