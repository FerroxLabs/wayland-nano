//! The OAuth login flow and the refresh discipline (P3 §6.2, §6.5).
//!
//! `login` is hook-driven: the operator approval of the DISCOVERED AS
//! origin, the browser handoff (never an auto-fetch), and the journal-first
//! grant record are injected by the owning lanes. Ordering is the security
//! property: discovery → issuer validation → operator approval → grant
//! journaled (BEFORE any endpoint grant exists in a live policy) → scoped
//! client built from the SAME endpoints → DCR/token exchange through it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use nano_egress::grant::HttpMethod;
use nano_egress::grant::normalize_endpoint;

use super::FailReason;
use super::OAuthError;
use super::OAuthTransport;
use super::bounded_error_code;
use super::discovery;
use super::loopback;
use super::pkce;
use super::storage::StoredTokens;
use super::storage::TokenStorage;

/// Proactive refresh skew (§6.5, codex's REFRESH_SKEW_MILLIS): a token
/// expiring within 30s is refreshed BEFORE the call carrying it.
pub const REFRESH_SKEW_SECS: u64 = 30;

/// One journaled endpoint: canonical (method, path) — normalized through
/// `normalize_endpoint` BEFORE the record is handed to the journal hook
/// (§6.3 step 3). No wildcards, ever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantEndpoint {
    pub method: HttpMethod,
    /// The canonical serialized path (see nano-egress grant rules).
    pub path: String,
}

/// The reference handle the journal persists (`Op::McpOauthGrant`, owned by
/// the journal lane). Carries ids, the origin, the issuer, and exact
/// endpoint pairs — NEVER token material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantRecord {
    /// Idempotence key for the journal op.
    pub grant_id: String,
    /// The stable server instance id.
    pub server_id: String,
    /// https://host[:port] — origin only (≤ 256 chars, validated).
    pub as_origin: String,
    /// The validated issuer string (≤ 512 chars).
    pub issuer: String,
    /// ≤ 4 exact {method, path} pairs from the VALIDATED AS metadata.
    pub endpoints: Vec<GrantEndpoint>,
}

/// What a completed login established (for the caller's bookkeeping; the
/// durable side already landed through the storage + journal hooks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOutcome {
    pub as_origin: String,
    pub issuer: String,
    pub endpoints: Vec<GrantEndpoint>,
    /// True when RFC 7591 dynamic client registration was used.
    pub dcr_used: bool,
}

/// The injected operator surfaces (§6.2 steps 2/4, §6.3 step 3).
pub struct LoginHooks<'a> {
    /// Operator approval of the DISCOVERED AS origin — displayed and
    /// explicitly confirmed BEFORE the browser is launched. Decline ⇒
    /// typed `McpOAuthFailed(operator_declined)`, nothing journaled, no
    /// listener.
    pub approve_as_origin: &'a dyn Fn(&str) -> bool,
    /// The browser handoff for the authorize URL (ACP
    /// `session/open_url`-style request or a printed URL) — this module
    /// NEVER fetches the authorize URL itself.
    pub open_authorize_url: &'a dyn Fn(&str),
    /// Journal-first grant seam: the journal lane appends
    /// `Op::McpOauthGrant` here. A failure aborts the login BEFORE the
    /// scoped client is built or used (append failure ⇒ policy untouched).
    pub record_grant: &'a dyn Fn(&GrantRecord) -> Result<(), OAuthError>,
    /// Clock (injected for tests).
    pub now_unix: &'a dyn Fn() -> u64,
}

/// The one-shot bootstrap seam (§6.3 step 2): for each candidate issuer the
/// factory builds a client bound to EXACTLY ONE endpoint grant (GET on the
/// RFC 8414 metadata URL), redirects disabled, consumed by one fetch.
/// Production: [`discovery::BootstrapClient`]; tests: scripted.
pub type BootstrapFactory<'a> = &'a dyn Fn(&str) -> Result<Box<dyn OAuthTransport>, OAuthError>;

/// The scoped-client seam (§6.3 step 4): builds the post-login client whose
/// policy is empty base + the journaled endpoint grants (redirects
/// disabled). Production: [`discovery::scoped_client`] + `EgressTransport`.
pub type ScopedFactory<'a> =
    &'a dyn Fn(&str, &[GrantEndpoint]) -> Result<Box<dyn OAuthTransport>, OAuthError>;

/// Everything one login attempt needs (bundled to keep the signature
/// readable). All hooks and factories are injected by the owning lanes.
pub struct LoginRequest<'a> {
    /// The stable server instance id (credential/journal/egress key).
    pub server_id: &'a str,
    /// The configured MCP server URL (https).
    pub server_url: &'a str,
    /// Static client id override; absent ⇒ RFC 7591 DCR when advertised.
    pub static_client_id: Option<&'a str>,
    /// The session egress transport (9728 fetch only).
    pub session_transport: &'a dyn OAuthTransport,
    /// One-shot bootstrap factory (per candidate issuer).
    pub bootstrap_factory: BootstrapFactory<'a>,
    /// Scoped-client factory (post-approval, post-journal).
    pub scoped_factory: ScopedFactory<'a>,
    /// Token storage.
    pub storage: &'a dyn TokenStorage,
    /// The operator surfaces.
    pub hooks: LoginHooks<'a>,
}

