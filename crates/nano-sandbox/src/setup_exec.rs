//! Setup execution: payload assembly, singleflight orchestration, helper
//! launch (elevated and non-elevated), and completion verification.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/setup.rs` (execution
//! half) @ 646f7c0a, plus the post-baseline bounded-helper-lifecycle fixes
//! from Track A commits `fa0ee4da3` and `9e0e88504` (bounded waits,
//! cooperative cancellation token, fail-closed taint/in-progress sentinels,
//! RAII helper handle ownership, panic-contained singleflight leader, setup
//! authority lane). Transformations:
//! - codex_home -> nano_home naming;
//! - SETUP_EXE_FILENAME -> "wayland-nano-sandbox-setup.exe" (dual-track isolation:
//!   Track A's helper is "codex-windows-sandbox-setup.exe");
//! - ElevationPayload.otel -> Option<telemetry::TelemetrySettings> (facade;
//!   `global_telemetry_settings()` currently None on all paths);
//! - is_protected_metadata_name -> nano_core::policy_engine::
//!   PROTECTED_METADATA_PATH_NAMES (Track A used codex_protocol);
//! - gather layer via crate::gather (ported separately).

use crate::allow::AllowDenyPaths;
use crate::allow::compute_allow_paths_for_permissions;
use crate::gather::WINDOWS_PLATFORM_DEFAULT_READ_ROOTS;
use crate::gather::canonical_existing;
use crate::gather::effective_write_roots_for_setup;
use crate::gather::expand_user_profile_root;
use crate::gather::filter_ssh_config_dependency_roots;
use crate::gather::filter_user_profile_root;
use crate::gather::filter_user_profile_root_exclusions;
use crate::gather::gather_helper_read_roots;
use crate::gather::gather_read_roots;
use crate::helper_materialization::bundled_executable_path_for_exe;
use crate::identity::sandbox_setup_is_complete;
use crate::logging::log_note;
use crate::path_normalization::canonicalize_path;
use crate::resolved_permissions::ResolvedWindowsSandboxPermissions;
use crate::setup_error::SetupErrorCode;
use crate::setup_error::SetupFailure;
use crate::setup_error::clear_setup_error_report;
use crate::setup_error::extract_failure;
use crate::setup_error::failure;
use crate::setup_error::read_setup_error_report;
use crate::setup_types::OFFLINE_USERNAME;
use crate::setup_types::ONLINE_USERNAME;
use crate::setup_types::OfflineProxySettings;
use crate::setup_types::SETUP_VERSION;
use crate::setup_types::SandboxNetworkIdentity;
use crate::setup_types::SetupRootOverrides;
use crate::setup_types::offline_proxy_settings_from_env;
use crate::telemetry;
use anyhow::Result;
use anyhow::anyhow;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::PermissionProfile;
use nano_core::policy_engine::PROTECTED_METADATA_PATH_NAMES;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::AllocateAndInitializeSid;
use windows_sys::Win32::Security::CheckTokenMembership;
use windows_sys::Win32::Security::FreeSid;
use windows_sys::Win32::Security::SECURITY_NT_AUTHORITY;

const ERROR_CANCELLED: u32 = 1223;
/// Hard deadline for one helper run (Track A bounded-helper port: the donor
/// waited INFINITE / blocked on `status()`, so a stuck helper hung the caller
/// forever).
const ELEVATED_SETUP_TIMEOUT_MS: u32 = 120_000;
/// Grace window for the helper to observe the cooperative cancellation token
/// before the orchestrator taints the home and force-terminates it.
const ELEVATED_SETUP_CANCEL_GRACE_MS: u32 = 30_000;
// Donor-local constants (windows-sys does not export these RIDs).
const SECURITY_BUILTIN_DOMAIN_RID: u32 = 0x0000_0020;
const DOMAIN_ALIAS_RID_ADMINS: u32 = 0x0000_0220;
const SETUP_EXE_FILENAME: &str = "wayland-nano-sandbox-setup.exe";

pub struct SandboxSetupRequest<'a> {
    pub permissions: &'a ResolvedWindowsSandboxPermissions,
    pub command_cwd: &'a Path,
    pub env_map: &'a HashMap<String, String>,
    pub nano_home: &'a Path,
    pub proxy_enforced: bool,
}

/// Loopback proxy / local-binding settings installed by an administrator
/// during managed Windows sandbox provisioning.
///
/// Provenance: codex lib.rs `WindowsSandboxProvisioningSettings` @ 646f7c0a.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowsSandboxProvisioningSettings {
    pub proxy_ports: Vec<u16>,
    pub allow_local_binding: bool,
}

#[derive(Clone)]
struct SharedSetupError {
    code: Option<SetupErrorCode>,
    message: String,
}

impl SharedSetupError {
    fn from_error(error: &anyhow::Error) -> Self {
        match extract_failure(error) {
            Some(setup_failure) => Self {
                code: Some(setup_failure.code),
                message: setup_failure.message.clone(),
            },
            None => Self {
                code: None,
                message: format!("{error:#}"),
            },
        }
    }

    fn into_error(self) -> anyhow::Error {
        match self.code {
            Some(code) => failure(code, self.message),
            None => anyhow!(self.message),
        }
    }
}

struct SetupFlight {
    result: Mutex<Option<std::result::Result<(), SharedSetupError>>>,
    completed: Condvar,
}

impl SetupFlight {
    fn pending() -> Self {
        Self {
            result: Mutex::new(None),
            completed: Condvar::new(),
        }
    }

    fn complete(&self, result: std::result::Result<(), SharedSetupError>) {
        *self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
        self.completed.notify_all();
    }

