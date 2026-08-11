//! Legacy backend: direct restricted-token spawn adapted into a live session
//! (pipes path only; the ConPTY/tty path is deferred — D8).
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/unified_exec/backends/legacy.rs`
//! @ 646f7c0a. Transformations: pty ProcessDriver/broadcast web ->
//! crate::spawn_types (mpsc); tty branch removed (fail-closed at router);
//! codex_home -> nano_home; codex_protocol/codex_utils_absolute_path ->
//! nano_core.

use crate::job::JobObject;
use crate::logging::log_failure;
use crate::logging::log_note;
use crate::logging::log_success;
use crate::process::ConsoleMode;
use crate::process::StderrMode;
use crate::process::StdinMode;
use crate::process::read_handle_loop;
use crate::process::spawn_process_with_pipes;
use crate::spawn_prep::LegacyAclSids;
use crate::spawn_prep::SpawnPrepOptions;
use crate::spawn_prep::allow_null_device_for_workspace_write;
use crate::spawn_prep::apply_legacy_session_acl_rules;
use crate::spawn_prep::legacy_session_capability_roots;
use crate::spawn_prep::prepare_legacy_session_security;
use crate::spawn_prep::prepare_legacy_spawn_context;
use crate::spawn_types::SandboxSessionHandle;
use crate::spawn_types::SpawnedProcess;
use anyhow::Result;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::PermissionProfile;
use std::collections::HashMap;
use std::path::Path;
use std::ptr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::System::Threading::PROCESS_INFORMATION;
use windows_sys::Win32::System::Threading::TerminateProcess;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const WAIT_TIMEOUT: u32 = 0x0000_0102;

struct LegacyProcessHandles {
    process: PROCESS_INFORMATION,
    job: Arc<JobObject>,
    output_join: std::thread::JoinHandle<()>,
    writer_handle: tokio::task::JoinHandle<()>,
    desktop: Option<crate::desktop::LaunchDesktop>,
}

#[allow(clippy::too_many_arguments)]
fn spawn_legacy_process(
    h_token: HANDLE,
    command: &[String],
    cwd: &Path,
    env_map: &HashMap<String, String>,
    use_private_desktop: bool,
    stdin_open: bool,
    stdout_tx: mpsc::Sender<Vec<u8>>,
    stderr_tx: mpsc::Sender<Vec<u8>>,
    writer_rx: mpsc::Receiver<Vec<u8>>,
    logs_base_dir: Option<&Path>,
) -> Result<LegacyProcessHandles> {
    let pipe_handles = spawn_process_with_pipes(
        h_token,
        command,
        cwd,
        env_map,
        if stdin_open {
            StdinMode::Open
        } else {
            StdinMode::Closed
        },
        StderrMode::Separate,
        ConsoleMode::Inherit,
        use_private_desktop,
        logs_base_dir,
    )?;
    let stdout_join = spawn_output_reader(pipe_handles.stdout_read, stdout_tx);
    let Some(stderr_read) = pipe_handles.stderr_read else {
        anyhow::bail!("separate stderr handle should be present");
    };
    let stderr_join = spawn_output_reader(stderr_read, stderr_tx);
    let output_join = std::thread::spawn(move || {
        let _ = stdout_join.join();
        let _ = stderr_join.join();
    });
    let writer_handle = spawn_input_writer(pipe_handles.stdin_write, writer_rx);
    Ok(LegacyProcessHandles {
        process: pipe_handles.process,
        job: pipe_handles.job(),
        output_join,
        writer_handle,
        desktop: Some(pipe_handles.desktop),
    })
}

fn spawn_output_reader(
    output_read: HANDLE,
    output_tx: mpsc::Sender<Vec<u8>>,
) -> std::thread::JoinHandle<()> {
    read_handle_loop(output_read, move |chunk| {
        let _ = output_tx.blocking_send(chunk.to_vec());
    })
}

fn spawn_input_writer(
    input_write: Option<HANDLE>,
    mut writer_rx: mpsc::Receiver<Vec<u8>>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        while let Some(bytes) = writer_rx.blocking_recv() {
            let Some(handle) = input_write else {
                continue;
            };
            if write_all_handle(handle, &bytes).is_err() {
                break;
            }
        }
        if let Some(handle) = input_write {
            unsafe {
                CloseHandle(handle);
            }
        }
    })
}

fn terminate_job_or_process(
    job: &JobObject,
    process_handle: &Arc<StdMutex<Option<HANDLE>>>,
    logs_base_dir: Option<&Path>,
) {
    if let Err(job_err) = job.terminate() {
        log_note(
            &format!("legacy spawn failed to terminate process tree: {job_err}"),
            logs_base_dir,
        );
        if let Ok(guard) = process_handle.lock()
            && let Some(handle) = guard.as_ref()
            && unsafe { TerminateProcess(*handle, 1) } == 0
        {
            log_note(
                &format!(
                    "legacy spawn failed to terminate root process: {}",
                    unsafe { GetLastError() }
                ),
                logs_base_dir,
            );
        }
    }
}

fn write_all_handle(handle: HANDLE, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                handle,
                bytes.as_ptr() as *const _,
                bytes.len() as u32,
                &mut written,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() } as i32;
            return Err(anyhow::anyhow!("WriteFile failed: {err}"));
        }
        if written == 0 {
            anyhow::bail!("WriteFile returned success but wrote 0 bytes");
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn finalize_exit(
    exit_tx: oneshot::Sender<i32>,
    process_handle: Arc<StdMutex<Option<HANDLE>>>,
    thread_handle: HANDLE,
    output_join: std::thread::JoinHandle<()>,
    logs_base_dir: Option<&Path>,
    command: Vec<String>,
) {
    let exit_code = {
        let mut raw_exit = 1u32;
        if let Ok(guard) = process_handle.lock()
            && let Some(handle) = guard.as_ref()
        {
            unsafe {
                WaitForSingleObject(*handle, INFINITE);
                GetExitCodeProcess(*handle, &mut raw_exit);
            }
        }
        raw_exit as i32
    };

    let _ = output_join.join();
    let _ = exit_tx.send(exit_code);

    unsafe {
        if thread_handle != 0 && thread_handle != INVALID_HANDLE_VALUE {
            CloseHandle(thread_handle);
        }
        if let Ok(mut guard) = process_handle.lock()
            && let Some(handle) = guard.take()
        {
            CloseHandle(handle);
        }
    }

    if exit_code == 0 {
        log_success(&command, logs_base_dir);
    } else {
        log_failure(&command, &format!("exit code {exit_code}"), logs_base_dir);
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn spawn_windows_sandbox_session_legacy(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    nano_home: &Path,
    command: Vec<String>,
    cwd: &Path,
    mut env_map: HashMap<String, String>,
    timeout_ms: Option<u64>,
    additional_deny_read_paths: &[AbsolutePathBuf],
    additional_deny_write_paths: &[AbsolutePathBuf],
    stdin_open: bool,
    use_private_desktop: bool,
) -> Result<SpawnedProcess> {
    let common = prepare_legacy_spawn_context(
        permission_profile,
        workspace_roots,
        nano_home,
        cwd,
        &mut env_map,
        &command,
        SpawnPrepOptions {
            inherit_path: false,
            add_git_safe_directory: false,
        },
    )?;
    if !common.permissions.has_full_disk_read_access() {
        anyhow::bail!("Restricted read-only access requires the elevated Windows sandbox backend");
    }
    // WRITE_RESTRICTED tokens consult restricting SIDs only for writes, so this
    // backend cannot make capability-SID deny-read ACLs authoritative.
    if !additional_deny_read_paths.is_empty() {
        anyhow::bail!("deny-read overrides require the elevated Windows sandbox backend");
    }
    let additional_deny_write_paths = additional_deny_write_paths
        .iter()
        .map(AbsolutePathBuf::to_path_buf)
        .collect::<Vec<_>>();
    let capability_roots = legacy_session_capability_roots(
        &common.permissions,
        &common.current_dir,
        &env_map,
        nano_home,
    );
    let security = prepare_legacy_session_security(
        common.uses_write_capabilities,
        nano_home,
        cwd,
        capability_roots,
    )?;
    allow_null_device_for_workspace_write(common.uses_write_capabilities);

    apply_legacy_session_acl_rules(
        &common.permissions,
        nano_home,
        &common.current_dir,
        &env_map,
        &[],
        &additional_deny_write_paths,
        LegacyAclSids {
            readonly_sid: security.readonly_sid.as_ref(),
            readonly_sid_str: security.readonly_sid_str.as_deref(),
            write_root_sids: &security.write_root_sids,
        },
    )?;

    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(128);
    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
    let (stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>(256);
    let (exit_tx, exit_rx) = oneshot::channel::<i32>();

    let LegacyProcessHandles {
        process: pi,
        job,
        output_join,
        writer_handle: _writer_handle,
        desktop,
    } = match spawn_legacy_process(
        security.h_token,
        &command,
        cwd,
        &env_map,
        use_private_desktop,
        stdin_open,
        stdout_tx,
        stderr_tx,
        writer_rx,
        common.logs_base_dir.as_deref(),
    ) {
        Ok(handles) => handles,
        Err(err) => {
            unsafe {
                CloseHandle(security.h_token);
            }
            return Err(err);
        }
    };

    let process_handle = Arc::new(StdMutex::new(Some(pi.hProcess)));
    let wait_handle = Arc::clone(&process_handle);
    let job_for_wait = Arc::clone(&job);
    let command_for_wait = command.clone();
    let wait_logs_base_dir = common.logs_base_dir.clone();
    let token_handle = security.h_token;
    std::thread::spawn(move || {
        let _desktop = desktop;
        let timeout = timeout_ms.map(|ms| ms as u32).unwrap_or(INFINITE);
        let wait_res = unsafe { WaitForSingleObject(pi.hProcess, timeout) };
        if wait_res == WAIT_TIMEOUT {
            terminate_job_or_process(&job_for_wait, &wait_handle, wait_logs_base_dir.as_deref());
        } else if let Err(err) = job_for_wait.preserve_descendants() {
            log_note(
                &format!("legacy spawn failed to preserve descendants after root exit: {err}"),
                wait_logs_base_dir.as_deref(),
            );
        }
        unsafe {
            if token_handle != 0 && token_handle != INVALID_HANDLE_VALUE {
                CloseHandle(token_handle);
            }
        }
        finalize_exit(
            exit_tx,
            wait_handle,
            pi.hThread,
            output_join,
            wait_logs_base_dir.as_deref(),
            command_for_wait,
        );
    });

    let terminator = {
        let job = Arc::clone(&job);
        let process_handle = Arc::clone(&process_handle);
        let logs_base_dir = common.logs_base_dir;
        move || terminate_job_or_process(&job, &process_handle, logs_base_dir.as_deref())
    };

    Ok(SpawnedProcess {
        session: SandboxSessionHandle::new(writer_tx, terminator),
        stdout_rx,
        stderr_rx,
        exit_rx,
    })
}

#[cfg(test)]
mod nano_tests {
    //! Track-B exercise tests (not from the donor): end-to-end spawns through
    //! the full ported stack — policy -> token -> restricted spawn -> session.
    use super::*;
    use std::fs;

    async fn spawn_and_collect(
        workspace: &Path,
        nano_home: &Path,
        command: Vec<String>,
        cwd: &Path,
    ) -> (String, String, i32) {
        let profile = PermissionProfile::workspace_write();
        let roots = [AbsolutePathBuf::from_absolute_path(workspace).unwrap()];
        let spawned = spawn_windows_sandbox_session_legacy(
            &profile,
            &roots,
            nano_home,
            command,
            cwd,
            HashMap::new(),
            None,
            &[],
            &[],
            true,
            false,
        )
        .await
        .expect("legacy spawn");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut stdout_rx = spawned.stdout_rx;
        let mut stderr_rx = spawned.stderr_rx;
        let exit = spawned.exit_rx;
        while let Some(chunk) = stdout_rx.recv().await {
            out.extend_from_slice(&chunk);
        }
        while let Some(chunk) = stderr_rx.recv().await {
            err.extend_from_slice(&chunk);
        }
        let code = exit.await.expect("exit code");
        (
            String::from_utf8_lossy(&out).to_string(),
            String::from_utf8_lossy(&err).to_string(),
            code,
        )
    }

    #[tokio::test]
    async fn workspace_write_spawn_echoes_and_exits_clean() {
        let tmp = std::env::temp_dir().join(format!("wayland-nano-ux-{}", std::process::id()));
        let workspace = tmp.join("workspace");
        let nano_home = tmp.join("nano-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&nano_home).unwrap();

        let (out, _err, code) = spawn_and_collect(
            &workspace,
            &nano_home,
            vec![
                "cmd.exe".into(),
                "/c".into(),
                "echo wayland-nano-hello".into(),
            ],
            &workspace,
        )
        .await;

        assert!(
            out.contains("wayland-nano-hello"),
            "stdout must echo: {out}"
        );
        assert_eq!(code, 0);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn workspace_write_allows_write_inside_root() {
        let tmp = std::env::temp_dir().join(format!("wayland-nano-uxw-{}", std::process::id()));
        let workspace = tmp.join("workspace");
        let nano_home = tmp.join("nano-home");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&nano_home).unwrap();

        let (_out, _err, code) = spawn_and_collect(
            &workspace,
            &nano_home,
            vec![
                "cmd.exe".into(),
                "/c".into(),
                "echo data > allowed.txt".into(),
            ],
            &workspace,
        )
        .await;

        assert_eq!(code, 0, "write inside workspace must succeed");
        assert!(workspace.join("allowed.txt").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn workspace_write_denies_write_outside_root() {
        let tmp = std::env::temp_dir().join(format!("wayland-nano-uxd-{}", std::process::id()));
        let workspace = tmp.join("workspace");
        let nano_home = tmp.join("nano-home");
        let outside = tmp.join("outside");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&nano_home).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("denied.txt");

        let (_out, _err, _code) = spawn_and_collect(
            &workspace,
            &nano_home,
            vec![
                "cmd.exe".into(),
                "/c".into(),
                format!("echo data > \"{}\"", target.display()),
            ],
            &workspace,
        )
        .await;

        assert!(
            !target.exists(),
            "restricted token must not write outside the workspace root"
        );
        let _ = fs::remove_dir_all(&tmp);
    }
}
