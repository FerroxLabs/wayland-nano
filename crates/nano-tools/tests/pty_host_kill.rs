#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    TerminateProcess,
};

fn alive(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle == 0 {
        return false;
    }
    let mut code = 0;
    let read = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    read != 0 && code == STILL_ACTIVE as u32
}

#[test]
fn abrupt_host_termination_reaps_direct_descendant() {
    let probe = env!("CARGO_BIN_EXE_pty_host_probe");
    let mut host = Command::new(probe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = host.stdout.take().unwrap();
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let pid: u32 = line
        .trim()
        .strip_prefix("DIRECT_PID=")
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("invalid probe output: {line:?}"));
    assert!(alive(pid), "direct descendant must exist before host kill");

    let host_handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, host.id()) };
    assert_ne!(host_handle, 0);
    assert_ne!(unsafe { TerminateProcess(host_handle, 137) }, 0);
    unsafe { CloseHandle(host_handle) };
    let _ = host.wait().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !alive(pid),
        "direct descendant survived abrupt host termination"
    );
}
