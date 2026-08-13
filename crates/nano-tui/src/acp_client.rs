//! Hand-rolled ACP (JSON-RPC over stdio) client subset (design doc §2):
//! initialize, session/new, session/load, session/prompt, session/cancel,
//! session/set_model, session/set_mode (C2), session/update,
//! session/request_permission, and the `_wayland/session/list` Nano
//! extension.
//!
//! The `agent-client-protocol` SDK crate is explicitly rejected (a new
//! external prod dep); this subset is small and the wire contract is the
//! tested boundary (`nano-protocol/src/acp.rs` documents the shapes the
//! engine emits — recorded fixtures pin them, see tests/fixtures/).
//!
//! nano-tui links NO nano engine crates: this module plus serde_json is the
//! whole client layer.

use std::io::BufRead as _;
use std::io::Write as _;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use tokio::sync::mpsc;

use nano_session::NanoErrorKind;

pub const ACP_PROTOCOL_VERSION: u32 = 1;
pub const SESSION_LIST_METHOD: &str = "_wayland/session/list";
/// P4 §9: `/review` rides the wire (the TUI is a pure ACP client).
pub const SESSION_REVIEW_METHOD: &str = "_wayland/session/review";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionListStatus {
    Closed,
    Live,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListSummary {
    pub session_id: String,
    pub cwd: String,
    pub modified_ms: u64,
    pub size_bytes: u64,
    pub status: SessionListStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListResponse {
    pub sessions: Vec<SessionListSummary>,
    pub truncated: bool,
    pub live_status_caveat: String,
}

/// The typed payload inside `error.data.nanoError` / `_meta.nanoError`
/// (C7). An unrecognized (future) kind deserializes to
/// [`NanoErrorKind::Unknown`], which both UIs classify TERMINAL — an older
/// UI must never auto-retry a newer engine's error (design §2/D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NanoErrorPayload {
    pub kind: NanoErrorKind,
    pub retryable: bool,
}

/// Parse a `nanoError` payload object (`{"kind": ..., "retryable": ...}`).
/// Missing/malformed fields fail closed: no payload, or retryable false.
pub fn parse_nano_error(value: &Value) -> Option<NanoErrorPayload> {
    let kind = value
        .get("kind")
        .cloned()
        .and_then(|k| serde_json::from_value::<NanoErrorKind>(k).ok())?;
    let retryable = value
        .get("retryable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(NanoErrorPayload { kind, retryable })
}

/// A JSON-RPC error with its code and optional typed payload preserved
/// (C7 — the pre-C7 client kept only the message string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError {
    pub code: i64,
    pub message: String,
    pub nano: Option<NanoErrorPayload>,
}

/// A selectable option on a session/request_permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: String,
    pub name: String,
}

/// Parsed session/update notification payloads the TUI renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionUpdate {
    AgentChunk(String),
    UserChunk(String),
    ToolCall {
        call_id: String,
        title: String,
        status: String,
        raw_input: Value,
    },
    ToolCallUpdate {
        call_id: String,
        status: String,
        raw_output: String,
        /// A structured before/after diff content block (C10 §6), when the
        /// host emitted one for this call.
        diff: Option<DiffBlock>,
        /// C7: the typed failure payload (`_meta.nanoError`) on failed
        /// cards — drives the honest title+hint result line.
        nano_error: Option<NanoErrorPayload>,
    },
    /// Context-compaction lifecycle notice (C1 §7): begin/complete/cancel,
    /// rendered as a system note in the transcript.
    Compaction {
        status: String,
    },
    /// C9 §3.3: a queued steer was dropped on cancel/close — one notice per
    /// dropped steer, carrying OUR request id and the text digest.
    SteerDropped {
        request_id: String,
        text_digest: String,
    },
    /// C9 §2.2: the host is in a reconnect sleep (typed fields; render,
    /// never parse).
    Reconnecting {
        attempt: u64,
        next_delay_ms: u64,
        deadline_remaining_ms: u64,
    },
    /// C9 §4 (Q3): a requested param was omitted/inert on the surface.
    ParamInert {
        param: String,
        surface: String,
        detail: String,
    },
    /// C9 §5 (Q4): the latest coalesced rate-limit snapshot. Every field is
    /// optional — render "unknown" for absent fields, never a guess.
    RateLimit {
        requests_remaining: Option<u64>,
        requests_limit: Option<u64>,
        tokens_remaining: Option<u64>,
        tokens_limit: Option<u64>,
    },
    /// P1 §5: the session meter's status payload (drives the status-line
    /// budget slot). `priced: false` renders `unpriced`, never $0.000.
    Budget {
        session_tokens: u64,
        microcents: u64,
        priced: bool,
        limit: Option<u64>,
        observed: Option<u64>,
    },
    /// P1 §4.1: the typed 80% BudgetWarn notice `{limit, observed,
    /// pct_used}`.
    BudgetWarn {
        limit: u64,
        observed: u64,
        pct_used: u64,
    },
    /// P1 §4.2: the typed clamp notice (a request's max_tokens was clamped
    /// to the reserved allowance).
    BudgetClamp {
        requested: u64,
        granted: u64,
    },
    /// P4 §3.4/§9: the bounded review child's terminal card. `status` is
    /// "completed" | "failed" | "interrupted"; `verdict` is the reviewer's
    /// overall_correctness string (empty when interrupted); `error` carries
    /// the typed failure (wire kind + presentation) when the host typed one
    /// (e.g. `review_parse_failed`).
    ReviewResult {
        task_id: String,
        status: String,
        verdict: String,
        text: String,
        error: Option<(String, String)>,
    },
    /// Forward-additive: kinds v1 doesn't render. Tolerated, never panics
    /// (torn/unknown replay frames must not kill the TUI, design §8).
    Unknown(String),
}

