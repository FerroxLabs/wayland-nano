//! Token storage (P3 §6.4): keyring-primary + operator-provisioned
//! `<VAR>_FILE` refresh path. Codex's plaintext fallback is NOT ported.
//!
//! - **Keyring-primary:** service `"wayland-nano MCP"` (the coexistence
//!   namespace rule), account = server instance_id. On Windows this is a
//!   hand-rolled wincred shim over the PINNED windows-sys 0.52 (Q5 RULED —
//!   the `keyring` crate pulls a second windows-sys version). Secret
//!   Service (Linux) / Keychain (macOS) are DEFERRED per the same ruling:
//!   off-Windows the keyring backend is typed-unavailable and resolution
//!   falls through to the refresh-file path.
//! - **`<VAR>_FILE` refresh path:** `NANO_MCP_OAUTH_REFRESH_FILE_<SERVER>`
//!   names an operator-provisioned file holding the REFRESH token (the
//!   headless/CI context where keyrings are absent). Unix: owner-only 0600
//!   enforced fail-closed. **Windows: TYPED-UNAVAILABLE** — the P2a
//!   blob-store ACL helper is a declared HARD PREREQUISITE (Windows-side
//!   permission checking is an unconditional `true` in `provider_key.rs`
//!   today), so until it lands the Windows file path refuses with
//!   `McpCredstoreUnavailable` naming the keyring alternative.
//! - **Atomic refresh rewrites:** same-directory temp file → 0600 → fsync →
//!   rename current to `<file>.prev` → rename temp over the target →
//!   post-write 0600 audit → delete `.prev`. A crash mid-rewrite leaves the
//!   old or the new value, never a torn file; recovery prefers the audited
//!   new file and falls back to `.prev`.
//! - **Redaction:** access and refresh tokens are registered with the value
//!   sanitizer the moment they exist; storage errors never echo values.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use super::OAuthError;

/// Env var prefix for the operator-provisioned refresh-token file.
pub const REFRESH_FILE_ENV_PREFIX: &str = "NANO_MCP_OAUTH_REFRESH_FILE_";

/// The durable token set. `access_token`/`expires_at_unix` are absent when
/// only the refresh token was loaded (the `<VAR>_FILE` path holds the
/// refresh token only) — the 30s-skew check then refreshes before use.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredTokens {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix: Option<u64>,
}

impl StoredTokens {
    /// Register every token with the redaction boundary AT BIRTH (§6.4
    /// redaction rule). Call the moment tokens exist, before any store or
    /// use.
    pub fn register_with_sanitizer(&self) {
        if let Some(access) = &self.access_token {
            nano_egress::redact::register_credential(access);
        }
        if let Some(refresh) = &self.refresh_token {
            nano_egress::redact::register_credential(refresh);
        }
    }
}

/// The env var naming one server's refresh-token file.
pub fn refresh_file_env_var(server_id: &str) -> String {
    let normalized: String = server_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{REFRESH_FILE_ENV_PREFIX}{normalized}")
}

/// The storage abstraction the OAuth flows program against. Production
/// uses [`CredentialStore`]; tests substitute in-memory stores.
pub trait TokenStorage: Send + Sync {
    /// Load the stored token set. `Ok(None)` = no usable token (the caller
    /// raises `McpAuthorizationRequired` — login needed, without a failed
    /// call first).
    fn load(&self, server_id: &str) -> Result<Option<StoredTokens>, OAuthError>;
    /// Store the full token set (login / refresh with keyring).
    fn store(&self, server_id: &str, tokens: &StoredTokens) -> Result<(), OAuthError>;
    /// Mutate storage after a refresh (§6.5: the §6.4 write path, never a
    /// side channel).
    fn store_after_refresh(&self, server_id: &str, tokens: &StoredTokens)
    -> Result<(), OAuthError>;
    /// Remove all stored state for `server_id` (`auth logout`).
    fn delete(&self, server_id: &str) -> Result<(), OAuthError>;
}