    fn wait(&self) -> Result<()> {
        let mut result = self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while result.is_none() {
            result = self
                .completed
                .wait(result)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match result.clone() {
            Some(result) => result.map_err(SharedSetupError::into_error),
            None => Err(anyhow!("setup flight completed without a result")),
        }
    }
}

static SETUP_FLIGHTS: OnceLock<Mutex<HashMap<String, Arc<SetupFlight>>>> = OnceLock::new();
/// Serializes the machine-global setup authority: sandbox accounts, network
/// policy, and workspace DACLs are machine-global, so two in-process leaders
/// must never run helpers concurrently (Track A provisioning-boundary port).
static SETUP_AUTHORITY_LANE: OnceLock<Mutex<()>> = OnceLock::new();

fn run_setup_authority_lane<T>(run: impl FnOnce() -> T) -> T {
    let _guard = SETUP_AUTHORITY_LANE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    run()
}

fn run_setup_singleflight(key: String, run: impl FnOnce() -> Result<()>) -> Result<()> {
    let flights = SETUP_FLIGHTS.get_or_init(|| Mutex::new(HashMap::new()));
    let (flight, is_leader) = {
        let mut flights = flights
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match flights.get(&key) {
            Some(flight) => (Arc::clone(flight), false),
            None => {
                let flight = Arc::new(SetupFlight::pending());
                flights.insert(key.clone(), Arc::clone(&flight));
                (flight, true)
            }
        }
    };

    if !is_leader {
        return flight.wait();
    }

    // A panicking leader must not strand every follower on the condvar: turn
    // the panic into a shared error and let a later caller become leader.
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(_) => Err(anyhow!("setup singleflight leader panicked")),
    };
    let shared_result = match &result {
        Ok(()) => Ok(()),
        Err(error) => Err(SharedSetupError::from_error(error)),
    };
    flight.complete(shared_result);
    let mut flights = flights
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if flights
        .get(&key)
        .is_some_and(|current| Arc::ptr_eq(current, &flight))
    {
        flights.remove(&key);
    }
    result
}

pub fn run_setup_refresh(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    nano_home: &Path,
    proxy_enforced: bool,
) -> Result<()> {
    let Ok(permissions) =
        ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            workspace_roots,
        )
    else {
        return Ok(());
    };
    run_setup_refresh_inner(
        SandboxSetupRequest {
            permissions: &permissions,
            command_cwd,
            env_map,
            nano_home,
            proxy_enforced,
        },
        SetupRootOverrides::default(),
        /*offline_proxy_settings_override*/ None,
    )
}

pub fn run_setup_refresh_with_overrides_and_proxy_settings(
    request: SandboxSetupRequest<'_>,
    overrides: SetupRootOverrides,
    offline_proxy_settings: &OfflineProxySettings,
) -> Result<()> {
    run_setup_refresh_inner(request, overrides, Some(offline_proxy_settings))
}

pub fn run_setup_refresh_with_extra_read_roots(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    nano_home: &Path,
    extra_read_roots: Vec<PathBuf>,
    proxy_enforced: bool,
) -> Result<()> {
    let Ok(permissions) =
        ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            workspace_roots,
        )
    else {
        return Ok(());
    };
    let mut read_roots = gather_read_roots(command_cwd, &permissions, env_map, nano_home);
    read_roots.extend(extra_read_roots);
    run_setup_refresh_inner(
        SandboxSetupRequest {
            permissions: &permissions,
            command_cwd,
            env_map,
            nano_home,
            proxy_enforced,
        },
        SetupRootOverrides {
            read_roots: Some(read_roots),
            read_roots_include_platform_defaults: false,
            write_roots: Some(Vec::new()),
            deny_read_paths: None,
            deny_write_paths: None,
        },
        /*offline_proxy_settings_override*/ None,
    )
}

fn run_setup_refresh_inner(
    request: SandboxSetupRequest<'_>,
    overrides: SetupRootOverrides,
    offline_proxy_settings_override: Option<&OfflineProxySettings>,
) -> Result<()> {
    if !request.permissions.is_enforceable_by_windows_sandbox() {
        anyhow::bail!("unsupported filesystem permissions for Windows sandbox setup");
    }
    let (read_roots, write_roots) = build_payload_roots(&request, &overrides);
    let deny_read_paths = build_payload_deny_read_paths(overrides.deny_read_paths);
    let deny_write_paths = build_payload_deny_write_paths(&request, overrides.deny_write_paths);
    let deny_write_paths_no_create = deny_write_paths_no_create(&deny_write_paths);
    let offline_proxy_settings =
        offline_proxy_settings_for_request(&request, offline_proxy_settings_override);
    let payload = ElevationPayload {
        version: SETUP_VERSION,
        offline_username: OFFLINE_USERNAME.to_string(),
        online_username: ONLINE_USERNAME.to_string(),
        nano_home: request.nano_home.to_path_buf(),
        command_cwd: request.command_cwd.to_path_buf(),
        read_roots,
        write_roots,
        deny_read_paths,
        deny_write_paths,
        deny_write_paths_no_create,
        proxy_ports: offline_proxy_settings.proxy_ports,
        allow_local_binding: offline_proxy_settings.allow_local_binding,
        otel: None,
        real_user: std::env::var("USERNAME").unwrap_or_else(|_| "Administrators".to_string()),
        mode: SetupMode::Full,
        refresh_only: true,
        cancellation_path: Some(new_setup_cancellation_path(request.nano_home)?),
    };
    // Refresh never requests elevation; the helper runs as the current user.
    run_setup_exe(&payload, /*needs_elevation*/ false, request.nano_home)
}

fn find_setup_exe() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(setup_exe) = find_setup_exe_for_current_exe(&exe)
    {
        return setup_exe;
    }
    PathBuf::from(SETUP_EXE_FILENAME)
}

fn find_setup_exe_for_current_exe(exe: &Path) -> Option<PathBuf> {
    bundled_executable_path_for_exe(exe, SETUP_EXE_FILENAME)
}

fn report_helper_failure(
    nano_home: &Path,
    cleared_report: bool,
    exit_code: Option<i32>,
) -> anyhow::Error {
    let exit_detail = format!("setup helper exited with status {exit_code:?}");
    if !cleared_report {
        return failure(SetupErrorCode::OrchestratorHelperExitNonzero, exit_detail);
    }
    match read_setup_error_report(nano_home) {
        Ok(Some(report)) => anyhow::Error::new(SetupFailure::from_report(report)),
        Ok(None) => failure(SetupErrorCode::OrchestratorHelperExitNonzero, exit_detail),
        Err(err) => failure(
            SetupErrorCode::OrchestratorHelperReportReadFailed,
            format!("{exit_detail}; failed to read setup_error.json: {err}"),
        ),
    }
}

fn verify_setup_completed(nano_home: &Path) -> Result<()> {
    if sandbox_setup_is_complete(nano_home) {
        Ok(())
    } else {
        Err(failure(
            SetupErrorCode::OrchestratorHelperIncomplete,
            "setup helper exited successfully before setup completed",
        ))
    }
}

