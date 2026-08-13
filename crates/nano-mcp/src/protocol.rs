//! JSON-RPC 2.0 protocol types for MCP.

use serde::Deserialize;
use serde::Serialize;

pub const JSONRPC_VERSION: &str = "2.0";
/// P3 D4: bumped from 2025-03-26 — elicitation is a 2025-06-18 feature and
/// the 2025-03-26 pin cannot carry it honestly (design note §2.7, flagged
/// deviation 2 against the V2).
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// The protocol version at which a server may send `elicitation/create`.
/// Date-stamped versions compare correctly as plain strings.
pub const MCP_ELICITATION_VERSION: &str = "2025-06-18";
pub const CLIENT_NAME: &str = "wayland-nano";
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// JSON-RPC error codes the dispatcher emits on the wire (design §2.5).
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id: serde_json::json!(id),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn initialized() -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: "notifications/initialized".into(),
            params: None,
        }
    }
}

// P3 D1: deny_unknown_fields is the attribute half of the V2 discriminator
// (the tag half is dispatcher::classify_frame) — a server REQUEST mis-routed
// to the response arm now fails typed-deserialize instead of silently
// dropping `method`/`params` (the §2.1.1 defect).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn into_result(self) -> Result<serde_json::Value, crate::client::McpError> {
        match (self.result, self.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(crate::client::McpError::Server {
                code: error.code,
                message: error.message,
            }),
            _ => Err(crate::client::McpError::Protocol(
                "response carries both/neither result and error".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub fn initialize_params() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": CLIENT_NAME,
            "version": CLIENT_VERSION,
        }
    })
}

/// Initialize params with the client-side `elicitation` capability
/// advertised (stdio only, design §2.7/§5.5). Advertised only when an
/// elicitation handler is actually installed — the honesty rule applied to
/// the handshake.
pub fn initialize_params_with_elicitation() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "elicitation": {} },
        "clientInfo": {
            "name": CLIENT_NAME,
            "version": CLIENT_VERSION,
        }
    })
}

/// The negotiated server capabilities recorded from the initialize result
/// (design §2.7) — the enforcing record for the §4.2/§5.5 gates.
/// `elicitation` here means *available on this connection*: we advertised
/// the client capability AND the negotiated protocol version carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegotiatedCapabilities {
    pub tools: bool,
    pub resources: bool,
    pub resource_templates: bool,
    pub elicitation: bool,
    /// ToolSearch deferred-tier marker (§3); always false until that lane.
    pub deferred_tools: bool,
    pub protocol_version: String,
}

impl NegotiatedCapabilities {
    pub fn from_initialize_result(
        result: &serde_json::Value,
        elicitation_advertised: bool,
    ) -> Self {
        let caps = result.get("capabilities").cloned().unwrap_or_default();
        let protocol_version = result
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Self {
            tools: caps.get("tools").is_some(),
            resources: caps.get("resources").is_some(),
            resource_templates: caps
                .get("resources")
                .and_then(|r| r.get("templates"))
                .is_some(),
            elicitation: elicitation_advertised
                && protocol_version.as_str() >= MCP_ELICITATION_VERSION,
            deferred_tools: false,
            protocol_version,
        }
    }
}

/// `notifications/cancelled` params (design §2.4) — spec-legal cancellation
/// of an in-flight request, sent best-effort on the priority lane.
pub fn cancelled_params(request_id: serde_json::Value, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "requestId": request_id,
        "reason": reason,
    })
}

pub fn cancelled_notification(request_id: serde_json::Value, reason: &str) -> JsonRpcNotification {
    JsonRpcNotification {
        jsonrpc: JSONRPC_VERSION.into(),
        method: "notifications/cancelled".into(),
        params: Some(cancelled_params(request_id, reason)),
    }
}

/// A JSON-RPC result reply frame (to a server-initiated request).
pub fn result_response(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "result": result,
    })
}

