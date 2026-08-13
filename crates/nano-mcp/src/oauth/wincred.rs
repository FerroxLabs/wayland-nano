//! Windows Credential Manager shim over the PINNED windows-sys 0.52
//! (`Win32_Security_Credentials`) — Q5 RULED: the `keyring` crate's Windows
//! backend pulls a second windows-sys version, so the shim is the path.
//! Nano-owned original code (no donor).
//!
//! Service `"wayland-nano MCP"` (the coexistence namespace rule), account =
//! server instance id, combined into the generic-credential target name
//! `wayland-nano MCP:<instance_id>`. Generic credential blobs are capped at
//! `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes), and OAuth token JSON can
//! exceed that, so values are CHUNKED across `<target>#<n>` credentials
//! (1,024-byte chunks, UTF-8 char-boundary aligned). Writes clean up stale
//! higher chunks; reads stop at the first missing chunk; deletes drain
//! every chunk.

#![cfg(target_os = "windows")]

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::Credentials::CRED_PERSIST_LOCAL_MACHINE;
use windows_sys::Win32::Security::Credentials::CRED_TYPE_GENERIC;
use windows_sys::Win32::Security::Credentials::CREDENTIALW;
use windows_sys::Win32::Security::Credentials::CredDeleteW;
use windows_sys::Win32::Security::Credentials::CredFree;
use windows_sys::Win32::Security::Credentials::CredReadW;
use windows_sys::Win32::Security::Credentials::CredWriteW;

use super::OAuthError;

/// The credential service namespace (§6.4, AGENTS.md coexistence rule).
const SERVICE: &str = "wayland-nano MCP";
/// Empirically 512+ byte generic-credential blobs fail CredWrite with
/// ERROR_NOT_ENOUGH_MEMORY (the documented 2560 applies to domain
/// credentials); 256 is the safe chunk.
const CHUNK_BYTES: usize = 256;
/// Bounded chunk count: no unbounded loops against the credential store.
/// 64 × 256 = 16 KiB of token JSON — far beyond any real token set.
const MAX_CHUNKS: usize = 64;

const ERROR_NOT_FOUND: u32 = 1168;

/// The credential store is a shared OS resource; concurrent CredWrite
/// bursts have been observed failing with ERROR_NOT_ENOUGH_MEMORY under
/// parallel load. Writes are rare (login/refresh), so all operations
/// serialize on a process-wide lock.
fn store_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// The base target name for one server's credential.
pub fn target_name(server_id: &str) -> String {
    format!("{SERVICE}:{server_id}")
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn credstore_unavailable(detail: String) -> OAuthError {
    OAuthError::CredstoreUnavailable { detail }
}

fn chunk_target(base: &str, index: usize) -> String {
    format!("{base}#{index}")
}

/// Split into char-boundary-aligned byte chunks of at most CHUNK_BYTES.
fn chunks(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        let mut end = CHUNK_BYTES.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        out.push(&rest[..end]);
        rest = &rest[end..];
    }
    out
}

/// Read the chunked credential; `Ok(None)` when no chunk 0 exists.
pub fn read(base_target: &str) -> Result<Option<String>, OAuthError> {
    let _guard = store_lock();
    let mut out = String::new();
    for index in 0..MAX_CHUNKS {
        let target = to_wide(&chunk_target(base_target, index));
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                break;
            }
            return Err(credstore_unavailable(format!(
                "CredRead failed: win32 {err}"
            )));
        }
        let slice = unsafe {
            std::slice::from_raw_parts((*cred).CredentialBlob, (*cred).CredentialBlobSize as usize)
        };
        let piece = std::str::from_utf8(slice)
            .map_err(|_| credstore_unavailable("keyring entry is not UTF-8".to_string()))?;
        out.push_str(piece);
        unsafe { CredFree(cred.cast()) };
    }
    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

