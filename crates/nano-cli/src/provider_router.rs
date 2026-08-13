//! Provider routing for acp-host (C8 §3/§5/§7/§8): the validated
//! `WAYLAND_NANO_PROVIDERS` payload, the `<provider>:<model>` namespace
//! parser, deterministic startup resolution (B2), and `set_model` →
//! provider+model binding with typed failures.
//!
//! TRUST BOUNDARY (codex B3): the payload is untrusted, advisory metadata.
//! It may only REFERENCE provider ids present in the vendored catalog
//! (nano_model::provider_catalog — the SOLE endpoint authority) and supply
//! `{models, hasKey}` annotations. It can never introduce, override, or
//! redirect an endpoint; unknown provider ids are ignored; a malformed or
//! oversize payload is ignored wholesale with a secret-free diagnostic
//! (`payload_invalid`) and the host falls back to Flux-only advertisement.
//!
//! Typed error kinds (C7 SSOT rows, pinned by the C8 design §8; wire codes
//! ride in `error.data` per C7 D2 — the integrator folds these constants
//! into C7's `nano-protocol/src/error_codes.rs` at merge):
//! `model_not_found` (existing row), `provider_key_missing`,
//! `provider_unproven`, `oauth_expired`, `payload_invalid`.

use nano_model::provider_catalog::{PROVIDERS, ProviderSpec, WireKind, provider_by_id};
use nano_protocol::acp::{AvailableModel, JsonRpcResponse};

use crate::provider_key::{Credential, CredentialResolution, resolve_credential, resolve_flux};

/// Typed error-kind strings (C7 D2 `error.data.kind` values).
pub const KIND_MODEL_NOT_FOUND: &str = "model_not_found";
pub const KIND_PROVIDER_KEY_MISSING: &str = "provider_key_missing";
pub const KIND_PROVIDER_UNPROVEN: &str = "provider_unproven";
pub const KIND_OAUTH_EXPIRED: &str = "oauth_expired";
pub const KIND_PAYLOAD_INVALID: &str = "payload_invalid";
/// P5 §1 step 5: NOTHING routable resolves a credential — distinct from the
/// §5 capability-empty refusal (different kind, different remedy: configure
/// a credential). The integrator folds this constant into C7's
/// `nano-protocol/src/error_codes.rs` at merge, like the rows above.
pub const KIND_NO_CREDENTIAL: &str = "no_credential";
/// P5 §5: capability filtering left NO admissible candidate (unknown
/// capability equals false) — distinct from `no_credential` (the remedy is
/// changing the turn's requirements or proving a leaf, not configuring a
/// credential). Same integrator-fold rule as `KIND_NO_CREDENTIAL`.
pub const KIND_CAPABILITY_EMPTY: &str = "capability_empty";

/// Payload limits (§5 invariants; Desktop bounds its emission identically).
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024;
pub const MAX_PAYLOAD_ENTRIES: usize = 64;
pub const MAX_MODELS_PER_PROVIDER: usize = 256;
pub const MAX_MODEL_ID_CHARS: usize = 128;

/// The respawn hint pre-wired onto `oauth_expired` (C7 retryable path, §7).
pub const OAUTH_EXPIRED_HINT: &str =
    "reconnect/respawn the session to pick up a fresh token from the host";

/// A typed provider-routing failure. Carries NO secrets — provider/model
/// ids and env-var NAMES only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub kind: &'static str,
    pub message: String,
    pub retryable: bool,
    pub hint: Option<&'static str>,
}

impl ProviderError {
    fn model_not_found(model_id: &str) -> Self {
        Self {
            kind: KIND_MODEL_NOT_FOUND,
            // Desktop greps the message for model_not_found — keep the shape.
            message: format!("model_not_found: {model_id} is not in the advertised catalog"),
            retryable: false,
            hint: None,
        }
    }

    fn provider_key_missing(spec: &ProviderSpec) -> Self {
        Self {
            kind: KIND_PROVIDER_KEY_MISSING,
            message: format!(
                "provider_key_missing: no usable credential for {} — connect {} in Wayland, then retry (env {} or {}_FILE)",
                spec.id, spec.display_name, spec.env_var, spec.env_var
            ),
            retryable: false,
            hint: None,
        }
    }

    fn provider_unproven(spec: &ProviderSpec) -> Self {
        Self {
            kind: KIND_PROVIDER_UNPROVEN,
            message: format!(
                "provider_unproven: {} is disabled pending its live proof (no silent fallback)",
                spec.id
            ),
            retryable: false,
            hint: None,
        }
    }