/// A JSON-RPC error reply frame (to a server-initiated request).
pub fn error_response(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": { "code": code, "message": message },
    })
}

pub fn tools_list_params() -> serde_json::Value {
    serde_json::json!({})
}

pub fn tools_call_params(name: &str, arguments: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "arguments": arguments,
    })
}

/// An MCP tool descriptor from tools/list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<serde_json::Value>,
}

pub fn resources_list_params() -> serde_json::Value {
    serde_json::json!({})
}

pub fn resources_read_params(uri: &str) -> serde_json::Value {
    serde_json::json!({
        "uri": uri,
    })
}

/// An MCP resource descriptor from resources/list (design §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpResourceDescriptor {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
}

/// The typed resources/list result (§4.1). `next_cursor` is RETAINED
/// (additive later) but NEVER followed in v1 — one `resources/list` call,
/// one page; its presence marks the served page truncated.
#[derive(Debug, Clone, PartialEq)]
pub struct McpResourceListResult {
    pub resources: Vec<McpResourceDescriptor>,
    pub next_cursor: Option<String>,
}

impl McpResourceListResult {
    /// Truncation report (§4.1): the server offered a continuation page that
    /// v1 deliberately does not fetch.
    pub fn truncated(&self) -> bool {
        self.next_cursor.is_some()
    }
}

/// One content entry of a resources/read result (§4.1). v1 carries TEXT
/// only — a blob (or any non-text entry) is refused typed by the client
/// before anything crosses into the agent path (§4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
    pub text: String,
}

/// The typed resources/read result (text contents only; see above).
#[derive(Debug, Clone, PartialEq)]
pub struct McpResourceReadResult {
    pub contents: Vec<McpResourceContent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape_matches_mcp() {
        let req = JsonRpcRequest::new(1, "initialize", Some(initialize_params()));
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["method"], "initialize");
        assert_eq!(json["params"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(json["params"]["clientInfo"]["name"], "wayland-nano");
    }

    #[test]
    fn response_result_vs_error() {
        let ok = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            result: Some(serde_json::json!({"tools": []})),
            error: None,
        };
        assert!(ok.into_result().is_ok());

        let bad = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            result: None,
            error: Some(JsonRpcError {
                code: -32002,
                message: "invalid bearer token".into(),
                data: None,
            }),
        };
        assert!(bad.into_result().is_err());
    }

    #[test]
    fn resources_param_shapes() {
        assert_eq!(resources_list_params(), serde_json::json!({}));
        assert_eq!(
            resources_read_params("mem://alpha"),
            serde_json::json!({"uri": "mem://alpha"})
        );
    }

    #[test]
    fn resource_descriptor_serde_discipline() {
        let full: McpResourceDescriptor = serde_json::from_value(serde_json::json!({
            "uri": "mem://alpha",
            "name": "alpha",
            "description": "first",
            "mimeType": "text/plain",
        }))
        .unwrap();
        assert_eq!(full.uri, "mem://alpha");
        assert_eq!(full.name, "alpha");
        assert_eq!(full.description.as_deref(), Some("first"));
        assert_eq!(full.mime_type.as_deref(), Some("text/plain"));
        assert_eq!(
            serde_json::to_value(&full).unwrap()["mimeType"],
            "text/plain"
        );
        // Optional fields are genuinely optional.
        let bare: McpResourceDescriptor =
            serde_json::from_value(serde_json::json!({"uri": "mem://beta", "name": "beta"}))
                .unwrap();
        assert_eq!(bare.description, None);
        assert_eq!(bare.mime_type, None);
    }

    #[test]
    fn resource_list_result_truncation_report() {
        let page = McpResourceListResult {
            resources: Vec::new(),
            next_cursor: None,
        };
        assert!(!page.truncated());
        let truncated = McpResourceListResult {
            resources: Vec::new(),
            next_cursor: Some("page-2".into()),
        };
        assert!(truncated.truncated());
    }
}
