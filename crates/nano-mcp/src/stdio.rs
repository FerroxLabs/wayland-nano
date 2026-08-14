//! Stdio transport: spawn a CONTAINED child process and split its pipes for
//! the full-duplex dispatcher (design note §2.2/§2.6).
//!
//! Windows (§2.6, F-P3-2): spawns route through nano-sandbox's
//! `spawn_process_with_pipes_contained` — a NO-BREAKAWAY job object with
//! KILL_ON_JOB_CLOSE, so terminating the job (or closing its last handle on
//! host death) kills the child and all its DIRECT descendants. Supervisor
//! teardown is a job terminate; the supervisor stays the SOLE owner of
//! teardown (§2.3). No restricted-token plumbing is reachable from the MCP
//! registration path today (`nano-agent::mcp` passes only command/args/env),
//! so the child runs under the host's own token: the job object is the
//! floor, not the ceiling — the §2.6 restricted-token inheritance lands with
//! that plumbing.
//!
//! Unix (DEVIATION from §2.6, recorded): nano-sandbox's unix backends
//! (bwrap/landlock/seatbelt) are argv/policy builders for the tool-exec
//! lane, not a piped-spawn-with-process-group-teardown primitive reusable
//! here, so unix keeps the plain `std::process::Command` spawn for v1 —
//! child-kill only, NO direct-descendant containment and NO host-death
//! reaping. This is exactly why the stdio-MCP capability flag stays FALSE
//! until the §13 leg-1b process-inventory proofs pass on every tier-1
//! platform.

use crate::client::McpError;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// The owned halves of a spawned child, one per dispatcher thread:
/// the writer thread owns `stdin`, the reader thread owns `stdout`, and the
/// supervisor owns `child` (it is the SOLE owner of child-kill, §2.3).
pub struct TransportParts {
    pub child: TransportChild,
    pub stdin: Box<dyn Write + Send>,
    pub stdout: BufReader<Box<dyn Read + Send>>,
}

/// The supervisor-side handle to the spawned server (§2.3): a nonblocking
/// exit check plus a terminate that, under the contained spawn, is a
/// job-object terminate covering the child's DIRECT descendants (§2.6).
pub enum TransportChild {
    /// Uncontained std child: unix v1 (see module doc) and unit-test pipes.
    Std(Child),
    /// Streamable HTTP pump. The supervisor owns the sole shutdown sender;
    /// dropping/sending it tears down the pump and closes both virtual pipes.
    Http(HttpChild),
    /// Windows contained spawn (`spawn_process_with_pipes_contained`):
    /// terminate is `JobObject::terminate` on a NO-BREAKAWAY job with
    /// KILL_ON_JOB_CLOSE.
    #[cfg(target_os = "windows")]
    Contained(ContainedChild),
}

impl TransportChild {
    /// The supervisor's `try_wait` equivalent: has the child exited?
    pub fn try_wait_exited(&mut self) -> bool {
        match self {
            Self::Std(child) => child.try_wait().ok().flatten().is_some(),
            Self::Http(child) => child.exited.load(Ordering::Acquire),
            #[cfg(target_os = "windows")]
            Self::Contained(child) => child.try_wait_exited(),
        }
    }

    /// Terminates the child — under the contained spawn, the whole job:
    /// the child and all its direct descendants (§2.6).
    pub fn terminate(&mut self) {
        match self {
            Self::Std(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Self::Http(child) => {
                let _ = child.shutdown.send(());
            }
            #[cfg(target_os = "windows")]
            Self::Contained(child) => child.terminate(),
        }
    }
}

pub struct HttpChild {
    pub(crate) shutdown: Sender<()>,
    pub(crate) exited: Arc<AtomicBool>,
}

/// Windows contained child: the NO-BREAKAWAY job object (kill authority,
/// KILL_ON_JOB_CLOSE) plus the raw process handle (exit reaping).
#[cfg(target_os = "windows")]
pub struct ContainedChild {
    job: std::sync::Arc<nano_sandbox::job::JobObject>,
    process: std::os::windows::io::OwnedHandle,
}

#[cfg(target_os = "windows")]
impl ContainedChild {
    fn try_wait_exited(&self) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;
        const STILL_ACTIVE: u32 = 259;
        let mut code: u32 = 0;
        let ok = unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) };
        // Exit code 259 is ambiguous (a real exit code can collide), the
        // same caveat std's try_wait shares on this check; accepted for the
        // supervisor's exit reaping.
        ok != 0 && code != STILL_ACTIVE
    }

    fn terminate(&self) {
        let _ = self.job.terminate();
    }
}

