//! Session-scoped persistent PTY manager and five-tool contract.

use nano_model::types::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BUFFER_CAPACITY: usize = 512 * 1024;
const MAX_LIVE_SESSIONS: usize = 8;
/// `pty_write.chars` cap in Unicode code points, matching the schema's
/// `maxLength` unit (JSON Schema `maxLength` counts code points, not bytes).
const MAX_WRITE_CHARS: usize = 16 * 1024;
const DEFAULT_READ_BYTES: usize = 32 * 1024;
const MAX_READ_BYTES: usize = 256 * 1024;
const DEFAULT_YIELD_MS: u64 = 1_000;
const MAX_YIELD_MS: u64 = 30_000;

/// Job Objects and Unix process groups contain direct descendants only.
/// Processes broker-spawned by Task Scheduler, WMI, or service RPC originate
/// outside that lifetime domain and are the documented post-RC2 limitation.
pub const PTY_BROKER_ESCAPE_LIMITATION: &str =
    "schtasks/WMI/service-RPC broker spawns are outside direct-descendant containment";

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("sandbox unavailable: {0}")]
    SandboxUnavailable(String),
    #[error("PTY spawn failed: {0}")]
    Spawn(String),
    #[error("that terminal session has exited{exit}", exit = exit_suffix(*.exit_code))]
    PtySessionGone { exit_code: Option<i32> },
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("PTY session limit reached (maximum {MAX_LIVE_SESSIONS})")]
    Capacity,
}

fn exit_suffix(code: Option<i32>) -> String {
    code.map_or_else(String::new, |code| format!(" (code {code})"))
}