    fn oauth_expired(spec: &ProviderSpec) -> Self {
        Self {
            kind: KIND_OAUTH_EXPIRED,
            message: format!(
                "oauth_expired: the injected bearer for {} has expired",
                spec.id
            ),
            // C7 retryable path, respawn hint pre-wired (§7): the next
            // spawn/respawn picks up a fresh bearer.
            retryable: true,
            hint: Some(OAUTH_EXPIRED_HINT),
        }
    }

    /// P5 §1 step 5: the typed no-credential failure — env-var NAMES only,
    /// never values (the startup `no_credential_message` discipline).
    pub fn no_credential(env_names: &str) -> Self {
        Self {
            kind: KIND_NO_CREDENTIAL,
            message: format!(
                "no_credential: no routable provider resolves a usable credential — set one of: {env_names}"
            ),
            retryable: false,
            hint: None,
        }
    }

    /// P5 §5: the capability-empty refusal — filtering left no admissible
    /// candidate for the turn's requirements. Names the requirement CLASS
    /// (images/tools), never content.
    pub fn capability_empty(requirement: &'static str) -> Self {
        Self {
            kind: KIND_CAPABILITY_EMPTY,
            message: format!(
                "capability_empty: no admitted candidate can prove {requirement} support (unknown capability is false; nothing was dispatched)"
            ),
            retryable: false,
            hint: None,
        }
    }

    /// Serialize as the ACP error frame: -32602 + `error.data` per C7 D2.
    pub fn acp_response(&self, id: serde_json::Value) -> JsonRpcResponse {
        let mut data = serde_json::json!({
            "kind": self.kind,
            "retryable": self.retryable,
        });
        if let Some(hint) = self.hint {
            data["hint"] = serde_json::json!(hint);
        }
        JsonRpcResponse::err_with_data(
            id,
            -32602,
            nano_egress::redact::sanitize_text(&self.message),
            data,
        )
    }
}

/// One validated payload entry: a catalog provider plus its advertised
/// models (payload order, deduped) and the advisory hasKey annotation.
#[derive(Debug, Clone)]
pub struct PayloadProvider {
    pub spec: &'static ProviderSpec,
    pub models: Vec<String>,
    pub has_key: bool,
}

/// The validated routing table for one acp-host process.
#[derive(Debug, Clone, Default)]
pub struct ProviderRouter {
    /// Catalog-table provider order (the deterministic advertisement and
    /// startup tie-break order); models in payload order within each.
    providers: Vec<PayloadProvider>,
    /// TEST SEAM (C8 §9): the live-proof gate is a vendored-catalog flag
    /// the proof lane flips after the live compat/Anthropic proofs; tests
    /// override it in-memory to exercise the routing matrix's success arm.
    /// Production code never sets this.
    proven_overrides: std::collections::BTreeSet<String>,
}

impl ProviderRouter {
    /// TEST SEAM (C8 §9 routing matrix) — see `proven_overrides`.
    #[doc(hidden)]
    pub fn mark_proven_for_tests(&mut self, ids: &[&str]) {
        for id in ids {
            self.proven_overrides.insert((*id).to_string());
        }
    }

    fn is_proven(&self, spec: &ProviderSpec) -> bool {
        spec.proven || self.proven_overrides.contains(spec.id)
    }

    /// P5 §3: the live-proof gate by provider id (candidate construction
    /// gates proven-ness at selection time). Unknown ids are unproven.
    pub fn is_provider_proven(&self, provider: &str) -> bool {
        provider_by_id(provider).is_some_and(|spec| self.is_proven(spec))
    }

