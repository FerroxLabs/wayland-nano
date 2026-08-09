//! Streamable HTTP transport: POST JSON-RPC, parse plain-JSON or SSE-framed
//! responses, track the Mcp-Session-Id header. All traffic flows through
//! nano-egress (policy-gated) — never a raw reqwest client.

use crate::client::McpError;
use crate::protocol::JsonRpcResponse;
use nano_egress::client::EgressClient;

pub struct HttpTransport {
    egress: EgressClient,
    endpoint: String,
    session_id: Option<String>,
    auth: AuthHeader,
}

#[derive(Clone)]
pub enum AuthHeader {
    Bearer(String),
    XApiKey(String),
    None,
}

impl HttpTransport {
    pub fn new(egress: EgressClient, endpoint: impl Into<String>, auth: AuthHeader) -> Self {
        Self {
            egress,
            endpoint: endpoint.into(),
            session_id: None,
            auth,
        }
    }

    fn build_request(&self) -> Result<reqwest::RequestBuilder, McpError> {
        let builder = self
            .egress
            .request(reqwest::Method::POST, &self.endpoint)
            .map_err(McpError::Egress)?;
        let builder = builder
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        let builder = match &self.auth {
            AuthHeader::Bearer(token) => builder.bearer_auth(token),
            AuthHeader::XApiKey(key) => builder.header("x-api-key", key),
            AuthHeader::None => builder,
        };
        if let Some(session) = &self.session_id {
            Ok(builder.header("Mcp-Session-Id", session.clone()))
        } else {
            Ok(builder)
        }
    }

    pub async fn round_trip(&mut self, body: &str) -> Result<JsonRpcResponse, McpError> {
        let response = self
            .build_request()?
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| McpError::Transport(e.to_string()))?;

        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(session.to_string());
        }

        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| McpError::Transport(e.to_string()))?;
        if status != 200 {
            return Err(McpError::Transport(format!("http {status}: {text}")));
        }

        parse_response_body(&text)
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}

/// Parses either a plain JSON response or an SSE-framed one
/// (`event: message\ndata: {...}`), as Flux's /mcp/ surface emits.
pub fn parse_response_body(text: &str) -> Result<JsonRpcResponse, McpError> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed)
            .map_err(|e| McpError::Protocol(format!("bad json response: {e}")));
    }
    // SSE frames: collect data lines and take the last JSON payload.
    let mut payload: Option<String> = None;
    for line in trimmed.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            payload = Some(rest.trim().to_string());
        }
    }
    let Some(payload) = payload else {
        return Err(McpError::Protocol(format!(
            "response is neither JSON nor SSE data: {}",
            &trimmed[..trimmed.len().min(200)]
        )));
    };
    serde_json::from_str(&payload)
        .map_err(|e| McpError::Protocol(format!("bad SSE payload: {e}")))
}

impl From<nano_egress::client::EgressError> for McpError {
    fn from(err: nano_egress::client::EgressError) -> Self {
        McpError::Egress(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_json_response() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26"}}"#;
        let resp = parse_response_body(body).unwrap();
        assert_eq!(resp.id, serde_json::json!(1));
    }

    #[test]
    fn parses_sse_framed_response_like_flux() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-03-26\"}}\n\n";
        let resp = parse_response_body(body).unwrap();
        let result = resp.into_result().unwrap();
        assert_eq!(result["protocolVersion"], "2025-03-26");
    }

    #[test]
    fn rejects_neither_json_nor_sse() {
        assert!(parse_response_body("<html>404</html>").is_err());
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::protocol::{JsonRpcRequest, initialize_params, tools_list_params};

    fn key() -> Option<String> {
        std::env::var("FLUX_TEST_KEY").ok().filter(|k| !k.is_empty())
    }

    /// Live against api.fluxrouter.ai/mcp/ (trailing slash matters — the
    /// fixture discovery: /mcp without slash 401s with a misleading error).
    #[tokio::test]
    async fn live_flux_mcp_handshake_and_tools_list() {
        let Some(key) = key() else {
            eprintln!("FLUX_TEST_KEY not set — skipping live MCP test");
            return;
        };
        let egress = nano_egress::client::EgressClient::flux();
        let mut transport = HttpTransport::new(
            egress,
            "https://api.fluxrouter.ai/mcp/",
            AuthHeader::Bearer(key),
        );

        let init = JsonRpcRequest::new(1, "initialize", Some(initialize_params()));
        let response = transport
            .round_trip(&serde_json::to_string(&init).unwrap())
            .await
            .expect("initialize");
        let result = response.into_result().expect("initialize result");
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert!(
            transport.session_id().is_some(),
            "server must issue Mcp-Session-Id"
        );

        let list = JsonRpcRequest::new(2, "tools/list", Some(tools_list_params()));
        let response = transport
            .round_trip(&serde_json::to_string(&list).unwrap())
            .await
            .expect("tools/list");
        let result = response.into_result().expect("tools/list result");
        assert!(
            result.get("tools").is_some(),
            "tools/list must return a tools array (empty catalog is the known current state)"
        );
    }
}
