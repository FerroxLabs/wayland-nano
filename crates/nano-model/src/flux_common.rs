//! Shared wire plumbing for all three Flux inference surfaces
//! (Completions / Responses / Anthropic Messages).
//!
//! Error classification and transport mapping live here exactly once so the
//! adapters can never diverge on the corrected live-wire rules (FINDINGS.md
//! batch 3): invalid key arrives as HTTP 500 with `error.type=="auth_error"`
//! (non-retryable Auth), context overflow arrives as HTTP 413, and burst load
//! saturates the edge with bare 503 HTML (retryable Server). These behaviors
//! were probed on the Completions surface; the Responses/Anthropic error
//! bodies were not separately probed (same LiteLLM proxy — flagged gap, not
//! a blocker), so all adapters share this single classification path.

use crate::types::ModelError;

pub const FLUX_BASE: &str = "https://api.fluxrouter.ai";

/// Map a reqwest transport failure through the single redaction path
/// (nano-egress strips userinfo/query — fail-closed).
pub fn classify_transport(err: reqwest::Error) -> ModelError {
    ModelError::Transport(nano_egress::client::sanitize_transport_error(&err))
}

pub async fn read_error_body(response: reqwest::Response) -> String {
    response.text().await.unwrap_or_default()
}

/// Classify an HTTP error status + body into a typed ModelError.
///
/// C8 §8 (B4): the provider-controlled body text passes through the ONE
/// credential-aware sanitization boundary before it can reach a Display —
/// a provider echoing the presented Authorization/x-api-key credential in
/// its error body must not leak it into logs, stderr, or ACP error frames.
pub fn classify_status(status: u16, body: String) -> ModelError {
    let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
    let error = parsed.as_ref().and_then(|v| v.get("error"));
    let message = nano_egress::redact::sanitize_text(
        error
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or(""),
    )
    .to_string();
    let error_type = error.and_then(|e| e.get("type")).and_then(|t| t.as_str());
    match status {
        401 | 403 => ModelError::Auth(message),
        // Live wire (batch-3 badkey fixture): an invalid key arrives as
        // HTTP 500 with error.type=="auth_error", NOT 401. The embedded
        // message carries `key=<sha256-of-presented-key>` — a digest, not
        // the key itself, matching this crate's hashed-digest convention —
        // so carrying it in ModelError::Auth logs is acceptable.
        500 if error_type == Some("auth_error") => ModelError::Auth(message),
        402 => ModelError::Entitlement(message),
        // Kept for spec compliance; live Flux never sends 429 (burst load
        // saturates the edge with bare 503 nginx HTML, no Retry-After).
        429 => ModelError::RateLimited {
            retry_after_ms: None,
        },
        // Live wire (batch-3 overlimit fixture): context overflow arrives as
        // HTTP 413 with error.message=="context_window_exceeded".
        413 => ModelError::ContextOverflow(message),
        400 if message.contains("context") || message.contains("token") => {
            ModelError::ContextOverflow(message)
        }
        s if s >= 500 => ModelError::Server { status: s, message },
        s => ModelError::Server { status: s, message },
    }
}

/// Map a parser cap violation to a protocol integrity error (fail-closed:
/// a hostile stream errors the completion, it is never truncated).
pub fn sse_integrity_error(err: crate::sse::SseError) -> ModelError {
    ModelError::Protocol(format!("sse stream rejected: {err}"))
}

/// Conservative context-window fallback for models the Flux catalog does
/// not classify with `max_input_tokens` (C1). 128k is below every window
/// the current Flux catalog reports, so unknown models trigger compaction
/// EARLY, never late; the reactive-overflow path backstops any tier whose
/// real window is smaller still.
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;

/// The model's context window: the catalog's `max_input_tokens` when the
/// router reports it for this id, else [`DEFAULT_CONTEXT_WINDOW_TOKENS`].
pub fn context_window_for(model_id: &str, catalog: &[crate::flux_models::FluxModel]) -> u64 {
    catalog
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.max_input_tokens)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
}
