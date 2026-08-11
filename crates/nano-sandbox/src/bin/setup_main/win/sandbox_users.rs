//! Provenance: ported from Codex `windows-sandbox-rs/src/bin/setup_main` @
//! 646f7c0a. Transformations: codex_windows_sandbox -> nano_sandbox;
//! codex_otel -> telemetry facade; codex_home -> nano_home.

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rand::RngCore;
use rand::SeedableRng;
use rand::rngs::SmallRng;
use serde::Deserialize;
use serde::Serialize;
use std::ffi::OsStr;
use std::ffi::c_void;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::Foundation::GENERIC_WRITE;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::NetworkManagement::NetManagement::LOCALGROUP_INFO_1;
use windows_sys::Win32::NetworkManagement::NetManagement::LOCALGROUP_MEMBERS_INFO_3;
use windows_sys::Win32::NetworkManagement::NetManagement::MAX_PREFERRED_LENGTH;
use windows_sys::Win32::NetworkManagement::NetManagement::NERR_Success;
use windows_sys::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows_sys::Win32::NetworkManagement::NetManagement::NetLocalGroupAdd;
use windows_sys::Win32::NetworkManagement::NetManagement::NetLocalGroupAddMembers;
use windows_sys::Win32::NetworkManagement::NetManagement::NetLocalGroupDel;
use windows_sys::Win32::NetworkManagement::NetManagement::NetLocalGroupGetMembers;
use windows_sys::Win32::NetworkManagement::NetManagement::NetUserAdd;
use windows_sys::Win32::NetworkManagement::NetManagement::NetUserDel;
use windows_sys::Win32::NetworkManagement::NetManagement::NetUserSetInfo;
use windows_sys::Win32::NetworkManagement::NetManagement::UF_DONT_EXPIRE_PASSWD;
use windows_sys::Win32::NetworkManagement::NetManagement::UF_SCRIPT;
use windows_sys::Win32::NetworkManagement::NetManagement::USER_INFO_1;
use windows_sys::Win32::NetworkManagement::NetManagement::USER_INFO_1003;
use windows_sys::Win32::NetworkManagement::NetManagement::USER_PRIV_USER;
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::Authorization::SDDL_REVISION_1;
use windows_sys::Win32::Security::CopySid;
use windows_sys::Win32::Security::GetLengthSid;
use windows_sys::Win32::Security::LookupAccountNameW;
use windows_sys::Win32::Security::LookupAccountSidW;
use windows_sys::Win32::Security::PSECURITY_DESCRIPTOR;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Security::SID_NAME_USE;
use windows_sys::Win32::Storage::FileSystem::CREATE_NEW;
use windows_sys::Win32::Storage::FileSystem::CreateFileW;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;

use nano_sandbox::SETUP_VERSION;
use nano_sandbox::SetupErrorCode;
use nano_sandbox::SetupFailure;
use nano_sandbox::dpapi_protect;
use nano_sandbox::sandbox_dir;
use nano_sandbox::sandbox_secrets_dir;
use nano_sandbox::string_from_sid_bytes;
use nano_sandbox::to_wide;

pub const SANDBOX_USERS_GROUP: &str = "NanoSandboxUsers";
const SANDBOX_USERS_GROUP_COMMENT: &str = "Wayland Nano sandbox internal group (managed)";
const SID_ADMINISTRATORS: &str = "S-1-5-32-544";
const SID_USERS: &str = "S-1-5-32-545";
const SID_AUTHENTICATED_USERS: &str = "S-1-5-11";
const SID_EVERYONE: &str = "S-1-1-0";
const SID_SYSTEM: &str = "S-1-5-18";

pub fn ensure_sandbox_users_group(log: &mut dyn Write) -> Result<()> {
    ensure_local_group(SANDBOX_USERS_GROUP, SANDBOX_USERS_GROUP_COMMENT, log)
}

pub fn resolve_sandbox_users_group_sid() -> Result<Vec<u8>> {
    resolve_sid(SANDBOX_USERS_GROUP)
}

pub fn provision_sandbox_users(
    nano_home: &Path,
    offline_username: &str,
    online_username: &str,
    log: &mut dyn Write,
) -> Result<()> {
    ensure_sandbox_users_group(log)?;
    super::log_line(
        log,
        &format!("ensuring sandbox users offline={offline_username} online={online_username}"),
    )?;
    let offline_password = random_password();
    let online_password = random_password();
    ensure_sandbox_user(offline_username, &offline_password, log)?;
    ensure_sandbox_user(online_username, &online_password, log)?;
    write_secrets(
        nano_home,
        offline_username,
        &offline_password,
        online_username,
        &online_password,
    )?;
    Ok(())
}

