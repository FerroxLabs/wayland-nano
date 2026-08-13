//! Contained Windows ConPTY process creation.
//!
//! Provenance: adapted from Codex
//! `codex-rs/windows-sandbox-rs/src/conpty/mod.rs` @ 646f7c0a. The donor's
//! uncontained `create_conpty` entry point is intentionally omitted. Its
//! `JobObject::create()` call is replaced with `create_without_breakaway()`;
//! fixed dimensions are 120x30; and `codex_utils_pty::RawConPty` is reduced to
//! the equivalent windows-sys pipe and pseudoconsole ownership in this file.

use crate::desktop::LaunchDesktop;
use crate::job::JobObject;
use crate::proc_thread_attr::ProcThreadAttributeList;
use crate::winutil::{format_last_error, quote_windows_arg, to_wide};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::process::make_env_block;

const CONPTY_COLS: i16 = 120;
const CONPTY_ROWS: i16 = 30;

fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: callers pass a newly-created, non-null handle and transfer its
    // ownership exactly once.
    unsafe { OwnedHandle::from_raw_handle(handle as _) }
}

/// Owns a pseudoconsole, its host-side pipes, and the no-breakaway job that
/// contains the spawned process and its direct descendants.
pub struct ConptyInstance {
    pseudoconsole: Arc<Pseudoconsole>,
    input_write: Option<OwnedHandle>,
    output_read: Option<OwnedHandle>,
    job: Arc<JobObject>,
    _desktop: LaunchDesktop,
}

struct Pseudoconsole {
    handle: HPCON,
    closed: AtomicBool,
}

unsafe impl Send for Pseudoconsole {}
unsafe impl Sync for Pseudoconsole {}

impl Pseudoconsole {
    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            unsafe { ClosePseudoConsole(self.handle) };
        }
    }
}

impl Drop for Pseudoconsole {
    fn drop(&mut self) {
        self.close();
    }
}

/// Cloneable completion side of a ConPTY. The process waiter closes the
/// pseudoconsole exactly once on exit so the drain reader observes EOF.
pub struct ConptyCompletionHandle(Arc<Pseudoconsole>);

impl ConptyCompletionHandle {
    pub fn close(self) {
        self.0.close();
    }
}

impl ConptyInstance {
    pub fn take_input_write(&mut self) -> Option<OwnedHandle> {
        self.input_write.take()
    }

    pub fn take_output_read(&mut self) -> Option<OwnedHandle> {
        self.output_read.take()
    }

    pub fn job(&self) -> Arc<JobObject> {
        Arc::clone(&self.job)
    }

    pub fn completion_handle(&self) -> ConptyCompletionHandle {
        ConptyCompletionHandle(Arc::clone(&self.pseudoconsole))
    }
}

struct RawConpty {
    pseudoconsole: HPCON,
    input_write: OwnedHandle,
    output_read: OwnedHandle,
}

impl RawConpty {
    fn new() -> Result<Self> {
        let mut input_read = 0;
        let mut input_write = 0;
        let mut output_read = 0;
        let mut output_write = 0;
        if unsafe { CreatePipe(&mut input_read, &mut input_write, std::ptr::null(), 0) } == 0 {
            return Err(std::io::Error::last_os_error()).context("create ConPTY input pipe");
        }
        let input_read = owned(input_read);
        let input_write = owned(input_write);
        if unsafe { CreatePipe(&mut output_read, &mut output_write, std::ptr::null(), 0) } == 0 {
            return Err(std::io::Error::last_os_error()).context("create ConPTY output pipe");
        }
        let output_read = owned(output_read);
        let output_write = owned(output_write);

        let mut pseudoconsole = 0;
        let result = unsafe {
            CreatePseudoConsole(
                COORD {
                    X: CONPTY_COLS,
                    Y: CONPTY_ROWS,
                },
                input_read.as_raw_handle() as HANDLE,
                output_write.as_raw_handle() as HANDLE,
                0,
                &mut pseudoconsole,
            )
        };
        if result < 0 {
            return Err(std::io::Error::from_raw_os_error(result))
                .context("create 120x30 pseudoconsole");
        }
        // CreatePseudoConsole retains the server ends. The host owns only its
        // writer and reader after construction.
        drop(input_read);
        drop(output_write);
        Ok(Self {
            pseudoconsole,
            input_write,
            output_read,
        })
    }
}

