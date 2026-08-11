//! Shell tool: command execution through the sandboxed capture path.
//!
//! Architecture rules:
//! - commands execute ONLY through nano-sandbox (restricted spawn) — never a
//!   raw std::process::Command anywhere in this crate;
//! - the model is told exactly which execution environment is active
//!   (shell selection policy: native cmd by default, PowerShell on request;
//!   POSIX `sh -c` on unix);
//! - outputs are bounded, timeouts enforced, and the result carries the
//!   shell identity for protocol reporting;
//! - FAIL CLOSED: if the platform sandbox transform cannot be built (missing
//!   `wayland-nano-linux-sandbox` helper, seatbelt policy build error, no backend
//!   for the target), `run` returns a typed error and never spawns
//!   unsandboxed.
//!
//! Unix wiring provenance: adapted from the caller-side transform arms of
//! Codex `codex-rs/sandboxing/src/manager.rs` @ 646f7c0a
//! (`transform_linux_seccomp_request` / seatbelt arm /
//! `linux_sandbox_arg0_override`). See ../../UPSTREAM.md.

#[cfg(windows)]
use nano_core::abs::AbsolutePathBuf;
#[cfg(any(unix, test))]
use nano_core::permissions::NetworkSandboxPolicy;
use nano_core::permissions::PermissionProfile;
#[cfg(any(unix, test))]
use nano_sandbox::SandboxType;
#[cfg(windows)]
use nano_sandbox::capture::{CaptureResult, run_windows_sandbox_capture};
#[cfg(any(target_os = "linux", test))]
use nano_sandbox::linux_landlock::NANO_LINUX_SANDBOX_ARG0;
#[cfg(any(target_os = "linux", test))]
use nano_sandbox::linux_landlock::create_linux_sandbox_command_args_for_permission_profile;
#[cfg(any(target_os = "macos", test))]
use nano_sandbox::macos_seatbelt::CreateSeatbeltCommandArgsParams;
#[cfg(any(target_os = "macos", test))]
use nano_sandbox::macos_seatbelt::MACOS_PATH_TO_SEATBELT_EXECUTABLE;
#[cfg(any(target_os = "macos", test))]
use nano_sandbox::macos_seatbelt::create_seatbelt_command_args;
#[cfg(any(windows, unix))]
use std::collections::HashMap;
use std::path::Path;
#[cfg(any(target_os = "linux", test))]
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// Native Windows cmd.exe — the v1 default.
    #[cfg(windows)]
    Cmd,
    /// PowerShell 7 when available, Windows PowerShell otherwise.
    #[cfg(windows)]
    PowerShell,
    /// POSIX `sh -c` — the unix shell.
    #[cfg(unix)]
    Sh,
}

impl ShellKind {
    pub fn describe(self) -> &'static str {
        match self {
            #[cfg(windows)]
            ShellKind::Cmd => "cmd.exe (native Windows)",
            #[cfg(windows)]
            ShellKind::PowerShell => "PowerShell (native Windows)",
            #[cfg(unix)]
            ShellKind::Sh => "sh (POSIX unix shell)",
        }
    }

    /// The native default shell for the current platform.
    #[cfg(any(windows, unix))]
    pub fn platform_default() -> Self {
        #[cfg(windows)]
        {
            ShellKind::Cmd
        }
        #[cfg(unix)]
        {
            ShellKind::Sh
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    pub shell: ShellKind,
    pub duration: std::time::Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("sandbox spawn failed: {0}")]
    Spawn(String),
    /// The platform sandbox transform could not be built (or no backend
    /// exists for this target). The command was NOT executed — fail closed.
    #[error("sandbox unavailable: {0}")]
    SandboxUnavailable(String),
}

const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct ShellTool {
    /// Read by the Windows capture path (sandbox setup state lives under the
    /// Nano home); the unix transforms resolve policy from the workspace only.
    #[cfg_attr(not(windows), allow(dead_code))]
    nano_home: std::path::PathBuf,
    workspace: std::path::PathBuf,
}

impl ShellTool {
    pub fn new(nano_home: &Path, workspace: &Path) -> Self {
        Self {
            nano_home: nano_home.to_path_buf(),
            workspace: workspace.to_path_buf(),
        }
    }

