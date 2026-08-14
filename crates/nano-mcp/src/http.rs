//! Streamable HTTP transport: POST JSON-RPC, parse plain-JSON or SSE-framed
//! responses, track the Mcp-Session-Id header. All traffic flows through
//! nano-egress (policy-gated) — never a raw reqwest client.

use crate::client::McpError;
use crate::protocol::JsonRpcResponse;
use nano_egress::client::EgressClient;
use std::io::{BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use crate::stdio::{HttpChild, TransportChild, TransportParts};

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
    pub fn endpoint_origin(endpoint: &str) -> Result<String, McpError> {
        let parsed = reqwest::Url::parse(endpoint)
            .map_err(|_| McpError::Protocol("http_endpoint_invalid".to_string()))?;
        let origin = parsed.origin().ascii_serialization();
        if origin == "null" {
            return Err(McpError::Protocol(
                "http_endpoint_has_no_origin".to_string(),
            ));
        }
        Ok(origin)
    }

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
        let text = self
            .exchange(body)
            .await?
            .ok_or_else(|| McpError::Protocol("http_response_missing_for_request".to_string()))?;
        parse_response_body(&text)
    }

    async fn exchange(&mut self, body: &str) -> Result<Option<String>, McpError> {
        let response = self
            .build_request()?
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| McpError::Transport(nano_egress::client::sanitize_transport_error(&e)))?;

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
            .map_err(|e| McpError::Transport(nano_egress::client::sanitize_transport_error(&e)))?;
        if status == 202 || status == 204 {
            return Ok(None);
        }
        if status != 200 {
            return Err(McpError::Transport(format!("http_status_{status}")));
        }
        Ok(Some(text))
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Adapts streamable HTTP to the dispatcher's locked pipe contract. The
    /// pump owns both the async runtime and this transport; no raw HTTP client
    /// can bypass nano-egress.
    pub fn into_parts(self) -> Result<TransportParts, McpError> {
        let (request_tx, request_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let (response_tx, response_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let exited = Arc::new(AtomicBool::new(false));
        let pump_exited = Arc::clone(&exited);
        std::thread::Builder::new()
            .name("nano-mcp-http-pump".to_string())
            .spawn(move || {
                http_pump(self, request_rx, response_tx, shutdown_rx);
                pump_exited.store(true, Ordering::Release);
            })
            .map_err(|_| McpError::Transport("http_pump_spawn_failed".to_string()))?;
        Ok(TransportParts {
            child: TransportChild::Http(HttpChild {
                shutdown: shutdown_tx,
                exited,
            }),
            stdin: Box::new(ChannelWriter {
                tx: request_tx,
                pending: Vec::new(),
            }),
            stdout: BufReader::new(Box::new(ChannelReader {
                rx: response_rx,
                current: std::io::Cursor::new(Vec::new()),
            })),
        })
    }
}

fn http_pump(
    mut transport: HttpTransport,
    requests: Receiver<Vec<u8>>,
    responses: SyncSender<Vec<u8>>,
    shutdown: Receiver<()>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    loop {
        if shutdown.try_recv().is_ok() {
            return;
        }
        match requests.recv_timeout(Duration::from_millis(25)) {
            Ok(frame) => {
                let Ok(body) = String::from_utf8(frame) else {
                    return;
                };
                match runtime.block_on(transport.exchange(body.trim_end())) {
                    Ok(Some(text)) => {
                        let payload = if text.trim_start().starts_with('{') {
                            text
                        } else {
                            match sse_payloads(&text).last() {
                                Some(payload) => payload.clone(),
                                None => return,
                            }
                        };
                        let mut framed = payload.into_bytes();
                        framed.push(b'\n');
                        if responses.send(framed).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn sse_payloads(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            line.strip_prefix("data:")
                .map(str::trim)
                .map(str::to_string)
        })
        .collect()
}

struct ChannelWriter {
    tx: SyncSender<Vec<u8>>,
    pending: Vec<u8>,
}
impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let frame: Vec<u8> = self.pending.drain(..=end).collect();
            self.tx.send(frame).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "HTTP pump closed")
            })?;
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct ChannelReader {
    rx: Receiver<Vec<u8>>,
    current: std::io::Cursor<Vec<u8>>,
}
impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.current.position() as usize == self.current.get_ref().len() {
            self.current = std::io::Cursor::new(
                self.rx
                    .recv()
                    .map_err(|_| std::io::ErrorKind::UnexpectedEof)?,
            );
        }
        self.current.read(buf)
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
        return Err(McpError::Protocol(
            "response_format_invalid: expected JSON or SSE data".to_string(),
        ));
    };
    serde_json::from_str(&payload).map_err(|e| McpError::Protocol(format!("bad SSE payload: {e}")))
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
        std::env::var("FLUX_TEST_KEY")
            .ok()
            .filter(|k| !k.is_empty())
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
