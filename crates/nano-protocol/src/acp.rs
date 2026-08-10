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
pub fn agent_capabilities() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": ACP_PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {
                "text": true,
                "image": false,
                "embeddedContext": false
            }
        },
        "agentInfo": {
            "name": "nanok3",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

/// session/new response.
pub fn session_new_result(session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "modes": {
            "availableModes": [{"id": "default", "name": "Default"}],
            "currentModeId": "default"
        }
    })
}

/// session/load response. Per the ACP shape Desktop expects
/// (AcpConnection.ts `loadSession`: "session/load returns
/// modes/models/configOptions but not sessionId"), the loaded session keeps
/// the id the client sent, so no sessionId is returned here.
pub fn session_load_result() -> serde_json::Value {
    serde_json::json!({
        "modes": {
            "availableModes": [{"id": "default", "name": "Default"}],
            "currentModeId": "default"
        }
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
        _ => "other",
    }
}

/// session/prompt response.
pub fn prompt_result(stop_reason: &str) -> serde_json::Value {
    serde_json::json!({ "stopReason": stop_reason })
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
        assert_eq!(caps["agentInfo"]["name"], "nanok3");
        assert_eq!(
            caps["agentCapabilities"]["promptCapabilities"]["text"],
            true
        );
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
    }

    #[test]
    fn prompt_result_stop_reason() {
        let result = prompt_result("end_turn");
        assert_eq!(result["stopReason"], "end_turn");
    }

    #[test]
    fn method_not_found_is_typed() {
        let resp = JsonRpcResponse::method_not_found(serde_json::json!(7), "bogus/method");
        assert_eq!(resp.error.unwrap().code, -32601);
    }
}
