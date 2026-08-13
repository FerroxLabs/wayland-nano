//! Flux Anthropic Messages adapter — COMPAT only, not the production wire.
//!
//! Endpoint: POST {base}/anthropic/v1/messages (+ /count_tokens). Evidence:
//! fixtures-flux/anthropic-messages/, tool-calls/, thinking/, cache/,
//! streaming/*_am_sse.txt, anthropic-count-tokens/.
//!
//! Per FINDINGS batch-2 WIRE-2 (`shared/fixtures/flux/FINDINGS.md:73-87`)
//! Chat Completions is the single production wire; this adapter exists for
//! compatibility and must never become the default route. The WIRE-2 failure
//! is server-side: `thinking` and `cache_control` are serialized faithfully
//! here (the adapter is an honest passthrough) but live Flux silently drops
//! them — proven on a routed `claude-sonnet-5`, so it is not an alias
//! artifact. Pinned-inert tests in `fixture_tests.rs` assert BOTH halves:
//! the request carries the blocks AND the recorded responses contain no
//! thinking block / no recorded cache write. Nothing is dropped silently.
//!
//! Recorded quirks encoded here:
//! - auth is the canonical Anthropic `x-api-key` header (FINDINGS quirk #6;
//!   Bearer is also accepted by the proxy — x-api-key is the canonical shape);
//! - `max_tokens` is REQUIRED on this surface (unlike completions, where omit
//!   is the fixture-proven contract): when the neutral request leaves it
//!   unset, [`DEFAULT_MAX_TOKENS`] is supplied and documented;
//! - tools use the Anthropic-native shape `{name, description, input_schema}`
//!   (no `type:"function"` wrapper);
//! - `tool_use.id` values are opaque — live Flux emits `call_*` (OpenAI-style,
//!   a translation-layer artifact per FINDINGS batch 2), never `toolu_*`.
//!   Ids round-trip verbatim; no prefix is assumed anywhere;
//! - an empty text block with `stop_reason:"max_tokens"` is a normal
//!   truncation outcome (batch-1 fixture), not an error — empty text blocks
//!   produce no `TextDelta`;
//! - `stop_reason` passes through verbatim (`end_turn`/`max_tokens`/
//!   `tool_use`), matching the completions adapter's pass-through.
//!
//! Error classification and transport mapping are shared with the other two
//! surfaces via `flux_common` (single path, never duplicated).

use crate::flux_common::{
    FLUX_BASE, classify_status, classify_transport, read_error_body, sse_integrity_error,
};
use crate::params::Surface;
use crate::retry::RetryConfig;
use crate::sse::SseParser;
use crate::types::{
    CallHooks, ContentBlock, Message, ModelError, ModelEvent, ModelObservation, ModelRequest,
    ModelResponse, Role, ToolCall, ToolDefinition, Usage,
};
use nano_egress::client::EgressClient;

pub const MESSAGES_PATH: &str = "/anthropic/v1/messages";
pub const COUNT_TOKENS_PATH: &str = "/anthropic/v1/messages/count_tokens";

