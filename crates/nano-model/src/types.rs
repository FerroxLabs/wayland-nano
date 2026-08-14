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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageData>,
    },
    /// P2a §2.1 (donor: wayland-core `wcore-types/src/message.rs:41-55`,
    /// verbatim variant shape). INTAKE ONLY in P2a: `Image` blocks appear
    /// only in user messages; tool-result images are P2b's type-widening
    /// decision.
    Image {
        /// Sniffed MIME (never the sender's claim), from the closed wire set.
        mime: String,
        /// Base64 WITHOUT a data-URI prefix; codecs add their native wrapping.
        data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub mime: String,
    pub data: String,
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

    /// P2a §2.1: a user message from ordered content blocks (text + image
    /// interleavings preserved exactly — the block list is the machine
    /// contract; no string parsing anywhere).
    pub fn user_blocks(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: blocks,
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
                images: Vec::new(),
            }],
        }
    }

    pub fn tool_result_with_images(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
        images: Vec<ImageData>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error,
                images,
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

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub tools: Vec<ToolDefinition>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
    /// Cross-surface reasoning effort (C9). Per-(surface, model) capability
    /// ladder: verified → mapped onto the wire; unverified → omitted with a
    /// typed notice; known-unsupported → rejected before any network I/O.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Cross-surface output verbosity (C9), same ladder.
    pub verbosity: Option<Verbosity>,
    /// Structured output schema (C9): one canonical extracted JSON value,
    /// validated identically on every surface before the caller sees it.
    pub output_schema: Option<serde_json::Value>,
    /// Codex parity: strict mode is the default. On the Anthropic surface a
    /// non-strict schema request is known-unsupported (schema-in-prompt is
    /// NEVER structured output) and rejected before network I/O.
    pub output_schema_strict: bool,
    /// Provider-specific extras (e.g. flux tier hints) — namespaced by the
    /// adapter, never first-class fields.
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl Default for ModelRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: Vec::new(),
            system: None,
            tools: Vec::new(),
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            verbosity: None,
            output_schema: None,
            output_schema_strict: true,
            metadata: BTreeMap::new(),
        }
    }
}

impl ModelRequest {
    pub fn has_tool_result_images(&self) -> bool {
        self.messages.iter().flat_map(|message| &message.content).any(
            |block| matches!(block, ContentBlock::ToolResult { images, .. } if !images.is_empty()),
        )
    }
}

/// Cross-surface reasoning effort levels (C9). Codex's Ultra/Max tiers have
/// no Flux analogue and are deliberately not invented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }

    /// Strict parser for config channels: an unknown level is a typed config
    /// error at the call site, never a silent clamp.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "low" => Some(ReasoningEffort::Low),
            "medium" => Some(ReasoningEffort::Medium),
            "high" => Some(ReasoningEffort::High),
            _ => None,
        }
    }
}

/// Cross-surface verbosity levels (C9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Low,
    Medium,
    High,
}

impl Verbosity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verbosity::Low => "low",
            Verbosity::Medium => "medium",
            Verbosity::High => "high",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "low" => Some(Verbosity::Low),
            "medium" => Some(Verbosity::Medium),
            "high" => Some(Verbosity::High),
            _ => None,
        }
    }
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
    /// P5 §6: the provider-reported model id from the successful terminal
    /// completion frame (the response body's `model` field), when supplied
    /// by the wire. Evidence only — alias-valued or absent means "no
    /// trustworthy leaf identity"; it never retroactively satisfies a
    /// pre-dispatch capability gate.
    pub model: Option<String>,
}

