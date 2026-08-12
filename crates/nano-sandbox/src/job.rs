//! Job Object ownership of spawned process trees (kill-on-close containment).
//!
//! Provenance: adapted from Codex `codex-rs/utils/pty/src/win/job.rs` @
//! 646f7c0a — extracted per B-VND-01 ("vendor/port piecemeal", not the whole
//! 4.7k-line PTY crate). Transformations: `winapi` → windows-sys 0.52;
//! `filedescriptor::OwnedHandle` → `std::os::windows::io::OwnedHandle`;
//! `log::warn` → `tracing::warn`. Semantics unchanged: KILL_ON_JOB_CLOSE,
//! breakaway policy, suspended-spawn assignment, preserve/terminate race
//! safety via the state mutex.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::os::windows::io::RawHandle;
use std::sync::Mutex;
use tokio::process::Child;
use tokio::process::Command;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
use windows_sys::Win32::System::JobObjects::CreateJobObjectW;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_BREAKAWAY_OK;
use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
use windows_sys::Win32::System::JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION;
use windows_sys::Win32::System::JobObjects::JobObjectExtendedLimitInformation;
use windows_sys::Win32::System::JobObjects::SetInformationJobObject;
use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_SET_QUOTA;
use windows_sys::Win32::System::Threading::PROCESS_SUSPEND_RESUME;
use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;
use windows_sys::Win32::System::Threading::TerminateProcess;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process_handle: HANDLE) -> i32;
}

fn raw(h: HANDLE) -> RawHandle {
    h as RawHandle
}

/// Owns a Windows Job Object used to terminate a spawned process tree.
#[derive(Debug)]
pub struct JobObject {
    handle: OwnedHandle,
    // A mutex makes the state check, Job Object API call, and state update
    // atomic with respect to concurrent preserve and terminate requests.
    preserve_descendants: Mutex<bool>,
}

