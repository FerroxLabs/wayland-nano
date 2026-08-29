//! Owner-controlled key-reference loader. References are opaque provider locators;
//! private key bytes are never returned or persisted by this crate.

use crate::authority::KeyRole;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyReference {
    provider: String,
    reference: String,
    role: KeyRole,
}

impl KeyReference {
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn role(&self) -> KeyRole {
        self.role
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyProviderError {
    #[error("key reference I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("key reference path is not a secure local regular file")]
    InsecurePath,
    #[error("key reference permissions or owner are not restricted")]
    InsecurePermissions,
    #[error("key reference uses the wrong cryptographic role")]
    RoleMismatch,
    #[error("key reference format is invalid")]
    InvalidReference,
}

pub fn load_key_reference(
    path: &Path,
    expected_role: KeyRole,
) -> Result<KeyReference, KeyProviderError> {
    reject_path(path)?;
    let before = std::fs::symlink_metadata(path)?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(KeyProviderError::InsecurePath);
    }
    verify_safe_parents(path)?;
    verify_owner_only(path, &before)?;
    let file = open_no_follow(path)?;
    let opened = identity(&file)?;
    let after_file = open_no_follow(path)?;
    if opened != identity(&after_file)? {
        return Err(KeyProviderError::InsecurePath);
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(KeyProviderError::InvalidReference);
    }
    let reference: KeyReference =
        serde_json::from_slice(&bytes).map_err(|_| KeyProviderError::InvalidReference)?;
    if reference.role != expected_role {
        return Err(KeyProviderError::RoleMismatch);
    }
    if reference.provider != "file" && reference.provider != "os" {
        return Err(KeyProviderError::InvalidReference);
    }
    if reference.reference.is_empty()
        || reference.reference.len() > 512
        || reference.reference.contains(['\r', '\n', '\0'])
    {
        return Err(KeyProviderError::InvalidReference);
    }
    Ok(reference)
}

pub(crate) fn audit_owner_only_path(path: &Path) -> Result<(), KeyProviderError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(KeyProviderError::InsecurePath);
    }
    verify_owner_only(path, &metadata)
}

fn reject_path(path: &Path) -> Result<(), KeyProviderError> {
    if !path.is_absolute() {
        return Err(KeyProviderError::InsecurePath);
    }
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with("\\\\?\\UNC\\") || text.starts_with("//") {
        return Err(KeyProviderError::InsecurePath);
    }
    if text.starts_with("\\\\") && !is_local_extended_drive(&text) {
        return Err(KeyProviderError::InsecurePath);
    }
    #[cfg(windows)]
    if is_remote_drive(path) {
        return Err(KeyProviderError::InsecurePath);
    }
    Ok(())
}

#[cfg(windows)]
fn is_remote_drive(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOTE: u32 = 4;
    let text = path.as_os_str().to_string_lossy();
    let root = if text.starts_with("\\\\?\\") {
        text.get(4..7)
    } else {
        text.get(0..3)
    };
    let Some(root) = root else {
        return true;
    };
    let wide: Vec<u16> = std::ffi::OsStr::new(root)
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_REMOTE }
}

#[cfg(windows)]
fn is_local_extended_drive(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 7
        && value.starts_with("\\\\?\\")
        && bytes[4].is_ascii_alphabetic()
        && bytes[5] == b':'
        && bytes[6] == b'\\'
}

#[cfg(not(windows))]
fn is_local_extended_drive(_value: &str) -> bool {
    false
}

fn verify_safe_parents(path: &Path) -> Result<(), KeyProviderError> {
    let mut current = path.parent();
    while let Some(parent) = current {
        let metadata = std::fs::symlink_metadata(parent)?;
        if metadata.file_type().is_symlink() {
            return Err(KeyProviderError::InsecurePath);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let mode = metadata.permissions().mode();
            let owner = metadata.uid();
            if owner != 0 && owner != unsafe { libc::geteuid() } {
                return Err(KeyProviderError::InsecurePermissions);
            }
            // `S_ISVTX` varies by libc target; MetadataExt::mode is always u32.
            #[allow(clippy::unnecessary_cast)]
            if mode & 0o022 != 0 && mode & libc::S_ISVTX as u32 == 0 {
                return Err(KeyProviderError::InsecurePermissions);
            }
        }
        current = parent.parent();
    }
    Ok(())
}