    /// Parse + validate the raw `WAYLAND_NANO_PROVIDERS` value. `None` (env
    /// unset) is a valid empty router — Flux-only. A malformed or oversize
    /// payload returns Err with a secret-free diagnostic reason and the
    /// caller ignores it WHOLESALE (Flux-only fallback, never a crash).
    pub fn from_payload(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        if raw.len() > MAX_PAYLOAD_BYTES {
            return Err(format!(
                "{KIND_PAYLOAD_INVALID}: WAYLAND_NANO_PROVIDERS exceeds {MAX_PAYLOAD_BYTES} bytes; ignoring it (Flux-only)"
            ));
        }
        let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
            format!("{KIND_PAYLOAD_INVALID}: WAYLAND_NANO_PROVIDERS is not valid json ({e}); ignoring it (Flux-only)")
        })?;
        let entries = value.as_array().ok_or_else(|| {
            format!("{KIND_PAYLOAD_INVALID}: WAYLAND_NANO_PROVIDERS must be a json array; ignoring it (Flux-only)")
        })?;
        if entries.len() > MAX_PAYLOAD_ENTRIES {
            return Err(format!(
                "{KIND_PAYLOAD_INVALID}: WAYLAND_NANO_PROVIDERS exceeds {MAX_PAYLOAD_ENTRIES} entries; ignoring it (Flux-only)"
            ));
        }
        // Merge per provider id (first occurrence of each model wins).
        let mut merged: std::collections::BTreeMap<String, (Vec<String>, bool)> =
            std::collections::BTreeMap::new();
        for entry in entries {
            let Some(obj) = entry.as_object() else {
                return Err(format!(
                    "{KIND_PAYLOAD_INVALID}: payload entries must be objects; ignoring it (Flux-only)"
                ));
            };
            // Entries carrying unknown fields — above all endpoint/routing
            // fields — are DROPPED (never merged): the payload may annotate,
            // never redirect.
            if obj
                .keys()
                .any(|k| !matches!(k.as_str(), "provider" | "models" | "hasKey"))
            {
                continue;
            }
            let (Some(provider), Some(models), Some(has_key)) = (
                obj.get("provider").and_then(|p| p.as_str()),
                obj.get("models").and_then(|m| m.as_array()),
                obj.get("hasKey").and_then(|h| h.as_bool()),
            ) else {
                return Err(format!(
                    "{KIND_PAYLOAD_INVALID}: payload entries require provider:string, models:string[], hasKey:bool; ignoring it (Flux-only)"
                ));
            };
            // Unknown provider ids are IGNORED (advisory payload).
            let Some(spec) = provider_by_id(provider) else {
                continue;
            };
            if models.len() > MAX_MODELS_PER_PROVIDER {
                return Err(format!(
                    "{KIND_PAYLOAD_INVALID}: provider {} exceeds {MAX_MODELS_PER_PROVIDER} models; ignoring it (Flux-only)",
                    spec.id
                ));
            }
            let slot = merged.entry(spec.id.to_string()).or_default();
            slot.1 = has_key;
            for model in models {
                let Some(model) = model.as_str() else {
                    return Err(format!(
                        "{KIND_PAYLOAD_INVALID}: model ids must be strings; ignoring it (Flux-only)"
                    ));
                };
                if model.chars().count() > MAX_MODEL_ID_CHARS {
                    return Err(format!(
                        "{KIND_PAYLOAD_INVALID}: a model id for {} exceeds {MAX_MODEL_ID_CHARS} chars; ignoring it (Flux-only)",
                        spec.id
                    ));
                }
                // A payload model id must be a bare id — never namespaced
                // (the host adds the namespace) and never empty.
                if model.is_empty() || model.contains(':') {
                    return Err(format!(
                        "{KIND_PAYLOAD_INVALID}: invalid model id for {}; ignoring it (Flux-only)",
                        spec.id
                    ));
                }
                if !slot.0.iter().any(|m| m == model) {
                    slot.0.push(model.to_string());
                }
            }
        }
        // Deterministic order: catalog-table order, then payload order.
        let mut providers = Vec::new();
        for spec in PROVIDERS {
            if let Some((models, has_key)) = merged.get(spec.id) {
                providers.push(PayloadProvider {
                    spec,
                    models: models.clone(),
                    has_key: *has_key,
                });
            }
        }
        Ok(Self {
            providers,
            proven_overrides: std::collections::BTreeSet::new(),
        })
    }

    /// Production entry: read + validate `WAYLAND_NANO_PROVIDERS`; a
    /// diagnostic reason is returned alongside the (possibly Flux-only)
    /// router for the caller to report on stderr — secret-free by
    /// construction.
    pub fn from_env() -> (Self, Option<String>) {
        match std::env::var("WAYLAND_NANO_PROVIDERS") {
            Ok(raw) if !raw.trim().is_empty() => match Self::from_payload(Some(&raw)) {
                Ok(router) => (router, None),
                Err(reason) => (Self::default(), Some(reason)),
            },
            _ => (Self::default(), None),
        }
    }

    /// Payload providers in deterministic (catalog-table) order.
    pub fn providers(&self) -> &[PayloadProvider] {
        &self.providers
    }

    /// The namespaced advertisement for payload providers: ids
    /// `<provider>:<model>` (Q2), display names human-friendly
    /// (`<model> (<display name>)`). Flux models are advertised bare by the
    /// caller and prepended separately.
    pub fn advertised_models(&self) -> Vec<AvailableModel> {
        let mut out = Vec::new();
        for provider in &self.providers {
            for model in &provider.models {
                out.push(AvailableModel {
                    id: format!("{}:{model}", provider.spec.id),
                    name: format!("{model} ({})", provider.spec.display_name),
                });
            }
        }
        out
    }

    /// The namespace parser (Q2): bare ids are Flux; exactly one colon
    /// splits provider/model; `a:b:c` and empty segments are REJECTED as
    /// the typed `model_not_found`.
    pub fn parse_model_id(model_id: &str) -> Result<ModelRef, ProviderError> {
        match model_id.split(':').collect::<Vec<_>>().as_slice() {
            [bare] => {
                if bare.is_empty() {
                    Err(ProviderError::model_not_found(model_id))
                } else {
                    Ok(ModelRef::Flux((*bare).to_string()))
                }
            }
            [provider, model] => {
                if provider.is_empty() || model.is_empty() {
                    Err(ProviderError::model_not_found(model_id))
                } else {
                    Ok(ModelRef::Namespaced {
                        provider: (*provider).to_string(),
                        model: (*model).to_string(),
                    })
                }
            }
            _ => Err(ProviderError::model_not_found(model_id)),
        }
    }

    /// Is this model id advertised for its provider? (Namespaced ids must
    /// come from the validated payload; unknown providers/models are not
    /// routable.)
    pub fn is_advertised(&self, provider: &str, model: &str) -> bool {
        self.providers
            .iter()
            .any(|p| p.spec.id == provider && p.models.iter().any(|m| m == model))
    }

    /// Resolve a `set_model` target into a dispatchable binding: parse the
    /// namespace → look up the catalog row → check advertisement → the
    /// live-proof gate (`provider_unproven`) → re-resolve the credential
    /// (env > file > injected bearer; expiry checked) — never the advisory
    /// `hasKey` (codex NB: a payload claiming `hasKey: false` with a
    /// resolvable credential SUCCEEDS; `hasKey: true` without one fails
    /// `provider_key_missing`).
    pub fn resolve_binding(
        &self,
        model_id: &str,
        get_env: &dyn Fn(&str) -> Option<String>,
        now_unix_secs: u64,
    ) -> Result<ProviderBinding, ProviderError> {
        match Self::parse_model_id(model_id)? {
            ModelRef::Flux(model) => {
                let spec = nano_model::provider_catalog::flux_router();
                match resolve_flux(get_env) {
                    Some(key) => {
                        nano_egress::redact::register_credential(&key);
                        Ok(ProviderBinding {
                            provider_id: spec.id.to_string(),
                            model,
                            wire: spec.wire,
                            base_url: spec.base_url.to_string(),
                            api_path: spec.api_path.to_string(),
                            credential: Credential::Key(key),
                            retry: None,
                        })
                    }
                    None => Err(ProviderError::provider_key_missing(spec)),
                }
            }
            ModelRef::Namespaced { provider, model } => {
                let Some(spec) = provider_by_id(&provider) else {
                    return Err(ProviderError::model_not_found(model_id));
                };
                if !self.is_advertised(&provider, &model) {
                    return Err(ProviderError::model_not_found(model_id));
                }
                // Q3: unproven arms fail with the typed error, never a
                // silent fallback onto another wire.
                if !self.is_proven(spec) {
                    return Err(ProviderError::provider_unproven(spec));
                }
                match resolve_credential(spec, get_env, now_unix_secs) {
                    CredentialResolution::Resolved(credential) => Ok(ProviderBinding {
                        provider_id: spec.id.to_string(),
                        model,
                        wire: spec.wire,
                        base_url: spec.base_url.to_string(),
                        api_path: spec.api_path.to_string(),
                        credential,
                        retry: None,
                    }),
                    CredentialResolution::ExpiredBearer => Err(ProviderError::oauth_expired(spec)),
                    CredentialResolution::Absent => Err(ProviderError::provider_key_missing(spec)),
                }
            }
        }
    }

    /// B2 startup resolution: the providers that are BOTH advertised (in
    /// the validated payload, with ≥1 model) AND hold a usable credential
    /// right now, in deterministic catalog-table order. Flux is handled by
    /// the caller (its advertisement/back-compat path predates the
    /// payload).
    pub fn credentialed_providers(
        &self,
        get_env: &dyn Fn(&str) -> Option<String>,
        now_unix_secs: u64,
    ) -> Vec<&PayloadProvider> {
        self.providers
            .iter()
            .filter(|p| !p.models.is_empty())
            .filter(|p| {
                matches!(
                    resolve_credential(p.spec, get_env, now_unix_secs),
                    CredentialResolution::Resolved(_)
                )
            })
            .collect()
    }

    /// P5 §1 step 4 / §3: the credential bootstrap PLUS the live-proof gate
    /// at SELECTION time — the corrected startup fallback and the Auto
    /// candidate constructor both exclude advertised, credentialed but
    /// unproven providers HERE, never after selection (today's
    /// `credentialed_providers` omits the gate and defers the unproven
    /// rejection to binding time; P5 fixes that rather than inheriting it).
    pub fn credentialed_proven_providers(
        &self,
        get_env: &dyn Fn(&str) -> Option<String>,
        now_unix_secs: u64,
    ) -> Vec<&PayloadProvider> {
        self.credentialed_providers(get_env, now_unix_secs)
            .into_iter()
            .filter(|p| self.is_proven(p.spec))
            .collect()
    }

    /// The deterministic initial binding when Flux has no credential: the
    /// first credentialed AND live-proven provider in catalog-table order,
    /// bound to its first advertised model in payload order (B2 + the P5
    /// §1 step-4 proven gate).
    pub fn initial_non_flux_model(
        &self,
        get_env: &dyn Fn(&str) -> Option<String>,
        now_unix_secs: u64,
    ) -> Option<String> {
        self.credentialed_proven_providers(get_env, now_unix_secs)
            .first()
            .map(|p| format!("{}:{}", p.spec.id, p.models[0]))
    }

    /// B2 initial model selection, whole rule in one place: Flux's
    /// `flux-auto` when its credential resolves (back-compat), else the
    /// deterministic non-Flux binding; None when NOTHING advertised
    /// resolves a credential (the caller exits 2 with
    /// [`ProviderRouter::no_credential_message`]).
    pub fn initial_model(
        &self,
        flux_key: Option<&str>,
        get_env: &dyn Fn(&str) -> Option<String>,
        now_unix_secs: u64,
    ) -> Option<String> {
        if flux_key.is_some() {
            return Some("flux-auto".to_string());
        }
        self.initial_non_flux_model(get_env, now_unix_secs)
    }

    /// The generalized no-credential startup message (B2): env-var NAMES
    /// only, never values. Returned when NO advertised provider — Flux
    /// included — resolves a credential.
    pub fn no_credential_message(&self) -> String {
        format!(
            "no usable provider credential: set FLUX_API_KEY (or FLUX_API_KEY_FILE), or have the host inject one of: {}",
            self.no_credential_env_names().join(", ")
        )
    }

    /// P5 §1 step 5: the env-var NAMES for the typed `no_credential` error
    /// (names only, never values).
    pub fn no_credential_env_names(&self) -> Vec<String> {
        let mut names = vec!["FLUX_API_KEY".to_string(), "FLUX_API_KEY_FILE".to_string()];
        for provider in &self.providers {
            names.push(provider.spec.env_var.to_string());
        }
        names
    }
}

