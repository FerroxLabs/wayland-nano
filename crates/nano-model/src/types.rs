//! Provider-neutral model types. Extensible metadata lives in maps; universal
//! types never carry Flux-specific fields.

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// The ONE synthesized-tool-result encoding (C1): journal-resume elision,
    /// the compaction repair pass, and repeat-protection skips all build their
    /// synthetic results through here so the Completions and Anthropic wire
    /// surfaces can never diverge in role/content encoding.
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    /// Provider-specific extras (e.g. flux tier hints) — namespaced by the
    /// adapter, never first-class fields.
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallComplete(ToolCall),
    Usage(Usage),
    Done { stop_reason: String },
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub events: Vec<ModelEvent>,
    pub usage: Usage,
    pub stop_reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("rate limited (retry after {retry_after_ms:?}ms)")]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("context overflow: {0}")]
    ContextOverflow(String),
    #[error("entitlement required: {0}")]
    Entitlement(String),
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("cancelled")]
    Cancelled,
    #[error("egress: {0}")]
    Egress(#[from] nano_egress::client::EgressError),
}
