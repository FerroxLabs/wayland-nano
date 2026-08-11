//! Protocol message types: host commands and engine events, in the
//! `wayland-desktop-core` v1 wire shapes for the Nano-supported subset.
//!
//! Profile rule: Nano speaks the corpus shapes for its supported lifecycle
//! (turn/stream/tool/approval/heartbeat) and fails closed on everything else
//! (unknown event types are Malformed, never panics or misroutes).

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: u32 = 1;
pub const ENGINE_ID: &str = "wayland-nano";
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Host → engine commands (stdin), corpus shapes for the supported subset;
/// unsupported corpus commands are handled as tolerated errors upstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Message {
        msg_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
    },
    Stop,
    Ping,
    ToolApprove {
        call_id: String,
    },
    ToolDeny {
        call_id: String,
    },
    ApprovalResume {
        approved: bool,
        resume_token: String,
    },
    Shutdown,
}

/// Engine → host events (stdout), `wayland-desktop-core` v1 wire shapes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ready {
        capabilities: NanoCapabilities,
        session_id: String,
        version: String,
    },
    StreamStart {
        msg_id: String,
    },
    TextDelta {
        msg_id: String,
        text: String,
    },
    Thinking {
        msg_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        text: String,
    },
    ToolRequest {
        call_id: String,
        msg_id: String,
        tool: ToolRequestBody,
    },
    ToolRunning {
        call_id: String,
        msg_id: String,
        tool_name: String,
    },
    ToolResult {
        call_id: String,
        msg_id: String,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_type: Option<String>,
        status: String,
        tool_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<BTreeMap<String, serde_json::Value>>,
    },
    ToolCancelled {
        call_id: String,
        msg_id: String,
        reason: String,
    },
    ApprovalRequired {
        call_id: String,
        reason: String,
        resume_token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        correlation_id: Option<String>,
    },
    Suspend {
        reason: String,
        resume_token: String,
    },
    ApprovalResume {
        approved: bool,
        resume_token: String,
    },
    Info {
        msg_id: String,
        message: String,
    },
    Error {
        error: ErrorBody,
        msg_id: String,
    },
    StreamEnd {
        finish_reason: String,
        msg_id: String,
        usage: UsageFrame,
        usage_delta: UsageFrame,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_run_id: Option<String>,
    },
    Pong,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolRequestBody {
    pub name: String,
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageFrame {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// Honest Nano capability advertisement in the corpus capabilities shape.
/// Forward-additive only; orchestration surfaces are false, never omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NanoCapabilities {
    pub cost_attribution: bool,
    pub mcp: bool,
    pub memory_enabled: bool,
    pub plugins: bool,
    pub streaming_tools: bool,
    pub structured_traces: bool,
    pub sub_agent_traces: bool,
    pub thinking: bool,
    pub tool_approval: bool,
    pub browser_suite: bool,
    pub computer_use: bool,
    pub modes: Vec<String>,
    pub current_mode: String,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_fixtures_parse_for_supported_subset() {
        let ready: serde_json::Value = serde_json::from_str(include_str!(
            "../corpus/wayland-desktop-core/v1/events/ready.json"
        ))
        .unwrap();
        assert_eq!(ready["type"], "ready");
        // Nano reads the shape; it must not require the full capability set.
        let caps = &ready["capabilities"];
        assert!(caps.get("thinking").is_some());

        let tool_request: serde_json::Value = serde_json::from_str(include_str!(
            "../corpus/wayland-desktop-core/v1/events/tool_request.json"
        ))
        .unwrap();
        assert_eq!(tool_request["type"], "tool_request");
        assert!(tool_request["tool"]["name"].is_string());
    }

    #[test]
    fn emitted_frames_match_corpus_shapes() {
        let event = Event::ToolRequest {
            call_id: "call-tool-001".into(),
            msg_id: "msg-001".into(),
            tool: ToolRequestBody {
                name: "Bash".into(),
                args: serde_json::json!({"command": "cargo test"}),
                category: Some("exec".into()),
                description: Some("Run the test suite".into()),
            },
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../corpus/wayland-desktop-core/v1/events/tool_request.json"
        ))
        .unwrap();
        assert_eq!(json["type"], corpus["type"]);
        assert_eq!(json["call_id"], corpus["call_id"]);
        assert_eq!(json["msg_id"], corpus["msg_id"]);
        assert_eq!(json["tool"]["name"], corpus["tool"]["name"]);
        assert_eq!(json["tool"]["args"], corpus["tool"]["args"]);
        assert_eq!(json["tool"]["category"], corpus["tool"]["category"]);
    }

    #[test]
    fn command_fixtures_parse() {
        let message: Command = serde_json::from_str(include_str!(
            "../corpus/wayland-desktop-core/v1/commands/message.json"
        ))
        .unwrap();
        assert_eq!(
            message,
            Command::Message {
                msg_id: "msg-001".into(),
                content: "Inspect the current workspace.".into(),
                files: vec!["README.md".into()],
            }
        );
    }
}