impl Drop for RawConpty {
    fn drop(&mut self) {
        if self.pseudoconsole != 0 {
            unsafe { ClosePseudoConsole(self.pseudoconsole) };
        }
    }
}

/// Spawn `argv` under `h_token`, atomically assigning the process to the
/// pseudoconsole and a no-breakaway kill-on-close job before it can run.
///
/// # Safety
/// `h_token` must be a valid primary token handle with process-creation access.
pub unsafe fn spawn_conpty_process_as_user(
    h_token: HANDLE,
    argv: &[String],
    cwd: &Path,
    env_map: &HashMap<String, String>,
    use_private_desktop: bool,
    logs_base_dir: Option<&Path>,
) -> Result<(PROCESS_INFORMATION, ConptyInstance)> {
    unsafe {
        spawn_conpty_process_as_user_inner(
            h_token,
            argv,
            cwd,
            env_map,
            use_private_desktop,
            logs_base_dir,
            false,
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn spawn_conpty_process_as_user_inner(
    h_token: HANDLE,
    argv: &[String],
    cwd: &Path,
    env_map: &HashMap<String, String>,
    use_private_desktop: bool,
    logs_base_dir: Option<&Path>,
    inject_pre_creation_failure: bool,
) -> Result<(PROCESS_INFORMATION, ConptyInstance)> {
    let cmdline_str = argv
        .iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let mut cmdline = to_wide(&cmdline_str);
    let env_block = make_env_block(env_map);
    let desktop = LaunchDesktop::prepare(use_private_desktop, logs_base_dir)?;
    let job =
        Arc::new(JobObject::create_without_breakaway().context("create no-breakaway PTY job")?);
    let raw = RawConpty::new()?;

    let mut attrs = ProcThreadAttributeList::new(2)?;
    attrs.set_pseudoconsole(raw.pseudoconsole)?;
    attrs.set_job(job.as_raw_handle() as HANDLE)?;

    let mut startup: STARTUPINFOEXW = std::mem::zeroed();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.lpDesktop = desktop.startup_info_desktop();
    startup.lpAttributeList = attrs.as_mut_ptr();

    if inject_pre_creation_failure {
        anyhow::bail!("injected failure after ConPTY/job attributes, before process creation");
    }

    let cwd_wide = to_wide(cwd);
    let mut process_info: PROCESS_INFORMATION = std::mem::zeroed();
    let created = CreateProcessAsUserW(
        h_token,
        std::ptr::null(),
        cmdline.as_mut_ptr(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        0,
        EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
        env_block.as_ptr() as *mut c_void,
        cwd_wide.as_ptr(),
        &startup.StartupInfo,
        &mut process_info,
    );
    if created == 0 {
        let error = GetLastError() as i32;
        let message = format!(
            "CreateProcessAsUserW failed: {error} ({}) | cwd={} | env_u16_len={}",
            format_last_error(error),
            cwd.display(),
            env_block.len()
        );
        return Err(std::io::Error::from_raw_os_error(error)).context(message);
    }
    CloseHandle(process_info.hThread);

    let raw = std::mem::ManuallyDrop::new(raw);
    let instance = ConptyInstance {
        pseudoconsole: Arc::new(Pseudoconsole {
            handle: raw.pseudoconsole,
            closed: AtomicBool::new(false),
        }),
        input_write: Some(std::ptr::read(&raw.input_write)),
        output_read: Some(std::ptr::read(&raw.output_read)),
        job,
        _desktop: desktop,
    };
    Ok((process_info, instance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_failure_injection_never_runs_process() {
        let workspace = tempfile::tempdir().unwrap();
        let canary = workspace.path().join("must-not-exist");
        let command = format!("echo ran>{}", canary.display());
        let argv = vec!["cmd.exe".into(), "/d".into(), "/c".into(), command];
        let token = unsafe { crate::token::get_current_token_for_restriction() }.unwrap();
        let result = unsafe {
            spawn_conpty_process_as_user_inner(
                token,
                &argv,
                workspace.path(),
                &std::env::vars().collect(),
                false,
                None,
                true,
            )
        };
        unsafe { CloseHandle(token) };
        assert!(result.is_err());
        assert!(!canary.exists(), "process ran despite pre-creation failure");
    }
}