/// The full authorization-code + PKCE login (§6.2), trust-chained (§6.3).
///
/// `session_transport` is the session egress client (the MCP origin is
/// already host-allowlisted, §6.1) used for the same-origin RFC 9728 fetch
/// ONLY; all AS traffic goes through the bootstrap/scoped factories.
pub async fn login(request: LoginRequest<'_>) -> Result<LoginOutcome, OAuthError> {
    let LoginRequest {
        server_id,
        server_url,
        static_client_id,
        session_transport,
        bootstrap_factory,
        scoped_factory,
        storage,
        hooks,
    } = request;
    let hooks = &hooks;
    // Step 1: same-origin RFC 9728 resource metadata (derived from the
    // configured origin, never from a server response). No metadata ⇒
    // typed failure, no heuristic fallback.
    let resource_url = discovery::protected_resource_metadata_url(server_url)?;
    let (status, body) = session_transport.get_bounded(&resource_url).await?;
    if !(200..300).contains(&status) {
        return Err(OAuthError::Failed {
            reason: FailReason::NoProtectedResourceMetadata,
        });
    }
    let resource = discovery::parse_resource_metadata(&body)?;

    // Step 2: one-shot bootstrap fetch per candidate issuer; first
    // candidate whose metadata validates (issuer EXACT match, same-origin
    // endpoints, S256 proven) wins. Every candidate failure is typed; the
    // last one surfaces.
    let mut validated = None;
    let mut last_err = OAuthError::Failed {
        reason: FailReason::MetadataInvalid,
    };
    for candidate in &resource.authorization_servers {
        let expected_url = match discovery::as_metadata_url(candidate) {
            Ok(u) => u,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let bootstrap = match bootstrap_factory(candidate) {
            Ok(b) => b,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        let (status, body) = match bootstrap.get_bounded(&expected_url).await {
            Ok(v) => v,
            Err(e) => {
                last_err = e;
                continue;
            }
        };
        // Redirects are DISABLED on the bootstrap client; a 3xx here is a
        // typed failure, never a followed hop.
        if (300..400).contains(&status) {
            last_err = OAuthError::Failed {
                reason: FailReason::RedirectRejected,
            };
            continue;
        }
        if !(200..300).contains(&status) {
            last_err = OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            };
            continue;
        }
        match discovery::validate_as_metadata(candidate, &expected_url, &body) {
            Ok(v) => {
                validated = Some(v);
                break;
            }
            Err(e) => last_err = e,
        }
    }
    let Some(validated) = validated else {
        return Err(last_err);
    };

    // §6.2 step 2: explicit operator approval of the DISCOVERED AS origin
    // BEFORE the browser is launched. Decline ⇒ typed, nothing journaled,
    // no listener.
    if !(hooks.approve_as_origin)(&validated.as_origin) {
        return Err(OAuthError::Failed {
            reason: FailReason::OperatorDeclined,
        });
    }

    // §6.3 step 3: the grant record, journal-first.
    let dcr_used = static_client_id.is_none() && validated.registration_endpoint.is_some();
    let grant_urls = discovery::endpoint_grants(&validated, dcr_used)?;
    let mut endpoints = Vec::with_capacity(grant_urls.len());
    for (method, url) in &grant_urls {
        let normalized = normalize_endpoint(*method, url).map_err(|_| OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        })?;
        endpoints.push(GrantEndpoint {
            method: *method,
            path: normalized.path,
        });
    }
    let record = GrantRecord {
        grant_id: pkce::random_token_128(),
        server_id: server_id.to_string(),
        as_origin: validated.as_origin.clone(),
        issuer: validated.issuer.clone(),
        endpoints: endpoints.clone(),
    };
    // Journal-first: the op lands BEFORE any endpoint grant exists in a
    // live policy; append failure ⇒ abort, policy untouched.
    (hooks.record_grant)(&record)?;

    // §6.3 step 4: the scoped client exists only now, from the SAME
    // endpoints that were journaled.
    let scoped = scoped_factory(&validated.as_origin, &endpoints)?;

    // §6.2 step 3: PKCE, S256 only (S256 support was proven in validation).
    let pkce = pkce::PkceChallenge::generate();

    // §6.2 step 4: the fully bound loopback listener.
    let binding = loopback::bind(server_id)?;
    let redirect_uri = binding.redirect_uri();

    let outcome = async {
        // §6.2 step 7: RFC 7591 dynamic client registration when the AS
        // supports it and no static client id is configured.
        let client_id = match (static_client_id, dcr_used) {
            (Some(id), _) => id.to_string(),
            (None, true) => register_client(&validated, &redirect_uri, scoped.as_ref()).await?,
            (None, false) => {
                return Err(OAuthError::Failed {
                    reason: FailReason::MetadataInvalid,
                });
            }
        };

        // The authorize URL targets ONLY the operator-approved AS origin;
        // the browser handoff is the caller's, never an auto-fetch.
        let authorize_url = build_authorize_url(&validated, &client_id, &binding, &pkce);
        (hooks.open_authorize_url)(&authorize_url);

        // §6.2 step 5: the callback. State/Host/method/path/duplicate/size
        // are enforced inside the listener; the verifier is memory-only.
        let code = tokio::task::spawn_blocking(move || binding.await_callback())
            .await
            .map_err(|e| OAuthError::Transport {
                detail: format!("callback join: {e}"),
            })??;

        // §6.2 step 6: token exchange POST through the §6.3 scoped client.
        let tokens = exchange_code(
            &validated,
            scoped.as_ref(),
            &client_id,
            &code,
            &redirect_uri,
            &pkce,
            hooks.now_unix,
        )
        .await?;
        tokens.register_with_sanitizer();
        storage.store(server_id, &tokens)?;
        Ok(())
    }
    .await;
    outcome?;

    Ok(LoginOutcome {
        as_origin: record.as_origin,
        issuer: record.issuer,
        endpoints: record.endpoints,
        dcr_used,
    })
}

