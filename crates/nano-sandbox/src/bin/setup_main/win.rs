//! Provenance: ported from Codex `windows-sandbox-rs/src/bin/setup_main` @
//! 646f7c0a. Transformations: codex_windows_sandbox -> nano_sandbox;
//! codex_otel -> telemetry facade; codex_home -> nano_home.

mod firewall;
mod read_acl_mutex;

use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use nano_sandbox::AclTreeTransaction;
use nano_sandbox::NANO_ACL_QUIESCENT_PRECONDITION;
use nano_sandbox::SETUP_VERSION;
use nano_sandbox::SetupErrorCode;
use nano_sandbox::SetupErrorReport;
use nano_sandbox::SetupFailure;
use nano_sandbox::add_deny_write_ace_on_tree_in_transaction_cancellable;
use nano_sandbox::convert_string_sid_to_sid;
use nano_sandbox::ensure_allow_mask_aces_with_inheritance;
use nano_sandbox::ensure_allow_write_aces_on_tree_in_transaction_cancellable;
use nano_sandbox::extract_setup_failure;
use nano_sandbox::hide_newly_created_users;
use nano_sandbox::install_wfp_filters;
use nano_sandbox::log_note;
use nano_sandbox::log_writer;
use nano_sandbox::path_mask_allows;
use nano_sandbox::path_write_aces_need_refresh;
use nano_sandbox::sandbox_bin_dir;
use nano_sandbox::sandbox_dir;
use nano_sandbox::sandbox_secrets_dir;
use nano_sandbox::sandbox_setup_artifacts_are_complete;
use nano_sandbox::setup_marker_path;
use nano_sandbox::string_from_sid_bytes;
use nano_sandbox::sync_persistent_deny_read_acls;
use nano_sandbox::telemetry::TelemetrySettings;
use nano_sandbox::to_wide;
use nano_sandbox::uninstall_wfp_filters;
use nano_sandbox::workspace_write_cap_sid_for_root;
use nano_sandbox::workspace_write_root_overlaps_path;
use nano_sandbox::write_setup_error_report;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::ffi::c_void;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HLOCAL;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::ACL;
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
use windows_sys::Win32::Security::Authorization::GRANT_ACCESS;
use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;
use windows_sys::Win32::Security::Authorization::SetEntriesInAclW;
use windows_sys::Win32::Security::Authorization::SetNamedSecurityInfoW;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_SID;
use windows_sys::Win32::Security::Authorization::TRUSTEE_W;
use windows_sys::Win32::Security::CONTAINER_INHERIT_ACE;
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::Security::OBJECT_INHERIT_ACE;
use windows_sys::Win32::Storage::FileSystem::DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;

const DENY_ACCESS: i32 = 3;
#[cfg(test)]
const WRITE_ROOT_ALLOW_MASK: u32 =
    FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

mod sandbox_users;
mod setup_runtime_bin;
use read_acl_mutex::acquire_read_acl_mutex;
use read_acl_mutex::read_acl_mutex_exists;
use sandbox_users::commit_setup_marker;
use sandbox_users::prepare_setup_marker;
use sandbox_users::provision_sandbox_users;
use sandbox_users::resolve_sandbox_users_group_sid;
use sandbox_users::resolve_sid;
use sandbox_users::sid_bytes_to_psid;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Payload {
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
    /// Protected metadata deny targets (`.git`, `.agents`, `.nano`) that must
    /// never be materialized when missing (Track A no-create contract).
    #[serde(default)]
    deny_write_paths_no_create: Vec<PathBuf>,
    proxy_ports: Vec<u16>,
    #[serde(default)]
    allow_local_binding: bool,
    #[serde(default)]
    otel: Option<TelemetrySettings>,
    real_user: String,
    #[serde(default)]
    mode: SetupMode,
    #[serde(default)]
    refresh_only: bool,
    #[serde(default)]
    refresh_marker_only: bool,
    /// Track-B addition: remove the Wayland Nano-track machine state (accounts,
    /// group, firewall rules, WFP objects, setup marker) and nothing else.
    #[serde(default)]
    uninstall: bool,
    /// Cooperative cancellation token for the bounded helper lifecycle (Track
    /// A port). Mandatory for Rust-orchestrated payloads; the Track-B
    /// script-built modes above (`uninstall`, `refresh_marker_only`) do not
    /// traverse workspace ACLs and are exempt.
    #[serde(default)]
    cancellation_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
enum SetupMode {
    #[default]
    Full,
    ProvisionOnly,
    ReadAclsOnly,
}

fn log_line(log: &mut dyn Write, msg: &str) -> Result<()> {
    let ts = chrono::Utc::now().to_rfc3339();
    writeln!(log, "[{ts}] {msg}").map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperLogFailed,
            format!("failed to write setup log line: {err}"),
        ))
    })?;
    Ok(())
}

fn workspace_write_cap_sids_for_path(
    nano_home: &Path,
    command_cwd: &Path,
    write_roots: &[PathBuf],
    path: &Path,
) -> Result<Vec<String>> {
    let mut sid_strs = Vec::new();
    for root in write_roots {
        if workspace_write_root_overlaps_path(root, path) {
            sid_strs.push(workspace_write_cap_sid_for_root(
                nano_home,
                command_cwd,
                root,
            )?);
        }
    }
    if sid_strs.is_empty() {
        if write_roots.is_empty() {
            sid_strs.push(workspace_write_cap_sid_for_root(
                nano_home,
                command_cwd,
                command_cwd,
            )?);
        } else {
            for root in write_roots {
                sid_strs.push(workspace_write_cap_sid_for_root(
                    nano_home,
                    command_cwd,
                    root,
                )?);
            }
        }
    }
    Ok(sid_strs)
}

fn spawn_read_acl_helper(payload: &Payload, _log: &mut dyn Write) -> Result<()> {
    let mut read_payload = payload.clone();
    read_payload.mode = SetupMode::ReadAclsOnly;
    read_payload.refresh_only = true;
    let payload_json = serde_json::to_vec(&read_payload)?;
    let payload_b64 = BASE64.encode(payload_json);
    let exe = std::env::current_exe().context("locate setup helper")?;
    Command::new(&exe)
        .arg(payload_b64)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .context("spawn read ACL helper")?;
    Ok(())
}

struct ReadAclSubjects<'a> {
    sandbox_group_psid: *mut c_void,
    rx_psids: &'a [*mut c_void],
}

fn apply_read_acls(
    read_roots: &[PathBuf],
    subjects: &ReadAclSubjects<'_>,
    log: &mut dyn Write,
    refresh_errors: &mut Vec<String>,
    access_mask: u32,
    access_label: &str,
    inheritance: u32,
) -> Result<()> {
    for root in read_roots {
        if !root.exists() {
            log_line(
                log,
                &format!("{access_label} root {} missing; skipping", root.display()),
            )?;
            continue;
        }
        let builtin_has = read_mask_allows_or_log(
            root,
            subjects.rx_psids,
            /*label*/ None,
            access_mask,
            access_label,
            refresh_errors,
            log,
        )?;
        if builtin_has {
            continue;
        }
        let sandbox_has = read_mask_allows_or_log(
            root,
            &[subjects.sandbox_group_psid],
            Some("sandbox_group"),
            access_mask,
            access_label,
            refresh_errors,
            log,
        )?;
        if sandbox_has {
            continue;
        }
        log_line(
            log,
            &format!(
                "granting {access_label} ACE to {} for sandbox users",
                root.display()
            ),
        )?;
        let result = unsafe {
            ensure_allow_mask_aces_with_inheritance(
                root,
                &[subjects.sandbox_group_psid],
                access_mask,
                inheritance,
            )
        };
        if let Err(err) = result {
            refresh_errors.push(format!(
                "grant {access_label} ACE failed on {} for sandbox_group: {err}",
                root.display()
            ));
            log_line(
                log,
                &format!(
                    "grant {access_label} ACE failed on {} for sandbox_group: {err}",
                    root.display()
                ),
            )?;
        }
    }
    Ok(())
}

fn read_mask_allows_or_log(
    root: &Path,
    psids: &[*mut c_void],
    label: Option<&str>,
    read_mask: u32,
    access_label: &str,
    refresh_errors: &mut Vec<String>,
    log: &mut dyn Write,
) -> Result<bool> {
    match path_mask_allows(root, psids, read_mask, /*require_all_bits*/ true) {
        Ok(has) => Ok(has),
        Err(e) => {
            let label_suffix = label
                .map(|value| format!(" for {value}"))
                .unwrap_or_default();
            refresh_errors.push(format!(
                "{access_label} mask check failed on {}{}: {}",
                root.display(),
                label_suffix,
                e
            ));
            log_line(
                log,
                &format!(
                    "{access_label} mask check failed on {}{}: {}; continuing",
                    root.display(),
                    label_suffix,
                    e
                ),
            )?;
            Ok(false)
        }
    }
}

