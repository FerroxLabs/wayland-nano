//! DACL allow/deny enforcement for sandbox filesystem policy.
//!
//! Provenance: ported from Codex `windows-sandbox-rs/src/acl.rs` @ 646f7c0a,
//! plus the post-baseline donor-defect fixes from Track A commits `1dae2d8ae`,
//! `33f112c87`, `fa0ee4da3`, `9e0e88504`, and `1e3400144` (transactional ACL
//! rollback, root-vs-descendant refresh, strict fail-closed DACL parsers,
//! AccessCheck-verified legacy DELETE grant). Transformations: module path
//! only; codex_home -> nano_home naming; the quiescent-precondition constant is
//! renamed (`G0_` was a Track A phase name). ACE ordering (deny-before-allow)
//! and inheritance flags are unchanged.

use crate::winutil::to_wide;
use anyhow::Result;
use anyhow::anyhow;
use std::ffi::c_void;
use std::path::Path;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS;
use windows_sys::Win32::Foundation::DuplicateHandle;
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Foundation::HLOCAL;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::ACCESS_ALLOWED_ACE;
use windows_sys::Win32::Security::ACCESS_DENIED_ACE;
use windows_sys::Win32::Security::ACE_HEADER;
use windows_sys::Win32::Security::ACL;
use windows_sys::Win32::Security::ACL_SIZE_INFORMATION;
use windows_sys::Win32::Security::AccessCheck;
use windows_sys::Win32::Security::AclSizeInformation;
use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
use windows_sys::Win32::Security::Authorization::GetSecurityInfo;
use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;
use windows_sys::Win32::Security::Authorization::SetEntriesInAclW;
use windows_sys::Win32::Security::Authorization::SetNamedSecurityInfoW;
use windows_sys::Win32::Security::Authorization::SetSecurityInfo;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_SID;
use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_UNKNOWN;
use windows_sys::Win32::Security::Authorization::TRUSTEE_W;
use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
use windows_sys::Win32::Security::DuplicateTokenEx;
use windows_sys::Win32::Security::EqualSid;
use windows_sys::Win32::Security::GENERIC_MAPPING;
use windows_sys::Win32::Security::GROUP_SECURITY_INFORMATION;
use windows_sys::Win32::Security::GetAce;
use windows_sys::Win32::Security::GetAclInformation;
use windows_sys::Win32::Security::GetSecurityDescriptorControl;
use windows_sys::Win32::Security::IsValidSid;
use windows_sys::Win32::Security::MapGenericMask;
use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
use windows_sys::Win32::Security::PRIVILEGE_SET;
use windows_sys::Win32::Security::PROTECTED_DACL_SECURITY_INFORMATION;
use windows_sys::Win32::Security::SE_DACL_PROTECTED;
use windows_sys::Win32::Security::SecurityImpersonation;
use windows_sys::Win32::Security::TOKEN_IMPERSONATE;
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::Security::TokenImpersonation;
use windows_sys::Win32::Security::UNPROTECTED_DACL_SECURITY_INFORMATION;
use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;
use windows_sys::Win32::Storage::FileSystem::CreateFileW;
use windows_sys::Win32::Storage::FileSystem::DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::Storage::FileSystem::FILE_APPEND_DATA;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_sys::Win32::Storage::FileSystem::FILE_DELETE_CHILD;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_EXECUTE;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_DATA;
use windows_sys::Win32::Storage::FileSystem::FILE_WRITE_EA;
use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;
use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
use windows_sys::Win32::Storage::FileSystem::OPEN_EXISTING;
use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;
use windows_sys::Win32::Storage::FileSystem::WRITE_DAC;
use windows_sys::Win32::System::Threading::GetCurrentProcess;
const SE_KERNEL_OBJECT: u32 = 6;
const INHERIT_ONLY_ACE: u8 = 0x08;
const INHERITED_ACE: u8 = 0x10;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const GENERIC_READ_MASK: u32 = 0x8000_0000;
const GENERIC_WRITE_MASK: u32 = 0x4000_0000;
const DENY_ACCESS: i32 = 3;
const GRANT_ACCESS: i32 = 1;
/// Honest precondition for ACL-based provisioning (Track A ADR
/// `g0-windows-provisioning-boundary`): ACLs are not kernel isolation, and no
/// link-count rescan closes the same-user hard-link race, so provisioning must
/// run while no same-user process mutates workspace aliases.
pub const NANO_ACL_QUIESCENT_PRECONDITION: &str = "Wayland Nano Windows ACL provisioning requires a quiescent same-user window; concurrent same-user hard-link mutation is unsupported";

/// Fetch DACL via handle-based query; caller must LocalFree the returned SD.
///
/// # Safety
/// Caller must free the returned security descriptor with `LocalFree` and pass an existing path.
pub unsafe fn fetch_dacl_handle(path: &Path) -> Result<(*mut ACL, *mut c_void)> {
    let wpath = to_wide(path);
    let h = CreateFileW(
        wpath.as_ptr(),
        READ_CONTROL,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        std::ptr::null_mut(),
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS,
        0,
    );
    if h == INVALID_HANDLE_VALUE {
        return Err(anyhow!("CreateFileW failed for {}", path.display()));
    }
    let mut p_sd: *mut c_void = std::ptr::null_mut();
    let mut p_dacl: *mut ACL = std::ptr::null_mut();
    let code = GetSecurityInfo(
        h,
        1, // SE_FILE_OBJECT
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut p_dacl,
        std::ptr::null_mut(),
        &mut p_sd,
    );
    CloseHandle(h);
    if code != ERROR_SUCCESS {
        return Err(anyhow!(
            "GetSecurityInfo failed for {}: {}",
            path.display(),
            code
        ));
    }
    Ok((p_dacl, p_sd))
}

/// Fast mask-based check: does an ACE for provided SIDs grant the desired mask? Skips inherit-only.
/// When `require_all_bits` is true, all bits in `desired_mask` must be present; otherwise any bit suffices.
/// Returns whether `p_dacl` grants `desired_mask` to any of `psids`.
///
/// Contract: this is an allow-only scan — it ignores deny ACEs and must never
/// be used to decide whether access is *permitted*. Effective-access decisions
/// require `AccessCheck` (see `token_effectively_allows_delete`) or a
/// deny-aware helper. Grant-time idempotency and conservative world-write
/// detection are the only sanctioned uses (docs/audits/deny-ace-scan.md).
///
/// # Safety
/// `p_dacl` must point to a valid, readable DACL and every entry of `psids`
/// must be a valid SID pointer for the duration of the call.
pub unsafe fn dacl_mask_allows(
    p_dacl: *mut ACL,
    psids: &[*mut c_void],
    desired_mask: u32,
    require_all_bits: bool,
) -> bool {
    dacl_mask_allows_with_scope(
        p_dacl,
        psids,
        desired_mask,
        require_all_bits,
        AceScope::Effective,
    )
}

#[derive(Clone, Copy)]
enum AceScope {
    Effective,
    Explicit,
}

