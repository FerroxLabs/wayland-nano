//! Flux Chat Completions adapter — the v1 primary wire.
//!
//! Endpoint: POST {base}/v1/chat/completions (default base
//! https://api.fluxrouter.ai). Evidence-based quirks encoded from
//! shared/fixtures/flux/FINDINGS.md:
//! - responses may carry `reasoning_content` (stream deltas and message-level)
//!   — mapped to ModelEvent::ReasoningDelta, never mixed into text;
//! - usage carries cost_usd + cache fields — mapped to Usage;
//! - alias routing differs per surface — this adapter only speaks Completions.

use crate::retry::RetryPolicy;
use crate::sse::SseParser;
use crate::types::{
    ContentBlock, Message, ModelError, ModelEvent, ModelRequest, ModelResponse, Role, ToolCall,
    ToolDefinition, Usage,
};
use nano_egress::client::EgressClient;

pub const FLUX_BASE: &str = "https://api.fluxrouter.ai";
pub const COMPLETIONS_PATH: &str = "/v1/chat/completions";

#[derive(Debug)]
pub struct FluxCompletionsClient {
    egress: EgressClient,
    base_url: String,
    retry: RetryPolicy,
}

impl FluxCompletionsClient {
    pub fn new(egress: EgressClient) -> Self {
        Self {
            egress,
            base_url: FLUX_BASE.to_string(),
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    fn endpoint(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            COMPLETIONS_PATH
        )
    }

    pub async fn complete(
        &self,
        request: &ModelRequest,
        api_key: &str,
    ) -> Result<ModelResponse, ModelError> {
        let body = build_request_body(request);
        let mut attempt = 0;
        loop {
            let builder = self
                .egress
                .request(reqwest::Method::POST, &self.endpoint())?
                .bearer_auth(api_key)
                .json(&body);

            let outcome = async {
                let response = builder.send().await.map_err(classify_transport)?;
                let status = response.status().as_u16();
                if status != 200 {
                    return Err(classify_status(status, read_error_body(response).await));
                }
                let text = response.text().await.map_err(classify_transport)?;
                if request.stream {
                    parse_sse_completion_stream(&text)
                } else {
                    parse_completion_body(&text)
                }
            }
            .await;

            match outcome {
                Ok(events_response) => return Ok(events_response),
                Err(err) => match self.retry.decide(attempt, &err) {
                    crate::retry::RetryAction::Retry {
                        attempt: next,
                        delay_ms,
                    } => {
                        attempt = next;
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    crate::retry::RetryAction::GiveUp => return Err(err),
                },
            }
        }
    }
}

fn classify_transport(err: reqwest::Error) -> ModelError {
    // Delegate to nano-egress so transport errors never echo userinfo/query
    // (single redaction path, fail-closed).
    ModelError::Transport(nano_egress::client::sanitize_transport_error(&err))
}

async fn read_error_body(response: reqwest::Response) -> String {
    response.text().await.unwrap_or_default()
}

pub fn classify_status(status: u16, body: String) -> ModelError {
    let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
    let error = parsed.as_ref().and_then(|v| v.get("error"));
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let error_type = error.and_then(|e| e.get("type")).and_then(|t| t.as_str());
    match status {
        401 | 403 => ModelError::Auth(message),
        // Live wire (batch-3 badkey fixture): an invalid key arrives as
        // HTTP 500 with error.type=="auth_error", NOT 401. The embedded
        // message carries `key=<sha256-of-presented-key>` — a digest, not
        // the key itself, matching this crate's hashed-digest convention —
        // so carrying it in ModelError::Auth logs is acceptable.
        500 if error_type == Some("auth_error") => ModelError::Auth(message),
        402 => ModelError::Entitlement(message),
        // Kept for spec compliance; live Flux never sends 429 (burst load
        // saturates the edge with bare 503 nginx HTML, no Retry-After).
        429 => ModelError::RateLimited {
            retry_after_ms: None,
        },
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
    let texts: Vec<String> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let mut wire = serde_json::json!({
        "role": role,
        "content": texts.join("\n"),
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

fn parse_usage(usage: Option<&serde_json::Value>) -> Usage {
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

/// Map a parser cap violation to a protocol integrity error (fail-closed:
/// a hostile stream errors the completion, it is never truncated).
fn sse_integrity_error(err: crate::sse::SseError) -> ModelError {
    ModelError::Protocol(format!("sse stream rejected: {err}"))
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