fn lock_sandbox_dir(
    dir: &Path,
    real_user: &str,
    sandbox_group_sid: &[u8],
    sandbox_group_access_mode: i32,
    sandbox_group_mask: u32,
    real_user_mask: u32,
    _log: &mut dyn Write,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let system_sid = resolve_sid("SYSTEM")?;
    let admins_sid = resolve_sid("Administrators")?;
    let real_sid = resolve_sid(real_user)?;
    let entries = [
        (
            sandbox_group_sid.to_vec(),
            sandbox_group_mask,
            sandbox_group_access_mode,
        ),
        (
            system_sid,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
            GRANT_ACCESS,
        ),
        (
            admins_sid,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
            GRANT_ACCESS,
        ),
        (real_sid, real_user_mask, GRANT_ACCESS),
    ];
    unsafe {
        let mut eas: Vec<EXPLICIT_ACCESS_W> = Vec::new();
        let mut sids: Vec<*mut c_void> = Vec::new();
        for (sid_bytes, mask, access_mode) in entries.iter().map(|(s, m, a)| (s, *m, *a)) {
            let sid_str = string_from_sid_bytes(sid_bytes).map_err(anyhow::Error::msg)?;
            let sid_w = to_wide(OsStr::new(&sid_str));
            let mut psid: *mut c_void = std::ptr::null_mut();
            if ConvertStringSidToSidW(sid_w.as_ptr(), &mut psid) == 0 {
                return Err(anyhow::anyhow!(
                    "ConvertStringSidToSidW failed: {}",
                    GetLastError()
                ));
            }
            sids.push(psid);
            eas.push(EXPLICIT_ACCESS_W {
                grfAccessPermissions: mask,
                grfAccessMode: access_mode,
                grfInheritance: OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_SID,
                    ptstrName: psid as *mut u16,
                },
            });
        }
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let set = SetEntriesInAclW(
            eas.len() as u32,
            eas.as_ptr(),
            std::ptr::null_mut(),
            &mut new_dacl,
        );
        if set != 0 {
            return Err(anyhow::anyhow!(
                "SetEntriesInAclW sandbox dir failed: {set}",
            ));
        }
        let path_w = to_wide(dir.as_os_str());
        let res = SetNamedSecurityInfoW(
            path_w.as_ptr() as *mut u16,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null_mut(),
        );
        if res != 0 {
            return Err(anyhow::anyhow!(
                "SetNamedSecurityInfoW sandbox dir failed: {res}",
            ));
        }
        if !new_dacl.is_null() {
            LocalFree(new_dacl as HLOCAL);
        }
        for sid in sids {
            if !sid.is_null() {
                LocalFree(sid as HLOCAL);
            }
        }
    }
    Ok(())
}

pub fn main() -> Result<()> {
    let ret = real_main();
    if let Err(e) = &ret {
        // Best-effort: log unexpected top-level errors.
        if let Ok(nano_home) = std::env::var("CODEX_HOME") {
            let sbx_dir = sandbox_dir(Path::new(&nano_home));
            let _ = std::fs::create_dir_all(&sbx_dir);
            if let Some(mut f) = log_writer(&sbx_dir) {
                let _ = writeln!(
                    f,
                    "[{}] top-level error: {}",
                    chrono::Utc::now().to_rfc3339(),
                    e
                );
            }
        }
    }
    ret
}

fn real_main() -> Result<()> {
    let mut args = std::env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperRequestArgsFailed,
            "expected payload argument",
        )));
    }
    let payload_b64 = args.remove(1);
    let payload_json = BASE64.decode(payload_b64).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperRequestArgsFailed,
            format!("failed to decode payload b64: {err}"),
        ))
    })?;
    let payload: Payload = serde_json::from_slice(&payload_json).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperRequestArgsFailed,
            format!("failed to parse payload json: {err}"),
        ))
    })?;
    if payload.version != SETUP_VERSION {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperRequestArgsFailed,
            format!(
                "setup version mismatch: expected {SETUP_VERSION}, got {}",
                payload.version
            ),
        )));
    }
    let sbx_dir = sandbox_dir(&payload.nano_home);
    std::fs::create_dir_all(&sbx_dir).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSandboxDirCreateFailed,
            format!("failed to create sandbox dir {}: {err}", sbx_dir.display()),
        ))
    })?;
    // The cancellation token is honored during ACL traversal, so it must be
    // exactly the orchestrator-issued path — never a preexisting or redirected
    // file. Track-B script payloads (uninstall / refresh-marker-only) mutate
    // no workspace ACLs and carry no token.
    if !payload.uninstall && !payload.refresh_marker_only {
        validate_cancellation_path(&payload, &sbx_dir)?;
    }
    let mut log = log_writer(&sbx_dir).ok_or_else(|| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperLogFailed,
            format!("open log in {} failed", sbx_dir.display()),
        ))
    })?;
    let result = run_setup(&payload, &mut log, &sbx_dir);
    if let Err(err) = &result {
        let _ = log_line(&mut log, &format!("setup error: {err:?}"));
        log_note(&format!("setup error: {err:?}"), Some(sbx_dir.as_path()));
        let failure = extract_setup_failure(err)
            .map(|f| SetupFailure::new(f.code, f.message.clone()))
            .unwrap_or_else(|| {
                SetupFailure::new(SetupErrorCode::HelperUnknownError, err.to_string())
            });
        let report = SetupErrorReport {
            code: failure.code,
            message: failure.message,
        };
        if let Err(write_err) = write_setup_error_report(&payload.nano_home, &report) {
            let _ = log_line(
                &mut log,
                &format!("setup error report write failed: {write_err}"),
            );
            log_note(
                &format!("setup error report write failed: {write_err}"),
                Some(sbx_dir.as_path()),
            );
        }
    }
    result
}

/// Strict, fail-closed validation of the orchestrator-issued cancellation
/// token path (Track A port): `cancel-<32 hex>` directly inside the canonical
/// sandbox dir, whose parent must not be a reparse point and whose leaf must
/// not exist yet.
fn validate_cancellation_path(payload: &Payload, sbx_dir: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let Some(path) = payload.cancellation_path.as_ref() else {
        anyhow::bail!("setup payload is missing cancellation path");
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("cancellation path has no UTF-8 file name"))?;
    let nonce = name
        .strip_prefix("cancel-")
        .ok_or_else(|| anyhow::anyhow!("cancellation path has invalid prefix"))?;
    if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("cancellation path has malformed GUID token");
    }
    let parent_metadata = std::fs::symlink_metadata(sbx_dir)?;
    if parent_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        anyhow::bail!("expected sandbox directory is a reparse point");
    }
    let expected_parent = dunce::canonicalize(sbx_dir)?;
    let supplied_parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cancellation path has no parent"))?;
    let supplied_parent = dunce::canonicalize(supplied_parent)?;
    if supplied_parent != expected_parent {
        anyhow::bail!("cancellation path is outside the expected sandbox directory");
    }
    if path.try_exists()? {
        let leaf_metadata = std::fs::symlink_metadata(path)?;
        let kind = if leaf_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            "reparse"
        } else {
            "preexisting"
        };
        anyhow::bail!("cancellation path has a {kind} leaf");
    }
    Ok(())
}

fn run_setup(payload: &Payload, log: &mut dyn Write, sbx_dir: &Path) -> Result<()> {
    if payload.uninstall {
        return run_uninstall(payload, log, sbx_dir);
    }
    if payload.refresh_marker_only {
        return run_refresh_marker_only(payload, log);
    }
    let writes_setup_marker = !payload.refresh_only && payload.mode != SetupMode::ReadAclsOnly;
    let mutates_workspace_acls = payload.mode == SetupMode::Full;
    let in_progress = sbx_dir.join("setup-in-progress");
    if writes_setup_marker {
        prepare_setup_marker(&payload.nano_home, &payload.real_user)?;
    }
    match payload.mode {
        SetupMode::ReadAclsOnly => run_read_acl_only(payload, log),
        SetupMode::ProvisionOnly => run_provision_only(payload, log, sbx_dir),
        SetupMode::Full => run_setup_full(payload, log, sbx_dir),
    }?;
    if writes_setup_marker {
        commit_setup_marker(
            &payload.nano_home,
            &payload.offline_username,
            &payload.online_username,
            &payload.proxy_ports,
            payload.allow_local_binding,
        )?;
    }
    if mutates_workspace_acls {
        // A previous force-terminated helper left the home tainted; this run's
        // verified completion is the audited repair that clears it. The
        // in-progress sentinel is removed only after full readiness holds.
        let taint = sbx_dir.join("setup-tainted");
        if taint.try_exists()? {
            log_line(
                log,
                "setup audit: repairing and clearing taint after successful full setup",
            )?;
            std::fs::remove_file(&taint)?;
        }
        if !sandbox_setup_artifacts_are_complete(&payload.nano_home) {
            anyhow::bail!("setup completed ACL mutation without full readiness");
        }
        std::fs::remove_file(&in_progress)?;
    }
    Ok(())
}