/// Fallback budget when the neutral request leaves `max_tokens` unset:
/// required on this surface (real Anthropic rejects its absence; the
/// count_tokens endpoint is the only exception, per fixture).
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Metadata key for opt-in Anthropic thinking passthrough (COMPAT-inert on
/// live Flux — see module header).
pub const METADATA_THINKING: &str = "anthropic.thinking";
/// Metadata key for opt-in cache_control on the system block (COMPAT-inert
/// on live Flux — see module header).
pub const METADATA_CACHE_CONTROL: &str = "anthropic.cache_control";

#[derive(Debug)]
pub struct AnthropicMessagesClient {
    egress: EgressClient,
    base_url: String,
    messages_path: String,
    retry: RetryConfig,
}

impl AnthropicMessagesClient {
    pub fn new(egress: EgressClient) -> Self {
        Self {
            egress,
            base_url: FLUX_BASE.to_string(),
            messages_path: MESSAGES_PATH.to_string(),
            retry: RetryConfig::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Override the messages path (C8: the vendored catalog's `api_path`;
    /// e.g. `/v1/messages` for api.anthropic.com — the Flux-compat default
    /// is `/anthropic/v1/messages`). count_tokens rides the same prefix.
    pub fn with_api_path(mut self, api_path: impl Into<String>) -> Self {
        self.messages_path = api_path.into();
        self
    }

    /// Test seam: a custom retry configuration (the production default is
    /// the Q2-bounded policy).
    pub fn with_retry_config(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn messages_endpoint(&self) -> String {
        self.endpoint(&self.messages_path)
    }

    fn count_tokens_endpoint(&self) -> String {
        format!("{}/count_tokens", self.messages_endpoint())
    }

    pub async fn complete(
        &self,
        request: &ModelRequest,
        api_key: &str,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_with_hooks(request, api_key, &CallHooks::none())
            .await
    }

    /// The hooked entry point (C9) — see FluxCompletionsClient. Anthropic
    /// strict structured output maps to a forced tool and extracts the
    /// forced call's INPUT (never the final text); non-strict schema and
    /// verbosity are known-unsupported here and reject before network I/O.
    pub async fn complete_with_hooks(
        &self,
        request: &ModelRequest,
        api_key: &str,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let mut body = build_request_body(request);
        let notices = crate::params::apply_params(Surface::Anthropic, &mut body, request)?;
        for notice in notices {
            hooks.observe(notice);
        }
        let mut response = crate::retry::run_with_retries(&self.retry, hooks, || async {
            self.attempt(&body, request.stream, api_key, hooks).await
        })
        .await?;
        if request.output_schema.is_some() {
            crate::structured::extract_and_validate(Surface::Anthropic, request, &mut response)?;
        }
        Ok(response)
    }

    async fn attempt(
        &self,
        body: &serde_json::Value,
        stream: bool,
        api_key: &str,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let builder = self
            .egress
            .request(reqwest::Method::POST, &self.messages_endpoint())?
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(body);

        let response = builder
            .send()
            .await
            .map_err(|e| classify_transport(e, false))?;
        if let Some(snapshot) =
            crate::rate_limits::parse_headers(response.headers(), crate::rate_limits::now_ms())
        {
            hooks.observe(ModelObservation::RateLimit(snapshot));
        }
        let status = response.status().as_u16();
        if status != 200 {
            return Err(classify_status(status, read_error_body(response).await));
        }
        let text = response
            .text()
            .await
            .map_err(|e| classify_transport(e, true))?;
        if stream {
            parse_sse_message_stream(&text)
        } else {
            parse_message_body(&text)
        }
    }

    /// Count the input tokens a messages call would consume. Body is the
    /// messages-only shape — no `max_tokens` (per the count-tokens fixture;
    /// real Anthropic would reject it on /messages but Flux tolerates the
    /// omission here). Not part of `ModelDriver`; call it on the concrete
    /// client (e.g. for context-budget checks).
    pub async fn count_tokens(
        &self,
        request: &ModelRequest,
        api_key: &str,
    ) -> Result<u64, ModelError> {
        let body = build_count_tokens_body(request);
        crate::retry::run_with_retries(&self.retry, &CallHooks::none(), || async {
            let builder = self
                .egress
                .request(reqwest::Method::POST, &self.count_tokens_endpoint())?
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body);

            let response = builder
                .send()
                .await
                .map_err(|e| classify_transport(e, false))?;
            let status = response.status().as_u16();
            if status != 200 {
                return Err(classify_status(status, read_error_body(response).await));
            }
            let text = response
                .text()
                .await
                .map_err(|e| classify_transport(e, true))?;
            parse_count_tokens_body(&text)
        })
        .await
    }
}

pub fn build_request_body(request: &ModelRequest) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        // Required on this surface — see DEFAULT_MAX_TOKENS.
        "max_tokens": request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "stream": request.stream,
    });

    // System context: top-level `system`. Plain string normally; the block
    // array form only when cache_control is explicitly requested (the
    // recorded cache fixture's shape).
    let mut system_parts: Vec<&str> = Vec::new();
    if let Some(system) = &request.system {
        system_parts.push(system);
    }
    for message in &request.messages {
        if message.role == Role::System {
            system_parts.extend(message.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }));
        }
    }
    if !system_parts.is_empty() {
        let text = system_parts.join("\n");
        body["system"] = match request.metadata.get(METADATA_CACHE_CONTROL) {
            Some(cache_control) => serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control,
            }]),
            None => serde_json::json!(text),
        };
    }

    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(message_to_wire)
        .collect();
    body["messages"] = serde_json::json!(messages);

    if !request.tools.is_empty() {
        body["tools"] =
            serde_json::json!(request.tools.iter().map(tool_to_wire).collect::<Vec<_>>());
    }

    // Faithful passthrough of opt-in extras (inert on live Flux per WIRE-2 —
    // serialized honestly, dropped server-side, pinned by fixture tests).
    if let Some(thinking) = request.metadata.get(METADATA_THINKING) {
        body["thinking"] = thinking.clone();
    }
    body
}