unsafe fn dacl_mask_allows_with_scope(
    p_dacl: *mut ACL,
    psids: &[*mut c_void],
    desired_mask: u32,
    require_all_bits: bool,
    scope: AceScope,
) -> bool {
    if p_dacl.is_null() {
        return false;
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    let ok = GetAclInformation(
        p_dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    );
    if ok == 0 {
        return false;
    }
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    for i in 0..(info.AceCount as usize) {
        let mut p_ace: *mut c_void = std::ptr::null_mut();
        if GetAce(p_dacl as *const ACL, i as u32, &mut p_ace) == 0 {
            continue;
        }
        let hdr = &*(p_ace as *const ACE_HEADER);
        if hdr.AceType != ACCESS_ALLOWED_ACE_TYPE {
            continue; // not ACCESS_ALLOWED
        }
        if (hdr.AceFlags & INHERIT_ONLY_ACE) != 0 {
            continue;
        }
        // SET_ACCESS cannot replace an ACE inherited from an ancestor, so it cannot make
        // an explicit-only repair converge when that inherited ACE contains stale rights.
        if matches!(scope, AceScope::Explicit) && (hdr.AceFlags & INHERITED_ACE) != 0 {
            continue;
        }
        let base = p_ace as usize;
        let sid_ptr =
            (base + std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>()) as *mut c_void;
        let mut matched = false;
        for sid in psids {
            if EqualSid(sid_ptr, *sid) != 0 {
                matched = true;
                break;
            }
        }
        if !matched {
            continue;
        }
        let ace = &*(p_ace as *const ACCESS_ALLOWED_ACE);
        let mut mask = ace.Mask;
        MapGenericMask(&mut mask, &mapping);
        if (require_all_bits && (mask & desired_mask) == desired_mask)
            || (!require_all_bits && (mask & desired_mask) != 0)
        {
            return true;
        }
    }
    false
}

/// Strict scan for an effective (non-inherit-only) *inherited* allow ACE
/// covering `desired_mask`, failing closed on any enumeration anomaly.
///
/// Ported from Track A (`dacl_has_inherited_allow_mask_strict`): the donor's
/// lenient scans silently skip `GetAce` failures, which would let a stale
/// inherited `FILE_DELETE_CHILD` grant go unnoticed. This parser bounds-checks
/// every ACE against the declared ACL allocation, rejects unsupported ACE
/// types and malformed SIDs, and propagates enumeration failures as errors.
/// `force_enumeration_failure` exists so tests can prove the fail-closed path.
unsafe fn dacl_has_inherited_allow_mask_strict(
    dacl: *mut ACL,
    psids: &[*mut c_void],
    desired_mask: u32,
    force_enumeration_failure: bool,
) -> Result<bool> {
    if dacl.is_null() {
        // A null DACL grants all access, including parent-mediated deletion.
        return Ok(true);
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if force_enumeration_failure
        || GetAclInformation(
            dacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
    {
        return Err(anyhow!("strict inherited ACL enumeration failed"));
    }
    let acl_start = dacl as usize;
    let acl_end = acl_start
        .checked_add(info.AclBytesInUse as usize)
        .ok_or_else(|| anyhow!("strict inherited ACL bounds overflow"))?;
    if info.AclBytesInUse < std::mem::size_of::<ACL>() as u32 {
        return Err(anyhow!("strict inherited ACL is shorter than its header"));
    }
    let declared_size = (*dacl).AclSize as u32;
    if info.AclBytesInUse > declared_size || info.AclBytesFree > declared_size - info.AclBytesInUse
    {
        return Err(anyhow!(
            "strict inherited ACL size information is inconsistent"
        ));
    }
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    for index in 0..info.AceCount {
        let mut ace_ptr = std::ptr::null_mut();
        if GetAce(dacl, index, &mut ace_ptr) == 0 {
            return Err(anyhow!("strict inherited ACL GetAce({index}) failed"));
        }
        let ace_start = ace_ptr as usize;
        let header_end = ace_start
            .checked_add(std::mem::size_of::<ACE_HEADER>())
            .ok_or_else(|| anyhow!("strict inherited ACE header bounds overflow"))?;
        if ace_start < acl_start || header_end > acl_end {
            return Err(anyhow!("strict inherited ACE header is outside the ACL"));
        }
        let header = &*(ace_ptr as *const ACE_HEADER);
        let ace_end = ace_start
            .checked_add(header.AceSize as usize)
            .ok_or_else(|| anyhow!("strict inherited ACE bounds overflow"))?;
        if ace_end > acl_end || header.AceSize < std::mem::size_of::<ACE_HEADER>() as u16 {
            return Err(anyhow!("strict inherited ACE size is invalid"));
        }
        let inherited_effective =
            (header.AceFlags & (INHERIT_ONLY_ACE | INHERITED_ACE)) == INHERITED_ACE;
        if inherited_effective && matches!(header.AceType, 4 | 5 | 9 | 11) {
            return Err(anyhow!(
                "strict inherited ACL contains unsupported allow ACE type {}",
                header.AceType
            ));
        }
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE || !inherited_effective {
            continue;
        }
        let sid_start = ace_start
            .checked_add(std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
            .ok_or_else(|| anyhow!("strict inherited SID offset overflow"))?;
        if sid_start.checked_add(8).is_none_or(|end| end > ace_end) {
            return Err(anyhow!("strict inherited allow ACE has a truncated SID"));
        }
        let subauthority_count = *((sid_start + 1) as *const u8) as usize;
        let sid_len = 8usize
            .checked_add(4usize.saturating_mul(subauthority_count))
            .ok_or_else(|| anyhow!("strict inherited SID length overflow"))?;
        if sid_start
            .checked_add(sid_len)
            .is_none_or(|end| end > ace_end)
        {
            return Err(anyhow!("strict inherited allow ACE SID exceeds ACE bounds"));
        }
        let sid_ptr = sid_start as *mut c_void;
        if IsValidSid(sid_ptr) == 0 {
            return Err(anyhow!(
                "strict inherited allow ACE contains an invalid SID"
            ));
        }
        if !psids.iter().any(|sid| EqualSid(sid_ptr, *sid) != 0) {
            continue;
        }
        let mut mask = (*(ace_ptr as *const ACCESS_ALLOWED_ACE)).Mask;
        MapGenericMask(&mut mask, &mapping);
        if (mask & desired_mask) != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Path-based wrapper around the mask check (single DACL fetch).
///
/// Contract: like `dacl_mask_allows`, this scan ignores deny ACEs; a `true`
/// result means an allow ACE exists, not that access is permitted. Do not use
/// it to authorize an operation (docs/audits/deny-ace-scan.md).
pub fn path_mask_allows(
    path: &Path,
    psids: &[*mut c_void],
    desired_mask: u32,
    require_all_bits: bool,
) -> Result<bool> {
    path_mask_allows_with_scope(
        path,
        psids,
        desired_mask,
        require_all_bits,
        AceScope::Effective,
    )
}

fn path_mask_allows_with_scope(
    path: &Path,
    psids: &[*mut c_void],
    desired_mask: u32,
    require_all_bits: bool,
    scope: AceScope,
) -> Result<bool> {
    unsafe {
        let (p_dacl, sd) = fetch_dacl_handle(path)?;
        let has = dacl_mask_allows_with_scope(p_dacl, psids, desired_mask, require_all_bits, scope);
        if !sd.is_null() {
            LocalFree(sd as HLOCAL);
        }
        Ok(has)
    }
}

/// Returns whether `p_dacl` contains a write-allow ACE for `psid`.
///
/// # Safety
/// `p_dacl` must point to a valid, readable DACL and `psid` must be a valid
/// SID pointer for the duration of the call.
pub unsafe fn dacl_has_write_allow_for_sid(p_dacl: *mut ACL, psid: *mut c_void) -> bool {
    if p_dacl.is_null() {
        return false;
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    let ok = GetAclInformation(
        p_dacl as *const ACL,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    );
    if ok == 0 {
        return false;
    }
    let count = info.AceCount as usize;
    for i in 0..count {
        let mut p_ace: *mut c_void = std::ptr::null_mut();
        if GetAce(p_dacl as *const ACL, i as u32, &mut p_ace) == 0 {
            continue;
        }
        let hdr = &*(p_ace as *const ACE_HEADER);
        if hdr.AceType != ACCESS_ALLOWED_ACE_TYPE {
            continue; // ACCESS_ALLOWED_ACE_TYPE
        }
        // Ignore ACEs that are inherit-only (do not apply to the current object)
        if (hdr.AceFlags & INHERIT_ONLY_ACE) != 0 {
            continue;
        }
        let ace = &*(p_ace as *const ACCESS_ALLOWED_ACE);
        let mask = ace.Mask;
        let base = p_ace as usize;
        let sid_ptr =
            (base + std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>()) as *mut c_void;
        let eq = EqualSid(sid_ptr, psid);
        if eq != 0 && (mask & FILE_GENERIC_WRITE) != 0 {
            return true;
        }
    }
    false
}

// Grant DELETE on each inheriting descendant instead of FILE_DELETE_CHILD on
// its parent. A parent delete-child grant would bypass a direct deny-write ACE
// on protected children such as `.git` or an explicit read-only subpath.
const WRITE_ALLOW_MASK: u32 =
    FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;
const ROOT_WRITE_ALLOW_MASK: u32 = WRITE_ALLOW_MASK & !DELETE;

/// Strict scan for an INHERIT_ONLY allow ACE covering every bit of
/// `desired_mask` for `psid`, failing closed on enumeration anomalies.
/// (Track A: `dacl_has_inherit_only_allow_mask`.)
unsafe fn dacl_has_inherit_only_allow_mask(
    dacl: *mut ACL,
    psid: *mut c_void,
    desired_mask: u32,
) -> Result<bool> {
    if dacl.is_null() {
        return Err(anyhow!("inheritable ACL unexpectedly has a null DACL"));
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        dacl,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return Err(anyhow!("inheritable ACL enumeration failed"));
    }
    let acl_start = dacl as usize;
    let acl_end = acl_start
        .checked_add(info.AclBytesInUse as usize)
        .ok_or_else(|| anyhow!("inheritable ACL bounds overflow"))?;
    let declared_size = (*dacl).AclSize as u32;
    if info.AclBytesInUse < std::mem::size_of::<ACL>() as u32
        || info.AclBytesInUse > declared_size
        || info.AclBytesFree > declared_size - info.AclBytesInUse
    {
        return Err(anyhow!("inheritable ACL size information is inconsistent"));
    }
    for index in 0..info.AceCount {
        let mut raw_ace = std::ptr::null_mut();
        if GetAce(dacl, index, &mut raw_ace) == 0 {
            return Err(anyhow!("inheritable ACL GetAce({index}) failed"));
        }
        let ace_start = raw_ace as usize;
        let header_end = ace_start
            .checked_add(std::mem::size_of::<ACE_HEADER>())
            .ok_or_else(|| anyhow!("inheritable ACE header bounds overflow"))?;
        if ace_start < acl_start || header_end > acl_end {
            return Err(anyhow!("inheritable ACE header is outside the ACL"));
        }
        let header = &*(raw_ace as *const ACE_HEADER);
        let ace_end = ace_start
            .checked_add(header.AceSize as usize)
            .ok_or_else(|| anyhow!("inheritable ACE bounds overflow"))?;
        if ace_end > acl_end || header.AceSize < std::mem::size_of::<ACE_HEADER>() as u16 {
            return Err(anyhow!("inheritable ACE size is invalid"));
        }
        let inherit_only = (header.AceFlags & INHERIT_ONLY_ACE) != 0;
        if inherit_only && matches!(header.AceType, 4 | 5 | 9 | 11) {
            return Err(anyhow!(
                "inheritable ACL contains unsupported allow ACE type {}",
                header.AceType
            ));
        }
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE || !inherit_only {
            continue;
        }
        let sid_start = ace_start
            .checked_add(std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
            .ok_or_else(|| anyhow!("inheritable SID offset overflow"))?;
        if sid_start.checked_add(8).is_none_or(|end| end > ace_end) {
            return Err(anyhow!("inheritable allow ACE has a truncated SID"));
        }
        let subauthority_count = *((sid_start + 1) as *const u8) as usize;
        let sid_len = 8usize
            .checked_add(4usize.saturating_mul(subauthority_count))
            .ok_or_else(|| anyhow!("inheritable SID length overflow"))?;
        if sid_start
            .checked_add(sid_len)
            .is_none_or(|end| end > ace_end)
        {
            return Err(anyhow!("inheritable allow ACE SID exceeds ACE bounds"));
        }
        let sid = sid_start as *mut c_void;
        if IsValidSid(sid) == 0 {
            return Err(anyhow!("inheritable allow ACE contains an invalid SID"));
        }
        let ace = &*(raw_ace as *const ACCESS_ALLOWED_ACE);
        if EqualSid(sid, psid) != 0 && (ace.Mask & desired_mask) == desired_mask {
            return Ok(true);
        }
    }
    Ok(false)
}

unsafe fn dacl_allow_mask_needs_refresh(
    p_dacl: *mut ACL,
    psid: *mut c_void,
    allow_mask: u32,
    disallow_mask: u32,
) -> bool {
    !dacl_mask_allows(p_dacl, &[psid], allow_mask, /*require_all_bits*/ true)
        || dacl_mask_allows_with_scope(
            p_dacl,
            &[psid],
            disallow_mask,
            /*require_all_bits*/ false,
            AceScope::Explicit,
        )
}

/// Returns whether any provided SID needs its writable-root allow ACE refreshed.
pub fn path_write_aces_need_refresh(path: &Path, psids: &[*mut c_void]) -> Result<bool> {
    unsafe {
        let (p_dacl, p_sd) = fetch_dacl_handle(path)?;
        let needs_refresh = psids.iter().any(|psid| {
            dacl_allow_mask_needs_refresh(p_dacl, *psid, WRITE_ALLOW_MASK, FILE_DELETE_CHILD)
        });
        if !p_sd.is_null() {
            LocalFree(p_sd as HLOCAL);
        }
        Ok(needs_refresh)
    }
}

unsafe fn ensure_allow_mask_aces_with_inheritance_impl(
    path: &Path,
    sids: &[*mut c_void],
    allow_mask: u32,
    disallow_mask: u32,
    inheritance: u32,
) -> Result<bool> {
    let (p_dacl, p_sd) = fetch_dacl_handle(path)?;
    let mut entries: Vec<EXPLICIT_ACCESS_W> = Vec::new();
    for sid in sids {
        if !dacl_allow_mask_needs_refresh(p_dacl, *sid, allow_mask, disallow_mask) {
            continue;
        }
        entries.push(EXPLICIT_ACCESS_W {
            grfAccessPermissions: allow_mask,
            grfAccessMode: 2, // SET_ACCESS
            grfInheritance: inheritance,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: *sid as *mut u16,
            },
        });
    }
    let mut added = false;
    if !entries.is_empty() {
        let mut p_new_dacl: *mut ACL = std::ptr::null_mut();
        let code2 = SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            p_dacl,
            &mut p_new_dacl,
        );
        if code2 == ERROR_SUCCESS {
            let code3 = SetNamedSecurityInfoW(
                to_wide(path).as_ptr() as *mut u16,
                1,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                p_new_dacl,
                std::ptr::null_mut(),
            );
            if code3 == ERROR_SUCCESS {
                added = true;
                if !p_new_dacl.is_null() {
                    LocalFree(p_new_dacl as HLOCAL);
                }
            } else {
                if !p_new_dacl.is_null() {
                    LocalFree(p_new_dacl as HLOCAL);
                }
                if !p_sd.is_null() {
                    LocalFree(p_sd as HLOCAL);
                }
                return Err(anyhow!("SetNamedSecurityInfoW failed: {code3}"));
            }
        } else {
            if !p_sd.is_null() {
                LocalFree(p_sd as HLOCAL);
            }
            return Err(anyhow!("SetEntriesInAclW failed: {code2}"));
        }
    }
    if !p_sd.is_null() {
        LocalFree(p_sd as HLOCAL);
    }
    Ok(added)
}

/// Ensure all provided SIDs have an allow ACE with the requested mask on the path.
/// Returns true if any ACE was added.
///
/// # Safety
/// Caller must pass valid SID pointers and an existing path; free the returned security descriptor with `LocalFree`.
pub unsafe fn ensure_allow_mask_aces_with_inheritance(
    path: &Path,
    sids: &[*mut c_void],
    allow_mask: u32,
    inheritance: u32,
) -> Result<bool> {
    ensure_allow_mask_aces_with_inheritance_impl(
        path,
        sids,
        allow_mask,
        /*disallow_mask*/ 0,
        inheritance,
    )
}

/// Ensure all provided SIDs have an allow ACE with the requested mask on the path.
/// Returns true if any ACE was added.
///
/// # Safety
/// Caller must pass valid SID pointers and an existing path; free the returned security descriptor with `LocalFree`.
pub unsafe fn ensure_allow_mask_aces(
    path: &Path,
    sids: &[*mut c_void],
    allow_mask: u32,
) -> Result<bool> {
    ensure_allow_mask_aces_with_inheritance(
        path,
        sids,
        allow_mask,
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
    )
}

/// Ensure all provided SIDs have a write-capable allow ACE on the path.
/// Returns true if any ACE was added.
///
/// # Safety
/// Caller must pass valid SID pointers and an existing path; free the returned security descriptor with `LocalFree`.
pub unsafe fn ensure_allow_write_aces(path: &Path, sids: &[*mut c_void]) -> Result<bool> {
    ensure_allow_mask_aces_with_inheritance_impl(
        path,
        sids,
        WRITE_ALLOW_MASK,
        FILE_DELETE_CHILD,
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
    )
}

/// Returns whether the path has a non-inherit-only deny-write ACE for `psid`.
///
/// # Safety
/// `psid` must point to a valid SID for the duration of this call.
pub unsafe fn path_has_write_deny_for_sid(path: &Path, psid: *mut c_void) -> Result<bool> {
    let (handle, _, _, _) = open_pinned_tree_handle(path)?;
    let (dacl, sd) = fetch_dacl_for_handle(handle.0)?;
    let denied = dacl_has_explicit_deny_mask(dacl, psid, DenyAceKind::Write.mask());
    LocalFree(sd as HLOCAL);
    Ok(denied)
}

/// Returns whether the object has an explicit deny for parent-mediated child deletion.
///
/// # Safety
/// `psid` must point to a valid SID for the duration of this call.
pub unsafe fn path_has_delete_child_deny_for_sid(path: &Path, psid: *mut c_void) -> Result<bool> {
    let (handle, _, _, _) = open_pinned_tree_handle(path)?;
    let (dacl, sd) = fetch_dacl_for_handle(handle.0)?;
    let denied = dacl_has_explicit_deny_mask(dacl, psid, FILE_DELETE_CHILD);
    LocalFree(sd as HLOCAL);
    Ok(denied)
}

/// Returns the exact ACL bytes on the opened object (the reparse object itself, when applicable).
pub fn path_dacl_bytes(path: &Path) -> Result<Vec<u8>> {
    let (handle, _, _, _) = open_pinned_tree_handle(path)?;
    let (dacl, sd) = unsafe { fetch_dacl_for_handle(handle.0)? };
    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        GetAclInformation(
            dacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        unsafe { LocalFree(sd as HLOCAL) };
        return Err(anyhow!("query exact ACL bytes failed"));
    }
    let acl_size = unsafe { (*dacl).AclSize as usize };
    let in_use = info.AclBytesInUse as usize;
    let free = info.AclBytesFree as usize;
    if acl_size < std::mem::size_of::<ACL>() || in_use.checked_add(free) != Some(acl_size) {
        unsafe { LocalFree(sd as HLOCAL) };
        return Err(anyhow!("invalid exact ACL allocation bounds"));
    }
    let mut bytes = vec![0u8; acl_size];
    unsafe {
        std::ptr::copy_nonoverlapping(dacl as *const u8, bytes.as_mut_ptr(), bytes.len());
        LocalFree(sd as HLOCAL);
    }
    Ok(bytes)
}

/// Returns whether the object's DACL is protected from inheritance.
pub fn path_dacl_is_protected(path: &Path) -> Result<bool> {
    let (handle, _, _, _) = open_pinned_tree_handle(path)?;
    let (_, sd) = unsafe { fetch_dacl_for_handle(handle.0)? };
    let mut control = 0u16;
    let mut revision = 0u32;
    if unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) } == 0 {
        unsafe { LocalFree(sd as HLOCAL) };
        return Err(anyhow!("query DACL protection state failed"));
    }
    unsafe { LocalFree(sd as HLOCAL) };
    Ok((control & SE_DACL_PROTECTED) != 0)
}

/// Ensures the scoped writable-root ACE on the root and every existing descendant.
///
/// Existing descendants may have disabled ACL inheritance, so updating only the root is not
/// sufficient. Reparse points are skipped entirely: traversing one could grant access outside the
/// writable root. DELETE is granted on each object, while FILE_DELETE_CHILD remains forbidden.
/// Existing deny ACEs are retained by `SetEntriesInAclW`.
///
/// # Safety
/// Every entry in `sids` must remain a valid SID pointer for the duration of this call.
pub unsafe fn ensure_allow_write_aces_on_tree(root: &Path, sids: &[*mut c_void]) -> Result<usize> {
    let mut transaction = AclTreeTransaction::new();
    let changed = match ensure_allow_write_aces_on_tree_in_transaction(root, sids, &mut transaction)
    {
        Ok(changed) => changed,
        Err(err) => {
            transaction.rollback_now().map_err(|rollback| {
                anyhow!("ACL update failed: {err}; rollback failed: {rollback}")
            })?;
            return Err(err);
        }
    };
    transaction.commit()?;
    Ok(changed)
}

/// Test helper for the legacy-only path using the current process token for effective access.
/// Elevated setup must use [`ensure_allow_write_aces_on_tree`] so its ACL model is unchanged.
///
/// # Safety
/// Every entry in `sids` and `legacy_user_sid` must remain a valid SID
/// pointer for the duration of this call.
#[cfg(test)]
pub unsafe fn ensure_allow_write_aces_on_tree_for_legacy_user(
    root: &Path,
    sids: &[*mut c_void],
    legacy_user_sid: Option<*mut c_void>,
) -> Result<usize> {
    let token = crate::token::get_current_token_for_restriction()?;
    let result = ensure_allow_write_aces_on_tree_for_legacy_user_and_token(
        root,
        sids,
        legacy_user_sid,
        token,
    );
    CloseHandle(token);
    result
}

/// Writable-tree grant variant for the legacy (non-elevated) session path:
/// existing descendants also receive an object-level DELETE grant for the
/// real user's SID, verified against `effective_token` via `AccessCheck`.
///
/// # Safety
/// Every entry in `sids` and `legacy_user_sid` must remain a valid SID
/// pointer for the duration of this call; `effective_token` must be a valid
/// token handle for the same user as `legacy_user_sid`.
pub unsafe fn ensure_allow_write_aces_on_tree_for_legacy_user_and_token(
    root: &Path,
    sids: &[*mut c_void],
    legacy_user_sid: Option<*mut c_void>,
    effective_token: HANDLE,
) -> Result<usize> {
    let mut transaction = AclTreeTransaction::new();
    let changed = match ensure_allow_write_aces_on_tree_in_transaction_cancellable_for_legacy_user(
        root,
        sids,
        legacy_user_sid,
        Some(effective_token),
        &mut transaction,
        &|| Ok(false),
    ) {
        Ok(changed) => changed,
        Err(err) => {
            transaction.rollback_now().map_err(|rollback| {
                anyhow!("ACL update failed: {err}; rollback failed: {rollback}")
            })?;
            return Err(err);
        }
    };
    transaction.commit()?;
    Ok(changed)
}

/// Adds this writable tree to an existing all-or-nothing ACL transaction.
///
/// # Safety
/// Every entry in `sids` must remain a valid SID pointer through the call.
pub unsafe fn ensure_allow_write_aces_on_tree_in_transaction(
    root: &Path,
    sids: &[*mut c_void],
    transaction: &mut AclTreeTransaction,
) -> Result<usize> {
    ensure_allow_write_aces_on_tree_in_transaction_cancellable(root, sids, transaction, &|| {
        Ok(false)
    })
}

/// Cancellable variant of [`ensure_allow_write_aces_on_tree_in_transaction`].
///
/// # Safety
/// Every SID pointer must remain valid through the call. The cancellation callback must not
/// mutate the traversed tree.
pub unsafe fn ensure_allow_write_aces_on_tree_in_transaction_cancellable(
    root: &Path,
    sids: &[*mut c_void],
    transaction: &mut AclTreeTransaction,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<usize> {
    ensure_allow_write_aces_on_tree_in_transaction_cancellable_for_legacy_user(
        root,
        sids,
        None,
        None,
        transaction,
        cancelled,
    )
}

unsafe fn ensure_allow_write_aces_on_tree_in_transaction_cancellable_for_legacy_user(
    root: &Path,
    sids: &[*mut c_void],
    legacy_user_sid: Option<*mut c_void>,
    effective_token: Option<HANDLE>,
    transaction: &mut AclTreeTransaction,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<usize> {
    validate_tree_has_no_multilink_files(root, None, "writable", cancelled)?;
    snapshot_tree_for_transaction(root, None, false, transaction, cancelled)?;
    ensure_allow_write_aces_on_tree_inner(
        root,
        sids,
        legacy_user_sid,
        effective_token,
        None,
        transaction,
        cancelled,
    )
}

struct PinnedTreeHandle(HANDLE);

impl Drop for PinnedTreeHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    volume: u32,
    index_high: u32,
    index_low: u32,
}

struct AclRollbackRecord {
    handle: PinnedTreeHandle,
    identity: FileIdentity,
    link_count: u32,
    acl_words: Vec<u32>,
    acl_size: usize,
    dacl_protected: bool,
}

/// Rolls every registered ACL back unless `commit` succeeds.
///
/// Callers must enforce [`NANO_ACL_QUIESCENT_PRECONDITION`]. This transaction does not claim
/// resistance to concurrent same-user hard-link mutation.
pub struct AclTreeTransaction {
    records: Vec<AclRollbackRecord>,
    finished: bool,
}

impl AclTreeTransaction {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            finished: false,
        }
    }

    pub fn commit(mut self) -> Result<()> {
        let validation = self.records.iter().try_for_each(|record| {
            let info = file_information(record.handle.0)?;
            if file_identity(&info) != record.identity || info.nNumberOfLinks != record.link_count {
                return Err(anyhow!(
                    "ACL transaction identity or link count changed before commit"
                ));
            }
            Ok(())
        });
        if let Err(err) = validation {
            if let Err(rollback) = self.rollback() {
                return Err(anyhow!(
                    "ACL transaction commit failed: {err}; rollback failed: {rollback}"
                ));
            }
            return Err(err);
        }
        self.finished = true;
        Ok(())
    }

    pub fn rollback_now(&mut self) -> Result<()> {
        self.rollback()
    }

    fn snapshot_before_mutation(&mut self, handle: HANDLE) -> Result<()> {
        let info = file_information(handle)?;
        let identity = file_identity(&info);
        if self
            .records
            .iter()
            .any(|record| record.identity == identity)
        {
            return Ok(());
        }
        let (dacl, sd) = unsafe { fetch_dacl_for_handle(handle)? };
        let mut control = 0u16;
        let mut revision = 0u32;
        if unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) } == 0 {
            unsafe { LocalFree(sd as HLOCAL) };
            return Err(anyhow!("query original security descriptor control failed"));
        }
        let (acl_words, acl_size) = match unsafe { copy_full_acl_allocation(dacl) } {
            Ok(snapshot) => snapshot,
            Err(err) => {
                unsafe { LocalFree(sd as HLOCAL) };
                return Err(err);
            }
        };
        unsafe { LocalFree(sd as HLOCAL) };
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate = INVALID_HANDLE_VALUE;
        if unsafe {
            DuplicateHandle(
                process,
                handle,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(anyhow!("duplicate ACL transaction handle failed"));
        }
        self.records.push(AclRollbackRecord {
            handle: PinnedTreeHandle(duplicate),
            identity,
            link_count: info.nNumberOfLinks,
            acl_words,
            acl_size,
            dacl_protected: (control & SE_DACL_PROTECTED) != 0,
        });
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        // Restore parents before descendants so propagation is overwritten by the exact saved
        // descendant ACL and inheritance-protection state.
        for record in &self.records {
            if record.acl_words.len() * std::mem::size_of::<u32>() < record.acl_size {
                errors.push(u32::MAX);
                continue;
            }
            let dacl = record.acl_words.as_ptr() as *mut ACL;
            let security_information = DACL_SECURITY_INFORMATION
                | if record.dacl_protected {
                    PROTECTED_DACL_SECURITY_INFORMATION
                } else {
                    UNPROTECTED_DACL_SECURITY_INFORMATION
                };
            let code = unsafe {
                SetSecurityInfo(
                    record.handle.0,
                    SE_FILE_OBJECT,
                    security_information,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    dacl,
                    std::ptr::null_mut(),
                )
            };
            if code != ERROR_SUCCESS {
                errors.push(code);
            }
        }
        self.finished = true;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("ACL transaction rollback failed: {errors:?}"))
        }
    }
}