// Uninstall path (Track-B addition — the donor has no teardown): removes ONLY
// Wayland Nano-track machine state — the two NanoSandbox* accounts, the
// NanoSandboxUsers group (only when its membership is exactly the
// provisioned accounts), the nano_sandbox_* firewall rules, the Wayland Nano WFP
// provider/sublayer/filters, the Winlogon UserList values hiding the sandbox
// accounts, the DPAPI sandbox_users.json secrets file (only after parsing it
// and verifying it names exactly the provisioned NanoSandbox* accounts),
// the setup marker, and the .sandbox log dir. Track A's Codex* objects are
// never touched: every removal is keyed by an exact Wayland Nano name, a verified
// file content, or a Track-B WFP GUID whose identity is verified before
// deletion, and any mismatch aborts the run (fail-closed). Every step
// tolerates already-absent state, so a rerun after a partial uninstall
// converges.
fn run_uninstall(payload: &Payload, log: &mut dyn Write, sbx_dir: &Path) -> Result<()> {
    log_line(
        log,
        "uninstall mode: removing Wayland Nano-track sandbox state",
    )?;

    // Firewall rules first: they reference the offline account SID, so they
    // must go before the accounts are deleted.
    let firewall_result = firewall::remove_offline_sandbox_rules(log);
    if let Err(err) = firewall_result {
        if extract_setup_failure(&err).is_some() {
            return Err(err);
        }
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!("remove offline sandbox firewall rules failed: {err}"),
        )));
    }

    // WFP filters also reference the offline account SID.
    let metrics_sink = nano_sandbox::telemetry::sink_from_settings(
        payload.otel.as_ref(),
        Some(&payload.nano_home),
    );
    let removed_filters = uninstall_wfp_filters(
        &payload.nano_home,
        metrics_sink
            .as_ref()
            .map(|s| s as &dyn nano_sandbox::telemetry::MetricsSink),
        |message| {
            let _ = log_line(log, message);
        },
    )
    .map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!("remove Wayland Nano WFP objects failed: {err}"),
        ))
    })?;
    log_line(log, &format!("removed {removed_filters} WFP filters"))?;

    sandbox_users::remove_sandbox_users(&payload.offline_username, &payload.online_username, log)?;

    // Login-screen hide entries and DPAPI credential blobs reference the
    // accounts by name, so they go after the accounts are removed.
    sandbox_users::remove_hide_user_entries(
        &payload.offline_username,
        &payload.online_username,
        log,
    )?;
    sandbox_users::remove_sandbox_secrets(
        &payload.nano_home,
        &payload.offline_username,
        &payload.online_username,
        log,
    )?;

    let marker_path = setup_marker_path(&payload.nano_home);
    match std::fs::remove_file(&marker_path) {
        Ok(()) => log_line(
            log,
            &format!("removed setup marker {}", marker_path.display()),
        )?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperUninstallFailed,
                format!(
                    "remove setup marker {} failed: {err}",
                    marker_path.display()
                ),
            )));
        }
    }

    log_note("setup uninstall completed", Some(sbx_dir));

    // Last step: the .sandbox log dir itself. The helper's own log file in
    // this dir is still open; std file handles are opened with delete-sharing,
    // so removal marks it delete-pending and it disappears when this process
    // exits.
    remove_sandbox_log_dir(&payload.nano_home, sbx_dir, log)?;
    Ok(())
}

/// Track-B addition: removes the `.sandbox` log dir as the final uninstall
/// step. Fail-closed on scope: the recursive delete only runs when the dir is
/// exactly `<nano_home>/.sandbox` under a `.nano` home — a payload pointing
/// anywhere else (e.g. at Track A's home) aborts instead of deleting foreign
/// state. A missing dir is tolerated (idempotent).
fn remove_sandbox_log_dir(nano_home: &Path, sbx_dir: &Path, log: &mut dyn Write) -> Result<()> {
    let home_is_nano = nano_home
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(".nano"));
    if !home_is_nano || sbx_dir != sandbox_dir(nano_home) {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!(
                "refusing to remove sandbox log dir {}: not the .nano-scoped .sandbox dir",
                sbx_dir.display()
            ),
        )));
    }
    log_line(
        log,
        &format!("removing sandbox log dir {}", sbx_dir.display()),
    )?;
    match std::fs::remove_dir_all(sbx_dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!("remove sandbox log dir {} failed: {err}", sbx_dir.display()),
        ))),
    }
}

// Marker-refresh-only path: performs exactly the marker steps provisioning
// runs (prepare the protected file, then commit valid contents) and nothing
// else — no user provisioning, firewall, or ACL work. Used by proof harnesses
// to verify marker idempotency; runs unelevated as the real user, who holds
// GA on the marker via its protected ACL.
fn run_refresh_marker_only(payload: &Payload, log: &mut dyn Write) -> Result<()> {
    log_line(log, "refresh-marker-only: rewriting setup marker")?;
    prepare_setup_marker(&payload.nano_home, &payload.real_user)?;
    commit_setup_marker(
        &payload.nano_home,
        &payload.offline_username,
        &payload.online_username,
        &payload.proxy_ports,
        payload.allow_local_binding,
    )?;
    log_line(log, "refresh-marker-only: setup marker rewritten")?;
    Ok(())
}

fn run_read_acl_only(payload: &Payload, log: &mut dyn Write) -> Result<()> {
    let _read_acl_guard = match acquire_read_acl_mutex()? {
        Some(guard) => guard,
        None => {
            log_line(log, "read ACL helper already running; skipping")?;
            return Ok(());
        }
    };
    log_line(log, "read-acl-only mode: applying read ACLs")?;
    let sandbox_group_sid = resolve_sandbox_users_group_sid()?;
    let sandbox_group_psid = sid_bytes_to_psid(&sandbox_group_sid)?;
    let mut refresh_errors: Vec<String> = Vec::new();
    if !payload.read_roots.is_empty() {
        let users_sid = resolve_sid("Users")?;
        let users_psid = sid_bytes_to_psid(&users_sid)?;
        let auth_sid = resolve_sid("Authenticated Users")?;
        let auth_psid = sid_bytes_to_psid(&auth_sid)?;
        let everyone_sid = resolve_sid("Everyone")?;
        let everyone_psid = sid_bytes_to_psid(&everyone_sid)?;
        let rx_psids = vec![users_psid, auth_psid, everyone_psid];
        let subjects = ReadAclSubjects {
            sandbox_group_psid,
            rx_psids: &rx_psids,
        };
        apply_read_acls(
            &payload.read_roots,
            &subjects,
            log,
            &mut refresh_errors,
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
            "read",
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
        )?;
        unsafe {
            if !users_psid.is_null() {
                LocalFree(users_psid as HLOCAL);
            }
            if !auth_psid.is_null() {
                LocalFree(auth_psid as HLOCAL);
            }
            if !everyone_psid.is_null() {
                LocalFree(everyone_psid as HLOCAL);
            }
        }
    }
    unsafe {
        if !sandbox_group_psid.is_null() {
            LocalFree(sandbox_group_psid as HLOCAL);
        }
    }
    if !refresh_errors.is_empty() {
        log_line(
            log,
            &format!("read ACL run completed with errors: {refresh_errors:?}"),
        )?;
        if payload.refresh_only {
            anyhow::bail!("read ACL run had errors");
        }
    }
    log_line(log, "read ACL run completed")?;
    Ok(())
}

fn provision_and_hide_sandbox_users(
    payload: &Payload,
    log: &mut dyn Write,
    sbx_dir: &Path,
) -> Result<()> {
    let provision_result = provision_sandbox_users(
        &payload.nano_home,
        &payload.offline_username,
        &payload.online_username,
        log,
    );
    if let Err(err) = provision_result {
        if extract_setup_failure(&err).is_some() {
            return Err(err);
        }
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUserProvisionFailed,
            format!("provision sandbox users failed: {err}"),
        )));
    }
    let users = vec![
        payload.offline_username.clone(),
        payload.online_username.clone(),
    ];
    hide_newly_created_users(&users, sbx_dir);
    Ok(())
}