/// The parsed `set_model` target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRef {
    /// Bare id — the Flux catalog (back-compat).
    Flux(String),
    /// `<provider>:<model>` (Q2).
    Namespaced { provider: String, model: String },
}

/// A dispatchable per-session binding: catalog endpoint fields + the bare
/// model id + the resolved credential (memory only, never journaled).
#[derive(Debug, Clone)]
pub struct ProviderBinding {
    pub provider_id: String,
    /// The bare model id (namespace stripped) sent on the wire.
    pub model: String,
    pub wire: WireKind,
    pub base_url: String,
    pub api_path: String,
    pub credential: Credential,
    /// P5 §4: Auto-ladder candidates dispatch with a single physical
    /// attempt (`RetryConfig::single_attempt`) so every attempt is visible
    /// to the ladder's global budget. `None` = the client's default retry
    /// posture (pins and implicit passthrough turns, unchanged).
    pub retry: Option<nano_model::retry::RetryConfig>,
}

impl ProviderBinding {
    /// P5: the ladder-candidate posture (single physical attempt).
    pub fn with_single_attempt(mut self) -> Self {
        self.retry = Some(nano_model::retry::RetryConfig::single_attempt());
        self
    }
}

impl ProviderBinding {
    /// Pre-dispatch freshness: a turn that would outlive its bearer fails
    /// with the typed, retryable `oauth_expired` instead of dispatching a
    /// doomed request (§7).
    pub fn check_fresh(&self, now_unix_secs: u64) -> Result<(), ProviderError> {
        if self.credential.is_stale(now_unix_secs) {
            let spec =
                provider_by_id(&self.provider_id).expect("bindings are built from catalog rows");
            return Err(ProviderError::oauth_expired(spec));
        }
        Ok(())
    }
}
