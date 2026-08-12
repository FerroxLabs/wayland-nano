//! web_fetch tool layer (C4): argument validation + outcome mapping ONLY.
//!
//! Every piece of HTTP machinery — policy gate, manual redirect loop, DNS
//! resolve/private-IP deny/pin, streaming byte cap — lives in nano-egress
//! (`EgressClient::fetch_bounded`), the only crate permitted to construct
//! reqwest clients. This module never touches reqwest and cannot bypass
//! policy, redirects, pinning, or caps.
//!
//! The fetch policy is a SECOND egress domain, separate from the Flux API
//! allowlist: an empty fetch allowlist denies everything, so an
//! unconfigured tool is inert (deny-by-default preserved).

use nano_egress::client::{EgressClient, EgressError, FetchOutcome};

/// Per-tool caps (C4 §3.1). The global engine-side result ceiling is a
/// tracked follow-up (docs/FOLLOWUPS.md F-1).
pub const FETCH_MAX_BYTES_CAP: usize = 64 * 1024;
pub const FETCH_DEFAULT_MAX_BYTES: usize = 32 * 1024;
pub const FETCH_MIN_TIMEOUT_MS: u64 = 1_000;
pub const FETCH_MAX_TIMEOUT_MS: u64 = 30_000;
pub const FETCH_DEFAULT_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, thiserror::Error)]
pub enum WebFetchError {
    #[error("invalid web_fetch arguments: {0}")]
    Args(String),
    #[error(transparent)]
    Egress(#[from] EgressError),
}

/// Validated web_fetch arguments (clamps applied).
#[derive(Debug, Clone)]
pub struct FetchArgs {
    pub url: String,
    pub max_bytes: usize,
    pub timeout: std::time::Duration,
}

impl FetchArgs {
    /// Parse + clamp the tool arguments. Negative or non-integer numeric
    /// args are typed errors, never silently ignored.
    pub fn parse(arguments: &serde_json::Value) -> Result<Self, WebFetchError> {
        let url = arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| WebFetchError::Args("missing url".into()))?
            .to_string();
        let max_bytes = opt_u64(arguments, "max_bytes")?
            .map(|v| v.clamp(1, FETCH_MAX_BYTES_CAP as u64) as usize)
            .unwrap_or(FETCH_DEFAULT_MAX_BYTES);
        let timeout_ms = opt_u64(arguments, "timeout_ms")?
            .map(|v| v.clamp(FETCH_MIN_TIMEOUT_MS, FETCH_MAX_TIMEOUT_MS))
            .unwrap_or(FETCH_DEFAULT_TIMEOUT_MS);
        Ok(Self {
            url,
            max_bytes,
            timeout: std::time::Duration::from_millis(timeout_ms),
        })
    }
}

fn opt_u64(arguments: &serde_json::Value, key: &str) -> Result<Option<u64>, WebFetchError> {
    match arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| WebFetchError::Args(format!("{key} must be a non-negative integer"))),
    }
}

/// The web_fetch tool: holds the SECOND-domain egress client (built from
/// configured fetch hosts; exact hosts only, https unless a host carries
/// the per-host http opt-in).
#[derive(Debug)]
pub struct WebFetchTool {
    client: EgressClient,
}

impl WebFetchTool {
    pub fn new(client: EgressClient) -> Self {
        Self { client }
    }

    /// GET-only bounded fetch; all policy/redirect/pin/cap enforcement is
    /// inside nano-egress.
    pub async fn fetch(&self, args: &FetchArgs) -> Result<FetchOutcome, WebFetchError> {
        Ok(self
            .client
            .fetch_bounded(&args.url, args.max_bytes, args.timeout)
            .await?)
    }
}

/// Render the model-facing output. The body is RAW remote content —
/// explicitly untrusted (no extraction, no sanitization claims). Metadata
/// lines and the truncation footer live inside the per-tool output ceiling;
/// `body_bytes` counts only the delivered body.
pub fn render_fetch_output(outcome: &FetchOutcome) -> String {
    let declared = outcome
        .declared_bytes
        .map(|d| d.to_string())
        .unwrap_or_else(|| "unknown".into());
    let body = String::from_utf8_lossy(&outcome.body);
    let mut out = format!(
        "status: {}\nfinal_url: {}\ncontent_type: {}\nbody_bytes: {}\ndeclared_bytes: {} (Content-Length as declared by the server; not a verified total)\n\n[untrusted remote content below — raw passthrough]\n{}",
        outcome.status, outcome.final_url, outcome.content_type, outcome.body_bytes, declared, body,
    );
    if outcome.truncated {
        out.push_str(&format!(
            "\n…[truncated: body cut at {} bytes; declared_bytes={declared}]",
            outcome.body_bytes,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_clamp_and_default() {
        let args = FetchArgs::parse(&serde_json::json!({"url": "https://example.com/"})).unwrap();
        assert_eq!(args.max_bytes, FETCH_DEFAULT_MAX_BYTES);
        assert_eq!(args.timeout.as_millis(), FETCH_DEFAULT_TIMEOUT_MS as u128);
        let args = FetchArgs::parse(
            &serde_json::json!({"url": "https://example.com/", "max_bytes": 10_u64.pow(9), "timeout_ms": 0}),
        )
        .unwrap();
        assert_eq!(args.max_bytes, FETCH_MAX_BYTES_CAP);
        assert_eq!(args.timeout.as_millis(), FETCH_MIN_TIMEOUT_MS as u128);
    }

    #[test]
    fn args_reject_bad_types_and_missing_url() {
        let err = FetchArgs::parse(&serde_json::json!({"url": "https://x/", "max_bytes": -1}))
            .expect_err("negative is a typed error");
        assert!(matches!(err, WebFetchError::Args(_)));
        let err = FetchArgs::parse(&serde_json::json!({"url": "https://x/", "timeout_ms": "fast"}))
            .expect_err("non-integer is a typed error");
        assert!(matches!(err, WebFetchError::Args(_)));
        let err =
            FetchArgs::parse(&serde_json::json!({})).expect_err("missing url is a typed error");
        assert!(matches!(err, WebFetchError::Args(_)));
    }

    /// Deny-by-default: a fetch tool whose policy domain has NO allowlisted
    /// hosts denies everything — the tool is inert when unconfigured.
    #[tokio::test]
    async fn unconfigured_fetch_domain_denies_everything() {
        let tool = WebFetchTool::new(EgressClient::new(nano_egress::policy::EgressPolicy::new()));
        let args = FetchArgs::parse(&serde_json::json!({"url": "https://example.com/"})).unwrap();
        let err = tool.fetch(&args).await.expect_err("must deny");
        assert!(matches!(
            err,
            WebFetchError::Egress(EgressError::Denied { .. })
        ));
    }

    #[test]
    fn render_labels_declared_bytes_and_marks_truncation() {
        let outcome = FetchOutcome {
            status: 200,
            final_url: "https://example.com/".into(),
            content_type: "text/html".into(),
            body_bytes: 3,
            declared_bytes: Some(1000),
            truncated: true,
            body: b"abc".to_vec(),
        };
        let out = render_fetch_output(&outcome);
        assert!(out.contains("declared_bytes: 1000 (Content-Length as declared"));
        assert!(out.contains("…[truncated: body cut at 3 bytes; declared_bytes=1000]"));
        assert!(out.contains("untrusted remote content"));
    }
}