/// Messages-only body for /count_tokens — no `max_tokens`, no `stream`
/// (pinned by the anthropic-count-tokens fixture).
pub fn build_count_tokens_body(request: &ModelRequest) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(message_to_wire)
        .collect();
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
    });
    if let Some(system) = &request.system {
        body["system"] = serde_json::json!(system);
    }
    body
}

fn message_to_wire(message: &Message) -> serde_json::Value {
    // Anthropic carries tool results as user-role messages.
    let role = match message.role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    let blocks: Vec<serde_json::Value> = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => serde_json::json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
                images,
            } => {
                let result_content = if images.is_empty() {
                    serde_json::json!(content)
                } else {
                    let mut parts = vec![serde_json::json!({"type": "text", "text": content})];
                    parts.extend(images.iter().map(|image| {
                        serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": image.mime,
                                "data": image.data,
                            },
                        })
                    }));
                    serde_json::Value::Array(parts)
                };
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": result_content,
                    "is_error": is_error,
                })
            }
            // P2a §2.2: the Anthropic-native base64 image source block.
            ContentBlock::Image { mime, data } => serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": mime, "data": data},
            }),
        })
        .collect();
    // The recorded requests use the plain-string content form for a lone
    // text block; keep it, array form otherwise.
    if blocks.len() == 1 && blocks[0].get("type").and_then(|t| t.as_str()) == Some("text") {
        return serde_json::json!({
            "role": role,
            "content": blocks[0]["text"],
        });
    }
    serde_json::json!({
        "role": role,
        "content": blocks,
    })
}

fn tool_to_wire(tool: &ToolDefinition) -> serde_json::Value {
    // Anthropic-native shape — no `type:"function"` wrapper.
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}

/// Parse a non-streaming Anthropic message body into a ModelResponse.
pub fn parse_message_body(text: &str) -> Result<ModelResponse, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ModelError::Protocol(format!("bad json: {e}")))?;
    let mut events = Vec::new();

    if let Some(content) = value.get("content").and_then(|c| c.as_array()) {
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                // Empty text blocks are a recorded truncation artifact
                // (batch-1: "" with stop_reason "max_tokens") — tolerated,
                // no empty TextDelta emitted.
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str())
                        && !text.is_empty()
                    {
                        events.push(ModelEvent::TextDelta(text.to_string()));
                    }
                }
                // Parsed even though live Flux never sends one (WIRE-2) —
                // real-Anthropic compat.
                Some("thinking") => {
                    if let Some(thinking) = block.get("thinking").and_then(|t| t.as_str())
                        && !thinking.is_empty()
                    {
                        events.push(ModelEvent::ReasoningDelta(thinking.to_string()));
                    }
                }
                Some("tool_use") => {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if !name.is_empty() {
                        // The id is opaque (recorded artifact: call_*, never
                        // assume toolu_*) and round-trips verbatim.
                        events.push(ModelEvent::ToolCallComplete(ToolCall {
                            id: block
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string(),
                            name: name.to_string(),
                            arguments: block
                                .get("input")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        }));
                    }
                }
                _ => {}
            }
        }
    }

    let stop_reason = value
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("end_turn")
        .to_string();
    events.push(ModelEvent::Done {
        stop_reason: stop_reason.clone(),
    });
    let usage = parse_usage(value.get("usage"));
    events.push(ModelEvent::Usage(usage.clone()));

    // P5 §6: the provider-reported model id from the terminal frame —
    // actual-leaf evidence for routing metering, never a client prediction.
    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .map(str::to_string);

    Ok(ModelResponse {
        events,
        usage,
        stop_reason,
        model,
    })
}

