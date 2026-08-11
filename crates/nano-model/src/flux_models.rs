//! Flux model catalog — GET {base}/v1/models (OpenAI list shape), cached
//! in-process, with the vendored fixture snapshot as the offline/unkeyed
//! fallback.
//!
//! The catalog backs the ACP `models` advertisement (session/new) and
//! session/set_model validation. Wire evidence: fixtures-flux/models/ (a
//! verbatim copy of shared/fixtures/flux), recorded from the live router.

use crate::types::ModelError;
use nano_egress::client::EgressClient;

pub const MODELS_PATH: &str = "/v1/models";

/// One catalog entry. `name` is the human label the ACP client renders;
/// Flux's /v1/models carries only `id`, so the id doubles as the label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluxModel {
    pub id: String,
    pub name: String,
}

/// GET /v1/models over the policy-gated egress client.
#[derive(Debug)]
pub struct FluxModelsClient {
    egress: EgressClient,
    base_url: String,
}

impl FluxModelsClient {
    pub fn new(egress: EgressClient) -> Self {
        Self {
            egress,
            base_url: crate::flux_completions::FLUX_BASE.to_string(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn endpoint(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), MODELS_PATH)
    }

    pub async fn list_models(&self, api_key: &str) -> Result<Vec<FluxModel>, ModelError> {
        let response = self
            .egress
            .request(reqwest::Method::GET, &self.endpoint())?
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(classify_transport)?;
        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            return Err(crate::flux_completions::classify_status(status, body));
        }
        let text = response.text().await.map_err(classify_transport)?;
        parse_models_body(&text)
    }
}

fn classify_transport(err: reqwest::Error) -> ModelError {
    // Single redaction path, same as the completions adapter.
    ModelError::Transport(nano_egress::client::sanitize_transport_error(&err))
}

/// Parse the OpenAI-style list body (`{"object":"list","data":[{"id":…}]}`).
/// Fail-closed: a body without a non-empty data array of id-carrying entries
/// is a protocol error, never an empty catalog.
pub fn parse_models_body(text: &str) -> Result<Vec<FluxModel>, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ModelError::Protocol(format!("bad json: {e}")))?;
    let data = value
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| ModelError::Protocol("models body has no data array".into()))?;
    let mut models = Vec::with_capacity(data.len());
    for entry in data {
        let Some(id) = entry.get("id").and_then(|i| i.as_str()) else {
            return Err(ModelError::Protocol(
                "models entry without a string id".into(),
            ));
        };
        models.push(FluxModel {
            id: id.to_string(),
            name: id.to_string(),
        });
    }
    if models.is_empty() {
        return Err(ModelError::Protocol("models catalog is empty".into()));
    }
    Ok(models)
}

/// The vendored fixture catalog (fixtures-flux/models/, newest snapshot).
/// Missing or malformed fixtures are an error, never a silent empty catalog
/// (the fixture IS the offline evidence; its absence must fail loudly).
pub fn fixture_catalog() -> Result<Vec<FluxModel>, ModelError> {
    let dir = format!("{}/fixtures-flux/models", env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| ModelError::Protocol(format!("fixture dir {dir}: {e}")))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with("_models.json"))
        .collect();
    files.sort();
    let path = files
        .pop()
        .ok_or_else(|| ModelError::Protocol(format!("no *_models.json fixture in {dir}")))?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| ModelError::Protocol(format!("fixture {} unreadable: {e}", path.display())))?;
    parse_models_body(&text)
}

/// The session-level catalog: fetched live once per process when keyed,
/// falling back to the vendored fixture when offline, unkeyed, or the live
/// fetch fails. The result is cached in-process — the picker and set_model
/// validation never re-hit the network.
#[derive(Debug)]
pub struct ModelCatalog {
    client: Option<FluxModelsClient>,
    api_key: Option<String>,
    cache: tokio::sync::OnceCell<Vec<FluxModel>>,
}

impl ModelCatalog {
    /// Live-capable catalog: with a key, the first `models()` call fetches
    /// GET /v1/models; without one it goes straight to the fixture.
    pub fn new(egress: EgressClient, api_key: Option<String>) -> Self {
        let client = api_key.as_ref().map(|_| FluxModelsClient::new(egress));
        Self {
            client,
            api_key,
            cache: tokio::sync::OnceCell::new(),
        }
    }

    /// Fixture-only catalog (tests, offline hosts).
    pub fn fixture_only() -> Self {
        Self {
            client: None,
            api_key: None,
            cache: tokio::sync::OnceCell::new(),
        }
    }

    pub async fn models(&self) -> Result<&[FluxModel], ModelError> {
        self.cache
            .get_or_try_init(|| async {
                match (&self.client, &self.api_key) {
                    (Some(client), Some(key)) => match client.list_models(key).await {
                        Ok(models) => Ok(models),
                        Err(_) => fixture_catalog(),
                    },
                    _ => fixture_catalog(),
                }
            })
            .await
            .map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_list_shape() {
        let body = r#"{"object":"list","data":[
            {"id":"flux-auto","object":"model","created":1677610602,"owned_by":"openai"},
            {"id":"flux-fast","object":"model"}
        ]}"#;
        let models = parse_models_body(body).expect("parse");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "flux-auto");
        assert_eq!(models[0].name, "flux-auto");
    }

    #[test]
    fn rejects_malformed_bodies_fail_closed() {
        assert!(parse_models_body("not json").is_err());
        assert!(parse_models_body(r#"{"object":"list"}"#).is_err());
        assert!(parse_models_body(r#"{"data":[]}"#).is_err());
        assert!(parse_models_body(r#"{"data":[{"object":"model"}]}"#).is_err());
    }

    #[test]
    fn fixture_catalog_loads_vendored_snapshot() {
        let models = fixture_catalog().expect("vendored fixture must load");
        assert!(
            models.len() >= 4,
            "the fixture snapshot carries the full router catalog: {}",
            models.len()
        );
        for expected in ["flux-auto", "flux-reasoning", "flux-standard", "flux-fast"] {
            assert!(
                models.iter().any(|m| m.id == expected),
                "fixture must carry the {expected} tier"
            );
        }
    }

    #[tokio::test]
    async fn fixture_only_catalog_resolves_and_caches() {
        let catalog = ModelCatalog::fixture_only();
        let first = catalog.models().await.expect("fixture catalog");
        let expected = fixture_catalog().expect("fixture");
        assert_eq!(first, expected.as_slice());
        // Second call comes from the in-process cache (same allocation).
        let second = catalog.models().await.expect("cached");
        assert!(std::ptr::eq(first.as_ptr(), second.as_ptr()));
    }
}
