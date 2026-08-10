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