/// WHERE a transport failure happened (C9). Retry classification consumes
/// this typed provenance ONLY — never string/error-chain inspection. The
/// slow reconnect class (`Connect`/`Tls`/`BeforeFirstByte`) applies only
/// when no response byte was observed for the request; `MidStream` is the
/// conservative class every adapter falls to when it cannot prove a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportPhase {
    /// TCP connect refused/reset/timeout. reqwest folds TLS handshake
    /// failure into its connect class, so adapters on reqwest report it
    /// here too (same retry class — no behavioral divergence).
    Connect,
    /// TLS handshake failure, when an adapter can prove it. reqwest cannot
    /// (its public taxonomy reports handshake failure as connect), so this
    /// variant is reserved for adapters with a typed TLS error channel.
    Tls,
    /// Request sent, connection lost before any response byte. Residual
    /// duplicate-execution risk (the server may have received the request)
    /// is bounded by the phase itself; Flux documents no idempotency key,
    /// so no request-identity header is sent (documented per surface).
    BeforeFirstByte,
    /// Failure after response bytes/events started flowing. Never
    /// reclassified into the reconnect class.
    MidStream,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// `status` is the typed HTTP-status provenance (C9): the 401 recovery
    /// seam retries only `status == Some(401)`; 403 and non-HTTP auth
    /// failures never retry. The embedded message may carry the provider's
    /// hashed key digest (never key material — see flux_common).
    #[error("auth failed (http_status={status:?}): {message}")]
    Auth {
        message: String,
        status: Option<u16>,
    },
    #[error("rate limited (retry after {retry_after_ms:?}ms)")]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("context overflow: {0}")]
    ContextOverflow(String),
    #[error("entitlement required: {0}")]
    Entitlement(String),
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },
    /// F-18: the provider answered HTTP 404 — the addressed model id is
    /// retired/unknown at the endpoint (the advertised snapshot is stale).
    /// A distinct typed class so kind-keyed fallback/retirement logic
    /// (NanoErrorKind::ModelNotFound on the wire, RoutingFailureClass::
    /// ModelNotFound in the routing journal) cannot miss it — pre-fix it
    /// surfaced as the undifferentiated model_server_4xx bucket.
    #[error("model not found (http_status={status}): {message}")]
    ModelNotFound { status: u16, message: String },
    /// The provider rejected the request FORMAT (F-P5-1): the live Flux
    /// edge answers a malformed request body with HTTP 5xx whose body
    /// carries `error.type == "invalid_request_error"`. Terminal for
    /// routing and never retried — another candidate would reject the
    /// identical bytes. Distinct from `Server` so the 5xx status signal
    /// cannot cascade it.
    #[error("invalid request (http_status={status}): {message}")]
    InvalidRequest { status: u16, message: String },
    #[error("transport/{phase:?}: {message}")]
    Transport {
        phase: TransportPhase,
        message: String,
    },
    #[error("protocol: {0}")]
    Protocol(String),
    /// Structured-output validation failure (C9): carries the LITERAL
    /// feedback text the model sees on the one allowed re-ask, so the
    /// journaled Op::SchemaReask is byte-faithful regardless of template
    /// wording changes across versions. Distinct from Protocol so the turn
    /// loop's re-ask eligibility is type-level, never string matching.
    #[error("structured output rejected: {0}")]
    OutputSchema(String),
    /// A requested parameter is known-unsupported on this (surface, model)
    /// (C9 ladder rung 3): rejected deterministically at request-build,
    /// BEFORE any network I/O. `message` is actionable — it names the
    /// setting to clear.
    #[error("unsupported parameter {param} on {surface}: {message}")]
    UnsupportedParam {
        param: String,
        surface: String,
        message: String,
    },
    #[error("cancelled")]
    Cancelled,
    #[error("egress: {0}")]
    Egress(#[from] nano_egress::client::EgressError),
}

/// A typed observation emitted by a model client OUTSIDE the response /
/// control flow (C9): reconnect state for UI banners, loud notices for
/// inert-on-this-surface params, and rate-limit snapshots. UIs render the
/// typed fields — they never parse strings.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelObservation {
    /// A connection-failure (slow class) retry is about to sleep.
    Reconnecting {
        attempt: u32,
        next_delay_ms: u64,
        deadline_remaining_ms: u64,
    },
    /// A requested param was omitted from the wire because it is unverified
    /// (or mapped-but-recorded-inert) on this surface — never a silent drop.
    ParamInert {
        param: String,
        surface: String,
        detail: String,
    },
    /// A parsed rate-limit snapshot (headers at completion, or stream
    /// events on the Responses surface). Observability only — Retry-After
    /// remains the sole timing authority.
    RateLimit(crate::rate_limits::RateLimitSnapshot),
    /// P1 §4.1: the session meter crossed 80% of its effective token limit
    /// — a typed `BudgetWarn` notice `{limit, observed, pct_used}`.
    /// Latest-wins observability, fires once per crossing.
    BudgetWarn {
        limit: u64,
        observed: u64,
        pct_used: u64,
    },
    /// P1 §4.2: a request's `max_tokens` was clamped to the reserved output
    /// allowance — the clamp is logged (typed), never silent.
    BudgetClamp { requested: u64, granted: u64 },
}

/// Per-call hooks threaded from the turn engine into the model client (C9):
/// the session cancel flag (reconnect sleeps are cancel-selectable waits —
/// cancel preempts sleeps) and the typed observation sink.
#[derive(Default)]
pub struct CallHooks<'a> {
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
    pub observer: Option<&'a (dyn Fn(ModelObservation) + Send + Sync)>,
}

impl std::fmt::Debug for CallHooks<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallHooks")
            .field("cancel", &self.cancel.is_some())
            .field("observer", &self.observer.is_some())
            .finish()
    }
}

impl CallHooks<'_> {
    /// No hooks: the pre-C9 behavior (no cancel-selectable sleeps, no
    /// observation sink).
    pub fn none() -> Self {
        Self::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
    }

    pub fn observe(&self, observation: ModelObservation) {
        if let Some(observer) = self.observer {
            observer(observation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2a §2.1: the Image variant's wire/journal serde shape is pinned
    /// (tagged `image`, snake_case fields — the wcore shape).
    #[test]
    fn image_block_serde_round_trip() {
        let block = ContentBlock::Image {
            mime: "image/png".into(),
            data: "aGVsbG8".into(),
        };
        let json = serde_json::to_value(&block).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"type": "image", "mime": "image/png", "data": "aGVsbG8"})
        );
        let back: ContentBlock = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, block);
    }

    #[test]
    fn user_blocks_preserves_order_and_multiplicity() {
        let message = Message::user_blocks(vec![
            ContentBlock::Text { text: "a".into() },
            ContentBlock::Image {
                mime: "image/jpeg".into(),
                data: "eA".into(),
            },
            ContentBlock::Image {
                mime: "image/jpeg".into(),
                data: "eA".into(),
            },
            ContentBlock::Text { text: "b".into() },
        ]);
        assert_eq!(message.role, Role::User);
        assert_eq!(message.content.len(), 4);
        assert!(matches!(message.content[1], ContentBlock::Image { .. }));
        assert!(matches!(message.content[2], ContentBlock::Image { .. }));
    }
}