/// The ACP-standard diff content block (C10 §6): one structured
/// old/new-text representation end-to-end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBlock {
    pub path: String,
    /// None = whole-file add.
    pub old_text: Option<String>,
    pub new_text: String,
}

/// A well-formed session/request_permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    pub id: u64,
    pub title: String,
    pub raw_input: Value,
    pub options: Vec<PermissionOption>,
}

/// Classification of one inbound JSON-RPC frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// A response to one of our requests (result XOR error).
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<WireError>,
    },
    Update(SessionUpdate),
    Permission(PermissionRequest),
    /// session/request_permission arrived malformed — the app auto-denies
    /// (fail-closed mirror of the engine's gate, design §4/§8).
    MalformedPermission {
        id: Option<u64>,
        reason: String,
    },
    /// A request we do not serve — answered method-not-found.
    UnknownRequest {
        id: Value,
        method: String,
    },
    /// A notification we do not consume (tolerated).
    UnknownNotification {
        method: String,
    },
    /// A frame that is none of the JSON-RPC shapes above.
    MalformedFrame(String),
}

/// Classify one parsed inbound frame. Pure — unit-tested hard (approval
/// spoofing cases land here).
pub fn classify(frame: &Value) -> Inbound {
    let method = frame.get("method").and_then(Value::as_str);
    let id = frame.get("id").filter(|i| !i.is_null());
    match (method, id) {
        (Some("session/request_permission"), Some(id)) => parse_permission(id.clone(), frame),
        (Some(method), Some(id)) => Inbound::UnknownRequest {
            id: id.clone(),
            method: method.to_string(),
        },
        (Some("session/update"), None) => match frame
            .get("params")
            .and_then(|p| p.get("update"))
            .map(parse_session_update)
        {
            Some(update) => Inbound::Update(update),
            None => Inbound::MalformedFrame("session/update without update object".into()),
        },
        (Some(method), None) => Inbound::UnknownNotification {
            method: method.to_string(),
        },
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                return Inbound::MalformedFrame("response id is not an integer".into());
            };
            if let Some(error) = frame.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let nano = error
                    .get("data")
                    .and_then(|d| d.get("nanoError"))
                    .and_then(parse_nano_error);
                Inbound::Response {
                    id,
                    result: None,
                    error: Some(WireError {
                        code,
                        message: message.to_string(),
                        nano,
                    }),
                }
            } else {
                Inbound::Response {
                    id,
                    result: frame.get("result").cloned(),
                    error: None,
                }
            }
        }
        (None, None) => Inbound::MalformedFrame("frame has neither method nor id".into()),
    }
}