unsafe fn copy_full_acl_allocation(dacl: *mut ACL) -> Result<(Vec<u32>, usize)> {
    let mut acl_info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        dacl,
        &mut acl_info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return Err(anyhow!("query original ACL size for transaction failed"));
    }
    let acl_size = (*dacl).AclSize as usize;
    let bytes_in_use = acl_info.AclBytesInUse as usize;
    let bytes_free = acl_info.AclBytesFree as usize;
    if acl_size < std::mem::size_of::<ACL>()
        || bytes_in_use > acl_size
        || bytes_free > acl_size
        || bytes_in_use.checked_add(bytes_free) != Some(acl_size)
    {
        return Err(anyhow!(
            "invalid ACL allocation bounds: size={acl_size} in_use={bytes_in_use} free={bytes_free}"
        ));
    }
    let mut words = vec![0u32; acl_size.div_ceil(std::mem::size_of::<u32>())];
    std::ptr::copy_nonoverlapping(dacl as *const u8, words.as_mut_ptr() as *mut u8, acl_size);
    Ok((words, acl_size))
}

impl Default for AclTreeTransaction {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AclTreeTransaction {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.rollback();
        }
    }
}

fn file_information(handle: HANDLE) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(anyhow!("query ACL transaction file identity failed"));
    }
    Ok(info)
}

fn file_identity(info: &BY_HANDLE_FILE_INFORMATION) -> FileIdentity {
    FileIdentity {
        volume: info.dwVolumeSerialNumber,
        index_high: info.nFileIndexHigh,
        index_low: info.nFileIndexLow,
    }
}

fn open_pinned_tree_handle(path: &Path) -> Result<(PinnedTreeHandle, u32, u32, PathBuf)> {
    let handle = unsafe {
        CreateFileW(
            to_wide(path).as_ptr(),
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(anyhow!(
            "open pinned ACL handle for {} failed",
            path.display()
        ));
    }
    let handle = PinnedTreeHandle(handle);
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
        return Err(anyhow!(
            "query pinned ACL handle for {} failed",
            path.display()
        ));
    }
    let final_path = final_path_for_handle(handle.0)?;
    Ok((
        handle,
        info.dwFileAttributes,
        info.nNumberOfLinks,
        final_path,
    ))
}

fn final_path_for_handle(handle: HANDLE) -> Result<PathBuf> {
    let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if needed == 0 {
        return Err(anyhow!("query final ACL handle path length failed"));
    }
    let mut buffer = vec![0u16; needed as usize + 1];
    let written =
        unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if written == 0 || written as usize >= buffer.len() {
        return Err(anyhow!("query final ACL handle path failed"));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(
        &buffer[..written as usize],
    )))
}

fn same_final_path(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(
            right
                .as_os_str()
                .to_string_lossy()
                .trim_end_matches(['\\', '/']),
        )
}

