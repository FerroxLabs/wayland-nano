//! Flux Responses adapter — POST {base}/v1/responses.
//!
//! Evidence: fixtures-flux/responses/ (batch 1) and streaming/*_rs_sse.txt
//! (batch 2). Recorded quirks encoded here:
//! - the budget field is `max_output_tokens` (rename of the neutral
//!   `ModelRequest.max_tokens` — each adapter owns its translation);
//! - a bare-string `input` is what the probe sent; this adapter emits the
//!   string form for a lone user text message and the input-item array
//!   otherwise (both accepted by the wire);
//! - `Role::System` / `request.system` map to top-level `instructions`;
//! - tools use the flattened Responses shape `{type:"function", name,
//!   description, parameters}` (no nested `function` object);
//! - empty visible output is NOT an error: the batch-1 fixture shows a
//!   response whose entire budget was eaten by a reasoning item
//!   (`status:"incomplete"`, `incomplete_details.reason:"max_output_tokens"`,
//!   no message item). The stop reason passes through verbatim, matching the
//!   completions adapter's `finish_reason` pass-through;
//! - auth is `Authorization: Bearer` (FINDINGS quirk #6).
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

pub const RESPONSES_PATH: &str = "/v1/responses";

#[derive(Debug)]
pub struct FluxResponsesClient {
    egress: EgressClient,
    base_url: String,
    retry: RetryConfig,
}

impl FluxResponsesClient {
    pub fn new(egress: EgressClient) -> Self {
        Self {
            egress,
            base_url: FLUX_BASE.to_string(),
            retry: RetryConfig::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Test seam: a custom retry configuration (the production default is
    /// the Q2-bounded policy).
    pub fn with_retry_config(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    fn endpoint(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), RESPONSES_PATH)
    }

    pub async fn complete(
        &self,
        request: &ModelRequest,
        api_key: &str,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_with_hooks(request, api_key, &CallHooks::none())
            .await
    }

    /// The hooked entry point (C9) — see FluxCompletionsClient. Responses-
    /// surface stream events can carry rate-limit payloads mid-stream; those
    /// ride the SAME observation channel as header-derived snapshots.
    pub async fn complete_with_hooks(
        &self,
        request: &ModelRequest,
        api_key: &str,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        if request.has_tool_result_images() {
            return Err(ModelError::UnsupportedParam {
                param: "tool_result_images".into(),
                surface: "flux-responses".into(),
                message: "image-bearing tool results require anthropic-messages in RC2".into(),
            });
        }
        let mut body = build_request_body(request);
        let notices = crate::params::apply_params(Surface::Responses, &mut body, request)?;
        for notice in notices {
            hooks.observe(notice);
        }
        let mut response = crate::retry::run_with_retries(&self.retry, hooks, || async {
            self.attempt(&body, request.stream, api_key, hooks).await
        })
        .await?;
        if request.output_schema.is_some() {
            crate::structured::extract_and_validate(Surface::Responses, request, &mut response)?;
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
            .request(reqwest::Method::POST, &self.endpoint())?
            .bearer_auth(api_key)
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
            parse_sse_responses_stream_observed(&text, &mut |snapshot| {
                hooks.observe(ModelObservation::RateLimit(snapshot));
            })
        } else {
            parse_response_body(&text)
        }
    }
}

pub fn build_request_body(request: &ModelRequest) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        "stream": request.stream,
    });
    if let Some(max_tokens) = request.max_tokens {
        body["max_output_tokens"] = serde_json::json!(max_tokens);
    }

    // System context maps to top-level `instructions` on this wire.
    let mut instructions: Vec<&str> = Vec::new();
    if let Some(system) = &request.system {
        instructions.push(system);
    }
    for message in &request.messages {
        if message.role == Role::System {
            instructions.extend(message.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            }));
        }
    }
    if !instructions.is_empty() {
        body["instructions"] = serde_json::json!(instructions.join("\n"));
    }

    let conversation: Vec<&Message> = request
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .collect();
    body["input"] = input_to_wire(&conversation);

    if !request.tools.is_empty() {
        body["tools"] =
            serde_json::json!(request.tools.iter().map(tool_to_wire).collect::<Vec<_>>());
        body["tool_choice"] = serde_json::json!("auto");
    }
    body
}