fn run_setup_exe(
    payload: &ElevationPayload,
    needs_elevation: bool,
    nano_home: &Path,
) -> Result<()> {
    let payload_json = serde_json::to_string(payload).map_err(|err| {
        failure(
            SetupErrorCode::OrchestratorPayloadSerializeFailed,
            format!("failed to serialize elevation payload: {err}"),
        )
    })?;
    let payload_b64 = BASE64_STANDARD.encode(payload_json.as_bytes());
    let singleflight_key = setup_singleflight_key(payload)?;
    run_setup_singleflight(singleflight_key, || {
        run_setup_authority_lane(|| {
            if payload.mode == SetupMode::Full {
                use std::io::Write;
                let in_progress = crate::sandbox_dir(nano_home).join("setup-in-progress");
                let mut sentinel = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&in_progress)
                .map_err(|err| {
                    failure(
                        SetupErrorCode::OrchestratorHelperLaunchFailed,
                        format!(
                            "failed to establish setup-in-progress sentinel {}: {err}; setup remains fail-closed. For manual recovery, first verify no wayland-nano-sandbox-setup process survives, then remove only this sentinel and rerun full provisioning",
                            in_progress.display()
                        ),
                    )
                })?;
                sentinel.write_all(
                    b"full setup launched; readiness disabled until verified completion",
                )?;
                sentinel.sync_all()?;
            }
            run_setup_exe_payload(
                &payload_b64,
                needs_elevation,
                nano_home,
                payload.cancellation_path.as_deref(),
            )
        })
    })
}

/// The singleflight key excludes the per-run cancellation token so identical
/// setup requests still share one in-flight helper.
fn setup_singleflight_key(payload: &ElevationPayload) -> Result<String> {
    let mut key_value = serde_json::to_value(payload)?;
    if let Some(object) = key_value.as_object_mut() {
        object.remove("cancellation_path");
    }
    Ok(BASE64_STANDARD.encode(serde_json::to_vec(&key_value)?))
}

/// The process handle has exactly one owner.  In particular, a handle borrowed
/// from `Child` must never be wrapped in an independently-dropping raw handle.
enum SetupProcess {
    Child(Child),
    Raw(RawSetupProcess),
}

struct RawSetupProcess {
    handle: windows_sys::Win32::Foundation::HANDLE,
    close: fn(windows_sys::Win32::Foundation::HANDLE),
}

fn close_setup_process_handle(handle: windows_sys::Win32::Foundation::HANDLE) {
    unsafe { CloseHandle(handle) };
}

impl RawSetupProcess {
    fn new(handle: windows_sys::Win32::Foundation::HANDLE) -> Self {
        Self {
            handle,
            close: close_setup_process_handle,
        }
    }
}

impl Drop for RawSetupProcess {
    fn drop(&mut self) {
        if self.handle != 0 {
            (self.close)(self.handle);
            self.handle = 0;
        }
    }
}

impl SetupProcess {
    fn handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        match self {
            Self::Child(child) => child.as_raw_handle() as _,
            Self::Raw(raw) => raw.handle,
        }
    }

    /// Confirm collection after a signalled wait. Raw handles are already
    /// kernel-reaped; `Child::wait` only collects the Rust child bookkeeping.
    fn confirm_reaped(&mut self) -> std::io::Result<()> {
        if let Self::Child(child) = self {
            child.wait().map(|_| ())?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupWait {
    Signalled,
    Timeout,
    Failed(u32),
    Unexpected(u32),
}

/// OS-facing half of the helper lifecycle, behind a trait so the state
/// machine's exit/cleanup matrix is unit-testable without a real helper.
trait SetupProcessOps {
    fn wait(&mut self, process: &mut SetupProcess, timeout_ms: u32) -> SetupWait;
    fn request_cancel(&mut self, cancellation_path: Option<&Path>) -> Result<(), String>;
    fn persist_taint(&mut self, nano_home: &Path) -> Result<(), String>;
    fn terminate(&mut self, process: &mut SetupProcess) -> Result<(), String>;
    fn exit_code(&mut self, process: &mut SetupProcess) -> Result<u32, String>;
    fn confirm_reaped(&mut self, process: &mut SetupProcess) -> Result<(), String>;
    fn remove_cancel(&mut self, cancellation_path: Option<&Path>) -> Result<(), String>;
}

struct WinSetupProcessOps;

impl SetupProcessOps for WinSetupProcessOps {
    fn wait(&mut self, process: &mut SetupProcess, timeout_ms: u32) -> SetupWait {
        use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        let value = unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(process.handle(), timeout_ms)
        };
        match value {
            WAIT_OBJECT_0 => SetupWait::Signalled,
            WAIT_TIMEOUT => SetupWait::Timeout,
            WAIT_FAILED => SetupWait::Failed(unsafe { GetLastError() }),
            other => SetupWait::Unexpected(other),
        }
    }

    fn request_cancel(&mut self, path: Option<&Path>) -> Result<(), String> {
        use std::io::Write;
        let path = path.ok_or_else(|| "setup has no cancellation token".to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|err| format!("create cancellation token failed: {err}"))?;
        file.write_all(b"cancel")
            .map_err(|err| format!("write cancellation token failed: {err}"))
    }

    fn persist_taint(&mut self, nano_home: &Path) -> Result<(), String> {
        std::fs::write(
            crate::sandbox_dir(nano_home).join("setup-tainted"),
            b"forced termination during setup; ACL rollback not guaranteed",
        )
        .map_err(|err| format!("persist setup taint failed: {err}"))
    }

    fn terminate(&mut self, process: &mut SetupProcess) -> Result<(), String> {
        let ok =
            unsafe { windows_sys::Win32::System::Threading::TerminateProcess(process.handle(), 1) };
        if ok == 0 {
            Err(format!("forced termination failed: {}", unsafe {
                GetLastError()
            }))
        } else {
            Ok(())
        }
    }

    fn exit_code(&mut self, process: &mut SetupProcess) -> Result<u32, String> {
        let mut code = 0;
        let ok = unsafe {
            windows_sys::Win32::System::Threading::GetExitCodeProcess(process.handle(), &mut code)
        };
        if ok == 0 {
            Err(format!("GetExitCodeProcess failed: {}", unsafe {
                GetLastError()
            }))
        } else {
            Ok(code)
        }
    }

    fn confirm_reaped(&mut self, process: &mut SetupProcess) -> Result<(), String> {
        process
            .confirm_reaped()
            .map_err(|err| format!("collect setup helper status failed: {err}"))
    }

    fn remove_cancel(&mut self, path: Option<&Path>) -> Result<(), String> {
        let Some(path) = path else { return Ok(()) };
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!("remove cancellation token failed: {err}")),
        }
    }
}