fn configure_offline_sandbox_network(
    payload: &Payload,
    offline_sid_str: &str,
    log: &mut dyn Write,
) -> Result<()> {
    let proxy_allowlist_result = firewall::ensure_offline_proxy_allowlist(
        offline_sid_str,
        &payload.proxy_ports,
        payload.allow_local_binding,
        log,
    );
    if let Err(err) = proxy_allowlist_result {
        if extract_setup_failure(&err).is_some() {
            return Err(err);
        }
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperFirewallRuleCreateOrAddFailed,
            format!("ensure offline proxy allowlist failed: {err}"),
        )));
    }
    let firewall_result = firewall::ensure_offline_outbound_block(offline_sid_str, log);
    if let Err(err) = firewall_result {
        if extract_setup_failure(&err).is_some() {
            return Err(err);
        }
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperFirewallRuleCreateOrAddFailed,
            format!("ensure offline outbound block failed: {err}"),
        )));
    }
    let metrics_sink = nano_sandbox::telemetry::sink_from_settings(
        payload.otel.as_ref(),
        Some(&payload.nano_home),
    );
    install_wfp_filters(
        &payload.nano_home,
        &payload.offline_username,
        metrics_sink
            .as_ref()
            .map(|s| s as &dyn nano_sandbox::telemetry::MetricsSink),
        |message| {
            let _ = log_line(log, message);
        },
    );
    Ok(())
}

fn lock_persistent_sandbox_dirs(
    payload: &Payload,
    sandbox_group_sid: &[u8],
    log: &mut dyn Write,
) -> Result<()> {
    lock_sandbox_dir(
        &sandbox_dir(&payload.nano_home),
        &payload.real_user,
        sandbox_group_sid,
        GRANT_ACCESS,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        log,
    )
    .map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSandboxLockFailed,
            format!(
                "lock sandbox dir {} failed: {err}",
                sandbox_dir(&payload.nano_home).display()
            ),
        ))
    })?;
    lock_sandbox_dir(
        &sandbox_secrets_dir(&payload.nano_home),
        &payload.real_user,
        sandbox_group_sid,
        DENY_ACCESS,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE,
        log,
    )
    .map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSandboxLockFailed,
            format!(
                "lock sandbox secrets dir {} failed: {err}",
                sandbox_secrets_dir(&payload.nano_home).display()
            ),
        ))
    })?;
    let legacy_users = sandbox_dir(&payload.nano_home).join("sandbox_users.json");
    if legacy_users.exists() {
        let _ = std::fs::remove_file(&legacy_users);
    }
    Ok(())
}

fn lock_sandbox_bin_dir(
    payload: &Payload,
    sandbox_group_sid: &[u8],
    log: &mut dyn Write,
) -> Result<()> {
    lock_sandbox_dir(
        &sandbox_bin_dir(&payload.nano_home),
        &payload.real_user,
        sandbox_group_sid,
        GRANT_ACCESS,
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE,
        log,
    )
    .map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSandboxLockFailed,
            format!(
                "lock sandbox bin dir {} failed: {err}",
                sandbox_bin_dir(&payload.nano_home).display()
            ),
        ))
    })
}

fn run_provision_only(payload: &Payload, log: &mut dyn Write, sbx_dir: &Path) -> Result<()> {
    provision_and_hide_sandbox_users(payload, log, sbx_dir)?;
    let offline_sid = resolve_sid(&payload.offline_username).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSidResolveFailed,
            format!(
                "resolve SID for offline user {} failed: {err}",
                payload.offline_username
            ),
        ))
    })?;
    let offline_sid_str = string_from_sid_bytes(&offline_sid).map_err(anyhow::Error::msg)?;

    let sandbox_group_sid = resolve_sandbox_users_group_sid().map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSidResolveFailed,
            format!("resolve sandbox users group SID failed: {err}"),
        ))
    })?;

    configure_offline_sandbox_network(payload, &offline_sid_str, log)?;

    lock_sandbox_bin_dir(payload, &sandbox_group_sid, log)?;
    lock_persistent_sandbox_dirs(payload, &sandbox_group_sid, log)?;
    log_note("setup provisioning binary completed", Some(sbx_dir));
    Ok(())
}