#[derive(Debug, Clone, Deserialize)]
pub struct PtySpawnRequest {
    pub command: String,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PtySpawnResponse {
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PtyWriteRequest {
    pub session_id: String,
    pub chars: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EmptyResponse {}

#[derive(Debug, Clone, Deserialize)]
pub struct PtyReadRequest {
    pub session_id: String,
    pub after_offset: Option<u64>,
    pub yield_time_ms: Option<u64>,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PtyReadResponse {
    pub chunk: String,
    pub start_offset: u64,
    pub next_offset: u64,
    pub dropped_before: u64,
    pub resynced: bool,
    pub exited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PtyKillRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PtyListEntry {
    pub session_id: String,
    pub command_summary: String,
    pub exited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub started_at: u64,
}

#[derive(Debug)]
struct RollingBuffer {
    bytes: VecDeque<u8>,
    oldest: u64,
    next: u64,
}

impl RollingBuffer {
    fn new() -> Self {
        Self {
            bytes: VecDeque::with_capacity(BUFFER_CAPACITY),
            oldest: 0,
            next: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.next = self.next.saturating_add(bytes.len() as u64);
        if bytes.len() >= BUFFER_CAPACITY {
            self.bytes.clear();
            self.bytes
                .extend(bytes[bytes.len() - BUFFER_CAPACITY..].iter().copied());
        } else {
            let overflow = self
                .bytes
                .len()
                .saturating_add(bytes.len())
                .saturating_sub(BUFFER_CAPACITY);
            self.bytes.drain(..overflow);
            self.bytes.extend(bytes.iter().copied());
        }
        // Snap the retained window's leading edge to a UTF-8 character
        // boundary: continuation bytes orphaned at the front (by eviction, or
        // by a stream that began mid-scalar) are unservable, so drop them and
        // account them in `oldest`. Every byte before `oldest` is then
        // recorded as dropped and every byte in `[oldest, next)` is retained
        // — eviction never silently loses bytes outside `dropped_before`.
        let leading = self
            .bytes
            .iter()
            .take(4)
            .position(|byte| (*byte & 0b1100_0000) != 0b1000_0000)
            .unwrap_or(self.bytes.len().min(4));
        self.bytes.drain(..leading);
        self.oldest = self.next - self.bytes.len() as u64;
    }

    fn read(&self, requested: u64, max_bytes: usize) -> Result<BufferRead, PtyError> {
        if requested > self.next {
            return Err(PtyError::InvalidParams(format!(
                "after_offset {requested} is beyond next offset {}",
                self.next
            )));
        }
        let resynced = requested < self.oldest;
        let mut start = requested.max(self.oldest);
        let available = (self.next - start) as usize;
        let mut bytes: Vec<u8> = self
            .bytes
            .iter()
            .skip((start - self.oldest) as usize)
            .take(available.min(max_bytes))
            .copied()
            .collect();

        // A caller-supplied cursor may land in UTF-8 continuation bytes.
        // Snap forward by at most three bytes to a character boundary.
        // (Eviction cannot trigger this: `push` already snapped the retained
        // window's leading edge and accounted the drop in `oldest`.)
        let leading = bytes
            .iter()
            .take(4)
            .position(|byte| (*byte & 0b1100_0000) != 0b1000_0000)
            .unwrap_or(bytes.len().min(4));
        bytes.drain(..leading);
        start += leading as u64;

        // Hold back only a trailing incomplete scalar. Complete invalid bytes
        // remain visible via lossy decoding; offset accounting always remains
        // in the original raw-byte unit.
        let emit_len = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => bytes.len(),
        };
        bytes.truncate(emit_len);
        Ok(BufferRead {
            chunk: String::from_utf8_lossy(&bytes).into_owned(),
            start,
            next: start + bytes.len() as u64,
            dropped_before: self.oldest,
            resynced,
        })
    }
}

struct BufferRead {
    chunk: String,
    start: u64,
    next: u64,
    dropped_before: u64,
    resynced: bool,
}

#[derive(Debug)]
struct SessionState {
    output: RollingBuffer,
    exited: bool,
    exit_code: Option<i32>,
    drain_done: bool,
    drain_failed: bool,
}

struct SharedState {
    state: Mutex<SessionState>,
    changed: Condvar,
}

enum Teardown {
    #[cfg(windows)]
    Windows {
        job: Arc<nano_sandbox::job::JobObject>,
        _conpty: nano_sandbox::conpty::ConptyInstance,
    },
    #[cfg(unix)]
    Unix { process_group: i32 },
}

impl Teardown {
    fn terminate(&self) -> Result<(), PtyError> {
        #[cfg(windows)]
        let Self::Windows { job, .. } = self;
        #[cfg(windows)]
        return job
            .terminate()
            .map_err(|error| PtyError::Spawn(format!("terminate PTY job: {error}")));

        #[cfg(unix)]
        let Self::Unix { process_group } = self;
        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(-*process_group, libc::SIGKILL) };
            if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(PtyError::Spawn(format!(
                    "terminate PTY process group: {}",
                    std::io::Error::last_os_error()
                )))
            }
        }
    }
}

struct PtySession {
    id: String,
    command_summary: String,
    started_at: u64,
    writer: Mutex<Box<dyn Write + Send>>,
    state: Arc<SharedState>,
    teardown: Teardown,
}

impl PtySession {
    fn snapshot(&self) -> PtyListEntry {
        let state = self.state.state.lock().unwrap_or_else(|p| p.into_inner());
        PtyListEntry {
            session_id: self.id.clone(),
            command_summary: self.command_summary.clone(),
            exited: state.exited,
            exit_code: state.exit_code,
            started_at: self.started_at,
        }
    }
}

/// Session-owned PTY registry. Dropping it terminates every live process tree.
pub struct PtySessionManager {
    workspace: PathBuf,
    sessions: Mutex<HashMap<String, Arc<PtySession>>>,
    next_id: Mutex<u64>,
}

impl PtySessionManager {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            sessions: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    pub fn spawn(&self, request: PtySpawnRequest) -> Result<PtySpawnResponse, PtyError> {
        if request.command.is_empty() {
            return Err(PtyError::InvalidParams("command must not be empty".into()));
        }
        self.prune_exited();
        let live = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|session| !session.snapshot().exited)
            .count();
        if live >= MAX_LIVE_SESSIONS {
            return Err(PtyError::Capacity);
        }
        let cwd = request.cwd.as_deref().unwrap_or(&self.workspace);
        let canonical_workspace = dunce::canonicalize(&self.workspace).map_err(|error| {
            PtyError::InvalidParams(format!("workspace cannot be canonicalized: {error}"))
        })?;
        let canonical_cwd = dunce::canonicalize(cwd).map_err(|error| {
            PtyError::InvalidParams(format!("cwd cannot be canonicalized: {error}"))
        })?;
        if !canonical_cwd.starts_with(&canonical_workspace) {
            return Err(PtyError::InvalidParams(
                "cwd must remain inside the session workspace".into(),
            ));
        }
        let id = {
            let mut next = self.next_id.lock().unwrap_or_else(|p| p.into_inner());
            let id = format!("pty_{}", *next);
            *next += 1;
            id
        };
        let command_summary: String = request.command.chars().take(80).collect();
        let state = Arc::new(SharedState {
            state: Mutex::new(SessionState {
                output: RollingBuffer::new(),
                exited: false,
                exit_code: None,
                drain_done: false,
                drain_failed: false,
            }),
            changed: Condvar::new(),
        });
        let spawned = spawn_platform(
            &request.command,
            &canonical_cwd,
            &canonical_workspace,
            Arc::clone(&state),
        )?;
        let session = Arc::new(PtySession {
            id: id.clone(),
            command_summary,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            writer: Mutex::new(spawned.writer),
            state,
            teardown: spawned.teardown,
        });
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), session);
        Ok(PtySpawnResponse { session_id: id })
    }

    pub fn write(&self, request: PtyWriteRequest) -> Result<EmptyResponse, PtyError> {
        if request.chars.chars().count() > MAX_WRITE_CHARS {
            return Err(PtyError::InvalidParams(format!(
                "chars exceeds {MAX_WRITE_CHARS} code points"
            )));
        }
        let session = self.session(&request.session_id)?;
        self.require_live(&session)?;
        let mut writer = session.writer.lock().unwrap_or_else(|p| p.into_inner());
        writer
            .write_all(request.chars.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|_| self.gone_error(&session))?;
        Ok(EmptyResponse {})
    }

    pub fn read(&self, request: PtyReadRequest) -> Result<PtyReadResponse, PtyError> {
        let max_bytes = request.max_bytes.unwrap_or(DEFAULT_READ_BYTES);
        if max_bytes == 0 || max_bytes > MAX_READ_BYTES {
            return Err(PtyError::InvalidParams(format!(
                "max_bytes must be between 1 and {MAX_READ_BYTES}"
            )));
        }
        let yield_ms = request.yield_time_ms.unwrap_or(DEFAULT_YIELD_MS);
        if yield_ms > MAX_YIELD_MS {
            return Err(PtyError::InvalidParams(format!(
                "yield_time_ms exceeds {MAX_YIELD_MS}"
            )));
        }
        let session = self.session(&request.session_id)?;
        let deadline = Instant::now() + Duration::from_millis(yield_ms);
        let mut state = session
            .state
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let requested = request.after_offset.unwrap_or(state.output.oldest);
        while requested == state.output.next
            && !(state.exited && state.drain_done)
            && !state.drain_failed
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let waited = session.state.changed.wait_timeout(state, remaining);
            let (next_state, result) = waited.unwrap_or_else(|p| p.into_inner());
            state = next_state;
            if result.timed_out() {
                break;
            }
        }
        if state.drain_failed {
            return Err(PtyError::PtySessionGone {
                exit_code: state.exit_code,
            });
        }
        if state.exited && state.drain_done && requested == state.output.next {
            return Err(PtyError::PtySessionGone {
                exit_code: state.exit_code,
            });
        }
        let read = state.output.read(requested, max_bytes)?;
        Ok(PtyReadResponse {
            chunk: read.chunk,
            start_offset: read.start,
            next_offset: read.next,
            dropped_before: read.dropped_before,
            resynced: read.resynced,
            exited: state.exited,
            exit_code: state.exit_code,
        })
    }

    pub fn kill(&self, request: PtyKillRequest) -> Result<EmptyResponse, PtyError> {
        let session = self.session(&request.session_id)?;
        self.require_live(&session)?;
        session.teardown.terminate()?;
        Ok(EmptyResponse {})
    }

    pub fn list(&self) -> Vec<PtyListEntry> {
        self.prune_exited();
        let mut entries: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|session| session.snapshot())
            .collect();
        entries.sort_by_key(|entry| entry.started_at);
        entries
    }

    pub fn terminate_all(&self) {
        let sessions: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        for session in sessions {
            let _ = session.teardown.terminate();
        }
    }

    fn session(&self, id: &str) -> Result<Arc<PtySession>, PtyError> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or(PtyError::PtySessionGone { exit_code: None })
    }

    fn require_live(&self, session: &PtySession) -> Result<(), PtyError> {
        let state = session
            .state
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if state.exited || state.drain_failed {
            Err(PtyError::PtySessionGone {
                exit_code: state.exit_code,
            })
        } else {
            Ok(())
        }
    }

    fn gone_error(&self, session: &PtySession) -> PtyError {
        let state = session
            .state
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        PtyError::PtySessionGone {
            exit_code: state.exit_code,
        }
    }

    fn prune_exited(&self) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        while sessions.len() >= MAX_LIVE_SESSIONS {
            let oldest_exited = sessions
                .iter()
                .filter_map(|(id, session)| {
                    let snapshot = session.snapshot();
                    snapshot.exited.then_some((id.clone(), snapshot.started_at))
                })
                .min_by_key(|(_, started_at)| *started_at)
                .map(|(id, _)| id);
            let Some(id) = oldest_exited else {
                break;
            };
            sessions.remove(&id);
        }
    }
}

