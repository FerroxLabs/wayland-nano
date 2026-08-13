//! Flux Chat Completions adapter — the v1 primary wire.
//!
//! Endpoint: POST {base}/v1/chat/completions (default base
//! https://api.fluxrouter.ai). Evidence-based quirks encoded from
//! shared/fixtures/flux/FINDINGS.md:
//! - responses may carry `reasoning_content` (stream deltas and message-level)
//!   — mapped to ModelEvent::ReasoningDelta, never mixed into text;
//! - usage carries cost_usd + cache fields — mapped to Usage;
//! - alias routing differs per surface — this adapter only speaks Completions.

use crate::flux_common::{classify_transport, read_error_body, sse_integrity_error};
use crate::params::Surface;
use crate::retry::RetryConfig;
use crate::sse::SseParser;
use crate::types::{
    CallHooks, ContentBlock, Message, ModelError, ModelEvent, ModelObservation, ModelRequest,
    ModelResponse, Role, ToolCall, ToolDefinition, Usage,
};
use nano_egress::client::EgressClient;

// Shared wire plumbing (single classification path across all three Flux
// surfaces) — re-exported so existing callers keep their import paths.
pub use crate::flux_common::{FLUX_BASE, classify_status};

pub const COMPLETIONS_PATH: &str = "/v1/chat/completions";

/// OpenAI chat-completions client — the provider-neutral implementation
/// (C8 §4). Flux is the `base_url = FLUX_BASE` special case; every other
/// OpenAI-compatible catalog provider differs only in base_url/api_path
/// (both from the vendored table, the sole endpoint authority) and the
/// bearer presented per call.
#[derive(Debug)]
pub struct OpenAiCompletionsClient {
    egress: EgressClient,
    base_url: String,
    api_path: String,
    retry: RetryConfig,
}

/// Back-compat alias (codex NB / claude NB: no rename churn). Existing call
/// sites and tests keep the Flux name; it IS the OpenAI-compat client.
pub type FluxCompletionsClient = OpenAiCompletionsClient;