fn parse_session_update(update: &Value) -> SessionUpdate {
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or("");
    let text_of = |u: &Value| {
        u.get("content")
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match kind {
        "agent_message_chunk" => SessionUpdate::AgentChunk(text_of(update)),
        "user_message_chunk" => SessionUpdate::UserChunk(text_of(update)),
        "tool_call" => SessionUpdate::ToolCall {
            call_id: update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            title: update
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string(),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("in_progress")
                .to_string(),
            raw_input: update.get("rawInput").cloned().unwrap_or(Value::Null),
        },
        "tool_call_update" => SessionUpdate::ToolCallUpdate {
            call_id: update
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            raw_output: update
                .get("rawOutput")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            diff: parse_diff_block(update),
            nano_error: update
                .get("_meta")
                .and_then(|m| m.get("nanoError"))
                .and_then(parse_nano_error),
        },
        "compaction" => SessionUpdate::Compaction {
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "steer_dropped" => SessionUpdate::SteerDropped {
            request_id: update
                .get("requestId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            text_digest: update
                .get("textDigest")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "reconnecting" => SessionUpdate::Reconnecting {
            attempt: update.get("attempt").and_then(Value::as_u64).unwrap_or(0),
            next_delay_ms: update
                .get("nextDelayMs")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            deadline_remaining_ms: update
                .get("deadlineRemainingMs")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
        "param_inert" => SessionUpdate::ParamInert {
            param: update
                .get("param")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            surface: update
                .get("surface")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            detail: update
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "rate_limit" => {
            let snapshot = update.get("snapshot").cloned().unwrap_or(Value::Null);
            let field = |name: &str| snapshot.get(name).and_then(Value::as_u64);
            SessionUpdate::RateLimit {
                requests_remaining: field("requests_remaining"),
                requests_limit: field("requests_limit"),
                tokens_remaining: field("tokens_remaining"),
                tokens_limit: field("tokens_limit"),
            }
        }
        "budget" => SessionUpdate::Budget {
            session_tokens: update
                .get("session_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            microcents: update
                .get("microcents")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            priced: update
                .get("priced")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            limit: update.get("limit").and_then(Value::as_u64),
            observed: update.get("observed").and_then(Value::as_u64),
        },
        "budget_warn" => SessionUpdate::BudgetWarn {
            limit: update.get("limit").and_then(Value::as_u64).unwrap_or(0),
            observed: update.get("observed").and_then(Value::as_u64).unwrap_or(0),
            pct_used: update.get("pct_used").and_then(Value::as_u64).unwrap_or(0),
        },
        "budget_clamp" => SessionUpdate::BudgetClamp {
            requested: update.get("requested").and_then(Value::as_u64).unwrap_or(0),
            granted: update.get("granted").and_then(Value::as_u64).unwrap_or(0),
        },
        "review_result" => SessionUpdate::ReviewResult {
            task_id: update
                .get("taskId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            verdict: update
                .get("verdict")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            text: update
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            error: update.get("error").and_then(|e| {
                let kind = e.get("kind").and_then(Value::as_str)?;
                let message = e.get("message").and_then(Value::as_str).unwrap_or("");
                Some((kind.to_string(), message.to_string()))
            }),
        },
        other => SessionUpdate::Unknown(other.to_string()),
    }
}

/// Approval-spoofing defense lives here: anything but a well-formed request
/// (integer id, toolCall.title, non-empty options each carrying optionId +
/// name) is [`Inbound::MalformedPermission`] → auto-deny.
fn parse_permission(id: Value, frame: &Value) -> Inbound {
    let Some(id) = id.as_u64() else {
        return Inbound::MalformedPermission {
            id: None,
            reason: "permission request id is not an integer".into(),
        };
    };
    let malformed = |reason: &str| Inbound::MalformedPermission {
        id: Some(id),
        reason: reason.to_string(),
    };
    let Some(params) = frame.get("params") else {
        return malformed("missing params");
    };
    let Some(tool_call) = params.get("toolCall") else {
        return malformed("missing toolCall");
    };
    let Some(title) = tool_call.get("title").and_then(Value::as_str) else {
        return malformed("missing toolCall.title");
    };
    let raw_input = tool_call.get("rawInput").cloned().unwrap_or(Value::Null);
    let Some(options) = params.get("options").and_then(Value::as_array) else {
        return malformed("missing options");
    };
    if options.is_empty() {
        return malformed("empty options");
    }
    let mut parsed = Vec::with_capacity(options.len());
    for option in options {
        let (Some(option_id), Some(name)) = (
            option.get("optionId").and_then(Value::as_str),
            option.get("name").and_then(Value::as_str),
        ) else {
            return malformed("option missing optionId/name");
        };
        parsed.push(PermissionOption {
            option_id: option_id.to_string(),
            kind: option
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: name.to_string(),
        });
    }
    Inbound::Permission(PermissionRequest {
        id,
        title: title.to_string(),
        raw_input,
        options: parsed,
    })
}

/// The first `{"type":"diff", path, oldText, newText}` content block, if
/// the update carries one (C10 §6). Malformed blocks are ignored, never
/// fatal — the done frame's rawOutput still flows.
fn parse_diff_block(update: &Value) -> Option<DiffBlock> {
    let blocks = update.get("content")?.as_array()?;
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("diff") {
            continue;
        }
        let path = block.get("path").and_then(Value::as_str)?.to_string();
        let new_text = block.get("newText").and_then(Value::as_str)?.to_string();
        // oldText is null for a whole-file add.
        let old_text = block
            .get("oldText")
            .and_then(Value::as_str)
            .map(str::to_string);
        return Some(DiffBlock {
            path,
            old_text,
            new_text,
        });
    }
    None
}

/// Parse the `models` block carried by session/new, session/load and
/// session/set_model responses (`{currentModelId, availableModels[]}`).
pub fn parse_models(result: &Value) -> Option<(String, Vec<(String, String)>)> {
    let models = result.get("models")?;
    let current = models.get("currentModelId")?.as_str()?.to_string();
    let available = models
        .get("availableModels")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|m| {
            Some((
                m.get("modelId")?.as_str()?.to_string(),
                m.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ))
        })
        .collect();
    Some((current, available))
}

/// Parse the `modes` block carried by session/new and session/load
/// responses (`{currentModeId, availableModes[]}`) — C2. Mode entries use
/// the plain `id` field (unlike models' renamed `modelId`). Unknown/FUTURE
/// mode ids are tolerated by construction: the picker renders whatever the
/// agent advertises; only the host validates ids (an inbound set_mode with
/// an unsupported id is the host's typed error, never a TUI crash).
pub fn parse_modes(result: &Value) -> Option<(String, Vec<(String, String)>)> {
    let modes = result.get("modes")?;
    let current = modes.get("currentModeId")?.as_str()?.to_string();
    let available = modes
        .get("availableModes")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|m| {
            Some((
                m.get("id")?.as_str()?.to_string(),
                m.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ))
        })
        .collect();
    Some((current, available))
}

// ── outbound frame builders ─────────────────────────────────────────────
/// Client request ids, allocated sequentially from 1.
#[derive(Debug, Default)]
pub struct RequestIds {
    next: u64,
}

impl RequestIds {
    pub fn alloc(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": ACP_PROTOCOL_VERSION,
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } },
        "clientInfo": { "name": "nano-tui", "version": env!("CARGO_PKG_VERSION") }
    })
}

pub fn session_new_params(cwd: &str) -> Value {
    json!({"cwd": cwd, "mcpServers": []})
}

pub fn session_load_params(session_id: &str, cwd: &str) -> Value {
    json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []})
}