impl Drop for PtySessionManager {
    fn drop(&mut self) {
        self.terminate_all();
    }
}

struct SpawnedPty {
    writer: Box<dyn Write + Send>,
    teardown: Teardown,
}

fn drain_output(mut reader: Box<dyn Read + Send>, state: Arc<SharedState>) {
    std::thread::spawn(move || {
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => {
                    let mut locked = state.state.lock().unwrap_or_else(|p| p.into_inner());
                    locked.drain_done = true;
                    state.changed.notify_all();
                    break;
                }
                Ok(count) => {
                    let mut locked = state.state.lock().unwrap_or_else(|p| p.into_inner());
                    locked.output.push(&chunk[..count]);
                    state.changed.notify_all();
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let mut locked = state.state.lock().unwrap_or_else(|p| p.into_inner());
                    locked.drain_failed = true;
                    state.changed.notify_all();
                    return;
                }
            }
        }
    });
}

fn monitor_exit(state: Arc<SharedState>, wait: impl FnOnce() -> i32 + Send + 'static) {
    std::thread::spawn(move || {
        let code = wait();
        let mut locked = state.state.lock().unwrap_or_else(|p| p.into_inner());
        locked.exited = true;
        locked.exit_code = Some(code);
        state.changed.notify_all();
    });
}

