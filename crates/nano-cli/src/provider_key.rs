//! Per-provider credential resolution (C8 §3/§7) — generalizes `flux_key.rs`.
//!
//! Resolution order per catalog provider:
//! 1. an injected short-lived OAuth bearer (`spec.bearer_env`, Q1b ruling —
//!    Desktop owns refresh; Nano NEVER reads or writes any OAuth token
//!    store), honored only while unexpired per its `expires_at_unix_secs`
//!    metadata;
//! 2. the provider's canonical env var (names EXACTLY as Desktop injects
//!    them, from the vendored catalog — the sole authority);
//! 3. the file named by `<VAR>_FILE` (path, not secret, in env — same
//!    secret-handling rationale as `flux_key.rs`).
//!
//! `flux-router` keeps its existing three-source order (`FLUX_API_KEY` →
//! `FLUX_TEST_KEY` → `FLUX_API_KEY_FILE`) via [`resolve_flux`].
//!
//! Every successfully resolved secret is registered with the credential
//! sanitizer (nano-egress's redaction boundary, C8 §8/B4) AT resolution —
//! registration at startup and at every `set_model` re-resolution is the
//! invariant that keeps echoed credentials out of error surfaces.

use nano_model::provider_catalog::ProviderSpec;

/// A resolved credential, held in memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A static API key (env or `<VAR>_FILE`).
    Key(String),
    /// An injected short-lived OAuth access token + non-secret expiry
    /// metadata. The refresh token never crosses the process boundary.
    Bearer {
        token: String,
        expires_at_unix_secs: Option<u64>,
    },
}

impl Credential {
    /// The secret presented to the provider (registered with the sanitizer
    /// at resolution time).
    pub fn secret(&self) -> &str {
        match self {
            Credential::Key(key) => key,
            Credential::Bearer { token, .. } => token,
        }
    }

    /// A bearer past its `expires_at_unix_secs` is stale — the caller
    /// pre-fails with the typed `oauth_expired` error instead of
    /// dispatching a doomed turn. Keys and expiry-less bearers never stale.
    pub fn is_stale(&self, now_unix_secs: u64) -> bool {
        match self {
            Credential::Key(_) => false,
            Credential::Bearer {
                expires_at_unix_secs,
                ..
            } => expires_at_unix_secs.is_some_and(|exp| now_unix_secs >= exp),
        }
    }
}

/// The outcome of resolving one provider's credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialResolution {
    Resolved(Credential),
    /// A bearer was injected but is already expired (and no static key
    /// resolved) — surfaces as the typed, retryable `oauth_expired`.
    ExpiredBearer,
    Absent,
}