/// The storage facade. Resolution order (§6.4): keyring → refresh-file →
/// typed `McpAuthorizationRequired` at read time; keyring → refresh-file →
/// typed `McpCredstoreUnavailable` at write time (loud, never a silent
/// skip). The env reader is injectable so tests never touch the process
/// env (the `provider_key.rs` pattern).
/// Injectable env reader (the `provider_key.rs` test pattern).
pub type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub struct CredentialStore {
    get_env: EnvReader,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore {
    /// Production store: reads the real process env.
    pub fn new() -> Self {
        Self {
            get_env: Arc::new(|name| std::env::var(name).ok()),
        }
    }

    /// Test seam: scripted env.
    pub fn with_env(get_env: EnvReader) -> Self {
        Self { get_env }
    }

    fn refresh_file_path(&self, server_id: &str) -> Option<PathBuf> {
        let var = refresh_file_env_var(server_id);
        let value = (self.get_env)(&var)?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }
}

impl TokenStorage for CredentialStore {
    fn load(&self, server_id: &str) -> Result<Option<StoredTokens>, OAuthError> {
        match keyring_load(server_id) {
            Ok(Some(tokens)) => {
                tokens.register_with_sanitizer();
                return Ok(Some(tokens));
            }
            Ok(None) => {}
            Err(unavailable) => {
                // Fall through to the refresh-file path; if that is absent
                // too, the keyring error is the loud one.
                let Some(path) = self.refresh_file_path(server_id) else {
                    return Err(unavailable);
                };
                return load_refresh_file(&path).map(|opt| {
                    opt.map(|refresh| {
                        let tokens = StoredTokens {
                            refresh_token: Some(refresh),
                            ..StoredTokens::default()
                        };
                        tokens.register_with_sanitizer();
                        tokens
                    })
                });
            }
        }
        match self.refresh_file_path(server_id) {
            Some(path) => load_refresh_file(&path).map(|opt| {
                opt.map(|refresh| {
                    let tokens = StoredTokens {
                        refresh_token: Some(refresh),
                        ..StoredTokens::default()
                    };
                    tokens.register_with_sanitizer();
                    tokens
                })
            }),
            None => Ok(None),
        }
    }

    /// Store the full token set (login / refresh with keyring). Keyring
    /// first; when the keyring is unavailable the operator-provisioned
    /// refresh file takes the REFRESH token via the atomic rewrite. Neither
    /// ⇒ typed `McpCredstoreUnavailable` (loud at `auth login` time).
    fn store(&self, server_id: &str, tokens: &StoredTokens) -> Result<(), OAuthError> {
        tokens.register_with_sanitizer();
        match keyring_store(server_id, tokens) {
            Ok(()) => Ok(()),
            Err(keyring_err) => {
                let Some(path) = self.refresh_file_path(server_id) else {
                    return Err(keyring_err);
                };
                let Some(refresh) = &tokens.refresh_token else {
                    return Err(OAuthError::CredstoreUnavailable {
                        detail: "token response carried no refresh token and the keyring is \
                                 unavailable"
                            .to_string(),
                    });
                };
                atomic_write_refresh_file(&path, refresh)
            }
        }
    }

    fn store_after_refresh(
        &self,
        server_id: &str,
        tokens: &StoredTokens,
    ) -> Result<(), OAuthError> {
        self.store(server_id, tokens)
    }

    /// Remove all stored state for `server_id` (`auth logout`).
    fn delete(&self, server_id: &str) -> Result<(), OAuthError> {
        keyring_delete(server_id)?;
        if let Some(path) = self.refresh_file_path(server_id) {
            delete_refresh_file(&path)?;
        }
        Ok(())
    }
}

// --- Keyring backend (per-platform) ---------------------------------------

#[cfg(target_os = "windows")]
fn keyring_load(server_id: &str) -> Result<Option<StoredTokens>, OAuthError> {
    match super::wincred::read(&crate::oauth::wincred::target_name(server_id)) {
        Ok(Some(json)) => {
            let tokens: StoredTokens =
                serde_json::from_str(&json).map_err(|_| OAuthError::CredstoreUnavailable {
                    detail: "keyring entry is not a valid token set".to_string(),
                })?;
            Ok(Some(tokens))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "windows")]
fn keyring_store(server_id: &str, tokens: &StoredTokens) -> Result<(), OAuthError> {
    let json = serde_json::to_string(tokens).map_err(|_| OAuthError::CredstoreUnavailable {
        detail: "token set is not serializable".to_string(),
    })?;
    super::wincred::write(&crate::oauth::wincred::target_name(server_id), &json)
}

#[cfg(target_os = "windows")]
fn keyring_delete(server_id: &str) -> Result<(), OAuthError> {
    super::wincred::delete(&crate::oauth::wincred::target_name(server_id))
}

/// Secret Service (Linux) / Keychain (macOS) are DEFERRED (Q5 ruling): the
/// keyring is absent off Windows. At LOAD time absence means "no usable
/// token" (Ok(None) → `McpAuthorizationRequired`, §6.4 resolution order);
/// at STORE time it is the loud typed refusal (`McpCredstoreUnavailable`
/// at `auth login`, naming the file alternative).
#[cfg(not(target_os = "windows"))]
fn keyring_load(_server_id: &str) -> Result<Option<StoredTokens>, OAuthError> {
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
fn keyring_store(_server_id: &str, _tokens: &StoredTokens) -> Result<(), OAuthError> {
    Err(OAuthError::CredstoreUnavailable {
        detail: "OS keyring support is deferred on this platform; provision \
                 NANO_MCP_OAUTH_REFRESH_FILE_<SERVER> (0600) instead"
            .to_string(),
    })
}

#[cfg(not(target_os = "windows"))]
fn keyring_delete(_server_id: &str) -> Result<(), OAuthError> {
    Ok(())
}

// --- Refresh-token file (unix live; Windows typed-unavailable) ------------

/// Windows: TYPED-UNAVAILABLE until the P2a blob-store ACL helper lands
/// (declared HARD PREREQUISITE, §6.4 r2 codex-F15) — the refusal names the
/// keyring alternative.
#[cfg(target_os = "windows")]
fn load_refresh_file(_path: &Path) -> Result<Option<String>, OAuthError> {
    Err(OAuthError::CredstoreUnavailable {
        detail: "file-based credential storage is unavailable on Windows until the ACL helper \
                 lands; use the Windows Credential Manager keyring path"
            .to_string(),
    })
}

#[cfg(target_os = "windows")]
fn atomic_write_refresh_file(_path: &Path, _value: &str) -> Result<(), OAuthError> {
    Err(OAuthError::CredstoreUnavailable {
        detail: "file-based credential storage is unavailable on Windows until the ACL helper \
                 lands; use the Windows Credential Manager keyring path"
            .to_string(),
    })
}

#[cfg(target_os = "windows")]
fn delete_refresh_file(_path: &Path) -> Result<(), OAuthError> {
    Ok(())
}

/// Recovery prefers the audited new file and falls back to `.prev` (§6.4).
#[cfg(not(target_os = "windows"))]
fn load_refresh_file(path: &Path) -> Result<Option<String>, OAuthError> {
    match read_audited(path) {
        Ok(Some(value)) => return Ok(Some(value)),
        Ok(None) => {}
        Err(e) => {
            // Audit/read failure on the new file: try the preserved previous
            // value before surfacing the error.
            if let Ok(Some(prev)) = read_audited(&prev_path(path)) {
                return Ok(Some(prev));
            }
            return Err(e);
        }
    }
    read_audited(&prev_path(path))
}

#[cfg(not(target_os = "windows"))]
fn read_audited(path: &Path) -> Result<Option<String>, OAuthError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if !permissions_owner_only(path)? {
                return Err(OAuthError::CredstoreUnavailable {
                    detail: "refresh-token file failed the owner-only (0600) audit".to_string(),
                });
            }
            let value = contents.trim();
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.to_string()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(OAuthError::CredstoreUnavailable {
            detail: format!("refresh-token file unreadable: {e}"),
        }),
    }
}

/// Owner-only (0600 or stricter) — any group/other bit rejects, fail-closed
/// (the `provider_key.rs` rule; Windows perms verification is the P2a ACL
/// helper's job, which is why the Windows path above is typed-unavailable).
#[cfg(not(target_os = "windows"))]
fn permissions_owner_only(path: &Path) -> Result<bool, OAuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).map_err(|e| OAuthError::CredstoreUnavailable {
            detail: format!("refresh-token file metadata: {e}"),
        })?;
        Ok(meta.permissions().mode() & 0o077 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(not(target_os = "windows"))]