pub fn ensure_sandbox_user(username: &str, password: &str, log: &mut dyn Write) -> Result<()> {
    ensure_local_user(username, password, log)?;
    ensure_local_group_member(SANDBOX_USERS_GROUP, username)?;
    Ok(())
}

pub fn ensure_local_user(name: &str, password: &str, log: &mut dyn Write) -> Result<()> {
    let name_w = to_wide(OsStr::new(name));
    let pwd_w = to_wide(OsStr::new(password));
    unsafe {
        let info = USER_INFO_1 {
            usri1_name: name_w.as_ptr() as *mut u16,
            usri1_password: pwd_w.as_ptr() as *mut u16,
            usri1_password_age: 0,
            usri1_priv: USER_PRIV_USER,
            usri1_home_dir: std::ptr::null_mut(),
            usri1_comment: std::ptr::null_mut(),
            usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD,
            usri1_script_path: std::ptr::null_mut(),
        };
        let status = NetUserAdd(
            std::ptr::null(),
            1,
            &info as *const _ as *mut u8,
            std::ptr::null_mut(),
        );
        if status != NERR_Success {
            // Try update password via level 1003.
            let pw_info = USER_INFO_1003 {
                usri1003_password: pwd_w.as_ptr() as *mut u16,
            };
            let upd = NetUserSetInfo(
                std::ptr::null(),
                name_w.as_ptr(),
                1003,
                &pw_info as *const _ as *mut u8,
                std::ptr::null_mut(),
            );
            if upd != NERR_Success {
                super::log_line(log, &format!("NetUserSetInfo failed for {name} code {upd}"))?;
                return Err(anyhow::Error::new(SetupFailure::new(
                    SetupErrorCode::HelperUserCreateOrUpdateFailed,
                    format!("failed to create/update user {name}, code {status}/{upd}"),
                )));
            }
        }

        // Ensure the principal is a regular local user account.
        if let Ok(group_name) = lookup_account_name_for_sid(SID_USERS) {
            let group = to_wide(OsStr::new(&group_name));
            let member = LOCALGROUP_MEMBERS_INFO_3 {
                lgrmi3_domainandname: name_w.as_ptr() as *mut u16,
            };
            let _ = NetLocalGroupAddMembers(
                std::ptr::null(),
                group.as_ptr(),
                3,
                &member as *const _ as *mut u8,
                1,
            );
        } else {
            super::log_line(
                log,
                "LookupAccountSidW failed for Users SID; skipping Users group membership",
            )?;
        }
    }
    Ok(())
}

pub fn ensure_local_group(name: &str, comment: &str, log: &mut dyn Write) -> Result<()> {
    const ERROR_ALIAS_EXISTS: u32 = 1379;
    const NERR_GROUP_EXISTS: u32 = 2223;

    let name_w = to_wide(OsStr::new(name));
    let comment_w = to_wide(OsStr::new(comment));
    unsafe {
        let info = LOCALGROUP_INFO_1 {
            lgrpi1_name: name_w.as_ptr() as *mut u16,
            lgrpi1_comment: comment_w.as_ptr() as *mut u16,
        };
        let mut parm_err: u32 = 0;
        let status = NetLocalGroupAdd(
            std::ptr::null(),
            1,
            &info as *const _ as *mut u8,
            &mut parm_err as *mut _,
        );
        if status != NERR_Success && status != ERROR_ALIAS_EXISTS && status != NERR_GROUP_EXISTS {
            super::log_line(
                log,
                &format!("NetLocalGroupAdd failed for {name} code {status} parm_err={parm_err}"),
            )?;
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperUsersGroupCreateFailed,
                format!("failed to create local group {name}, code {status}"),
            )));
        }
    }
    Ok(())
}

pub fn ensure_local_group_member(group_name: &str, member_name: &str) -> Result<()> {
    // If the member is already in the group, NetLocalGroupAddMembers may
    // return an error code. We don't care.
    let group_w = to_wide(OsStr::new(group_name));
    let member_w = to_wide(OsStr::new(member_name));
    unsafe {
        let member = LOCALGROUP_MEMBERS_INFO_3 {
            lgrmi3_domainandname: member_w.as_ptr() as *mut u16,
        };
        let _ = NetLocalGroupAddMembers(
            std::ptr::null(),
            group_w.as_ptr(),
            3,
            &member as *const _ as *mut u8,
            1,
        );
    }
    Ok(())
}

