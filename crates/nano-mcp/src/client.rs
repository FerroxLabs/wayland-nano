//! MCP client facade over the full-duplex dispatcher [`Connection`]
//! (P3 design note §2.2): mint id → insert pending oneshot → enqueue on the
//! writer's normal lane → wait on the oneshot with an absolute deadline.
//!
//! `McpClient` is `Clone` (an `Arc` over the connection) and every call
//! takes `&self`: callers lock-to-clone the client and never hold a
//! registry lock across a blocking wire wait (the §2.1.5 defect — the
//! registry-side change lands in the nano-agent lane).
//!
//! Reconnect-once is GONE (§2.3): a poisoned connection returns the same
//! typed error to every subsequent call without touching the child;
//! reconnect policy is a registry decision (§15), not a transport secret.

use crate::dispatcher::{
    Connection, ConnectionOptions, DeadlineCell, PendingSlot, ServerRequestHandler,
};
use crate::http::HttpTransport;
use crate::protocol::{
    JsonRpcNotification, JsonRpcRequest, McpResourceContent, McpResourceDescriptor,
    McpResourceListResult, McpResourceReadResult, McpToolDescriptor, NegotiatedCapabilities,
    cancelled_notification, initialize_params, initialize_params_with_elicitation,
    resources_list_params, resources_read_params, tools_call_params, tools_list_params,
};
use crate::stdio::StdioTransport;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("server error {code}: {message}")]
    Server { code: i64, message: String },
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("timeout after {0}ms")]
    Timeout(u64),
    #[error("output exceeded bound ({0} bytes)")]
    OutputBounded(usize),
    #[error("egress: {0}")]
    Egress(nano_egress::client::EgressError),
    /// Turn cancellation (§2.4): cancel is terminal; the late response is
    /// dropped via the retired-id arm. Maps to `NanoErrorKind::UserCancelled`
    /// — the error-map arm lands with the integrator (nano-agent lane).
    #[error("cancelled")]
    Cancelled,
    /// §4.2: the negotiated capabilities lack `resources` — typed refusal
    /// BEFORE any wire write.
    #[error("server does not support resources")]
    ResourceUnsupported,
    /// §4.3: blob / non-text resource content is refused at this layer (P2b
    /// owns binary-to-model content); nothing crosses into the agent path.
    #[error("non-text resource content refused")]
    ContentUnsupported,
    /// S5 Leg B: unix stdio containment is mandatory — no raw-Command
    /// fallback. Display keeps the `SANDBOX_UNAVAILABLE:` prefix so existing
    /// assertions and log greps keep matching; maps to
    /// `NanoErrorKind::SandboxUnavailable` (fail-closed, never downgraded).
    #[error("SANDBOX_UNAVAILABLE: {0}")]
    SandboxUnavailable(String),
}

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_OUTPUT_BYTES: usize = 512 * 1024;

/// Poll slice for the caller wait loop: cancel flags are observed within
/// one slice, and a one-time deadline extension (§2.4 rule 1) takes effect
/// at the next slice boundary.
const CANCEL_POLL: Duration = Duration::from_millis(25);

#[derive(Clone)]
pub struct McpClient {
    conn: Arc<Connection>,
    timeout: Duration,
    cached_tools: Arc<Vec<McpToolDescriptor>>,
}

impl McpClient {
    pub fn connect_http(transport: HttpTransport) -> Result<Self, McpError> {
        Self::connect_http_with_options(transport, ConnectionOptions::default())
    }

    pub fn connect_http_with_options(
        transport: HttpTransport,
        options: ConnectionOptions,
    ) -> Result<Self, McpError> {
        let conn = Connection::spawn(transport.into_parts()?, options);
        let client = Self {
            conn,
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            cached_tools: Arc::new(Vec::new()),
        };
        client.initialize()?;
        let tools = client.list_tools_inner()?;
        Ok(Self {
            cached_tools: Arc::new(tools),
            ..client
        })
    }