/// Read a `<VAR>_FILE` key file: trimmed contents, or None when the file is
/// missing, unreadable, empty, or fails the platform permissions gate.
///
/// C8 §8 (claude NB): the perms gate is `#[cfg(unix)]`-only and rejects a
/// group/world-accessible key file fail-closed. On Windows POSIX mode bits
/// are fiction (`std::fs` reports the read-only flag), so no POSIX-style
/// verification is attempted there and a file is NEVER rejected solely
/// because Windows perms verification is unsupported — the Windows
/// guarantee is the userData directory ACL on Desktop's write side.
pub fn read_key_file(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    if !key_file_permissions_ok(path.as_ref()) {
        return None;
    }
    let contents = std::fs::read_to_string(path).ok()?;
    let key = contents.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

/// Unix: owner-only (0600 or stricter) — any group/other bit rejects.
/// Windows: no POSIX check (see module docs); the write-side ACL governs.
#[cfg(unix)]
fn key_file_permissions_ok(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.permissions().mode() & 0o077 == 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn key_file_permissions_ok(_path: &std::path::Path) -> bool {
    true
}

/// Flux's unchanged three-source order (`flux_key.rs:9-26`), expressed over
/// an injectable env reader so routing tests drive it without touching the
/// process env.
pub fn resolve_flux(get_env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    for var in ["FLUX_API_KEY", "FLUX_TEST_KEY"] {
        if let Some(key) = get_env(var) {
            let key = key.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    get_env("FLUX_API_KEY_FILE").and_then(|path| read_key_file(&path))
}

/// Resolve one catalog provider's credential per the module's order.
/// Resolved secrets are registered with the sanitizer before returning
/// (the C8 §8 registration invariant). `flux-router` delegates to
/// [`resolve_flux`].
pub fn resolve_credential(
    spec: &ProviderSpec,
    get_env: &dyn Fn(&str) -> Option<String>,
    now_unix_secs: u64,
) -> CredentialResolution {
    if spec.id == "flux-router" {
        return match resolve_flux(get_env) {
            Some(key) => {
                nano_egress::redact::register_credential(&key);
                CredentialResolution::Resolved(Credential::Key(key))
            }
            None => CredentialResolution::Absent,
        };
    }
    // 1. Injected bearer (unexpired only). Present-but-unparseable expiry
    //    metadata voids the bearer (fail-closed); absent metadata means the
    //    bearer carries no expiry claim and is accepted as-is.
    if let Some(raw) = get_env(spec.bearer_env) {
        let token = raw.trim();
        if !token.is_empty() {
            let expiry = match get_env(spec.bearer_expires_env) {
                // Garbage expiry: treat the bearer as absent entirely —
                // never trust a credential whose freshness is unknowable.
                Some(raw_exp) => raw_exp.trim().parse::<u64>().ok(),
                None => Some(u64::MAX), // no metadata: no expiry claim
            };
            if let Some(expires_at_unix_secs) = expiry {
                let fresh =
                    expires_at_unix_secs == u64::MAX || now_unix_secs < expires_at_unix_secs;
                let expires_at_unix_secs =
                    (expires_at_unix_secs != u64::MAX).then_some(expires_at_unix_secs);
                if fresh {
                    nano_egress::redact::register_credential(token);
                    return CredentialResolution::Resolved(Credential::Bearer {
                        token: token.to_string(),
                        expires_at_unix_secs,
                    });
                }
            }
            // Expired (or freshness-unknowable) bearer: fall through to the
            // static-key arms; if none resolve, report the expired bearer.
            return match resolve_static_key(spec, get_env) {
                resolved @ CredentialResolution::Resolved(_) => resolved,
                CredentialResolution::Absent | CredentialResolution::ExpiredBearer => {
                    CredentialResolution::ExpiredBearer
                }
            };
        }
    }
    resolve_static_key(spec, get_env)
}

/// Arms 2+3: canonical env var, then the `<VAR>_FILE` key file.
fn resolve_static_key(
    spec: &ProviderSpec,
    get_env: &dyn Fn(&str) -> Option<String>,
) -> CredentialResolution {
    if let Some(raw) = get_env(spec.env_var) {
        let key = raw.trim();
        if !key.is_empty() {
            nano_egress::redact::register_credential(key);
            return CredentialResolution::Resolved(Credential::Key(key.to_string()));
        }
    }
    let file_var = format!("{}_FILE", spec.env_var);
    if let Some(path) = get_env(&file_var)
        && let Some(key) = read_key_file(&path)
    {
        nano_egress::redact::register_credential(&key);
        return CredentialResolution::Resolved(Credential::Key(key));
    }
    CredentialResolution::Absent
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_model::provider_catalog::provider_by_id;
    use std::collections::BTreeMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'static {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn env_beats_file_and_none_is_absent() {
        let spec = provider_by_id("openai").expect("catalog carries openai");
        let now = 1_000u64;
        // None present.
        assert_eq!(
            resolve_credential(spec, &env_of(&[]), now),
            CredentialResolution::Absent
        );
        // Env resolves.
        let canary = format!("sk-openai-canary-{}", std::process::id());
        assert_eq!(
            resolve_credential(spec, &env_of(&[("OPENAI_API_KEY", &canary)]), now),
            CredentialResolution::Resolved(Credential::Key(canary.clone()))
        );
        // Registered with the sanitizer at resolution.
        assert!(
            !nano_egress::redact::sanitize_text(&format!("echo {canary}")).contains(&canary),
            "resolved key must be registered with the redaction boundary"
        );
        // Blank env does not resolve.
        assert_eq!(
            resolve_credential(spec, &env_of(&[("OPENAI_API_KEY", "   ")]), now),
            CredentialResolution::Absent
        );
    }

    #[test]
    fn bearer_contract_fresh_expired_and_metadata() {
        let spec = provider_by_id("xai").expect("catalog carries xai");
        let now = 1_000u64;
        let bearer = format!("bearer-canary-{}", std::process::id());
        // Fresh bearer wins over a static key.
        let fresh = env_of(&[
            ("WAYLAND_NANO_OAUTH_BEARER_XAI", &bearer),
            ("WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS", "2000"),
            ("XAI_API_KEY", "sk-static"),
        ]);
        assert_eq!(
            resolve_credential(spec, &fresh, now),
            CredentialResolution::Resolved(Credential::Bearer {
                token: bearer.clone(),
                expires_at_unix_secs: Some(2000),
            })
        );
        // Expired bearer + no static key → ExpiredBearer (typed oauth_expired).
        let expired = env_of(&[
            ("WAYLAND_NANO_OAUTH_BEARER_XAI", &bearer),
            ("WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS", "999"),
        ]);
        assert_eq!(
            resolve_credential(spec, &expired, now),
            CredentialResolution::ExpiredBearer
        );
        // Expired bearer + static key → the key resolves (no doomed turn).
        let expired_with_key = env_of(&[
            ("WAYLAND_NANO_OAUTH_BEARER_XAI", &bearer),
            ("WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS", "999"),
            ("XAI_API_KEY", "sk-static"),
        ]);
        assert_eq!(
            resolve_credential(spec, &expired_with_key, now),
            CredentialResolution::Resolved(Credential::Key("sk-static".into()))
        );
        // Garbage expiry metadata voids the bearer (fail-closed).
        let garbage = env_of(&[
            ("WAYLAND_NANO_OAUTH_BEARER_XAI", &bearer),
            (
                "WAYLAND_NANO_OAUTH_BEARER_XAI_EXPIRES_AT_UNIX_SECS",
                "not-a-number",
            ),
        ]);
        assert_eq!(
            resolve_credential(spec, &garbage, now),
            CredentialResolution::ExpiredBearer
        );
        // No expiry metadata: accepted without an expiry claim.
        let no_meta = env_of(&[("WAYLAND_NANO_OAUTH_BEARER_XAI", &bearer)]);
        assert_eq!(
            resolve_credential(spec, &no_meta, now),
            CredentialResolution::Resolved(Credential::Bearer {
                token: bearer,
                expires_at_unix_secs: None,
            })
        );
    }

    #[test]
    fn staleness_clock() {
        let bearer = Credential::Bearer {
            token: "t".into(),
            expires_at_unix_secs: Some(100),
        };
        assert!(!bearer.is_stale(99));
        assert!(bearer.is_stale(100));
        assert!(!Credential::Key("k".into()).is_stale(u64::MAX));
        assert!(
            !Credential::Bearer {
                token: "t".into(),
                expires_at_unix_secs: None,
            }
            .is_stale(u64::MAX)
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_perms_gate_rejects_group_or_world_accessible_key_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir();
        let path = dir.join(format!("nano-c8-keyfile-{}.txt", std::process::id()));
        std::fs::write(&path, "sk-file-canary\n").unwrap();
        // World-readable: rejected fail-closed.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_key_file(path.to_str().unwrap()), None);
        // Group-writable: rejected.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o620)).unwrap();
        assert_eq!(read_key_file(path.to_str().unwrap()), None);
        // Owner-only: accepted.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            read_key_file(path.to_str().unwrap()).as_deref(),
            Some("sk-file-canary")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(not(unix))]
    #[test]
    fn windows_skips_the_posix_perms_gate() {
        // The Windows guarantee is Desktop's userData directory ACL on the
        // write side; Nano must NOT reject a key file over POSIX mode bits
        // that do not exist here (a POSIX-only check would brick Windows).
        let dir = std::env::temp_dir();
        let path = dir.join(format!("nano-c8-keyfile-{}.txt", std::process::id()));
        std::fs::write(&path, "sk-file-canary\n").unwrap();
        assert_eq!(
            read_key_file(path.to_str().unwrap()).as_deref(),
            Some("sk-file-canary")
        );
        let _ = std::fs::remove_file(&path);
    }
}
