//! ACP (Agent Client Protocol) wire types — the stdio JSON-RPC protocol
//! Wayland Desktop speaks to third-party agents.
//!
//! Evidence-based subset (from Desktop's AcpConnection.ts + @agentclientprotocol/sdk 0.18.2):
//! - initialize: protocolVersion 1, clientCapabilities.fs
//! - session/new: {cwd, mcpServers[]} → {sessionId}
//! - session/load: {sessionId, cwd, mcpServers[]} → {modes} after replaying
//!   the journaled transcript as session/update notifications
//! - session/prompt: {sessionId, prompt:[{type:"text",text}]} → {stopReason}
//! - session/cancel: notification {sessionId}
//! - session/update: notification {sessionId, update:{sessionUpdate: kind, ...}}
//! - session/request_permission: agent→host request (approval UI)
//!
//! Everything else fails typed (method-not-found), never panics.

use serde::Deserialize;
use serde::Serialize;

use nano_session::NanoErrorKind;

pub use crate::error_codes::error_presentation;
use crate::error_codes::spec;
use crate::permission_mode::PermissionMode;

pub const ACP_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorBody>,
}

impl JsonRpcResponse {
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcErrorBody {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// A TYPED error response (C7): the numeric code comes from the error
    /// table (standard JSON-RPC codes only) and the typing rides in
    /// `data.nanoError` — closed typed fields only (kind enum, retryable
    /// bool, bounded numeric codes, egress-redacted host), never free-form
    /// detail strings (design §2/D2, §7).
    pub fn err_typed(
        id: serde_json::Value,
        kind: NanoErrorKind,
        message: impl Into<String>,
        extras: NanoErrorExtras,
    ) -> Self {
        let spec = spec(kind);
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcErrorBody {
                code: spec.wire_code,
                message: message.into(),
                data: Some(nano_error_data(kind, &extras)),
            }),
        }
    }

    pub fn method_not_found(id: serde_json::Value, method: &str) -> Self {
        Self::err(id, -32601, format!("method not found: {method}"))
    }
}

/// The closed set of optional typed detail fields a `nanoError` payload may
/// carry (design §2/D2: booleans, bounded numeric codes, the egress-redacted
/// host — NO free-form strings, ever).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NanoErrorExtras {
    pub status: Option<u16>,
    pub retry_after_ms: Option<u64>,
    pub host: Option<String>,
}

/// `error.data` / `_meta` payload: `{ "nanoError": { "kind", "retryable",
/// ...closed extras } }`.
pub fn nano_error_data(kind: NanoErrorKind, extras: &NanoErrorExtras) -> serde_json::Value {
    let mut nano = serde_json::json!({
        "kind": kind,
        "retryable": spec(kind).retryable,
    });
    if let Some(status) = extras.status {
        nano["status"] = serde_json::json!(status);
    }
    if let Some(retry_after_ms) = extras.retry_after_ms {
        nano["retry_after_ms"] = serde_json::json!(retry_after_ms);
    }
    if let Some(host) = &extras.host {
        nano["host"] = serde_json::json!(host);
    }
    serde_json::json!({ "nanoError": nano })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcErrorBody {
    pub code: i64,
    pub message: String,
    /// C7: typed error payload (`{"nanoError": {...}}`) — absent on
    /// pre-C7-style generic errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: Some(params),
        }
    }
}

/// Agent capabilities advertised in the initialize response.
///
/// `mcpCapabilities` is present because session/new (and session/load)
/// genuinely consume `mcpServers`: Desktop parses the block's PRESENCE as
/// stdio support (acpTypes.ts `parseAgentCapabilitiesObject`: `stdio: mcp
/// !== null`) and reads `http`/`sse` as booleans — so the honest shape is
/// exactly `{http: false, sse: false}` (stdio-only, implied by presence).
pub fn agent_capabilities() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": ACP_PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {
                "text": true,
                "image": false,
                "embeddedContext": false
            },
            "mcpCapabilities": {
                "http": false,
                "sse": false
            }
        },
        "agentInfo": {
            "name": "wayland-nano",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// A model the session can switch to (the ACP `models` block, unstable API).
/// The ACP-spec field name is `modelId` — proven by live Desktop behavior
/// (SessionLifecycle.ts maps m.modelId; rows sent as `id` render with
/// undefined ids and no-op). The Rust field stays `id`; the wire carries
/// `modelId` via serde rename.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableModel {
    #[serde(rename = "modelId")]
    pub id: String,
    pub name: String,
}

/// The `models` block shared by session/new, session/load and
/// session/set_model responses. Desktop reads it top-level
/// (AcpConnection.ts `parseSessionCapabilities`).
pub fn session_models_value(
    current_model_id: &str,
    available: &[AvailableModel],
) -> serde_json::Value {
    serde_json::json!({
        "currentModelId": current_model_id,
        "availableModels": available,
    })
}