#[cfg(windows)]
fn spawn_platform(
    command: &str,
    cwd: &Path,
    _workspace: &Path,
    state: Arc<SharedState>,
) -> Result<SpawnedPty, PtyError> {
    use std::fs::File;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };

    let token = unsafe { nano_sandbox::token::get_current_token_for_restriction() }
        .map_err(|error| PtyError::SandboxUnavailable(format!("current user token: {error:#}")))?;
    let env = std::env::vars().collect();
    let argv = vec![
        "cmd.exe".into(),
        "/d".into(),
        "/v:on".into(),
        "/s".into(),
        "/c".into(),
        command.into(),
    ];
    let spawned = unsafe {
        nano_sandbox::conpty::spawn_conpty_process_as_user(token, &argv, cwd, &env, false, None)
    };
    unsafe { CloseHandle(token) };
    let (process_info, mut conpty) = spawned.map_err(|error| {
        PtyError::SandboxUnavailable(format!("contained ConPTY creation: {error:#}"))
    })?;
    let input = conpty
        .take_input_write()
        .ok_or_else(|| PtyError::Spawn("missing ConPTY input writer".into()))?;
    let output = conpty
        .take_output_read()
        .ok_or_else(|| PtyError::Spawn("missing ConPTY output reader".into()))?;
    let process = unsafe { OwnedHandle::from_raw_handle(process_info.hProcess as _) };
    let completion = conpty.completion_handle();
    let reader: File = output.into();
    let writer: File = input.into();
    drain_output(Box::new(reader), Arc::clone(&state));
    monitor_exit(state, move || {
        unsafe { WaitForSingleObject(process.as_raw_handle() as HANDLE, INFINITE) };
        completion.close();
        let mut code = 1;
        if unsafe { GetExitCodeProcess(process.as_raw_handle() as HANDLE, &mut code) } == 0 {
            1
        } else {
            code as i32
        }
    });
    Ok(SpawnedPty {
        writer: Box::new(writer),
        teardown: Teardown::Windows {
            job: conpty.job(),
            _conpty: conpty,
        },
    })
}

#[cfg(unix)]
fn spawn_platform(
    command: &str,
    cwd: &Path,
    workspace: &Path,
    state: Arc<SharedState>,
) -> Result<SpawnedPty, PtyError> {
    use nano_core::permissions::{NetworkSandboxPolicy, PermissionProfile};
    use nano_sandbox::portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let profile =
        PermissionProfile::workspace_write_with(&[], NetworkSandboxPolicy::Restricted, true, true);
    let argv = unix_pty_guard_argv(unix_sandbox_argv(command, cwd, workspace, &profile)?)?;
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| PtyError::SandboxUnavailable("empty sandbox transform".into()))?;
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| PtyError::Spawn(format!("open PTY: {error:#}")))?;
    let mut builder = CommandBuilder::new(program);
    builder.args(args);
    builder.cwd(cwd);
    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| PtyError::Spawn(format!("spawn sandboxed PTY: {error:#}")))?;
    let process_group = child
        .process_id()
        .ok_or_else(|| PtyError::Spawn("PTY child has no process id".into()))?
        as i32;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| PtyError::Spawn(format!("clone PTY reader: {error:#}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| PtyError::Spawn(format!("take PTY writer: {error:#}")))?;
    drain_output(reader, Arc::clone(&state));
    monitor_exit(state, move || {
        child
            .wait()
            .map(|status| status.exit_code() as i32)
            .unwrap_or(1)
    });
    Ok(SpawnedPty {
        writer,
        teardown: Teardown::Unix { process_group },
    })
}

/// Basename of the PTY host-death sentinel exec'd as the unix PTY child
/// (resolved next to the running executable, or via `NANO_PTY_GUARD_EXE`).
#[cfg(unix)]
const NANO_PTY_GUARD_ARG0: &str = "wayland-nano-pty-guard";

/// Wraps the sandboxed argv in the host-death guard. portable-pty `setsid()`s
/// the PTY child into its own session with no pre-exec hook, so the sandbox
/// helper alone would survive a host `kill -9` (§14 leg 3(b)). The guard
/// becomes the session leader, watches the host, and SIGKILLs its process
/// group when the host dies. Fail closed when the guard is absent: an
/// unwatched PTY is never spawned.
#[cfg(unix)]
fn unix_pty_guard_argv(inner: Vec<String>) -> Result<Vec<String>, PtyError> {
    let guard = std::env::var_os("NANO_PTY_GUARD_EXE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
            [directory.clone(), directory.join("..")]
                .into_iter()
                .map(|dir| dir.join(NANO_PTY_GUARD_ARG0))
                .find(|path| path.is_file())
        })
        .ok_or_else(|| {
            PtyError::SandboxUnavailable(format!(
                "`{NANO_PTY_GUARD_ARG0}` helper not found; refusing unwatched PTY"
            ))
        })?;
    let mut argv = vec![guard.to_string_lossy().into_owned()];
    argv.extend(inner);
    Ok(argv)
}

#[cfg(target_os = "linux")]
fn unix_sandbox_argv(
    command: &str,
    cwd: &Path,
    workspace: &Path,
    profile: &nano_core::permissions::PermissionProfile,
) -> Result<Vec<String>, PtyError> {
    use nano_sandbox::linux_landlock::{
        NANO_LINUX_SANDBOX_ARG0, create_linux_sandbox_command_args_for_permission_profile,
    };
    let helper = std::env::var_os("NANO_LINUX_SANDBOX_EXE")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
            [directory.clone(), directory.join("..")]
                .into_iter()
                .map(|dir| dir.join(NANO_LINUX_SANDBOX_ARG0))
                .find(|path| path.is_file())
        })
        .ok_or_else(|| {
            PtyError::SandboxUnavailable(format!(
                "`{NANO_LINUX_SANDBOX_ARG0}` helper not found; refusing unsandboxed PTY"
            ))
        })?;
    let mut argv = vec![helper.to_string_lossy().into_owned()];
    argv.extend(create_linux_sandbox_command_args_for_permission_profile(
        vec!["sh".into(), "-c".into(), command.into()],
        cwd,
        profile,
        workspace,
        false,
    ));
    Ok(argv)
}

