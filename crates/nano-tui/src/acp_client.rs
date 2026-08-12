//! Hand-rolled ACP (JSON-RPC over stdio) client subset (design doc §2):
//! initialize, session/new, session/load, session/prompt, session/cancel,
//! session/set_model, session/update, session/request_permission.
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

use serde_json::Value;
use serde_json::json;
use tokio::sync::mpsc;

pub const ACP_PROTOCOL_VERSION: u32 = 1;

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
    },
    /// Context-compaction lifecycle notice (C1 §7): begin/complete/cancel,
    /// rendered as a system note in the transcript.
    Compaction {
        status: String,
    },
    /// Forward-additive: kinds v1 doesn't render. Tolerated, never panics
    /// (torn/unknown replay frames must not kill the TUI, design §8).
    Unknown(String),
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
        error: Option<String>,
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
                Inbound::Response {
                    id,
                    result: None,
                    error: Some(message.to_string()),
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
        },
        "compaction" => SessionUpdate::Compaction {
            status: update
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
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

pub fn prompt_params(session_id: &str, text: &str) -> Value {
    json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]})
}

pub fn set_model_params(session_id: &str, model_id: &str) -> Value {
    json!({"sessionId": session_id, "modelId": model_id})
}

/// session/compact (C1): manual engine-side compaction of the session
/// context, journaled identically to the auto path.
pub fn compact_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
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

        let compact = request(6, "session/compact", compact_params("s1"));
        assert_eq!(compact["params"]["sessionId"], "s1");

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
                error: Some("bad".into()),
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
}
