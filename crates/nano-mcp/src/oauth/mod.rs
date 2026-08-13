//! OAuth for remote MCP servers (P3 design §6): authorization-code + PKCE
//! (S256 only), RFC 9728 → RFC 8414 trust-chained AS discovery, a fully
//! bound loopback callback listener, keyring-primary token storage with an
//! operator-provisioned `<VAR>_FILE` refresh path, and the refresh-on-401
//! discipline.
//!
//! Secrets discipline (AGENTS.md §Secrets, absolute): access and refresh
//! tokens are registered with the value sanitizer
//! (`nano_egress::redact::register_credential`) the moment they exist and are
//! NEVER journaled, logged, framed, or returned in tool results — the
//! journal carries reference handles only (the `GrantRecord` handed to the
//! `record_grant` hook carries ids, origins, and canonical endpoint
//! method+path pairs, never token material). Error `Display` impls render
//! bounded reason codes only — never provider prose, endpoint URLs, or
//! token values.
//!
//! Cross-lane wiring (owned elsewhere, programmed against here):
//! - the dispatcher lane owns the stdio/HTTP transports and calls into this
//!   module for token acquisition/refresh;
//! - the journal lane owns `Op::McpOauthGrant` — this module hands it a
//!   `GrantRecord` through the `record_grant` hook BEFORE any endpoint grant
//!   exists in a live policy (journal-first);
//! - the CLI/TUI lane owns `nano auth login|status|logout`, the operator
//!   approval prompt, and the browser handoff — all injected here as hooks;
//! - the error-table lane maps [`OAuthError`] onto the §7 `NanoErrorKind`
//!   variants (`McpAuthorizationRequired` / `McpCredstoreUnavailable` /
//!   `McpOAuthFailed`) — this module's variants are named to match 1:1.

pub mod discovery;
pub mod flow;
pub mod loopback;
pub mod pkce;
pub mod storage;
#[cfg(target_os = "windows")]
pub mod wincred;

use std::fmt;

/// The typed failures of the OAuth subsystem. Named 1:1 with the §7 error
/// kinds so the error-table lane maps them mechanically; every variant is
/// non-retryable at this layer (retry discipline lives in `flow`).
#[derive(Debug)]
pub enum OAuthError {
    /// No usable token at call time; 401 after one refresh+retry
    /// (§6.5) → `McpAuthorizationRequired`.
    AuthorizationRequired { server_id: String },
    /// Keyring unavailable AND no operator refresh-file; 0600/ACL audit
    /// failure (§6.4) → `McpCredstoreUnavailable`.
    CredstoreUnavailable { detail: String },
    /// Discovery/trust-chain failure, S256-only violation, state mismatch,
    /// callback timeout, provider error params (§6.2–6.3) → `McpOAuthFailed`.
    Failed { reason: FailReason },
    /// Egress denial surfaced from the gate (deny = zero socket activity).
    EgressDenied { detail: String },
    /// Sanitized transport failure (never carries URLs or bodies).
    Transport { detail: String },
}

/// The bounded reason vocabulary for [`OAuthError::Failed`] — an enum
/// string, never provider prose (§7: "the `McpOAuthFailed` reason is a
/// bounded enum string"). `ProviderError` carries the provider's `error`
/// CODE ONLY, sanitized to a bounded charset and length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailReason {
    /// Operator declined the discovered AS origin (§6.2 step 2).
    OperatorDeclined,
    /// The server doesn't speak RFC 9728 (no heuristic fallback).
    NoProtectedResourceMetadata,
    /// Fetched metadata's `issuer` ≠ the candidate (RFC 8414 issuer check).
    IssuerMismatch,
    /// An endpoint on a different origin than the validated AS.
    CrossOriginEndpoint,
    /// The AS metadata doesn't prove S256 support — never downgrade.
    S256Unsupported,
    /// Callback `state` ≠ the bound state (listener closed, nothing stored).
    StateMismatch,
    /// No valid callback within the listener's expiry.
    CallbackTimeout,
    /// Malformed callback (bad method/path/Host/oversized/duplicate).
    CallbackRejected,
    /// A metadata/token endpoint answered a redirect (redirects DISABLED on
    /// every OAuth client) — a typed failure, never a followed hop.
    RedirectRejected,
    /// Metadata JSON invalid or missing required members.
    MetadataInvalid,
    /// The provider answered with an `error` parameter (bounded code only).
    ProviderError(String),
    /// Dynamic client registration failed (bounded code only).
    RegistrationFailed(String),
}