#[cfg(target_os = "macos")]
fn unix_sandbox_argv(
    command: &str,
    cwd: &Path,
    workspace: &Path,
    profile: &nano_core::permissions::PermissionProfile,
) -> Result<Vec<String>, PtyError> {
    use nano_sandbox::macos_seatbelt::{
        CreateSeatbeltCommandArgsParams, MACOS_PATH_TO_SEATBELT_EXECUTABLE,
        create_seatbelt_command_args,
    };
    let (file_system_sandbox_policy, network_sandbox_policy) = profile.to_runtime_permissions();
    let mut args = create_seatbelt_command_args(CreateSeatbeltCommandArgsParams {
        command: vec!["sh".into(), "-c".into(), command.into()],
        file_system_sandbox_policy: &file_system_sandbox_policy,
        network_sandbox_policy,
        sandbox_policy_cwd: workspace,
        extra_allow_unix_sockets: &[],
    })
    .map_err(|error| PtyError::SandboxUnavailable(format!("seatbelt policy: {error}")))?;
    let mut argv = vec![MACOS_PATH_TO_SEATBELT_EXECUTABLE.into()];
    argv.append(&mut args);
    Ok(argv)
}

pub fn pty_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        definition(
            "pty_spawn",
            "Spawn a persistent terminal session. This action always requires host approval.",
            json!({"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"}},"required":["command"],"additionalProperties":false}),
        ),
        definition(
            "pty_write",
            "Write characters to an existing terminal session.",
            json!({"type":"object","properties":{"session_id":{"type":"string"},"chars":{"type":"string","maxLength":16384}},"required":["session_id","chars"],"additionalProperties":false}),
        ),
        definition(
            "pty_read",
            "Read bounded output from an existing terminal session using an absolute byte cursor and yield deadline.",
            json!({"type":"object","properties":{"session_id":{"type":"string"},"after_offset":{"type":"integer","minimum":0},"yield_time_ms":{"type":"integer","minimum":0,"maximum":30000},"max_bytes":{"type":"integer","minimum":1,"maximum":262144}},"required":["session_id"],"additionalProperties":false}),
        ),
        definition(
            "pty_kill",
            "Terminate an existing terminal session and its direct-descendant process tree.",
            json!({"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"],"additionalProperties":false}),
        ),
        definition(
            "pty_list",
            "List terminal sessions owned by this host session.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
    ]
}

