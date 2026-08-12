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
            }),
        }
    }

    pub fn method_not_found(id: serde_json::Value, method: &str) -> Self {
        Self::err(id, -32601, format!("method not found: {method}"))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonRpcErrorBody {
    pub code: i64,
    pub message: String,
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
            },
            // C9: extension-method advertisement with a version marker, the
            // same discipline as session/compact / session/set_model.
            // Clients discover support HERE, never by probing; a client
            // that sends an unknown method gets the standard JSON-RPC
            // -32601 fallback.
            "nanoExtensions": {
                "session/steer": { "version": 1 }
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
/// the work happened in a previous process lifetime.
pub fn tool_call_replay(
    session_id: &str,
    call_id: &str,
    name: &str,
    args: &serde_json::Value,
    ok: bool,
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
                "status": if ok { "completed" } else { "failed" },
                "rawInput": args
            }
        }),
    )
}

/// A tool call completing.
pub fn tool_call_done(
    session_id: &str,
    call_id: &str,
    ok: bool,
    output: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": call_id,
                "status": if ok { "completed" } else { "failed" },
                "rawOutput": output
            }
        }),
    )
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

// ── C9 robustness-pack wire shapes ──────────────────────────────────────

/// The `session/steer` extension method (C9 Q1 RULED shape (b)): mid-turn
/// user input that the running turn drains at its next loop top. The
/// response resolves IMMEDIATELY with the enqueue ack — it is NOT the turn
/// result (the terminal result still belongs to the original prompt).
pub const SESSION_STEER_METHOD: &str = "session/steer";

/// session/steer queued ack: the submitter's proof of acceptance.
pub fn steer_queued_result(position: usize) -> serde_json::Value {
    serde_json::json!({ "queued": true, "position": position })
}

/// The dropped-steer notice (C9 §3.3): because the queued ack resolves
/// before any later cancellation, a drop-on-cancel travels as a LATER
/// session/update carrying the submitter's request id and the steer text
/// digest (never the text itself — it was never model-visible). Exactly
/// one notice per dropped steer; none is dropped silently.
pub fn steer_dropped_notice(
    session_id: &str,
    request_id: &str,
    text_digest: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "steer_dropped",
                "requestId": request_id,
                "textDigest": text_digest
            }
        }),
    )
}

/// Reconnect banner (C9 §2.2): one notice per reconnect sleep, typed
/// fields only — UIs render, never parse strings.
pub fn reconnect_notice(
    session_id: &str,
    attempt: u32,
    next_delay_ms: u64,
    deadline_remaining_ms: u64,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "reconnecting",
                "attempt": attempt,
                "nextDelayMs": next_delay_ms,
                "deadlineRemainingMs": deadline_remaining_ms
            }
        }),
    )
}

/// Loud inert-param notice (C9 §4, Q3 rung 2): a requested param was
/// omitted from the wire (or mapped-but-recorded-inert) on this surface.
pub fn param_inert_notice(
    session_id: &str,
    param: &str,
    surface: &str,
    detail: &str,
) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "param_inert",
                "param": param,
                "surface": surface,
                "detail": detail
            }
        }),
    )
}

/// Rate-limit observation (C9 §5, Q4): the coalesced latest snapshot per
/// turn iteration. `snapshot` is the serialized RateLimitSnapshot (all
/// fields optional — UIs render "unknown" on absence, never a guess).
pub fn rate_limit_notice(session_id: &str, snapshot: serde_json::Value) -> JsonRpcNotification {
    JsonRpcNotification::new(
        "session/update",
        serde_json::json!({
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "rate_limit",
                "snapshot": snapshot
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
            let frame = tool_call_done("s1", "c1", true, output);
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
}
