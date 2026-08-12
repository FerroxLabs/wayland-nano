//! Vendored provider-catalog schema + semantic validation (C8 §2).
//!
//! Shared source: compiled into the crate (unit tests validate the vendored
//! JSON through exactly this code) AND `include!`d by `build.rs` (codegen
//! refuses to emit a table from an invalid catalog). One validation path —
//! the build-time gate and the test gate can never diverge.
//!
//! Rules (codex r2: no invented defaults — missing/ambiguous endpoint or
//! wire fields REJECT):
//! - every field is required; there are no defaults to invent;
//! - ids are unique, lowercase `[a-z0-9-]`;
//! - `wire` is a closed enum (`openai-completions` | `anthropic-messages`);
//! - `base_url` is absolute https with a host, no userinfo/query/fragment;
//! - `api_path` starts with `/`;
//! - `env_var` is `[A-Z0-9_]+`;
//! - normalized bearer-env names (see [`normalize_provider_id`]) must not
//!   collide across providers.

// This file is include!d into build.rs as well as compiled as a module;
// the build script uses only `validate_catalog_json` + friends.
#![allow(dead_code)]

use serde::Deserialize;

/// One vendored catalog row, exactly as authored. No serde defaults: a
/// missing field is a parse error, never an invented value.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RawProvider {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub wire: String,
    pub api_path: String,
    pub env_var: String,
    pub proven: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RawCatalog {
    pub version: u32,
    pub providers: Vec<RawProvider>,
}

/// The closed wire enum. Every arm has a production client.
pub const WIRE_OPENAI_COMPLETIONS: &str = "openai-completions";
pub const WIRE_ANTHROPIC_MESSAGES: &str = "anthropic-messages";
pub const WIRE_KINDS: [&str; 2] = [WIRE_OPENAI_COMPLETIONS, WIRE_ANTHROPIC_MESSAGES];

/// Env-name normalization for the injected-bearer contract (C8 §6.3):
/// uppercase, every char outside `[A-Z0-9]` becomes `_`. Punctuation-bearing
/// ids (`google-gemini` → `GOOGLE_GEMINI`) are exactly the case this pins;
/// collisions after normalization are a catalog error (rejected in
/// [`validate_catalog_json`]).
pub fn normalize_provider_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The env var carrying a provider's injected short-lived bearer.
pub fn bearer_env_name(id: &str) -> String {
    format!("WAYLAND_NANO_OAUTH_BEARER_{}", normalize_provider_id(id))
}

/// The env var carrying the bearer's non-secret expiry metadata
/// (`expires_at_unix_secs`).
pub fn bearer_expires_env_name(id: &str) -> String {
    format!("{}_EXPIRES_AT_UNIX_SECS", bearer_env_name(id))
}

/// Parse + validate the vendored catalog JSON. Returns the typed rows or a
/// human-readable, secret-free reason. Fail-closed: ANY violation rejects
/// the whole catalog.
pub fn validate_catalog_json(text: &str) -> Result<RawCatalog, String> {
    let catalog: RawCatalog =
        serde_json::from_str(text).map_err(|e| format!("catalog json: {e}"))?;
    if catalog.version != 1 {
        return Err(format!(
            "catalog version must be 1, got {}",
            catalog.version
        ));
    }
    if catalog.providers.is_empty() {
        return Err("catalog has no providers".to_string());
    }
    let mut ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut bearer_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for p in &catalog.providers {
        if p.id.is_empty()
            || !p
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!("provider id {:?} must be [a-z0-9-]+", p.id));
        }
        if !ids.insert(p.id.as_str()) {
            return Err(format!("duplicate provider id {:?}", p.id));
        }
        if !bearer_names.insert(bearer_env_name(&p.id)) {
            return Err(format!(
                "bearer env-name collision after normalization for id {:?}",
                p.id
            ));
        }
        if p.display_name.trim().is_empty() {
            return Err(format!("provider {:?} has an empty display_name", p.id));
        }
        if !WIRE_KINDS.contains(&p.wire.as_str()) {
            return Err(format!(
                "provider {:?} wire {:?} is not one of {WIRE_KINDS:?}",
                p.id, p.wire
            ));
        }
        validate_base_url(&p.id, &p.base_url)?;
        if !p.api_path.starts_with('/') {
            return Err(format!(
                "provider {:?} api_path {:?} must start with '/'",
                p.id, p.api_path
            ));
        }
        if p.env_var.is_empty()
            || !p
                .env_var
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!(
                "provider {:?} env_var {:?} must be [A-Z0-9_]+",
                p.id, p.env_var
            ));
        }
    }
    Ok(catalog)
}

/// `base_url` must be an absolute https URL with a host and nothing
/// credential- or query-bearing: it is the sole endpoint authority, so any
/// ambiguity is rejected rather than resolved.
fn validate_base_url(id: &str, base_url: &str) -> Result<(), String> {
    let Some(rest) = base_url.strip_prefix("https://") else {
        return Err(format!(
            "provider {id:?} base_url {base_url:?} must be absolute https"
        ));
    };
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.contains('?') {
        return Err(format!(
            "provider {id:?} base_url {base_url:?} has a missing/invalid authority"
        ));
    }
    if base_url.contains('?') || base_url.contains('#') {
        return Err(format!(
            "provider {id:?} base_url {base_url:?} must not carry query/fragment"
        ));
    }
    Ok(())
}