/// Windows contained spawn (§2.6): the ONLY spawn path for MCP stdio
/// children on Windows. Fail closed — a contained-spawn error fails the
/// connect typed; there is NO raw-`Command` fallback.
#[cfg(target_os = "windows")]
fn spawn_contained(
    command: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<TransportParts, McpError> {
    use nano_sandbox::ConsoleMode;
    use nano_sandbox::StderrMode;
    use nano_sandbox::StdinMode;
    use nano_sandbox::spawn_process_with_pipes_contained;
    use std::collections::HashMap;
    use std::os::windows::io::FromRawHandle;
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;

    // §2.6 v1 scope: no restricted-token plumbing is reachable from the MCP
    // registration path, so the child spawns under the host's own token
    // (the established `get_current_token_for_restriction` pattern); the
    // NO-BREAKAWAY job object is the containment floor.
    let token = unsafe { nano_sandbox::get_current_token_for_restriction() }
        .map_err(|e| McpError::Transport(format!("spawn {command}: token: {e}")))?;
    let argv: Vec<String> = std::iter::once(command.to_string())
        .chain(args.iter().cloned())
        .collect();
    // Match std::process::Command env semantics: inherit the parent
    // environment, then apply the caller's overrides.
    let env_map: HashMap<String, String> = std::env::vars().chain(env.iter().cloned()).collect();
    let cwd = std::env::current_dir()
        .map_err(|e| McpError::Transport(format!("spawn {command}: cwd: {e}")))?;
    let spawned = spawn_process_with_pipes_contained(
        token,
        &argv,
        &cwd,
        &env_map,
        StdinMode::Open,
        // stderr must never merge into the protocol pipe; it is drained and
        // discarded below (the pre-containment Stdio::null policy).
        StderrMode::Separate,
        ConsoleMode::NoWindow,
        /*use_private_desktop*/ false,
        /*logs_base_dir*/ None,
    );
    unsafe { CloseHandle(token) };
    let spawned = spawned.map_err(|e| McpError::Transport(format!("spawn {command}: {e}")))?;

    let h_process = spawned.process.hProcess;
    let h_thread = spawned.process.hThread;
    unsafe { CloseHandle(h_thread) };
    let stdin_write = spawned
        .stdin_write
        .ok_or_else(|| McpError::Transport(format!("spawn {command}: stdin pipe missing")))?;
    let stdout_read = spawned.stdout_read;
    let stderr_read = spawned.stderr_read;
    let job = spawned.job();

    let stdin: Box<dyn Write + Send> =
        Box::new(unsafe { std::fs::File::from_raw_handle(stdin_write as RawHandle) });
    let stdout: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(unsafe {
        std::fs::File::from_raw_handle(stdout_read as RawHandle)
    }));
    if let Some(stderr_read) = stderr_read {
        // Detached drain: ends at EOF when the child tree dies (the job owns
        // the only other write end). Discarding matches the pre-containment
        // Stdio::null behavior.
        nano_sandbox::read_handle_loop(stderr_read, |_| {});
    }
    let child = TransportChild::Contained(ContainedChild {
        job,
        process: unsafe {
            std::os::windows::io::OwnedHandle::from_raw_handle(h_process as RawHandle)
        },
    });
    Ok(TransportParts {
        child,
        stdin,
        stdout,
    })
}

/// Unix v1 spawn (the recorded §2.6 deviation — see module doc).
#[cfg(not(target_os = "windows"))]
fn spawn_std(
    command: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<TransportParts, McpError> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(command)
        .args(args)
        .envs(env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| McpError::Transport(format!("spawn {command}: {e}")))?;
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    Ok(TransportParts {
        child: TransportChild::Std(child),
        stdin: Box::new(stdin),
        stdout: BufReader::new(Box::new(stdout)),
    })
}

pub struct StdioTransport {
    parts: Option<TransportParts>,
}

impl StdioTransport {
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, McpError> {
        #[cfg(target_os = "windows")]
        let parts = spawn_contained(command, args, env)?;
        #[cfg(not(target_os = "windows"))]
        let parts = spawn_std(command, args, env)?;
        Ok(Self { parts: Some(parts) })
    }

    #[cfg(test)]
    pub fn from_pipes(
        child: Child,
        stdin: std::process::ChildStdin,
        stdout: std::process::ChildStdout,
    ) -> Self {
        Self {
            parts: Some(TransportParts {
                child: TransportChild::Std(child),
                stdin: Box::new(stdin),
                stdout: BufReader::new(Box::new(stdout)),
            }),
        }
    }

    /// Splits the transport into the per-thread parts. After the split the
    /// `StdioTransport` shell is inert (its Drop kills nothing; the
    /// connection's supervisor owns the child from here on).
    pub fn into_parts(mut self) -> TransportParts {
        self.parts.take().expect("parts present")
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Only fires for a transport that was never split (e.g. a spawn
        // followed by a connect-time failure before Connection::spawn).
        // Under the contained spawn this terminates the job — the child and
        // its direct descendants (§2.6).
        if let Some(mut parts) = self.parts.take() {
            parts.child.terminate();
        }
    }
}