/// The `modes` block shared by session/new and session/load responses
/// (C2): every advertised mode comes from the single [`PermissionMode`]
/// metadata, exactly the way `session_models_value` parameterizes models,
/// so the wire can never drift from what the gate enforces. `currentModeId`
/// is always the session's actual mode — sessions start (and resume) in
/// `default`, never a resurrected one.
pub fn session_modes_value(current: PermissionMode) -> serde_json::Value {
    serde_json::json!({
        "availableModes": PermissionMode::ALL
            .iter()
            .map(|mode| serde_json::json!({"id": mode.id(), "name": mode.label()}))
            .collect::<Vec<_>>(),
        "currentModeId": current.id(),
    })
}

/// session/new response.
pub fn session_new_result(
    session_id: &str,
    current_model_id: &str,
    available: &[AvailableModel],
) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "modes": session_modes_value(PermissionMode::default()),
        "models": session_models_value(current_model_id, available)
    })
}

/// session/load response. Per the ACP shape Desktop expects
/// (AcpConnection.ts `loadSession`: "session/load returns
/// modes/models/configOptions but not sessionId"), the loaded session keeps
/// the id the client sent, so no sessionId is returned here. C2: the mode
/// is NOT restored on load — a resumed session starts in `default` and
/// re-entering an elevated mode takes a fresh, explicit session/set_mode.
pub fn session_load_result(
    current_model_id: &str,
    available: &[AvailableModel],
) -> serde_json::Value {
    serde_json::json!({
        "modes": session_modes_value(PermissionMode::default()),
        "models": session_models_value(current_model_id, available)
    })
}

/// session/set_model response: the updated models state (Desktop updates its
/// cache from the requested id; echoing the state keeps the agent the source
/// of truth).
pub fn set_model_result(current_model_id: &str, available: &[AvailableModel]) -> serde_json::Value {
    serde_json::json!({
        "models": session_models_value(current_model_id, available)
    })
}

/// A replayed user message (session/load history restore). Desktop ignores
/// these (its local DB already shows the user's own messages) but real ACP
/// agents emit them, so the replay is a faithful transcript.
pub fn user_message_chunk(session_id: &str, text: &str) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "user_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
}

/// A streamed text chunk (the assistant's reply, incrementally).
pub fn agent_message_chunk(session_id: &str, text: &str) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": { "type": "text", "text": text }
            }
        }),
    )
}

/// A tool call starting (shown in Desktop as a tool card).
pub fn tool_call_update(
    session_id: &str,
    call_id: &str,
    name: &str,
    args: &serde_json::Value,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call",
                "toolCallId": call_id,
                "title": name,
                "kind": tool_kind(name),
                "status": "in_progress",
                "rawInput": args
            }
        }),
    )
}

/// A replayed tool call (session/load history restore): the same `tool_call`
/// card shape as a live call, but already carrying its final status, since
/// the work happened in a previous process lifetime. A failed call with a
/// journaled `error_kind` carries the typed presentation + `_meta.nanoError`
/// exactly like the live path (D3/D5 — one typed op, identical frames).
pub fn tool_call_replay(
    session_id: &str,
    call_id: &str,
    name: &str,
    args: &serde_json::Value,
    ok: bool,
    error_kind: Option<NanoErrorKind>,
) -> JsonRpcNotification {
    let mut update = serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": call_id,
        "title": name,
        "kind": tool_kind(name),
        "status": if ok { "completed" } else { "failed" },
        "rawInput": args
    });
    if let Some(kind) = error_kind {
        attach_typed_failure(&mut update, kind);
    }
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": update
        }),
    )
}

/// A tool call completing. On a typed failure (`error_kind`), the frame
/// gains (design §2/D3):
/// - `content`: the static table presentation — the exact shape Desktop's
///   normalizer stringifies today, so failed cards show an honest message
///   with ZERO Desktop change;
/// - `_meta.nanoError`: the closed typed payload for typed consumers.
///   `rawOutput` keeps the digest for back-compat.
pub fn tool_call_done(
    session_id: &str,
    call_id: &str,
    ok: bool,
    output: &str,
    error_kind: Option<NanoErrorKind>,
) -> JsonRpcNotification {
    let mut update = serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": call_id,
        "status": if ok { "completed" } else { "failed" },
        "rawOutput": output
    });
    if let Some(kind) = error_kind {
        attach_typed_failure(&mut update, kind);
    }
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": update
        }),
    )
}