impl JobObject {
    /// Creates a Job Object configured to terminate all members when its last handle closes.
    pub fn create() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if handle == 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw(handle)) };

        Self::set_limit_flags(
            &handle,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        )?;

        Ok(Self {
            handle,
            preserve_descendants: Mutex::new(false),
        })
    }

    /// Creates a Job Object whose owned descendants cannot explicitly break away.
    pub fn create_without_breakaway() -> io::Result<Self> {
        let job = Self::create()?;
        Self::set_limit_flags(&job.handle, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
        Ok(job)
    }

    /// Captures an owned process handle before its numeric identifier can be reused.
    pub fn open_process_handle(process_id: u32) -> io::Result<OwnedHandle> {
        let handle = unsafe {
            OpenProcess(PROCESS_TERMINATE, /*bInheritHandle*/ 0, process_id)
        };
        if handle == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(unsafe { OwnedHandle::from_raw_handle(raw(handle)) })
    }

    /// Terminates the exact process identified by a previously captured handle.
    pub fn terminate_process_handle(handle: &OwnedHandle) -> io::Result<()> {
        let terminated = unsafe {
            TerminateProcess(handle.as_raw_handle() as HANDLE, /*uExitCode*/ 1)
        };
        if terminated == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn set_limit_flags(handle: &OwnedHandle, flags: u32) -> io::Result<()> {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = flags;
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle() as HANDLE,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of_mut!(limits).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Assigns a running process to this job.
    ///
    /// Assignment is not retroactive: descendants created before this call
    /// completes are not guaranteed to become members of the job.
    pub(crate) fn assign_process(&self, process_handle: RawHandle) -> io::Result<()> {
        let assigned = unsafe {
            AssignProcessToJobObject(
                self.handle.as_raw_handle() as HANDLE,
                process_handle as HANDLE,
            )
        };
        if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Prevents a child from running before it can be assigned to this job.
    pub fn prepare_suspended_spawn(&self, command: &mut Command) {
        command.creation_flags(CREATE_SUSPENDED).kill_on_drop(true);
    }

    /// Assigns and resumes a suspended child, returning whether assignment succeeded.
    ///
    /// Nested jobs can reject assignment. Such a child is resumed without
    /// containment so callers can preserve their existing compatibility fallback.
    pub fn assign_and_resume_process(&self, process_id: u32) -> io::Result<bool> {
        let process = unsafe {
            OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_SUSPEND_RESUME,
                /*bInheritHandle*/ 0,
                process_id,
            )
        };
        if process == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(raw(process)) };
        let assignment = self.assign_process(process.as_raw_handle());

        let status = unsafe { NtResumeProcess(process.as_raw_handle() as HANDLE) };
        if status < 0 {
            unsafe {
                TerminateProcess(process.as_raw_handle() as HANDLE, /*uExitCode*/ 1);
            }
            return Err(io::Error::other(format!(
                "failed to resume suspended process: NTSTATUS {status:#x}"
            )));
        }

        match assignment {
            Ok(()) => Ok(true),
            Err(error) => {
                tracing::warn!(
                    "Windows process job assignment unavailable for pid {process_id}: {error}"
                );
                Ok(false)
            }
        }
    }

    /// Starts a process only after assigning it to this Job Object.
    pub fn spawn_contained(&self, command: &mut Command) -> io::Result<Child> {
        self.prepare_suspended_spawn(command);
        let child = command.spawn()?;
        let process_handle = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("missing child process handle"))?;
        self.assign_process(process_handle)?;

        let status = unsafe { NtResumeProcess(process_handle as HANDLE) };
        if status < 0 {
            return Err(io::Error::other(format!(
                "failed to resume contained process: NTSTATUS {status:#x}"
            )));
        }

        Ok(child)
    }

    /// Allows contained descendants to keep running after the root exits normally.
    ///
    /// This disables both explicit job termination and kill-on-close for this
    /// object. Calls race safely with [`Self::terminate`]: whichever operation
    /// acquires the state lock first determines whether the process tree is
    /// preserved or terminated.
    pub fn preserve_descendants(&self) -> io::Result<()> {
        let mut preserve_descendants = self
            .preserve_descendants
            .lock()
            .map_err(|_| io::Error::other("job state lock poisoned"))?;
        if *preserve_descendants {
            return Ok(());
        }

        Self::set_limit_flags(&self.handle, JOB_OBJECT_LIMIT_BREAKAWAY_OK)?;
        *preserve_descendants = true;
        Ok(())
    }

    /// Terminates every process currently assigned to the job.
    pub fn terminate(&self) -> io::Result<()> {
        let preserve_descendants = self
            .preserve_descendants
            .lock()
            .map_err(|_| io::Error::other("job state lock poisoned"))?;
        if *preserve_descendants {
            return Ok(());
        }

        let terminated = unsafe {
            TerminateJobObject(self.handle.as_raw_handle() as HANDLE, /*uExitCode*/ 1)
        };
        if terminated == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl AsRawHandle for JobObject {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

#[cfg(test)]
mod nano_tests {
    //! Track-B exercise tests (not from the donor): contained spawn and
    //! whole-tree termination against real processes on this host.
    use super::*;

    #[test]
    fn contained_spawn_succeeds_and_tree_kill_works() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // 1. Contained spawn of a trivial process runs to a clean exit.
            let job = JobObject::create().unwrap();
            let mut cmd = Command::new("cmd.exe");
            cmd.args(["/c", "exit 0"]);
            let mut child = job.spawn_contained(&mut cmd).unwrap();
            let status = child.wait().await.unwrap();
            assert!(status.success(), "contained spawn must run: {status}");

            // 2. Tree kill: cmd starts a sleeping descendant; terminating the
            //    job must kill parent and descendant (no orphans).
            let job2 = JobObject::create().unwrap();
            let mut cmd2 = Command::new("cmd.exe");
            cmd2.args(["/c", "ping -t 127.0.0.1 > NUL"]);
            let mut child2 = job2.spawn_contained(&mut cmd2).unwrap();
            job2.terminate().unwrap();
            let status2 = child2.wait().await.unwrap();
            assert!(
                !status2.success(),
                "terminated job must not exit cleanly: {status2}"
            );
        });
    }

    /// Re-exec helper for the breakaway test: runs INSIDE the job as the
    /// child process. Attempts to spawn a grandchild with
    /// CREATE_BREAKAWAY_FROM_JOB and reports the outcome to the file named
    /// by NANO_JOB_BREAKAWAY_REPORT as `spawned <pid>` or
    /// `spawn-failed <error>`, then sleeps so the parent test can terminate
    /// the job around it. A no-op without the marker env var.
    #[test]
    fn breakaway_helper_entry() {
        if std::env::var_os("NANO_JOB_BREAKAWAY_HELPER").is_none() {
            return;
        }
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        let spawned = std::process::Command::new("ping.exe")
            .args(["-t", "127.0.0.1"])
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW)
            // Null stdio: a surviving (breakaway) grandchild must never
            // hold the test harness's pipes open.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let report = match spawned {
            Ok(child) => format!("spawned {}", child.id()),
            Err(err) => format!("spawn-failed {err}"),
        };
        let path = std::env::var("NANO_JOB_BREAKAWAY_REPORT").expect("report path");
        std::fs::write(path, report).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    /// C6 mandatory amendment (claude): a job created WITHOUT breakaway
    /// permission refuses a CREATE_BREAKAWAY_FROM_JOB grandchild outright —
    /// so no descendant can escape `terminate()`. The control leg (default
    /// `create()`, BREAKAWAY_OK) shows the grandchild DOES escape and
    /// survive `terminate()`, proving the test can tell the two apart.
    #[test]
    fn breakaway_attempting_grandchild_cannot_escape_no_breakaway_job() {
        use windows_sys::Win32::System::Threading::OpenProcess;
        use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
        use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;

        fn alive(pid: u32) -> bool {
            // A terminated process OBJECT lingers while anyone holds a
            // handle (the helper holds its Child), so aliveness is the exit
            // code, not the object.
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if handle == 0 {
                return false;
            }
            let mut code: u32 = 0;
            let got = unsafe {
                windows_sys::Win32::System::Threading::GetExitCodeProcess(handle, &mut code)
            };
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            const STILL_ACTIVE: u32 = 259;
            got != 0 && code == STILL_ACTIVE
        }

        fn run_leg(allow_breakaway: bool, report_path: &std::path::Path) -> String {
            let job = if allow_breakaway {
                JobObject::create().unwrap()
            } else {
                JobObject::create_without_breakaway().unwrap()
            };
            let exe = std::env::current_exe().unwrap();
            let mut cmd = Command::new(exe);
            cmd.args([
                "--exact",
                "job::nano_tests::breakaway_helper_entry",
                "--test-threads=1",
            ])
            .env("NANO_JOB_BREAKAWAY_HELPER", "1")
            .env("NANO_JOB_BREAKAWAY_REPORT", report_path)
            .kill_on_drop(true);
            let mut child = job.spawn_contained(&mut cmd).unwrap();
            // Wait for the helper's report file (it writes before sleeping).
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let report = loop {
                if let Ok(report) = std::fs::read_to_string(report_path) {
                    break report.trim().to_string();
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "helper report timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            // Teardown: terminate the job (kills the helper tree).
            job.terminate().unwrap();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let _ = child.kill().await;
                let _ = child.wait().await;
            });
            report
        }

        let tmp = std::env::temp_dir().join(format!("nano-job-breakaway-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // Under a no-breakaway job the grandchild spawn is REFUSED.
        let report = run_leg(false, &tmp.join("report-nb.txt"));
        assert!(
            report.starts_with("spawn-failed"),
            "no-breakaway job must refuse the breakaway grandchild: {report:?}"
        );

        // Control: under the default (BREAKAWAY_OK) job the grandchild
        // spawns and SURVIVES job termination — the exact escape the
        // amendment forbids on the task path. Clean it up explicitly.
        let report = run_leg(true, &tmp.join("report-ok.txt"));
        let pid: u32 = report
            .strip_prefix("spawned ")
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("control leg must spawn the grandchild: {report:?}"));
        let survived = alive(pid);
        // Always reap the control grandchild before asserting (a failed
        // assert must not leak it).
        let terminated = unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(
                OpenProcess(PROCESS_TERMINATE, 0, pid),
                1,
            )
        };
        assert!(terminated != 0, "control grandchild reaped");
        assert!(
            survived,
            "breakaway grandchild must SURVIVE terminate under the default job (control)"
        );
        // Process object teardown is asynchronous: poll for death.
        let death_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive(pid) && std::time::Instant::now() < death_deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!alive(pid), "reaped grandchild must be gone");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