/// The batch-1 fixture sends `input` as a plain string; keep that form for a
/// lone user text message, otherwise emit the Responses input-item array.
fn input_to_wire(messages: &[&Message]) -> serde_json::Value {
    let lone_user_text = messages.len() == 1
        && messages[0].role == Role::User
        && messages[0]
            .content
            .iter()
            .all(|b| matches!(b, ContentBlock::Text { .. }));
    if lone_user_text {
        let text: Vec<&str> = messages[0]
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        return serde_json::json!(text.join("\n"));
    }
    serde_json::json!(
        messages
            .iter()
            .flat_map(|m| message_to_items(m))
            .collect::<Vec<_>>()
    )
}

fn message_to_items(message: &Message) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    let (role, part_type) = match message.role {
        Role::Assistant => ("assistant", "output_text"),
        _ => ("user", "input_text"),
    };
    // P2a §2.2: an image-bearing message emits ONE input item whose content
    // is the ordered part array — input_text parts for text, the
    // Responses-API input_image part (data: URL) for each image — so block
    // order survives onto the wire. The all-Text fast path in input_to_wire
    // already branches on block type, so text-only messages keep their
    // historical shape.
    let has_images = message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    if has_images {
        let parts: Vec<serde_json::Value> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => {
                    Some(serde_json::json!({"type": part_type, "text": text}))
                }
                ContentBlock::Image { mime, data } => Some(serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("data:{mime};base64,{data}"),
                })),
                _ => None,
            })
            .collect();
        items.push(serde_json::json!({
            "role": role,
            "content": parts,
        }));
        return items;
    }
    let texts: Vec<&str> = message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if !texts.is_empty() {
        items.push(serde_json::json!({
            "role": role,
            "content": [{"type": part_type, "text": texts.join("\n")}],
        }));
    }
    for block in &message.content {
        match block {
            ContentBlock::ToolUse { id, name, input } => items.push(serde_json::json!({
                "type": "function_call",
                "call_id": id,
                "name": name,
                "arguments": input.to_string(),
            })),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => items.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": tool_use_id,
                "output": content,
            })),
            ContentBlock::Text { .. } => {}
            // Image-bearing messages returned above; an Image can never
            // reach this loop (P2a: images only in user messages).
            ContentBlock::Image { .. } => {}
        }
    }
    items
}

fn tool_to_wire(tool: &ToolDefinition) -> serde_json::Value {
    // Flattened Responses shape — no nested `function` object.
    serde_json::json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

/// Parse a non-streaming Responses object into a ModelResponse.
pub fn parse_response_body(text: &str) -> Result<ModelResponse, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ModelError::Protocol(format!("bad json: {e}")))?;
    let (events, usage, stop_reason) = parse_response_object(&value);
    Ok(ModelResponse {
        events,
        usage,
        stop_reason,
    })
}

/// Walk a Responses object (`output[]` + terminal status + usage). Shared by
/// the non-streaming parse and the `response.completed` stream terminal.
fn parse_response_object(value: &serde_json::Value) -> (Vec<ModelEvent>, Usage, String) {
    let mut events = Vec::new();
    if let Some(output) = value.get("output").and_then(|o| o.as_array()) {
        for item in output {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("reasoning") => {
                    if let Some(summary) = item.get("summary").and_then(|s| s.as_array()) {
                        for part in summary {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                if !text.is_empty() {
                                    events.push(ModelEvent::ReasoningDelta(text.to_string()));
                                }
                            }
                        }
                    }
                }
                Some("message") => {
                    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                        for part in content {
                            if part.get("type").and_then(|t| t.as_str()) == Some("output_text")
                                && let Some(text) = part.get("text").and_then(|t| t.as_str())
                                && !text.is_empty()
                            {
                                events.push(ModelEvent::TextDelta(text.to_string()));
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if !name.is_empty() {
                        let arguments = item
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(serde_json::Value::Null);
                        events.push(ModelEvent::ToolCallComplete(ToolCall {
                            id: item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string(),
                            name: name.to_string(),
                            arguments,
                        }));
                    }
                }
                _ => {}
            }
        }
    }
    // Stop reason passes through verbatim: incomplete_details.reason when the
    // run was cut short (e.g. "max_output_tokens"), else the status string.
    let stop_reason = value
        .get("incomplete_details")
        .and_then(|d| d.get("reason"))
        .and_then(|r| r.as_str())
        .or_else(|| value.get("status").and_then(|s| s.as_str()))
        .unwrap_or("completed")
        .to_string();
    events.push(ModelEvent::Done {
        stop_reason: stop_reason.clone(),
    });
    let usage = parse_usage(value.get("usage"));
    events.push(ModelEvent::Usage(usage.clone()));
    (events, usage, stop_reason)
}

