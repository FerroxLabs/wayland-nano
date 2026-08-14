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
//! Unix (§2.6, F-P3-2): Linux runs through the wayland-nano-linux-sandbox
//! helper (modern bwrap default); macOS runs through sandbox-exec with the
//! seatbelt policy builder. A process-group guardian owns each sandboxed
//! tree, reaping it on supervisor teardown or host death. Backend selection,
//! helper resolution, policy construction, setsid, and spawn all fail closed.

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
    #[cfg(unix)]
    UnixContained(UnixContainedChild),
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
            #[cfg(unix)]
            Self::UnixContained(child) => child.try_wait_exited(),
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
            #[cfg(unix)]
            Self::UnixContained(child) => child.terminate(),
            #[cfg(target_os = "windows")]
            Self::Contained(child) => child.terminate(),
        }
    }
}

pub struct HttpChild {
    pub(crate) shutdown: Sender<()>,
    pub(crate) exited: Arc<AtomicBool>,
}

#[cfg(unix)]
pub struct UnixContainedChild {
    child: Child,
    process_group: libc::pid_t,
}

#[cfg(unix)]
impl UnixContainedChild {
    fn try_wait_exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }

    fn terminate(&mut self) {
        unsafe {
            libc::killpg(self.process_group, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
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

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(unix, test))]
struct PlatformCommand {
    program: std::path::PathBuf,
    args: Vec<String>,
}

#[cfg(target_os = "linux")]
const NANO_LINUX_SANDBOX_EXE_ENV_VAR: &str = "NANO_LINUX_SANDBOX_EXE";
#[cfg(any(unix, test))]
const NANO_LINUX_SANDBOX_ARG0: &str = "wayland-nano-linux-sandbox";

#[cfg(any(unix, test))]
fn platform_sandbox_command_for(
    sandbox: Option<nano_sandbox::SandboxType>,
    command: Vec<String>,
    cwd: &std::path::Path,
    linux_resolver: impl FnOnce() -> Option<std::path::PathBuf>,
) -> Result<PlatformCommand, McpError> {
    use nano_core::permissions::{NetworkSandboxPolicy, PermissionProfile};
    let profile =
        PermissionProfile::workspace_write_with(&[], NetworkSandboxPolicy::Restricted, true, true);
    match sandbox {
        Some(nano_sandbox::SandboxType::LinuxSeccomp) => {
            let helper = linux_resolver().ok_or_else(|| {
                McpError::SandboxUnavailable(format!(
                    "`{NANO_LINUX_SANDBOX_ARG0}` helper not found"
                ))
            })?;
            let args = nano_sandbox::linux_landlock::create_linux_sandbox_command_args_for_permission_profile(
                command,
                cwd,
                &profile,
                cwd,
                false,
            );
            Ok(PlatformCommand {
                program: helper,
                args,
            })
        }
        Some(nano_sandbox::SandboxType::MacosSeatbelt) => {
            let (file_system_sandbox_policy, network_sandbox_policy) =
                profile.to_runtime_permissions();
            let args = nano_sandbox::macos_seatbelt::create_seatbelt_command_args(
                nano_sandbox::macos_seatbelt::CreateSeatbeltCommandArgsParams {
                    command,
                    file_system_sandbox_policy: &file_system_sandbox_policy,
                    network_sandbox_policy,
                    sandbox_policy_cwd: cwd,
                    extra_allow_unix_sockets: &[],
                },
            )
            .map_err(|_| McpError::SandboxUnavailable("seatbelt policy build failed".into()))?;
            Ok(PlatformCommand {
                program: nano_sandbox::macos_seatbelt::MACOS_PATH_TO_SEATBELT_EXECUTABLE.into(),
                args,
            })
        }
        other => Err(McpError::SandboxUnavailable(format!(
            "no unix backend for selection {other:?}"
        ))),
    }
}

#[cfg(any(target_os = "linux", test))]
fn resolve_linux_sandbox_exe_from(
    env_override: Option<std::path::PathBuf>,
    current_exe_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if let Some(path) = env_override
        && path.is_file()
    {
        return Some(path);
    }
    let dir = current_exe_dir?;
    [dir.to_path_buf(), dir.join("..")]
        .into_iter()
        .map(|dir| dir.join(NANO_LINUX_SANDBOX_ARG0))
        .find(|path| path.is_file())
}

#[cfg(target_os = "linux")]
fn resolve_linux_sandbox_exe() -> Option<std::path::PathBuf> {
    let env_override =
        std::env::var_os(NANO_LINUX_SANDBOX_EXE_ENV_VAR).map(std::path::PathBuf::from);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
    resolve_linux_sandbox_exe_from(env_override, exe_dir.as_deref())
}

#[cfg(target_os = "macos")]
fn resolve_linux_sandbox_exe() -> Option<std::path::PathBuf> {
    None
}

/// The guardian is the session/process-group leader. It keeps the sandboxed
/// server in the same group, watches the Nano host PID externally, and kills
/// the complete group if the host disappears.
///
/// Structure constraint: the server must run in the FOREGROUND (`exec "$@"`),
/// never backgrounded with `&`. A non-interactive POSIX shell (dash, macOS
/// bash-as-sh) assigns /dev/null to the stdin of an asynchronous command
/// BEFORE applying explicit redirections, so a backgrounded server sees an
/// instant stdin EOF and exits — the host's stdin pipe never reaches it, and
/// even `<&0` cannot recover it (it dups the already-reassigned fd). The
/// host-death watcher therefore moves into a backgrounded SUBSHELL (its own
/// stdin is irrelevant): it polls the host pid and the group-leader pid
/// (`$$`, which `exec` reuses for the sandboxed command), SIGKILLs the whole
/// process group when the host vanishes, and exits quietly when the server
/// finishes first.
#[cfg(unix)]
const UNIX_GUARDIAN: &str = r#"
parent=$1
shift
(
  while kill -0 "$parent" 2>/dev/null && kill -0 "$$" 2>/dev/null; do
    sleep 0.05
  done
  if ! kill -0 "$parent" 2>/dev/null; then
    kill -KILL -$$
  fi
) &
exec "$@"
"#;

/// Unix contained spawn. There is no raw-Command fallback.
#[cfg(unix)]
fn spawn_std(
    command: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<TransportParts, McpError> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let cwd = std::env::current_dir()
        .map_err(|_| McpError::SandboxUnavailable("current directory unavailable".into()))?;
    let sandboxed = platform_sandbox_command_for(
        nano_sandbox::get_platform_sandbox(true),
        std::iter::once(command.to_string())
            .chain(args.iter().cloned())
            .collect(),
        &cwd,
        resolve_linux_sandbox_exe,
    )?;
    use std::os::unix::fs::PermissionsExt;
    let executable = std::fs::metadata(&sandboxed.program)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0);
    if !executable {
        return Err(McpError::SandboxUnavailable(
            "sandbox executable is missing or not executable".into(),
        ));
    }
    let mut guardian_args = vec![
        "-c".to_string(),
        UNIX_GUARDIAN.to_string(),
        "wayland-nano-mcp-guardian".to_string(),
        std::process::id().to_string(),
        sandboxed.program.to_string_lossy().into_owned(),
    ];
    guardian_args.extend(sandboxed.args);
    let mut command = Command::new("/bin/sh");
    command
        .args(guardian_args)
        .envs(env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command
        .spawn()
        .map_err(|_| McpError::SandboxUnavailable("contained spawn failed".into()))?;
    let process_group = child.id() as libc::pid_t;
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    Ok(TransportParts {
        child: TransportChild::UnixContained(UnixContainedChild {
            child,
            process_group,
        }),
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

#[cfg(test)]
mod unix_transform_tests {
    use super::*;
    use nano_sandbox::SandboxType;

    #[test]
    fn absent_backend_fails_closed_typed() {
        let err = platform_sandbox_command_for(
            None,
            vec!["server".into()],
            std::path::Path::new("/workspace"),
            || None,
        )
        .expect_err("no backend must refuse");
        assert!(matches!(err, McpError::SandboxUnavailable(_)));
        assert!(err.to_string().contains("SANDBOX_UNAVAILABLE:"));
    }

    #[test]
    fn missing_linux_helper_fails_closed_typed() {
        let err = platform_sandbox_command_for(
            Some(SandboxType::LinuxSeccomp),
            vec!["server".into()],
            std::path::Path::new("/workspace"),
            || None,
        )
        .expect_err("missing helper must refuse");
        assert!(matches!(err, McpError::SandboxUnavailable(_)));
        assert!(err.to_string().contains(NANO_LINUX_SANDBOX_ARG0));
    }

    #[test]
    fn linux_transform_routes_through_helper_and_restricted_profile() {
        let helper = std::path::PathBuf::from("/opt/wayland-nano-linux-sandbox");
        let transformed = platform_sandbox_command_for(
            Some(SandboxType::LinuxSeccomp),
            vec!["server".into(), "--flag".into()],
            std::path::Path::new("/workspace"),
            || Some(helper.clone()),
        )
        .expect("transform");
        assert_eq!(transformed.program, helper);
        assert!(
            transformed
                .args
                .iter()
                .any(|arg| arg == "--permission-profile")
        );
        assert!(
            transformed
                .args
                .windows(2)
                .any(|pair| pair == ["--", "server"])
        );
        assert_eq!(transformed.args.last().map(String::as_str), Some("--flag"));
    }

    #[test]
    fn linux_helper_resolution_is_override_then_sibling_then_parent() {
        let root =
            std::env::temp_dir().join(format!("nano-mcp-helper-resolution-{}", std::process::id()));
        let deps = root.join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        let parent_helper = root.join(NANO_LINUX_SANDBOX_ARG0);
        std::fs::write(&parent_helper, b"probe").unwrap();
        assert_eq!(
            resolve_linux_sandbox_exe_from(None, Some(&deps))
                .and_then(|path| path.canonicalize().ok()),
            parent_helper.canonicalize().ok()
        );
        let override_helper = root.join("override-helper");
        std::fs::write(&override_helper, b"probe").unwrap();
        assert_eq!(
            resolve_linux_sandbox_exe_from(Some(override_helper.clone()), Some(&deps)),
            Some(override_helper)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