/// D3 fields on a failed tool card: ACP-spec `content` (Desktop's
/// normalizer reads it verbatim) + `_meta.nanoError` (typed consumers).
fn attach_typed_failure(update: &mut serde_json::Value, kind: NanoErrorKind) {
    update["content"] = serde_json::json!([{
        "type": "content",
        "content": { "type": "text", "text": error_presentation(kind) }
    }]);
    update["_meta"] = nano_error_data(kind, &NanoErrorExtras::default());
}

fn tool_kind(name: &str) -> &'static str {
    match name {
        n if n.starts_with("fs_read") => "read",
        n if n.starts_with("fs_edit") || n.starts_with("fs_write") => "edit",
        n if n.starts_with("shell") => "execute",
        n if n.starts_with("search") || n.starts_with("glob") => "search",
        n if n.starts_with("web_fetch") => "fetch",
        _ => "other",
    }
}

/// session/prompt response.
pub fn prompt_result(stop_reason: &str) -> serde_json::Value {
    serde_json::json!({ "stopReason": stop_reason })
}

/// A context-compaction lifecycle notice (C1 §7): emitted on
/// CompactionBegin/Complete/Cancel so UIs can render the event as a system
/// note in the transcript. `status` is "begin" | "complete" | "cancel".
/// Clients that do not know the kind tolerate it (unknown sessionUpdate
/// kinds convert to zero messages — pinned by Desktop's adapter tests).
pub fn compaction_notice(session_id: &str, status: &str) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "compaction",
                "status": status
            }
        }),
    )
}