pub fn session_list_params() -> Value {
    json!({})
}

pub fn parse_session_list(result: Value) -> Option<SessionListResponse> {
    serde_json::from_value(result).ok()
}

pub fn prompt_params(session_id: &str, text: &str) -> Value {
    prompt_params_with_attachments(session_id, text, &[])
}

/// P2a §3.1: the prompt params attachment form — each attached path rides
/// as an `{"type":"image_path","path":…}` extension block under the same
/// params object, in manifest (mint) order. ACP tolerates extension blocks
/// and the host owns the allowlist; the TUI process never reads image bytes
/// (the host's §4 loader resolves them at prompt time).
pub fn prompt_params_with_attachments(
    session_id: &str,
    text: &str,
    attachments: &[String],
) -> Value {
    let mut prompt = vec![json!({"type": "text", "text": text})];
    for path in attachments {
        prompt.push(json!({"type": "image_path", "path": path}));
    }
    json!({"sessionId": session_id, "prompt": prompt})
}

pub fn set_model_params(session_id: &str, model_id: &str) -> Value {
    json!({"sessionId": session_id, "modelId": model_id})
}

/// session/set_mode (C2): switch the session's permission mode. The ack is
/// an empty object; the TUI applies the mode it requested on success.
pub fn set_mode_params(session_id: &str, mode_id: &str) -> Value {
    json!({"sessionId": session_id, "modeId": mode_id})
}

/// session/compact (C1): manual engine-side compaction of the session
/// context, journaled identically to the auto path.
/// P1 §4.1: `/budget continue <tokens>` → session/budget.
pub fn budget_params(session_id: &str, tokens: u64) -> Value {
    serde_json::json!({ "sessionId": session_id, "tokens": tokens })
}

pub fn compact_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}

/// session/steer (C9): mid-turn input queued for the running turn's next
/// loop-top drain. The ack resolves immediately; it is NOT the turn result.
pub fn steer_params(session_id: &str, text: &str) -> Value {
    json!({"sessionId": session_id, "text": text})
}

/// session/steer support, discovered from the initialize response's
/// `agentCapabilities.nanoExtensions` block — never by probing (an
/// unadvertised method would earn the standard -32601 fallback). (The key
/// contains a literal slash, so JSON-pointer escaping would be needed —
/// plain navigation is clearer.)
pub fn parse_steer_capability(result: &Value) -> bool {
    result
        .get("agentCapabilities")
        .and_then(|c| c.get("nanoExtensions"))
        .and_then(|e| e.get("session/steer"))
        .and_then(|s| s.get("version"))
        .is_some()
}

/// Session-list support is capability-discovered, never probed.
pub fn parse_session_list_capability(result: &Value) -> bool {
    result
        .get("agentCapabilities")
        .and_then(|c| c.get("nanoExtensions"))
        .and_then(|extensions| extensions.get(SESSION_LIST_METHOD))
        .and_then(|surface| surface.get("version"))
        .and_then(Value::as_u64)
        == Some(1)
}

/// P4 §9: `_wayland/session/review` params — `{ }` in v1 beyond the
/// session id (working-tree scope only).
pub fn session_review_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}

/// Review-mode support is capability-discovered, never probed (the
/// advertisement flips host-side only with the §14 leg-2 live proof —
/// the honesty rule).
pub fn parse_review_capability(result: &Value) -> bool {
    result
        .get("agentCapabilities")
        .and_then(|c| c.get("nanoExtensions"))
        .and_then(|extensions| extensions.get(SESSION_REVIEW_METHOD))
        .and_then(|surface| surface.get("version"))
        .and_then(Value::as_u64)
        == Some(1)
}

/// _wayland/goal/* (C11): the goal lifecycle mirror. The subcommand selects
/// the extension method; `set` carries the objective.
pub fn goal_method(action: &str) -> String {
    format!("_wayland/goal/{action}")
}

