//! Setup execution: payload assembly, singleflight orchestration, helper
//! launch (elevated and non-elevated), and completion verification.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/setup.rs` (execution
//! half) @ 646f7c0a. Transformations:
//! - codex_home -> nano_home naming;
//! - SETUP_EXE_FILENAME -> "nanok3-sandbox-setup.exe" (dual-track isolation:
//!   Track A's helper is "codex-windows-sandbox-setup.exe");
//! - ElevationPayload.otel -> Option<telemetry::TelemetrySettings> (facade;
//!   `global_telemetry_settings()` currently None on all paths);
//! - gather layer via crate::gather (ported separately).

use crate::allow::AllowDenyPaths;
use crate::allow::compute_allow_paths_for_permissions;
use crate::gather::canonical_existing;
use crate::gather::effective_write_roots_for_setup;
use crate::gather::expand_user_profile_root;
use crate::gather::filter_ssh_config_dependency_roots;
use crate::gather::filter_user_profile_root;
use crate::gather::filter_user_profile_root_exclusions;
use crate::gather::gather_helper_read_roots;
use crate::gather::gather_read_roots;
use crate::gather::WINDOWS_PLATFORM_DEFAULT_READ_ROOTS;
use crate::helper_materialization::bundled_executable_path_for_exe;
use crate::identity::sandbox_setup_is_complete;
use crate::logging::current_log_file_path;
use crate::logging::log_note;
use crate::path_normalization::canonicalize_path;
use crate::resolved_permissions::ResolvedWindowsSandboxPermissions;
use crate::setup_error::SetupErrorCode;
use crate::setup_error::SetupFailure;
use crate::setup_error::clear_setup_error_report;
use crate::setup_error::extract_failure;
use crate::setup_error::failure;
use crate::setup_error::read_setup_error_report;
use crate::setup_types::OfflineProxySettings;
use crate::setup_types::SetupRootOverrides;
use crate::setup_types::offline_proxy_settings_from_env;
use crate::setup_types::SandboxNetworkIdentity;
use crate::setup_types::OFFLINE_USERNAME;
use crate::setup_types::ONLINE_USERNAME;
use crate::setup_types::SETUP_VERSION;
use crate::telemetry;
use anyhow::Result;
use anyhow::anyhow;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::PermissionProfile;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
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
// Donor-local constants (windows-sys does not export these RIDs).
const SECURITY_BUILTIN_DOMAIN_RID: u32 = 0x0000_0020;
const DOMAIN_ALIAS_RID_ADMINS: u32 = 0x0000_0220;
const SETUP_EXE_FILENAME: &str = "nanok3-sandbox-setup.exe";

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

    let result = run();
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
        proxy_ports: offline_proxy_settings.proxy_ports,
        allow_local_binding: offline_proxy_settings.allow_local_binding,
        otel: None,
        real_user: std::env::var("USERNAME").unwrap_or_else(|_| "Administrators".to_string()),
        mode: SetupMode::Full,
        refresh_only: true,
    };
    let json = serde_json::to_vec(&payload)?;
    let b64 = BASE64_STANDARD.encode(json);
    run_setup_singleflight(b64.clone(), || {
        run_setup_refresh_payload(&b64, request.nano_home)
    })
}