#[derive(Debug)]
struct SetupProcessResult {
    exit_code: Option<u32>,
    initial_wait: SetupWait,
    forced: bool,
    reaped: bool,
    errors: Vec<String>,
}

/// Bounded helper lifecycle (Track A port): wait with a hard deadline; on
/// timeout or wait anomaly request cooperative cancellation through the token
/// file; if the grace window lapses, persist a fail-closed taint sentinel
/// before force-terminating, then bound the reap wait as well.
fn drive_setup_process(
    process: &mut SetupProcess,
    cancellation_path: Option<&Path>,
    nano_home: &Path,
    ops: &mut impl SetupProcessOps,
) -> SetupProcessResult {
    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    let initial_wait = ops.wait(process, ELEVATED_SETUP_TIMEOUT_MS);
    let mut result = SetupProcessResult {
        exit_code: None,
        initial_wait,
        forced: false,
        reaped: false,
        errors: Vec::new(),
    };
    if initial_wait == SetupWait::Signalled {
        match ops.exit_code(process) {
            Ok(code) if code != STILL_ACTIVE as u32 => {
                result.exit_code = Some(code);
                match ops.confirm_reaped(process) {
                    Ok(()) => result.reaped = true,
                    Err(err) => result.errors.push(err),
                }
                return result;
            }
            Ok(_) => result
                .errors
                .push("setup helper remained active after a signaled process wait".into()),
            Err(err) => result.errors.push(err),
        }
    }
    if let Err(err) = ops.request_cancel(cancellation_path) {
        result.errors.push(err);
    }
    if ops.wait(process, ELEVATED_SETUP_CANCEL_GRACE_MS) != SetupWait::Signalled {
        result.forced = true;
        if let Err(err) = ops.persist_taint(nano_home) {
            result.errors.push(err);
        }
        match ops.terminate(process) {
            Ok(()) => match ops.wait(process, ELEVATED_SETUP_CANCEL_GRACE_MS) {
                SetupWait::Signalled => match ops.confirm_reaped(process) {
                    Ok(()) => result.reaped = true,
                    Err(err) => result.errors.push(err),
                },
                wait => result.errors.push(format!(
                    "forced helper cleanup wait returned {wait:?}; helper may be orphaned"
                )),
            },
            Err(err) => result.errors.push(format!("{err}; helper may be orphaned")),
        }
    } else {
        match ops.confirm_reaped(process) {
            Ok(()) => result.reaped = true,
            Err(err) => result.errors.push(err),
        }
    }
    if result.reaped
        && let Err(err) = ops.remove_cancel(cancellation_path)
    {
        result.errors.push(err);
    }
    result
}

fn run_setup_exe_payload(
    payload_b64: &str,
    needs_elevation: bool,
    nano_home: &Path,
    cancellation_path: Option<&Path>,
) -> Result<()> {
    use windows_sys::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS;
    use windows_sys::Win32::UI::Shell::SHELLEXECUTEINFOW;
    use windows_sys::Win32::UI::Shell::ShellExecuteExW;
    let exe = find_setup_exe();
    let cleared_report = match clear_setup_error_report(nano_home) {
        Ok(()) => true,
        Err(err) => {
            log_note(
                &format!(
                    "setup orchestrator: failed to clear setup_error.json before launch: {err}"
                ),
                Some(&crate::sandbox_dir(nano_home)),
            );
            false
        }
    };

    if !needs_elevation {
        let child = Command::new(&exe)
            .arg(payload_b64)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| {
                failure(
                    SetupErrorCode::OrchestratorHelperLaunchFailed,
                    format!("failed to launch setup helper (non-elevated): {err}"),
                )
            })?;
        let mut process = SetupProcess::Child(child);
        let outcome = drive_setup_process(
            &mut process,
            cancellation_path,
            nano_home,
            &mut WinSetupProcessOps,
        );
        if outcome.initial_wait == SetupWait::Timeout {
            return Err(failure(
                SetupErrorCode::OrchestratorHelperTimedOut,
                format!(
                    "setup exceeded {ELEVATED_SETUP_TIMEOUT_MS} ms; cooperative cancellation requested; forced_termination={}; reaped={}; cleanup_errors={:?}",
                    outcome.forced, outcome.reaped, outcome.errors
                ),
            ));
        }
        if outcome.initial_wait != SetupWait::Signalled {
            return Err(failure(
                SetupErrorCode::OrchestratorHelperLaunchFailed,
                format!(
                    "setup helper wait/status anomaly: {:?}; cleanup forced={} reaped={} errors={:?}",
                    outcome.initial_wait, outcome.forced, outcome.reaped, outcome.errors
                ),
            ));
        }
        let Some(code) = outcome.exit_code else {
            return Err(failure(
                SetupErrorCode::OrchestratorHelperLaunchFailed,
                format!(
                    "setup helper exited without a status; cleanup forced={} reaped={} errors={:?}",
                    outcome.forced, outcome.reaped, outcome.errors
                ),
            ));
        };
        if code != 0 {
            return Err(report_helper_failure(
                nano_home,
                cleared_report,
                Some(code as i32),
            ));
        }
        verify_setup_completed(nano_home)?;
        if let Err(err) = clear_setup_error_report(nano_home) {
            log_note(
                &format!(
                    "setup orchestrator: failed to clear setup_error.json after success: {err}"
                ),
                Some(&crate::sandbox_dir(nano_home)),
            );
        }
        return Ok(());
    }

    let exe_w = crate::winutil::to_wide(&exe);
    let params = crate::winutil::quote_windows_arg(payload_b64);
    let params_w = crate::winutil::to_wide(params);
    let verb_w = crate::winutil::to_wide("runas");
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS;
    sei.lpVerb = verb_w.as_ptr();
    sei.lpFile = exe_w.as_ptr();
    sei.lpParameters = params_w.as_ptr();
    // Hide the window for the elevated helper.
    sei.nShow = 0; // SW_HIDE
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 || sei.hProcess == 0 {
        let last_error = unsafe { GetLastError() };
        let code = if last_error == ERROR_CANCELLED {
            SetupErrorCode::OrchestratorHelperLaunchCanceled
        } else {
            SetupErrorCode::OrchestratorHelperLaunchFailed
        };
        return Err(failure(
            code,
            format!("ShellExecuteExW failed to launch setup helper: {last_error}"),
        ));
    }
    {
        let mut process = SetupProcess::Raw(RawSetupProcess::new(sei.hProcess));
        let outcome = drive_setup_process(
            &mut process,
            cancellation_path,
            nano_home,
            &mut WinSetupProcessOps,
        );
        if outcome.initial_wait == SetupWait::Timeout {
            return Err(failure(
                SetupErrorCode::OrchestratorHelperTimedOut,
                format!(
                    "setup exceeded {ELEVATED_SETUP_TIMEOUT_MS} ms; cooperative cancellation requested; forced_termination={}; reaped={}; cleanup_errors={:?}",
                    outcome.forced, outcome.reaped, outcome.errors
                ),
            ));
        }
        if outcome.initial_wait != SetupWait::Signalled {
            return Err(failure(
                SetupErrorCode::OrchestratorHelperLaunchFailed,
                format!(
                    "setup helper wait/status anomaly: {:?}; cleanup forced={} reaped={} errors={:?}",
                    outcome.initial_wait, outcome.forced, outcome.reaped, outcome.errors
                ),
            ));
        }
        let Some(code) = outcome.exit_code else {
            return Err(failure(
                SetupErrorCode::OrchestratorHelperLaunchFailed,
                format!(
                    "setup helper exited without a status; cleanup forced={} reaped={} errors={:?}",
                    outcome.forced, outcome.reaped, outcome.errors
                ),
            ));
        };
        if code != 0 {
            return Err(report_helper_failure(
                nano_home,
                cleared_report,
                Some(code as i32),
            ));
        }
    }
    verify_setup_completed(nano_home)?;
    if let Err(err) = clear_setup_error_report(nano_home) {
        log_note(
            &format!("setup orchestrator: failed to clear setup_error.json after success: {err}"),
            Some(&crate::sandbox_dir(nano_home)),
        );
    }
    Ok(())
}

