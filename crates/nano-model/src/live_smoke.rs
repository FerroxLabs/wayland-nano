//! Live Flux smoke tests — gated on FLUX_TEST_KEY in the environment.
//! Ignored by default (paid network calls); run explicitly:
//!   FLUX_TEST_KEY=$(cat ../../.secrets/flux-test-key) cargo test -p nano-model -- --ignored
//! Results are recorded as live evidence. In the monorepo layout they land in
//! shared/fixtures/flux (the evidence store of record); standalone checkouts
//! write into the crate's vendored snapshot (fixtures-flux/) — copy new
//! recordings back to shared/fixtures/flux when running from a full checkout.

use crate::flux_completions::FluxCompletionsClient;
use crate::types::{Message, ModelEvent, ModelRequest};
use nano_egress::client::EgressClient;

fn key() -> Option<String> {
    std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

fn record(name: &str, content: &str) {
    // Prefer the monorepo evidence store when present; otherwise the vendored
    // snapshot (standalone checkout / CI). See module header.
    let shared = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/fixtures/flux/client-smoke"
    );
    let vendored = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures-flux/client-smoke");
    let dir = if std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/fixtures/flux"
    ))
    .exists()
    {
        shared
    } else {
        vendored
    };
    std::fs::create_dir_all(dir).expect("fixture dir");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(format!("{dir}/{ts}_{name}"), content).expect("write fixture");
}

#[tokio::test]
async fn live_complete_non_streaming() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live smoke");
        return;
    };
    let client = FluxCompletionsClient::new(EgressClient::flux());
    let request = ModelRequest {
        model: "flux-fast".into(),
        messages: vec![Message::user("Reply with exactly the word: ok")],
        max_tokens: Some(512),
        stream: false,
        ..Default::default()
    };
    let response = client
        .complete(&request, &key)
        .await
        .expect("live non-streaming call");

    let text: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.to_lowercase().contains("ok"),
        "expected ok in text: {text}"
    );
    assert!(response.usage.output_tokens > 0);
    record("non_streaming.json", &format!("{:#?}", response.events));
}

#[tokio::test]
async fn live_complete_streaming() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live smoke");
        return;
    };
    let client = FluxCompletionsClient::new(EgressClient::flux());
    let request = ModelRequest {
        model: "flux-fast".into(),
        messages: vec![Message::user("Count from 1 to 3, one number per line.")],
        max_tokens: Some(512),
        stream: true,
        ..Default::default()
    };
    let response = client
        .complete(&request, &key)
        .await
        .expect("live streaming call");

    let text: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(text.contains('1'), "stream must contain 1: {text}");
    assert!(text.contains('3'), "stream must contain 3: {text}");
    record("streaming.json", &format!("{:#?}", response.events));
}

#[tokio::test]
async fn live_egress_denies_non_flux_host() {
    // Policy proof with zero network: a request to a non-Flux host must be
    // denied before any socket activity (construction fails).
    let egress = EgressClient::flux();
    let result = egress.request(reqwest::Method::GET, "https://api.openai.com/v1/models");
    assert!(
        result.is_err(),
        "non-Flux host must be denied at construction"
    );
}

#[tokio::test]
async fn live_responses_complete() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live smoke");
        return;
    };
    let client = crate::flux_responses::FluxResponsesClient::new(EgressClient::flux());
    let request = ModelRequest {
        model: "flux-fast".into(),
        messages: vec![Message::user("Reply with exactly the word: ok")],
        max_tokens: Some(512),
        stream: false,
        ..Default::default()
    };
    let response = client
        .complete(&request, &key)
        .await
        .expect("live responses call");

    let text: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.to_lowercase().contains("ok"),
        "expected ok in text: {text}"
    );
    assert!(response.usage.output_tokens > 0);
    record("responses.json", &format!("{:#?}", response.events));
}

#[tokio::test]
async fn live_anthropic_complete() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live smoke");
        return;
    };
    let client = crate::anthropic_messages::AnthropicMessagesClient::new(EgressClient::flux());
    let request = ModelRequest {
        model: "flux-auto".into(),
        messages: vec![Message::user("Reply with exactly the word: ok")],
        max_tokens: Some(512),
        stream: false,
        ..Default::default()
    };
    let response = client
        .complete(&request, &key)
        .await
        .expect("live anthropic call");

    let text: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.to_lowercase().contains("ok"),
        "expected ok in text: {text}"
    );
    assert!(response.usage.output_tokens > 0);
    record("anthropic.json", &format!("{:#?}", response.events));
}

#[tokio::test]
async fn live_anthropic_count_tokens() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live smoke");
        return;
    };
    let client = crate::anthropic_messages::AnthropicMessagesClient::new(EgressClient::flux());
    let request = ModelRequest {
        model: "flux-auto".into(),
        messages: vec![Message::user("Reply with exactly the word: ok")],
        ..Default::default()
    };
    let count = client
        .count_tokens(&request, &key)
        .await
        .expect("live count_tokens call");
    assert!(count > 0, "count_tokens must return a positive count");
    record("count_tokens.json", &format!("input_tokens={count}"));
}