/// Parse the count_tokens response (`{"input_tokens":N}` — bare, per fixture).
pub fn parse_count_tokens_body(text: &str) -> Result<u64, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ModelError::Protocol(format!("bad json: {e}")))?;
    value
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ModelError::Protocol("count_tokens body missing input_tokens".into()))
}

/// Usage has a single cached slot; the Anthropic surface reports cache
/// creation and read separately, so they are summed here (the write half is
/// always 0 on live Flux — WIRE-2 — so in practice this is the read count).
fn parse_usage(usage: Option<&serde_json::Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    let get_u64 = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    Usage {
        input_tokens: get_u64("input_tokens"),
        output_tokens: get_u64("output_tokens"),
        cached_input_tokens: Some(
            get_u64("cache_creation_input_tokens") + get_u64("cache_read_input_tokens"),
        ),
        reasoning_tokens: None,
        cost_usd: None,
    }
}

/// Parse a recorded SSE stream of the Anthropic message lifecycle
/// (message_start → content_block_* → message_delta → message_stop; no
/// [DONE] sentinel on this surface).
pub fn parse_sse_message_stream(text: &str) -> Result<ModelResponse, ModelError> {
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    // content-block index → (id, name, accumulated partial_json)
    let mut block_acc: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cached_tokens = 0u64;
    let mut stop_reason = "end_turn".to_string();
    // P5 §6: the provider-reported model id (the message_start shell carries
    // it) — actual-leaf evidence for routing metering.
    let mut model: Option<String> = None;

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
        let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        match event.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message_start" => {
                // Message shell: carries the input token count up front.
                if let Some(shell_model) = event
                    .get("message")
                    .and_then(|m| m.get("model"))
                    .and_then(|m| m.as_str())
                {
                    model = Some(shell_model.to_string());
                }
                if let Some(usage) = event.get("message").and_then(|m| m.get("usage")) {
                    input_tokens = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    cached_tokens = cached_tokens.max(
                        usage
                            .get("cache_creation_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                            + usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                    );
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                if let Some(block) = event.get("content_block") {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let entry = block_acc.entry(index).or_default();
                        if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                            entry.0 = id.to_string();
                        }
                        if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                            entry.1 = name.to_string();
                        }
                    }
                }
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                if let Some(delta) = event.get("delta") {
                    match delta.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(|t| t.as_str())
                                && !text.is_empty()
                            {
                                events.push(ModelEvent::TextDelta(text.to_string()));
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(thinking) = delta.get("thinking").and_then(|t| t.as_str())
                                && !thinking.is_empty()
                            {
                                events.push(ModelEvent::ReasoningDelta(thinking.to_string()));
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(partial) =
                                delta.get("partial_json").and_then(|p| p.as_str())
                            {
                                block_acc.entry(index).or_default().2.push_str(partial);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                if let Some((id, name, args)) = block_acc.remove(&index)
                    && !name.is_empty()
                {
                    events.push(ModelEvent::ToolCallComplete(ToolCall {
                        id,
                        name,
                        arguments: serde_json::from_str(&args).unwrap_or(serde_json::Value::Null),
                    }));
                }
            }
            "message_delta" => {
                if let Some(stop) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|s| s.as_str())
                {
                    stop_reason = stop.to_string();
                    events.push(ModelEvent::Done {
                        stop_reason: stop_reason.clone(),
                    });
                }
                if let Some(usage) = event.get("usage") {
                    output_tokens = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(output_tokens);
                    cached_tokens = cached_tokens.max(
                        usage
                            .get("cache_creation_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                            + usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                    );
                }
            }
            // message_stop and ping frames carry no payload.
            _ => {}
        }
    }

    let usage = Usage {
        input_tokens,
        output_tokens,
        cached_input_tokens: Some(cached_tokens),
        reasoning_tokens: None,
        cost_usd: None,
    };
    events.push(ModelEvent::Usage(usage.clone()));

    Ok(ModelResponse {
        events,
        usage,
        stop_reason,
        model,
    })
}
