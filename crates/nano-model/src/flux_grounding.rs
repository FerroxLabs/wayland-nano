//! Flux web_search grounding (P1 design §2.2, D2): the ISOLATED, no-tools
//! grounding completion that powers the `flux` search backend.
//!
//! The request is a STANDALONE completions call — never a turn, never a
//! tool call on the main conversation. The body carries ONLY: the pinned
//! model (`flux-fast`, Q3), ONE user message with the bounded query, the
//! live-proven grounding flag `tools: [{"type":"web_search"}]`, optional
//! domain filters, and the strict `max_tokens` output cap. NO system
//! prompt, NO conversation history, NO tool definitions, NO steer/plan/goal
//! context — recursion and context bleed are structurally impossible, and
//! the shape is fixture-pinned by the unit tests below.
//!
//! Cancellation is selectable IN-FLIGHT (r2 codex-F3): the HTTP send AND
//! the response-body read run under [`cancel_select`] (the `sleep_or_cancel`
//! precedent, retry.rs), and retry sleeps ride `run_with_retries`, which is
//! already cancel-selectable. Every surface terminates with typed
//! `ModelError::Cancelled`, promptly.
//!
//! Response contract (wcore §5.4 shape, `wcore-types/src/llm.rs`
//! `FluxSearchResult`): top-level `citations` (URL strings) and
//! `search_results` (source cards). A present-but-empty `search_results` is
//! a legitimate empty outcome; a MALFORMED or ABSENT `search_results` —
//! including a prose-only answer — is a typed parse error, never fabricated
//! hits.
//!
//! Provenance note: the request-side domain-filter field
//! (`search_domain_filter`, the Perplexity-Sonar spelling Flux routes to)
//! is the ASSUMED shape pending the §10.1 live leg; it is sent only when
//! the caller passes domains, so the no-filters body is unaffected.

use crate::flux_common::{classify_transport, read_error_body};
use crate::flux_completions::{OpenAiCompletionsClient, classify_status};
use crate::retry::run_with_retries;
use crate::types::{CallHooks, ModelError, Usage};

/// Q3 RULED: ALL searches pin `flux-fast` — never the session's tier alias
/// (cost containment: one constant, documented search profile).
pub const GROUNDING_MODEL: &str = "flux-fast";

/// The strict output cap default (config-overridable by the caller, design
/// §2.2); ADDITIONALLY clamped by the meter's atomic allowance reservation
/// (§4.2 — Lane B wires the reservation through the same meter handle).
pub const GROUNDING_DEFAULT_MAX_TOKENS: u32 = 1024;

/// One normalized search hit (design §2.1 shape; normalization of the
/// wire's `search_results[]` lives HERE, in nano-model).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub date: Option<String>,
}

/// The parsed grounding outcome: normalized hits, the citation URL set for
/// the render, and the round-trip's usage. `usage_reported = false` when
/// the wire carried no usage — the meter applies the §3.5 conservative
/// charge (never zero) for those.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundingOutcome {
    pub hits: Vec<SearchHit>,
    pub citations: Vec<String>,
    pub usage: Usage,
    pub usage_reported: bool,
}

/// The isolated grounding request body (fixture-pinned below). `domains`
/// is the optional domain filter; absent filters keep the field out of the
/// body entirely.
pub fn build_grounding_request_body(
    query: &str,
    domains: Option<&[String]>,
    max_tokens: u32,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": GROUNDING_MODEL,
        "messages": [{"role": "user", "content": query}],
        "stream": false,
        "tools": [{"type": "web_search"}],
        "max_tokens": max_tokens,
    });
    if let Some(domains) = domains
        && !domains.is_empty()
    {
        body["search_domain_filter"] = serde_json::json!(domains);
    }
    body
}

/// Run a fallible future to completion OR until the cancel flag fires —
/// the in-flight half of the `sleep_or_cancel` precedent (retry.rs:199):
/// the flag is polled in small chunks so a fired flag aborts the send /
/// body-read promptly instead of waiting on the socket.
pub async fn cancel_select<F: std::future::Future>(
    future: F,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<F::Output, ModelError> {
    const CHUNK: std::time::Duration = std::time::Duration::from_millis(50);
    let Some(flag) = cancel else {
        return Ok(future.await);
    };
    tokio::pin!(future);
    loop {
        if flag.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ModelError::Cancelled);
        }
        tokio::select! {
            output = &mut future => return Ok(output),
            () = tokio::time::sleep(CHUNK) => {}
        }
    }
}