/// RFC 7591 registration POST (JSON) through the scoped client.
async fn register_client(
    validated: &discovery::ValidatedAs,
    redirect_uri: &str,
    scoped: &dyn OAuthTransport,
) -> Result<String, OAuthError> {
    let endpoint = validated
        .registration_endpoint
        .as_ref()
        .ok_or(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        })?;
    let body = serde_json::json!({
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "client_name": "wayland-nano",
    });
    let (status, response) = scoped.post_json(endpoint, &body).await?;
    if !(200..300).contains(&status) {
        return Err(OAuthError::Failed {
            reason: FailReason::RegistrationFailed(provider_error_code(&response)),
        });
    }
    let value: serde_json::Value =
        serde_json::from_slice(&response).map_err(|_| OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        })?;
    value
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or(OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        })
}

fn build_authorize_url(
    validated: &discovery::ValidatedAs,
    client_id: &str,
    binding: &loopback::LoopbackBinding,
    pkce: &pkce::PkceChallenge,
) -> String {
    let mut url = reqwest::Url::parse(&validated.authorization_endpoint)
        .unwrap_or_else(|_| reqwest::Url::parse("https://invalid.invalid/").expect("static"));
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", &binding.redirect_uri())
        .append_pair("state", &binding.state)
        .append_pair("code_challenge", pkce.challenge())
        .append_pair("code_challenge_method", "S256");
    url.to_string()
}

/// Token exchange POST (§6.2 step 6) through the scoped client, riding the
/// already-journaled grant set.
async fn exchange_code(
    validated: &discovery::ValidatedAs,
    scoped: &dyn OAuthTransport,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    pkce: &pkce::PkceChallenge,
    now_unix: &dyn Fn() -> u64,
) -> Result<StoredTokens, OAuthError> {
    let form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("client_id".to_string(), client_id.to_string()),
        ("code_verifier".to_string(), pkce.verifier().to_string()),
    ];
    let (status, body) = scoped.post_form(&validated.token_endpoint, &form).await?;
    if (300..400).contains(&status) {
        // Redirects are disabled on the scoped client; a 3xx from the
        // token endpoint is a typed failure, never a followed hop.
        return Err(OAuthError::Failed {
            reason: FailReason::RedirectRejected,
        });
    }
    if !(200..300).contains(&status) {
        return Err(OAuthError::Failed {
            reason: FailReason::ProviderError(provider_error_code(&body)),
        });
    }
    parse_token_response(&body, now_unix())
}

/// Parse a token endpoint response (RFC 6749 §5.1): `access_token`
/// required, `token_type` must be Bearer, `refresh_token`/`expires_in`
/// optional. Bounded members only.
fn parse_token_response(body: &[u8], now_unix: u64) -> Result<StoredTokens, OAuthError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| OAuthError::Failed {
            reason: FailReason::MetadataInvalid,
        })?;
    let invalid = || OAuthError::Failed {
        reason: FailReason::MetadataInvalid,
    };
    let access_token = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .ok_or_else(invalid)?
        .to_string();
    let token_type = value
        .get("token_type")
        .and_then(|v| v.as_str())
        .ok_or_else(invalid)?;
    if !token_type.eq_ignore_ascii_case("bearer") {
        return Err(OAuthError::Failed {
            reason: FailReason::ProviderError("non_bearer_token_type".to_string()),
        });
    }
    let refresh_token = value
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && s.len() <= 8192)
        .map(str::to_string);
    let expires_at_unix = value
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .map(|secs| now_unix.saturating_add(secs));
    Ok(StoredTokens {
        access_token: Some(access_token),
        refresh_token,
        expires_at_unix,
    })
}

/// The provider's `error` CODE, bounded and sanitized (never prose).
fn provider_error_code(body: &[u8]) -> String {
    let code = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_default();
    bounded_error_code(&code)
}

// --- Refresh-on-401 discipline (§6.5) --------------------------------------

/// The proactive check (§6.5): 30s-skew — a token expiring within 30s, or
/// a credential set with NO access token (file-path load carries the
/// refresh token only), is refreshed BEFORE the call carrying it.
pub fn token_needs_refresh(tokens: &StoredTokens, now_unix: u64) -> bool {
    if tokens.refresh_token.is_none() {
        return false; // nothing to refresh WITH — the caller types AuthorizationRequired
    }
    match (&tokens.access_token, tokens.expires_at_unix) {
        (None, _) => true,
        (Some(_), Some(exp)) => now_unix.saturating_add(REFRESH_SKEW_SECS) >= exp,
        (Some(_), None) => false, // no expiry advertised: usable until a 401 says otherwise
    }
}

/// Which MCP calls may be retried after a reactive 401 + one refresh
/// (§6.5, r2 codex-F14 — no blind double-execution): handshake/read-only
/// operations, or calls carrying a server-supported idempotency key.
/// Everything else — all default `tools/call`s — gets typed
/// `McpAuthorizationRequired` and requires an EXPLICIT user retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    ReadOnly,
    IdempotencyKeyed,
    NonIdempotent,
}

/// Classify an MCP call for the reactive-401 rule.
pub fn retry_class(rpc_method: &str, has_idempotency_key: bool) -> RetryClass {
    if matches!(
        rpc_method,
        "initialize" | "tools/list" | "resources/list" | "resources/read"
    ) {
        RetryClass::ReadOnly
    } else if has_idempotency_key {
        RetryClass::IdempotencyKeyed
    } else {
        RetryClass::NonIdempotent
    }
}

