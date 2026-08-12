//! C8 §2/§9 — vendored provider catalog: SHA-256 drift pin, golden-file pin
//! of the generated Rust table, and schema/semantic validation batteries.
//!
//! Refresh process (design §2): source = Desktop's
//! `providerCatalog.generated.json` (+ wcore providers.toml for the
//! engine-native endpoints); refresh = copy + update RECORDED_SHA256 +
//! regenerate the golden + review the endpoint diff, landed as its own PR.

use nano_model::catalog_schema::validate_catalog_json;
use nano_model::provider_catalog::{PROVIDERS, WireKind};

/// sha256 of data/providerCatalog.vendored.json. Any byte change to the
/// vendored file fails this test — endpoint changes are deliberate,
/// reviewed, and recorded here.
const RECORDED_SHA256: &str = "f0503b33deb370d53ad87747b1e3ea33d242fb5fc3610caa11fefc5294e5f5da";

const VENDORED: &[u8] = include_bytes!("../data/providerCatalog.vendored.json");

#[test]
fn vendored_catalog_sha256_matches_recorded_hash() {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(VENDORED);
    let digest = format!("{:x}", h.finalize());
    assert_eq!(
        digest, RECORDED_SHA256,
        "vendored provider catalog drifted — review the endpoint diff and update RECORDED_SHA256 deliberately"
    );
}

#[test]
fn generated_table_matches_golden_file() {
    let generated = include_str!(concat!(env!("OUT_DIR"), "/provider_catalog.rs"));
    let golden = include_str!("golden/provider_catalog.golden.rs");
    assert_eq!(
        generated, golden,
        "codegen output drifted from the golden file — an unchanged input hash with changed output means a codegen bug; regenerate deliberately"
    );
}

#[test]
fn vendored_catalog_passes_schema_validation() {
    let catalog = validate_catalog_json(std::str::from_utf8(VENDORED).expect("utf-8"))
        .expect("vendored catalog must validate");
    assert_eq!(catalog.providers.len(), PROVIDERS.len());
    for (raw, spec) in catalog.providers.iter().zip(PROVIDERS.iter()) {
        assert_eq!(raw.id, spec.id);
        assert_eq!(raw.base_url, spec.base_url);
        assert_eq!(raw.api_path, spec.api_path);
        assert_eq!(raw.env_var, spec.env_var);
        assert_eq!(raw.proven, spec.proven);
        let wire = match spec.wire {
            WireKind::OpenAiCompletions => "openai-completions",
            WireKind::AnthropicMessages => "anthropic-messages",
        };
        assert_eq!(raw.wire, wire);
    }
}

/// The v1 scope (Q3 ruling): flux-router + anthropic + the engine-native
/// OpenAI-compat set + google-gemini (carried but gated).
#[test]
fn v1_provider_scope_is_exact() {
    let ids: Vec<&str> = PROVIDERS.iter().map(|p| p.id).collect();
    assert_eq!(
        ids,
        [
            "flux-router",
            "anthropic",
            "openai",
            "openrouter",
            "groq",
            "mistral",
            "deepseek",
            "together",
            "fireworks",
            "perplexity",
            "cohere",
            "cerebras",
            "xai",
            "moonshot",
            "nvidia",
            "minimax",
            "google-gemini",
        ]
    );
}

#[test]
fn schema_rejects_missing_fields_no_invented_defaults() {
    // Missing api_path: a parse/validation error, never a default.
    let bad = r#"{"version":1,"providers":[{"id":"x","display_name":"X","base_url":"https://x.example","wire":"openai-completions","env_var":"X_API_KEY","proven":false}]}"#;
    assert!(validate_catalog_json(bad).is_err());
}

#[test]
fn schema_rejects_bad_wire_and_non_https_and_userinfo() {
    let base = |base_url: &str, wire: &str| {
        format!(
            r#"{{"version":1,"providers":[{{"id":"x","display_name":"X","base_url":"{base_url}","wire":"{wire}","api_path":"/v1/chat/completions","env_var":"X_API_KEY","proven":false}}]}}"#
        )
    };
    assert!(validate_catalog_json(&base("https://x.example", "grpc")).is_err());
    assert!(validate_catalog_json(&base("http://x.example", "openai-completions")).is_err());
    assert!(validate_catalog_json(&base("https://u:p@x.example", "openai-completions")).is_err());
    assert!(validate_catalog_json(&base("https://x.example?q=1", "openai-completions")).is_err());
    assert!(validate_catalog_json(&base("x.example", "openai-completions")).is_err());
}

#[test]
fn schema_rejects_duplicate_ids_and_bearer_env_collisions() {
    let dup = r#"{"version":1,"providers":[
        {"id":"x","display_name":"X","base_url":"https://x.example","wire":"openai-completions","api_path":"/v1/chat/completions","env_var":"X_API_KEY","proven":false},
        {"id":"x","display_name":"X2","base_url":"https://y.example","wire":"openai-completions","api_path":"/v1/chat/completions","env_var":"Y_API_KEY","proven":false}
    ]}"#;
    assert!(validate_catalog_json(dup).is_err());
    // "a-b" and "a_b" normalize to the same bearer env name — rejected.
    let collision = r#"{"version":1,"providers":[
        {"id":"a-b","display_name":"AB","base_url":"https://a.example","wire":"openai-completions","api_path":"/v1/chat/completions","env_var":"A_API_KEY","proven":false},
        {"id":"a_b","display_name":"AB2","base_url":"https://b.example","wire":"openai-completions","api_path":"/v1/chat/completions","env_var":"B_API_KEY","proven":false}
    ]}"#;
    // "a-b" and "a_b" would normalize to the same bearer env name; the
    // [a-z0-9-] id charset already rejects "a_b", so within the enforced
    // charset the collision guard is defense-in-depth (it fires the moment
    // the charset ever widens to admit other punctuation).
    assert!(validate_catalog_json(collision).is_err());
}

#[test]
fn schema_rejects_bad_env_var_and_api_path_and_version() {
    let mk = |env_var: &str, api_path: &str, version: u32| {
        format!(
            r#"{{"version":{version},"providers":[{{"id":"x","display_name":"X","base_url":"https://x.example","wire":"openai-completions","api_path":"{api_path}","env_var":"{env_var}","proven":false}}]}}"#
        )
    };
    assert!(validate_catalog_json(&mk("x_api_key", "/v1/chat/completions", 1)).is_err());
    assert!(validate_catalog_json(&mk("X_API_KEY", "v1/chat/completions", 1)).is_err());
    assert!(validate_catalog_json(&mk("X_API_KEY", "/v1/chat/completions", 2)).is_err());
    assert!(validate_catalog_json(&mk("X_API_KEY", "/v1/chat/completions", 1)).is_ok());
}