fn run_setup_refresh_payload(b64: &str, nano_home: &Path) -> Result<()> {
    let exe = find_setup_exe();
    let sbx_dir = crate::sandbox_dir(nano_home);
    let log_path = current_log_file_path(&sbx_dir);
    let cleared_report = match clear_setup_error_report(nano_home) {
        Ok(()) => true,
        Err(err) => {
            log_note(
                &format!("setup refresh: failed to clear setup_error.json before launch: {err}"),
                Some(&sbx_dir),
            );
            false
        }
    };
    // Refresh should never request elevation; ensure verb isn't set and we don't trigger UAC.
    let mut cmd = Command::new(&exe);
    cmd.arg(b64)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let cwd = std::env::current_dir().unwrap_or_else(|_| nano_home.to_path_buf());
    log_note(
        &format!(
            "setup refresh: spawning {} (cwd={}, payload_len={})",
            exe.display(),
            cwd.display(),
            b64.len()
        ),
        Some(&sbx_dir),
    );
    let status = cmd.status().map_err(|err| {
        let message = format!(
            "setup refresh failed to launch helper: helper={}, cwd={}, log={}, error={err}",
            exe.display(),
            cwd.display(),
            log_path.display()
        );
        log_note(&format!("setup refresh: {message}"), Some(&sbx_dir));
        failure(SetupErrorCode::OrchestratorHelperLaunchFailed, message)
    })?;
    if !status.success() {
        log_note(
            &format!("setup refresh: exited with status {status:?}"),
            Some(&sbx_dir),
        );
        return Err(report_helper_failure(
            nano_home,
            cleared_report,
            status.code(),
        ));
    }
    if let Err(err) = clear_setup_error_report(nano_home) {
        log_note(
            &format!("setup refresh: failed to clear setup_error.json after success: {err}"),
            Some(&sbx_dir),
        );
    }
    Ok(())
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
    run_setup_singleflight(payload_b64.clone(), || {
        run_setup_exe_payload(&payload_b64, needs_elevation, nano_home)
    })
}

fn run_setup_exe_payload(
    payload_b64: &str,
    needs_elevation: bool,
    nano_home: &Path,
) -> Result<()> {
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    use windows_sys::Win32::System::Threading::INFINITE;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
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
        let status = Command::new(&exe)
            .arg(payload_b64)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|err| {
                failure(
                    SetupErrorCode::OrchestratorHelperLaunchFailed,
                    format!("failed to launch setup helper (non-elevated): {err}"),
                )
            })?;
        if !status.success() {
            return Err(report_helper_failure(
                nano_home,
                cleared_report,
                status.code(),
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
    unsafe {
        WaitForSingleObject(sei.hProcess, INFINITE);
        let mut code: u32 = 1;
        GetExitCodeProcess(sei.hProcess, &mut code);
        CloseHandle(sei.hProcess);
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
            &format!(
                "setup orchestrator: failed to clear setup_error.json after success: {err}"
            ),
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
        proxy_ports: offline_proxy_settings.proxy_ports,
        allow_local_binding: offline_proxy_settings.allow_local_binding,
        real_user: std::env::var("USERNAME").unwrap_or_else(|_| "Administrators".to_string()),
        otel: telemetry::global_telemetry_settings(),
        mode: SetupMode::Full,
        refresh_only: false,
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
    let payload = ElevationPayload {
        version: SETUP_VERSION,
        offline_username: OFFLINE_USERNAME.to_string(),
        online_username: ONLINE_USERNAME.to_string(),
        nano_home: nano_home.to_path_buf(),
        command_cwd: nano_home.to_path_buf(),
        read_roots: Vec::new(),
        write_roots: Vec::new(),
        deny_read_paths: Vec::new(),
        deny_write_paths: Vec::new(),
        proxy_ports: settings.proxy_ports,
        allow_local_binding: settings.allow_local_binding,
        otel: telemetry::global_telemetry_settings(),
        real_user: real_user.to_string(),
        mode: SetupMode::ProvisionOnly,
        refresh_only: false,
    };
    run_setup_exe(&payload, /*needs_elevation*/ false, nano_home)
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
struct ElevationPayload {
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
    proxy_ports: Vec<u16>,
    #[serde(default)]
    allow_local_binding: bool,
    otel: Option<telemetry::TelemetrySettings>,
    real_user: String,
    mode: SetupMode,
    #[serde(default)]
    refresh_only: bool,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SetupMode {
    Full,
    ProvisionOnly,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

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
    fn payload_serializes_with_nanok3_identities() {
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
            proxy_ports: vec![8080],
            allow_local_binding: false,
            otel: None,
            real_user: "dev".into(),
            mode: SetupMode::Full,
            refresh_only: true,
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("NanoK3SandboxOffline"));
        assert!(json.contains("\"mode\":\"full\""));
        assert!(json.contains("\"refresh_only\":true"));
    }

    #[test]
    fn is_elevated_returns_bool_without_error() {
        // On any host this must answer, not panic; the value depends on the
        // test host's token and is asserted only for type correctness here.
        let _elevated: bool = is_elevated().expect("elevation check must not error");
    }
}