fn parse_usage(usage: Option<&serde_json::Value>) -> Usage {
    let Some(usage) = usage else {
        return Usage::default();
    };
    let get_u64 = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    Usage {
        input_tokens: get_u64("input_tokens"),
        output_tokens: get_u64("output_tokens"),
        cached_input_tokens: usage
            .get("input_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|c| c.as_u64()),
        reasoning_tokens: usage
            .get("output_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|r| r.as_u64()),
        // No cost field on this wire (completions-only per FINDINGS).
        cost_usd: None,
    }
}

/// Parse a recorded SSE stream of Responses lifecycle events.
pub fn parse_sse_responses_stream(text: &str) -> Result<ModelResponse, ModelError> {
    parse_sse_responses_stream_observed(text, &mut |_| {})
}

/// The observed variant (C9): rate-limit stream events
/// (`response.rate_limits` frames, or any frame carrying a `rate_limits`
/// payload) are parsed mid-stream and handed to the observer — the
/// dedicated typed observation channel, never a ModelEvent.
pub fn parse_sse_responses_stream_observed(
    text: &str,
    observer: &mut dyn FnMut(crate::rate_limits::RateLimitSnapshot),
) -> Result<ModelResponse, ModelError> {
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    // output_index → (call_id, name, accumulated arguments)
    let mut tool_acc: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut usage = Usage::default();
    let mut stop_reason = "completed".to_string();

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
            "response.reasoning_summary_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(|d| d.as_str())
                    && !delta.is_empty()
                {
                    events.push(ModelEvent::ReasoningDelta(delta.to_string()));
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.get("delta").and_then(|d| d.as_str())
                    && !delta.is_empty()
                {
                    events.push(ModelEvent::TextDelta(delta.to_string()));
                }
            }
            "response.output_item.added" => {
                // Lifecycle frame: only used to register function_call
                // identity (call_id/name) before its argument deltas arrive.
                let item = event.get("item");
                if item.and_then(|i| i.get("type")).and_then(|t| t.as_str())
                    == Some("function_call")
                {
                    let index = event
                        .get("output_index")
                        .and_then(|i| i.as_u64())
                        .unwrap_or(0);
                    let entry = tool_acc.entry(index).or_default();
                    if let Some(id) = item
                        .and_then(|i| i.get("call_id").or_else(|| i.get("id")))
                        .and_then(|i| i.as_str())
                    {
                        entry.0 = id.to_string();
                    }
                    if let Some(name) = item.and_then(|i| i.get("name")).and_then(|n| n.as_str()) {
                        entry.1 = name.to_string();
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let index = event
                    .get("output_index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(0);
                if let Some(delta) = event.get("delta").and_then(|d| d.as_str()) {
                    tool_acc.entry(index).or_default().2.push_str(delta);
                }
            }
            "response.output_item.done" => {
                // Terminal duplicate of the assembled function_call: use it
                // only to backfill arguments when no delta frames were seen.
                let item = event.get("item");
                if item.and_then(|i| i.get("type")).and_then(|t| t.as_str())
                    == Some("function_call")
                {
                    let index = event
                        .get("output_index")
                        .and_then(|i| i.as_u64())
                        .unwrap_or(0);
                    let entry = tool_acc.entry(index).or_default();
                    if entry.2.is_empty()
                        && let Some(args) = item
                            .and_then(|i| i.get("arguments"))
                            .and_then(|a| a.as_str())
                    {
                        entry.2 = args.to_string();
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                // Terminal frame: carries the full final object incl. usage.
                // `output[]` duplicates already-emitted deltas, so only the
                // stop reason and usage are taken from it.
                if let Some(response) = event.get("response") {
                    stop_reason = response
                        .get("incomplete_details")
                        .and_then(|d| d.get("reason"))
                        .and_then(|r| r.as_str())
                        .or_else(|| response.get("status").and_then(|s| s.as_str()))
                        .unwrap_or("completed")
                        .to_string();
                    usage = parse_usage(response.get("usage"));
                    events.push(ModelEvent::Done {
                        stop_reason: stop_reason.clone(),
                    });
                }
            }
            // response.created / in_progress / *.done lifecycle frames carry
            // duplicates of already-emitted deltas — ignored by design,
            // EXCEPT frames carrying rate-limit payloads (C9): the dedicated
            // `response.rate_limits` frame or a piggybacked payload both
            // parse mid-stream into the observation channel.
            other => {
                if (other == "response.rate_limits" || event.get("rate_limits").is_some())
                    && let Some(snapshot) =
                        crate::rate_limits::parse_stream_event(&event, crate::rate_limits::now_ms())
                {
                    observer(snapshot);
                }
            }
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