/// True when `username` is a Wayland Nano sandbox account name. Uninstall
/// refuses to delete anything else: Track A's accounts use a different
/// prefix, and the retired NanoK3* codename identities are rejected
/// explicitly so stale pre-rebrand state can never match.
fn is_nano_sandbox_account(username: &str) -> bool {
    username.starts_with("NanoSandbox") && !username.starts_with("NanoK3")
}

/// True when a `DOMAIN\name` group-member entry names one of the expected
/// provisioned accounts (comparison is case-insensitive, domain-agnostic).
fn member_name_matches_expected(domain_and_name: &str, expected: &[&str]) -> bool {
    let name = domain_and_name
        .rsplit('\\')
        .next()
        .unwrap_or(domain_and_name);
    expected.iter().any(|e| name.eq_ignore_ascii_case(e))
}

/// Removes the Track-B sandbox accounts and the sandbox users group.
///
/// Track-B addition (the donor has no teardown). Fail-closed: account names
/// must carry the NanoSandbox prefix, and the group is deleted only when
/// every current member is one of the two provisioned accounts — any other
/// content means the group is not exactly what provisioning created and the
/// run aborts instead of deleting foreign state. Missing accounts/group are
/// skipped (idempotent).
pub fn remove_sandbox_users(
    offline_username: &str,
    online_username: &str,
    log: &mut dyn Write,
) -> Result<()> {
    let expected_members = [offline_username, online_username];
    ensure_group_membership_as_provisioned(&expected_members)?;
    for username in expected_members {
        if !is_nano_sandbox_account(username) {
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperUserDeleteFailed,
                format!(
                    "refusing to delete account {username}: not a Wayland Nano sandbox account"
                ),
            )));
        }
        delete_local_user(username, log)?;
    }
    delete_sandbox_users_group(log)
}

/// Verifies the sandbox users group contains only the provisioned accounts
/// (or does not exist). Anything else aborts the uninstall.
fn ensure_group_membership_as_provisioned(expected_members: &[&str; 2]) -> Result<()> {
    let members = local_group_member_names(SANDBOX_USERS_GROUP)?;
    let unexpected = members
        .iter()
        .filter(|member| !member_name_matches_expected(member, expected_members))
        .cloned()
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUsersGroupDeleteFailed,
            format!(
                "refusing to delete group {SANDBOX_USERS_GROUP}: unexpected members {unexpected:?}"
            ),
        )));
    }
    Ok(())
}

/// Enumerates `DOMAIN\name` members of a local group; an absent group yields
/// an empty list.
fn local_group_member_names(group_name: &str) -> Result<Vec<String>> {
    const NERR_GROUP_NOT_FOUND: u32 = 2220;
    const ERROR_NO_SUCH_ALIAS: u32 = 1376;
    const ERROR_MORE_DATA: u32 = 234;

    let group_w = to_wide(OsStr::new(group_name));
    let mut members = Vec::new();
    let mut resume: usize = 0;
    loop {
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut entries_read: u32 = 0;
        let mut total_entries: u32 = 0;
        let status = unsafe {
            NetLocalGroupGetMembers(
                std::ptr::null(),
                group_w.as_ptr(),
                3,
                &mut buf,
                MAX_PREFERRED_LENGTH,
                &mut entries_read,
                &mut total_entries,
                &mut resume,
            )
        };
        if status == NERR_GROUP_NOT_FOUND || status == ERROR_NO_SUCH_ALIAS {
            return Ok(members);
        }
        if status != NERR_Success && status != ERROR_MORE_DATA {
            if !buf.is_null() {
                unsafe {
                    NetApiBufferFree(buf as *const c_void);
                }
            }
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperUsersGroupDeleteFailed,
                format!("NetLocalGroupGetMembers failed for {group_name}, code {status}"),
            )));
        }
        if !buf.is_null() {
            let infos = unsafe {
                std::slice::from_raw_parts(
                    buf as *const LOCALGROUP_MEMBERS_INFO_3,
                    entries_read as usize,
                )
            };
            for info in infos {
                members.push(unsafe { string_from_wide(info.lgrmi3_domainandname) });
            }
            unsafe {
                NetApiBufferFree(buf as *const c_void);
            }
        }
        if status != ERROR_MORE_DATA {
            break;
        }
    }
    Ok(members)
}