/// The reactive-401 outcome: after ONE refresh, read-only /
/// idempotency-keyed calls retry once; every other call surfaces
/// `McpAuthorizationRequired` for an explicit user retry (the tool result
/// tells the model the call was NOT executed twice — the dispatcher lane
/// owns that wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReactiveOutcome {
    Retry,
    AuthorizationRequired,
}

/// Per-server refresh serialization (§6.5: concurrent refreshes serialize
/// on a per-server mutex — grok's dedup lesson) and the refresh execution
/// itself. Holds no tokens; storage is the source of truth.
#[derive(Default)]
pub struct RefreshCoordinator {
    per_server: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RefreshCoordinator {
    fn lock_for(&self, server_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.per_server.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(server_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Refresh `server_id`'s tokens if needed (proactive) — serialized
    /// per-server; a concurrent refresh that already landed is observed by
    /// the re-load under the lock and not repeated. The refresh POST rides
    /// the scoped client (§6.3) and is EXCLUDED from the MCP handshake
    /// timeout budget (the caller's budget starts after this returns).
    pub async fn refresh_if_needed(
        &self,
        server_id: &str,
        storage: &dyn TokenStorage,
        scoped_factory: ScopedFactory<'_>,
        record: &GrantRecord,
        now_unix: &dyn Fn() -> u64,
    ) -> Result<StoredTokens, OAuthError> {
        self.refresh_impl(server_id, storage, scoped_factory, record, now_unix, false)
            .await
    }

    /// Reactive 401 (§6.5): ONE refresh (forced — the server just rejected
    /// the presented token), then retry ONLY handshake/read-only or
    /// idempotency-keyed calls. Non-idempotent calls get
    /// `AuthorizationRequired` — the caller surfaces the explicit-retry
    /// tool result.
    pub async fn on_unauthorized(
        &self,
        server_id: &str,
        class: RetryClass,
        storage: &dyn TokenStorage,
        scoped_factory: ScopedFactory<'_>,
        record: &GrantRecord,
        now_unix: &dyn Fn() -> u64,
    ) -> Result<ReactiveOutcome, OAuthError> {
        self.refresh_impl(server_id, storage, scoped_factory, record, now_unix, true)
            .await?;
        Ok(match class {
            RetryClass::ReadOnly | RetryClass::IdempotencyKeyed => ReactiveOutcome::Retry,
            RetryClass::NonIdempotent => ReactiveOutcome::AuthorizationRequired,
        })
    }

    async fn refresh_impl(
        &self,
        server_id: &str,
        storage: &dyn TokenStorage,
        scoped_factory: ScopedFactory<'_>,
        record: &GrantRecord,
        now_unix: &dyn Fn() -> u64,
        force: bool,
    ) -> Result<StoredTokens, OAuthError> {
        let lock = self.lock_for(server_id);
        let _guard = lock.lock().await;
        // Re-load UNDER the lock: a concurrent refresh may have landed.
        let tokens = storage
            .load(server_id)?
            .ok_or_else(|| OAuthError::AuthorizationRequired {
                server_id: server_id.to_string(),
            })?;
        if !force && !token_needs_refresh(&tokens, now_unix()) {
            return Ok(tokens);
        }
        let refresh_token =
            tokens
                .refresh_token
                .clone()
                .ok_or_else(|| OAuthError::AuthorizationRequired {
                    server_id: server_id.to_string(),
                })?;
        let scoped = scoped_factory(&record.as_origin, &record.endpoints)?;
        // The client_id used at exchange time is not persisted (reference
        // handles only, §6.4); refresh omits it — RFC 6749 §6 permits this
        // for public clients, and DCR'd registrations re-identify by the
        // refresh token itself.
        let form = vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            ("refresh_token".to_string(), refresh_token),
        ];
        // The token endpoint is the FIRST POST grant by construction
        // (`endpoint_grants` order: token, metadata GET, registration).
        let token_url = record
            .endpoints
            .iter()
            .find(|e| e.method == HttpMethod::Post)
            .map(|e| format!("{}{}", record.as_origin, e.path))
            .ok_or(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            })?;
        let (status, body) = scoped.post_form(&token_url, &form).await?;
        if (300..400).contains(&status) {
            return Err(OAuthError::Failed {
                reason: FailReason::RedirectRejected,
            });
        }
        if status == 400 || status == 401 {
            // invalid_grant & friends: the refresh token is dead — this is
            // an authorization problem, not a transient one.
            return Err(OAuthError::AuthorizationRequired {
                server_id: server_id.to_string(),
            });
        }
        if !(200..300).contains(&status) {
            return Err(OAuthError::Failed {
                reason: FailReason::ProviderError(provider_error_code(&body)),
            });
        }
        let mut fresh = parse_token_response(&body, now_unix())?;
        // RFC 6749 §6: the AS MAY omit a new refresh token; keep the old one.
        if fresh.refresh_token.is_none() {
            fresh.refresh_token = tokens.refresh_token.clone();
        }
        fresh.register_with_sanitizer();
        // The §6.4 write path (keyring set / atomic file rewrite), never a
        // side channel.
        storage.store_after_refresh(server_id, &fresh)?;
        Ok(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpStream;
    use std::sync::Arc;

    /// Scripted OAuthTransport: URL-keyed responses; every call recorded;
    /// form bodies logged for the PKCE assertion.
    #[derive(Default)]
    struct Script {
        gets: Mutex<Vec<(String, u16, Vec<u8>)>>,
        forms: Mutex<Vec<(String, u16, Vec<u8>)>>,
        jsons: Mutex<Vec<(String, u16, Vec<u8>)>>,
        calls: Mutex<Vec<String>>,
        form_log: Mutex<Vec<Vec<(String, String)>>>,
    }

    impl Script {
        fn with_get(self, url: &str, status: u16, body: &str) -> Self {
            self.gets
                .lock()
                .unwrap()
                .push((url.to_string(), status, body.as_bytes().to_vec()));
            self
        }
        fn with_form(self, url: &str, status: u16, body: &str) -> Self {
            self.forms
                .lock()
                .unwrap()
                .push((url.to_string(), status, body.as_bytes().to_vec()));
            self
        }
        fn with_json(self, url: &str, status: u16, body: &str) -> Self {
            self.jsons
                .lock()
                .unwrap()
                .push((url.to_string(), status, body.as_bytes().to_vec()));
            self
        }
        fn lookup(map: &Mutex<Vec<(String, u16, Vec<u8>)>>, url: &str) -> (u16, Vec<u8>) {
            map.lock()
                .unwrap()
                .iter()
                .find(|(u, _, _)| u == url)
                .map(|(_, s, b)| (*s, b.clone()))
                .unwrap_or((500, b"unscripted url".to_vec()))
        }
        fn call_log(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl OAuthTransport for Arc<Script> {
        async fn get_bounded(&self, url: &str) -> Result<(u16, Vec<u8>), OAuthError> {
            self.calls.lock().unwrap().push(format!("GET {url}"));
            Ok(Script::lookup(&self.gets, url))
        }
        async fn post_form(
            &self,
            url: &str,
            form: &[(String, String)],
        ) -> Result<(u16, Vec<u8>), OAuthError> {
            self.calls.lock().unwrap().push(format!("FORM {url}"));
            self.form_log.lock().unwrap().push(form.to_vec());
            Ok(Script::lookup(&self.forms, url))
        }
        async fn post_json(
            &self,
            url: &str,
            _body: &serde_json::Value,
        ) -> Result<(u16, Vec<u8>), OAuthError> {
            self.calls.lock().unwrap().push(format!("JSON {url}"));
            Ok(Script::lookup(&self.jsons, url))
        }
    }

    const ISSUER: &str = "https://as.example/tenant1";
    const AS_META_URL: &str = "https://as.example/.well-known/oauth-authorization-server/tenant1";
    const ACCESS: &str = "access-token-CANARY-7f3a9c51d2b8";
    const REFRESH: &str = "refresh-token-CANARY-9b2e4d68a1c3";

    /// In-memory TokenStorage for the flow tests (OS credential facilities
    /// are exercised separately, in storage.rs / wincred.rs tests).
    #[derive(Default)]
    struct MemStore {
        inner: Mutex<std::collections::HashMap<String, StoredTokens>>,
    }

    impl TokenStorage for MemStore {
        fn load(&self, server_id: &str) -> Result<Option<StoredTokens>, OAuthError> {
            Ok(self.inner.lock().unwrap().get(server_id).cloned())
        }
        fn store(&self, server_id: &str, tokens: &StoredTokens) -> Result<(), OAuthError> {
            tokens.register_with_sanitizer();
            self.inner
                .lock()
                .unwrap()
                .insert(server_id.to_string(), tokens.clone());
            Ok(())
        }
        fn store_after_refresh(
            &self,
            server_id: &str,
            tokens: &StoredTokens,
        ) -> Result<(), OAuthError> {
            self.store(server_id, tokens)
        }
        fn delete(&self, server_id: &str) -> Result<(), OAuthError> {
            self.inner.lock().unwrap().remove(server_id);
            Ok(())
        }
    }

    fn as_metadata_json() -> String {
        format!(
            r#"{{"issuer":"{ISSUER}",
               "authorization_endpoint":"{ISSUER}/authorize",
               "token_endpoint":"{ISSUER}/token",
               "registration_endpoint":"{ISSUER}/register",
               "code_challenge_methods_supported":["S256"]}}"#
        )
    }

    /// The §12 loopback mock IdP rig: scripted 9728/8414/DCR/token
    /// endpoints, REAL loopback listener, recording hooks.
    struct Rig {
        session: Arc<Script>,
        bootstrap: Arc<Script>,
        scoped: Arc<Script>,
        grants: Arc<Mutex<Vec<GrantRecord>>>,
        opened: Arc<Mutex<Vec<String>>>,
        approved: Arc<Mutex<Vec<String>>>,
        scoped_builds: Arc<Mutex<usize>>,
        pkce_challenge: Arc<Mutex<Option<String>>>,
        approve: bool,
        tamper_state: bool,
        fail_record_grant: bool,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                session: Arc::new(Script::default().with_get(
                    "https://mcp.example/.well-known/oauth-protected-resource",
                    200,
                    r#"{"authorization_servers":["https://as.example/tenant1"]}"#,
                )),
                bootstrap: Arc::new(Script::default().with_get(
                    AS_META_URL,
                    200,
                    &as_metadata_json(),
                )),
                scoped: Arc::new(
                    Script::default()
                        .with_json(
                            "https://as.example/tenant1/register",
                            201,
                            r#"{"client_id":"dcr-client-1"}"#,
                        )
                        .with_form(
                            "https://as.example/tenant1/token",
                            200,
                            &format!(
                                r#"{{"access_token":"{ACCESS}",
                                    "refresh_token":"{REFRESH}",
                                    "token_type":"Bearer","expires_in":3600}}"#
                            ),
                        ),
                ),
                grants: Arc::new(Mutex::new(Vec::new())),
                opened: Arc::new(Mutex::new(Vec::new())),
                approved: Arc::new(Mutex::new(Vec::new())),
                scoped_builds: Arc::new(Mutex::new(0)),
                pkce_challenge: Arc::new(Mutex::new(None)),
                approve: true,
                tamper_state: false,
                fail_record_grant: false,
            }
        }