    #[cfg(windows)]
    fn argv(shell: ShellKind, command: &str) -> Vec<String> {
        match shell {
            ShellKind::Cmd => vec!["cmd.exe".into(), "/c".into(), command.into()],
            ShellKind::PowerShell => vec![
                "powershell.exe".into(),
                "-NoProfile".into(),
                "-Command".into(),
                command.into(),
            ],
        }
    }

    /// Executes `command` contained to the workspace (workspace-write
    /// profile), with output bounds and an optional timeout.
    #[cfg(windows)]
    pub fn run(
        &self,
        shell: ShellKind,
        command: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<ShellOutput, ShellError> {
        self.run_with_env(shell, command, timeout, HashMap::new())
    }

    /// `run` with extra environment entries for the captured child. Tests use
    /// this to give each test its own TEMP/TMP: the transactional ACL
    /// traversal requires a quiescent tree, and a shared scoped temp root
    /// races under parallel cargo test (fail-closed verification trips).
    #[cfg(windows)]
    pub fn run_with_env(
        &self,
        shell: ShellKind,
        command: &str,
        timeout: Option<std::time::Duration>,
        extra_env: HashMap<String, String>,
    ) -> Result<ShellOutput, ShellError> {
        let started = std::time::Instant::now();
        let roots = [AbsolutePathBuf::from_absolute_path(&self.workspace)
            .map_err(|e| ShellError::Spawn(format!("workspace root: {e}")))?];
        let result: CaptureResult = run_windows_sandbox_capture(
            &PermissionProfile::workspace_write(),
            &roots,
            &self.nano_home,
            Self::argv(shell, command),
            &self.workspace,
            extra_env,
            timeout.map(|t| t.as_millis() as u64),
            None,
            false,
        )
        .map_err(|e| ShellError::Spawn(format!("{e:#}")))?;

        let (stdout, out_trunc) = truncate_lossy(result.stdout, MAX_OUTPUT_BYTES);
        let (stderr, err_trunc) = truncate_lossy(result.stderr, MAX_OUTPUT_BYTES);

        Ok(ShellOutput {
            exit_code: result.exit_code,
            stdout,
            stderr,
            timed_out: result.timed_out,
            truncated: out_trunc || err_trunc,
            shell,
            duration: started.elapsed(),
        })
    }

    /// Executes `command` contained to the workspace (workspace-write
    /// profile), with output bounds and an optional timeout.
    ///
    /// The spawn goes through the platform sandbox transform (Seatbelt on
    /// macOS, the `wayland-nano-linux-sandbox` helper on Linux). If the transform
    /// cannot be built, this returns a typed error and spawns nothing.
    #[cfg(unix)]
    pub fn run(
        &self,
        shell: ShellKind,
        command: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<ShellOutput, ShellError> {
        self.run_with_env(shell, command, timeout, HashMap::new())
    }

    /// `run` with extra environment entries for the spawned child (merged
    /// over the inherited environment). Mirrors the Windows seam; tests use
    /// it for per-test TEMP/TMP isolation.
    #[cfg(unix)]
    pub fn run_with_env(
        &self,
        shell: ShellKind,
        command: &str,
        timeout: Option<std::time::Duration>,
        extra_env: HashMap<String, String>,
    ) -> Result<ShellOutput, ShellError> {
        let started = std::time::Instant::now();
        let profile = unix_workspace_write_profile();
        let platform =
            platform_sandbox_command(unix_shell_argv(command), &profile, &self.workspace)?;
        let result = capture_unix_command_env(platform, &self.workspace, timeout, extra_env)?;

        let (stdout, out_trunc) = truncate_lossy(result.stdout, MAX_OUTPUT_BYTES);
        let (stderr, err_trunc) = truncate_lossy(result.stderr, MAX_OUTPUT_BYTES);

        Ok(ShellOutput {
            exit_code: result.exit_code,
            stdout,
            stderr,
            timed_out: result.timed_out,
            truncated: out_trunc || err_trunc,
            shell,
            duration: started.elapsed(),
        })
    }

    /// No sandbox backend exists for this target — fail closed, always.
    #[cfg(not(any(windows, unix)))]
    pub fn run(
        &self,
        _shell: ShellKind,
        _command: &str,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ShellOutput, ShellError> {
        Err(ShellError::SandboxUnavailable(
            "no sandbox backend exists for this platform; refusing to run unsandboxed".into(),
        ))
    }
}

/// The unix shell invocation: every unix command runs through `sh -c`.
#[cfg(any(unix, test))]
fn unix_shell_argv(command: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), command.into()]
}

/// The unix shell profile: workspace-write with the tmp roots EXCLUDED.
///
/// `PermissionProfile::workspace_write()` keeps the donor's `:slash_tmp` and
/// `:tmpdir` writable entries, which resolve to a writable `/tmp` bind (bwrap)
/// or Landlock RW rule (legacy path) — and the workspace's parent directory
/// typically lives inside the system temp dir, so those entries let a
/// "sandboxed" command write outside the workspace (the adversarial escape
/// tests write exactly there). Containment for this tool is strictly the
/// workspace, so both tmp roots are excluded here. Deviation from the donor
/// default recorded in UPSTREAM.md.
#[cfg(any(unix, test))]
fn unix_workspace_write_profile() -> PermissionProfile {
    PermissionProfile::workspace_write_with(
        &[],
        NetworkSandboxPolicy::Restricted,
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    )
}

/// A fully transformed, sandboxed command line ready to spawn.
#[cfg(any(unix, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlatformCommand {
    /// Complete argv with the sandbox program at index 0
    /// (`/usr/bin/sandbox-exec` on macOS, the helper binary on Linux).
    argv: Vec<String>,
    /// argv[0] override for self-invocation dispatch (donor `arg0` semantics).
    arg0: Option<String>,
}

