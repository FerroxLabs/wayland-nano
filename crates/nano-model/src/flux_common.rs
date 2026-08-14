//! Shared wire plumbing for all three Flux inference surfaces
//! (Completions / Responses / Anthropic Messages).
//!
//! Error classification and transport mapping live here exactly once so the
//! adapters can never diverge on the corrected live-wire rules (FINDINGS.md
//! batch 3): invalid key arrives as HTTP 500 with `error.type=="auth_error"`
//! (non-retryable Auth), context overflow arrives as HTTP 413, burst load
//! saturates the edge with bare 503 HTML (retryable Server), and a retired
//! model id arrives as HTTP 404 (F-18: typed `ModelNotFound`, terminal —
//! never the undifferentiated 4xx bucket). These behaviors
//! were probed on the Completions surface; the Responses/Anthropic error
//! bodies were not separately probed (same LiteLLM proxy — flagged gap, not
//! a blocker), so all adapters share this single classification path.

use crate::types::{ModelError, TransportPhase};

pub const FLUX_BASE: &str = "https://api.fluxrouter.ai";

/// Map a reqwest transport failure to a typed TransportPhase (C9):
/// reqwest's typed accessors + whether the adapter has observed any
/// response byte — never string inspection. `is_connect()` covers TCP
/// connect AND TLS handshake failures (reqwest's public taxonomy folds the
/// handshake into the connect class; the retry classes are identical, so no
/// behavior diverges). A `send()`-phase failure that is not connect means
/// the request went out but no response byte arrived: `BeforeFirstByte`.
/// Failures while reading the body are `MidStream`. Message text still
/// passes through the single nano-egress redaction path (fail-closed).
pub fn classify_transport(err: reqwest::Error, response_started: bool) -> ModelError {
    let phase = if err.is_connect() {
        TransportPhase::Connect
    } else if response_started {
        TransportPhase::MidStream
    } else {
        TransportPhase::BeforeFirstByte
    };
    ModelError::Transport {
        phase,
        message: nano_egress::client::sanitize_transport_error(&err),
    }
}

/// F-14 (sev-2, 2026-08-14 adjudication): the provider error body is
/// provider-controlled — read it BOUNDED. `response.text()` would allocate
/// whatever a hostile/misbehaving endpoint sends; the cap keeps the
/// classification inputs (a small JSON error object) intact while refusing
/// unbounded allocation. Truncated JSON simply falls through to the generic
/// status-class arms in `classify_status`.
pub const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

pub async fn read_error_body(mut response: reqwest::Response) -> String {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => {
                let room = MAX_ERROR_BODY_BYTES.saturating_sub(buf.len());
                if room == 0 {
                    break;
                }
                let take = bytes.len().min(room);
                buf.extend_from_slice(&bytes[..take]);
                if take < bytes.len() {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
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
        // Typed HTTP-status provenance (C9): the 401 recovery seam retries
        // exactly Some(401); 403 and every other auth failure never retry.
        401 | 403 => ModelError::Auth {
            message,
            status: Some(status),
        },
        // Live wire (batch-3 badkey fixture): an invalid key arrives as
        // HTTP 500 with error.type=="auth_error", NOT 401. The embedded
        // message carries `key=<sha256-of-presented-key>` — a digest, not
        // the key itself, matching this crate's hashed-digest convention —
        // so carrying it in ModelError::Auth logs is acceptable.
        500 if error_type == Some("auth_error") => ModelError::Auth {
            message,
            status: Some(500),
        },
        402 => ModelError::Entitlement(message),
        // F-18: a 404 on an inference call is the provider reporting the
        // addressed model id unknown/retired (live provider proofs:
        // cerebras, fireworks). The typed class, never the 4xx bucket, so
        // kind-keyed fallback/model-retirement logic fires.
        404 => ModelError::ModelNotFound {
            status: 404,
            message,
        },
        // Kept for spec compliance; live Flux never sends 429 (burst load
        // saturates the edge with bare 503 nginx HTML, no Retry-After).
        // P5 §4 classifier precedence (conservative wins): a 429 whose body
        // carries the auth_error marker is an auth failure — terminal, never
        // rate-limited (same live-wire fold as the 500 arm above).
        429 if error_type == Some("auth_error") => ModelError::Auth {
            message,
            status: Some(429),
        },
        429 => ModelError::RateLimited {
            retry_after_ms: None,
        },
        // Live wire (F-P5-1): the edge answers a malformed request body
        // (e.g. a tool payload a leaf cannot parse) with HTTP 5xx whose
        // body carries error.type=="invalid_request_error". That is a
        // FORMAT rejection, not a server fault: terminal, never retried,
        // and — via signals_of_model_error → FormatRejected — never a
        // routing cascade (the identical bytes would fail every rung).
        s if s >= 500 && error_type == Some("invalid_request_error") => {
            ModelError::InvalidRequest { status: s, message }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// F-14: provider-controlled error bodies are read BOUNDED — a hostile
    /// endpoint streaming gigabytes must not allocate them. The classification
    /// input (a small JSON error object) is unaffected.
    #[tokio::test]
    async fn error_body_read_is_bounded() {
        let big = vec![b'x'; MAX_ERROR_BODY_BYTES + 128 * 1024];
        let response = reqwest::Response::from(
            http::Response::builder()
                .status(500)
                .body(reqwest::Body::from(big))
                .unwrap(),
        );
        let body = read_error_body(response).await;
        assert_eq!(body.len(), MAX_ERROR_BODY_BYTES);

        let small = br#"{"error":{"type":"invalid_request_error"}}"#.to_vec();
        let response = reqwest::Response::from(
            http::Response::builder()
                .status(500)
                .body(reqwest::Body::from(small.clone()))
                .unwrap(),
        );
        let body = read_error_body(response).await;
        assert_eq!(body.as_bytes(), small.as_slice());
    }

    /// F-18: a provider 404 (retired/unknown model id at the endpoint —
    /// the live-proof matrix hit this on cerebras/fireworks) classifies as
    /// the TYPED model-not-found, never the undifferentiated 4xx bucket, so
    /// kind-keyed fallback/retirement logic fires.
    #[test]
    fn provider_404_classifies_as_model_not_found() {
        let body = r#"{"error":{"message":"model llama-v3p1-70b-instruct not found","type":"invalid_request_error","code":"model_not_found"}}"#;
        let err = classify_status(404, body.to_string());
        let ModelError::ModelNotFound { status, message } = err else {
            panic!("404 must classify as ModelNotFound, got {err:?}");
        };
        assert_eq!(status, 404);
        assert!(message.contains("not found"), "{message}");

        // The classification keys on the STATUS alone — a bare/HTML 404
        // body (no parseable error object) is still model-not-found.
        let err = classify_status(404, "<html>nginx 404</html>".to_string());
        assert!(
            matches!(err, ModelError::ModelNotFound { status: 404, .. }),
            "bare 404 must classify as ModelNotFound, got {err:?}"
        );
    }
}