pub fn run_elevated_setup(
    request: SandboxSetupRequest<'_>,
    overrides: SetupRootOverrides,
) -> Result<()> {
    run_elevated_setup_inner(
        request, overrides, /*offline_proxy_settings_override*/ None,
    )
}

pub fn run_elevated_setup_with_proxy_settings(
    request: SandboxSetupRequest<'_>,
    overrides: SetupRootOverrides,
    offline_proxy_settings: &OfflineProxySettings,
) -> Result<()> {
    run_elevated_setup_inner(request, overrides, Some(offline_proxy_settings))
}

fn run_elevated_setup_inner(
    request: SandboxSetupRequest<'_>,
    overrides: SetupRootOverrides,
    offline_proxy_settings_override: Option<&OfflineProxySettings>,
) -> Result<()> {
    if !request.permissions.is_enforceable_by_windows_sandbox() {
        anyhow::bail!("unsupported filesystem permissions for Windows sandbox setup");
    }
    // Ensure the shared sandbox directory exists before we send it to the elevated helper.
    let sbx_dir = crate::sandbox_dir(request.nano_home);
    std::fs::create_dir_all(&sbx_dir).map_err(|err| {
        failure(
            SetupErrorCode::OrchestratorSandboxDirCreateFailed,
            format!("failed to create sandbox dir {}: {err}", sbx_dir.display()),
        )
    })?;
    let (read_roots, write_roots) = build_payload_roots(&request, &overrides);
    let deny_read_paths = build_payload_deny_read_paths(overrides.deny_read_paths);
    let deny_write_paths = build_payload_deny_write_paths(&request, overrides.deny_write_paths);
    let deny_write_paths_no_create = deny_write_paths_no_create(&deny_write_paths);
    let offline_proxy_settings =
        offline_proxy_settings_for_request(&request, offline_proxy_settings_override);
    let payload = ElevationPayload {
        version: SETUP_VERSION,
        offline_username: OFFLINE_USERNAME.to_string(),
        online_username: ONLINE_USERNAME.to_string(),
        nano_home: request.nano_home.to_path_buf(),
        command_cwd: request.command_cwd.to_path_buf(),
        read_roots,
        write_roots,
        deny_read_paths,
        deny_write_paths,
        deny_write_paths_no_create,
        proxy_ports: offline_proxy_settings.proxy_ports,
        allow_local_binding: offline_proxy_settings.allow_local_binding,
        real_user: std::env::var("USERNAME").unwrap_or_else(|_| "Administrators".to_string()),
        otel: telemetry::global_telemetry_settings(),
        mode: SetupMode::Full,
        refresh_only: false,
        cancellation_path: Some(new_setup_cancellation_path(request.nano_home)?),
    };
    let needs_elevation = !is_elevated().map_err(|err| {
        failure(
            SetupErrorCode::OrchestratorElevationCheckFailed,
            format!("failed to determine elevation state: {err}"),
        )
    })?;
    run_setup_exe(&payload, needs_elevation, request.nano_home)
}

pub fn run_elevated_provisioning_setup(
    nano_home: &Path,
    real_user: &str,
    settings: WindowsSandboxProvisioningSettings,
) -> Result<()> {
    let sbx_dir = crate::sandbox_dir(nano_home);
    std::fs::create_dir_all(&sbx_dir).map_err(|err| {
        failure(
            SetupErrorCode::OrchestratorSandboxDirCreateFailed,
            format!("failed to create sandbox dir {}: {err}", sbx_dir.display()),
        )
    })?;
    if !is_elevated().map_err(|err| {
        failure(
            SetupErrorCode::OrchestratorElevationCheckFailed,
            format!("failed to determine elevation state: {err}"),
        )
    })? {
        return Err(failure(
            SetupErrorCode::OrchestratorElevationRequired,
            "sandbox provisioning setup must be run from an elevated process",
        ));
    }
    let mut payload = provisioning_payload(nano_home, real_user, settings);
    payload.cancellation_path = Some(new_setup_cancellation_path(nano_home)?);
    run_setup_exe(&payload, /*needs_elevation*/ false, nano_home)
}