fn prev_path(path: &Path) -> PathBuf {
    let mut prev = path.as_os_str().to_os_string();
    prev.push(".prev");
    PathBuf::from(prev)
}

/// Atomic, recoverable rewrite (§6.4): temp file in the SAME directory →
/// 0600 → fsync → rename target to `.prev` → rename temp over target →
/// POST-WRITE audit re-checks 0600 → `.prev` deleted only after the new
/// value passes audit. A crash mid-rewrite leaves old or new, never torn.
#[cfg(not(target_os = "windows"))]
fn atomic_write_refresh_file(path: &Path, value: &str) -> Result<(), OAuthError> {
    let dir = path
        .parent()
        .ok_or_else(|| OAuthError::CredstoreUnavailable {
            detail: "refresh-token file has no parent directory".to_string(),
        })?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let write_result = (|| -> Result<(), OAuthError> {
        {
            let mut file =
                std::fs::File::create(&tmp).map_err(|e| OAuthError::CredstoreUnavailable {
                    detail: format!("refresh-token temp file: {e}"),
                })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| OAuthError::CredstoreUnavailable {
                        detail: format!("refresh-token chmod: {e}"),
                    })?;
            }
            use std::io::Write;
            file.write_all(value.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .and_then(|()| file.sync_all())
                .map_err(|e| OAuthError::CredstoreUnavailable {
                    detail: format!("refresh-token write/fsync: {e}"),
                })?;
        }
        // Preserve the previous value, then swing the rename.
        if path.exists() {
            std::fs::rename(path, prev_path(path)).map_err(|e| {
                OAuthError::CredstoreUnavailable {
                    detail: format!("refresh-token .prev preserve: {e}"),
                }
            })?;
        }
        std::fs::rename(&tmp, path).map_err(|e| OAuthError::CredstoreUnavailable {
            detail: format!("refresh-token rename: {e}"),
        })?;
        // Post-write audit on the final path.
        if !permissions_owner_only(path)? {
            return Err(OAuthError::CredstoreUnavailable {
                detail: "refresh-token file failed the post-write 0600 audit".to_string(),
            });
        }
        // The new value passed audit: the preserved previous value goes.
        let _ = std::fs::remove_file(prev_path(path));
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}

#[cfg(not(target_os = "windows"))]
fn delete_refresh_file(path: &Path) -> Result<(), OAuthError> {
    for p in [path.to_path_buf(), prev_path(path)] {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(OAuthError::CredstoreUnavailable {
                    detail: format!("refresh-token file delete: {e}"),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "windows"))]
    fn env_with(pairs: &[(&str, &str)]) -> EnvReader {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Arc::new(move |name| map.get(name).cloned())
    }

    #[test]
    fn env_var_naming_is_normalized() {
        assert_eq!(
            refresh_file_env_var("srv_ab12cd34ef56"),
            "NANO_MCP_OAUTH_REFRESH_FILE_SRV_AB12CD34EF56"
        );
        assert_eq!(
            refresh_file_env_var("my-server.1"),
            "NANO_MCP_OAUTH_REFRESH_FILE_MY_SERVER_1"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn no_keyring_and_no_file_is_typed_unavailable_at_store() {
        let store = CredentialStore::with_env(env_with(&[]));
        let tokens = StoredTokens {
            access_token: Some("access-unused".to_string()),
            refresh_token: Some("refresh-unused".to_string()),
            expires_at_unix: None,
        };
        // §12: keyring unavailable + no file ⇒ McpCredstoreUnavailable at
        // login (store) time — loud, and no file is written.
        let err = store
            .store("srv_none", &tokens)
            .expect_err("no backend must be typed");
        assert!(matches!(err, OAuthError::CredstoreUnavailable { .. }));
        // Load with nothing provisioned ⇒ Ok(None): the caller raises
        // McpAuthorizationRequired (login needed) without a failed call.
        assert_eq!(store.load("srv_none").expect("load"), None);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn refresh_file_roundtrip_and_atomic_rewrite() {
        let dir = std::env::temp_dir().join(format!("nano-oauth-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("refresh");
        atomic_write_refresh_file(&path, "refresh-v1-long-enough-to-register").unwrap();
        assert_eq!(
            load_refresh_file(&path).unwrap().as_deref(),
            Some("refresh-v1-long-enough-to-register")
        );
        // Rewrite: previous value preserved across the swap, then cleaned.
        atomic_write_refresh_file(&path, "refresh-v2-long-enough-to-register").unwrap();
        assert_eq!(
            load_refresh_file(&path).unwrap().as_deref(),
            Some("refresh-v2-long-enough-to-register")
        );
        assert!(
            !dir.join("refresh.prev").exists(),
            ".prev is deleted after the new value passes audit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §12: kill mid-refresh ⇒ resume finds the audited new value OR
    /// `.prev`, never a torn file. Simulate the crash window: new file
    /// missing, `.prev` present ⇒ recovery falls back to `.prev`.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn recovery_prefers_new_and_falls_back_to_prev() {
        let dir = std::env::temp_dir().join(format!("nano-oauth-recover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("refresh");
        atomic_write_refresh_file(&path, "refresh-new-long-enough").unwrap();
        // Crash window A: rename to .prev done, temp rename not — only
        // .prev exists.
        std::fs::rename(&path, dir.join("refresh.prev")).unwrap();
        assert_eq!(
            load_refresh_file(&path).unwrap().as_deref(),
            Some("refresh-new-long-enough"),
            "recovery falls back to .prev"
        );
        // Crash window B: both exist (post-rename, pre-cleanup) — the
        // audited new file wins.
        atomic_write_refresh_file(&dir.join("refresh"), "refresh-newer-long-enough").unwrap();
        std::fs::write(dir.join("refresh.prev"), "refresh-older\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.join("refresh.prev"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        assert_eq!(
            load_refresh_file(&path).unwrap().as_deref(),
            Some("refresh-newer-long-enough"),
            "the audited new file is preferred over .prev"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §12: a group/world-readable refresh file is refused fail-closed.
    #[cfg(unix)]
    #[test]
    fn refresh_file_with_group_bits_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nano-oauth-perms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("refresh");
        std::fs::write(&path, "refresh-token-value\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = load_refresh_file(&path).expect_err("0644 must be refused");
        assert!(matches!(err, OAuthError::CredstoreUnavailable { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// §12/§6.4: Windows file storage is TYPED-UNAVAILABLE until the P2a
    /// ACL helper lands (declared hard prerequisite).
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_file_storage_is_typed_unavailable() {
        let path = Path::new("C:\\unused\\refresh");
        let err = load_refresh_file(path).expect_err("windows file path must refuse");
        assert!(matches!(err, OAuthError::CredstoreUnavailable { .. }));
        let err = atomic_write_refresh_file(path, "value").expect_err("write must refuse");
        assert!(matches!(err, OAuthError::CredstoreUnavailable { .. }));
    }

    #[test]
    fn tokens_register_with_the_sanitizer() {
        let canary = format!("oauth-canary-{}", std::process::id());
        let tokens = StoredTokens {
            access_token: Some(canary.clone()),
            refresh_token: None,
            expires_at_unix: None,
        };
        tokens.register_with_sanitizer();
        let scrubbed = nano_egress::redact::sanitize_text(&format!("leaked: {canary}"));
        assert!(
            !scrubbed.contains(&canary),
            "token must be scrubbed everywhere: {scrubbed}"
        );
    }
}