/// Track-B addition (the donor has no teardown): removes the Winlogon
/// SpecialAccounts\UserList values provisioning created to keep the sandbox
/// accounts off the login screen. Fail-closed: only values named exactly
/// after NanoSandbox* accounts are touched — a non-Wayland Nano name aborts
/// before any registry write, so Track A's CodexSandbox* values are never
/// matched. Missing key/values are tolerated (idempotent).
pub fn remove_hide_user_entries(
    offline_username: &str,
    online_username: &str,
    log: &mut dyn Write,
) -> Result<()> {
    for username in [offline_username, online_username] {
        if !is_nano_sandbox_account(username) {
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperUninstallFailed,
                format!(
                    "refusing to remove UserList value for {username}: not a Wayland Nano sandbox account"
                ),
            )));
        }
    }
    nano_sandbox::remove_userlist_entries(&[offline_username, online_username]).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!("remove Winlogon UserList entries failed: {err}"),
        ))
    })?;
    super::log_line(
        log,
        &format!("removed Winlogon UserList values for {offline_username}, {online_username}"),
    )?;
    Ok(())
}

/// Minimal parse shape for verifying a secrets file before deletion. Only the
/// account names are inspected; the DPAPI blobs are never decoded here.
#[derive(Deserialize)]
struct SandboxUsersFileProbe {
    offline: SandboxUserRecordProbe,
    online: SandboxUserRecordProbe,
}

#[derive(Deserialize)]
struct SandboxUserRecordProbe {
    username: String,
}

/// True only when the file parses as a sandbox users file AND both usernames
/// are exactly the provisioned NanoSandbox* accounts. Anything else
/// (malformed JSON, Track-A/foreign accounts, a mismatched pair) means the
/// file is not what provisioning wrote for this payload.
fn secrets_file_matches_provisioned(bytes: &[u8], offline: &str, online: &str) -> bool {
    let parsed: SandboxUsersFileProbe = match serde_json::from_slice(bytes) {
        Ok(parsed) => parsed,
        Err(_) => return false,
    };
    parsed.offline.username == offline
        && parsed.online.username == online
        && is_nano_sandbox_account(&parsed.offline.username)
        && is_nano_sandbox_account(&parsed.online.username)
}

/// Track-B addition (the donor has no teardown): removes the DPAPI credential
/// file provisioning wrote for the sandbox accounts. Fail-closed: the file is
/// parsed and BOTH usernames must be exactly the provisioned NanoSandbox*
/// accounts before anything is deleted — a file whose contents do not match
/// is foreign state and aborts the uninstall untouched. The containing
/// `.sandbox-secrets` dir is removed only when left empty (other content such
/// as the `creds` subdir is out of uninstall scope). A missing file is
/// tolerated (idempotent).
pub fn remove_sandbox_secrets(
    nano_home: &Path,
    offline_username: &str,
    online_username: &str,
    log: &mut dyn Write,
) -> Result<()> {
    let users_path = sandbox_secrets_dir(nano_home).join("sandbox_users.json");
    let bytes = match std::fs::read(&users_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperUninstallFailed,
                format!(
                    "read sandbox secrets file {} failed: {err}",
                    users_path.display()
                ),
            )));
        }
    };
    if !secrets_file_matches_provisioned(&bytes, offline_username, online_username) {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!(
                "refusing to delete {}: contents are not the provisioned Wayland Nano sandbox accounts",
                users_path.display()
            ),
        )));
    }
    std::fs::remove_file(&users_path).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUninstallFailed,
            format!(
                "remove sandbox secrets file {} failed: {err}",
                users_path.display()
            ),
        ))
    })?;
    super::log_line(
        log,
        &format!("removed sandbox secrets file {}", users_path.display()),
    )?;
    let secrets_dir = sandbox_secrets_dir(nano_home);
    // Only an empty dir is removed; anything else (non-empty, locked) leaves
    // the dir in place — only the verified secrets file is in uninstall scope.
    if let Ok(()) = std::fs::remove_dir(&secrets_dir) {
        super::log_line(
            log,
            &format!("removed empty secrets dir {}", secrets_dir.display()),
        )?;
    }
    Ok(())
}

fn delete_local_user(username: &str, log: &mut dyn Write) -> Result<()> {
    const NERR_USER_NOT_FOUND: u32 = 2221;

    let name_w = to_wide(OsStr::new(username));
    let status = unsafe { NetUserDel(std::ptr::null(), name_w.as_ptr()) };
    if status == NERR_Success {
        super::log_line(log, &format!("deleted sandbox user {username}"))?;
        return Ok(());
    }
    if status == NERR_USER_NOT_FOUND {
        return Ok(());
    }
    Err(anyhow::Error::new(SetupFailure::new(
        SetupErrorCode::HelperUserDeleteFailed,
        format!("NetUserDel failed for {username}, code {status}"),
    )))
}