fn validate_tree_has_no_multilink_files(
    path: &Path,
    expected_final_path: Option<&Path>,
    tree_kind: &str,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<()> {
    if cancelled()? {
        anyhow::bail!("workspace ACL setup cancelled");
    }
    let (_handle, attributes, link_count, final_path) = open_pinned_tree_handle(path)?;
    if let Some(expected) = expected_final_path
        && !same_final_path(&final_path, expected)
    {
        return Err(anyhow!(
            "{tree_kind} tree path was replaced during hard-link preflight: expected {} opened {}",
            expected.display(),
            final_path.display()
        ));
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0 && link_count > 1 {
        return Err(anyhow!(
            "unsupported hard-linked {tree_kind} file {} has {link_count} links; refusing ACL mutation because all aliases cannot be proven in scope",
            path.display()
        ));
    }
    if (attributes & FILE_ATTRIBUTE_REPARSE_POINT) == 0
        && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0
    {
        for entry in std::fs::read_dir(path)
            .map_err(|err| anyhow!("enumerate {tree_kind} preflight {}: {err}", path.display()))?
        {
            let entry = entry.map_err(|err| {
                anyhow!(
                    "enumerate {tree_kind} preflight child {}: {err}",
                    path.display()
                )
            })?;
            let expected_child = final_path.join(entry.file_name());
            validate_tree_has_no_multilink_files(
                &entry.path(),
                Some(&expected_child),
                tree_kind,
                cancelled,
            )?;
        }
    }
    Ok(())
}

fn snapshot_tree_for_transaction(
    path: &Path,
    expected_final_path: Option<&Path>,
    snapshot_reparse_object: bool,
    transaction: &mut AclTreeTransaction,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<()> {
    if cancelled()? {
        anyhow::bail!("workspace ACL setup cancelled");
    }
    let (handle, attributes, link_count, final_path) = open_pinned_tree_handle(path)?;
    if let Some(expected) = expected_final_path
        && !same_final_path(&final_path, expected)
    {
        return Err(anyhow!(
            "ACL snapshot path identity changed: {}",
            path.display()
        ));
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0 && link_count > 1 {
        return Err(anyhow!(
            "unsupported hard-linked file during ACL snapshot: {}",
            path.display()
        ));
    }
    let is_reparse = (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    if !is_reparse || snapshot_reparse_object {
        transaction.snapshot_before_mutation(handle.0)?;
    }
    if !is_reparse && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        for entry in std::fs::read_dir(path)
            .map_err(|err| anyhow!("enumerate ACL snapshot tree {}: {err}", path.display()))?
        {
            let entry = entry
                .map_err(|err| anyhow!("enumerate ACL snapshot child {}: {err}", path.display()))?;
            let expected_child = final_path.join(entry.file_name());
            snapshot_tree_for_transaction(
                &entry.path(),
                Some(&expected_child),
                snapshot_reparse_object,
                transaction,
                cancelled,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn ensure_allow_write_aces_on_tree_inner(
    path: &Path,
    sids: &[*mut c_void],
    legacy_user_sid: Option<*mut c_void>,
    effective_token: Option<HANDLE>,
    expected_final_path: Option<&Path>,
    transaction: &mut AclTreeTransaction,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<usize> {
    let is_tree_root = expected_final_path.is_none();
    if cancelled()? {
        anyhow::bail!("workspace ACL setup cancelled");
    }
    let (handle, attributes, link_count, final_path) = open_pinned_tree_handle(path)?;
    if let Some(expected) = expected_final_path
        && !same_final_path(&final_path, expected)
    {
        return Err(anyhow!(
            "writable tree path was replaced during traversal: expected {} opened {}",
            expected.display(),
            final_path.display()
        ));
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0 && link_count > 1 {
        return Err(anyhow!(
            "unsupported hard-linked writable file {} has {link_count} links; refusing ACL mutation because every link cannot be proven inside the writable root",
            path.display()
        ));
    }
    if (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        return Ok(0);
    }
    // An explicit capability write deny marks a protected carveout. Do not add allows anywhere in
    // that subtree: a later user-specific DELETE grant or descendant mutation could otherwise
    // undermine the carveout even though the deny on this object itself remains ordered first.
    let (current_dacl, current_sd) = fetch_dacl_for_handle(handle.0)?;
    let protected_carveout =
        dacl_has_explicit_deny_mask_strict(current_dacl, sids, DenyAceKind::Write.mask(), false);
    LocalFree(current_sd as HLOCAL);
    let protected_carveout = protected_carveout?;
    if protected_carveout {
        return Ok(0);
    }
    let inheritance = if (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE
    } else {
        0
    };
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        reject_inherited_delete_child_allows(handle.0, sids)?;
    }
    let mut changed = 0;
    if is_tree_root {
        changed += usize::from(set_allow_aces_on_handle(
            handle.0,
            sids,
            ROOT_WRITE_ALLOW_MASK,
            DELETE | FILE_DELETE_CHILD,
            inheritance,
            transaction,
        )?);
        if (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
            changed += usize::from(ensure_inherit_only_delete_aces_on_handle(
                handle.0,
                sids,
                transaction,
            )?);
        }
    } else {
        changed += usize::from(set_allow_aces_on_handle(
            handle.0,
            sids,
            WRITE_ALLOW_MASK,
            FILE_DELETE_CHILD,
            inheritance,
            transaction,
        )?);
    }
    // Deleting the writable root itself is neither necessary nor desirable. In particular, an
    // ordinary user's effective token can legitimately be denied DELETE on that directory while
    // still being able to create and update children. Existing descendants do need an object-level
    // DELETE grant because FILE_DELETE_CHILD is deliberately never granted on their parents.
    if !is_tree_root && let Some(user_sid) = legacy_user_sid {
        changed += usize::from(grant_legacy_user_delete_on_handle(
            handle.0,
            user_sid,
            effective_token.ok_or_else(|| anyhow!("legacy effective token is missing"))?,
            transaction,
        )?);
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        // The non-delete-share handle pins this directory and every ancestor call keeps its own
        // handle alive, so `path` cannot be swapped to a junction while it is enumerated.
        for entry in std::fs::read_dir(path)
            .map_err(|err| anyhow!("enumerate pinned directory {}: {err}", path.display()))?
        {
            let entry =
                entry.map_err(|err| anyhow!("enumerate child of {}: {err}", path.display()))?;
            let expected_child = final_path.join(entry.file_name());
            changed += ensure_allow_write_aces_on_tree_inner(
                &entry.path(),
                sids,
                legacy_user_sid,
                effective_token,
                Some(&expected_child),
                transaction,
                cancelled,
            )?;
        }
    }
    Ok(changed)
}

unsafe fn grant_legacy_user_delete_on_handle(
    handle: HANDLE,
    user_sid: *mut c_void,
    effective_token: HANDLE,
    transaction: &mut AclTreeTransaction,
) -> Result<bool> {
    let (dacl, sd) = fetch_dacl_for_handle(handle)?;
    if dacl_mask_allows(dacl, &[user_sid], DELETE, true) {
        LocalFree(sd as HLOCAL);
        return token_effectively_allows_delete(handle, effective_token).and_then(|allowed| {
            if allowed {
                Ok(false)
            } else {
                Err(anyhow!("legacy token has an effective DELETE denial"))
            }
        });
    }
    transaction.snapshot_before_mutation(handle)?;
    let explicit = EXPLICIT_ACCESS_W {
        grfAccessPermissions: DELETE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: user_sid as *mut u16,
        },
    };
    let mut new_dacl = std::ptr::null_mut();
    let code = SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl);
    LocalFree(sd as HLOCAL);
    if code != ERROR_SUCCESS {
        return Err(anyhow!("SetEntriesInAclW(legacy DELETE) failed: {code}"));
    }
    let set_code = SetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        new_dacl,
        std::ptr::null_mut(),
    );
    LocalFree(new_dacl as HLOCAL);
    if set_code != ERROR_SUCCESS {
        return Err(anyhow!("SetSecurityInfo(legacy DELETE) failed: {set_code}"));
    }
    if !token_effectively_allows_delete(handle, effective_token)? {
        return Err(anyhow!(
            "legacy DELETE grant remains denied for the effective token"
        ));
    }
    Ok(true)
}

/// Effective-access oracle: does `token` (duplicated to an impersonation
/// token) actually receive DELETE on the object behind `handle`, per
/// `AccessCheck` against the live security descriptor? The allow-only DACL
/// scan cannot answer this — a deny ACE invisible to the scan would make a
/// fast-path `true` a lie, so every legacy DELETE fast path is verified here
/// (Track A fix, deny-ace-scan.md latent-risk pairing).
unsafe fn token_effectively_allows_delete(handle: HANDLE, token: HANDLE) -> Result<bool> {
    let initial_privilege_words = privilege_buffer_words(std::mem::size_of::<PRIVILEGE_SET>())?;
    let mut sd = std::ptr::null_mut();
    let code = GetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut sd,
    );
    if code != ERROR_SUCCESS {
        return Err(anyhow!(
            "GetSecurityInfo for DELETE AccessCheck failed: {code}"
        ));
    }
    let mut impersonation = 0;
    if DuplicateTokenEx(
        token,
        TOKEN_QUERY | TOKEN_IMPERSONATE,
        std::ptr::null_mut(),
        SecurityImpersonation,
        TokenImpersonation,
        &mut impersonation,
    ) == 0
    {
        LocalFree(sd as HLOCAL);
        return Err(anyhow!("DuplicateTokenEx for DELETE AccessCheck failed"));
    }
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut privileges = vec![0usize; initial_privilege_words];
    let (ok, granted, access_status, final_error) = loop {
        let Some(byte_capacity) = privileges.len().checked_mul(std::mem::size_of::<usize>()) else {
            CloseHandle(impersonation);
            LocalFree(sd as HLOCAL);
            return Err(anyhow!("AccessCheck privilege buffer capacity overflow"));
        };
        let Ok(mut privilege_len) = u32::try_from(byte_capacity) else {
            CloseHandle(impersonation);
            LocalFree(sd as HLOCAL);
            return Err(anyhow!("AccessCheck privilege buffer exceeds u32"));
        };
        let mut granted = 0;
        let mut access_status = 0;
        let ok = AccessCheck(
            sd,
            impersonation,
            DELETE,
            &mapping,
            privileges.as_mut_ptr() as *mut PRIVILEGE_SET,
            &mut privilege_len,
            &mut granted,
            &mut access_status,
        );
        let error = GetLastError();
        if ok != 0 || error != ERROR_INSUFFICIENT_BUFFER {
            break (ok, granted, access_status, error);
        }
        if privilege_len as usize <= byte_capacity {
            CloseHandle(impersonation);
            LocalFree(sd as HLOCAL);
            return Err(anyhow!(
                "AccessCheck returned a non-growing privilege buffer size"
            ));
        }
        let words = match privilege_buffer_words(privilege_len as usize) {
            Ok(words) => words,
            Err(err) => {
                CloseHandle(impersonation);
                LocalFree(sd as HLOCAL);
                return Err(err);
            }
        };
        privileges.resize(words, 0);
    };
    CloseHandle(impersonation);
    LocalFree(sd as HLOCAL);
    if ok == 0 {
        return Err(anyhow!(
            "AccessCheck for ordinary-token DELETE failed: {final_error}"
        ));
    }
    Ok(access_status != 0 && (granted & DELETE) == DELETE)
}

fn privilege_buffer_words(byte_len: usize) -> Result<usize> {
    let word = std::mem::size_of::<usize>();
    byte_len
        .checked_add(word - 1)
        .map(|rounded| rounded / word)
        .filter(|words| *words != 0)
        .ok_or_else(|| anyhow!("AccessCheck privilege buffer size overflow"))
}

unsafe fn reject_inherited_delete_child_allows(handle: HANDLE, sids: &[*mut c_void]) -> Result<()> {
    let (dacl, sd) = fetch_dacl_for_handle(handle)?;
    let stale = dacl_has_inherited_allow_mask_strict(
        dacl,
        sids,
        FILE_DELETE_CHILD,
        /*force_enumeration_failure*/ false,
    );
    LocalFree(sd as HLOCAL);
    let stale = stale?;
    if stale {
        return Err(anyhow!(
            "unsupported inherited FILE_DELETE_CHILD allow for writable capability; remove the stale ancestor grant before sandbox provisioning"
        ));
    }
    Ok(())
}

unsafe fn set_allow_aces_on_handle(
    handle: HANDLE,
    sids: &[*mut c_void],
    allow_mask: u32,
    disallow_mask: u32,
    inheritance: u32,
    transaction: &mut AclTreeTransaction,
) -> Result<bool> {
    let (p_dacl, p_sd) = fetch_dacl_for_handle(handle)?;
    let entries: Vec<EXPLICIT_ACCESS_W> = sids
        .iter()
        .filter(|sid| dacl_allow_mask_needs_refresh(p_dacl, **sid, allow_mask, disallow_mask))
        .map(|sid| EXPLICIT_ACCESS_W {
            grfAccessPermissions: allow_mask,
            grfAccessMode: 2,
            grfInheritance: inheritance,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: *sid as *mut u16,
            },
        })
        .collect();
    if entries.is_empty() {
        LocalFree(p_sd as HLOCAL);
        return Ok(false);
    }
    transaction.snapshot_before_mutation(handle)?;
    let mut new_dacl = std::ptr::null_mut();
    let code = SetEntriesInAclW(
        entries.len() as u32,
        entries.as_ptr(),
        p_dacl,
        &mut new_dacl,
    );
    LocalFree(p_sd as HLOCAL);
    if code != ERROR_SUCCESS {
        return Err(anyhow!("SetEntriesInAclW for pinned allow failed: {code}"));
    }
    let set_code = SetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        new_dacl,
        std::ptr::null_mut(),
    );
    LocalFree(new_dacl as HLOCAL);
    if set_code != ERROR_SUCCESS {
        return Err(anyhow!(
            "SetSecurityInfo for pinned allow failed: {set_code}"
        ));
    }
    let (verify_dacl, verify_sd) = fetch_dacl_for_handle(handle)?;
    let valid = sids
        .iter()
        .all(|sid| !dacl_allow_mask_needs_refresh(verify_dacl, *sid, allow_mask, disallow_mask));
    LocalFree(verify_sd as HLOCAL);
    if !valid {
        return Err(anyhow!("pinned allow ACL verification failed"));
    }
    Ok(true)
}

unsafe fn ensure_inherit_only_delete_aces_on_handle(
    handle: HANDLE,
    sids: &[*mut c_void],
    transaction: &mut AclTreeTransaction,
) -> Result<bool> {
    let (dacl, sd) = fetch_dacl_for_handle(handle)?;
    let mut entries = Vec::new();
    for sid in sids {
        let inherited = match dacl_has_inherit_only_allow_mask(dacl, *sid, DELETE) {
            Ok(inherited) => inherited,
            Err(err) => {
                LocalFree(sd as HLOCAL);
                return Err(err);
            }
        };
        if inherited {
            continue;
        }
        entries.push(EXPLICIT_ACCESS_W {
            grfAccessPermissions: DELETE,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: CONTAINER_INHERIT_ACE
                | OBJECT_INHERIT_ACE
                | u32::from(INHERIT_ONLY_ACE),
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: *sid as *mut u16,
            },
        });
    }
    if entries.is_empty() {
        LocalFree(sd as HLOCAL);
        return Ok(false);
    }
    transaction.snapshot_before_mutation(handle)?;
    let mut new_dacl = std::ptr::null_mut();
    let code = SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), dacl, &mut new_dacl);
    LocalFree(sd as HLOCAL);
    if code != ERROR_SUCCESS {
        return Err(anyhow!(
            "SetEntriesInAclW for inheritable DELETE failed: {code}"
        ));
    }
    let set_code = SetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        new_dacl,
        std::ptr::null_mut(),
    );
    LocalFree(new_dacl as HLOCAL);
    if set_code != ERROR_SUCCESS {
        return Err(anyhow!(
            "SetSecurityInfo for inheritable DELETE failed: {set_code}"
        ));
    }
    let (verify_dacl, verify_sd) = fetch_dacl_for_handle(handle)?;
    let valid = sids.iter().try_fold(true, |valid, sid| {
        Ok::<_, anyhow::Error>(
            valid && dacl_has_inherit_only_allow_mask(verify_dacl, *sid, DELETE)?,
        )
    });
    LocalFree(verify_sd as HLOCAL);
    let valid = valid?;
    if !valid {
        return Err(anyhow!("inheritable DELETE ACL verification failed"));
    }
    Ok(true)
}