/// Selects and builds the platform sandbox transform for the current target.
///
/// Provenance: codex `sandboxing/src/manager.rs` @ 646f7c0a (the
/// `SandboxType` match in `transform_request`), reduced to the two unix
/// backends and fail-closed everywhere else.
#[cfg(unix)]
fn platform_sandbox_command(
    command: Vec<String>,
    profile: &PermissionProfile,
    cwd: &Path,
) -> Result<PlatformCommand, ShellError> {
    platform_sandbox_command_for(
        nano_sandbox::get_platform_sandbox(true),
        command,
        profile,
        cwd,
    )
}

/// Builds the sandboxed command for an explicit backend selection. Split from
/// [`platform_sandbox_command`] so the fail-closed arms are test-covered on
/// every host.
#[cfg(any(unix, test))]
fn platform_sandbox_command_for(
    sandbox: Option<SandboxType>,
    command: Vec<String>,
    profile: &PermissionProfile,
    cwd: &Path,
) -> Result<PlatformCommand, ShellError> {
    match sandbox {
        Some(SandboxType::MacosSeatbelt) => {
            #[cfg(any(target_os = "macos", test))]
            {
                let argv = seatbelt_sandbox_argv(command, profile, cwd)?;
                Ok(PlatformCommand { argv, arg0: None })
            }
            #[cfg(not(any(target_os = "macos", test)))]
            {
                let _ = (command, profile, cwd);
                Err(ShellError::SandboxUnavailable(
                    "seatbelt argv builder is not compiled for this target".into(),
                ))
            }
        }
        Some(SandboxType::LinuxSeccomp) => {
            #[cfg(any(target_os = "linux", test))]
            {
                linux_sandbox_command(command, profile, cwd)
            }
            #[cfg(not(any(target_os = "linux", test)))]
            {
                let _ = (command, profile, cwd);
                Err(ShellError::SandboxUnavailable(
                    "linux sandbox argv builder is not compiled for this target".into(),
                ))
            }
        }
        other => Err(ShellError::SandboxUnavailable(format!(
            "no unix sandbox backend for platform selection {other:?}; refusing to run unsandboxed"
        ))),
    }
}