fn delete_sandbox_users_group(log: &mut dyn Write) -> Result<()> {
    const NERR_GROUP_NOT_FOUND: u32 = 2220;
    const ERROR_NO_SUCH_ALIAS: u32 = 1376;

    let name_w = to_wide(OsStr::new(SANDBOX_USERS_GROUP));
    let status = unsafe { NetLocalGroupDel(std::ptr::null(), name_w.as_ptr()) };
    if status == NERR_Success {
        super::log_line(log, &format!("deleted local group {SANDBOX_USERS_GROUP}"))?;
        return Ok(());
    }
    if status == NERR_GROUP_NOT_FOUND || status == ERROR_NO_SUCH_ALIAS {
        return Ok(());
    }
    Err(anyhow::Error::new(SetupFailure::new(
        SetupErrorCode::HelperUsersGroupDeleteFailed,
        format!("NetLocalGroupDel failed for {SANDBOX_USERS_GROUP}, code {status}"),
    )))
}

/// Reads a null-terminated UTF-16 string returned by the Net* APIs.
unsafe fn string_from_wide(ptr: *mut u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

pub fn resolve_sid(name: &str) -> Result<Vec<u8>> {
    if let Some(sid_str) = well_known_sid_str(name) {
        return sid_bytes_from_string(sid_str);
    }
    let name_w = to_wide(OsStr::new(name));
    let mut sid_buffer = vec![0u8; 68];
    let mut sid_len: u32 = sid_buffer.len() as u32;
    let mut domain: Vec<u16> = Vec::new();
    let mut domain_len: u32 = 0;
    let mut use_type: SID_NAME_USE = 0;
    loop {
        let ok = unsafe {
            LookupAccountNameW(
                std::ptr::null(),
                name_w.as_ptr(),
                sid_buffer.as_mut_ptr() as *mut c_void,
                &mut sid_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut use_type,
            )
        };
        if ok != 0 {
            sid_buffer.truncate(sid_len as usize);
            return Ok(sid_buffer);
        }
        let err = unsafe { GetLastError() };
        if err == ERROR_INSUFFICIENT_BUFFER {
            sid_buffer.resize(sid_len as usize, 0);
            domain.resize(domain_len as usize, 0);
            continue;
        }
        return Err(anyhow::anyhow!(
            "LookupAccountNameW failed for {name}: {err}"
        ));
    }
}

fn well_known_sid_str(name: &str) -> Option<&'static str> {
    match name {
        "Administrators" => Some(SID_ADMINISTRATORS),
        "Users" => Some(SID_USERS),
        "Authenticated Users" => Some(SID_AUTHENTICATED_USERS),
        "Everyone" => Some(SID_EVERYONE),
        "SYSTEM" => Some(SID_SYSTEM),
        _ => None,
    }
}

fn sid_bytes_from_string(sid_str: &str) -> Result<Vec<u8>> {
    let sid_w = to_wide(OsStr::new(sid_str));
    let mut psid: *mut c_void = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(sid_w.as_ptr(), &mut psid) } == 0 {
        return Err(anyhow::anyhow!(
            "ConvertStringSidToSidW failed for {sid_str}: {}",
            unsafe { GetLastError() }
        ));
    }
    let sid_len = unsafe { GetLengthSid(psid) };
    if sid_len == 0 {
        unsafe {
            LocalFree(psid as _);
        }
        return Err(anyhow::anyhow!("GetLengthSid failed for {sid_str}"));
    }
    let mut out = vec![0u8; sid_len as usize];
    let ok = unsafe { CopySid(sid_len, out.as_mut_ptr() as *mut c_void, psid) };
    unsafe {
        LocalFree(psid as _);
    }
    if ok == 0 {
        return Err(anyhow::anyhow!("CopySid failed for {sid_str}"));
    }
    Ok(out)
}