unsafe fn fetch_dacl_for_handle(handle: HANDLE) -> Result<(*mut ACL, *mut c_void)> {
    let mut sd = std::ptr::null_mut();
    let mut dacl = std::ptr::null_mut();
    let code = GetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut dacl,
        std::ptr::null_mut(),
        &mut sd,
    );
    if code != ERROR_SUCCESS {
        return Err(anyhow!("GetSecurityInfo for pinned ACL failed: {code}"));
    }
    Ok((dacl, sd))
}

/// Adds an allow ACE granting read/write/execute to the given SID on the target path.
///
/// # Safety
/// Caller must ensure `psid` points to a valid SID and `path` refers to an existing file or directory.
pub unsafe fn add_allow_ace(path: &Path, psid: *mut c_void) -> Result<bool> {
    let mut p_sd: *mut c_void = std::ptr::null_mut();
    let mut p_dacl: *mut ACL = std::ptr::null_mut();
    let code = GetNamedSecurityInfoW(
        to_wide(path).as_ptr(),
        1,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut p_dacl,
        std::ptr::null_mut(),
        &mut p_sd,
    );
    if code != ERROR_SUCCESS {
        return Err(anyhow!("GetNamedSecurityInfoW failed: {code}"));
    }
    // Already has write? Skip costly DACL rewrite.
    if dacl_has_write_allow_for_sid(p_dacl, psid) {
        if !p_sd.is_null() {
            LocalFree(p_sd as HLOCAL);
        }
        return Ok(false);
    }
    let mut added = false;
    // Always ensure write is present: if an allow ACE exists without write, add one with write+RX.
    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: psid as *mut u16,
    };
    let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
    explicit.grfAccessPermissions = FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;
    explicit.grfAccessMode = 2; // SET_ACCESS
    explicit.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    explicit.Trustee = trustee;
    let mut p_new_dacl: *mut ACL = std::ptr::null_mut();
    let code2 = SetEntriesInAclW(1, &explicit, p_dacl, &mut p_new_dacl);
    if code2 == ERROR_SUCCESS {
        let code3 = SetNamedSecurityInfoW(
            to_wide(path).as_ptr() as *mut u16,
            1,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            p_new_dacl,
            std::ptr::null_mut(),
        );
        if code3 == ERROR_SUCCESS {
            added = !dacl_has_write_allow_for_sid(p_dacl, psid);
        }
        if !p_new_dacl.is_null() {
            LocalFree(p_new_dacl as HLOCAL);
        }
    }
    if !p_sd.is_null() {
        LocalFree(p_sd as HLOCAL);
    }
    Ok(added)
}

/// Adds a deny ACE to prevent write/append/delete for the given SID on the target path.
///
/// The deny is applied through a pinned handle to the object itself (a reparse
/// point is denied, never followed), post-write verified against a fresh DACL
/// fetch, and rolled back to the exact prior ACL bytes on any failure.
/// Hard-linked files are refused: an alias outside the intended scope would
/// otherwise receive the same DACL mutation.
///
/// # Safety
/// Caller must ensure `psid` points to a valid SID and `path` refers to an existing file or directory.
pub unsafe fn add_deny_write_ace(path: &Path, psid: *mut c_void) -> Result<bool> {
    add_deny_ace(path, psid, DenyAceKind::Write)
}

/// Applies a deny-write ACE to an existing tree without traversing reparse points.
///
/// This repairs protected trees whose existing descendants have disabled inheritance.
///
/// # Safety
/// `psid` must remain a valid SID pointer for the duration of this call.
pub unsafe fn add_deny_write_ace_on_tree(root: &Path, psid: *mut c_void) -> Result<usize> {
    let mut transaction = AclTreeTransaction::new();
    let changed = match add_deny_write_ace_on_tree_in_transaction(root, psid, &mut transaction) {
        Ok(changed) => changed,
        Err(err) => {
            transaction.rollback_now().map_err(|rollback| {
                anyhow!("ACL update failed: {err}; rollback failed: {rollback}")
            })?;
            return Err(err);
        }
    };
    transaction.commit()?;
    Ok(changed)
}

/// Adds this protected tree to an existing all-or-nothing ACL transaction.
///
/// # Safety
/// `psid` must remain a valid SID pointer through the call.
pub unsafe fn add_deny_write_ace_on_tree_in_transaction(
    root: &Path,
    psid: *mut c_void,
    transaction: &mut AclTreeTransaction,
) -> Result<usize> {
    add_deny_write_ace_on_tree_in_transaction_cancellable(root, psid, transaction, &|| Ok(false))
}