/// Builds the Seatbelt transform: policy argv from
/// [`create_seatbelt_command_args`] with `/usr/bin/sandbox-exec` prepended.
///
/// Provenance: codex `sandboxing/src/manager.rs` seatbelt arm @ 646f7c0a,
/// minus the managed-network-proxy parameters (dropped with the seatbelt
/// port; nano-egress owns egress).
#[cfg(any(target_os = "macos", test))]
fn seatbelt_sandbox_argv(
    command: Vec<String>,
    profile: &PermissionProfile,
    cwd: &Path,
) -> Result<Vec<String>, ShellError> {
    let (file_system_sandbox_policy, network_sandbox_policy) = profile.to_runtime_permissions();
    let mut args = create_seatbelt_command_args(CreateSeatbeltCommandArgsParams {
        command,
        file_system_sandbox_policy: &file_system_sandbox_policy,
        network_sandbox_policy,
        sandbox_policy_cwd: cwd,
        extra_allow_unix_sockets: &[],
    })
    .map_err(|e| ShellError::SandboxUnavailable(format!("seatbelt policy build failed: {e}")))?;
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(MACOS_PATH_TO_SEATBELT_EXECUTABLE.to_string());
    argv.append(&mut args);
    Ok(argv)
}

/// Environment variable overriding the location of the `wayland-nano-linux-sandbox`
/// helper binary (CI legs point it at the cargo-built helper).
#[cfg(any(target_os = "linux", test))]
pub const NANO_LINUX_SANDBOX_EXE_ENV_VAR: &str = "NANO_LINUX_SANDBOX_EXE";

/// Resolves the `wayland-nano-linux-sandbox` helper binary: explicit override
/// first, then next to the current executable, then the cargo `target/`
/// parent of a `deps/` test executable. Returns `None` when no candidate is
/// a real file — the caller must fail closed.
#[cfg(any(target_os = "linux", test))]
fn resolve_linux_sandbox_exe() -> Option<PathBuf> {
    let env_override = std::env::var_os(NANO_LINUX_SANDBOX_EXE_ENV_VAR).map(PathBuf::from);
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    resolve_linux_sandbox_exe_from(env_override, exe_dir.as_deref())
}