fn lookup_account_name_for_sid(sid_str: &str) -> Result<String> {
    let sid_w = to_wide(OsStr::new(sid_str));
    let mut psid: *mut c_void = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(sid_w.as_ptr(), &mut psid) } == 0 {
        return Err(anyhow::anyhow!(
            "ConvertStringSidToSidW failed for {sid_str}: {}",
            unsafe { GetLastError() }
        ));
    }
    let mut name_len: u32 = 0;
    let mut domain_len: u32 = 0;
    let mut use_type: SID_NAME_USE = 0;
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            psid,
            std::ptr::null_mut(),
            &mut name_len,
            std::ptr::null_mut(),
            &mut domain_len,
            &mut use_type,
        )
    };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        if err != ERROR_INSUFFICIENT_BUFFER {
            unsafe {
                LocalFree(psid as _);
            }
            return Err(anyhow::anyhow!(
                "LookupAccountSidW preflight failed for {sid_str}: {err}"
            ));
        }
    }
    let mut name_buf: Vec<u16> = vec![0u16; name_len as usize];
    let mut domain_buf: Vec<u16> = vec![0u16; domain_len as usize];
    let ok = unsafe {
        LookupAccountSidW(
            std::ptr::null(),
            psid,
            name_buf.as_mut_ptr(),
            &mut name_len,
            domain_buf.as_mut_ptr(),
            &mut domain_len,
            &mut use_type,
        )
    };
    unsafe {
        LocalFree(psid as _);
    }
    if ok == 0 {
        return Err(anyhow::anyhow!(
            "LookupAccountSidW failed for {sid_str}: {}",
            unsafe { GetLastError() }
        ));
    }
    let name = String::from_utf16_lossy(&name_buf);
    Ok(name.trim_end_matches('\0').to_string())
}

pub fn sid_bytes_to_psid(sid: &[u8]) -> Result<*mut c_void> {
    let sid_str = string_from_sid_bytes(sid).map_err(anyhow::Error::msg)?;
    let sid_w = to_wide(OsStr::new(&sid_str));
    let mut psid: *mut c_void = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(sid_w.as_ptr(), &mut psid) } == 0 {
        return Err(anyhow::anyhow!(
            "ConvertStringSidToSidW failed: {}",
            unsafe { GetLastError() }
        ));
    }
    Ok(psid)
}

fn random_password() -> String {
    const CHARS: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()-_=+";
    let mut rng = SmallRng::from_entropy();
    let mut buf = [0u8; 24];
    rng.fill_bytes(&mut buf);
    buf.iter()
        .map(|b| {
            let idx = (*b as usize) % CHARS.len();
            CHARS[idx] as char
        })
        .collect()
}

#[derive(Serialize)]
struct SandboxUserRecord {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SandboxUsersFile {
    version: u32,
    offline: SandboxUserRecord,
    online: SandboxUserRecord,
}

#[derive(Serialize)]
struct SetupMarker {
    version: u32,
    offline_username: String,
    online_username: String,
    created_at: String,
    proxy_ports: Vec<u16>,
    allow_local_binding: bool,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
}

fn write_secrets(
    nano_home: &Path,
    offline_user: &str,
    offline_pwd: &str,
    online_user: &str,
    online_pwd: &str,
) -> Result<()> {
    let secrets_dir = sandbox_secrets_dir(nano_home);
    std::fs::create_dir_all(&secrets_dir).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUsersFileWriteFailed,
            format!(
                "failed to create secrets dir {}: {err}",
                secrets_dir.display()
            ),
        ))
    })?;
    let offline_blob = dpapi_protect(offline_pwd.as_bytes()).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperDpapiProtectFailed,
            format!("dpapi protect failed for offline user: {err}"),
        ))
    })?;
    let online_blob = dpapi_protect(online_pwd.as_bytes()).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperDpapiProtectFailed,
            format!("dpapi protect failed for online user: {err}"),
        ))
    })?;
    let users = SandboxUsersFile {
        version: SETUP_VERSION,
        offline: SandboxUserRecord {
            username: offline_user.to_string(),
            password: BASE64.encode(offline_blob),
        },
        online: SandboxUserRecord {
            username: online_user.to_string(),
            password: BASE64.encode(online_blob),
        },
    };
    let users_path = secrets_dir.join("sandbox_users.json");
    let users_json = serde_json::to_vec_pretty(&users).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUsersFileWriteFailed,
            format!("serialize sandbox users failed: {err}"),
        ))
    })?;
    std::fs::write(&users_path, users_json).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperUsersFileWriteFailed,
            format!(
                "write sandbox users file {} failed: {err}",
                users_path.display()
            ),
        ))
    })?;
    Ok(())
}