/// Cancellable variant of [`add_deny_write_ace_on_tree_in_transaction`].
///
/// # Safety
/// `psid` must remain valid through the call. The cancellation callback must not mutate the
/// traversed tree.
pub unsafe fn add_deny_write_ace_on_tree_in_transaction_cancellable(
    root: &Path,
    psid: *mut c_void,
    transaction: &mut AclTreeTransaction,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<usize> {
    validate_tree_has_no_multilink_files(root, None, "protected", cancelled)?;
    snapshot_tree_for_transaction(root, None, true, transaction, cancelled)?;
    add_deny_write_ace_on_tree_inner(root, psid, None, transaction, cancelled)
}

unsafe fn add_deny_write_ace_on_tree_inner(
    path: &Path,
    psid: *mut c_void,
    expected_final_path: Option<&Path>,
    transaction: &mut AclTreeTransaction,
    cancelled: &dyn Fn() -> Result<bool>,
) -> Result<usize> {
    if cancelled()? {
        anyhow::bail!("workspace ACL setup cancelled");
    }
    let (handle, attributes, link_count, final_path) = open_pinned_tree_handle(path)?;
    if let Some(expected) = expected_final_path
        && !same_final_path(&final_path, expected)
    {
        return Err(anyhow!(
            "protected tree path was replaced during traversal: expected {} opened {}",
            expected.display(),
            final_path.display()
        ));
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0 && link_count > 1 {
        return Err(anyhow!(
            "unsupported hard-linked protected file {} has {link_count} links; refusing ACL mutation because every link cannot be proven inside the protected tree",
            path.display()
        ));
    }
    let mut changed = usize::from(set_deny_ace_on_handle(
        handle.0,
        psid,
        DenyAceKind::Write,
        Some(transaction),
        false,
    )?);
    if (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        // Protect the link object itself, but never enumerate through its target.
        return Ok(changed);
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        for entry in std::fs::read_dir(path)
            .map_err(|err| anyhow!("enumerate pinned protected dir {}: {err}", path.display()))?
        {
            let entry = entry
                .map_err(|err| anyhow!("enumerate protected child of {}: {err}", path.display()))?;
            let expected_child = final_path.join(entry.file_name());
            changed += add_deny_write_ace_on_tree_inner(
                &entry.path(),
                psid,
                Some(&expected_child),
                transaction,
                cancelled,
            )?;
        }
    }
    Ok(changed)
}

#[derive(Clone, Copy)]
enum DenyAceKind {
    #[cfg(test)]
    DeleteOnly,
    Read,
    Write,
}

impl DenyAceKind {
    fn mask(self) -> u32 {
        match self {
            #[cfg(test)]
            Self::DeleteOnly => DELETE,
            Self::Read => FILE_GENERIC_READ | GENERIC_READ_MASK,
            Self::Write => {
                FILE_GENERIC_WRITE
                    | FILE_WRITE_DATA
                    | FILE_APPEND_DATA
                    | FILE_WRITE_EA
                    | FILE_WRITE_ATTRIBUTES
                    | GENERIC_WRITE_MASK
                    | DELETE
                    | FILE_DELETE_CHILD
            }
        }
    }
}

/// Idempotency check for deny insertion: is there an *explicit* (neither
/// inherited nor inherit-only) deny ACE for `psid` covering **every** bit of
/// `required_mask` after generic mapping?
///
/// Track A fix: the donor scan counted any single matching bit (and inherited
/// ACEs) as "present", so a partial deny — e.g. write-denied but not
/// DELETE-denied — suppressed the full deny grant and left the sandbox able
/// to delete protected content.
unsafe fn dacl_has_explicit_deny_mask(
    dacl: *mut ACL,
    psid: *mut c_void,
    required_mask: u32,
) -> bool {
    if dacl.is_null() {
        return false;
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        dacl,
        &mut info as *mut _ as *mut c_void,
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return false;
    }
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut required_mask = required_mask;
    MapGenericMask(&mut required_mask, &mapping);
    for index in 0..info.AceCount {
        let mut ace_ptr = std::ptr::null_mut();
        if GetAce(dacl, index, &mut ace_ptr) == 0 {
            continue;
        }
        let header = &*(ace_ptr as *const ACE_HEADER);
        if header.AceType != ACCESS_DENIED_ACE_TYPE
            || (header.AceFlags & (INHERIT_ONLY_ACE | INHERITED_ACE)) != 0
        {
            continue;
        }
        let ace = &*(ace_ptr as *const ACCESS_DENIED_ACE);
        let sid_ptr = (ace_ptr as usize
            + std::mem::size_of::<ACE_HEADER>()
            + std::mem::size_of::<u32>()) as *mut c_void;
        let mut ace_mask = ace.Mask;
        MapGenericMask(&mut ace_mask, &mapping);
        if EqualSid(sid_ptr, psid) != 0 && (ace_mask & required_mask) == required_mask {
            return true;
        }
    }
    false
}

/// Fail-closed variant of the explicit-deny scan over multiple SIDs: every
/// enumeration anomaly (failed `GetAce`, out-of-bounds or truncated ACE,
/// unsupported deny ACE type, invalid SID) is an error, never a silent skip.
/// `force_enumeration_failure` exists so tests can prove the fail-closed path.
unsafe fn dacl_has_explicit_deny_mask_strict(
    dacl: *mut ACL,
    psids: &[*mut c_void],
    required_mask: u32,
    force_enumeration_failure: bool,
) -> Result<bool> {
    if dacl.is_null() {
        return Err(anyhow!(
            "strict explicit-deny ACL unexpectedly has a null DACL"
        ));
    }
    let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if force_enumeration_failure
        || GetAclInformation(
            dacl,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
    {
        return Err(anyhow!("strict explicit-deny ACL enumeration failed"));
    }
    let acl_start = dacl as usize;
    let acl_end = acl_start
        .checked_add(info.AclBytesInUse as usize)
        .ok_or_else(|| anyhow!("strict explicit-deny ACL bounds overflow"))?;
    let declared_size = (*dacl).AclSize as u32;
    if info.AclBytesInUse < std::mem::size_of::<ACL>() as u32
        || info.AclBytesInUse > declared_size
        || info.AclBytesFree > declared_size - info.AclBytesInUse
    {
        return Err(anyhow!(
            "strict explicit-deny ACL size information is inconsistent"
        ));
    }
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    let mut required_mask = required_mask;
    MapGenericMask(&mut required_mask, &mapping);
    for index in 0..info.AceCount {
        let mut ace_ptr = std::ptr::null_mut();
        if GetAce(dacl, index, &mut ace_ptr) == 0 {
            return Err(anyhow!("strict explicit-deny ACL GetAce({index}) failed"));
        }
        let ace_start = ace_ptr as usize;
        let header_end = ace_start
            .checked_add(std::mem::size_of::<ACE_HEADER>())
            .ok_or_else(|| anyhow!("strict explicit-deny ACE header bounds overflow"))?;
        if ace_start < acl_start || header_end > acl_end {
            return Err(anyhow!(
                "strict explicit-deny ACE header is outside the ACL"
            ));
        }
        let header = &*(ace_ptr as *const ACE_HEADER);
        let ace_end = ace_start
            .checked_add(header.AceSize as usize)
            .ok_or_else(|| anyhow!("strict explicit-deny ACE bounds overflow"))?;
        if ace_end > acl_end || header.AceSize < std::mem::size_of::<ACE_HEADER>() as u16 {
            return Err(anyhow!("strict explicit-deny ACE size is invalid"));
        }
        let explicit_effective = (header.AceFlags & (INHERIT_ONLY_ACE | INHERITED_ACE)) == 0;
        if explicit_effective && matches!(header.AceType, 6 | 10 | 12) {
            return Err(anyhow!(
                "strict explicit-deny ACL contains unsupported deny ACE type {}",
                header.AceType
            ));
        }
        if header.AceType != ACCESS_DENIED_ACE_TYPE || !explicit_effective {
            continue;
        }
        let sid_start = ace_start
            .checked_add(std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
            .ok_or_else(|| anyhow!("strict explicit-deny SID offset overflow"))?;
        if sid_start.checked_add(8).is_none_or(|end| end > ace_end) {
            return Err(anyhow!("strict explicit-deny ACE has a truncated SID"));
        }
        let subauthority_count = *((sid_start + 1) as *const u8) as usize;
        let sid_len = 8usize
            .checked_add(4usize.saturating_mul(subauthority_count))
            .ok_or_else(|| anyhow!("strict explicit-deny SID length overflow"))?;
        if sid_start
            .checked_add(sid_len)
            .is_none_or(|end| end > ace_end)
        {
            return Err(anyhow!("strict explicit-deny ACE SID exceeds ACE bounds"));
        }
        let sid = sid_start as *mut c_void;
        if IsValidSid(sid) == 0 {
            return Err(anyhow!("strict explicit-deny ACE contains an invalid SID"));
        }
        if !psids.iter().any(|candidate| EqualSid(sid, *candidate) != 0) {
            continue;
        }
        let mut ace_mask = (*(ace_ptr as *const ACCESS_DENIED_ACE)).Mask;
        MapGenericMask(&mut ace_mask, &mapping);
        if (ace_mask & required_mask) == required_mask {
            return Ok(true);
        }
    }
    Ok(false)
}

unsafe fn set_deny_ace_on_handle(
    handle: HANDLE,
    psid: *mut c_void,
    kind: DenyAceKind,
    transaction: Option<&mut AclTreeTransaction>,
    force_verification_failure: bool,
) -> Result<bool> {
    let required_mask = kind.mask();
    let (dacl, sd) = fetch_dacl_for_handle(handle)?;
    if dacl_has_explicit_deny_mask(dacl, psid, required_mask) {
        LocalFree(sd as HLOCAL);
        return Ok(false);
    }
    if let Some(transaction) = transaction
        && let Err(err) = transaction.snapshot_before_mutation(handle)
    {
        LocalFree(sd as HLOCAL);
        return Err(err);
    }
    let explicit = EXPLICIT_ACCESS_W {
        grfAccessPermissions: required_mask,
        grfAccessMode: DENY_ACCESS,
        grfInheritance: CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: psid as *mut u16,
        },
    };
    let mut new_dacl = std::ptr::null_mut();
    let code = SetEntriesInAclW(1, &explicit, dacl, &mut new_dacl);
    LocalFree(sd as HLOCAL);
    if code != ERROR_SUCCESS {
        return Err(anyhow!("SetEntriesInAclW for pinned deny failed: {code}"));
    }
    let set_code = SetSecurityInfo(
        handle,
        SE_FILE_OBJECT,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        new_dacl,
        std::ptr::null_mut(),
    );
    LocalFree(new_dacl as HLOCAL);
    if set_code != ERROR_SUCCESS {
        return Err(anyhow!(
            "SetSecurityInfo for pinned deny failed: {set_code}"
        ));
    }
    let (verify_dacl, verify_sd) = fetch_dacl_for_handle(handle)?;
    let verified = dacl_has_explicit_deny_mask(verify_dacl, psid, required_mask);
    LocalFree(verify_sd as HLOCAL);
    if !verified || force_verification_failure {
        return Err(anyhow!("pinned explicit deny ACL verification failed"));
    }
    Ok(true)
}

unsafe fn add_deny_ace(path: &Path, psid: *mut c_void, kind: DenyAceKind) -> Result<bool> {
    add_deny_ace_impl(path, psid, kind, false, false)
}

/// Single-object deny with the transaction discipline of the tree variants.
///
/// Track A fix for the donor's fail-open write path: the donor returned
/// `Ok(false)` — indistinguishable from "deny already present" — when
/// `SetEntriesInAclW`/`SetNamedSecurityInfoW` failed, and never verified the
/// landed DACL. Here every set failure is an error, the result is verified
/// against a fresh fetch, and any failure after the snapshot rolls the object
/// back to its exact prior ACL bytes and protection state.
unsafe fn add_deny_ace_impl(
    path: &Path,
    psid: *mut c_void,
    kind: DenyAceKind,
    force_verification_failure: bool,
    force_rollback_failure: bool,
) -> Result<bool> {
    let (handle, attributes, link_count, _) = open_pinned_tree_handle(path)?;
    if (attributes & FILE_ATTRIBUTE_DIRECTORY) == 0 && link_count > 1 {
        return Err(anyhow!(
            "unsupported hard-linked file {} has {link_count} links; refusing deny ACL mutation",
            path.display()
        ));
    }
    let mut transaction = AclTreeTransaction::new();
    let changed = match set_deny_ace_on_handle(
        handle.0,
        psid,
        kind,
        Some(&mut transaction),
        force_verification_failure,
    ) {
        Ok(changed) => changed,
        Err(err) => {
            if force_rollback_failure && let Some(record) = transaction.records.first_mut() {
                CloseHandle(record.handle.0);
                record.handle.0 = 0;
            }
            if let Err(rollback) = transaction.rollback_now() {
                return Err(anyhow!(
                    "standalone deny ACL update failed: {err}; rollback failed: {rollback}"
                ));
            }
            return Err(err);
        }
    };
    transaction.commit()?;
    Ok(changed)
}

/// Adds a deny ACE to prevent reads for the given SID on the target path.
///
/// `SetEntriesInAclW` places newly-created deny ACEs before allow ACEs, which
/// keeps the resulting DACL in the order Windows expects for denies to win.
/// The ACE is inheritable so a deny applied to a materialized directory also
/// covers files and directories later created underneath it.
///
/// # Safety
/// Caller must ensure `psid` points to a valid SID and `path` refers to an existing file or directory.
pub unsafe fn add_deny_read_ace(path: &Path, psid: *mut c_void) -> Result<bool> {
    add_deny_ace(path, psid, DenyAceKind::Read)
}

/// Removes ACEs for `psid` from the DACL of `path` (best effort).
///
/// # Safety
/// `psid` must be a valid SID pointer. The process token must hold
/// `WRITE_DAC` access to `path`; failures are ignored by design.
pub unsafe fn revoke_ace(path: &Path, psid: *mut c_void) {
    let mut p_sd: *mut c_void = std::ptr::null_mut();
    let mut p_dacl: *mut ACL = std::ptr::null_mut();
    let code = GetNamedSecurityInfoW(
        to_wide(path).as_ptr(),
        1,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut p_dacl,
        std::ptr::null_mut(),
        &mut p_sd,
    );
    if code != ERROR_SUCCESS {
        if !p_sd.is_null() {
            LocalFree(p_sd as HLOCAL);
        }
        return;
    }
    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: 0,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: psid as *mut u16,
    };
    let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
    explicit.grfAccessPermissions = 0;
    explicit.grfAccessMode = 4; // REVOKE_ACCESS
    explicit.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    explicit.Trustee = trustee;
    let mut p_new_dacl: *mut ACL = std::ptr::null_mut();
    let code2 = SetEntriesInAclW(1, &explicit, p_dacl, &mut p_new_dacl);
    if code2 == ERROR_SUCCESS {
        let _ = SetNamedSecurityInfoW(
            to_wide(path).as_ptr() as *mut u16,
            1,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            p_new_dacl,
            std::ptr::null_mut(),
        );
        if !p_new_dacl.is_null() {
            LocalFree(p_new_dacl as HLOCAL);
        }
    }
    if !p_sd.is_null() {
        LocalFree(p_sd as HLOCAL);
    }
}

/// Grants RX to the null device for the given SID to support stdout/stderr redirection.
///
/// # Safety
/// Caller must ensure `psid` is a valid SID pointer.
pub unsafe fn allow_null_device(psid: *mut c_void) {
    let desired = 0x00020000 | 0x00040000; // READ_CONTROL | WRITE_DAC
    let h = CreateFileW(
        to_wide(r"\\\\.\\NUL").as_ptr(),
        desired,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        std::ptr::null_mut(),
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        0,
    );
    if h == 0 || h == INVALID_HANDLE_VALUE {
        return;
    }
    let mut p_sd: *mut c_void = std::ptr::null_mut();
    let mut p_dacl: *mut ACL = std::ptr::null_mut();
    let code = GetSecurityInfo(
        h,
        SE_KERNEL_OBJECT as i32,
        DACL_SECURITY_INFORMATION,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut p_dacl,
        std::ptr::null_mut(),
        &mut p_sd,
    );
    if code == ERROR_SUCCESS {
        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: psid as *mut u16,
        };
        let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
        explicit.grfAccessPermissions =
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE;
        explicit.grfAccessMode = 2; // SET_ACCESS
        explicit.grfInheritance = 0;
        explicit.Trustee = trustee;
        let mut p_new_dacl: *mut ACL = std::ptr::null_mut();
        let code2 = SetEntriesInAclW(1, &explicit, p_dacl, &mut p_new_dacl);
        if code2 == ERROR_SUCCESS {
            let _ = SetSecurityInfo(
                h,
                SE_KERNEL_OBJECT as i32,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                p_new_dacl,
                std::ptr::null_mut(),
            );
            if !p_new_dacl.is_null() {
                LocalFree(p_new_dacl as HLOCAL);
            }
        }
    }
    if !p_sd.is_null() {
        LocalFree(p_sd as HLOCAL);
    }
    CloseHandle(h);
}
const CONTAINER_INHERIT_ACE: u32 = 0x2;
const OBJECT_INHERIT_ACE: u32 = 0x1;

#[cfg(test)]
mod nano_tests {
    //! Track-B exercise tests (not from the donor): prove DACL enforcement
    //! against the real filesystem on the current host.
    use super::*;
    use crate::token::LocalSid;
    use crate::token::create_workspace_write_token_with_caps_from;
    use crate::token::get_current_token_for_restriction;
    use crate::winutil::resolve_sid;
    use rand::RngCore;
    use rand::SeedableRng;
    use rand::rngs::SmallRng;
    use std::fs;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Security::AccessCheck;
    use windows_sys::Win32::Security::Authorization::GRANT_ACCESS;
    use windows_sys::Win32::Security::DuplicateTokenEx;
    use windows_sys::Win32::Security::GROUP_SECURITY_INFORMATION;
    use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
    use windows_sys::Win32::Security::PRIVILEGE_SET;
    use windows_sys::Win32::Security::SecurityImpersonation;
    use windows_sys::Win32::Security::TOKEN_DUPLICATE;
    use windows_sys::Win32::Security::TOKEN_IMPERSONATE;
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::Security::TokenImpersonation;
    use windows_sys::Win32::System::Threading::SetThreadToken;

    /// Adds an allow ACE with `mask` for `psid` on `path` (test-only mirror
    /// of `add_deny_ace`, so probe identities start from a known DACL).
    unsafe fn add_allow_ace(path: &Path, psid: *mut c_void, mask: u32) -> Result<()> {
        let mut p_sd: *mut c_void = std::ptr::null_mut();
        let mut p_dacl: *mut ACL = std::ptr::null_mut();
        let code = GetNamedSecurityInfoW(
            to_wide(path).as_ptr(),
            1, // SE_FILE_OBJECT
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut p_dacl,
            std::ptr::null_mut(),
            &mut p_sd,
        );
        if code != ERROR_SUCCESS {
            return Err(anyhow!("GetNamedSecurityInfoW failed: {code}"));
        }
        let trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: psid as *mut u16,
        };
        let mut explicit: EXPLICIT_ACCESS_W = std::mem::zeroed();
        explicit.grfAccessPermissions = mask;
        explicit.grfAccessMode = GRANT_ACCESS;
        explicit.grfInheritance = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
        explicit.Trustee = trustee;
        let mut p_new_dacl: *mut ACL = std::ptr::null_mut();
        let code2 = SetEntriesInAclW(1, &explicit, p_dacl, &mut p_new_dacl);
        if code2 == ERROR_SUCCESS {
            let code3 = SetNamedSecurityInfoW(
                to_wide(path).as_ptr() as *mut u16,
                1, // SE_FILE_OBJECT
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                p_new_dacl,
                std::ptr::null_mut(),
            );
            if !p_new_dacl.is_null() {
                LocalFree(p_new_dacl as HLOCAL);
            }
            if !p_sd.is_null() {
                LocalFree(p_sd as HLOCAL);
            }
            if code3 != ERROR_SUCCESS {
                return Err(anyhow!("SetNamedSecurityInfoW failed: {code3}"));
            }
            return Ok(());
        }
        if !p_sd.is_null() {
            LocalFree(p_sd as HLOCAL);
        }
        Err(anyhow!("SetEntriesInAclW failed: {code2}"))
    }

    /// Builds a non-admin impersonation token carrying `psid_capability` as a
    /// restricting capability SID — the same restricted-token construction
    /// the sandbox launcher uses for sandboxed processes (LUA token, max
    /// privileges stripped, write-restricted).
    fn non_admin_impersonation_token(psid_capability: *mut c_void) -> Result<HANDLE> {
        unsafe {
            let base = get_current_token_for_restriction()?;
            let restricted = create_workspace_write_token_with_caps_from(base, &[psid_capability]);
            CloseHandle(base);
            let restricted = restricted?;
            let mut impersonation: HANDLE = 0;
            let ok = DuplicateTokenEx(
                restricted,
                TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_DUPLICATE,
                std::ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut impersonation,
            );
            CloseHandle(restricted);
            if ok == 0 {
                return Err(anyhow!("DuplicateTokenEx failed: {}", GetLastError()));
            }
            Ok(impersonation)
        }
    }

    /// Runs `f` with `token` impersonated on the current thread.
    fn impersonated<T>(token: HANDLE, f: impl FnOnce() -> T) -> T {
        unsafe {
            assert_ne!(
                SetThreadToken(std::ptr::null(), token),
                0,
                "SetThreadToken(impersonate) failed: {}",
                GetLastError()
            );
            let result = f();
            assert_ne!(
                SetThreadToken(std::ptr::null(), 0),
                0,
                "SetThreadToken(revert) failed: {}",
                GetLastError()
            );
            result
        }
    }

    /// Effective access check: does `token` get `desired_mask` on `path`?
    /// Pure (security descriptor, token) evaluation — the harness process
    /// opens nothing, so privileges it may hold (e.g. an elevated CI
    /// runneradmin) cannot skew the result.
    fn access_check_allows(path: &Path, token: HANDLE, desired_mask: u32) -> Result<bool> {
        unsafe {
            let mut p_sd: *mut c_void = std::ptr::null_mut();
            let mut p_owner: *mut c_void = std::ptr::null_mut();
            let mut p_group: *mut c_void = std::ptr::null_mut();
            let mut p_dacl: *mut ACL = std::ptr::null_mut();
            let code = GetNamedSecurityInfoW(
                to_wide(path).as_ptr(),
                1, // SE_FILE_OBJECT
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut p_owner,
                &mut p_group,
                &mut p_dacl,
                std::ptr::null_mut(),
                &mut p_sd,
            );
            if code != ERROR_SUCCESS {
                return Err(anyhow!("GetNamedSecurityInfoW failed: {code}"));
            }
            let mapping = GENERIC_MAPPING {
                GenericRead: FILE_GENERIC_READ,
                GenericWrite: FILE_GENERIC_WRITE,
                GenericExecute: FILE_GENERIC_EXECUTE,
                GenericAll: FILE_ALL_ACCESS,
            };
            let mut privileges = [0u8; 512];
            let mut privileges_len = privileges.len() as u32;
            let mut granted: u32 = 0;
            let mut status: i32 = 0;
            let ok = AccessCheck(
                p_sd,
                token,
                desired_mask,
                &mapping,
                privileges.as_mut_ptr() as *mut PRIVILEGE_SET,
                &mut privileges_len,
                &mut granted,
                &mut status,
            );
            LocalFree(p_sd as HLOCAL);
            if ok == 0 {
                return Err(anyhow!("AccessCheck failed: {}", GetLastError()));
            }
            Ok(status != 0)
        }
    }

    #[test]
    fn deny_write_ace_blocks_write_and_check_reports_denied() {
        let dir = std::env::temp_dir().join(format!("wayland-nano-acl-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        // Baseline: the harness user can write, and the allow-scan reports it.
        let username = std::env::var("USERNAME").expect("USERNAME env");
        let user = resolve_sid(&username).expect("resolve current-user SID");
        let psid = user.as_ptr() as *mut c_void;
        assert!(path_mask_allows(&dir, &[psid], FILE_GENERIC_WRITE, true).unwrap());
        fs::write(dir.join("harness-baseline.txt"), b"x").expect("harness baseline write");

        // Probe identity: a fresh capability-style SID. A fresh temp dir's
        // DACL has no ACE for it, so every ACE naming it is one this test
        // added. Denying the probe — instead of the harness user, as an
        // earlier version of this test did — keeps the harness' own
        // READ_CONTROL intact, so the post-deny probes behave identically for
        // a standard user and for an elevated CI runner (runneradmin), whose
        // privileges would otherwise bypass the harness' self-deny and expose
        // the stale allow ACE that `path_mask_allows` (an allow-only scan)
        // still reports. In the real sandbox flow deny ACEs likewise target
        // the *sandboxed* identity, not the broker.
        let mut rng = SmallRng::from_entropy();
        let probe_sid = LocalSid::from_string(&format!(
            "S-1-5-21-{}-{}-{}-{}",
            rng.next_u32(),
            rng.next_u32(),
            rng.next_u32(),
            rng.next_u32()
        ))
        .expect("valid capability SID string");
        let probe_psid = probe_sid.as_ptr();
        unsafe {
            add_allow_ace(&dir, probe_psid, FILE_GENERIC_WRITE).expect("add probe allow ACE");
        }

        // Non-admin impersonation token carrying the probe capability SID.
        let token = non_admin_impersonation_token(probe_psid).expect("restricted token");

        // Probe baseline before the deny: the allow-scan sees the ACE, the
        // effective check grants write, and the OS lets the token write.
        assert!(path_mask_allows(&dir, &[probe_psid], FILE_GENERIC_WRITE, true).unwrap());
        assert!(access_check_allows(&dir, token, FILE_GENERIC_WRITE).unwrap());
        impersonated(token, || {
            fs::write(dir.join("probe-baseline.txt"), b"x").expect("probe baseline write")
        });

        // Deny write for the probe identity.
        unsafe {
            add_deny_write_ace(&dir, probe_psid).expect("add deny-write ACE");
        }

        // DACL level: the deny ACE landed (deny-before-allow ordering is
        // guaranteed by SetEntriesInAclW).
        unsafe {
            assert!(
                path_has_write_deny_for_sid(&dir, probe_psid)
                    .expect("query deny-write ACE after add"),
                "deny-write ACE for the probe SID must be present"
            );
        }

        // OS level: the non-admin token bearing the denied capability SID can
        // no longer write, even though the DACL still carries the probe's
        // (now overridden) allow ACE.
        let write_result = impersonated(token, || fs::write(dir.join("blocked.txt"), b"x"));
        assert!(
            write_result.is_err(),
            "deny ACE must block the write: {write_result:?}"
        );

        // Effective check: AccessCheck against the non-admin token must
        // report the write mask denied.
        let after = access_check_allows(&dir, token, FILE_GENERIC_WRITE).unwrap();
        assert!(!after, "deny must not report allowed");

        unsafe {
            CloseHandle(token);
            revoke_ace(&dir, probe_psid);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod pinned_tree_tests {
    //! Ported from Track A `windows-sandbox-rs/src/acl.rs` `pinned_tree_tests`
    //! (commits `1dae2d8ae`, `fa0ee4da3`, `9e0e88504`, `1e3400144`): regression
    //! coverage for the transactional ACL machinery — exact rollback, strict
    //! fail-closed parsers, hard-link/reparse refusal, legacy DELETE
    //! verification. Renames only (`G0_...` -> `NANO_...` precondition const).
    use super::ACL;
    use super::ACL_SIZE_INFORMATION;
    use super::AclSizeInformation;
    use super::AclTreeTransaction;
    use super::CloseHandle;
    use super::DACL_SECURITY_INFORMATION;
    use super::DenyAceKind;
    use super::GetAce;
    use super::GetAclInformation;
    use super::PROTECTED_DACL_SECURITY_INFORMATION;
    use super::SE_FILE_OBJECT;
    use super::SetSecurityInfo;
    use super::UNPROTECTED_DACL_SECURITY_INFORMATION;
    use super::add_deny_ace_impl;
    use super::copy_full_acl_allocation;
    use super::dacl_has_explicit_deny_mask_strict;
    use super::dacl_has_inherited_allow_mask_strict;
    use super::fetch_dacl_for_handle;
    use super::open_pinned_tree_handle;
    use super::path_dacl_bytes;
    use super::path_dacl_is_protected;
    use super::same_final_path;
    use std::ffi::c_void;
    use std::fs;
    use windows_sys::Win32::Security::ACL_REVISION;
    use windows_sys::Win32::Security::AddAce;
    use windows_sys::Win32::Security::InitializeAcl;

    fn protect_path_dacl(path: &std::path::Path) {
        let (handle, _, _, _) = open_pinned_tree_handle(path).expect("open path to protect");
        let (dacl, sd) = unsafe { fetch_dacl_for_handle(handle.0).expect("fetch DACL to protect") };
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    handle.0,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    dacl,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        unsafe { super::LocalFree(sd as super::HLOCAL) };
    }

    #[test]
    fn pinned_tree_identity_detects_directory_swap() {
        let temp = tempfile::tempdir().expect("tempdir");
        let original = temp.path().join("original");
        let replacement = temp.path().join("replacement");
        let outside = temp.path().join("outside");
        fs::create_dir(&original).expect("create original");
        fs::create_dir(&outside).expect("create outside");
        fs::write(outside.join("outside.txt"), b"outside").expect("create outside file");
        let (_original_handle, _, _, expected_identity) =
            open_pinned_tree_handle(&original).expect("pin original");
        fs::rename(&original, &replacement).expect("swap pinned path");
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&original)
            .arg(&outside)
            .status()
            .expect("create replacement junction");
        assert!(status.success());
        let (_outside_handle, _, _, outside_identity) =
            open_pinned_tree_handle(&original.join("outside.txt")).expect("open escaped child");

        assert!(!same_final_path(
            &expected_identity.join("outside.txt"),
            &outside_identity
        ));
    }

    #[test]
    fn transaction_restores_full_acl_allocation_and_protection_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("protected");
        fs::create_dir(&path).expect("create protected dir");
        let (handle, _, _, _) = open_pinned_tree_handle(&path).expect("open path");
        let (dacl, sd) = unsafe { fetch_dacl_for_handle(handle.0).expect("fetch ACL") };
        let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    &mut info as *mut _ as *mut c_void,
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        let padded_size = (info.AclBytesInUse as usize + 64 + 3) & !3;
        let mut padded = vec![0u32; padded_size / 4];
        let padded_acl = padded.as_mut_ptr() as *mut ACL;
        assert_ne!(
            unsafe { InitializeAcl(padded_acl, padded_size as u32, ACL_REVISION) },
            0
        );
        for index in 0..info.AceCount {
            let mut ace = std::ptr::null_mut();
            assert_ne!(unsafe { GetAce(dacl, index, &mut ace) }, 0);
            let ace_size = unsafe { (*(ace as *const super::ACE_HEADER)).AceSize as u32 };
            assert_ne!(
                unsafe { AddAce(padded_acl, ACL_REVISION, u32::MAX, ace, ace_size) },
                0
            );
        }
        let (free_capacity_snapshot, snapshot_size) =
            unsafe { copy_full_acl_allocation(padded_acl).expect("snapshot free-capacity ACL") };
        assert_eq!(snapshot_size, padded_size);
        assert_eq!(free_capacity_snapshot.len() * 4, padded_size);
        unsafe { super::LocalFree(sd as super::HLOCAL) };
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    handle.0,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    padded_acl,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let before = path_dacl_bytes(&path).expect("full padded ACL");
        assert!(path_dacl_is_protected(&path).expect("protected before"));

        let mut transaction = AclTreeTransaction::new();
        transaction
            .snapshot_before_mutation(handle.0)
            .expect("snapshot padded ACL");
        let mut empty_words = vec![0u32; 16];
        let empty_acl = empty_words.as_mut_ptr() as *mut ACL;
        assert_ne!(unsafe { InitializeAcl(empty_acl, 64, ACL_REVISION) }, 0);
        assert_eq!(
            unsafe {
                SetSecurityInfo(
                    handle.0,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    empty_acl,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        transaction.rollback_now().expect("rollback exact ACL");

        assert_eq!(path_dacl_bytes(&path).expect("ACL after rollback"), before);
        assert!(path_dacl_is_protected(&path).expect("protected after"));

        let (dacl, sd) = unsafe { fetch_dacl_for_handle(handle.0).expect("fetch strict-test ACL") };
        let before_strict_failure = path_dacl_bytes(&path).expect("ACL before strict failure");
        let error = unsafe {
            dacl_has_inherited_allow_mask_strict(
                dacl,
                &[],
                super::FILE_DELETE_CHILD,
                /*force_enumeration_failure*/ true,
            )
        }
        .expect_err("injected enumeration failure must propagate");
        unsafe { super::LocalFree(sd as super::HLOCAL) };
        assert!(
            error
                .to_string()
                .contains("strict inherited ACL enumeration failed")
        );
        assert_eq!(
            path_dacl_bytes(&path).expect("ACL after strict failure"),
            before_strict_failure,
            "strict preflight failure must not mutate the ACL"
        );

        let mut malformed_words = vec![0u32; 16];
        let malformed_acl = malformed_words.as_mut_ptr() as *mut ACL;
        assert_ne!(unsafe { InitializeAcl(malformed_acl, 64, ACL_REVISION) }, 0);
        let mut malformed_ace = [0u8; 16];
        malformed_ace[0] = super::ACCESS_ALLOWED_ACE_TYPE;
        malformed_ace[1] = super::INHERITED_ACE;
        malformed_ace[2..4].copy_from_slice(&(16u16).to_le_bytes());
        malformed_ace[4..8].copy_from_slice(&super::FILE_DELETE_CHILD.to_le_bytes());
        // A zero SID revision is bounds-consistent but invalid.
        assert_ne!(
            unsafe {
                AddAce(
                    malformed_acl,
                    ACL_REVISION,
                    u32::MAX,
                    malformed_ace.as_ptr() as *const c_void,
                    malformed_ace.len() as u32,
                )
            },
            0
        );
        let malformed_error = unsafe {
            dacl_has_inherited_allow_mask_strict(
                malformed_acl,
                &[],
                super::FILE_DELETE_CHILD,
                false,
            )
        }
        .expect_err("invalid inherited allow SID must fail closed");
        assert!(malformed_error.to_string().contains("invalid SID"));

        let first_ace = unsafe {
            let mut ace = std::ptr::null_mut();
            assert_ne!(GetAce(malformed_acl, 0, &mut ace), 0);
            ace as *mut super::ACE_HEADER
        };
        unsafe { (*first_ace).AceType = 5 };
        let object_error = unsafe {
            dacl_has_inherited_allow_mask_strict(
                malformed_acl,
                &[],
                super::FILE_DELETE_CHILD,
                false,
            )
        }
        .expect_err("nonbasic inherited allow ACE must fail closed");
        assert!(
            object_error
                .to_string()
                .contains("unsupported allow ACE type 5")
        );
    }

    #[test]
    fn strict_explicit_deny_parser_fails_closed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        fs::create_dir(&root).expect("create workspace");
        let (handle, _, _, _) = open_pinned_tree_handle(&root).expect("open workspace");
        let (dacl, sd) = unsafe { fetch_dacl_for_handle(handle.0).expect("fetch workspace DACL") };
        let enumeration_error = unsafe {
            dacl_has_explicit_deny_mask_strict(dacl, &[], DenyAceKind::Write.mask(), true)
        }
        .expect_err("injected enumeration failure must propagate");
        unsafe { super::LocalFree(sd as super::HLOCAL) };
        assert!(enumeration_error.to_string().contains("enumeration failed"));

        let mut malformed_words = vec![0u32; 16];
        let malformed_acl = malformed_words.as_mut_ptr() as *mut ACL;
        assert_ne!(unsafe { InitializeAcl(malformed_acl, 64, ACL_REVISION) }, 0);
        let mut malformed_ace = [0u8; 16];
        malformed_ace[0] = super::ACCESS_DENIED_ACE_TYPE;
        malformed_ace[2..4].copy_from_slice(&(16u16).to_le_bytes());
        malformed_ace[4..8].copy_from_slice(&DenyAceKind::Write.mask().to_le_bytes());
        assert_ne!(
            unsafe {
                AddAce(
                    malformed_acl,
                    ACL_REVISION,
                    u32::MAX,
                    malformed_ace.as_ptr() as *const c_void,
                    malformed_ace.len() as u32,
                )
            },
            0
        );
        let malformed_error = unsafe {
            dacl_has_explicit_deny_mask_strict(malformed_acl, &[], DenyAceKind::Write.mask(), false)
        }
        .expect_err("invalid explicit deny SID must fail closed");
        assert!(malformed_error.to_string().contains("invalid SID"));

        let first_ace = unsafe {
            let mut ace = std::ptr::null_mut();
            assert_ne!(GetAce(malformed_acl, 0, &mut ace), 0);
            ace as *mut super::ACE_HEADER
        };
        unsafe { (*first_ace).AceType = 6 };
        let unsupported_error = unsafe {
            dacl_has_explicit_deny_mask_strict(malformed_acl, &[], DenyAceKind::Write.mask(), false)
        }
        .expect_err("object-specific explicit deny must fail closed");
        assert!(
            unsupported_error
                .to_string()
                .contains("unsupported deny ACE type 6")
        );
    }

    #[test]
    fn recursive_deny_closes_inheritance_disabled_direct_allow_descendants() {
        let temp = tempfile::tempdir().expect("tempdir");
        let protected = temp.path().join("protected");
        let nested = protected.join("nested");
        let file = nested.join("state.json");
        fs::create_dir_all(&nested).expect("create protected tree");
        fs::write(&file, b"state").expect("create protected file");
        let sid = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-901-902-903-904").expect("test SID")
        };
        unsafe {
            super::ensure_allow_mask_aces(&nested, &[sid], super::WRITE_ALLOW_MASK)
                .expect("seed direct nested allow");
            super::ensure_allow_mask_aces(&file, &[sid], super::WRITE_ALLOW_MASK)
                .expect("seed direct file allow");
        }
        protect_path_dacl(&nested);
        assert!(
            super::path_mask_allows(&file, &[sid], super::FILE_GENERIC_WRITE, true)
                .expect("confirm stale descendant allow")
        );

        unsafe { super::add_deny_write_ace_on_tree(&protected, sid) }
            .expect("protect entire carveout");

        assert!(unsafe { super::path_has_write_deny_for_sid(&nested, sid) }.expect("nested deny"));
        assert!(unsafe { super::path_has_write_deny_for_sid(&file, sid) }.expect("file deny"));
        unsafe { super::LocalFree(sid as super::HLOCAL) };
    }

    #[test]
    fn cancellable_workspace_acl_traversal_rolls_back_partial_mutation() {
        use std::cell::Cell;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        fs::create_dir_all(root.join("child")).expect("create workspace tree");
        let before = path_dacl_bytes(&root).expect("root ACL before cancellation");
        let psid = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-123-456-789-1001").expect("test SID")
        };
        let checks = Cell::new(0usize);
        let cancelled = || {
            let next = checks.get() + 1;
            checks.set(next);
            Ok(next >= 6)
        };
        let mut transaction = AclTreeTransaction::new();
        let error = unsafe {
            super::ensure_allow_write_aces_on_tree_in_transaction_cancellable(
                &root,
                &[psid],
                &mut transaction,
                &cancelled,
            )
        }
        .expect_err("recursive ACL traversal should observe cancellation");
        transaction
            .rollback_now()
            .expect("cancelled ACL transaction rollback");
        unsafe { super::LocalFree(psid as super::HLOCAL) };

        assert!(error.to_string().contains("workspace ACL setup cancelled"));
        assert_eq!(
            path_dacl_bytes(&root).expect("root ACL after rollback"),
            before
        );
    }

    #[test]
    fn rollback_native_failure_is_returned() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("path");
        fs::create_dir(&path).expect("create path");
        let (handle, _, _, _) = open_pinned_tree_handle(&path).expect("open path");
        let mut transaction = AclTreeTransaction::new();
        transaction
            .snapshot_before_mutation(handle.0)
            .expect("snapshot ACL");
        let record = transaction.records.first_mut().expect("rollback record");
        unsafe { CloseHandle(record.handle.0) };
        record.handle.0 = 0;

        let error = transaction
            .rollback_now()
            .expect_err("native rollback failure must propagate");
        assert!(error.to_string().contains("rollback failed"));
    }

    #[test]
    fn standalone_deny_verification_failure_restores_exact_acl_and_protection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("protected");
        fs::create_dir(&path).expect("create protected path");
        protect_path_dacl(&path);
        let before = path_dacl_bytes(&path).expect("ACL before injected failure");
        let protected_before = path_dacl_is_protected(&path).expect("protection before");
        let psid = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-101-202-303-404").expect("test SID")
        };

        let error = unsafe { add_deny_ace_impl(&path, psid, DenyAceKind::Write, true, false) }
            .expect_err("verification fault must fail");
        unsafe { super::LocalFree(psid as super::HLOCAL) };

        assert!(error.to_string().contains("verification failed"));
        assert_eq!(path_dacl_bytes(&path).expect("ACL after rollback"), before);
        assert_eq!(
            path_dacl_is_protected(&path).expect("protection after rollback"),
            protected_before
        );
    }

    #[test]
    fn standalone_deny_combines_verification_and_rollback_failures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("path");
        fs::create_dir(&path).expect("create path");
        let psid = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-111-222-333-444").expect("test SID")
        };

        let error = unsafe { add_deny_ace_impl(&path, psid, DenyAceKind::Write, true, true) }
            .expect_err("verification and rollback faults must propagate");
        unsafe { super::LocalFree(psid as super::HLOCAL) };
        let message = error.to_string();
        assert!(message.contains("verification failed"));
        assert!(message.contains("rollback failed"));
    }

    #[test]
    fn legacy_delete_grant_is_additive_and_uses_supplied_token_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        fs::create_dir(&root).expect("create workspace");
        let child = root.join("existing.txt");
        fs::write(&child, b"existing").expect("create existing child");
        let cap = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-10-20-30-40").expect("cap SID")
        };
        let token_user = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-11-22-33-44").expect("user SID")
        };
        let preserved = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-12-24-36-48").expect("preserved SID")
        };
        unsafe {
            super::ensure_allow_mask_aces(&root, &[preserved], super::FILE_GENERIC_READ)
                .expect("seed unrelated allow");
            super::ensure_allow_mask_aces(&root, &[cap], super::WRITE_ALLOW_MASK)
                .expect("seed stale root DELETE allow");
            super::ensure_allow_write_aces_on_tree_for_legacy_user(&root, &[cap], Some(token_user))
                .expect("grant legacy delete");
        }
        let future_child = root.join("future.txt");
        fs::write(&future_child, b"future").expect("create future child");
        assert!(
            !super::path_mask_allows(&root, &[token_user], super::DELETE, true)
                .expect("query token-user root DELETE")
        );
        assert!(
            super::path_mask_allows(&child, &[token_user], super::DELETE, true)
                .expect("query token-user child DELETE")
        );
        assert!(
            !super::path_mask_allows(&root, &[cap], super::DELETE, true)
                .expect("query capability root DELETE")
        );
        assert!(
            super::path_mask_allows(&child, &[cap], super::DELETE, true)
                .expect("query capability existing-child DELETE")
        );
        assert!(
            super::path_mask_allows(&future_child, &[cap], super::DELETE, true)
                .expect("query capability inherited future-child DELETE")
        );
        assert!(
            super::path_mask_allows(&root, &[preserved], super::FILE_GENERIC_READ, true)
                .expect("query preserved allow")
        );
        unsafe {
            super::LocalFree(cap as super::HLOCAL);
            super::LocalFree(token_user as super::HLOCAL);
            super::LocalFree(preserved as super::HLOCAL);
        }
    }

    #[test]
    fn legacy_delete_grant_fails_closed_and_rolls_back_on_user_deny() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        fs::create_dir(&root).expect("create workspace");
        let child = root.join("existing.txt");
        fs::write(&child, b"existing").expect("create existing child");
        let cap = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-50-60-70-80").expect("cap SID")
        };
        let token_user = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-55-66-77-88").expect("user SID")
        };
        let everyone =
            unsafe { crate::token::convert_string_sid_to_sid("S-1-1-0").expect("Everyone SID") };
        let (handle, _, _, _) = super::open_pinned_tree_handle(&child).expect("open child");
        unsafe {
            super::set_deny_ace_on_handle(
                handle.0,
                everyone,
                super::DenyAceKind::DeleteOnly,
                None,
                false,
            )
            .expect("seed group DELETE deny");
        }
        let before = super::path_dacl_bytes(&root).expect("ACL before rejected grant");
        let error = unsafe {
            super::ensure_allow_write_aces_on_tree_for_legacy_user(&root, &[cap], Some(token_user))
        }
        .expect_err("applicable user deny must reject grant");
        assert!(error.to_string().contains("remains denied"));
        assert_eq!(
            super::path_dacl_bytes(&root).expect("ACL after rejected grant"),
            before
        );
        unsafe {
            super::LocalFree(cap as super::HLOCAL);
            super::LocalFree(token_user as super::HLOCAL);
            super::LocalFree(everyone as super::HLOCAL);
        }
    }

    #[test]
    fn elevated_write_tree_api_does_not_add_legacy_user_delete() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        fs::create_dir(&root).expect("create workspace");
        let cap = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-90-91-92-93").expect("cap SID")
        };
        let unrelated_user = unsafe {
            crate::token::convert_string_sid_to_sid("S-1-5-21-94-95-96-97").expect("user SID")
        };
        unsafe {
            super::ensure_allow_write_aces_on_tree(&root, &[cap]).expect("grant capability");
        }
        assert!(
            !super::path_mask_allows(&root, &[unrelated_user], super::DELETE, true)
                .expect("query unrelated user DELETE")
        );
        unsafe {
            super::LocalFree(cap as super::HLOCAL);
            super::LocalFree(unrelated_user as super::HLOCAL);
        }
    }

    #[test]
    fn privilege_buffer_rounding_is_aligned_and_checked() {
        let word = std::mem::size_of::<usize>();
        assert_eq!(super::privilege_buffer_words(1).expect("one byte"), 1);
        assert_eq!(
            super::privilege_buffer_words(word + 1).expect("partial word"),
            2
        );
        assert!(super::privilege_buffer_words(0).is_err());
        assert!(super::privilege_buffer_words(usize::MAX).is_err());
    }

    #[test]
    fn quiescent_precondition_documents_unsupported_hard_link_window() {
        assert!(super::NANO_ACL_QUIESCENT_PRECONDITION.contains("quiescent same-user window"));
        assert!(
            super::NANO_ACL_QUIESCENT_PRECONDITION.contains("hard-link mutation is unsupported")
        );
    }
}