impl OpenAiCompletionsClient {
    pub fn new(egress: EgressClient) -> Self {
        Self {
            egress,
            base_url: FLUX_BASE.to_string(),
            api_path: COMPLETIONS_PATH.to_string(),
            retry: RetryConfig::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Override the completions path (vendored-catalog `api_path`; e.g.
    /// `/chat/completions` when the base already carries a version segment).
    pub fn with_api_path(mut self, api_path: impl Into<String>) -> Self {
        self.api_path = api_path.into();
        self
    }

    /// Test seam: a custom retry configuration (the production default is
    /// the Q2-bounded policy).
    pub fn with_retry_config(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    pub(crate) fn endpoint(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.api_path)
    }

    /// P1 grounding seam (flux_grounding.rs): the isolated web_search
    /// completion sends through the SAME egress client (the Flux base is
    /// already allowlisted — zero new egress surface).
    pub(crate) fn egress(&self) -> &EgressClient {
        &self.egress
    }

    /// P1 grounding seam: the Q2-bounded retry policy (retry sleeps are
    /// cancel-selectable via `run_with_retries`).
    pub(crate) fn retry_config(&self) -> &RetryConfig {
        &self.retry
    }

    pub async fn complete(
        &self,
        request: &ModelRequest,
        api_key: &str,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_with_hooks(request, api_key, &CallHooks::none())
            .await
    }

    /// The hooked entry point (C9): cancel-selectable reconnect sleeps,
    /// typed observations (reconnect state, inert-param notices, rate-limit
    /// snapshots), the Q3 param ladder, and structured-output validation.
    /// Known-unsupported params are rejected BEFORE any network I/O.
    pub async fn complete_with_hooks(
        &self,
        request: &ModelRequest,
        api_key: &str,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        if request.has_tool_result_images() {
            return Err(ModelError::UnsupportedParam {
                param: "tool_result_images".into(),
                surface: "flux-completions".into(),
                message: "image-bearing tool results require anthropic-messages in RC2".into(),
            });
        }
        // Rung-3 rejections (and every param decision) happen HERE — before
        // a single packet: plan first, network second.
        let mut body = build_request_body(request);
        let notices = crate::params::apply_params(Surface::Completions, &mut body, request)?;
        for notice in notices {
            hooks.observe(notice);
        }
        let mut response = crate::retry::run_with_retries(&self.retry, hooks, || async {
            self.attempt(&body, request.stream, api_key, hooks).await
        })
        .await?;
        if request.output_schema.is_some() {
            // One canonical extracted value, validated before the caller
            // sees the response; a failure carries the literal re-ask text.
            crate::structured::extract_and_validate(Surface::Completions, request, &mut response)?;
        }
        Ok(response)
    }

    /// One wire attempt. Re-invoked byte-identically by the retry driver;
    /// never mutates anything outside itself.
    async fn attempt(
        &self,
        body: &serde_json::Value,
        stream: bool,
        api_key: &str,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let builder = self
            .egress
            .request(reqwest::Method::POST, &self.endpoint())?
            .bearer_auth(api_key)
            .json(body);

        let response = builder
            .send()
            .await
            .map_err(|e| classify_transport(e, false))?;
        // Rate-limit headers are observability, never control flow: a
        // parsed snapshot rides the observation channel, a malformed or
        // absent set yields None (UIs render "unknown").
        if let Some(snapshot) =
            crate::rate_limits::parse_headers(response.headers(), crate::rate_limits::now_ms())
        {
            hooks.observe(ModelObservation::RateLimit(snapshot));
        }
        let status = response.status().as_u16();
        if status != 200 {
            return Err(classify_status(status, read_error_body(response).await));
        }
        // The response has started (status 200): a body-read failure is
        // MidStream by construction.
        let text = response
            .text()
            .await
            .map_err(|e| classify_transport(e, true))?;
        if stream {
            parse_sse_completion_stream(&text)
        } else {
            parse_completion_body(&text)
        }
    }
}

pub fn build_request_body(request: &ModelRequest) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = request.messages.iter().map(message_to_wire).collect();
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "stream": request.stream,
    });
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }
    if !request.tools.is_empty() {
        body["tools"] =
            serde_json::json!(request.tools.iter().map(tool_to_wire).collect::<Vec<_>>());
        body["tool_choice"] = serde_json::json!("auto");
    }
    body
}

fn message_to_wire(message: &Message) -> serde_json::Value {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    // P2a §2.2: when any Image block is present the user message's `content`
    // becomes an ARRAY of parts — text parts as {"type":"text","text":…}
    // (NOT the bare-string fast path, so part order is preserved) and each
    // image as a data-URL image_url part (codex models.rs:722 shape). No
    // `detail` field in P2a (Q7 RULED).
    let has_images = message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    let content: serde_json::Value = if has_images {
        let parts: Vec<serde_json::Value> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => {
                    Some(serde_json::json!({"type": "text", "text": text}))
                }
                ContentBlock::Image { mime, data } => Some(serde_json::json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{mime};base64,{data}")},
                })),
                _ => None,
            })
            .collect();
        serde_json::json!(parts)
    } else {
        let texts: Vec<String> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        serde_json::json!(texts.join("\n"))
    };
    let mut wire = serde_json::json!({
        "role": role,
        "content": content,
    });
    let tool_calls: Vec<serde_json::Value> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": input.to_string(),
                }
            })),
            _ => None,
        })
        .collect();
    if !tool_calls.is_empty() {
        wire["tool_calls"] = serde_json::json!(tool_calls);
    }
    if let Some(ContentBlock::ToolResult {
        tool_use_id,
        content,
        ..
    }) = message
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::ToolResult { .. }))
    {
        wire["tool_call_id"] = serde_json::json!(tool_use_id);
        wire["content"] = serde_json::json!(content);
    }
    wire
}

fn tool_to_wire(tool: &ToolDefinition) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