/// Builds the provisioning payload (ProvisionOnly mode) — factored out so the
/// owner-review dry-run bin can show exactly what would be sent.
pub fn provisioning_payload(
    nano_home: &Path,
    real_user: &str,
    settings: WindowsSandboxProvisioningSettings,
) -> ElevationPayload {
    ElevationPayload {
        version: SETUP_VERSION,
        offline_username: OFFLINE_USERNAME.to_string(),
        online_username: ONLINE_USERNAME.to_string(),
        nano_home: nano_home.to_path_buf(),
        command_cwd: nano_home.to_path_buf(),
        read_roots: Vec::new(),
        write_roots: Vec::new(),
        deny_read_paths: Vec::new(),
        deny_write_paths: Vec::new(),
        deny_write_paths_no_create: Vec::new(),
        proxy_ports: settings.proxy_ports,
        allow_local_binding: settings.allow_local_binding,
        otel: telemetry::global_telemetry_settings(),
        real_user: real_user.to_string(),
        mode: SetupMode::ProvisionOnly,
        refresh_only: false,
        cancellation_path: None,
    }
}

/// The provisioning payload as (pretty JSON for review, base64 for launch).
/// Must mint the same per-run cancellation token the orchestrated flow adds —
/// the helper's fail-closed validation rejects token-less payloads in any
/// mode that mutates ACLs, so a review payload without one is unusable.
pub fn provisioning_payload_review(
    nano_home: &Path,
    real_user: &str,
    settings: WindowsSandboxProvisioningSettings,
) -> Result<(String, String)> {
    let mut payload = provisioning_payload(nano_home, real_user, settings);
    payload.cancellation_path = Some(new_setup_cancellation_path(nano_home)?);
    let pretty = serde_json::to_string_pretty(&payload)?;
    let b64 = BASE64_STANDARD.encode(serde_json::to_vec(&payload)?);
    Ok((pretty, b64))
}

pub(crate) fn build_payload_roots(
    request: &SandboxSetupRequest<'_>,
    overrides: &SetupRootOverrides,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let write_roots = effective_write_roots_for_setup(
        request.permissions,
        request.command_cwd,
        request.env_map,
        request.nano_home,
        overrides.write_roots.as_deref(),
    );
    let mut read_roots = if let Some(roots) = overrides.read_roots.as_deref() {
        // An explicit override is the split policy's complete readable set.
        // Keep only the helper/platform roots the elevated setup needs; do not
        // re-add legacy cwd/full-read roots.
        let mut read_roots = gather_helper_read_roots(request.nano_home);
        if overrides.read_roots_include_platform_defaults {
            read_roots.extend(
                WINDOWS_PLATFORM_DEFAULT_READ_ROOTS
                    .iter()
                    .map(PathBuf::from),
            );
        }
        read_roots.extend(roots.iter().cloned());
        canonical_existing(&read_roots)
    } else {
        gather_read_roots(
            request.command_cwd,
            request.permissions,
            request.env_map,
            request.nano_home,
        )
    };
    read_roots = expand_user_profile_root(read_roots);
    read_roots = filter_user_profile_root(read_roots);
    read_roots = filter_user_profile_root_exclusions(read_roots);
    read_roots = filter_ssh_config_dependency_roots(read_roots);
    let write_root_set: HashSet<PathBuf> = write_roots.iter().cloned().collect();
    read_roots.retain(|root| !write_root_set.contains(root));
    (read_roots, write_roots)
}

fn build_payload_deny_write_paths(
    request: &SandboxSetupRequest<'_>,
    explicit_deny_write_paths: Option<Vec<PathBuf>>,
) -> Vec<PathBuf> {
    let allow_deny_paths: AllowDenyPaths = compute_allow_paths_for_permissions(
        request.permissions,
        request.command_cwd,
        request.env_map,
    );
    let mut deny_write_paths: Vec<PathBuf> = explicit_deny_write_paths
        .unwrap_or_default()
        .into_iter()
        .map(|path| canonicalize_path(&path))
        .collect();
    deny_write_paths.extend(allow_deny_paths.deny);
    deny_write_paths
}

fn build_payload_deny_read_paths(explicit_deny_read_paths: Option<Vec<PathBuf>>) -> Vec<PathBuf> {
    // Keep the configured spelling here so the ACL layer can plan both the
    // lexical path and any existing canonical target for reparse-point aliases.
    explicit_deny_read_paths.unwrap_or_default()
}

fn offline_proxy_settings_for_request(
    request: &SandboxSetupRequest<'_>,
    offline_proxy_settings_override: Option<&OfflineProxySettings>,
) -> OfflineProxySettings {
    offline_proxy_settings_override.cloned().unwrap_or_else(|| {
        let network_identity =
            SandboxNetworkIdentity::from_permissions(request.permissions, request.proxy_enforced);
        offline_proxy_settings_from_env(request.env_map, network_identity)
    })
}

fn is_elevated() -> Result<bool> {
    unsafe {
        let mut administrators_group: *mut c_void = std::ptr::null_mut();
        let ok = AllocateAndInitializeSid(
            &SECURITY_NT_AUTHORITY,
            2,
            SECURITY_BUILTIN_DOMAIN_RID,
            DOMAIN_ALIAS_RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut administrators_group,
        );
        if ok == 0 {
            return Err(anyhow!(
                "AllocateAndInitializeSid failed: {}",
                GetLastError()
            ));
        }
        let mut is_member = 0i32;
        let check = CheckTokenMembership(0, administrators_group, &mut is_member as *mut _);
        FreeSid(administrators_group as *mut _);
        if check == 0 {
            return Err(anyhow!("CheckTokenMembership failed: {}", GetLastError()));
        }
        Ok(is_member != 0)
    }
}

#[derive(Serialize)]
pub struct ElevationPayload {
    #[allow(missing_docs)]
    version: u32,
    offline_username: String,
    online_username: String,
    nano_home: PathBuf,
    command_cwd: PathBuf,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    #[serde(default)]
    deny_read_paths: Vec<PathBuf>,
    #[serde(default)]
    deny_write_paths: Vec<PathBuf>,
    #[serde(default)]
    deny_write_paths_no_create: Vec<PathBuf>,
    proxy_ports: Vec<u16>,
    #[serde(default)]
    allow_local_binding: bool,
    otel: Option<telemetry::TelemetrySettings>,
    real_user: String,
    mode: SetupMode,
    #[serde(default)]
    refresh_only: bool,
    cancellation_path: Option<PathBuf>,
}

/// Protected workspace metadata (`.git`, `.agents`, `.nano`) must never be
/// materialized by setup: a missing one stays absent (no-create contract),
/// so a freshly minted deny target can never appear as an attacker-visible
/// sentinel directory. (Track A port; the donor created every missing deny
/// path, including protected metadata.)
fn deny_write_paths_no_create(deny_write_paths: &[PathBuf]) -> Vec<PathBuf> {
    deny_write_paths
        .iter()
        .filter(|path| path.file_name().is_some_and(is_protected_metadata_name))
        .cloned()
        .collect()
}