pub fn goal_params(session_id: &str, objective: Option<&str>) -> Value {
    match objective {
        Some(objective) => json!({"sessionId": session_id, "objective": objective}),
        None => json!({"sessionId": session_id}),
    }
}

pub fn cancel_notification(session_id: &str) -> Value {
    json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session_id}})
}

/// The explicit approval decision — the ONLY response shape the TUI ever
/// sends for a permission request (Desktop's resolver maps user choice to
/// exactly this shape). Esc/deny flows use option_id "deny"; the engine
/// approves only `allow*` and fails closed on everything else.
pub fn permission_response(id: u64, option_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
    })
}

pub fn method_not_found_response(id: &Value, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32601, "message": format!("method not found: {method}") }
    })
}

// ── the subprocess connection ───────────────────────────────────────────

/// What the connection delivers to the app loop.
#[derive(Debug)]
pub enum ConnEvent {
    Frame(Value),
    /// A stdout line that was not valid JSON (fail-closed: reported, never
    /// silently skipped).
    ParseError(String),
    /// The host closed its stdout (process exit or pipe break). Carries the
    /// captured stderr tail for diagnostics.
    Closed(String),
}

/// The connection the app's select! loop drives. `send` is synchronous
/// (single NDJSON lines — tiny); `next_event` is the async receive half.
// Internal trait (crate-only impls); async-fn-in-trait is deliberate.
#[allow(async_fn_in_trait)]
pub trait Connection {
    fn send(&mut self, frame: &Value) -> Result<(), String>;
    async fn next_event(&mut self) -> Option<ConnEvent>;
}

/// ACP over a spawned `wayland-nano acp-host` subprocess. A dedicated
/// thread owns stdout parsing (the acp-host reader-thread pattern); another
/// captures stderr for post-mortem display. The child is killed on drop —
/// the TUI never leaves stray wayland-nano processes behind.
pub struct SubprocessConnection {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::UnboundedReceiver<ConnEvent>,
    stderr: Arc<Mutex<String>>,
}

impl SubprocessConnection {
    pub fn spawn(
        program: &std::path::Path,
        args: &[String],
        cwd: &std::path::Path,
    ) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");

        let (tx, rx) = mpsc::unbounded_channel();
        let stderr: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

        let stderr_sink = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stderr_pipe);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut buf = stderr_sink
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        // Keep only a tail — a noisy child must not grow this
                        // without bound.
                        const TAIL: usize = 4096;
                        buf.push_str(&line);
                        let len = buf.len();
                        if len > TAIL {
                            let keep = buf.split_off(len - TAIL);
                            *buf = keep;
                        }
                    }
                }
            }
        });

        let stderr_tail = Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                let n = match reader.read_line(&mut line) {
                    Ok(n) => n,
                    Err(err) => {
                        let _ = tx.send(ConnEvent::ParseError(format!("stdout read: {err}")));
                        break;
                    }
                };
                if n == 0 {
                    let tail = stderr_tail
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    let _ = tx.send(ConnEvent::Closed(tail));
                    break;
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let event = match serde_json::from_str::<Value>(trimmed) {
                    Ok(frame) => ConnEvent::Frame(frame),
                    Err(err) => ConnEvent::ParseError(format!("malformed frame: {err}")),
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            rx,
            stderr,
        })
    }

    /// Best-effort stderr snapshot (for error notes after a failed spawn).
    pub fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Connection for SubprocessConnection {
    fn send(&mut self, frame: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(frame).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("acp-host stdin: {e}"))
    }

    async fn next_event(&mut self) -> Option<ConnEvent> {
        self.rx.recv().await
    }
}

impl Drop for SubprocessConnection {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2a §3.1: the attachment form rides as image_path extension blocks
    /// under the same params object, in mint order; the no-attachment form
    /// is byte-identical to the pre-P2a shape.
    #[test]
    fn p2a_prompt_params_attachment_form() {
        let prompt = prompt_params_with_attachments(
            "s1",
            "look at this",
            &["C:/pics/a.png".to_string(), "C:/pics/b.png".to_string()],
        );
        assert_eq!(prompt["prompt"][0]["type"], "text");
        assert_eq!(prompt["prompt"][0]["text"], "look at this");
        assert_eq!(
            prompt["prompt"][1],
            json!({"type": "image_path", "path": "C:/pics/a.png"})
        );
        assert_eq!(
            prompt["prompt"][2],
            json!({"type": "image_path", "path": "C:/pics/b.png"})
        );
        // No attachments: exactly the legacy single-text-block shape.
        assert_eq!(
            prompt_params("s1", "hi"),
            json!({"sessionId": "s1", "prompt": [{"type": "text", "text": "hi"}]})
        );
    }