#[cfg(unix)]
fn verify_owner_only(_path: &Path, metadata: &std::fs::Metadata) -> Result<(), KeyProviderError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mode = metadata.permissions().mode();
    let owner = metadata.uid();
    let effective = unsafe { libc::geteuid() };
    if mode & 0o077 != 0 || owner != effective {
        return Err(KeyProviderError::InsecurePermissions);
    }
    Ok(())
}

#[cfg(windows)]
fn verify_owner_only(path: &Path, _metadata: &std::fs::Metadata) -> Result<(), KeyProviderError> {
    windows_security::audit(path)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<File, KeyProviderError> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> Result<File, KeyProviderError> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & 0x400 != 0 {
        return Err(KeyProviderError::InsecurePath);
    }
    Ok(file)
}

#[cfg(unix)]
fn identity(file: &File) -> Result<(u64, u64), KeyProviderError> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn identity(file: &File) -> Result<(u64, u64), KeyProviderError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(windows)]
mod windows_security {
    use super::KeyProviderError;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree, PSID};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid,
        GetTokenInformation, IsWellKnownSid, OWNER_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
        TokenUser, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub fn audit(path: &Path) -> Result<(), KeyProviderError> {
        let user = current_user_sid()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let mut owner: PSID = ptr::null_mut();
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut sd = ptr::null_mut();
            let rc = GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut sd,
            );
            if rc != 0
                || sd.is_null()
                || owner.is_null()
                || EqualSid(owner, user.as_ptr() as PSID) == 0
                || dacl.is_null()
            {
                if !sd.is_null() {
                    LocalFree(sd as HLOCAL);
                }
                return Err(KeyProviderError::InsecurePermissions);
            }
            let mut user_allow = false;
            for index in 0..u32::from((*dacl).AceCount) {
                let mut ace = ptr::null_mut();
                if GetAce(dacl, index, &mut ace) == 0 || ace.is_null() {
                    LocalFree(sd as HLOCAL);
                    return Err(KeyProviderError::InsecurePermissions);
                }
                let header = &*(ace as *const windows_sys::Win32::Security::ACE_HEADER);
                match header.AceType {
                    0 => {
                        let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
                        let sid = &raw const allowed.SidStart as PSID;
                        if EqualSid(sid, user.as_ptr() as PSID) != 0 {
                            user_allow = true;
                        } else if IsWellKnownSid(sid, WinLocalSystemSid) == 0
                            && IsWellKnownSid(sid, WinBuiltinAdministratorsSid) == 0
                            && allowed.Mask & 0x000D_0116 != 0
                        {
                            LocalFree(sd as HLOCAL);
                            return Err(KeyProviderError::InsecurePermissions);
                        }
                    }
                    1 => {}
                    _ => {
                        LocalFree(sd as HLOCAL);
                        return Err(KeyProviderError::InsecurePermissions);
                    }
                }
            }
            LocalFree(sd as HLOCAL);
            if !user_allow {
                return Err(KeyProviderError::InsecurePermissions);
            }
        }
        Ok(())
    }

    fn current_user_sid() -> Result<Vec<u8>, KeyProviderError> {
        unsafe {
            let mut token: HANDLE = 0;
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(KeyProviderError::InsecurePermissions);
            }
            let mut size = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            let mut buffer = vec![0u8; size as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
            {
                CloseHandle(token);
                return Err(KeyProviderError::InsecurePermissions);
            }
            CloseHandle(token);
            let token_user = &*(buffer.as_ptr() as *const TOKEN_USER);
            let sid_len = GetLengthSid(token_user.User.Sid) as usize;
            let mut sid = vec![0u8; sid_len];
            ptr::copy_nonoverlapping(token_user.User.Sid as *const u8, sid.as_mut_ptr(), sid_len);
            Ok(sid)
        }
    }
}