// Create the final marker path with its protected ACL before provisioning begins. The empty file
// intentionally fails readiness checks while setup is in progress, and sandbox users cannot read,
// modify, or replace it. Once every setup step succeeds, `commit_setup_marker` writes the valid
// marker contents without changing the file's ACL.
pub(super) fn prepare_setup_marker(nano_home: &Path, real_user: &str) -> Result<()> {
    let marker_path = sandbox_dir(nano_home).join("setup_marker.json");
    match std::fs::remove_file(&marker_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperSetupMarkerWriteFailed,
                format!(
                    "remove setup marker file {} failed: {err}",
                    marker_path.display()
                ),
            )));
        }
    }

    let real_user_sid = resolve_sid(real_user)
        .and_then(|sid| string_from_sid_bytes(&sid).map_err(anyhow::Error::msg))
        .map_err(|err| {
            anyhow::Error::new(SetupFailure::new(
                SetupErrorCode::HelperSetupMarkerWriteFailed,
                format!("resolve real user SID for setup marker failed: {err}"),
            ))
        })?;
    let sddl = to_wide(format!(
        "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{real_user_sid})"
    ));
    let mut security_descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut security_descriptor,
            std::ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSetupMarkerWriteFailed,
            format!(
                "create setup marker security descriptor failed: {}",
                unsafe { GetLastError() }
            ),
        )));
    }

    let security_attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security_descriptor,
        bInheritHandle: 0,
    };
    let marker_path_wide = to_wide(marker_path.as_os_str());
    let marker_handle = unsafe {
        CreateFileW(
            marker_path_wide.as_ptr(),
            GENERIC_WRITE,
            /*dwsharemode*/ 0,
            &security_attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            /*htemplatefile*/ 0,
        )
    };
    let create_error = unsafe { GetLastError() };
    unsafe {
        LocalFree(security_descriptor as _);
    }
    if marker_handle == INVALID_HANDLE_VALUE {
        return Err(anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSetupMarkerWriteFailed,
            format!(
                "create protected setup marker file {} failed: {}",
                marker_path.display(),
                create_error
            ),
        )));
    }
    unsafe {
        CloseHandle(marker_handle);
    }
    Ok(())
}