    /// Spawns a server, performs the initialize handshake, records the
    /// negotiated capabilities (§2.7), and caches tools/list once (§2.7
    /// kills the duplicate list at the registry).
    pub fn connect(
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, McpError> {
        Self::connect_with_options(command, args, env, ConnectionOptions::default())
    }

    pub fn connect_with_options(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        options: ConnectionOptions,
    ) -> Result<Self, McpError> {
        let transport = StdioTransport::spawn(command, args, env)?;
        let conn = Connection::spawn(transport.into_parts(), options);
        let client = Self {
            conn,
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            cached_tools: Arc::new(Vec::new()),
        };
        client.initialize()?;
        let tools = client.list_tools_inner()?;
        Ok(Self {
            cached_tools: Arc::new(tools),
            ..client
        })
    }

    /// The tools cached at connect time (initialize + tools/list).
    pub fn cached_tools(&self) -> &[McpToolDescriptor] {
        &self.cached_tools
    }

    /// The recorded initialize-time capabilities (§2.7) — the enforcing
    /// record for the §4.2/§5.5 gates (resources/elicitation lanes).
    pub fn negotiated(&self) -> Option<NegotiatedCapabilities> {
        self.conn.negotiated().cloned()
    }

    /// Mutable-call alias kept source-compatible with the current registry
    /// (`mcp.rs:157`); the dispatcher client needs no `&mut`.
    pub fn call_tool_mutable(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        self.call_tool(name, arguments)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Same typed error every call returns after poison, without touching
    /// the child (§2.3).
    pub fn poisoned_reason(&self) -> Option<String> {
        self.conn.poisoned_reason()
    }

    /// §2.2 violation counter (bad lines + impossible ids; poison at 8).
    pub fn violations(&self) -> usize {
        self.conn.violations()
    }

    /// `-32603` overflow replies emitted for a flooded server-request queue.
    pub fn overflow_replies(&self) -> u64 {
        self.conn.overflow_replies()
    }

    fn initialize(&self) -> Result<(), McpError> {
        // The elicitation client capability is advertised ONLY when a
        // handler is installed (§2.7/§5.5 — the honesty rule on the
        // handshake); stdio-only, and this is the stdio facade.
        let advertised = self.conn.advertises_elicitation();
        let params = if advertised {
            initialize_params_with_elicitation()
        } else {
            initialize_params()
        };
        let result = self.request("initialize", Some(params), None)?;
        if result.get("protocolVersion").is_none() {
            return Err(McpError::Protocol(
                "initialize response missing protocolVersion".into(),
            ));
        }
        self.conn
            .set_negotiated(NegotiatedCapabilities::from_initialize_result(
                &result, advertised,
            ));
        let notification = JsonRpcNotification::initialized();
        self.conn.enqueue_normal(
            serde_json::to_string(&notification).expect("notification serializes"),
        )?;
        Ok(())
    }

    /// The caller-side round trip (§2.2): mint id → insert pending oneshot
    /// → enqueue → `recv_timeout` against the absolute deadline. The
    /// deadline is re-read every slice so the ONE granted extension (§2.4
    /// rule 1) takes effect; server traffic can never move it again.
    fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
        cancel: Option<&AtomicBool>,
    ) -> Result<serde_json::Value, McpError> {
        if let Some(reason) = self.conn.poisoned_reason() {
            return Err(McpError::Transport(reason));
        }
        let id = self.conn.mint_id();
        let (tx, rx) = mpsc::sync_channel::<Result<serde_json::Value, McpError>>(1);
        let deadline = Arc::new(DeadlineCell::new(Instant::now() + self.timeout));
        self.conn.pending_insert(
            id,
            PendingSlot {
                tx,
                deadline: deadline.clone(),
            },
        );
        // Designation BEFORE the frame goes on the wire (§2.4 rule 2): at
        // most one designated foreground call per connection.
        if method == "tools/call" && self.conn.negotiated().is_some_and(|n| n.elicitation) {
            self.conn.designate_slot(id);
        }
        let frame = serde_json::to_string(&JsonRpcRequest::new(id, method, params))
            .expect("request serializes");
        if let Err(err) = self.conn.enqueue_normal(frame) {
            self.conn.pending_retire(id);
            return Err(err);
        }
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                // Turn cancellation (§2.4): terminal; the late response is
                // dropped via the retired-id arm.
                self.conn.pending_retire(id);
                self.send_cancel(id, "cancelled");
                return Err(McpError::Cancelled);
            }
            let now = Instant::now();
            let wait_until = deadline.deadline();
            if now >= wait_until {
                // Caller timeout (§2.4): retire, cancel best-effort on the
                // priority lane, typed Timeout.
                self.conn.pending_retire(id);
                self.send_cancel(id, "timeout");
                return Err(McpError::Timeout(self.timeout.as_millis() as u64));
            }
            match rx.recv_timeout((wait_until - now).min(CANCEL_POLL)) {
                Ok(message) => return message,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    self.conn.pending_retire(id);
                    return Err(McpError::Transport("pending waiter dropped".into()));
                }
            }
        }
    }

    /// `notifications/cancelled` on the priority lane, best-effort (§2.4):
    /// the lane is RESERVED for cancels; if it is full the connection is
    /// already broken and `enqueue_priority` poisons it per §2.2.
    fn send_cancel(&self, id: u64, reason: &str) {
        let notification = cancelled_notification(serde_json::json!(id), reason);
        let _ = self.conn.enqueue_priority(
            serde_json::to_string(&notification).expect("notification serializes"),
        );
    }

    fn list_tools_inner(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let result = self.request("tools/list", Some(tools_list_params()), None)?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for tool in tools {
            out.push(
                serde_json::from_value(tool)
                    .map_err(|e| McpError::Protocol(format!("bad tool descriptor: {e}")))?,
            );
        }
        Ok(out)
    }

    /// Kept `&mut self` for source compatibility with the current registry
    /// (`mcp.rs:50-56`); the dispatcher needs no `&mut`.
    pub fn list_tools(&mut self) -> Result<Vec<McpToolDescriptor>, McpError> {
        self.list_tools_inner()
    }

    pub fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        self.call_tool_inner(name, arguments, None)
    }

    /// Turn-cancellable call (§2.4): when `cancel` is set, the request is
    /// retired, `notifications/cancelled {reason:"cancelled"}` goes out
    /// best-effort, and the caller gets typed `Cancelled`.
    pub fn call_tool_cancellable(
        &self,
        name: &str,
        arguments: serde_json::Value,
        cancel: &AtomicBool,
    ) -> Result<serde_json::Value, McpError> {
        self.call_tool_inner(name, arguments, Some(cancel))
    }

    fn call_tool_inner(
        &self,
        name: &str,
        arguments: serde_json::Value,
        cancel: Option<&AtomicBool>,
    ) -> Result<serde_json::Value, McpError> {
        let result = self.request(
            "tools/call",
            Some(tools_call_params(name, arguments)),
            cancel,
        )?;
        let encoded = serde_json::to_vec(&result)
            .map_err(|e| McpError::Protocol(format!("encode result: {e}")))?;
        if encoded.len() > MAX_OUTPUT_BYTES {
            return Err(McpError::OutputBounded(encoded.len()));
        }
        Ok(result)
    }

    /// §4.2 capability gate: absent `resources` in the negotiated
    /// capabilities ⇒ typed refusal BEFORE any wire write (fail-closed when
    /// the handshake record is missing too).
    fn require_resources(&self) -> Result<(), McpError> {
        if self.conn.negotiated().is_some_and(|n| n.resources) {
            Ok(())
        } else {
            Err(McpError::ResourceUnsupported)
        }
    }

    /// The same MAX_OUTPUT_BYTES bound as `call_tool_inner`, enforced at the
    /// same layer (§4.1).
    fn bound_output(result: serde_json::Value) -> Result<serde_json::Value, McpError> {
        let encoded = serde_json::to_vec(&result)
            .map_err(|e| McpError::Protocol(format!("encode result: {e}")))?;
        if encoded.len() > MAX_OUTPUT_BYTES {
            return Err(McpError::OutputBounded(encoded.len()));
        }
        Ok(result)
    }

    /// resources/list over the dispatcher (§4.1): the same pending-map
    /// round trip as every request. NO pagination in v1 — one call, one
    /// page; a `nextCursor` marks the page truncated (retained, reported,
    /// never followed).
    pub fn list_resources(&self) -> Result<McpResourceListResult, McpError> {
        self.require_resources()?;
        let result = Self::bound_output(self.request(
            "resources/list",
            Some(resources_list_params()),
            None,
        )?)?;
        let mut resources = Vec::new();
        for entry in result
            .get("resources")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default()
        {
            resources.push(
                serde_json::from_value::<McpResourceDescriptor>(entry)
                    .map_err(|e| McpError::Protocol(format!("bad resource descriptor: {e}")))?,
            );
        }
        let next_cursor = result
            .get("nextCursor")
            .and_then(|c| c.as_str())
            .map(str::to_string);
        if next_cursor.is_some() {
            eprintln!(
                "wayland-nano mcp: resources/list page truncated (nextCursor present; not followed in v1)"
            );
        }
        Ok(McpResourceListResult {
            resources,
            next_cursor,
        })
    }

    /// resources/read over the dispatcher (§4.1/§4.3): bounded at
    /// MAX_OUTPUT_BYTES like a tool call; blob / non-text content entries
    /// are a typed refusal — no resource content crosses into the agent
    /// path.
    pub fn read_resource(&self, uri: &str) -> Result<McpResourceReadResult, McpError> {
        self.require_resources()?;
        let result = Self::bound_output(self.request(
            "resources/read",
            Some(resources_read_params(uri)),
            None,
        )?)?;
        let mut contents = Vec::new();
        for entry in result
            .get("contents")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default()
        {
            if entry.get("blob").is_some() || entry.get("text").and_then(|t| t.as_str()).is_none() {
                return Err(McpError::ContentUnsupported);
            }
            contents.push(
                serde_json::from_value::<McpResourceContent>(entry)
                    .map_err(|e| McpError::Protocol(format!("bad resource content: {e}")))?,
            );
        }
        Ok(McpResourceReadResult { contents })
    }

    /// Graceful shutdown (§2.6): refuse new requests, cancel pending ids on
    /// the wire, close stdin, bounded wait, supervisor terminates the child
    /// and joins the threads. Also runs on drop of the last clone.
    pub fn close(self) {
        self.conn.shutdown();
    }
}

// Compile-time pin: the facade is clone-cheap and shareable across threads
// — the registry lane's lock-to-clone fix (§2.1.5) depends on it.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<McpClient>();
    assert_send_sync::<Arc<dyn ServerRequestHandler>>();
};
