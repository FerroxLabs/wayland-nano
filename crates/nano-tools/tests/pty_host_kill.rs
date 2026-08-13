//! External proof (design note §14 leg 3(b)): abrupt host termination leaves
//! zero direct-descendant survivors, verified with external process inventory
//! on Windows (job object) and on unix (host-death guard + process-group
//! kill).

#[cfg(windows)]
mod windows_leg {
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
}

#[cfg(unix)]
mod unix_leg {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn alive(pid: u32) -> bool {
        let probed = unsafe { libc::kill(pid as i32, 0) };
        probed == 0
    }

    /// External process inventory: the direct descendants of `host_pid`,
    /// per `pgrep -P` (procfs on Linux, libproc-backed on macOS).
    fn direct_descendants(host_pid: u32) -> Vec<u32> {
        let output = Command::new("pgrep")
            .arg("-P")
            .arg(host_pid.to_string())
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect()
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
        assert_eq!(line.trim(), "READY", "invalid probe output: {line:?}");

        // Inventory BEFORE the kill: the PTY guard must be a live direct
        // descendant of the probe host.
        let descendants = direct_descendants(host.id());
        assert!(
            !descendants.is_empty(),
            "PTY session leader must be a direct descendant of the host"
        );
        for &pid in &descendants {
            assert!(
                alive(pid),
                "direct descendant {pid} must exist before host kill"
            );
        }

        assert_eq!(unsafe { libc::kill(host.id() as i32, libc::SIGKILL) }, 0);
        let _ = host.wait().unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let survivors: Vec<_> = descendants
                .iter()
                .copied()
                .filter(|&pid| alive(pid))
                .collect();
            if survivors.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "direct descendants survived abrupt host termination: {survivors:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