        fn storage(&self) -> MemStore {
            MemStore::default()
        }

        async fn run(&self, server_id: &str) -> Result<LoginOutcome, OAuthError> {
            let session_view = Arc::clone(&self.session);
            let bootstrap = Arc::clone(&self.bootstrap);
            let scoped = Arc::clone(&self.scoped);
            let scoped_builds = Arc::clone(&self.scoped_builds);
            let grants = Arc::clone(&self.grants);
            let opened = Arc::clone(&self.opened);
            let approved = Arc::clone(&self.approved);
            let challenge_slot = Arc::clone(&self.pkce_challenge);
            let approve = self.approve;
            let tamper = self.tamper_state;
            let fail_grant = self.fail_record_grant;
            let storage = self.storage();
            let now = || 1_800_000_000u64;
            login(LoginRequest {
                server_id,
                server_url: "https://mcp.example/mcp/",
                static_client_id: None,
                session_transport: &session_view,
                bootstrap_factory: &move |_candidate| {
                    Ok(Box::new(Arc::clone(&bootstrap)) as Box<dyn OAuthTransport>)
                },
                scoped_factory: &move |_origin, _endpoints| {
                    *scoped_builds.lock().unwrap() += 1;
                    Ok(Box::new(Arc::clone(&scoped)) as Box<dyn OAuthTransport>)
                },
                storage: &storage,
                hooks: LoginHooks {
                    approve_as_origin: &move |origin| {
                        approved.lock().unwrap().push(origin.to_string());
                        approve
                    },
                    open_authorize_url: &move |url| {
                        opened.lock().unwrap().push(url.to_string());
                        let parsed = reqwest::Url::parse(url).expect("authorize url");
                        let param = |name: &str| {
                            parsed
                                .query_pairs()
                                .find(|(k, _)| k == name)
                                .map(|(_, v)| v.into_owned())
                        };
                        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
                        *challenge_slot.lock().unwrap() = param("code_challenge");
                        let state = if tamper {
                            "tampered-state".to_string()
                        } else {
                            param("state").expect("state")
                        };
                        let redirect = param("redirect_uri").expect("redirect_uri");
                        // The simulated AS/browser: issue the loopback GET.
                        std::thread::spawn(move || {
                            let target = format!("{redirect}?code=the-auth-code&state={state}");
                            let parsed = reqwest::Url::parse(&target).expect("callback url");
                            let port = parsed.port().expect("port");
                            let path_query =
                                format!("{}?{}", parsed.path(), parsed.query().unwrap_or(""));
                            let mut stream =
                                TcpStream::connect(("127.0.0.1", port)).expect("connect");
                            let req = format!(
                                "GET {path_query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                            );
                            stream.write_all(req.as_bytes()).unwrap();
                            let mut buf = Vec::new();
                            let _ = stream.read_to_end(&mut buf);
                        });
                    },
                    record_grant: &move |record| {
                        if fail_grant {
                            return Err(OAuthError::Failed {
                                reason: FailReason::MetadataInvalid,
                            });
                        }
                        grants.lock().unwrap().push(record.clone());
                        Ok(())
                    },
                    now_unix: &now,
                },
            })
            .await
        }
    }

