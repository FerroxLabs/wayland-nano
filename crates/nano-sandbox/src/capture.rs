//! Legacy capture backend: direct restricted-token spawn with captured
//! stdout/stderr and exit code (the pre-unified-exec API, retained for
//! preflight checks and elevated result shaping).
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/lib.rs`
//! `mod windows_impl` @ 646f7c0a. Transformations: super:: -> crate:: paths;
//! codex_protocol -> nano_core::permissions; codex_utils_absolute_path ->
//! nano_core::abs; ensure_codex_home_exists -> ensure_nano_home_exists.
//! The donor's `#[cfg(not(windows))] mod stub` is intentionally not ported
//! (these modules compile only on Windows; noted in UPSTREAM.md).

use crate::WindowsSandboxCancellationToken;
use crate::logging::log_failure;
use crate::logging::log_note;
use crate::logging::log_success;
use crate::process::ConsoleMode;
use crate::process::create_process_as_user_with_job_policy;
use crate::sandbox_utils::ensure_nano_home_exists;
use crate::spawn_prep::LegacyAclSids;
use crate::spawn_prep::SpawnPrepOptions;
use crate::spawn_prep::allow_null_device_for_workspace_write;
use crate::spawn_prep::apply_legacy_session_acl_rules;
use crate::spawn_prep::legacy_session_capability_roots;
use crate::spawn_prep::prepare_legacy_session_security;
use crate::spawn_prep::prepare_legacy_spawn_context;
use anyhow::Result;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::PermissionProfile;
use std::collections::HashMap;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT;
use windows_sys::Win32::Foundation::SetHandleInformation;
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

type PipeHandles = ((HANDLE, HANDLE), (HANDLE, HANDLE), (HANDLE, HANDLE));

enum WaitOutcome {
    Exited,
    TimedOut,
    Cancelled,
}

/// Collects a capture-reader thread with a bounded wait.
///
/// Track A fix (`f77057450`): the donor joined the reader threads with no
/// bound — a sandboxed descendant that inherits the pipe write end keeps
/// `ReadFile` blocked after the root process exits, hanging the caller
/// forever. Here the reader gets a short drain window, then
/// `CancelSynchronousIo` against the reader thread, then a timed-out error
/// instead of an unbounded join.
fn finish_capture_reader(
    reader: std::thread::JoinHandle<Vec<u8>>,
    stream_name: &str,
) -> io::Result<Vec<u8>> {
    let drain_deadline = Instant::now() + Duration::from_secs(1);
    while !reader.is_finished() && Instant::now() < drain_deadline {
        std::thread::sleep(Duration::from_millis(5));
    }

    // A descendant can inherit the root's pipe handles and keep ReadFile blocked
    // after the root exits. Cancel only this reader's synchronous I/O; the
    // process/job lifecycle has already been decided by the caller.
    let cancel_deadline = Instant::now() + Duration::from_secs(1);
    while !reader.is_finished() && Instant::now() < cancel_deadline {
        let cancelled = unsafe { CancelSynchronousIo(reader.as_raw_handle() as HANDLE) };
        if cancelled == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_NOT_FOUND {
                return Err(io::Error::other(format!(
                    "CancelSynchronousIo failed for sandbox {stream_name} reader: {error}"
                )));
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    if !reader.is_finished() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("sandbox {stream_name} reader did not stop after cancellation"),
        ));
    }

    reader
        .join()
        .map_err(|_| io::Error::other(format!("sandbox {stream_name} reader panicked")))
}

fn wait_for_process(
    process: HANDLE,
    timeout_ms: Option<u64>,
    cancellation: Option<&WindowsSandboxCancellationToken>,
) -> WaitOutcome {
    let Some(cancellation) = cancellation else {
        let timeout = timeout_ms.map(|ms| ms as u32).unwrap_or(INFINITE);
        let res = unsafe { WaitForSingleObject(process, timeout) };
        return if res == 0x0000_0102 {
            WaitOutcome::TimedOut
        } else {
            WaitOutcome::Exited
        };
    };

    let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
    loop {
        if cancellation.is_cancelled() {
            return WaitOutcome::Cancelled;
        }
        let wait_ms = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return WaitOutcome::TimedOut;
                }
                remaining.min(Duration::from_millis(50)).as_millis() as u32
            }
            None => 50,
        };
        let res = unsafe { WaitForSingleObject(process, wait_ms) };
        if res == 0x0000_0102 {
            continue;
        }
        return WaitOutcome::Exited;
    }
}