/// session/request_permission request payload (agent → host).
pub fn request_permission_request(
    id: u64,
    session_id: &str,
    call_id: &str,
    title: &str,
    args: &serde_json::Value,
) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(id),
        method: "session/request_permission".into(),
        params: Some(serde_json::json!({
            "sessionId": session_id,
            "toolCall": {
                "toolCallId": call_id,
                "title": title,
                "rawInput": args
            },
            "options": [
                { "optionId": "allow", "kind": "allow_once", "name": "Allow once" },
                { "optionId": "deny", "kind": "reject_once", "name": "Deny" }
            ]
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_response_shape() {
        let caps = agent_capabilities();
        assert_eq!(caps["protocolVersion"], 1);
        assert_eq!(caps["agentInfo"]["name"], "wayland-nano");
        assert_eq!(
            caps["agentCapabilities"]["promptCapabilities"]["text"],
            true
        );
        // stdio-only MCP: the block's presence advertises stdio to Desktop
        // (acpTypes.ts), http/sse stay honestly false.
        let mcp = &caps["agentCapabilities"]["mcpCapabilities"];
        assert!(
            mcp.is_object(),
            "mcpCapabilities must be advertised: {caps}"
        );
        assert_eq!(mcp["http"], false);
        assert_eq!(mcp["sse"], false);
    }

    #[test]
    fn session_update_shapes() {
        let chunk = agent_message_chunk("s1", "hello");
        let json = serde_json::to_value(&chunk).unwrap();
        assert_eq!(json["method"], "session/update");
        assert_eq!(
            json["params"]["update"]["sessionUpdate"],
            "agent_message_chunk"
        );
        assert_eq!(json["params"]["update"]["content"]["text"], "hello");

        let tool = tool_call_update("s1", "c1", "shell", &serde_json::json!({"command":"ls"}));
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["params"]["update"]["kind"], "execute");
        assert_eq!(json["params"]["update"]["status"], "in_progress");

        let fetch = tool_call_update(
            "s1",
            "c2",
            "web_fetch",
            &serde_json::json!({"url":"https://example.com/"}),
        );
        let json = serde_json::to_value(&fetch).unwrap();
        assert_eq!(json["params"]["update"]["kind"], "fetch");
    }

    /// Cross-cutting C3/C4: a full-size fs_read page (100 KB) and a
    /// full-size web_fetch body (64 KB) ride `tool_call_done`'s rawOutput
    /// through the frame codec (serde JSON / NDJSON) without corruption.
    /// The global engine-side ceiling is tracked separately
    /// (docs/FOLLOWUPS.md F-1).
    #[test]
    fn full_size_tool_results_round_trip_through_the_codec() {
        let page = "line content\n".repeat(8 * 1024); // ~106 KB, C3 page-sized
        let body = "x".repeat(64 * 1024); // C4 body cap
        for output in [&page, &body] {
            let frame = tool_call_done("s1", "c1", true, output, None);
            let line = serde_json::to_string(&frame).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                parsed["params"]["update"]["rawOutput"].as_str().unwrap(),
                output,
            );
        }
    }

    #[test]
    fn session_new_carries_the_desktop_models_shape() {
        let available = vec![
            AvailableModel {
                id: "flux-auto".into(),
                name: "flux-auto".into(),
            },
            AvailableModel {
                id: "flux-fast".into(),
                name: "flux-fast".into(),
            },
        ];
        let result = session_new_result("s1", "flux-auto", &available);
        assert_eq!(result["sessionId"], "s1");
        assert_eq!(result["models"]["currentModelId"], "flux-auto");
        let models = result["models"]["availableModels"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[1]["modelId"], "flux-fast");
        assert_eq!(models[1]["name"], "flux-fast");

        // session/load carries the same models block, without a sessionId.
        let loaded = session_load_result("flux-fast", &available);
        assert!(loaded.get("sessionId").is_none());
        assert_eq!(loaded["models"]["currentModelId"], "flux-fast");

        let switched = set_model_result("flux-fast", &available);
        assert_eq!(switched["models"]["currentModelId"], "flux-fast");
        assert_eq!(
            switched["models"]["availableModels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn prompt_result_stop_reason() {
        let result = prompt_result("end_turn");
        assert_eq!(result["stopReason"], "end_turn");
    }

    #[test]
    fn modes_block_comes_from_the_permission_mode_metadata() {
        let result = session_new_result("s1", "flux-auto", &[]);
        let modes = &result["modes"];
        assert_eq!(modes["currentModeId"], "default");
        let advertised = modes["availableModes"].as_array().unwrap();
        let ids: Vec<&str> = advertised
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        // The full C2 vocabulary, in privilege order, from PermissionMode::ALL.
        assert_eq!(ids, ["read_only", "default", "full_auto"]);
        for entry in advertised {
            let mode = PermissionMode::parse(entry["id"].as_str().unwrap()).unwrap();
            assert_eq!(entry["name"].as_str().unwrap(), mode.label());
        }
        // session/load advertises the same block and never resurrects a mode.
        let loaded = session_load_result("flux-auto", &[]);
        assert_eq!(loaded["modes"]["currentModeId"], "default");
        assert_eq!(loaded["modes"]["availableModes"], modes["availableModes"]);
    }

    #[test]
    fn method_not_found_is_typed() {
        let resp = JsonRpcResponse::method_not_found(serde_json::json!(7), "bogus/method");
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    /// C7/D3: a typed tool failure carries the ACP-spec `content`
    /// presentation (Desktop's normalizer stringifies it today — zero
    /// Desktop change), the `_meta.nanoError` typed payload, and keeps the
    /// digest in rawOutput for back-compat.
    #[test]
    fn failed_tool_card_carries_content_and_meta() {
        let frame = tool_call_done(
            "s1",
            "c1",
            false,
            "len:42",
            Some(NanoErrorKind::ApprovalDenied),
        );
        let json = serde_json::to_value(&frame).unwrap();
        let update = &json["params"]["update"];
        assert_eq!(update["status"], "failed");
        assert_eq!(update["rawOutput"], "len:42");
        assert_eq!(update["content"][0]["content"]["text"], "Denied by user");
        let nano = &update["_meta"]["nanoError"];
        assert_eq!(nano["kind"], "approval_denied");
        assert_eq!(nano["retryable"], false);

        // The replayed card carries the same typing.
        let replay = tool_call_replay(
            "s1",
            "c1",
            "fs_write",
            &serde_json::json!({"path": "a"}),
            false,
            Some(NanoErrorKind::FsWriteDenied),
        );
        let json = serde_json::to_value(&replay).unwrap();
        let update = &json["params"]["update"];
        assert_eq!(update["status"], "failed");
        assert_eq!(update["_meta"]["nanoError"]["kind"], "fs_write_denied");
        assert_eq!(
            update["content"][0]["content"]["text"],
            "Denied by policy — Path is outside the allowed set; ask the user"
        );

        // Untyped completions are byte-compatible with the pre-C7 shape.
        let plain = tool_call_done("s1", "c1", true, "len:1", None);
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json["params"]["update"].get("_meta").is_none());
        assert!(json["params"]["update"].get("content").is_none());
    }

    /// C7/D2: typed error responses carry standard codes + closed data.
    #[test]
    fn typed_error_response_shape() {
        let resp = JsonRpcResponse::err_typed(
            serde_json::json!(3),
            NanoErrorKind::ModelRateLimited,
            "Rate limited — Retrying automatically; wait a moment",
            crate::acp::NanoErrorExtras {
                retry_after_ms: Some(1500),
                ..Default::default()
            },
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32603);
        let nano = &json["error"]["data"]["nanoError"];
        assert_eq!(nano["kind"], "model_rate_limited");
        assert_eq!(nano["retryable"], true);
        assert_eq!(nano["retry_after_ms"], 1500);
        assert!(nano.get("host").is_none());

        // Untyped errors never grow a data field.
        let plain = JsonRpcResponse::err(serde_json::json!(1), -32700, "parse error");
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json["error"].get("data").is_none());
    }
}