fn run_setup_full(payload: &Payload, log: &mut dyn Write, sbx_dir: &Path) -> Result<()> {
    let cancelled = || -> Result<bool> {
        payload
            .cancellation_path
            .as_ref()
            .map_or(Ok(false), |path| {
                path.try_exists().context("query setup cancellation token")
            })
    };
    let refresh_only = payload.refresh_only;
    log_line(log, "setup stage: identity provisioning start")?;
    if !refresh_only {
        provision_and_hide_sandbox_users(payload, log, sbx_dir)?;
    }
    log_line(log, "setup stage: identity provisioning complete")?;
    let offline_sid = resolve_sid(&payload.offline_username).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSidResolveFailed,
            format!(
                "resolve SID for offline user {} failed: {err}",
                payload.offline_username
            ),
        ))
    })?;
    let offline_sid_str = string_from_sid_bytes(&offline_sid).map_err(anyhow::Error::msg)?;

    let sandbox_group_sid = resolve_sandbox_users_group_sid().map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSidResolveFailed,
            format!("resolve sandbox users group SID failed: {err}"),
        ))
    })?;
    let sandbox_group_psid = sid_bytes_to_psid(&sandbox_group_sid).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSidResolveFailed,
            format!("convert sandbox users group SID to PSID failed: {err}"),
        ))
    })?;
    let sandbox_group_sid_str =
        string_from_sid_bytes(&sandbox_group_sid).map_err(anyhow::Error::msg)?;

    let mut refresh_errors: Vec<String> = Vec::new();
    if !refresh_only {
        log_line(log, "setup stage: firewall configuration start")?;
        configure_offline_sandbox_network(payload, &offline_sid_str, log)?;
        log_line(log, "setup stage: firewall configuration complete")?;
    }

    // Deny-read ACEs must be present before the sandboxed command starts. Apply
    // them synchronously here instead of delegating them to the background
    // helper used for read grants.
    let applied_deny_read_paths = unsafe {
        sync_persistent_deny_read_acls(
            &payload.nano_home,
            &sandbox_group_sid_str,
            &payload.deny_read_paths,
            sandbox_group_psid,
        )
    }
    .context("apply deny-read ACLs")?;
    if !applied_deny_read_paths.is_empty() {
        log_line(
            log,
            &format!("applied {} deny-read ACLs", applied_deny_read_paths.len()),
        )?;
    }

    if payload.read_roots.is_empty() {
        log_line(log, "no read roots to grant; skipping read ACL helper")?;
    } else {
        match read_acl_mutex_exists() {
            Ok(true) => {
                log_line(log, "read ACL helper already running; skipping spawn")?;
            }
            Ok(false) => {
                spawn_read_acl_helper(payload, log).map_err(|err| {
                    anyhow::Error::new(SetupFailure::new(
                        SetupErrorCode::HelperReadAclHelperSpawnFailed,
                        format!("spawn read ACL helper failed: {err}"),
                    ))
                })?;
            }
            Err(err) => {
                log_line(
                    log,
                    &format!("read ACL mutex check failed: {err}; spawning anyway"),
                )?;
                spawn_read_acl_helper(payload, log).map_err(|spawn_err| {
                    anyhow::Error::new(SetupFailure::new(
                        SetupErrorCode::HelperReadAclHelperSpawnFailed,
                        format!(
                            "spawn read ACL helper failed after mutex error {err}: {spawn_err}"
                        ),
                    ))
                })?;
            }
        }
    }

    if refresh_only {
        setup_runtime_bin::ensure_codex_app_runtime_paths_readable(
            sandbox_group_psid,
            &mut refresh_errors,
            log,
        )?;
    }

    let mut grant_tasks: Vec<(PathBuf, String)> = Vec::new();

    let mut seen_deny_paths: HashSet<PathBuf> = HashSet::new();
    let mut seen_write_roots: HashSet<PathBuf> = HashSet::new();
    for root in &payload.write_roots {
        if !seen_write_roots.insert(root.clone()) {
            continue;
        }
        if !root.exists() {
            log_line(
                log,
                &format!("write root {} missing; skipping", root.display()),
            )?;
            continue;
        }
        let root_cap_sid_str =
            workspace_write_cap_sid_for_root(&payload.nano_home, &payload.command_cwd, root)?;
        let root_cap_psid = unsafe {
            convert_string_sid_to_sid(&root_cap_sid_str)
                .ok_or_else(|| anyhow::anyhow!("convert write root capability SID failed"))?
        };
        let need_grant =
            match path_write_aces_need_refresh(root, &[sandbox_group_psid, root_cap_psid]) {
                Ok(needs_refresh) => needs_refresh,
                Err(e) => {
                    refresh_errors.push(format!(
                        "write ACE check failed on {}: {}",
                        root.display(),
                        e
                    ));
                    log_line(
                        log,
                        &format!(
                            "write ACE check failed on {}: {}; continuing",
                            root.display(),
                            e
                        ),
                    )?;
                    true
                }
            };
        unsafe {
            LocalFree(root_cap_psid as HLOCAL);
        }
        if need_grant {
            log_line(
                log,
                &format!(
                    "granting write ACE to {} for sandbox group and capability SID",
                    root.display()
                ),
            )?;
        }
        // Descendants can have protected DACLs that do not inherit a refreshed root ACE, so walk
        // every root even when the root itself is already current.
        grant_tasks.push((root.clone(), root_cap_sid_str));
    }

    if refresh_only && !refresh_errors.is_empty() {
        log_line(
            log,
            &format!("setup refresh aborted before ACL mutation: {refresh_errors:?}"),
        )?;
        anyhow::bail!("setup refresh had pre-ACL errors");
    }

    // Wayland Nano inherited provisioning requires a quiescent same-user window: no same-user
    // process may mutate workspace aliases while this setup phase runs. Concurrent same-user
    // hard-link attack resistance is intentionally unsupported here and must be kernel-enforced
    // before any shell exposure. Classify all deny paths once, then materialize only ordinary
    // missing targets before the first transactional ACL mutation. Protected metadata is
    // strictly no-create.
    log_line(log, "setup stage: workspace ACL transaction start")?;
    log_line(log, NANO_ACL_QUIESCENT_PRECONDITION)?;
    let no_create_deny_paths: HashSet<&Path> = payload
        .deny_write_paths_no_create
        .iter()
        .map(PathBuf::as_path)
        .collect();
    let mut skipped_missing_deny_paths = HashSet::new();
    let mut deny_paths_to_materialize = Vec::new();
    let mut materialized_deny_paths = HashSet::new();
    for path in &payload.deny_write_paths {
        if !materialized_deny_paths.insert(path.clone()) {
            continue;
        }
        match classify_deny_write_path(path, no_create_deny_paths.contains(path.as_path()))
            .with_context(|| format!("query deny-write path {}", path.display()))?
        {
            DenyWritePathDisposition::Existing => {}
            DenyWritePathDisposition::SkipMissing => {
                skipped_missing_deny_paths.insert(path.clone());
            }
            DenyWritePathDisposition::MaterializeMissing => {
                deny_paths_to_materialize.push(path.clone());
            }
        }
    }
    for path in deny_paths_to_materialize {
        std::fs::create_dir_all(&path).with_context(|| {
            format!(
                "failed to pre-materialize deny-write path {}",
                path.display()
            )
        })?;
    }

    // Process roots serially because roots can overlap. Concurrent DACL read/modify/write cycles
    // on a shared descendant could otherwise lose one root's capability ACE.
    let mut acl_transaction = AclTreeTransaction::new();
    let acl_phase_result = (|| -> Result<()> {
        for (root, root_cap_sid_str) in grant_tasks {
            log_line(
                log,
                &format!("setup stage: grant workspace ACL start: {}", root.display()),
            )?;
            let sid_strings = [&sandbox_group_sid_str, &root_cap_sid_str];
            let mut psids: Vec<*mut c_void> = Vec::new();
            for sid_str in sid_strings {
                if let Some(psid) = unsafe { convert_string_sid_to_sid(sid_str) } {
                    psids.push(psid);
                } else {
                    break;
                }
            }
            let res = if psids.len() == 2 {
                unsafe {
                    ensure_allow_write_aces_on_tree_in_transaction_cancellable(
                        &root,
                        &psids,
                        &mut acl_transaction,
                        &cancelled,
                    )
                }
                .map(|changed| changed != 0)
            } else {
                Err(anyhow::anyhow!("convert SID failed"))
            };
            for psid in psids {
                unsafe { LocalFree(psid as HLOCAL) };
            }
            if let Err(e) = res {
                log_line(
                    log,
                    &format!("write ACE grant failed on {}: {}", root.display(), e),
                )?;
                return Err(e).with_context(|| format!("apply write ACL tree {}", root.display()));
            }
            log_line(
                log,
                &format!(
                    "setup stage: grant workspace ACL complete: {}",
                    root.display()
                ),
            )?;
        }

        for path in &payload.deny_write_paths {
            if !seen_deny_paths.insert(path.clone()) {
                continue;
            }
            if skipped_missing_deny_paths.contains(path) {
                log_line(
                    log,
                    &format!(
                        "deny workspace ACL target {} missing; preserving no-create contract",
                        path.display()
                    ),
                )?;
                continue;
            }
            log_line(
                log,
                &format!("setup stage: deny workspace ACL start: {}", path.display()),
            )?;

            // These are deny-write carveouts, not deny-read paths. They may come from explicit
            // read-only-under-a-writable-root carveouts in the transformed sandbox policy, or from
            // legacy protected children such as `.git`, `.nano`, and `.agents`.
            //
            let deny_sid_strs = workspace_write_cap_sids_for_path(
                &payload.nano_home,
                &payload.command_cwd,
                &payload.write_roots,
                path,
            )?;
            for deny_sid_str in deny_sid_strs {
                let deny_psid = unsafe {
                    convert_string_sid_to_sid(&deny_sid_str)
                        .ok_or_else(|| anyhow::anyhow!("convert deny capability SID failed"))?
                };

                let deny_result = unsafe {
                    add_deny_write_ace_on_tree_in_transaction_cancellable(
                        path,
                        deny_psid,
                        &mut acl_transaction,
                        &cancelled,
                    )
                };
                unsafe {
                    LocalFree(deny_psid as HLOCAL);
                }
                match deny_result {
                    Ok(changed) if changed != 0 => {
                        log_line(
                            log,
                            &format!("applied deny ACE to protect {}", path.display()),
                        )?;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        log_line(
                            log,
                            &format!("deny ACE failed on {}: {err}", path.display()),
                        )?;
                        return Err(err).with_context(|| {
                            format!("apply deny-write ACL tree to {}", path.display())
                        });
                    }
                }
            }
            log_line(
                log,
                &format!(
                    "setup stage: deny workspace ACL complete: {}",
                    path.display()
                ),
            )?;
        }

        lock_sandbox_bin_dir(payload, &sandbox_group_sid, log)?;

        if refresh_only {
            log_line(
                log,
                &format!(
                    "setup refresh: processed {} write roots (read roots delegated); errors={:?}",
                    payload.write_roots.len(),
                    refresh_errors
                ),
            )?;
        }
        if !refresh_only {
            lock_persistent_sandbox_dirs(payload, &sandbox_group_sid, log)?;
        }

        Ok(())
    })();

    if let Err(err) = acl_phase_result {
        if let Err(rollback_err) = acl_transaction.rollback_now() {
            anyhow::bail!(
                "workspace ACL phase failed: {err}; rollback also failed: {rollback_err}"
            );
        }
        return Err(err).context("workspace ACL phase rolled back");
    }

    acl_transaction
        .commit()
        .context("commit workspace ACL transaction")?;
    log_line(log, "setup stage: workspace ACL transaction complete")?;

    unsafe {
        if !sandbox_group_psid.is_null() {
            LocalFree(sandbox_group_psid as HLOCAL);
        }
    }
    log_note("setup binary completed", Some(sbx_dir));
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DenyWritePathDisposition {
    Existing,
    SkipMissing,
    MaterializeMissing,
}

fn classify_deny_write_path(
    path: &Path,
    no_create: bool,
) -> std::io::Result<DenyWritePathDisposition> {
    classify_deny_write_path_with(path, no_create, |path| {
        std::fs::symlink_metadata(path).map(|_| ())
    })
}

