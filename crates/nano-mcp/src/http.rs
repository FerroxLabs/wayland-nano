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

/// Server-controlled response bodies are read BOUNDED at the MCP output
/// bound (`client::MAX_OUTPUT_BYTES`, 512 KiB): `Response::text()` would
/// allocate whatever a hostile endpoint sends before the status check and
/// the JSON/SSE parse. A present Content-Length over the cap fails early;
/// otherwise chunks stream into a capped buffer and overshoot aborts with
/// the typed `McpError::OutputBounded`. Same philosophy as the F-14
/// provider error-body bound (`nano-model flux_common::read_error_body`),
/// sized to the protocol's output bound rather than the error bound.
pub const MAX_HTTP_BODY_BYTES: usize = crate::client::MAX_OUTPUT_BYTES;

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
        let text = read_bounded_body(response).await?;
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

/// Reads a server-controlled response body with the MAX_HTTP_BODY_BYTES
/// cap. A declared Content-Length over the cap aborts before any body
/// byte is read; a chunked/undeclared stream aborts on the first chunk
/// that would overshoot. The typed error carries a byte count only —
/// never body content (the F-P3-12 error-surface discipline).
async fn read_bounded_body(mut response: reqwest::Response) -> Result<String, McpError> {
    if let Some(declared) = response.content_length()
        && declared > MAX_HTTP_BODY_BYTES as u64
    {
        return Err(McpError::OutputBounded(declared as usize));
    }
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => {
                if buf.len() + bytes.len() > MAX_HTTP_BODY_BYTES {
                    return Err(McpError::OutputBounded(buf.len() + bytes.len()));
                }
                buf.extend_from_slice(&bytes);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(McpError::Transport(
                    nano_egress::client::sanitize_transport_error(&e),
                ));
            }
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
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

    // --- F-P3-12: the error surface never carries bodies or credentials ----

    /// Reads one full request (head + Content-Length body) from `stream`.
    /// Draining the request keeps the response close FIN-ordered, never RST.
    fn drain_request(stream: &mut std::net::TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut need = usize::MAX;
        loop {
            let n = stream.read(&mut chunk).expect("read");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if need == usize::MAX
                && let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let head = String::from_utf8_lossy(&buf[..end]);
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(String::from)
                    })
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                need = end + 4 + len;
            }
            if buf.len() >= need {
                break;
            }
        }
    }

    /// Read the request, then answer with a fixed Content-Length body.
    fn serve_once(listener: std::net::TcpListener, status: &str, body: String) {
        let (mut stream, _) = listener.accept().expect("accept");
        drain_request(&mut stream);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("write");
        stream.flush().expect("flush");
    }

    /// Read the request, then stream `count` copies of `chunk` as a chunked
    /// response (no Content-Length) of `content_type`. Write failures after
    /// the client aborts on the cap are expected and ignored.
    fn serve_chunked(
        listener: std::net::TcpListener,
        content_type: &str,
        chunk: &[u8],
        count: usize,
    ) {
        let (mut stream, _) = listener.accept().expect("accept");
        drain_request(&mut stream);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).expect("write head");
        for _ in 0..count {
            let frame = format!("{:x}\r\n", chunk.len());
            let write = stream
                .write_all(frame.as_bytes())
                .and_then(|_| stream.write_all(chunk))
                .and_then(|_| stream.write_all(b"\r\n"));
            if write.is_err() {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n");
    }

    fn loopback_egress() -> EgressClient {
        let policy = nano_egress::policy::EgressPolicy::new().allow_host_with_http("127.0.0.1");
        EgressClient::new(policy)
    }

    /// F-P3-12 regression pin: `McpError::Transport` reaches model-visible
    /// tool results (nano-agent `resource_error_of_mcp` stringifies it), so
    /// it must carry neither the response body, nor credentials, nor the
    /// full request URL — per the nano-egress redaction discipline
    /// (`sanitize_transport_error`).
    #[tokio::test]
    async fn http_error_surface_carries_no_body_or_credentials() {
        let body_marker = "FAKE-SECRET-MARKER-7c1d9e";
        let bearer_marker = "FAKE-BEARER-MARKER-3b8a41";
        let query_marker = "FAKE-QUERY-MARKER-51f0c2";
        let userinfo_marker = "FAKE-USERINFO-MARKER-a94d07";

        // Leg A: a non-200 status carries NO body — the marker and the
        // 64 KiB pad in the response body must not reach the error, and the
        // presented bearer credential must not either.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let body = format!("{{\"error\":\"{body_marker}\"}}{}", "x".repeat(64 * 1024));
        std::thread::spawn(move || serve_once(listener, "500 Internal Server Error", body));
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::Bearer(bearer_marker.to_string()),
        );
        let err = transport
            .round_trip("{}")
            .await
            .expect_err("500 is an error");
        let rendered = err.to_string();
        assert!(!rendered.contains(body_marker), "body leaked: {rendered}");
        assert!(
            !rendered.contains(bearer_marker),
            "credential leaked: {rendered}"
        );
        assert!(
            rendered.len() <= 256,
            "error text stays bounded: {rendered}"
        );
        assert!(rendered.contains("500"), "the status survives: {rendered}");

        // Leg B: a 200 whose body is garbage — the parse failure names the
        // defect class, never the body.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let body = format!("{body_marker}{}", "y".repeat(64 * 1024));
        std::thread::spawn(move || serve_once(listener, "200 OK", body));
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::Bearer(bearer_marker.to_string()),
        );
        let err = transport
            .round_trip("{}")
            .await
            .expect_err("garbage 200 is an error");
        let rendered = err.to_string();
        assert!(!rendered.contains(body_marker), "body leaked: {rendered}");
        assert!(
            !rendered.contains(bearer_marker),
            "credential leaked: {rendered}"
        );

        // Leg C: a reqwest transport failure (connection refused) carries
        // the request URL in its Display — the sanitizer must strip query
        // and userinfo. (A policy denial instead is equally clean: host +
        // hashed path only.)
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let closed_port = closed.local_addr().expect("addr").port();
        drop(closed);
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!(
                "http://user:{userinfo_marker}@127.0.0.1:{closed_port}/mcp?session={query_marker}"
            ),
            AuthHeader::None,
        );
        let err = transport
            .round_trip("{}")
            .await
            .expect_err("closed port is an error");
        let rendered = err.to_string();
        assert!(
            !rendered.contains(query_marker),
            "query leaked into the error: {rendered}"
        );
        assert!(
            !rendered.contains(userinfo_marker),
            "userinfo leaked into the error: {rendered}"
        );
    }

    // --- response bodies are read BOUNDED at MAX_HTTP_BODY_BYTES ----------

    /// A declared Content-Length over the cap aborts BEFORE the body is
    /// read: the typed OutputBounded error, no body allocation.
    #[tokio::test]
    async fn http_declared_length_over_cap_aborts_typed_early() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let declared = MAX_HTTP_BODY_BYTES + 1;
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            drain_request(&mut stream);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes()).expect("write head");
            // The client must have aborted on the header; a further body
            // write may already fail — either is fine.
            let _ = stream.write_all(&vec![b'x'; 4096]);
        });
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::None,
        );
        let err = transport
            .round_trip("{}")
            .await
            .expect_err("declared over-cap body is an error");
        assert!(
            matches!(err, McpError::OutputBounded(_)),
            "typed bound error: {err}"
        );
        assert!(
            err.to_string().len() <= 256,
            "error carries a count, never body: {err}"
        );
    }

    /// A chunked stream (no Content-Length) overshooting the cap aborts
    /// with the typed error mid-stream; the connection is cleaned up and
    /// the same transport still round-trips against a healthy server.
    #[tokio::test]
    async fn http_chunked_body_over_cap_aborts_typed_and_transport_recovers() {
        let chunk = vec![b'x'; 64 * 1024];
        let count = MAX_HTTP_BODY_BYTES / chunk.len() + 4; // ~1.25x over cap
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let chunk_body = chunk.clone();
        std::thread::spawn(move || serve_chunked(listener, "application/json", &chunk_body, count));
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::None,
        );
        let err = transport
            .round_trip("{}")
            .await
            .expect_err("streamed over-cap body is an error");
        assert!(
            matches!(err, McpError::OutputBounded(_)),
            "typed bound error: {err}"
        );

        // Connection cleanup: the transport is reusable — a healthy
        // loopback server answers the next round trip.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            serve_once(
                listener,
                "200 OK",
                r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#.to_string(),
            )
        });
        transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::None,
        );
        let resp = transport.round_trip("{}").await.expect("recovery");
        assert_eq!(resp.id, serde_json::json!(1));
    }

    /// The SSE read path is bounded identically: an event stream that
    /// overshoots the cap aborts typed before any SSE parse.
    #[tokio::test]
    async fn http_sse_stream_over_cap_aborts_typed() {
        let chunk = b"data: {\"partial\":true}\n\n".repeat(4096); // ~104 KiB
        let count = MAX_HTTP_BODY_BYTES / chunk.len() + 2;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || serve_chunked(listener, "text/event-stream", &chunk, count));
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::None,
        );
        let err = transport
            .round_trip("{}")
            .await
            .expect_err("over-cap SSE stream is an error");
        assert!(
            matches!(err, McpError::OutputBounded(_)),
            "typed bound error: {err}"
        );
    }

    /// A well-formed chunked SSE response UNDER the cap still parses —
    /// the bounded read path is the only read path, so it must serve the
    /// ordinary Flux framing too.
    #[tokio::test]
    async fn http_chunked_sse_under_cap_parses() {
        let chunk = b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-03-26\"}}\n\n";
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || serve_chunked(listener, "text/event-stream", chunk, 1));
        let mut transport = HttpTransport::new(
            loopback_egress(),
            format!("http://127.0.0.1:{port}/mcp"),
            AuthHeader::None,
        );
        let resp = transport.round_trip("{}").await.expect("under-cap SSE");
        let result = resp.into_result().expect("result");
        assert_eq!(result["protocolVersion"], "2025-03-26");
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