    #[test]
    fn builders_match_the_wire_shapes() {
        let init = request(1, "initialize", initialize_params());
        assert_eq!(init["params"]["protocolVersion"], 1);
        assert_eq!(init["params"]["clientInfo"]["name"], "nano-tui");

        let new = request(2, "session/new", session_new_params("/ws"));
        assert_eq!(new["params"]["mcpServers"], json!([]));

        let load = request(3, "session/load", session_load_params("s1", "/ws"));
        assert_eq!(load["params"]["sessionId"], "s1");

        let prompt = request(4, "session/prompt", prompt_params("s1", "hi"));
        assert_eq!(prompt["params"]["prompt"][0]["type"], "text");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "hi");

        let set = request(5, "session/set_model", set_model_params("s1", "flux-fast"));
        assert_eq!(set["params"]["modelId"], "flux-fast");

        let mode = request(7, "session/set_mode", set_mode_params("s1", "full_auto"));
        assert_eq!(mode["params"]["modeId"], "full_auto");

        let compact = request(6, "session/compact", compact_params("s1"));
        assert_eq!(compact["params"]["sessionId"], "s1");

        // P1 §4.1: /budget continue rides session/budget.
        let budget = request(8, "session/budget", budget_params("s1", 200));
        assert_eq!(budget["params"]["sessionId"], "s1");
        assert_eq!(budget["params"]["tokens"], 200);