unsafe fn setup_stdio_pipes() -> io::Result<PipeHandles> {
    let mut in_r: HANDLE = 0;
    let mut in_w: HANDLE = 0;
    let mut out_r: HANDLE = 0;
    let mut out_w: HANDLE = 0;
    let mut err_r: HANDLE = 0;
    let mut err_w: HANDLE = 0;
    if CreatePipe(&mut in_r, &mut in_w, ptr::null_mut(), 0) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if CreatePipe(&mut out_r, &mut out_w, ptr::null_mut(), 0) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if CreatePipe(&mut err_r, &mut err_w, ptr::null_mut(), 0) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if SetHandleInformation(in_r, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if SetHandleInformation(out_w, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    if SetHandleInformation(err_w, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
    }
    Ok(((in_r, in_w), (out_r, out_w), (err_r, err_w)))
}

pub struct CaptureResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run_windows_sandbox_capture(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    codex_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    cancellation: Option<WindowsSandboxCancellationToken>,
    use_private_desktop: bool,
) -> Result<CaptureResult> {
    run_windows_sandbox_capture_with_filesystem_overrides(
        permission_profile,
        workspace_roots,
        codex_home,
        command,
        cwd,
        env_map,
        timeout_ms,
        cancellation,
        &[],
        &[],
        use_private_desktop,
    )
}

/// The C6 background-task capture variant: the command's job is created
/// WITHOUT breakaway permission (a CREATE_BREAKAWAY_FROM_JOB grandchild can
/// never escape teardown) and its handle is pushed onto `job_sink` so the
/// task registry can terminate the whole command tree at teardown; the
/// handle is removed again when the command completes.
#[allow(clippy::too_many_arguments)]
pub fn run_windows_sandbox_capture_for_task(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    codex_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    job_sink: &std::sync::Mutex<Vec<std::sync::Arc<crate::job::JobObject>>>,
) -> Result<CaptureResult> {
    capture_inner(
        permission_profile,
        workspace_roots,
        codex_home,
        command,
        cwd,
        env_map,
        timeout_ms,
        None,
        &[],
        &[],
        false,
        /*allow_breakaway*/ false,
        Some(job_sink),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_windows_sandbox_capture_with_filesystem_overrides(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    codex_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    cancellation: Option<WindowsSandboxCancellationToken>,
    additional_deny_read_paths: &[AbsolutePathBuf],
    additional_deny_write_paths: &[AbsolutePathBuf],
    use_private_desktop: bool,
) -> Result<CaptureResult> {
    capture_inner(
        permission_profile,
        workspace_roots,
        codex_home,
        command,
        cwd,
        env_map,
        timeout_ms,
        cancellation,
        additional_deny_read_paths,
        additional_deny_write_paths,
        use_private_desktop,
        /*allow_breakaway*/ true,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn capture_inner(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    codex_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    mut env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    cancellation: Option<WindowsSandboxCancellationToken>,
    additional_deny_read_paths: &[AbsolutePathBuf],
    additional_deny_write_paths: &[AbsolutePathBuf],
    use_private_desktop: bool,
    allow_breakaway: bool,
    job_sink: Option<&std::sync::Mutex<Vec<std::sync::Arc<crate::job::JobObject>>>>,
) -> Result<CaptureResult> {
    let additional_deny_read_paths = additional_deny_read_paths
        .iter()
        .map(AbsolutePathBuf::to_path_buf)
        .collect::<Vec<_>>();
    let additional_deny_write_paths = additional_deny_write_paths
        .iter()
        .map(AbsolutePathBuf::to_path_buf)
        .collect::<Vec<_>>();
    let common = prepare_legacy_spawn_context(
        permission_profile,
        workspace_roots,
        codex_home,
        cwd,
        &mut env_map,
        &command,
        SpawnPrepOptions {
            inherit_path: false,
            add_git_safe_directory: false,
        },
    )?;
    let permissions = common.permissions;
    let current_dir = common.current_dir;
    let logs_base_dir = common.logs_base_dir.as_deref();
    let uses_write_capabilities = common.uses_write_capabilities;
    if !permissions.has_full_disk_read_access() {
        anyhow::bail!("Restricted read-only access requires the elevated Windows sandbox backend");
    }
    // WRITE_RESTRICTED tokens consult restricting SIDs only for writes, so this
    // backend cannot make capability-SID deny-read ACLs authoritative.
    if !additional_deny_read_paths.is_empty() {
        anyhow::bail!("deny-read overrides require the elevated Windows sandbox backend");
    }
    let capability_roots =
        legacy_session_capability_roots(&permissions, &current_dir, &env_map, codex_home);
    let security = prepare_legacy_session_security(
        uses_write_capabilities,
        codex_home,
        cwd,
        capability_roots,
    )?;
    allow_null_device_for_workspace_write(uses_write_capabilities);
    apply_legacy_session_acl_rules(
        &permissions,
        codex_home,
        &current_dir,
        &env_map,
        &additional_deny_read_paths,
        &additional_deny_write_paths,
        LegacyAclSids {
            readonly_sid: security.readonly_sid.as_ref(),
            readonly_sid_str: security.readonly_sid_str.as_deref(),
            write_root_sids: &security.write_root_sids,
            legacy_user_sid: security.legacy_user_sid_ptr(),
            effective_token: Some(security.h_token),
        },
    )?;
    let (stdin_pair, stdout_pair, stderr_pair) = unsafe { setup_stdio_pipes()? };
    let ((in_r, in_w), (out_r, out_w), (err_r, err_w)) = (stdin_pair, stdout_pair, stderr_pair);
    let spawn_res = unsafe {
        create_process_as_user_with_job_policy(
            security.h_token,
            &command,
            cwd,
            &env_map,
            logs_base_dir,
            Some((in_r, out_w, err_w)),
            ConsoleMode::Inherit,
            use_private_desktop,
            allow_breakaway,
        )
    };
    let created = match spawn_res {
        Ok(v) => v,
        Err(err) => {
            unsafe {
                CloseHandle(in_r);
                CloseHandle(in_w);
                CloseHandle(out_r);
                CloseHandle(out_w);
                CloseHandle(err_r);
                CloseHandle(err_w);
                CloseHandle(security.h_token);
            }
            return Err(err);
        }
    };
    let pi = created.process_info;
    let job = Arc::clone(&created.job);
    let _desktop = created;
    // C6: register the job handle with the task's kill domain BEFORE the
    // wait begins, so a teardown racing the command still terminates it;
    // deregister when the command is reaped (a live sink entry must always
    // mean a live command tree).
    if let Some(sink) = job_sink {
        sink.lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::clone(&job));
    }

    unsafe {
        CloseHandle(in_r);
        // Close the parent's stdin write end so the child sees EOF immediately.
        CloseHandle(in_w);
        CloseHandle(out_w);
        CloseHandle(err_w);
    }

    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let mut read_bytes: u32 = 0;
            let ok = unsafe {
                windows_sys::Win32::Storage::FileSystem::ReadFile(
                    out_r,
                    tmp.as_mut_ptr(),
                    tmp.len() as u32,
                    &mut read_bytes,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read_bytes == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read_bytes as usize]);
        }
        unsafe {
            CloseHandle(out_r);
        }
        buf
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 8192];
        loop {
            let mut read_bytes: u32 = 0;
            let ok = unsafe {
                windows_sys::Win32::Storage::FileSystem::ReadFile(
                    err_r,
                    tmp.as_mut_ptr(),
                    tmp.len() as u32,
                    &mut read_bytes,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read_bytes == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..read_bytes as usize]);
        }
        unsafe {
            CloseHandle(err_r);
        }
        buf
    });

    let wait_outcome = wait_for_process(pi.hProcess, timeout_ms, cancellation.as_ref());
    let timed_out = matches!(wait_outcome, WaitOutcome::TimedOut);
    let cancelled = matches!(wait_outcome, WaitOutcome::Cancelled);
    let mut exit_code_u32: u32 = 1;
    if !timed_out && !cancelled {
        unsafe {
            GetExitCodeProcess(pi.hProcess, &mut exit_code_u32);
        }
    }
    if timed_out || cancelled {
        if let Err(job_err) = job.terminate() {
            log_note(
                &format!("capture failed to terminate process tree: {job_err}"),
                logs_base_dir,
            );
            let root_result =
                unsafe { windows_sys::Win32::System::Threading::TerminateProcess(pi.hProcess, 1) };
            if root_result == 0 {
                log_note(
                    &format!("capture failed to terminate root process: {}", unsafe {
                        GetLastError()
                    }),
                    logs_base_dir,
                );
            }
        }
    } else if let Err(err) = job.preserve_descendants() {
        log_note(
            &format!("capture failed to preserve descendants after root exit: {err}"),
            logs_base_dir,
        );
    }

    if let Some(sink) = job_sink {
        sink.lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|registered| !Arc::ptr_eq(registered, &job));
    }

    unsafe {
        if pi.hThread != 0 {
            CloseHandle(pi.hThread);
        }
        if pi.hProcess != 0 {
            CloseHandle(pi.hProcess);
        }
        CloseHandle(security.h_token);
    }
    let stdout_result = finish_capture_reader(t_out, "stdout");
    let stderr_result = finish_capture_reader(t_err, "stderr");
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    let exit_code = if timed_out {
        128 + 64
    } else {
        exit_code_u32 as i32
    };

    if exit_code == 0 {
        log_success(&command, logs_base_dir);
    } else {
        log_failure(&command, &format!("exit code {exit_code}"), logs_base_dir);
    }

    Ok(CaptureResult {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

pub fn run_windows_sandbox_legacy_preflight(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    codex_home: &Path,
    cwd: &Path,
    env_map: &HashMap<String, String>,
) -> Result<()> {
    let Ok(permissions) = crate::resolved_permissions::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
        permission_profile,
        workspace_roots,
    ) else {
        return Ok(());
    };
    if !permissions.uses_write_capabilities_for_cwd(cwd, env_map) {
        return Ok(());
    }

    ensure_nano_home_exists(codex_home)?;
    let current_dir = cwd.to_path_buf();
    let capability_roots =
        legacy_session_capability_roots(&permissions, &current_dir, env_map, codex_home);
    // The legacy path grants the real user DELETE on existing descendants; the
    // grant must be verified against the user's own effective token, so build
    // the full session security (token included) instead of bare SIDs.
    let security = prepare_legacy_session_security(
        /*uses_write_capabilities*/ true,
        codex_home,
        cwd,
        capability_roots,
    )?;
    let result = apply_legacy_session_acl_rules(
        &permissions,
        codex_home,
        &current_dir,
        env_map,
        &[],
        &[],
        LegacyAclSids {
            readonly_sid: None,
            readonly_sid_str: None,
            write_root_sids: &security.write_root_sids,
            legacy_user_sid: security.legacy_user_sid_ptr(),
            effective_token: Some(security.h_token),
        },
    );
    unsafe { CloseHandle(security.h_token) };
    result?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::resolved_permissions::ResolvedWindowsSandboxPermissions;
    use nano_core::permissions::NetworkSandboxPolicy;
    use nano_core::permissions::PermissionProfile;
    use std::collections::HashMap;
    use std::path::Path;

    fn workspace_profile(network_policy: NetworkSandboxPolicy) -> PermissionProfile {
        PermissionProfile::workspace_write_with(
            &[],
            network_policy,
            /*exclude_tmpdir_env_var*/ false,
            /*exclude_slash_tmp*/ false,
        )
    }

    fn should_apply_network_block(permission_profile: &PermissionProfile) -> bool {
        ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            &[],
        )
        .expect("managed permissions")
        .should_apply_network_block()
    }

    #[test]
    fn applies_network_block_when_access_is_disabled() {
        assert!(should_apply_network_block(&workspace_profile(
            NetworkSandboxPolicy::Restricted
        )));
    }

    #[test]
    fn skips_network_block_when_access_is_allowed() {
        assert!(!should_apply_network_block(&workspace_profile(
            NetworkSandboxPolicy::Enabled
        )));
    }

    #[test]
    fn applies_network_block_for_read_only() {
        assert!(should_apply_network_block(&PermissionProfile::read_only()));
    }

    #[test]
    fn legacy_preflight_skips_profiles_without_managed_filesystem_permissions() {
        for permission_profile in [
            PermissionProfile::Disabled,
            PermissionProfile::External {
                network: NetworkSandboxPolicy::Restricted,
            },
        ] {
            crate::capture::run_windows_sandbox_legacy_preflight(
                &permission_profile,
                &[],
                Path::new("."),
                Path::new("."),
                &HashMap::new(),
            )
            .expect("unsupported profiles do not need ACL preflight");
        }
    }

    #[test]
    fn legacy_preflight_provisions_workspace_write_with_effective_token() {
        // Track A port: the preflight must build the session token and verify
        // the legacy DELETE grant against it, on a per-test quiescent tree.
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let nano_home = temp.path().join("home");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::write(workspace.join("fixture.txt"), "fixture").expect("seed fixture");
        let env = HashMap::from([
            ("TEMP".to_string(), workspace.to_string_lossy().into_owned()),
            ("TMP".to_string(), workspace.to_string_lossy().into_owned()),
        ]);

        super::run_windows_sandbox_legacy_preflight(
            &workspace_profile(NetworkSandboxPolicy::Restricted),
            &[],
            &nano_home,
            &workspace,
            &env,
        )
        .expect("workspace-write legacy preflight");
    }
}

#[cfg(test)]
mod nano_tests {
    //! Track-B exercise tests (not from the donor): the ported legacy capture
    //! backend end-to-end — the direct equivalent of Track A's baseline
    //! failure #3 ("legacy capture timed out twice").
    use super::*;
    use std::fs;

    fn fixture_dirs(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let tmp =
            std::env::temp_dir().join(format!("wayland-nano-cap-{tag}-{}", std::process::id()));
        let workspace = tmp.join("workspace");
        let nano_home = tmp.join("nano-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&nano_home).unwrap();
        (workspace, nano_home)
    }

    /// Per-test TEMP/TMP so `scope_temp_env` derives a per-test scoped temp
    /// root. The transactional ACL traversal requires a quiescent tree: with
    /// the process default, every capture/legacy test in every concurrent
    /// `cargo test` process would mutate the same fixed
    /// `%TEMP%\wayland-nano-temp` tree and the (correct) fail-closed
    /// verification would trip on the cross-test race.
    fn fixture_temp_env(fixture_tmp: &Path) -> HashMap<String, String> {
        HashMap::from([
            (
                "TEMP".to_string(),
                fixture_tmp.to_string_lossy().into_owned(),
            ),
            (
                "TMP".to_string(),
                fixture_tmp.to_string_lossy().into_owned(),
            ),
        ])
    }

    #[test]
    fn workspace_write_capture_echoes_and_exits_without_timeout() {
        let (workspace, nano_home) = fixture_dirs("echo");
        let roots = [AbsolutePathBuf::from_absolute_path(&workspace).unwrap()];
        let result = run_windows_sandbox_capture(
            &PermissionProfile::workspace_write(),
            &roots,
            &nano_home,
            vec![
                "cmd.exe".into(),
                "/c".into(),
                "echo wayland-nano-capture".into(),
            ],
            &workspace,
            fixture_temp_env(workspace.parent().unwrap()),
            Some(20_000), // 20s bound: must not come anywhere near a timeout
            None,
            false,
        )
        .expect("capture spawn");

        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out, "capture must not time out");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("wayland-nano-capture"), "stdout: {stdout}");
        let _ = fs::remove_dir_all(workspace.parent().unwrap());
    }

    #[test]
    fn workspace_write_capture_denies_outside_root_write() {
        let (workspace, nano_home) = fixture_dirs("deny");
        let outside = workspace.parent().unwrap().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("denied.txt");
        let roots = [AbsolutePathBuf::from_absolute_path(&workspace).unwrap()];

        let result = run_windows_sandbox_capture(
            &PermissionProfile::workspace_write(),
            &roots,
            &nano_home,
            vec![
                "cmd.exe".into(),
                "/c".into(),
                format!("echo data > \"{}\"", target.display()),
            ],
            &workspace,
            fixture_temp_env(workspace.parent().unwrap()),
            Some(20_000),
            None,
            false,
        )
        .expect("capture spawn");

        assert!(!result.timed_out);
        assert!(
            !target.exists(),
            "capture path must enforce workspace write bounds"
        );
        let _ = fs::remove_dir_all(workspace.parent().unwrap());
    }

    #[test]
    fn capture_cancellation_terminates_fast() {
        let (workspace, nano_home) = fixture_dirs("cancel");
        let roots = [AbsolutePathBuf::from_absolute_path(&workspace).unwrap()];
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cancelled2 = std::sync::Arc::clone(&cancelled);
        let token = crate::WindowsSandboxCancellationToken::new(move || {
            cancelled2.load(std::sync::atomic::Ordering::SeqCst)
        });

        let cancel_thread = {
            let cancelled = std::sync::Arc::clone(&cancelled);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };

        let started = std::time::Instant::now();
        let result = run_windows_sandbox_capture(
            &PermissionProfile::workspace_write(),
            &roots,
            &nano_home,
            vec![
                "cmd.exe".into(),
                "/c".into(),
                "ping -t 127.0.0.1 > NUL".into(),
            ],
            &workspace,
            fixture_temp_env(workspace.parent().unwrap()),
            None, // no timeout: cancellation must be what ends it
            Some(token),
            false,
        )
        .expect("capture spawn");
        let elapsed = started.elapsed();
        cancel_thread.join().unwrap();

        // NOTE: elapsed includes spawn prep (token + ACL session security),
        // which dominates on this host (~30s); the cancellation itself fires
        // at 1.5s into an infinite child. Track A's baseline failure was a
        // TIMEOUT, i.e. the wait never ended — our bound only guards that.
        assert!(
            elapsed < std::time::Duration::from_secs(120),
            "cancellation path hung ({elapsed:?}) — prep should dominate, not the wait"
        );
        assert!(!result.timed_out);
        assert_ne!(
            result.exit_code, 0,
            "cancelled capture must not exit cleanly"
        );
        let _ = fs::remove_dir_all(workspace.parent().unwrap());
    }
}