/// One isolated grounding completion. Goes through the client's existing
/// egress client — the Flux base is already allowlisted, so this adds ZERO
/// new egress surface. Retry sleeps ride `run_with_retries`
/// (cancel-selectable); send and body-read ride [`cancel_select`].
pub async fn grounded_search(
    client: &OpenAiCompletionsClient,
    api_key: &str,
    query: &str,
    domains: Option<&[String]>,
    max_tokens: u32,
    hooks: &CallHooks<'_>,
) -> Result<GroundingOutcome, ModelError> {
    let body = build_grounding_request_body(query, domains, max_tokens);
    run_with_retries(client.retry_config(), hooks, || async {
        grounding_attempt(client, &body, api_key, hooks).await
    })
    .await
}

/// One wire attempt; re-invoked byte-identically by the retry driver.
async fn grounding_attempt(
    client: &OpenAiCompletionsClient,
    body: &serde_json::Value,
    api_key: &str,
    hooks: &CallHooks<'_>,
) -> Result<GroundingOutcome, ModelError> {
    if hooks.is_cancelled() {
        return Err(ModelError::Cancelled);
    }
    let builder = client
        .egress()
        .request(reqwest::Method::POST, &client.endpoint())?
        .bearer_auth(api_key)
        .json(body);
    let response = cancel_select(builder.send(), hooks.cancel)
        .await?
        .map_err(|e| classify_transport(e, false))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(classify_status(status, read_error_body(response).await));
    }
    // Status 200 observed: a body-read failure is MidStream by construction.
    let text = cancel_select(response.text(), hooks.cancel)
        .await?
        .map_err(|e| classify_transport(e, true))?;
    parse_grounding_body(&text)
}

/// Parse a non-streaming grounding response. `search_results` MUST be
/// present and an array (empty is a legitimate zero-hit outcome); anything
/// else — malformed JSON, a prose-only answer, source cards of the wrong
/// shape — is a typed protocol error, never fabricated hits (design §2.2).
pub fn parse_grounding_body(text: &str) -> Result<GroundingOutcome, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ModelError::Protocol(format!("bad json: {e}")))?;
    let usage = crate::flux_completions::parse_usage(value.get("usage"));
    let usage_reported = value.get("usage").is_some();
    let citations: Vec<String> = value
        .get("citations")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|u| u.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let results = value.get("search_results").ok_or_else(|| {
        ModelError::Protocol(
            "grounding response carries no search_results (prose-only answers are not search outcomes)".into(),
        )
    })?;
    let results = results
        .as_array()
        .ok_or_else(|| ModelError::Protocol("grounding search_results is not an array".into()))?;
    let mut hits = Vec::with_capacity(results.len());
    for card in results {
        hits.push(normalize_hit(card)?);
    }
    Ok(GroundingOutcome {
        hits,
        citations,
        usage,
        usage_reported,
    })
}

