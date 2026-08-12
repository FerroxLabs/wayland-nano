//! C8 §8/§9 (B4) — fail-closed credential redaction, per wire client.
//!
//! The ONE sanitization boundary is nano-egress's registry: every resolved
//! credential is registered at resolution, and every provider-controlled
//! error surface passes through it. These canaries force each error class
//! per wire client and assert the canary is absent from Display AND Debug.
//!
//! Error classes: (1) request-construction/policy errors, (2) response
//! status + error-BODY echo (a provider reflecting the presented credential
//! in its error body), (3) transport errors (URL userinfo — covered
//! exhaustively in nano-egress; a canary variant lives here), (4) tracing /
//! stderr / ACP serialization surfaces consume these same sanitized strings
//! (the ACP-frame canary lives in nano-cli's c8_provider_parity.rs).

use nano_egress::client::EgressClient;
use nano_egress::redact::{register_credential, sanitize_text};
use nano_model::anthropic_messages::AnthropicMessagesClient;
use nano_model::flux_completions::{FluxCompletionsClient, classify_status};
use nano_model::types::ModelRequest;

fn canary(tag: &str) -> String {
    format!("sk-C8CANARY-{tag}-{}", std::process::id())
}

fn assert_scrubbed(canary: &str, rendered: &str, surface: &str) {
    assert!(
        !rendered.contains(canary),
        "canary leaked through {surface}: {rendered}"
    );
}

/// (2) A provider echoing the presented credential in its error body must
/// not leak it through ModelError Display/Debug. `classify_status` is the
/// single classification path BOTH wire clients share (flux_common) — the
/// completions and anthropic adapters cannot diverge on redaction.
#[test]
fn error_body_echo_is_scrubbed_for_every_status_class() {
    let key = canary("body");
    register_credential(&key);
    for (status, body) in [
        (
            401u16,
            format!(r#"{{"error":{{"type":"auth_error","message":"bad key {key}"}}}}"#),
        ),
        (
            500,
            format!(r#"{{"error":{{"message":"upstream saw {key}"}}}}"#),
        ),
        (
            402,
            format!(r#"{{"error":{{"message":"quota for {key}"}}}}"#),
        ),
        (
            413,
            format!(r#"{{"error":{{"message":"context_window_exceeded {key}"}}}}"#),
        ),
    ] {
        let err = classify_status(status, body);
        assert_scrubbed(&key, &err.to_string(), "Display");
        assert_scrubbed(&key, &format!("{err:?}"), "Debug");
    }
}

/// (1) Request-construction/policy errors: a driver pointed at an
/// off-policy host fails BEFORE socket activity, and the error carries
/// host + path digest only — the presented credential never appears. One
/// case per wire client (openai-compat completions, anthropic messages).
#[tokio::test]
async fn construction_errors_carry_no_credential_per_wire_client() {
    let key = canary("construction");
    register_credential(&key);
    let request = ModelRequest {
        model: "canary-model".into(),
        stream: false,
        ..ModelRequest::default()
    };
    // The flux_only policy allows neither api.openai.com nor
    // api.anthropic.com — both constructions deny.
    let completions = FluxCompletionsClient::new(EgressClient::flux())
        .with_base_url("https://api.openai.com/v1")
        .with_api_path("/chat/completions");
    let err = completions
        .complete(&request, &key)
        .await
        .expect_err("off-policy host must fail");
    assert_scrubbed(&key, &err.to_string(), "completions construction Display");
    assert_scrubbed(&key, &format!("{err:?}"), "completions construction Debug");

    let anthropic = AnthropicMessagesClient::new(EgressClient::flux())
        .with_base_url("https://api.anthropic.com")
        .with_api_path("/v1/messages");
    let err = anthropic
        .complete(&request, &key)
        .await
        .expect_err("off-policy host must fail");
    assert_scrubbed(&key, &err.to_string(), "anthropic construction Display");
    assert_scrubbed(&key, &format!("{err:?}"), "anthropic construction Debug");
}

/// (3) Transport errors: a credential in URL userinfo is stripped by the
/// egress sanitizer (fail-closed to host + digest when redaction can't be
/// proven complete).
#[test]
fn transport_error_userinfo_canary_is_scrubbed() {
    let key = canary("transport");
    register_credential(&key);
    let dirty = format!("error sending request for url (https://user:{key}@example.com/v1)");
    let clean = sanitize_text(&dirty);
    assert_scrubbed(&key, &clean, "transport text");
}

/// The boundary itself: unregistered text passes through; registered
/// values are replaced wherever they appear, repeatedly.
#[test]
fn sanitizer_is_total_for_registered_values() {
    let key = canary("total");
    register_credential(&key);
    let dirty = format!("{key} mid {key} end {key}");
    let clean = sanitize_text(&dirty);
    assert_scrubbed(&key, &clean, "repeated occurrence");
    assert_eq!(sanitize_text("ordinary error text"), "ordinary error text");
}
