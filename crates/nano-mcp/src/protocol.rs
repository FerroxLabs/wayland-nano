//! JSON-RPC 2.0 protocol types for MCP.

use serde::Deserialize;
use serde::Serialize;

pub const JSONRPC_VERSION: &str = "2.0";
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
pub const CLIENT_NAME: &str = "wayland-nano";
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}