fn classify_deny_write_path_with(
    path: &Path,
    no_create: bool,
    query: impl FnOnce(&Path) -> std::io::Result<()>,
) -> std::io::Result<DenyWritePathDisposition> {
    match query(path) {
        Ok(()) => Ok(DenyWritePathDisposition::Existing),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(if no_create {
            DenyWritePathDisposition::SkipMissing
        } else {
            DenyWritePathDisposition::MaterializeMissing
        }),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::BASE64;
    use super::Payload;
    use super::SETUP_VERSION;
    use super::WRITE_ROOT_ALLOW_MASK;
    use super::convert_string_sid_to_sid;
    use super::workspace_write_cap_sids_for_path;
    use base64::Engine;
    use nano_sandbox::AclTreeTransaction;
    use nano_sandbox::NANO_ACL_QUIESCENT_PRECONDITION;
    use nano_sandbox::add_deny_write_ace;
    use nano_sandbox::add_deny_write_ace_on_tree;
    use nano_sandbox::add_deny_write_ace_on_tree_in_transaction;
    use nano_sandbox::ensure_allow_mask_aces;
    use nano_sandbox::ensure_allow_write_aces;
    use nano_sandbox::ensure_allow_write_aces_on_tree;
    use nano_sandbox::ensure_allow_write_aces_on_tree_in_transaction;
    use nano_sandbox::load_or_create_cap_sids;
    use nano_sandbox::path_dacl_bytes;
    use nano_sandbox::path_dacl_is_protected;
    use nano_sandbox::path_has_write_deny_for_sid;
    use nano_sandbox::path_mask_allows;
    use nano_sandbox::path_write_aces_need_refresh;
    use nano_sandbox::telemetry::TelemetrySettings;
    use nano_sandbox::workspace_write_cap_sid_for_root;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::fs;
    use windows_sys::Win32::Foundation::HLOCAL;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Storage::FileSystem::DELETE;
    use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;

    fn payload_json() -> serde_json::Value {
        json!({
            "version": SETUP_VERSION,
            "offline_username": "NanoSandboxOffline",
            "online_username": "NanoSandboxOnline",
            "nano_home": "C:\\codex-home",
            "command_cwd": "C:\\workspace",
            "read_roots": [],
            "write_roots": [],
            "proxy_ports": [],
            "real_user": "User",
        })
    }

    #[test]
    fn payload_defaults_otel_absent() {
        let payload: Payload = serde_json::from_value(payload_json()).expect("payload");

        assert_eq!(payload.otel, None);
    }

    #[test]
    fn payload_defaults_refresh_marker_only_absent() {
        let payload: Payload = serde_json::from_value(payload_json()).expect("payload");

        assert!(!payload.refresh_marker_only);
    }

    #[test]
    fn payload_accepts_refresh_marker_only() {
        let mut payload = payload_json();
        payload["refresh_marker_only"] = json!(true);
        let payload_json = serde_json::to_vec(&payload).expect("serialize payload");
        // Exercise the same base64 wire contract the CLI entry point uses.
        let payload_b64 = BASE64.encode(payload_json);
        let decoded = BASE64.decode(payload_b64).expect("decode payload b64");
        let payload: Payload = serde_json::from_slice(&decoded).expect("payload");

        assert!(payload.refresh_marker_only);
        assert!(!payload.refresh_only);
        assert_eq!(payload.mode, super::SetupMode::Full);
    }

    #[test]
    fn payload_defaults_uninstall_absent() {
        let payload: Payload = serde_json::from_value(payload_json()).expect("payload");

        assert!(!payload.uninstall);
    }

    #[test]
    fn payload_accepts_uninstall() {
        let mut payload = payload_json();
        payload["uninstall"] = json!(true);
        let payload_json = serde_json::to_vec(&payload).expect("serialize payload");
        // Exercise the same base64 wire contract the CLI entry point uses.
        let payload_b64 = BASE64.encode(payload_json);
        let decoded = BASE64.decode(payload_b64).expect("decode payload b64");
        let payload: Payload = serde_json::from_slice(&decoded).expect("payload");

        assert!(payload.uninstall);
        assert!(!payload.refresh_only);
        assert!(!payload.refresh_marker_only);
        assert_eq!(payload.mode, super::SetupMode::Full);
    }

    #[test]
    fn payload_accepts_provision_only_mode() {
        let mut payload = payload_json();
        payload["mode"] = json!("provision-only");
        let payload: Payload = serde_json::from_value(payload).expect("payload");

        assert_eq!(payload.mode, super::SetupMode::ProvisionOnly);
    }

    #[test]
    fn payload_accepts_otel_settings() {
        let mut payload = payload_json();
        payload["otel"] = json!({
            "environment": "prod",
        });
        let payload: Payload = serde_json::from_value(payload).expect("payload");

        assert_eq!(
            payload.otel,
            Some(TelemetrySettings {
                environment: "prod".to_string(),
                service_name: String::new(),
            })
        );
    }

    #[test]
    fn write_root_refresh_replaces_stale_delete_child_grant() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert workspace sid") };
        let stale_write_mask = WRITE_ROOT_ALLOW_MASK | FILE_DELETE_CHILD;
        let seeded = unsafe { ensure_allow_mask_aces(&workspace, &[psid], stale_write_mask) }
            .expect("seed stale write ACE");
        let needs_refresh_before =
            path_write_aces_need_refresh(&workspace, &[psid]).expect("check stale write ACE");
        let replaced = unsafe { ensure_allow_write_aces(&workspace, &[psid]) }
            .expect("replace stale write ACE");
        let needs_refresh_after =
            path_write_aces_need_refresh(&workspace, &[psid]).expect("check refreshed write ACE");
        unsafe {
            LocalFree(psid as HLOCAL);
        }

        assert_eq!(
            (seeded, needs_refresh_before, replaced, needs_refresh_after),
            (true, true, true, false)
        );
    }

    #[test]
    fn write_root_refresh_checks_each_sid() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let other_root = temp.path().join("other-root");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&other_root).expect("create other root");

        let workspace_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let other_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &other_root)
            .expect("other root sid");
        let workspace_psid =
            unsafe { convert_string_sid_to_sid(&workspace_sid).expect("convert workspace sid") };
        let other_psid =
            unsafe { convert_string_sid_to_sid(&other_sid).expect("convert other root sid") };

        let seeded = unsafe { ensure_allow_write_aces(&workspace, &[workspace_psid]) }
            .expect("seed workspace SID");
        let needs_refresh_before =
            path_write_aces_need_refresh(&workspace, &[workspace_psid, other_psid])
                .expect("check both SIDs");
        let refreshed =
            unsafe { ensure_allow_write_aces(&workspace, &[workspace_psid, other_psid]) }
                .expect("refresh both SIDs");
        let needs_refresh_after =
            path_write_aces_need_refresh(&workspace, &[workspace_psid, other_psid])
                .expect("recheck both SIDs");
        unsafe {
            LocalFree(workspace_psid as HLOCAL);
            LocalFree(other_psid as HLOCAL);
        }

        assert_eq!(
            (seeded, needs_refresh_before, refreshed, needs_refresh_after,),
            (true, true, true, false)
        );
    }

    #[test]
    fn write_root_refresh_rejects_inherited_delete_child_grant() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let parent = temp.path().join("parent");
        let workspace = parent.join("workspace");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert workspace sid") };
        let seeded_explicit =
            unsafe { ensure_allow_mask_aces(&workspace, &[psid], WRITE_ROOT_ALLOW_MASK) }
                .expect("seed explicit write ACE");
        let seeded_parent = unsafe { ensure_allow_mask_aces(&parent, &[psid], 0x1000_0000) }
            .expect("seed inherited stale write ACE");
        let has_inherited_delete_child = path_mask_allows(
            &workspace,
            &[psid],
            FILE_DELETE_CHILD,
            /*require_all_bits*/ false,
        )
        .expect("check inherited stale write ACE");
        let needs_refresh = path_write_aces_need_refresh(&workspace, &[psid])
            .expect("check inherited stale write ACE");
        let refresh_error = unsafe { ensure_allow_write_aces_on_tree(&workspace, &[psid]) }
            .expect_err("stale inherited delete-child grant must fail closed");
        unsafe {
            LocalFree(psid as HLOCAL);
        }

        assert_eq!(
            (
                seeded_explicit,
                seeded_parent,
                has_inherited_delete_child,
                needs_refresh,
                refresh_error
                    .to_string()
                    .contains("unsupported inherited FILE_DELETE_CHILD"),
            ),
            (true, true, true, false, true)
        );
    }

    #[test]
    fn deny_write_path_classification_is_no_follow_and_fail_closed() {
        use super::DenyWritePathDisposition;

        let temp = tempfile::tempdir().expect("tempdir");
        let missing_metadata = temp.path().join(".git");
        assert_eq!(
            super::classify_deny_write_path(&missing_metadata, true)
                .expect("classify missing metadata"),
            DenyWritePathDisposition::SkipMissing
        );
        assert!(!missing_metadata.exists());

        let existing = temp.path().join(".codex");
        fs::create_dir(&existing).expect("create metadata directory");
        assert_eq!(
            super::classify_deny_write_path(&existing, true).expect("classify existing metadata"),
            DenyWritePathDisposition::Existing
        );

        // A successful no-follow query represents files, directories, dangling symlinks, and
        // reparse points alike. Injection avoids requiring symlink privileges on the test host.
        assert_eq!(
            super::classify_deny_write_path_with(&missing_metadata, true, |_| Ok(()))
                .expect("classify no-follow reparse result"),
            DenyWritePathDisposition::Existing
        );

        let error = super::classify_deny_write_path_with(&missing_metadata, true, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected",
            ))
        })
        .expect_err("metadata query errors must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

        let ordinary = temp.path().join("ordinary-deny");
        assert_eq!(
            super::classify_deny_write_path(&ordinary, false)
                .expect("classify ordinary missing deny"),
            DenyWritePathDisposition::MaterializeMissing
        );
        fs::create_dir_all(&ordinary).expect("materialize ordinary deny");
        assert!(ordinary.is_dir());
    }

    #[test]
    fn nano_acl_precondition_documents_quiescent_unsupported_hard_link_window() {
        assert!(NANO_ACL_QUIESCENT_PRECONDITION.contains("quiescent same-user window"));
        assert!(NANO_ACL_QUIESCENT_PRECONDITION.contains("hard-link mutation is unsupported"));
    }

    #[test]
    fn cancellation_path_validation_is_strict_and_fail_closed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let sbx_dir = temp.path().join(".sandbox");
        fs::create_dir(&sbx_dir).expect("sandbox dir");
        let mut value = payload_json();
        value["nano_home"] = json!(temp.path());
        let valid = sbx_dir.join("cancel-0123456789abcdef0123456789abcdef");
        value["cancellation_path"] = json!(valid);
        let payload: Payload = serde_json::from_value(value.clone()).expect("payload");
        super::validate_cancellation_path(&payload, &sbx_dir).expect("valid cancellation path");

        for invalid in [
            sbx_dir.join("cancel-not-a-guid"),
            temp.path().join("cancel-0123456789abcdef0123456789abcdef"),
            sbx_dir.join(r"..\cancel-0123456789abcdef0123456789abcdef"),
        ] {
            value["cancellation_path"] = json!(invalid);
            let payload: Payload = serde_json::from_value(value.clone()).expect("payload");
            super::validate_cancellation_path(&payload, &sbx_dir)
                .expect_err("invalid cancellation path must fail closed");
        }

        fs::write(&valid, b"preexisting").expect("precreate token");
        value["cancellation_path"] = json!(valid);
        let payload: Payload = serde_json::from_value(value.clone()).expect("payload");
        super::validate_cancellation_path(&payload, &sbx_dir)
            .expect_err("preexisting cancellation leaf must fail closed");

        fs::remove_file(&valid).expect("remove preexisting token");
        let real_sbx = temp.path().join("real-sandbox");
        fs::rename(&sbx_dir, &real_sbx).expect("move sandbox dir");
        let junction = std::process::Command::new("cmd.exe")
            .args([
                "/D",
                "/C",
                "mklink",
                "/J",
                &sbx_dir.display().to_string(),
                &real_sbx.display().to_string(),
            ])
            .status()
            .expect("create sandbox junction");
        assert!(junction.success());
        value["cancellation_path"] = json!(sbx_dir.join("cancel-0123456789abcdef0123456789abcdef"));
        let payload: Payload = serde_json::from_value(value).expect("payload");
        super::validate_cancellation_path(&payload, &sbx_dir)
            .expect_err("reparse sandbox parent must fail closed");
    }

    #[test]
    fn write_root_refresh_grants_delete_to_existing_descendants_and_preserves_denies() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let nested = workspace.join("existing").join("nested");
        let file = nested.join("file.txt");
        let protected = workspace.join(".codex");
        let protected_file = protected.join("state.json");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&nested).expect("create nested directory");
        fs::create_dir_all(&protected).expect("create protected directory");
        fs::write(&file, b"existing").expect("create existing file");
        fs::write(&protected_file, b"protected").expect("create protected file");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert workspace sid") };
        let denied =
            unsafe { add_deny_write_ace(&protected, psid) }.expect("seed protected metadata deny");
        let changed = unsafe { ensure_allow_write_aces_on_tree(&workspace, &[psid]) }
            .expect("refresh writable tree");
        unsafe { add_deny_write_ace_on_tree(&protected, psid) }
            .expect("reapply protected metadata deny tree");

        let descendant_delete =
            path_mask_allows(&file, &[psid], DELETE, true).expect("check descendant delete grant");
        let root_delete_child = path_mask_allows(
            &workspace,
            &[psid],
            FILE_DELETE_CHILD,
            /*require_all_bits*/ false,
        )
        .expect("check root delete-child grant");
        let nested_delete_child = path_mask_allows(
            &nested,
            &[psid],
            FILE_DELETE_CHILD,
            /*require_all_bits*/ false,
        )
        .expect("check nested delete-child grant");
        let protected_deny = unsafe { path_has_write_deny_for_sid(&protected, psid) }
            .expect("check protected metadata deny");
        let protected_file_deny = unsafe { path_has_write_deny_for_sid(&protected_file, psid) }
            .expect("check protected metadata descendant deny");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(denied);
        assert!(changed > 0);
        assert!(descendant_delete);
        assert!(!root_delete_child);
        assert!(!nested_delete_child);
        assert!(protected_deny);
        assert!(protected_file_deny);
    }

    #[test]
    fn write_root_refresh_does_not_follow_junctions() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        let junction = workspace.join("outside-link");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&outside).expect("create outside directory");
        fs::write(outside.join("outside.txt"), b"outside").expect("create outside file");
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .expect("launch mklink");
        assert!(status.success(), "create test junction");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert workspace sid") };
        unsafe { ensure_allow_write_aces_on_tree(&workspace, &[psid]) }
            .expect("refresh writable tree");
        let outside_allowed =
            path_mask_allows(&outside, &[psid], DELETE, true).expect("check outside target");
        unsafe { add_deny_write_ace_on_tree(&junction, psid) }
            .expect("protect junction object without following target");
        let junction_denied = unsafe { path_has_write_deny_for_sid(&junction, psid) }
            .expect("check junction object deny");
        let outside_denied = unsafe { path_has_write_deny_for_sid(&outside, psid) }
            .expect("check junction target deny");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(
            !outside_allowed,
            "junction target must remain outside the grant scope"
        );
        assert!(
            junction_denied,
            "protected junction object must receive an explicit deny"
        );
        assert!(
            !outside_denied,
            "protected junction target must not receive the deny"
        );
    }

    #[test]
    fn write_root_refresh_rejects_hard_link_without_changing_outside_acl() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside.txt");
        let linked = workspace.join("linked.txt");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::write(&outside, b"outside").expect("create outside file");
        fs::hard_link(&outside, &linked).expect("create in-root hard link");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert workspace sid") };
        let error = unsafe { ensure_allow_write_aces_on_tree(&workspace, &[psid]) }
            .expect_err("hard-linked descendant must fail closed");
        let outside_allowed =
            path_mask_allows(&outside, &[psid], DELETE, true).expect("check outside allow ACL");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(
            error
                .to_string()
                .contains("unsupported hard-linked writable file")
        );
        assert!(
            !outside_allowed,
            "outside hard-link target ACL must remain unchanged"
        );
    }

    #[test]
    fn deny_tree_rejects_hard_link_without_changing_outside_acl() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let protected = workspace.join(".codex");
        let outside = temp.path().join("outside.txt");
        let linked = protected.join("linked.txt");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&protected).expect("create protected tree");
        fs::write(&outside, b"outside").expect("create outside file");
        fs::hard_link(&outside, &linked).expect("create protected hard link");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert workspace sid") };
        let error = unsafe { add_deny_write_ace_on_tree(&protected, psid) }
            .expect_err("protected hard link must fail closed");
        let outside_denied =
            unsafe { path_has_write_deny_for_sid(&outside, psid) }.expect("check outside deny ACL");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(
            error
                .to_string()
                .contains("unsupported hard-linked protected file")
        );
        assert!(
            !outside_denied,
            "outside hard-link target ACL must remain unchanged"
        );
    }

    #[test]
    fn acl_transaction_rolls_back_first_allow_root_when_second_root_is_unsafe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let safe_root = workspace.join("safe");
        let unsafe_parent = workspace.join("unsafe-parent");
        let unsafe_root = unsafe_parent.join("unsafe");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&safe_root).expect("create safe root");
        fs::create_dir_all(&unsafe_root).expect("create unsafe root");
        let before = path_dacl_bytes(&safe_root).expect("capture safe root ACL");
        let protected_before = path_dacl_is_protected(&safe_root).expect("safe root protection");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &safe_root)
            .expect("safe root sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert safe root sid") };
        unsafe { ensure_allow_mask_aces(&unsafe_parent, &[psid], 0x1000_0000) }
            .expect("seed inherited GENERIC_ALL on second root");
        let mut transaction = AclTreeTransaction::new();
        unsafe {
            ensure_allow_write_aces_on_tree_in_transaction(&safe_root, &[psid], &mut transaction)
        }
        .expect("mutate safe first root");
        let error = unsafe {
            ensure_allow_write_aces_on_tree_in_transaction(&unsafe_root, &[psid], &mut transaction)
        }
        .expect_err("stale inherited second root must fail");
        transaction.rollback_now().expect("roll back first root");
        let after = path_dacl_bytes(&safe_root).expect("capture rolled back ACL");
        let protected_after = path_dacl_is_protected(&safe_root).expect("rolled back protection");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(
            error
                .to_string()
                .contains("unsupported inherited FILE_DELETE_CHILD")
        );
        assert_eq!(
            after, before,
            "first root DACL must be restored byte-for-byte"
        );
        assert_eq!(protected_after, protected_before);
    }

    #[test]
    fn acl_transaction_rolls_back_allows_when_deny_tree_is_unsafe() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let safe_root = workspace.join("safe");
        let protected = workspace.join(".codex");
        let outside = temp.path().join("outside.txt");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&safe_root).expect("create safe root");
        fs::create_dir_all(&protected).expect("create protected root");
        fs::write(&outside, b"outside").expect("create outside file");
        fs::hard_link(&outside, protected.join("linked.txt")).expect("create protected link");
        let before = path_dacl_bytes(&safe_root).expect("capture safe root ACL");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &safe_root)
            .expect("safe root sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert safe root sid") };
        let mut transaction = AclTreeTransaction::new();
        unsafe {
            ensure_allow_write_aces_on_tree_in_transaction(&safe_root, &[psid], &mut transaction)
        }
        .expect("mutate safe allow root");
        let error = unsafe {
            add_deny_write_ace_on_tree_in_transaction(&protected, psid, &mut transaction)
        }
        .expect_err("unsafe deny tree must fail");
        drop(transaction);
        let after = path_dacl_bytes(&safe_root).expect("capture rolled back allow ACL");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(
            error
                .to_string()
                .contains("unsupported hard-linked protected file")
        );
        assert_eq!(
            after, before,
            "allow DACL must roll back after deny failure"
        );
    }

    #[test]
    fn acl_transaction_detects_link_injected_after_preflight_and_rolls_back() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let safe_root = workspace.join("safe");
        let file = safe_root.join("file.txt");
        let injected_alias = temp.path().join("injected-alias.txt");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&safe_root).expect("create safe root");
        fs::write(&file, b"safe").expect("create safe file");
        let before_root = path_dacl_bytes(&safe_root).expect("capture root ACL");
        let before_file = path_dacl_bytes(&file).expect("capture file ACL");

        let sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &safe_root)
            .expect("safe root sid");
        let psid = unsafe { convert_string_sid_to_sid(&sid).expect("convert safe root sid") };
        let mut transaction = AclTreeTransaction::new();
        unsafe {
            ensure_allow_write_aces_on_tree_in_transaction(&safe_root, &[psid], &mut transaction)
        }
        .expect("apply transaction before injection");
        fs::hard_link(&file, &injected_alias).expect("inject out-of-root alias after preflight");
        let error = transaction
            .commit()
            .expect_err("commit must detect injected hard link");
        let after_root = path_dacl_bytes(&safe_root).expect("capture rolled back root ACL");
        let after_file = path_dacl_bytes(&file).expect("capture rolled back file ACL");
        unsafe { LocalFree(psid as HLOCAL) };

        assert!(error.to_string().contains("identity or link count changed"));
        assert_eq!(after_root, before_root);
        assert_eq!(after_file, before_file);
    }

    #[test]
    fn deny_path_under_active_root_uses_only_matching_root_sid() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let active_root = temp.path().join("active-root");
        let stale_root = temp.path().join("stale-root");
        let deny_path = active_root.join("protected");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&active_root).expect("create active root");
        fs::create_dir_all(&stale_root).expect("create stale root");
        fs::create_dir_all(&deny_path).expect("create deny path");

        let stale_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &stale_root)
            .expect("stale sid");
        let active_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &active_root)
            .expect("active sid");
        let workspace_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let caps = load_or_create_cap_sids(&nano_home).expect("load caps");

        let deny_sids = workspace_write_cap_sids_for_path(
            &nano_home,
            &workspace,
            &[workspace.clone(), active_root],
            &deny_path,
        )
        .expect("deny sids");

        assert_eq!(deny_sids, vec![active_sid]);
        assert!(!deny_sids.contains(&workspace_sid));
        assert!(!deny_sids.contains(&stale_sid));
        assert!(!deny_sids.contains(&caps.workspace));
    }

    #[test]
    fn deny_path_outside_active_roots_falls_back_to_all_active_root_sids() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let active_root = temp.path().join("active-root");
        let stale_root = temp.path().join("stale-root");
        let deny_path = temp.path().join("outside-deny");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&active_root).expect("create active root");
        fs::create_dir_all(&stale_root).expect("create stale root");
        fs::create_dir_all(&deny_path).expect("create deny path");

        let stale_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &stale_root)
            .expect("stale sid");
        let active_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &active_root)
            .expect("active sid");
        let workspace_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let caps = load_or_create_cap_sids(&nano_home).expect("load caps");

        let deny_sids = workspace_write_cap_sids_for_path(
            &nano_home,
            &workspace,
            &[workspace.clone(), active_root],
            &deny_path,
        )
        .expect("deny sids");

        assert_eq!(deny_sids.len(), 2);
        assert!(deny_sids.contains(&workspace_sid));
        assert!(deny_sids.contains(&active_sid));
        assert!(!deny_sids.contains(&stale_sid));
        assert!(!deny_sids.contains(&caps.workspace));
    }

    #[test]
    fn deny_path_includes_nested_active_root_sid() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let workspace = temp.path().join("workspace");
        let protected_dir = workspace.join(".codex");
        let nested_root = protected_dir.join("nested-root");
        fs::create_dir_all(&nano_home).expect("create codex home");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&nested_root).expect("create nested root");

        let workspace_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &workspace)
            .expect("workspace sid");
        let nested_sid = workspace_write_cap_sid_for_root(&nano_home, &workspace, &nested_root)
            .expect("nested sid");

        let deny_sids = workspace_write_cap_sids_for_path(
            &nano_home,
            &workspace,
            &[workspace.clone(), nested_root],
            &protected_dir,
        )
        .expect("deny sids");

        assert_eq!(deny_sids, vec![workspace_sid, nested_sid]);
    }

    #[test]
    fn remove_sandbox_log_dir_removes_dir_with_open_log_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join(".nano");
        let sbx_dir = super::sandbox_dir(&nano_home);
        fs::create_dir_all(&sbx_dir).expect("create sandbox dir");
        let log_path = sbx_dir.join("sandbox.2026-08-10.log");
        // Simulate the helper's own still-open log handle: std opens files
        // with delete-sharing, so the removal marks the open file
        // delete-pending and the dir can go away before the handle closes.
        let open_log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("open log file");
        let mut log: Vec<u8> = Vec::new();

        super::remove_sandbox_log_dir(&nano_home, &sbx_dir, &mut log).expect("remove log dir");

        assert!(!sbx_dir.exists());
        drop(open_log);
    }

    #[test]
    fn remove_sandbox_log_dir_refuses_non_nano_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join("codex-home");
        let sbx_dir = super::sandbox_dir(&nano_home);
        fs::create_dir_all(&sbx_dir).expect("create sandbox dir");
        let mut log: Vec<u8> = Vec::new();

        let err = super::remove_sandbox_log_dir(&nano_home, &sbx_dir, &mut log)
            .expect_err("non-.nano home must abort");

        assert!(
            err.to_string()
                .contains("refusing to remove sandbox log dir")
        );
        assert!(sbx_dir.exists());
    }

    #[test]
    fn remove_sandbox_log_dir_tolerates_missing_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path().join(".nano");
        let sbx_dir = super::sandbox_dir(&nano_home);
        let mut log: Vec<u8> = Vec::new();

        super::remove_sandbox_log_dir(&nano_home, &sbx_dir, &mut log)
            .expect("missing dir is idempotent-ok");
    }
}