/// One source card → [`SearchHit`] (the wcore `FluxSearchResult` mapping:
/// title/url/snippet default to empty, `date`/`last_updated` are frequently
/// absent and stay None rather than failing the array).
fn normalize_hit(card: &serde_json::Value) -> Result<SearchHit, ModelError> {
    if !card.is_object() {
        return Err(ModelError::Protocol(
            "grounding search_results card is not an object".into(),
        ));
    }
    let str_field = |key: &str| {
        card.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let date = card
        .get("date")
        .and_then(|v| v.as_str())
        .or_else(|| card.get("last_updated").and_then(|v| v.as_str()))
        .map(str::to_string);
    Ok(SearchHit {
        title: str_field("title"),
        url: str_field("url"),
        snippet: str_field("snippet"),
        date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request-body fixture pin (design §8): pinned model, ONE user
    /// message with the bounded query, the grounding flag, the capped
    /// max_tokens — and NOTHING else: no system prompt, no conversation
    /// history, no function tool definitions.
    #[test]
    fn request_body_is_isolated_and_pinned() {
        let body = build_grounding_request_body("wayland nano", None, 512);
        assert_eq!(body["model"], "flux-fast");
        assert_eq!(body["stream"], false);
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["tools"], serde_json::json!([{"type": "web_search"}]));
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1, "exactly one message, never history");
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "wayland nano");
        let mut keys: Vec<&str> = body
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["max_tokens", "messages", "model", "stream", "tools"],
            "no system prompt, no tool_choice, no context fields, no filters without domains"
        );
    }

    #[test]
    fn request_body_carries_domain_filters_only_when_given() {
        let domains = vec!["example.com".to_string(), "docs.example.org".to_string()];
        let body = build_grounding_request_body("q", Some(&domains), 1024);
        assert_eq!(
            body["search_domain_filter"],
            serde_json::json!(["example.com", "docs.example.org"])
        );
        let empty: Vec<String> = vec![];
        let body = build_grounding_request_body("q", Some(&empty), 1024);
        assert!(body.get("search_domain_filter").is_none());
    }

    /// Normalization from the wcore §5.4 card shape: title/url/snippet +
    /// optional date, citations as URL strings, usage mapped.
    #[test]
    fn parses_search_results_and_citations() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "answer with [1]"}}],
            "citations": ["https://example.com/a", "https://example.org/b"],
            "search_results": [
                {"title": "A", "url": "https://example.com/a", "snippet": "alpha", "date": "2026-08-01"},
                {"title": "B", "url": "https://example.org/b", "snippet": "beta"}
            ],
            "usage": {"prompt_tokens": 11, "completion_tokens": 7}
        });
        let outcome = parse_grounding_body(&body.to_string()).expect("parse");
        assert_eq!(outcome.hits.len(), 2);
        assert_eq!(outcome.hits[0].title, "A");
        assert_eq!(outcome.hits[0].url, "https://example.com/a");
        assert_eq!(outcome.hits[0].snippet, "alpha");
        assert_eq!(outcome.hits[0].date.as_deref(), Some("2026-08-01"));
        assert_eq!(outcome.hits[1].date, None);
        assert_eq!(
            outcome.citations,
            ["https://example.com/a", "https://example.org/b"]
        );
        assert!(outcome.usage_reported);
        assert_eq!(outcome.usage.input_tokens, 11);
        assert_eq!(outcome.usage.output_tokens, 7);
    }

    /// A present-but-empty search_results is a legitimate zero-hit outcome
    /// (Ok, never a fall-through trigger — design §2.1).
    #[test]
    fn empty_results_is_ok_with_zero_hits() {
        let body = serde_json::json!({"choices": [], "search_results": [], "citations": []});
        let outcome = parse_grounding_body(&body.to_string()).expect("empty is Ok");
        assert!(outcome.hits.is_empty());
        assert!(
            !outcome.usage_reported,
            "no usage key ⇒ §3.5 conservative charge upstream"
        );
    }

    /// Malformed/absent search_results — including a prose-only answer — is
    /// a typed parse error, NEVER fabricated hits (design §2.2/§8).
    #[test]
    fn missing_or_malformed_results_are_typed_parse_errors() {
        let prose_only = serde_json::json!({
            "choices": [{"message": {"content": "I found nothing structured."}}]
        });
        let err = parse_grounding_body(&prose_only.to_string())
            .expect_err("prose without search_results is a parse error");
        assert!(matches!(err, ModelError::Protocol(_)));

        let wrong_shape = serde_json::json!({"search_results": {"title": "x"}});
        let err = parse_grounding_body(&wrong_shape.to_string()).expect_err("non-array");
        assert!(matches!(err, ModelError::Protocol(_)));

        let bad_card = serde_json::json!({"search_results": ["not-a-card"]});
        let err = parse_grounding_body(&bad_card.to_string()).expect_err("non-object card");
        assert!(matches!(err, ModelError::Protocol(_)));

        let err = parse_grounding_body("not json").expect_err("bad json");
        assert!(matches!(err, ModelError::Protocol(_)));
    }

    /// last_updated maps to date when `date` is absent (wcore shape).
    #[test]
    fn last_updated_is_the_date_fallback() {
        let body = serde_json::json!({
            "search_results": [{"title": "t", "url": "u", "snippet": "s", "last_updated": "2026-07-30"}]
        });
        let outcome = parse_grounding_body(&body.to_string()).expect("parse");
        assert_eq!(outcome.hits[0].date.as_deref(), Some("2026-07-30"));
    }

    /// In-flight cancellation: a fired flag aborts a pending future with
    /// typed Cancelled promptly (the cancel_select half of r2 codex-F3).
    #[tokio::test]
    async fn cancel_select_aborts_a_pending_future() {
        let flag = std::sync::atomic::AtomicBool::new(false);
        let pending = std::future::pending::<()>();
        let driver = cancel_select(pending, Some(&flag));
        tokio::pin!(driver);
        let canceller = async {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        };
        let (result, ()) = tokio::join!(driver, canceller);
        assert!(matches!(result, Err(ModelError::Cancelled)));
    }

    #[tokio::test]
    async fn cancel_select_passes_through_without_a_flag() {
        let ready = async { 42u32 };
        let out = cancel_select(ready, None).await.expect("no flag");
        assert_eq!(out, 42);
    }
}