impl FailReason {
    /// The stable wire/log token for this reason.
    pub fn as_str(&self) -> &str {
        match self {
            FailReason::OperatorDeclined => "operator_declined",
            FailReason::NoProtectedResourceMetadata => "no_protected_resource_metadata",
            FailReason::IssuerMismatch => "issuer_mismatch",
            FailReason::CrossOriginEndpoint => "cross_origin_endpoint",
            FailReason::S256Unsupported => "s256_unsupported",
            FailReason::StateMismatch => "state_mismatch",
            FailReason::CallbackTimeout => "callback_timeout",
            FailReason::CallbackRejected => "callback_rejected",
            FailReason::RedirectRejected => "redirect_rejected",
            FailReason::MetadataInvalid => "metadata_invalid",
            FailReason::ProviderError(_) => "provider_error",
            FailReason::RegistrationFailed(_) => "registration_failed",
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Bounded reason codes only — never provider prose, endpoint URLs,
        // or token material (§7 nanoError extras stay a closed vocabulary).
        match self {
            OAuthError::AuthorizationRequired { server_id } => {
                write!(f, "login required for MCP server {server_id}")
            }
            OAuthError::CredstoreUnavailable { detail } => {
                write!(f, "secure credential storage is unavailable: {detail}")
            }
            OAuthError::Failed { reason } => {
                write!(f, "OAuth login failed ({})", reason.as_str())
            }
            OAuthError::EgressDenied { detail } => write!(f, "egress denied: {detail}"),
            OAuthError::Transport { detail } => write!(f, "transport: {detail}"),
        }
    }
}

impl std::error::Error for OAuthError {}

/// The HTTP seam every OAuth flow rides. Production adapters wrap
/// `nano_egress::client::EgressClient` (the policy gate fires per call;
/// deny = zero socket activity); tests substitute scripted transports.
///
/// Implementations MUST NOT follow redirects (every OAuth client is built
/// with redirects disabled, §6.3 step 4) and MUST bound response bodies to
/// [`MAX_METADATA_BYTES`].
#[async_trait::async_trait]
pub trait OAuthTransport: Send + Sync {
    /// Bounded GET. Returns `(status, body)`; a 3xx is RETURNED (never
    /// followed) so the caller can raise `RedirectRejected`.
    async fn get_bounded(&self, url: &str) -> Result<(u16, Vec<u8>), OAuthError>;
    /// Bounded POST of an `application/x-www-form-urlencoded` body.
    async fn post_form(
        &self,
        url: &str,
        form: &[(String, String)],
    ) -> Result<(u16, Vec<u8>), OAuthError>;
    /// Bounded POST of a JSON body (RFC 7591 dynamic client registration).
    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, Vec<u8>), OAuthError>;
}

/// Hard cap for RFC 9728/8414 metadata, DCR, and token-endpoint bodies.
pub const MAX_METADATA_BYTES: usize = 64 * 1024;

/// Production [`OAuthTransport`] over an [`EgressClient`] (owned, so
/// factory closures can hand scoped/bootstrap clients to the flow). The
/// egress policy gate fires per call — deny = zero socket activity. Bodies
/// are length-capped; egress denials map to [`OAuthError::EgressDenied`];
/// transport errors pass through `sanitize_transport_error` (no URL
/// userinfo/query echo).
pub struct EgressTransport {
    client: nano_egress::client::EgressClient,
}

impl EgressTransport {
    pub fn new(client: nano_egress::client::EgressClient) -> Self {
        Self { client }
    }

    async fn read_bounded(
        &self,
        response: reqwest::Response,
    ) -> Result<(u16, Vec<u8>), OAuthError> {
        let status = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|n| n as usize > MAX_METADATA_BYTES)
        {
            return Err(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            });
        }
        let body = response.bytes().await.map_err(|e| OAuthError::Transport {
            detail: nano_egress::client::sanitize_transport_error(&e),
        })?;
        if body.len() > MAX_METADATA_BYTES {
            return Err(OAuthError::Failed {
                reason: FailReason::MetadataInvalid,
            });
        }
        Ok((status, body.to_vec()))
    }

    fn gated(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, OAuthError> {
        self.client
            .request(method, url)
            .map_err(|e| OAuthError::EgressDenied {
                detail: e.to_string(),
            })
    }
}

#[async_trait::async_trait]
impl OAuthTransport for EgressTransport {
    async fn get_bounded(&self, url: &str) -> Result<(u16, Vec<u8>), OAuthError> {
        let response = self
            .gated(reqwest::Method::GET, url)?
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| OAuthError::Transport {
                detail: nano_egress::client::sanitize_transport_error(&e),
            })?;
        self.read_bounded(response).await
    }

    async fn post_form(
        &self,
        url: &str,
        form: &[(String, String)],
    ) -> Result<(u16, Vec<u8>), OAuthError> {
        let response = self
            .gated(reqwest::Method::POST, url)?
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .body(form_encode(form))
            .send()
            .await
            .map_err(|e| OAuthError::Transport {
                detail: nano_egress::client::sanitize_transport_error(&e),
            })?;
        self.read_bounded(response).await
    }

    async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, Vec<u8>), OAuthError> {
        let response = self
            .gated(reqwest::Method::POST, url)?
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| OAuthError::Transport {
                detail: nano_egress::client::sanitize_transport_error(&e),
            })?;
        self.read_bounded(response).await
    }
}

/// application/x-www-form-urlencoded per RFC 6749 §4.1.3: every name and
/// value percent-encoded with the url crate's form rules (the same crate
/// the egress client sends through).
pub fn form_encode(pairs: &[(String, String)]) -> String {
    let mut url = reqwest::Url::parse("https://form.invalid/").expect("static URL");
    {
        let mut ser = url.query_pairs_mut();
        for (k, v) in pairs {
            ser.append_pair(k, v);
        }
    }
    url.query().unwrap_or("").to_string()
}

/// Sanitize a provider `error` code to the bounded vocabulary: at most 64
/// chars of `[a-z0-9_.-]` (lowercased); anything else collapses to a
/// generic token. Provider prose never crosses this boundary.
pub fn bounded_error_code(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "provider_error".to_string()
    } else {
        cleaned
    }
}