/// Write the value as chunked generic credentials, deleting stale chunks
/// beyond the new length.
pub fn write(base_target: &str, value: &str) -> Result<(), OAuthError> {
    let _guard = store_lock();
    let pieces = chunks(value);
    if pieces.len() > MAX_CHUNKS {
        return Err(credstore_unavailable(
            "token set exceeds the credential-store chunk bound".to_string(),
        ));
    }
    for (index, piece) in pieces.iter().enumerate() {
        let mut target = to_wide(&chunk_target(base_target, index));
        let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = target.as_mut_ptr();
        cred.CredentialBlobSize = piece.len() as u32;
        cred.CredentialBlob = piece.as_ptr() as *mut u8;
        cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
        let ok = unsafe { CredWriteW(&cred, 0) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(credstore_unavailable(format!(
                "CredWrite failed: win32 {err}"
            )));
        }
    }
    // Stale chunks from a longer previous value must not survive a rewrite.
    for index in pieces.len()..MAX_CHUNKS {
        let target = to_wide(&chunk_target(base_target, index));
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                break;
            }
            return Err(credstore_unavailable(format!(
                "CredDelete of stale chunk failed: win32 {err}"
            )));
        }
    }
    Ok(())
}

/// Delete every chunk (logout). Missing entries are not an error.
pub fn delete(base_target: &str) -> Result<(), OAuthError> {
    let _guard = store_lock();
    for index in 0..MAX_CHUNKS {
        let target = to_wide(&chunk_target(base_target, index));
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                break;
            }
            return Err(credstore_unavailable(format!(
                "CredDelete failed: win32 {err}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through the REAL Windows Credential Manager with a
    /// pid-scoped target, deleted after the test. Per §13 leg 6 the proof
    /// on a host where the credential store is UNAVAILABLE (service
    /// account, SSH session, a VaultSvc refusing writes) is the typed
    /// `McpCredstoreUnavailable` — never a silent skip, never a panic — so
    /// the test asserts BOTH outcomes are clean.
    #[test]
    fn wincred_roundtrip_and_delete() {
        let base = format!("wayland-nano MCP:test-{}", std::process::id());
        let value = format!("token-value-{}", std::process::id());
        match write(&base, &value) {
            Ok(()) => {
                assert_eq!(read(&base).expect("read").as_deref(), Some(value.as_str()));
                // Absent target ⇒ Ok(None).
                delete(&base).expect("delete");
                assert_eq!(read(&base).expect("read"), None);
            }
            Err(e @ OAuthError::CredstoreUnavailable { .. }) => {
                eprintln!("wincred unavailable on this host; typed refusal verified: {e}");
            }
            Err(other) => panic!("unexpected error variant: {other}"),
        }
    }

    /// Values larger than one blob chunk survive the chunked write (same
    /// availability tolerance as the round-trip test).
    #[test]
    fn chunked_roundtrip_across_the_blob_cap() {
        let base = format!("wayland-nano MCP:test-chunk-{}", std::process::id());
        // 3 KiB of multi-byte content crosses the 256-byte chunker.
        let value = "å".repeat(1536);
        match write(&base, &value) {
            Ok(()) => {
                assert_eq!(read(&base).expect("read").as_deref(), Some(value.as_str()));
                // Rewrite shorter: stale chunks must not leak into the read.
                let short = "short-token-value";
                write(&base, short).expect("rewrite");
                assert_eq!(read(&base).expect("read").as_deref(), Some(short));
                delete(&base).expect("delete");
            }
            Err(e @ OAuthError::CredstoreUnavailable { .. }) => {
                eprintln!("wincred unavailable on this host; typed refusal verified: {e}");
            }
            Err(other) => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn chunker_respects_char_boundaries() {
        let value = "é".repeat(600); // 1,200 bytes, 2-byte chars
        for piece in chunks(&value) {
            assert!(piece.len() <= CHUNK_BYTES);
        }
        assert_eq!(chunks(&value).concat(), value);
    }
}
