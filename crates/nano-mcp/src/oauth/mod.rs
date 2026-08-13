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
    /// The validated grant failed journal-side validation (§6.3 bounds) —
    /// the login aborts BEFORE any endpoint grant exists in a live policy.
    GrantRejected,
    /// The grant journal append failed (F-36 producer): journal-first means
    /// the login aborts and no scoped client is built.
    JournalUnavailable,
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
            FailReason::GrantRejected => "grant_rejected",
            FailReason::JournalUnavailable => "journal_unavailable",
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

    /// Bounded body read. The Content-Length pre-check is only a fast path:
    /// a content-length-less (chunked) body is STREAMED with a hard byte
    /// counter (the `fetch_bounded` pattern), refusing at the cap — never
    /// fully buffered before the check, so a hostile AS cannot force an
    /// unbounded allocation.
    async fn read_bounded(
        &self,
        mut response: reqwest::Response,
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
        let mut body: Vec<u8> = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > MAX_METADATA_BYTES {
                        return Err(OAuthError::Failed {
                            reason: FailReason::MetadataInvalid,
                        });
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => {
                    return Err(OAuthError::Transport {
                        detail: nano_egress::client::sanitize_transport_error(&e),
                    });
                }
            }
        }
        Ok((status, body))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// MEDIUM-3 regression pin (§6.3): a content-length-less (chunked) body
    /// larger than MAX_METADATA_BYTES is cut AT the cap — streamed with a
    /// hard byte counter, never fully buffered first. The server streams
    /// forever, so the read can only end via the cap cut; a fully-buffering
    /// read would hang to the client's 300s total timeout (and then surface
    /// as Transport, not the typed refusal).
    #[tokio::test]
    async fn chunked_body_over_cap_is_refused_without_full_buffering() {
        use std::io::Read as _;
        use std::io::Write as _;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            // Consume the request head.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .expect("headers");
            // Stream 4 KiB chunks forever — no terminating chunk, no end.
            let payload = vec![b'x'; 4096];
            loop {
                let frame = format!("{:x}\r\n", payload.len());
                if stream.write_all(frame.as_bytes()).is_err()
                    || stream.write_all(&payload).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    break; // the client cut the stream
                }
            }
        });

        let transport = EgressTransport::new(nano_egress::client::EgressClient::without_redirects(
            nano_egress::policy::EgressPolicy::new().allow_host_with_http("127.0.0.1"),
        ));
        let url = format!("http://127.0.0.1:{port}/.well-known/oauth-authorization-server");
        let started = std::time::Instant::now();
        let err = transport
            .get_bounded(&url)
            .await
            .expect_err("an over-cap chunked body must be refused");
        assert!(
            matches!(
                err,
                OAuthError::Failed {
                    reason: FailReason::MetadataInvalid
                }
            ),
            "err: {err}"
        );
        // The never-ending body was cut at the cap. A buffering read would
        // only end at the 300s client timeout — a 30s bound is a 10x margin
        // over the sub-second streaming cut and 10x under the broken mode.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the refusal must come from the streaming cap cut, not a timeout: {:?}",
            started.elapsed()
        );
    }
}