// ── C9 Q3 capability-ladder probes ──────────────────────────────────────
// Each probe posts a hand-built body carrying ONE candidate param and
// records a SANITIZED verdict (status + structural facts only — never raw
// bodies, never the key). A probe verdict upgrades a ladder rung in
// params.rs; until then the param stays omitted-with-notice.

/// POST a raw JSON body to the Completions endpoint; returns (status,
/// body). The egress client keeps the Flux-only policy boundary.
async fn probe_post(body: serde_json::Value, key: &str) -> (u16, String) {
    let egress = EgressClient::flux();
    let response = egress
        .request(
            reqwest::Method::POST,
            &format!(
                "{}{}",
                crate::flux_common::FLUX_BASE,
                crate::flux_completions::COMPLETIONS_PATH
            ),
        )
        .expect("flux host allowed")
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .expect("probe send");
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    (status, text)
}

fn probe_body(extra: serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": "flux-reasoning",
        "messages": [{"role": "user", "content": "What is 17*23? Answer with the number only."}],
        "max_tokens": 512,
        "stream": false
    });
    body.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    body
}

#[tokio::test]
async fn live_probe_reasoning_effort_param() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live probe");
        return;
    };
    let (status, text) = probe_post(
        probe_body(serde_json::json!({"reasoning_effort": "low"})),
        &key,
    )
    .await;
    let parsed = serde_json::from_str::<serde_json::Value>(&text).ok();
    let has_choices = parsed.as_ref().and_then(|v| v.get("choices")).is_some();
    let reasoning_tokens = parsed
        .as_ref()
        .and_then(|v| v.pointer("/usage/completion_tokens_details/reasoning_tokens"))
        .and_then(|v| v.as_u64());
    record(
        "probe_reasoning_effort.json",
        &format!("status={status} has_choices={has_choices} reasoning_tokens={reasoning_tokens:?}"),
    );
    eprintln!(
        "probe reasoning_effort: status={status} has_choices={has_choices} reasoning_tokens={reasoning_tokens:?}"
    );
}

#[tokio::test]
async fn live_probe_verbosity_param() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live probe");
        return;
    };
    let (status, text) =
        probe_post(probe_body(serde_json::json!({"verbosity": "low"})), &key).await;
    let has_choices = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("choices").cloned())
        .is_some();
    record(
        "probe_verbosity.json",
        &format!("status={status} has_choices={has_choices}"),
    );
    eprintln!("probe verbosity: status={status} has_choices={has_choices}");
}

#[tokio::test]
async fn live_probe_response_format_json_schema() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live probe");
        return;
    };
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    });
    let (status, text) = probe_post(
        probe_body(serde_json::json!({
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "probe", "schema": schema, "strict": true}
            }
        })),
        &key,
    )
    .await;
    let parsed = serde_json::from_str::<serde_json::Value>(&text).ok();
    let content = parsed
        .as_ref()
        .and_then(|v| v.pointer("/choices/0/message/content"))
        .and_then(|c| c.as_str())
        .map(str::to_string);
    let content_is_json = content
        .as_deref()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok());
    let schema_valid = content_is_json
        .as_ref()
        .map(|v| jsonschema::validate(&schema, v).is_ok());
    record(
        "probe_response_format.json",
        &format!(
            "status={status} has_content={} content_parses_json={} schema_valid={schema_valid:?}",
            content.is_some(),
            content_is_json.is_some()
        ),
    );
    eprintln!(
        "probe response_format: status={status} content_parses_json={} schema_valid={schema_valid:?}",
        content_is_json.is_some()
    );
}

/// Follow-up probe: does the wire VALIDATE these params (bogus value →
/// 400) or silently accept anything? And what does the response_format 400
/// actually say? Verdict text is recorded sanitized.
#[tokio::test]
async fn live_probe_param_validation_sharpness() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live probe");
        return;
    };
    // Bogus effort value: validated params reject, ignored params accept.
    let (status_bogus_effort, _) = probe_post(
        probe_body(serde_json::json!({"reasoning_effort": "bogus-tier"})),
        &key,
    )
    .await;
    let (status_bogus_verbosity, _) = probe_post(
        probe_body(serde_json::json!({"verbosity": "bogus-level"})),
        &key,
    )
    .await;
    // The response_format 400 body: the error TYPE only (message could
    // embed request echoes — record the classified shape, not raw text).
    let (status_rf, body_rf) = probe_post(
        probe_body(serde_json::json!({
            "response_format": {"type": "json_schema", "json_schema": {"name": "probe", "schema": {"type": "object"}}}
        })),
        &key,
    )
    .await;
    let error_type = serde_json::from_str::<serde_json::Value>(&body_rf)
        .ok()
        .and_then(|v| {
            v.pointer("/error/type")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        });
    let mentions_response_format = body_rf.contains("response_format");
    record(
        "probe_validation_sharpness.json",
        &format!(
            "bogus_effort_status={status_bogus_effort} bogus_verbosity_status={status_bogus_verbosity} response_format_status={status_rf} error_type={error_type:?} mentions_response_format={mentions_response_format}"
        ),
    );
    eprintln!(
        "sharpness: bogus_effort={status_bogus_effort} bogus_verbosity={status_bogus_verbosity} response_format={status_rf} error_type={error_type:?} mentions_rf={mentions_response_format}"
    );
}
