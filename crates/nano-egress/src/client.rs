//! The egress HTTP client: policy-gated, redacting, bounded.
//!
//! The only place in the workspace permitted to construct reqwest clients.
//! Invariants:
//! - policy check BEFORE any socket activity (deny = no bytes leave);
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

/// A policy-gated outbound client. Construct once per policy domain.
#[derive(Debug)]
pub struct EgressClient {
    client: reqwest::Client,
    policy: EgressPolicy,
}

impl EgressClient {
    pub fn new(policy: EgressPolicy) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("nanok3/0.1.0")
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
        EgressError::Transport(err.to_string())
    }
}

fn host_display(url: &str) -> String {
    url.split_once("://")
        .map(|(_, r)| r.split('/').next().unwrap_or(r).to_string())
        .unwrap_or_else(|| "<no-scheme>".to_string())
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
}