pub(super) fn commit_setup_marker(
    nano_home: &Path,
    offline_user: &str,
    online_user: &str,
    proxy_ports: &[u16],
    allow_local_binding: bool,
) -> Result<()> {
    let marker = SetupMarker {
        version: SETUP_VERSION,
        offline_username: offline_user.to_string(),
        online_username: online_user.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        proxy_ports: proxy_ports.to_vec(),
        allow_local_binding,
        read_roots: Vec::new(),
        write_roots: Vec::new(),
    };
    let marker_path = sandbox_dir(nano_home).join("setup_marker.json");
    let marker_json = serde_json::to_vec_pretty(&marker).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSetupMarkerWriteFailed,
            format!("serialize setup marker failed: {err}"),
        ))
    })?;
    std::fs::write(&marker_path, marker_json).map_err(|err| {
        anyhow::Error::new(SetupFailure::new(
            SetupErrorCode::HelperSetupMarkerWriteFailed,
            format!(
                "write setup marker file {} failed: {err}",
                marker_path.display()
            ),
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_nano_sandbox_account;
    use super::member_name_matches_expected;
    use super::remove_hide_user_entries;
    use super::remove_sandbox_secrets;
    use super::secrets_file_matches_provisioned;

    const OFFLINE: &str = "NanoSandboxOffline";
    const ONLINE: &str = "NanoSandboxOnline";

    fn secrets_json(offline: &str, online: &str) -> Vec<u8> {
        // Placeholder blobs only — no real credentials in tests.
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 5,
            "offline": { "username": offline, "password": "cGxhY2Vob2xkZXI=" },
            "online": { "username": online, "password": "cGxhY2Vob2xkZXI=" },
        }))
        .expect("serialize secrets")
    }

    #[test]
    fn nano_account_guard_accepts_nano_sandbox_names() {
        assert!(is_nano_sandbox_account("NanoSandboxOffline"));
        assert!(is_nano_sandbox_account("NanoSandboxOnline"));
    }

    #[test]
    fn nano_account_guard_rejects_nanok3_track_a_and_foreign_names() {
        // Retired NanoK3* codename identities, Track-A, and built-in accounts
        // must never pass the uninstall guard.
        assert!(!is_nano_sandbox_account("NanoK3SandboxOffline"));
        assert!(!is_nano_sandbox_account("NanoK3SandboxOnline"));
        assert!(!is_nano_sandbox_account("NanoK3SandboxUsers"));
        assert!(!is_nano_sandbox_account("CodexSandboxOffline"));
        assert!(!is_nano_sandbox_account("Administrator"));
        assert!(!is_nano_sandbox_account("nanosandboxoffline"));
    }

    #[test]
    fn member_match_strips_domain_and_ignores_case() {
        let expected = ["NanoSandboxOffline", "NanoSandboxOnline"];
        assert!(member_name_matches_expected(
            "DESKTOP\\NanoSandboxOffline",
            &expected
        ));
        assert!(member_name_matches_expected("nanosandboxonline", &expected));
        assert!(!member_name_matches_expected(
            "DESKTOP\\CodexSandboxOffline",
            &expected
        ));
        assert!(!member_name_matches_expected(
            "DESKTOP\\Administrator",
            &expected
        ));
    }

    #[test]
    fn secrets_probe_accepts_exact_provisioned_pair() {
        let bytes = secrets_json(OFFLINE, ONLINE);
        assert!(secrets_file_matches_provisioned(&bytes, OFFLINE, ONLINE));
    }

    #[test]
    fn secrets_probe_rejects_foreign_and_malformed_content() {
        // Track-A accounts must never verify.
        assert!(!secrets_file_matches_provisioned(
            &secrets_json("CodexSandboxOffline", "CodexSandboxOnline"),
            OFFLINE,
            ONLINE
        ));
        // A different Wayland Nano pair than the payload's is stale foreign state.
        assert!(!secrets_file_matches_provisioned(
            &secrets_json(OFFLINE, "NanoSandboxOther"),
            OFFLINE,
            ONLINE
        ));
        // Swapped order is not what provisioning wrote.
        assert!(!secrets_file_matches_provisioned(
            &secrets_json(ONLINE, OFFLINE),
            OFFLINE,
            ONLINE
        ));
        // Malformed JSON does not verify.
        assert!(!secrets_file_matches_provisioned(
            b"not json",
            OFFLINE,
            ONLINE
        ));
    }

    #[test]
    fn remove_sandbox_secrets_deletes_verified_file_and_empty_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path();
        let secrets_dir = nano_sandbox::sandbox_secrets_dir(nano_home);
        std::fs::create_dir_all(&secrets_dir).expect("create secrets dir");
        let users_path = secrets_dir.join("sandbox_users.json");
        std::fs::write(&users_path, secrets_json(OFFLINE, ONLINE)).expect("write secrets");
        let mut log: Vec<u8> = Vec::new();

        remove_sandbox_secrets(nano_home, OFFLINE, ONLINE, &mut log).expect("remove secrets");

        assert!(!users_path.exists());
        assert!(!secrets_dir.exists());
    }

    #[test]
    fn remove_sandbox_secrets_keeps_non_empty_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path();
        let secrets_dir = nano_sandbox::sandbox_secrets_dir(nano_home);
        let creds_dir = secrets_dir.join("creds");
        std::fs::create_dir_all(&creds_dir).expect("create creds dir");
        std::fs::write(creds_dir.join("other.json"), b"{}").expect("write other file");
        let users_path = secrets_dir.join("sandbox_users.json");
        std::fs::write(&users_path, secrets_json(OFFLINE, ONLINE)).expect("write secrets");
        let mut log: Vec<u8> = Vec::new();

        remove_sandbox_secrets(nano_home, OFFLINE, ONLINE, &mut log).expect("remove secrets");

        assert!(!users_path.exists());
        assert!(secrets_dir.exists());
        assert!(creds_dir.join("other.json").exists());
    }

    #[test]
    fn remove_sandbox_secrets_refuses_wrong_content_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nano_home = temp.path();
        let secrets_dir = nano_sandbox::sandbox_secrets_dir(nano_home);
        std::fs::create_dir_all(&secrets_dir).expect("create secrets dir");
        let users_path = secrets_dir.join("sandbox_users.json");
        // Track-A-shaped content must survive the uninstall untouched.
        std::fs::write(
            &users_path,
            secrets_json("CodexSandboxOffline", "CodexSandboxOnline"),
        )
        .expect("write secrets");
        let mut log: Vec<u8> = Vec::new();

        let err = remove_sandbox_secrets(nano_home, OFFLINE, ONLINE, &mut log)
            .expect_err("wrong-content file must abort");

        assert!(err.to_string().contains("refusing to delete"));
        assert!(users_path.exists());
        assert_eq!(
            std::fs::read(&users_path).expect("read secrets"),
            secrets_json("CodexSandboxOffline", "CodexSandboxOnline")
        );
    }

    #[test]
    fn remove_sandbox_secrets_tolerates_missing_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut log: Vec<u8> = Vec::new();

        remove_sandbox_secrets(temp.path(), OFFLINE, ONLINE, &mut log)
            .expect("missing secrets file is idempotent-ok");
    }

    #[test]
    fn remove_hide_user_entries_rejects_track_a_names_before_registry_write() {
        let mut log: Vec<u8> = Vec::new();

        let err = remove_hide_user_entries("CodexSandboxOffline", ONLINE, &mut log)
            .expect_err("Track-A name must abort");

        assert!(
            err.to_string()
                .contains("refusing to remove UserList value")
        );
    }
}