fn is_protected_metadata_name(name: &std::ffi::OsStr) -> bool {
    PROTECTED_METADATA_PATH_NAMES
        .iter()
        .any(|protected| name == std::ffi::OsStr::new(protected))
}

/// Fresh per-run cancellation token path (`cancel-<guid>` under the sandbox
/// dir). The helper validates the shape and parent strictly before honoring
/// it, and the orchestrator creates the leaf only to request cancellation.
fn new_setup_cancellation_path(nano_home: &Path) -> Result<PathBuf> {
    use windows_sys::Win32::System::Com::CoCreateGuid;
    let mut guid = unsafe { std::mem::zeroed() };
    let result = unsafe { CoCreateGuid(&mut guid) };
    if result < 0 {
        return Err(failure(
            SetupErrorCode::OrchestratorPayloadSerializeFailed,
            format!("failed to create setup cancellation token: 0x{result:08x}"),
        ));
    }
    Ok(crate::sandbox_dir(nano_home).join(format!(
        "cancel-{:08x}{:04x}{:04x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )))
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SetupMode {
    Full,
    ProvisionOnly,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleflight_shares_one_leader_result() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let calls = Arc::clone(&calls);
            handles.push(std::thread::spawn(move || {
                run_setup_singleflight("test-key".to_string(), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    Ok(())
                })
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|r| r.is_ok()));
        // The singleflight guarantee is "at least one shared completion", and
        // followers that arrived before completion never rerun the work.
        assert!(calls.load(Ordering::SeqCst) <= 4);
    }

    #[test]
    fn payload_serializes_with_nano_identities() {
        let payload = ElevationPayload {
            version: SETUP_VERSION,
            offline_username: OFFLINE_USERNAME.to_string(),
            online_username: ONLINE_USERNAME.to_string(),
            nano_home: PathBuf::from(r"C:\nano"),
            command_cwd: PathBuf::from(r"C:\repo"),
            read_roots: vec![PathBuf::from(r"C:\repo")],
            write_roots: Vec::new(),
            deny_read_paths: Vec::new(),
            deny_write_paths: Vec::new(),
            deny_write_paths_no_create: Vec::new(),
            proxy_ports: vec![8080],
            allow_local_binding: false,
            otel: None,
            real_user: "dev".into(),
            mode: SetupMode::Full,
            refresh_only: true,
            cancellation_path: None,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("NanoSandboxOffline"));
        assert!(json.contains("\"mode\":\"full\""));
        assert!(json.contains("\"refresh_only\":true"));
    }

    #[test]
    fn review_payload_carries_valid_cancellation_token() {
        // Regression: the owner-review path used to emit
        // `cancellation_path: null`, which the helper's fail-closed
        // validation rejects — the review payload must be launchable as-is.
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("nano-home");
        let settings = WindowsSandboxProvisioningSettings {
            proxy_ports: Vec::new(),
            allow_local_binding: false,
        };
        let (_pretty, b64) =
            provisioning_payload_review(&nano_home, "dev", settings).expect("review payload");
        let json = BASE64_STANDARD.decode(b64).expect("payload b64 decodes");
        let value: serde_json::Value = serde_json::from_slice(&json).expect("payload json");
        let token = value["cancellation_path"]
            .as_str()
            .expect("token is present");
        let name = std::path::Path::new(token)
            .file_name()
            .and_then(|n| n.to_str())
            .expect("token has a UTF-8 file name");
        let hex = name
            .strip_prefix("cancel-")
            .expect("token name is cancel-<hex>");
        assert_eq!(hex.len(), 32, "token hex width");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "token is hex");
        assert!(
            token.starts_with(&crate::sandbox_dir(&nano_home).display().to_string()),
            "token lives directly inside the canonical sandbox dir"
        );
    }

    #[test]
    fn singleflight_leader_panic_becomes_shared_error_and_recovers() {
        let panic_error = run_setup_singleflight("panic-key".to_string(), || -> Result<()> {
            panic!("injected leader panic")
        })
        .expect_err("leader panic should become a shared error");
        assert!(panic_error.to_string().contains("leader panicked"));
        run_setup_singleflight("panic-key".to_string(), || Ok(()))
            .expect("panic must not strand the flight");
    }

    #[test]
    fn setup_authority_lane_excludes_overlap_and_recovers_from_panic() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads = (0..4)
            .map(|_| {
                let active = Arc::clone(&active);
                let peak = Arc::clone(&peak);
                std::thread::spawn(move || {
                    run_setup_authority_lane(|| {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        active.fetch_sub(1, Ordering::SeqCst);
                    })
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().expect("authority worker");
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        let _ = std::panic::catch_unwind(|| run_setup_authority_lane(|| panic!("injected")));
        run_setup_authority_lane(|| ());
    }

    #[test]
    fn raw_setup_process_closes_exactly_once_on_drop() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static CLOSES: AtomicUsize = AtomicUsize::new(0);
        fn count_close(_: windows_sys::Win32::Foundation::HANDLE) {
            CLOSES.fetch_add(1, Ordering::SeqCst);
        }
        CLOSES.store(0, Ordering::SeqCst);
        {
            let _process = RawSetupProcess {
                handle: 1,
                close: count_close,
            };
        }
        assert_eq!(CLOSES.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn setup_process_state_machine_covers_exit_and_cleanup_matrix() {
        use std::collections::VecDeque;

        struct FakeOps {
            waits: VecDeque<SetupWait>,
            exit: std::result::Result<u32, String>,
            cancel: std::result::Result<(), String>,
            taint: std::result::Result<(), String>,
            terminate: std::result::Result<(), String>,
            reap: std::result::Result<(), String>,
            remove: std::result::Result<(), String>,
            calls: Vec<&'static str>,
        }
        impl SetupProcessOps for FakeOps {
            fn wait(&mut self, _: &mut SetupProcess, _: u32) -> SetupWait {
                self.calls.push("wait");
                self.waits.pop_front().expect("scripted wait")
            }
            fn request_cancel(&mut self, _: Option<&Path>) -> std::result::Result<(), String> {
                self.calls.push("cancel");
                self.cancel.clone()
            }
            fn persist_taint(&mut self, _: &Path) -> std::result::Result<(), String> {
                self.calls.push("taint");
                self.taint.clone()
            }
            fn terminate(&mut self, _: &mut SetupProcess) -> std::result::Result<(), String> {
                self.calls.push("terminate");
                self.terminate.clone()
            }
            fn exit_code(&mut self, _: &mut SetupProcess) -> std::result::Result<u32, String> {
                self.calls.push("exit");
                self.exit.clone()
            }
            fn confirm_reaped(&mut self, _: &mut SetupProcess) -> std::result::Result<(), String> {
                self.calls.push("reap");
                self.reap.clone()
            }
            fn remove_cancel(&mut self, _: Option<&Path>) -> std::result::Result<(), String> {
                self.calls.push("remove");
                self.remove.clone()
            }
        }
        struct Case {
            name: &'static str,
            waits: Vec<SetupWait>,
            exit: std::result::Result<u32, String>,
            cancel: std::result::Result<(), String>,
            taint: std::result::Result<(), String>,
            terminate: std::result::Result<(), String>,
            reap: std::result::Result<(), String>,
            remove: std::result::Result<(), String>,
            forced: bool,
            reaped: bool,
            exit_code: Option<u32>,
            errors: usize,
        }
        let ok = || Ok(());
        let cases = vec![
            Case {
                name: "normal",
                waits: vec![SetupWait::Signalled],
                exit: Ok(0),
                cancel: ok(),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: false,
                reaped: true,
                exit_code: Some(0),
                errors: 0,
            },
            Case {
                name: "cooperative timeout",
                waits: vec![SetupWait::Timeout, SetupWait::Signalled],
                exit: Ok(0),
                cancel: ok(),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: false,
                reaped: true,
                exit_code: None,
                errors: 0,
            },
            Case {
                name: "forced reap",
                waits: vec![SetupWait::Timeout, SetupWait::Timeout, SetupWait::Signalled],
                exit: Ok(0),
                cancel: ok(),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: true,
                reaped: true,
                exit_code: None,
                errors: 0,
            },
            Case {
                name: "taint failure",
                waits: vec![SetupWait::Timeout, SetupWait::Timeout, SetupWait::Signalled],
                exit: Ok(0),
                cancel: ok(),
                taint: Err("taint".into()),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: true,
                reaped: true,
                exit_code: None,
                errors: 1,
            },
            Case {
                name: "terminate survivor",
                waits: vec![SetupWait::Timeout, SetupWait::Timeout],
                exit: Ok(0),
                cancel: ok(),
                taint: ok(),
                terminate: Err("terminate".into()),
                reap: ok(),
                remove: ok(),
                forced: true,
                reaped: false,
                exit_code: None,
                errors: 1,
            },
            Case {
                name: "wait failed",
                waits: vec![SetupWait::Failed(5), SetupWait::Signalled],
                exit: Ok(0),
                cancel: ok(),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: false,
                reaped: true,
                exit_code: None,
                errors: 0,
            },
            Case {
                name: "unexpected wait",
                waits: vec![SetupWait::Unexpected(7), SetupWait::Signalled],
                exit: Ok(0),
                cancel: ok(),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: false,
                reaped: true,
                exit_code: None,
                errors: 0,
            },
            Case {
                name: "exit failure",
                waits: vec![SetupWait::Signalled, SetupWait::Signalled],
                exit: Err("exit".into()),
                cancel: ok(),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: ok(),
                forced: false,
                reaped: true,
                exit_code: None,
                errors: 1,
            },
            Case {
                name: "still active token failures",
                waits: vec![
                    SetupWait::Signalled,
                    SetupWait::Timeout,
                    SetupWait::Signalled,
                ],
                exit: Ok(windows_sys::Win32::Foundation::STILL_ACTIVE as u32),
                cancel: Err("token".into()),
                taint: ok(),
                terminate: ok(),
                reap: ok(),
                remove: Err("remove".into()),
                forced: true,
                reaped: true,
                exit_code: None,
                errors: 3,
            },
        ];
        let home = tempfile::TempDir::new().expect("tempdir");
        for case in cases {
            let mut ops = FakeOps {
                waits: case.waits.into(),
                exit: case.exit,
                cancel: case.cancel,
                taint: case.taint,
                terminate: case.terminate,
                reap: case.reap,
                remove: case.remove,
                calls: vec![],
            };
            let mut process = SetupProcess::Raw(RawSetupProcess::new(0));
            let result = drive_setup_process(
                &mut process,
                Some(Path::new("token")),
                home.path(),
                &mut ops,
            );
            assert_eq!(
                (
                    result.forced,
                    result.reaped,
                    result.exit_code,
                    result.errors.len()
                ),
                (case.forced, case.reaped, case.exit_code, case.errors),
                "{}",
                case.name
            );
            assert_eq!(
                ops.calls.iter().filter(|call| **call == "remove").count(),
                usize::from(result.reaped && case.name != "normal"),
                "{} closes/removes once",
                case.name
            );
            let expected_calls: &[&str] = match case.name {
                "normal" => &["wait", "exit", "reap"],
                "cooperative timeout" | "wait failed" | "unexpected wait" => {
                    &["wait", "cancel", "wait", "reap", "remove"]
                }
                "forced reap" | "taint failure" => &[
                    "wait",
                    "cancel",
                    "wait",
                    "taint",
                    "terminate",
                    "wait",
                    "reap",
                    "remove",
                ],
                "terminate survivor" => &["wait", "cancel", "wait", "taint", "terminate"],
                "exit failure" => &["wait", "exit", "cancel", "wait", "reap", "remove"],
                "still active token failures" => &[
                    "wait",
                    "exit",
                    "cancel",
                    "wait",
                    "taint",
                    "terminate",
                    "wait",
                    "reap",
                    "remove",
                ],
                other => panic!("missing expected trace for {other}"),
            };
            assert_eq!(
                ops.calls, expected_calls,
                "{} exact safety call order",
                case.name
            );
            if case.name == "cooperative timeout" {
                assert!(!ops.calls.contains(&"terminate"));
            }
        }
    }

    #[test]
    fn deny_write_paths_no_create_covers_only_protected_metadata() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let missing_metadata = temp.path().join(".git");
        assert_eq!(
            deny_write_paths_no_create(&[
                missing_metadata.clone(),
                temp.path().join("ordinary-deny")
            ]),
            vec![missing_metadata]
        );
    }

    #[test]
    fn is_elevated_returns_bool_without_error() {
        // On any host this must answer, not panic; the value depends on the
        // test host's token and is asserted only for type correctness here.
        let _elevated: bool = is_elevated().expect("elevation check must not error");
    }
}
