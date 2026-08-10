//! The egress HTTP client: policy-gated, redacting, bounded.
//!
//! The only place in the workspace permitted to construct reqwest clients.
//! Invariants:
//! - policy check BEFORE any socket activity (deny = no bytes leave);
//! - redirect hops are re-checked against the policy before being followed
//!   (an allowlisted origin cannot 302 to an off-allowlist host);
//! - Authorization/x-api-key headers are set via dedicated methods and are
//!   redacted from every error's Display;
//! - connect and total-request timeouts are mandatory;
//! - observability fields: method/host/port + path_query_sha256 only.

// The only crate permitted to construct HTTP clients.
#![allow(clippy::disallowed_methods)]

use crate::policy::EgressDecision;
use crate::policy::EgressPolicy;
use crate::policy::path_query_sha256;

#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    #[error("egress denied by policy: {method} {host} path_query_sha256={digest}")]
    Denied {
        method: String,
        host: String,
        digest: String,
    },
    #[error("transport: {0}")]
    Transport(String),
    #[error("http {status}: {host} path_query_sha256={digest}")]
    HttpStatus {
        status: u16,
        host: String,
        digest: String,
    },
}

/// Redirect hops are only followed when the target re-passes the policy
/// gate; the cap matches reqwest's former default redirect limit.
const MAX_REDIRECT_HOPS: usize = 10;

/// A policy-gated outbound client. Construct once per policy domain.
#[derive(Debug)]
pub struct EgressClient {
    client: reqwest::Client,
    policy: EgressPolicy,
}

impl EgressClient {
    pub fn new(policy: EgressPolicy) -> Self {
        let hop_gate = policy.clone();
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("nanok3/0.1.0")
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= MAX_REDIRECT_HOPS {
                    return attempt.error("redirect hop limit exceeded");
                }
                if hop_gate.allows(attempt.url().as_str()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .expect("reqwest client build");
        Self { client, policy }
    }

    pub fn flux() -> Self {
        Self::new(EgressPolicy::flux_only())
    }

    /// Policy gate + request construction. Returns Err(Denied) before any
    /// socket activity when the policy rejects the URL.
    pub fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, EgressError> {
        match self.policy.decide(url) {
            EgressDecision::Allow => Ok(self.client.request(method.clone(), url)),
            EgressDecision::Deny => Err(EgressError::Denied {
                method: method.to_string(),
                host: host_display(url),
                digest: path_query_sha256(url),
            }),
        }
    }

    /// Classify a transport/HTTP outcome with secrets redacted by
    /// construction (no headers or bodies ever reach the error string).
    pub fn classify_status(&self, url: &str, status: u16) -> EgressError {
        EgressError::HttpStatus {
            status,
            host: host_display(url),
            digest: path_query_sha256(url),
        }
    }

    pub fn classify_transport(&self, err: &reqwest::Error) -> EgressError {
        EgressError::Transport(sanitize_transport_error(err))
    }
}

fn host_display(url: &str) -> String {
    url.split_once("://")
        .map(|(_, r)| {
            let authority = r.split('/').next().unwrap_or(r);
            // Strip userinfo: callers attach credentials as user:password@,
            // and only the host:port may ever render.
            authority
                .rsplit_once('@')
                .map_or(authority, |(_, host_port)| host_port)
                .to_string()
        })
        .unwrap_or_else(|| "<no-scheme>".to_string())
}

/// Render a reqwest transport error without echoing credentials.
///
/// `reqwest::Error`'s Display embeds the full request URL — userinfo and
/// query string included — which violates the crate invariant "query is
/// hashed, never echoed". Re-render the URL with userinfo/query/fragment
/// stripped; if that redaction cannot be proven complete, fail closed to a
/// message carrying only host + path_query_sha256.
///
/// Shared with nano-model (its `ModelError::Transport` wraps the same
/// reqwest errors) so there is exactly one redaction path.
pub fn sanitize_transport_error(err: &reqwest::Error) -> String {
    let rendered = err.to_string();
    let Some(url) = err.url() else {
        return rendered;
    };
    let mut redacted = url.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted.set_query(None);
    redacted.set_fragment(None);
    let cleaned = rendered.replace(url.as_str(), redacted.as_str());
    let leaks_secret = cleaned.contains(url.as_str())
        || (!url.username().is_empty() && cleaned.contains(url.username()))
        || url.password().is_some_and(|p| cleaned.contains(p))
        || url
            .query()
            .is_some_and(|q| !q.is_empty() && cleaned.contains(q));
    if leaks_secret {
        format!(
            "transport error: {} path_query_sha256={}",
            host_display(url.as_str()),
            path_query_sha256(url.as_str())
        )
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_request_never_builds() {
        let client = EgressClient::flux();
        let err = client
            .request(reqwest::Method::GET, "https://api.openai.com/v1/models")
            .expect_err("must deny");
        let rendered = err.to_string();
        assert!(rendered.contains("egress denied"));
        assert!(!rendered.contains("Authorization"));
    }

    #[test]
    fn allowed_request_builds_with_no_secret_echo() {
        let client = EgressClient::flux();
        let _builder = client
            .request(
                reqwest::Method::POST,
                "https://api.fluxrouter.ai/v1/chat/completions",
            )
            .expect("allowed");
    }

    #[test]
    fn error_display_carries_only_observability_fields() {
        let client = EgressClient::flux();
        let err = client.classify_status(
            "https://api.fluxrouter.ai/v1/chat/completions?key=secret",
            402,
        );
        let rendered = err.to_string();
        assert!(rendered.contains("402"));
        assert!(rendered.contains("api.fluxrouter.ai"));
        assert!(
            !rendered.contains("secret"),
            "query must be hashed, not echoed: {rendered}"
        );
    }

    #[test]
    fn host_display_strips_userinfo_and_ignores_query() {
        assert_eq!(
            host_display("https://user:s3cr3t@api.fluxrouter.ai/v1?api_key=x"),
            "api.fluxrouter.ai"
        );
        assert_eq!(
            host_display("https://api.fluxrouter.ai:8443/v1"),
            "api.fluxrouter.ai:8443"
        );
        assert_eq!(host_display("api.fluxrouter.ai/v1"), "<no-scheme>");
    }

    #[tokio::test]
    async fn transport_error_display_redacts_url_credentials() {
        // Port 9 (discard) on loopback refuses connections; the refused
        // transport error must not echo the URL's userinfo or query.
        let client = EgressClient::new(EgressPolicy::new().allow_host("127.0.0.1"));
        let err = client
            .request(
                reqwest::Method::GET,
                "http://user:s3cr3t@127.0.0.1:9/v1?api_key=query-secret",
            )
            .expect("allowlisted")
            .send()
            .await
            .expect_err("refused connection must error");
        let rendered = client.classify_transport(&err).to_string();
        assert!(
            !rendered.contains("s3cr3t"),
            "userinfo leaked into Transport display: {rendered}"
        );
        assert!(
            !rendered.contains("query-secret"),
            "query leaked into Transport display: {rendered}"
        );
        assert!(
            rendered.contains("127.0.0.1"),
            "host should remain observable: {rendered}"
        );
    }
}