/// Parse a non-streaming chat.completion body into a ModelResponse.
pub fn parse_completion_body(text: &str) -> Result<ModelResponse, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ModelError::Protocol(format!("bad json: {e}")))?;
    let mut events = Vec::new();

    if let Some(choices) = value.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            if let Some(message) = choice.get("message") {
                if let Some(reasoning) = message.get("reasoning_content").and_then(|r| r.as_str()) {
                    if !reasoning.is_empty() {
                        events.push(ModelEvent::ReasoningDelta(reasoning.to_string()));
                    }
                }
                if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
                    if !content.is_empty() {
                        events.push(ModelEvent::TextDelta(content.to_string()));
                    }
                }
                if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
                    for call in tool_calls {
                        if let Some(tool_call) = parse_tool_call(call) {
                            events.push(ModelEvent::ToolCallComplete(tool_call));
                        }
                    }
                }
            }
            if let Some(finish) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                events.push(ModelEvent::Done {
                    stop_reason: finish.to_string(),
                });
            }
        }
    }

    let usage = parse_usage(value.get("usage"));
    events.push(ModelEvent::Usage(usage.clone()));

    let stop_reason = events
        .iter()
        .find_map(|e| match e {
            ModelEvent::Done { stop_reason } => Some(stop_reason.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "stop".to_string());

    Ok(ModelResponse {
        events,
        usage,
        stop_reason,
    })
}

fn parse_tool_call(call: &serde_json::Value) -> Option<ToolCall> {
    let function = call.get("function")?;
    let arguments_str = function.get("arguments")?.as_str().unwrap_or("{}");
    Some(ToolCall {
        id: call.get("id")?.as_str()?.to_string(),
        name: function.get("name")?.as_str()?.to_string(),
        arguments: serde_json::from_str(arguments_str).unwrap_or(serde_json::Value::Null),
    })
}

pub(crate) fn parse_usage(usage: Option<&serde_json::Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    let get_u64 = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    Usage {
        input_tokens: get_u64("prompt_tokens"),
        output_tokens: get_u64("completion_tokens"),
        cached_input_tokens: usage
            .get("prompt_cache_hit_tokens")
            .and_then(|v| v.as_u64()),
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|r| r.as_u64()),
        cost_usd: usage.get("cost_usd").and_then(|c| c.as_f64()),
    }
}

/// Parse a recorded SSE stream of chat.completion.chunk frames.
pub fn parse_sse_completion_stream(text: &str) -> Result<ModelResponse, ModelError> {
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    let mut tool_acc: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut usage = Usage::default();
    let mut stop_reason = "stop".to_string();

    let frames = {
        let mut all = parser.feed(text).map_err(sse_integrity_error)?;
        all.extend(parser.finish().map_err(sse_integrity_error)?);
        all
    };

    for frame in frames {
        let data = frame.data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        if let Some(choice) = chunk
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta.get("reasoning_content").and_then(|r| r.as_str()) {
                    if !reasoning.is_empty() {
                        events.push(ModelEvent::ReasoningDelta(reasoning.to_string()));
                    }
                }
                if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                    if !content.is_empty() {
                        events.push(ModelEvent::TextDelta(content.to_string()));
                    }
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                    for call in tool_calls {
                        let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                        let entry = tool_acc.entry(index).or_default();
                        if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                            entry.0 = id.to_string();
                        }
                        if let Some(function) = call.get("function") {
                            if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                                entry.1 = name.to_string();
                            }
                            if let Some(args) = function.get("arguments").and_then(|a| a.as_str()) {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
            }
            if let Some(finish) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                stop_reason = finish.to_string();
                events.push(ModelEvent::Done {
                    stop_reason: stop_reason.clone(),
                });
            }
        }
        if chunk.get("usage").is_some() {
            usage = parse_usage(chunk.get("usage"));
        }
    }

    for (_index, (id, name, args)) in tool_acc {
        if !name.is_empty() {
            events.push(ModelEvent::ToolCallComplete(ToolCall {
                id,
                name,
                arguments: serde_json::from_str(&args).unwrap_or(serde_json::Value::Null),
            }));
        }
    }
    events.push(ModelEvent::Usage(usage.clone()));

    Ok(ModelResponse {
        events,
        usage,
        stop_reason,
    })
}