fn definition(name: &str, description: &str, parameters: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: parameters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_tail_is_contiguous_and_resyncs_exactly() {
        let mut buffer = RollingBuffer::new();
        let data = vec![b'x'; BUFFER_CAPACITY + 137];
        buffer.push(&data);
        assert_eq!(buffer.oldest, 137);
        assert_eq!(buffer.next, (BUFFER_CAPACITY + 137) as u64);
        for offset in [buffer.oldest, buffer.oldest + 1, buffer.next - 1] {
            let read = buffer.read(offset, 1).unwrap();
            assert_eq!(read.start, offset);
            assert_eq!(read.next, offset + 1);
        }
        let stale = buffer.read(0, 32).unwrap();
        assert!(stale.resynced);
        assert_eq!(stale.start, 137);
        assert_eq!(stale.dropped_before, 137);
    }

    #[test]
    fn future_cursor_is_invalid() {
        let mut buffer = RollingBuffer::new();
        buffer.push(b"abc");
        assert!(matches!(buffer.read(4, 1), Err(PtyError::InvalidParams(_))));
    }

    #[test]
    fn broker_escape_limitation_is_explicit() {
        assert!(PTY_BROKER_ESCAPE_LIMITATION.contains("schtasks"));
        assert!(PTY_BROKER_ESCAPE_LIMITATION.contains("WMI"));
        assert!(PTY_BROKER_ESCAPE_LIMITATION.contains("direct-descendant"));
    }

    #[test]
    fn cwd_escape_is_refused_before_spawn() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        assert!(matches!(
            manager.spawn(PtySpawnRequest {
                command: "echo must-not-run".into(),
                cwd: Some(outside.path().to_path_buf()),
            }),
            Err(PtyError::InvalidParams(_))
        ));
        assert!(manager.list().is_empty());
    }

    #[test]
    fn split_utf8_scalar_is_held_back() {
        let mut buffer = RollingBuffer::new();
        buffer.push("A€B".as_bytes());
        let first = buffer.read(0, 2).unwrap();
        assert_eq!(first.chunk, "A");
        assert_eq!(first.next, 1);
        let second = buffer.read(first.next, 4).unwrap();
        assert_eq!(second.chunk, "€B");
        assert!(!second.chunk.contains('\u{fffd}'));
    }

    #[test]
    fn multibyte_flood_accounts_every_byte_exactly() {
        let mut buffer = RollingBuffer::new();
        // 3-byte scalars flooded past capacity so eviction lands mid-scalar.
        let flood = "€".repeat(BUFFER_CAPACITY / 3 + 7).into_bytes();
        buffer.push(&flood[..BUFFER_CAPACITY - 5]);
        buffer.push(&flood[BUFFER_CAPACITY - 5..]);
        assert_eq!(buffer.next, flood.len() as u64);
        // The leading edge snapped to a char boundary and the skipped
        // continuation bytes are accounted in `oldest`, not silently lost.
        assert_eq!(buffer.oldest, buffer.next - buffer.bytes.len() as u64);
        assert!(buffer.bytes.len() <= BUFFER_CAPACITY);
        assert_ne!(buffer.bytes[0] & 0b1100_0000, 0b1000_0000);

        let first = buffer.read(0, MAX_READ_BYTES).unwrap();
        assert!(first.resynced);
        assert_eq!(first.start, buffer.oldest);
        assert_eq!(first.dropped_before, buffer.oldest);
        assert!(!first.chunk.contains('\u{fffd}'));

        let second = buffer.read(first.next, MAX_READ_BYTES).unwrap();
        assert!(!second.resynced);
        assert_eq!(second.start, first.next);
        assert_eq!(second.next, buffer.next);
        assert!(!second.chunk.contains('\u{fffd}'));

        // Exactness: dropped + served == produced; no byte is lost,
        // duplicated, or left unaccounted.
        let served = (first.chunk.len() + second.chunk.len()) as u64;
        assert_eq!(served, buffer.next - buffer.oldest);
        assert_eq!(first.dropped_before + served, buffer.next);
    }

    #[test]
    fn independent_reader_cursors_each_observe_the_stream() {
        let mut buffer = RollingBuffer::new();
        buffer.push(b"first second");
        let reader_a_first = buffer.read(0, 5).unwrap();
        let reader_b = buffer.read(0, 12).unwrap();
        let reader_a_second = buffer.read(reader_a_first.next, 7).unwrap();
        assert_eq!(reader_a_first.chunk, "first");
        assert_eq!(reader_a_second.chunk, " second");
        assert_eq!(reader_b.chunk, "first second");
    }

    #[test]
    fn concurrent_readers_do_not_consume_global_output() {
        let buffer = Arc::new(Mutex::new(RollingBuffer::new()));
        buffer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(b"shared stream");
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let buffer = Arc::clone(&buffer);
                std::thread::spawn(move || {
                    buffer
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .read(0, 64)
                        .unwrap()
                        .chunk
                })
            })
            .collect();
        for reader in readers {
            assert_eq!(reader.join().unwrap(), "shared stream");
        }
    }

    #[test]
    fn parameter_bounds_are_fail_closed() {
        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        assert!(matches!(
            manager.read(PtyReadRequest {
                session_id: "missing".into(),
                after_offset: None,
                yield_time_ms: Some(MAX_YIELD_MS + 1),
                max_bytes: None,
            }),
            Err(PtyError::InvalidParams(_))
        ));
        assert!(matches!(
            manager.write(PtyWriteRequest {
                session_id: "missing".into(),
                chars: "x".repeat(MAX_WRITE_CHARS + 1),
            }),
            Err(PtyError::InvalidParams(_))
        ));
        // The cap counts code points, matching the schema's `maxLength` unit:
        // 16384 three-byte characters (48 KiB) pass the bound — the
        // missing-session error proves the cap check accepted the length.
        assert!(matches!(
            manager.write(PtyWriteRequest {
                session_id: "missing".into(),
                chars: "€".repeat(MAX_WRITE_CHARS),
            }),
            Err(PtyError::PtySessionGone { .. })
        ));
        assert!(matches!(
            manager.write(PtyWriteRequest {
                session_id: "missing".into(),
                chars: "€".repeat(MAX_WRITE_CHARS + 1),
            }),
            Err(PtyError::InvalidParams(_))
        ));
    }

    #[test]
    fn definitions_are_exactly_the_five_session_tools() {
        let names: Vec<_> = pty_tool_definitions().into_iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            ["pty_spawn", "pty_write", "pty_read", "pty_kill", "pty_list"]
        );
    }

    #[test]
    fn session_definitions_are_not_in_the_child_production_set() {
        let wiring = include_str!("../../nano-agent/src/wiring.rs");
        let production_start = wiring
            .find("pub fn v1_tool_definitions")
            .expect("production definition builder");
        let production_end = wiring[production_start..]
            .find("fn web_search_tool_definition")
            .map(|offset| production_start + offset)
            .expect("end of production definition builder");
        let production = &wiring[production_start..production_end];
        for definition in pty_tool_definitions() {
            assert!(
                !production.contains(&format!("\"{}\"", definition.name)),
                "{} must remain absent from child v1 definitions",
                definition.name
            );
        }
    }

    #[cfg(windows)]
    fn read_until(manager: &PtySessionManager, session_id: &str, needle: &str) -> PtyReadResponse {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut offset = 0;
        let mut all = String::new();
        loop {
            let read = manager
                .read(PtyReadRequest {
                    session_id: session_id.into(),
                    after_offset: Some(offset),
                    yield_time_ms: Some(250),
                    max_bytes: None,
                })
                .unwrap();
            offset = read.next_offset;
            all.push_str(&read.chunk);
            if all.contains(needle) {
                return read;
            }
            assert!(Instant::now() < deadline, "missing {needle:?} in {all:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn real_conpty_write_read_round_trip_and_yield_bound() {
        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        let spawned = manager
            .spawn(PtySpawnRequest {
                command: "set /p line= & echo marker:!line! & %SystemRoot%\\System32\\timeout.exe /t 2 /nobreak >nul".into(),
                cwd: None,
            })
            .unwrap();
        manager
            .write(PtyWriteRequest {
                session_id: spawned.session_id.clone(),
                chars: "hello\r\n".into(),
            })
            .unwrap();
        read_until(&manager, &spawned.session_id, "marker:hello");

        let start = Instant::now();
        let _ = manager
            .read(PtyReadRequest {
                session_id: spawned.session_id,
                after_offset: None,
                yield_time_ms: Some(50),
                max_bytes: None,
            })
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[cfg(windows)]
    #[test]
    fn manager_drop_reaps_a_real_direct_descendant() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };

        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        let spawned = manager
            .spawn(PtySpawnRequest {
                command: "%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe -NoProfile -EncodedCommand JABwAD0AUwB0AGEAcgB0AC0AUAByAG8AYwBlAHMAcwAgAHAAaQBuAGcALgBlAHgAZQAgAC0AQQByAGcAdQBtAGUAbgB0AEwAaQBzAHQAIAAnAC0AdAAnACwAJwAxADIANwAuADAALgAwAC4AMQAnACAALQBQAGEAcwBzAFQAaAByAHUAOwAgAFcAcgBpAHQAZQAtAE8AdQB0AHAAdQB0ACAAKAAnAEQASQBSAEUAQwBUAF8AUABJAEQAPQAnACAAKwAgACQAcAAuAEkAZAApADsAIABXAGEAaQB0AC0AUAByAG8AYwBlAHMAcwAgACQAcAAuAEkAZAA=".into(),
                cwd: None,
            })
            .unwrap();
        let mut offset = 0;
        let mut all = String::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let pid = loop {
            let read = manager
                .read(PtyReadRequest {
                    session_id: spawned.session_id.clone(),
                    after_offset: Some(offset),
                    yield_time_ms: Some(250),
                    max_bytes: None,
                })
                .unwrap();
            offset = read.next_offset;
            all.push_str(&read.chunk);
            if let Some(marker) = all.find("DIRECT_PID=") {
                let digits: String = all[marker + 11..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if !digits.is_empty() {
                    break digits.parse::<u32>().unwrap();
                }
            }
            assert!(
                Instant::now() < deadline,
                "direct child pid not reported; output={all:?}"
            );
        };

        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert_ne!(process, 0, "direct descendant must exist before teardown");
        drop(manager);
        let reaped = unsafe { WaitForSingleObject(process, 5_000) };
        unsafe { CloseHandle(process) };
        assert_eq!(
            reaped, WAIT_OBJECT_0,
            "direct descendant survived manager Drop"
        );
    }

    #[cfg(windows)]
    #[test]
    fn flood_without_readers_drains_and_exits() {
        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        // UTF-16LE base64 for: [Console]::Out.Write(('x' * 600000))
        let spawned = manager
            .spawn(PtySpawnRequest {
                command: "%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe -NoProfile -EncodedCommand WwBDAG8AbgBzAG8AbABlAF0AOgA6AE8AdQB0AC4AVwByAGkAdABlACgAKAAnAHgAJwAgACoAIAA2ADAAMAAwADAAMAApACkA".into(),
                cwd: None,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let entry = manager
                .list()
                .into_iter()
                .find(|entry| entry.session_id == spawned.session_id)
                .unwrap();
            if entry.exited {
                break;
            }
            assert!(Instant::now() < deadline, "flooding child deadlocked");
            std::thread::sleep(Duration::from_millis(25));
        }
        let read = manager
            .read(PtyReadRequest {
                session_id: spawned.session_id,
                after_offset: Some(0),
                yield_time_ms: Some(0),
                max_bytes: Some(MAX_READ_BYTES),
            })
            .unwrap();
        assert!(read.resynced);
        assert!(read.dropped_before > 0);
        assert_eq!(read.start_offset, read.dropped_before);
    }

    #[cfg(windows)]
    #[test]
    fn ninth_live_session_is_refused_and_kill_marks_exit() {
        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        let mut ids = Vec::new();
        for _ in 0..MAX_LIVE_SESSIONS {
            ids.push(
                manager
                    .spawn(PtySpawnRequest {
                        command: "%SystemRoot%\\System32\\timeout.exe /t 30 /nobreak >nul".into(),
                        cwd: None,
                    })
                    .unwrap()
                    .session_id,
            );
        }
        assert!(matches!(
            manager.spawn(PtySpawnRequest {
                command: "echo ninth".into(),
                cwd: None,
            }),
            Err(PtyError::Capacity)
        ));
        manager
            .kill(PtyKillRequest {
                session_id: ids[0].clone(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // list() prunes at capacity, and prune only removes EXITED
            // sessions — so the killed session being absent from the listing
            // proves the exit just as observing `exited` does.
            let entry = manager
                .list()
                .into_iter()
                .find(|entry| entry.session_id == ids[0]);
            match entry {
                Some(entry) if !entry.exited => {}
                Some(_) | None => break,
            }
            assert!(Instant::now() < deadline, "killed PTY did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            manager.write(PtyWriteRequest {
                session_id: ids[0].clone(),
                chars: "x".into(),
            }),
            Err(PtyError::PtySessionGone { .. })
        ));
        assert!(matches!(
            manager.kill(PtyKillRequest {
                session_id: "pty_unknown".into(),
            }),
            Err(PtyError::PtySessionGone { exit_code: None })
        ));
    }

    #[cfg(windows)]
    #[test]
    fn list_prunes_oldest_exited_at_capacity() {
        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        for _ in 0..MAX_LIVE_SESSIONS {
            manager
                .spawn(PtySpawnRequest {
                    command: "exit 0".into(),
                    cwd: None,
                })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let entries = manager.list();
            if !entries.is_empty() && entries.iter().all(|entry| entry.exited) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "quick-exit sessions never exited"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        // prune_exited runs at list() entry: at capacity the oldest exited
        // session is dropped, leaving MAX_LIVE_SESSIONS - 1 entries.
        assert_eq!(manager.list().len(), MAX_LIVE_SESSIONS - 1);
    }

    #[cfg(unix)]
    #[test]
    fn unix_sandbox_profile_round_trip_and_process_group_teardown() {
        let workspace = tempfile::tempdir().unwrap();
        let manager = PtySessionManager::new(workspace.path());
        let spawned = manager
            .spawn(PtySpawnRequest {
                command: "read line; sleep 30 & echo marker:$line; wait".into(),
                cwd: None,
            })
            .unwrap();
        let process_group = match &manager.session(&spawned.session_id).unwrap().teardown {
            Teardown::Unix { process_group } => *process_group,
        };
        manager
            .write(PtyWriteRequest {
                session_id: spawned.session_id.clone(),
                chars: "unix\n".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut offset = 0;
        let mut output = String::new();
        loop {
            let read = manager
                .read(PtyReadRequest {
                    session_id: spawned.session_id.clone(),
                    after_offset: Some(offset),
                    yield_time_ms: Some(250),
                    max_bytes: None,
                })
                .unwrap();
            offset = read.next_offset;
            output.push_str(&read.chunk);
            if output.contains("marker:unix") {
                break;
            }
            assert!(Instant::now() < deadline, "unix PTY output={output:?}");
        }
        assert_eq!(
            unsafe { libc::kill(-process_group, 0) },
            0,
            "registered host process group must be externally live"
        );
        drop(manager);
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::kill(-process_group, 0) } == 0 {
            assert!(
                Instant::now() < deadline,
                "direct descendant survived Unix manager Drop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
