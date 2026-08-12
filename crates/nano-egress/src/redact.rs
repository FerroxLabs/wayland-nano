//! Credential-aware sanitization boundary (C8 §8, codex B4).
//!
//! The single registry every resolved credential is registered with — at
//! startup resolution and at each `set_model` re-resolution — and the single
//! scrub every error surface passes through: response-status and
//! response-BODY-derived errors (`nano-model`'s `classify_status`), tracing /
//! stderr diagnostics, and ACP error serialization. A provider that echoes
//! the presented credential inside an error body must not leak it through
//! ANY of those surfaces.
//!
//! Discipline:
//! - registration is value-based (the secret string itself), never name-based;
//! - secrets shorter than [`MIN_SECRET_LEN`] are refused — redacting a short
//!   common substring would mangle legitimate output (and a real credential
//!   is never that short);
//! - sanitization is total substring replacement, so completeness is provable
//!   by construction: after [`sanitize_text`] the registered value cannot
//!   appear in the output. There is no "best effort" mode to fail open
//!   through;
//! - the registry is append-only for the process lifetime. Resolved
//!   credentials never leave it (a session that switched away from a provider
//!   must still have that provider's key redacted from later output).

use std::sync::{OnceLock, RwLock};

/// Below this length a would-be secret is not registered: substring
/// replacement of something short and common would corrupt ordinary text,
/// and no real provider credential is shorter than this.
pub const MIN_SECRET_LEN: usize = 8;

/// The redaction marker substituted for any registered credential.
pub const REDACTED: &str = "[redacted]";

fn registry() -> &'static RwLock<Vec<String>> {
    static REGISTRY: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(Vec::new()))
}

/// Register a resolved credential (provider API key or injected OAuth
/// bearer) with the sanitization boundary. Idempotent; no-ops on empty or
/// implausibly short values. Call at startup resolution AND at every
/// re-resolution (`set_model` switches resolve fresh values).
pub fn register_credential(secret: &str) {
    let secret = secret.trim();
    if secret.len() < MIN_SECRET_LEN {
        return;
    }
    let mut guard = registry().write().unwrap_or_else(|p| p.into_inner());
    if !guard.iter().any(|s| s == secret) {
        guard.push(secret.to_string());
    }
}

/// Scrub every registered credential out of `text`. Total substring
/// replacement: a registered value CANNOT survive in the output, so the
/// caller never has to reason about partial redaction.
pub fn sanitize_text(text: &str) -> String {
    let guard = registry().read().unwrap_or_else(|p| p.into_inner());
    let mut out = text.to_string();
    for secret in guard.iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), REDACTED);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_secret_is_scrubbed_wherever_it_appears() {
        let canary = format!("sk-canary-{}-redact", std::process::id());
        register_credential(&canary);
        let dirty = format!("401 unauthorized: bad key {canary} presented; retry with {canary}");
        let clean = sanitize_text(&dirty);
        assert!(!clean.contains(&canary), "canary must be scrubbed: {clean}");
        assert!(clean.contains(REDACTED));
        // Unregistered lookalikes pass through untouched.
        assert!(sanitize_text("no secrets here").contains("no secrets here"));
    }

    #[test]
    fn short_values_are_not_registered() {
        let short = "abc123";
        register_credential(short);
        assert_eq!(sanitize_text("abc123 stays"), "abc123 stays");
    }
}