/// Pure candidate walk behind [`resolve_linux_sandbox_exe`], parameterized
/// for tests.
#[cfg(any(target_os = "linux", test))]
fn resolve_linux_sandbox_exe_from(
    env_override: Option<PathBuf>,
    current_exe_dir: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = env_override
        && path.is_file()
    {
        return Some(path);
    }
    let exe_dir = current_exe_dir?;
    for dir in [exe_dir.to_path_buf(), exe_dir.join("..")] {
        let candidate = dir.join(NANO_LINUX_SANDBOX_ARG0);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// argv[0] for the helper invocation: the full path when the binary is
/// already named `wayland-nano-linux-sandbox`, otherwise the canonical basename so
/// self-invoking dispatch sees the helper identity.
///
/// Provenance: codex `sandboxing/src/manager.rs` @ 646f7c0a
/// (`linux_sandbox_arg0_override`, near-verbatim).
#[cfg(any(target_os = "linux", test))]
fn linux_sandbox_arg0_override(exe: &Path) -> String {
    if exe.file_name().and_then(|name| name.to_str()) == Some(NANO_LINUX_SANDBOX_ARG0) {
        exe.to_string_lossy().into_owned()
    } else {
        NANO_LINUX_SANDBOX_ARG0.to_string()
    }
}

/// Builds the Linux transform: helper argv from
/// [`create_linux_sandbox_command_args_for_permission_profile`] with the
/// resolved helper binary prepended. Modern bubblewrap is the helper's
/// default; legacy Landlock stays opt-in and is not requested here.
///
/// Provenance: codex `sandboxing/src/manager.rs` @ 646f7c0a
/// (`transform_linux_seccomp_request`), minus the managed-network-proxy flag
/// (dropped with the landlock port; nano-egress owns egress).
#[cfg(any(target_os = "linux", test))]
fn linux_sandbox_command(
    command: Vec<String>,
    profile: &PermissionProfile,
    cwd: &Path,
) -> Result<PlatformCommand, ShellError> {
    linux_sandbox_command_with_resolver(command, profile, cwd, resolve_linux_sandbox_exe)
}

/// Resolver-parameterized core of [`linux_sandbox_command`] so the
/// missing-helper fail-closed path is test-covered on every host.
#[cfg(any(target_os = "linux", test))]
fn linux_sandbox_command_with_resolver(
    command: Vec<String>,
    profile: &PermissionProfile,
    cwd: &Path,
    resolve: impl FnOnce() -> Option<PathBuf>,
) -> Result<PlatformCommand, ShellError> {
    let helper_exe = resolve().ok_or_else(|| {
        ShellError::SandboxUnavailable(format!(
            "`{NANO_LINUX_SANDBOX_ARG0}` helper binary not found (set \
             {NANO_LINUX_SANDBOX_EXE_ENV_VAR} or place the helper next to the \
             current executable); refusing to run unsandboxed"
        ))
    })?;
    let mut args =
        create_linux_sandbox_command_args_for_permission_profile(command, cwd, profile, cwd, false);
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(helper_exe.to_string_lossy().into_owned());
    argv.append(&mut args);
    Ok(PlatformCommand {
        argv,
        arg0: Some(linux_sandbox_arg0_override(&helper_exe)),
    })
}

#[cfg(unix)]
struct UnixCaptureResult {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

/// Spawns an already-transformed sandboxed command and captures its output,
/// mirroring the Windows capture's semantics: piped stdout/stderr drained on
/// reader threads, a poll loop against the deadline, kill past the timeout,
/// and the same synthetic timeout exit code (128 + 64) the Windows capture
/// reports. `extra_env` entries are merged over the inherited environment.
#[cfg(unix)]
fn capture_unix_command_env(
    platform: PlatformCommand,
    cwd: &Path,
    timeout: Option<std::time::Duration>,
    extra_env: HashMap<String, String>,
) -> Result<UnixCaptureResult, ShellError> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::process::Stdio;

    let Some((program, args)) = platform.argv.split_first() else {
        return Err(ShellError::SandboxUnavailable(
            "sandbox transform produced an empty argv; refusing to run".into(),
        ));
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(extra_env);
    if let Some(arg0) = platform.arg0 {
        command.arg0(arg0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| ShellError::Spawn(format!("{e}")))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let (tx_out, rx_out) = std::sync::mpsc::channel::<Vec<u8>>();
    let (tx_err, rx_err) = std::sync::mpsc::channel::<Vec<u8>>();
    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        let _ = tx_out.send(buf);
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        let _ = tx_err.send(buf);
    });

    let deadline = timeout.map(|t| std::time::Instant::now() + t);
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) => {
                if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                    break true;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ShellError::Spawn(format!("wait failed: {e}")));
            }
        }
    };
    if timed_out {
        let _ = child.kill();
    }
    let status = child
        .wait()
        .map_err(|e| ShellError::Spawn(format!("reap failed: {e}")))?;
    let _ = t_out.join();
    let _ = t_err.join();
    let stdout = rx_out.recv().unwrap_or_default();
    let stderr = rx_err.recv().unwrap_or_default();
    let exit_code = if timed_out {
        // Mirror the Windows capture's synthetic timeout exit code.
        128 + 64
    } else {
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
    };

    Ok(UnixCaptureResult {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

fn truncate_lossy(bytes: Vec<u8>, max: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(&bytes);
    if text.len() <= max {
        return (text.into_owned(), false);
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (format!("{}…[truncated]", &text[..cut]), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("nano-home");
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        (tmp, home, ws)
    }

    /// Per-test TEMP/TMP so the sandbox capture ACLs a per-test scoped temp
    /// root instead of the shared one (parallel cargo test races the
    /// fail-closed ACL verification otherwise).
    #[cfg(windows)]
    fn fixture_env(tmp: &tempfile::TempDir) -> HashMap<String, String> {
        let p = tmp.path().to_string_lossy().into_owned();
        HashMap::from([("TEMP".to_string(), p.clone()), ("TMP".to_string(), p)])
    }

    #[cfg(windows)]
    #[test]
    fn cmd_echo_returns_zero_and_output() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run_with_env(
                ShellKind::Cmd,
                "echo nano-shell",
                Some(std::time::Duration::from_secs(60)),
                fixture_env(&_tmp),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("nano-shell"), "stdout: {}", out.stdout);
        assert!(!out.timed_out);
        assert!(matches!(out.shell, ShellKind::Cmd));
    }

    #[cfg(windows)]
    #[test]
    fn cmd_write_inside_workspace_lands() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run_with_env(
                ShellKind::Cmd,
                "echo data > shell-out.txt && type shell-out.txt",
                Some(std::time::Duration::from_secs(60)),
                fixture_env(&_tmp),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 0);
        assert!(ws.join("shell-out.txt").exists());
        assert!(out.stdout.contains("data"));
    }

    #[cfg(windows)]
    #[test]
    fn nonzero_exit_code_surfaces() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run_with_env(
                ShellKind::Cmd,
                "exit 3",
                Some(std::time::Duration::from_secs(60)),
                fixture_env(&_tmp),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 3);
    }

    // --- Cross-platform transform tests ---------------------------------
    // The unix argv/policy builders are pure string construction (compiled
    // into nano-sandbox for downstream tests via the `unix-argv-builders`
    // feature), so these run on every host, including Windows.

    #[test]
    fn unix_shell_argv_is_sh_dash_c() {
        assert_eq!(
            unix_shell_argv("echo hi"),
            vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()]
        );
    }

    #[test]
    fn unix_profile_denies_writes_outside_the_workspace() {
        // The adversarial escape tests write one level above the workspace,
        // which lives inside the system temp dir on unix. The shell profile
        // must not make that location writable (donor `workspace_write`
        // grants `/tmp` + `$TMPDIR`; this tool excludes both).
        let (tmp, _home, ws) = fixture();
        let profile = unix_workspace_write_profile();
        let (file_system_sandbox_policy, _) = profile.to_runtime_permissions();
        assert!(!file_system_sandbox_policy.has_full_disk_write_access());
        let inside = ws.join("nano-inside-workspace.txt");
        let outside = tmp.path().join("nano-outside-workspace.txt");
        assert!(
            file_system_sandbox_policy.can_write_path_with_cwd(&inside, &ws),
            "workspace must stay writable"
        );
        assert!(
            !file_system_sandbox_policy.can_write_path_with_cwd(&outside, &ws),
            "workspace sibling under the system temp dir must NOT be writable"
        );
    }

    #[test]
    fn macos_transform_prepends_sandbox_exec() {
        let (_tmp, _home, ws) = fixture();
        let profile = PermissionProfile::workspace_write();
        let argv = seatbelt_sandbox_argv(unix_shell_argv("echo hi"), &profile, &ws)
            .expect("seatbelt argv builds for workspace-write");
        assert_eq!(argv[0], "/usr/bin/sandbox-exec");
        assert_eq!(argv[1], "-p");
        let sep = argv.iter().position(|arg| arg == "--").expect("separator");
        assert_eq!(argv[sep + 1..], ["sh", "-c", "echo hi"]);
    }

    #[test]
    fn linux_transform_prepends_helper_and_profile_flags() {
        let (_tmp, home, ws) = fixture();
        let helper = home.join(NANO_LINUX_SANDBOX_ARG0);
        std::fs::write(&helper, b"fake helper").unwrap();
        let profile = PermissionProfile::workspace_write();
        let command =
            linux_sandbox_command_with_resolver(unix_shell_argv("echo hi"), &profile, &ws, || {
                Some(helper.clone())
            })
            .expect("helper present");
        assert_eq!(command.argv[0], helper.to_string_lossy());
        for flag in [
            "--sandbox-policy-cwd",
            "--command-cwd",
            "--permission-profile",
        ] {
            assert!(command.argv.iter().any(|arg| arg == flag), "missing {flag}");
        }
        let sep = command
            .argv
            .iter()
            .position(|arg| arg == "--")
            .expect("separator");
        assert_eq!(command.argv[sep + 1..], ["sh", "-c", "echo hi"]);
        // Helper named correctly: arg0 is the full path (donor semantics).
        assert_eq!(
            command.arg0.as_deref(),
            Some(helper.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn linux_arg0_falls_back_to_basename_for_other_exe_names() {
        let named = Path::new("/tmp/nano-session/wayland-nano-linux-sandbox");
        assert_eq!(linux_sandbox_arg0_override(named), named.to_string_lossy());
        assert_eq!(
            linux_sandbox_arg0_override(Path::new("/tmp/nano-session/not-the-helper")),
            NANO_LINUX_SANDBOX_ARG0
        );
    }

    #[test]
    fn missing_helper_is_typed_error_and_nothing_spawns() {
        let (_tmp, home, ws) = fixture();
        let profile = PermissionProfile::workspace_write();
        let err = linux_sandbox_command_with_resolver(
            unix_shell_argv("echo pwn > must-not-run.txt"),
            &profile,
            &ws,
            || None,
        )
        .expect_err("missing helper must fail closed");
        assert!(
            matches!(err, ShellError::SandboxUnavailable(_)),
            "unexpected error: {err:?}"
        );
        assert!(!home.join("must-not-run.txt").exists());
        assert!(!ws.join("must-not-run.txt").exists());
    }

    #[test]
    fn helper_resolution_prefers_env_override_then_sibling_then_target_parent() {
        let (_tmp, home, _ws) = fixture();
        let env_helper = home.join("elsewhere").join(NANO_LINUX_SANDBOX_ARG0);
        std::fs::create_dir_all(env_helper.parent().unwrap()).unwrap();
        std::fs::write(&env_helper, b"fake helper").unwrap();
        assert_eq!(
            resolve_linux_sandbox_exe_from(Some(env_helper.clone()), None),
            Some(env_helper)
        );
        // An env override pointing at a missing file falls through to the
        // sibling candidates (still sandboxed either way).
        let exe_dir = home.join("bin");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let sibling = exe_dir.join(NANO_LINUX_SANDBOX_ARG0);
        std::fs::write(&sibling, b"fake helper").unwrap();
        assert_eq!(
            resolve_linux_sandbox_exe_from(Some(home.join("missing")), Some(&exe_dir)),
            Some(sibling)
        );
        // Nothing available: None — the caller turns this into a typed error.
        let empty = home.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(resolve_linux_sandbox_exe_from(None, Some(&empty)), None);
        assert_eq!(resolve_linux_sandbox_exe_from(None, None), None);
    }

    #[test]
    fn no_backend_selection_is_typed_error_not_unsandboxed_exec() {
        let (_tmp, home, ws) = fixture();
        let profile = PermissionProfile::workspace_write();
        for selection in [None, Some(SandboxType::None)] {
            let err = platform_sandbox_command_for(
                selection,
                unix_shell_argv("echo pwn > must-not-run.txt"),
                &profile,
                &ws,
            )
            .expect_err("no backend must fail closed");
            assert!(
                matches!(err, ShellError::SandboxUnavailable(_)),
                "unexpected error: {err:?}"
            );
        }
        assert!(!home.join("must-not-run.txt").exists());
        assert!(!ws.join("must-not-run.txt").exists());
    }

    // --- Unix runtime tests (execute for real on the CI unix legs) -------

    #[cfg(unix)]
    #[test]
    fn unix_echo_returns_zero_and_output() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Sh,
                "echo nano-shell",
                Some(std::time::Duration::from_secs(60)),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("nano-shell"), "stdout: {}", out.stdout);
        assert!(!out.timed_out);
        assert!(matches!(out.shell, ShellKind::Sh));
    }

    #[cfg(unix)]
    #[test]
    fn unix_write_inside_workspace_lands() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Sh,
                "echo data > shell-out.txt && cat shell-out.txt",
                Some(std::time::Duration::from_secs(60)),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(ws.join("shell-out.txt").exists());
        assert!(out.stdout.contains("data"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_nonzero_exit_code_surfaces() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Sh,
                "exit 3",
                Some(std::time::Duration::from_secs(60)),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 3);
    }

    #[cfg(unix)]
    #[test]
    fn unix_timeout_kills_and_reports() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Sh,
                "sleep 30",
                Some(std::time::Duration::from_millis(500)),
            )
            .expect("spawn");
        assert!(out.timed_out, "expected timeout, got {out:?}");
        // Synthetic timeout exit code, mirroring the Windows capture.
        assert_eq!(out.exit_code, 128 + 64);
    }
}