        let cancel = cancel_notification("s1");
        assert_eq!(cancel["method"], "session/cancel");
        assert!(cancel.get("id").is_none());
    }

    #[test]
    fn permission_response_is_the_only_decision_shape() {
        let resp = permission_response(42, "allow");
        assert_eq!(resp["id"], 42);
        assert_eq!(resp["result"]["outcome"]["outcome"], "selected");
        assert_eq!(resp["result"]["outcome"]["optionId"], "allow");
        assert!(resp.get("method").is_none());
    }

    #[test]
    fn ids_allocate_sequentially() {
        let mut ids = RequestIds::default();
        assert_eq!(ids.alloc(), 1);
        assert_eq!(ids.alloc(), 2);
    }

    /// P1 §5: the budget session/update notices parse into typed updates.
    #[test]
    fn classify_budget_update_variants() {
        let budget = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"budget",
            "session_tokens":12300,"microcents":0,"priced":false,
            "limit":100,"observed":90}}});
        assert_eq!(
            classify(&budget),
            Inbound::Update(SessionUpdate::Budget {
                session_tokens: 12300,
                microcents: 0,
                priced: false,
                limit: Some(100),
                observed: Some(90),
            })
        );

        let warn = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"budget_warn",
            "limit":100,"observed":90,"pct_used":90}}});
        assert_eq!(
            classify(&warn),
            Inbound::Update(SessionUpdate::BudgetWarn {
                limit: 100,
                observed: 90,
                pct_used: 90,
            })
        );

        let clamp = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"budget_clamp",
            "requested":4096,"granted":10}}});
        assert_eq!(
            classify(&clamp),
            Inbound::Update(SessionUpdate::BudgetClamp {
                requested: 4096,
                granted: 10,
            })
        );

        // Uncapped sessions carry null cap fields.
        let uncapped = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"budget",
            "session_tokens":5,"microcents":0,"priced":true,
            "limit":null,"observed":null}}});
        assert_eq!(
            classify(&uncapped),
            Inbound::Update(SessionUpdate::Budget {
                session_tokens: 5,
                microcents: 0,
                priced: true,
                limit: None,
                observed: None,
            })
        );
    }

    #[test]
    fn classify_update_variants() {
        let chunk = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"agent_message_chunk",
            "content":{"type":"text","text":"hi"}}}});
        assert_eq!(
            classify(&chunk),
            Inbound::Update(SessionUpdate::AgentChunk("hi".into()))
        );

        let tool = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"tool_call","toolCallId":"c1",
            "title":"shell","status":"in_progress","rawInput":{"command":"ls"}}}});
        assert_eq!(
            classify(&tool),
            Inbound::Update(SessionUpdate::ToolCall {
                call_id: "c1".into(),
                title: "shell".into(),
                status: "in_progress".into(),
                raw_input: json!({"command": "ls"}),
            })
        );

        // C10 §6: a done frame carrying a diff content block parses it.
        let diffed = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"c2",
            "status":"completed","rawOutput":"written",
            "content":[{"type":"diff","path":"src/main.rs","oldText":"old","newText":"new"}]}}});
        assert_eq!(
            classify(&diffed),
            Inbound::Update(SessionUpdate::ToolCallUpdate {
                call_id: "c2".into(),
                status: "completed".into(),
                raw_output: "written".into(),
                diff: Some(DiffBlock {
                    path: "src/main.rs".into(),
                    old_text: Some("old".into()),
                    new_text: "new".into(),
                }),
                nano_error: None,
            })
        );
        // A done frame without content parses with no diff.
        let plain = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"c3",
            "status":"completed","rawOutput":"ok"}}});
        assert_eq!(
            classify(&plain),
            Inbound::Update(SessionUpdate::ToolCallUpdate {
                call_id: "c3".into(),
                status: "completed".into(),
                raw_output: "ok".into(),
                diff: None,
                nano_error: None,
            })
        );

        // Compaction notices parse to their own variant (C1).
        let notice = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"compaction","status":"complete"}}});
        assert_eq!(
            classify(&notice),
            Inbound::Update(SessionUpdate::Compaction {
                status: "complete".into()
            })
        );

        // Unknown kinds are tolerated (forward-additive / torn replay).
        let future = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"plan","entries":[]}}});
        assert_eq!(
            classify(&future),
            Inbound::Update(SessionUpdate::Unknown("plan".into()))
        );
    }

    #[test]
    fn classify_permission_request() {
        let req = json!({"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{
            "sessionId":"s",
            "toolCall":{"toolCallId":"c1","title":"fs_write","rawInput":{"path":"a"}},
            "options":[
                {"optionId":"allow","kind":"allow_once","name":"Allow once"},
                {"optionId":"deny","kind":"reject_once","name":"Deny"}]}});
        let Inbound::Permission(p) = classify(&req) else {
            panic!("expected permission request");
        };
        assert_eq!(p.id, 7);
        assert_eq!(p.title, "fs_write");
        assert_eq!(p.options.len(), 2);
        assert_eq!(p.options[1].option_id, "deny");
    }

    #[test]
    fn classify_malformed_permission_requests_all_fail_closed() {
        // Non-integer id.
        let bad_id =
            json!({"jsonrpc":"2.0","id":"x","method":"session/request_permission","params":{}});
        assert!(matches!(
            classify(&bad_id),
            Inbound::MalformedPermission { id: None, .. }
        ));
        // Missing params / toolCall / title / options / empty options /
        // option missing optionId.
        let cases = [
            json!({"id":1,"method":"session/request_permission"}),
            json!({"id":1,"method":"session/request_permission","params":{}}),
            json!({"id":1,"method":"session/request_permission","params":{"toolCall":{}}}),
            json!({"id":1,"method":"session/request_permission","params":{"toolCall":{"title":"t"}}}),
            json!({"id":1,"method":"session/request_permission","params":{"toolCall":{"title":"t"},"options":[]}}),
            json!({"id":1,"method":"session/request_permission","params":{"toolCall":{"title":"t"},"options":[{"name":"Allow"}]}}),
        ];
        for case in cases {
            assert!(
                matches!(
                    classify(&case),
                    Inbound::MalformedPermission { id: Some(1), .. }
                ),
                "case must auto-deny: {case}"
            );
        }
    }

    #[test]
    fn classify_responses_and_unknowns() {
        let ok = json!({"jsonrpc":"2.0","id":3,"result":{"sessionId":"s1"}});
        assert_eq!(
            classify(&ok),
            Inbound::Response {
                id: 3,
                result: Some(json!({"sessionId": "s1"})),
                error: None,
            }
        );
        let err = json!({"jsonrpc":"2.0","id":3,"error":{"code":-32602,"message":"bad"}});
        assert_eq!(
            classify(&err),
            Inbound::Response {
                id: 3,
                result: None,
                error: Some(WireError {
                    code: -32602,
                    message: "bad".into(),
                    nano: None,
                }),
            }
        );
        let unknown_req = json!({"jsonrpc":"2.0","id":9,"method":"fs/read_text_file","params":{}});
        assert!(matches!(
            classify(&unknown_req),
            Inbound::UnknownRequest { .. }
        ));
        let garbage = json!({"hello": "world"});
        assert!(garbage.is_object());
        assert!(matches!(classify(&garbage), Inbound::MalformedFrame(_)));
    }

    #[test]
    fn parse_models_block() {
        let result = json!({"sessionId":"s","models":{
            "currentModelId":"flux-auto",
            "availableModels":[{"modelId":"flux-auto","name":"Auto"},{"modelId":"flux-fast","name":"Fast"}]}});
        let (current, available) = parse_models(&result).unwrap();
        assert_eq!(current, "flux-auto");
        assert_eq!(available.len(), 2);
        assert!(parse_models(&json!({})).is_none());
    }

    #[test]
    fn parse_modes_block_tolerates_future_ids() {
        let result = json!({"modes":{
            "currentModeId":"read_only",
            "availableModes":[
                {"id":"read_only","name":"Read Only"},
                {"id":"default","name":"Default"},
                {"id":"full_auto","name":"Full Auto"},
                {"id":"quantum","name":"Quantum"}]}});
        let (current, available) = parse_modes(&result).unwrap();
        assert_eq!(current, "read_only");
        let ids: Vec<&str> = available.iter().map(|(id, _)| id.as_str()).collect();
        // A newer agent's unknown mode id parses through untouched — the
        // picker renders it and the HOST decides whether set_mode accepts.
        assert_eq!(ids, ["read_only", "default", "full_auto", "quantum"]);
        assert!(parse_modes(&json!({})).is_none());
        assert!(parse_modes(&json!({"modes": {"availableModes": []}})).is_none());
    }

    #[test]
    fn session_list_capability_and_response_are_typed() {
        let initialize = json!({"agentCapabilities":{"nanoExtensions":{
            "_wayland/session/list":{"version":1}
        }}});
        assert!(parse_session_list_capability(&initialize));
        assert!(!parse_session_list_capability(&json!({})));
        let response = parse_session_list(json!({
            "sessions":[{
                "sessionId":"s1","cwd":"C:/workspace","modifiedMs":12,
                "sizeBytes":34,"status":"live"
            }],
            "truncated":false,
            "liveStatusCaveat":"point-in-time"
        }))
        .unwrap();
        assert_eq!(response.sessions[0].status, SessionListStatus::Live);
        assert!(
            parse_session_list(json!({
                "sessions":[{"status":"future"}],
                "truncated":false,"liveStatusCaveat":"point-in-time"
            }))
            .is_none()
        );
    }

    /// P4 §9/§3.4: the review capability is discovered (never probed) and
    /// the review_result card parses typed, tolerating a missing error.
    #[test]
    fn review_capability_and_card_are_typed() {
        let initialize = json!({"agentCapabilities":{"nanoExtensions":{
            "_wayland/session/review":{"version":1}
        }}});
        assert!(parse_review_capability(&initialize));
        assert!(!parse_review_capability(&json!({})));
        assert_eq!(SESSION_REVIEW_METHOD, "_wayland/session/review");
        assert_eq!(session_review_params("s1"), json!({"sessionId":"s1"}));

        let frame = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"review_result",
                "taskId":"task-1","status":"completed",
                "verdict":"patch is incorrect","text":"Review comment:\n\n- [P1] x — a.rs:1"}}});
        let Inbound::Update(SessionUpdate::ReviewResult {
            task_id,
            status,
            verdict,
            text,
            error,
        }) = classify(&frame)
        else {
            panic!("review_result classifies: {:?}", classify(&frame));
        };
        assert_eq!(task_id, "task-1");
        assert_eq!(status, "completed");
        assert_eq!(verdict, "patch is incorrect");
        assert!(text.contains("[P1]"), "{text}");
        assert_eq!(error, None);

        // The typed failure payload round-trips.
        let failed = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"review_result",
                "taskId":"task-2","status":"failed","verdict":"","text":"",
                "error":{"kind":"review_parse_failed","message":"couldn't parse"}}}});
        let Inbound::Update(SessionUpdate::ReviewResult { error, .. }) = classify(&failed) else {
            panic!("typed failure classifies");
        };
        assert_eq!(
            error,
            Some((
                "review_parse_failed".to_string(),
                "couldn't parse".to_string()
            ))
        );
    }

    /// C7: the numeric code AND the typed data payload survive parsing.
    #[test]
    fn classify_error_response_keeps_code_and_nano_payload() {
        let err = json!({"jsonrpc":"2.0","id":5,"error":{
            "code":-32603,"message":"Rate limited",
            "data":{"nanoError":{"kind":"model_rate_limited","retryable":true,"retry_after_ms":500}}}});
        assert_eq!(
            classify(&err),
            Inbound::Response {
                id: 5,
                result: None,
                error: Some(WireError {
                    code: -32603,
                    message: "Rate limited".into(),
                    nano: Some(NanoErrorPayload {
                        kind: NanoErrorKind::ModelRateLimited,
                        retryable: true,
                    }),
                }),
            }
        );
    }

    /// C7 (design §2/D2): a kind from the FUTURE parses as Unknown, and a
    /// forged `retryable:true` on an unknown kind still classifies terminal
    /// downstream (Unknown's spec is terminal; app renders the wire flag
    /// only for KNOWN kinds — see app.rs::error_cell).
    #[test]
    fn future_kind_parses_as_unknown() {
        let err = json!({"jsonrpc":"2.0","id":6,"error":{
            "code":-32603,"message":"mystery",
            "data":{"nanoError":{"kind":"kind_from_the_future","retryable":true}}}});
        let Inbound::Response {
            error: Some(err), ..
        } = classify(&err)
        else {
            panic!("expected error response");
        };
        assert_eq!(err.nano.map(|n| n.kind), Some(NanoErrorKind::Unknown));
    }

    /// C7: `_meta.nanoError` on a failed tool_call_update parses through.
    #[test]
    fn tool_call_update_parses_nano_error_meta() {
        let frame = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"c1",
            "status":"failed","rawOutput":"len:21",
            "_meta":{"nanoError":{"kind":"approval_denied","retryable":false}}}}});
        assert_eq!(
            classify(&frame),
            Inbound::Update(SessionUpdate::ToolCallUpdate {
                call_id: "c1".into(),
                status: "failed".into(),
                raw_output: "len:21".into(),
                nano_error: Some(NanoErrorPayload {
                    kind: NanoErrorKind::ApprovalDenied,
                    retryable: false,
                }),
                diff: None,
            })
        );
    }
}