    /// §12: full PKCE round-trip through the mock IdP — discovery →
    /// operator approval → journal-first grant → DCR → browser handoff →
    /// bound loopback callback → token exchange → storage. The token
    /// endpoint's logged form proves S256: the verifier hashes to the
    /// challenge the authorize URL carried.
    #[tokio::test]
    async fn full_pkce_login_roundtrip() {
        let rig = Rig::new();
        let outcome = rig.run("srvtest-full").await.expect("login");
        assert_eq!(outcome.as_origin, "https://as.example");
        assert_eq!(outcome.issuer, ISSUER);
        assert!(outcome.dcr_used);
        // The journaled grant: exact endpoints, canonical paths.
        let grants = rig.grants.lock().unwrap();
        assert_eq!(grants.len(), 1, "exactly one grant record");
        let record = &grants[0];
        assert_eq!(record.server_id, "srvtest-full");
        assert_eq!(
            record.endpoints,
            vec![
                GrantEndpoint {
                    method: HttpMethod::Post,
                    path: "/tenant1/token".into(),
                },
                GrantEndpoint {
                    method: HttpMethod::Get,
                    path: "/.well-known/oauth-authorization-server/tenant1".into(),
                },
                GrantEndpoint {
                    method: HttpMethod::Post,
                    path: "/tenant1/register".into(),
                },
            ]
        );
        drop(grants);
        // Operator approval saw the DISCOVERED origin before the browser.
        assert_eq!(
            rig.approved.lock().unwrap().as_slice(),
            &["https://as.example".to_string()]
        );
        assert_eq!(rig.opened.lock().unwrap().len(), 1, "browser handoff once");
        // The scoped client was built exactly once (ordering proven by the
        // append-failure test below).
        assert_eq!(*rig.scoped_builds.lock().unwrap(), 1);
        // PKCE: the verifier in the exchange form S256-matches the
        // challenge the authorize URL carried.
        let form_log = rig.scoped.form_log.lock().unwrap();
        let exchange = form_log
            .iter()
            .find(|f| f.iter().any(|(k, _)| k == "code_verifier"))
            .expect("token exchange form");
        let verifier = exchange
            .iter()
            .find(|(k, _)| k == "code_verifier")
            .map(|(_, v)| v.clone())
            .unwrap();
        use base64::Engine;
        use sha2::Digest;
        let recomputed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(&verifier));
        assert_eq!(
            Some(recomputed),
            rig.pkce_challenge.lock().unwrap().clone(),
            "S256: SHA256(verifier) must equal the advertised challenge"
        );
        assert_eq!(
            exchange
                .iter()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.as_str()),
            Some("the-auth-code")
        );
        // Canary: both token values are registered with the sanitizer.
        let scrubbed = nano_egress::redact::sanitize_text(&format!("{ACCESS} {REFRESH}"));
        assert!(
            !scrubbed.contains("CANARY"),
            "tokens must scrub: {scrubbed}"
        );
        // Bootstrap discipline: exactly the RFC 8414 metadata URL was hit.
        assert_eq!(
            rig.bootstrap.call_log(),
            vec![format!("GET {AS_META_URL}")],
            "the bootstrap fetch hits exactly the RFC 8414 URL"
        );
    }

    /// §12 operator approval: decline ⇒ typed OperatorDeclined, no browser,
    /// no grant, no listener, nothing stored.
    #[tokio::test]
    async fn operator_decline_stops_everything() {
        let rig = Rig {
            approve: false,
            ..Rig::new()
        };
        let err = rig.run("srvtest-decline").await.expect_err("declined");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::OperatorDeclined
            }
        ));
        assert!(rig.opened.lock().unwrap().is_empty(), "no browser launch");
        assert!(rig.grants.lock().unwrap().is_empty(), "no grant journaled");
        assert_eq!(
            *rig.scoped_builds.lock().unwrap(),
            0,
            "no scoped client built"
        );
        assert!(rig.scoped.call_log().is_empty(), "no AS traffic");
    }

    /// §12 journal-first: append failure ⇒ the scoped client is never built
    /// and no OAuth endpoint traffic happens (policy untouched).
    #[tokio::test]
    async fn grant_append_failure_aborts_before_any_scoped_request() {
        let rig = Rig {
            fail_record_grant: true,
            ..Rig::new()
        };
        let err = rig.run("srvtest-jfail").await.expect_err("must abort");
        assert!(matches!(err, OAuthError::Failed { .. }));
        assert_eq!(*rig.scoped_builds.lock().unwrap(), 0);
        assert!(rig.scoped.call_log().is_empty());
        assert!(rig.opened.lock().unwrap().is_empty(), "no browser launch");
    }

    /// §12: no 9728 metadata ⇒ typed, zero stored, bootstrap never built.
    #[tokio::test]
    async fn missing_resource_metadata_is_typed() {
        let rig = Rig {
            session: Arc::new(Script::default()),
            ..Rig::new()
        };
        let err = rig.run("srvtest-no9728").await.expect_err("typed");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::NoProtectedResourceMetadata
            }
        ));
        assert!(rig.bootstrap.call_log().is_empty());
        assert!(rig.grants.lock().unwrap().is_empty());
    }

    /// §12: a metadata REDIRECT is typed, never followed (redirects are
    /// disabled on the bootstrap client).
    #[tokio::test]
    async fn metadata_redirect_is_typed_never_followed() {
        let rig = Rig {
            bootstrap: Arc::new(Script::default().with_get(AS_META_URL, 302, "")),
            ..Rig::new()
        };
        let err = rig.run("srvtest-redir").await.expect_err("typed");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::RedirectRejected
            }
        ));
        assert_eq!(rig.bootstrap.call_log().len(), 1, "one fetch, no follow");
        assert!(rig.scoped.call_log().is_empty());
    }

    /// §12: S256-only — an AS offering only `plain` fails typed.
    #[tokio::test]
    async fn plain_only_challenge_method_is_refused() {
        let plain = format!(
            r#"{{"issuer":"{ISSUER}",
               "authorization_endpoint":"{ISSUER}/authorize",
               "token_endpoint":"{ISSUER}/token",
               "code_challenge_methods_supported":["plain"]}}"#
        );
        let rig = Rig {
            bootstrap: Arc::new(Script::default().with_get(AS_META_URL, 200, &plain)),
            ..Rig::new()
        };
        let err = rig.run("srvtest-plain").await.expect_err("typed");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::S256Unsupported
            }
        ));
    }

    /// §12: state mismatch ⇒ typed rejection, nothing exchanged or stored.
    #[tokio::test]
    async fn tampered_state_fails_and_exchanges_nothing() {
        let rig = Rig {
            tamper_state: true,
            ..Rig::new()
        };
        let err = rig.run("srvtest-state").await.expect_err("typed");
        assert!(matches!(
            err,
            OAuthError::Failed {
                reason: FailReason::StateMismatch
            }
        ));
        assert!(
            rig.scoped.call_log().iter().all(|c| !c.starts_with("FORM")),
            "no token exchange after state mismatch: {:?}",
            rig.scoped.call_log()
        );
    }

    // --- §6.5 refresh discipline ------------------------------------------

    fn grant_record(server_id: &str) -> GrantRecord {
        GrantRecord {
            grant_id: "g1".into(),
            server_id: server_id.into(),
            as_origin: "https://as.example".into(),
            issuer: "https://as.example".into(),
            endpoints: vec![
                GrantEndpoint {
                    method: HttpMethod::Post,
                    path: "/token".into(),
                },
                GrantEndpoint {
                    method: HttpMethod::Get,
                    path: "/.well-known/oauth-authorization-server".into(),
                },
            ],
        }
    }

    fn refresh_script(
        refresh_hits: Arc<Mutex<usize>>,
    ) -> impl Fn(&str, &[GrantEndpoint]) -> Result<Box<dyn OAuthTransport>, OAuthError> {
        move |_origin, _endpoints| {
            *refresh_hits.lock().unwrap() += 1;
            let script = Script::default().with_form(
                "https://as.example/token",
                200,
                r#"{"access_token":"fresh-access-token-value","token_type":"Bearer","expires_in":3600}"#,
            );
            Ok(Box::new(Arc::new(script)) as Box<dyn OAuthTransport>)
        }
    }

    fn seed_stale(storage: &dyn TokenStorage, server_id: &str) {
        let stale = StoredTokens {
            access_token: Some("stale-access-token-value".into()),
            refresh_token: Some("stale-refresh-token-value".into()),
            expires_at_unix: Some(1),
        };
        storage.store(server_id, &stale).expect("seed");
    }

    #[test]
    fn proactive_refresh_skew_matrix() {
        let now = 1_000u64;
        let fresh = StoredTokens {
            access_token: Some("a".into()),
            refresh_token: Some("r".into()),
            expires_at_unix: Some(now + 120),
        };
        assert!(!token_needs_refresh(&fresh, now));
        let expiring = StoredTokens {
            expires_at_unix: Some(now + REFRESH_SKEW_SECS),
            ..fresh.clone()
        };
        assert!(token_needs_refresh(&expiring, now), "30s skew boundary");
        // No access token (file-path load) ⇒ refresh before use.
        let no_access = StoredTokens {
            access_token: None,
            refresh_token: Some("r".into()),
            expires_at_unix: None,
        };
        assert!(token_needs_refresh(&no_access, now));
        // No expiry advertised ⇒ usable until a 401 says otherwise.
        let no_expiry = StoredTokens {
            expires_at_unix: None,
            ..fresh.clone()
        };
        assert!(!token_needs_refresh(&no_expiry, now));
        // Nothing to refresh WITH ⇒ false (the caller types
        // AuthorizationRequired instead).
        let no_refresh = StoredTokens {
            refresh_token: None,
            ..fresh
        };
        assert!(!token_needs_refresh(&no_refresh, now));
    }

    #[test]
    fn reactive_401_retry_classes() {
        // Handshake/read-only retry once after the refresh.
        for m in [
            "initialize",
            "tools/list",
            "resources/list",
            "resources/read",
        ] {
            assert_eq!(retry_class(m, false), RetryClass::ReadOnly, "{m}");
        }
        // Idempotency-keyed calls retry.
        assert_eq!(
            retry_class("tools/call", true),
            RetryClass::IdempotencyKeyed
        );
        // Default tools/call: NO retry — typed AuthorizationRequired +
        // explicit user retry (no blind double-execution).
        assert_eq!(retry_class("tools/call", false), RetryClass::NonIdempotent);
        assert_eq!(
            retry_class("unknown/method", false),
            RetryClass::NonIdempotent
        );
    }

    /// §12 401 exactly-once (module-level half): ONE refresh, then
    /// read-only ⇒ Retry, non-idempotent ⇒ AuthorizationRequired. The
    /// wire-level single-effect proof is the §13 side-effect fixture.
    #[tokio::test]
    async fn on_unauthorized_refreshes_once_then_classifies() {
        let refresh_hits = Arc::new(Mutex::new(0usize));
        let scoped_factory = refresh_script(Arc::clone(&refresh_hits));
        let server_id = "srvtest401";
        let storage = MemStore::default();
        seed_stale(&storage, server_id);
        let coordinator = RefreshCoordinator::default();
        let record = grant_record(server_id);
        let now = || 1_800_000_000u64;

        let outcome = coordinator
            .on_unauthorized(
                server_id,
                RetryClass::ReadOnly,
                &storage,
                &scoped_factory,
                &record,
                &now,
            )
            .await
            .expect("refresh");
        assert_eq!(outcome, ReactiveOutcome::Retry);
        assert_eq!(*refresh_hits.lock().unwrap(), 1, "exactly ONE refresh");

        // Non-idempotent: the refresh is forced (the 401 means the server
        // rejected the token), but the outcome demands an explicit retry.
        let outcome = coordinator
            .on_unauthorized(
                server_id,
                RetryClass::NonIdempotent,
                &storage,
                &scoped_factory,
                &record,
                &now,
            )
            .await
            .expect("refresh");
        assert_eq!(outcome, ReactiveOutcome::AuthorizationRequired);
        assert_eq!(*refresh_hits.lock().unwrap(), 2, "one refresh per 401");

        // The stored set carries the fresh access token.
        let stored = storage.load(server_id).expect("load").expect("stored");
        assert_eq!(
            stored.access_token.as_deref(),
            Some("fresh-access-token-value")
        );
        let _ = storage.delete(server_id);
    }

    /// §6.5 dedup: concurrent proactive refreshes serialize on the
    /// per-server mutex — the refresh POST happens ONCE.
    #[tokio::test]
    async fn concurrent_refreshes_serialize_on_the_per_server_mutex() {
        let refresh_hits = Arc::new(Mutex::new(0usize));
        let scoped_factory = refresh_script(Arc::clone(&refresh_hits));
        let server_id = "srvtestdedup";
        let storage = MemStore::default();
        seed_stale(&storage, server_id);
        let coordinator = RefreshCoordinator::default();
        let record = grant_record(server_id);
        let now = || 1_800_000_000u64;
        let (a, b) = tokio::join!(
            coordinator.refresh_if_needed(server_id, &storage, &scoped_factory, &record, &now),
            coordinator.refresh_if_needed(server_id, &storage, &scoped_factory, &record, &now),
        );
        a.expect("first");
        b.expect("second");
        assert_eq!(
            *refresh_hits.lock().unwrap(),
            1,
            "the second refresh observes the first's stored result"
        );
        let _ = storage.delete(server_id);
    }
}
