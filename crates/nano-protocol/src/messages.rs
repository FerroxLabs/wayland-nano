//! Protocol message types: host commands and engine events.

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: u32 = 1;
pub const ENGINE_ID: &str = "nanok3";

/// Host → engine commands (stdin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Message {
        msg_id: String,
        content: String,
    },
    Stop,
    Ping,
    ToolApprove {
        call_id: String,
    },
    ToolDeny {
        call_id: String,
    },
    Shutdown,
}

/// Engine → host events (stdout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Ready {
        engine: String,
        protocol_version: u32,
        runtime_version: String,
        session_id: String,
        capabilities: Capabilities,
    },
    StreamStart {
        msg_id: String,
    },
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolRequest {
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolRunning {
        call_id: String,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        output: String,
    },
    ToolCancelled {
        call_id: String,
    },
    ApprovalRequired {
        call_id: String,
        name: String,
        reason: String,
    },
    StreamEnd {
        msg_id: String,
        stop_reason: String,
        usage: Option<UsageFrame>,
    },
    Error {
        message: String,
        recoverable: bool,
    },
    Pong,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageFrame {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

/// Honest capability advertisement. Forward-additive only; absences are
/// explicit so the host never assumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub files: bool,
    pub shell: bool,
    pub streaming: bool,
    pub thinking: bool,
    pub approvals: bool,
    pub mcp: bool,
    pub skills: bool,
    pub subagents: u32,
    /// Explicit absences (Desktop must show these as unavailable).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
    /// Extension metadata for future capability keys.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_round_trips() {
        let cmd = Command::Message {
            msg_id: "m1".into(),
            content: "fix it".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"message\""));
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cmd);

        let stop: Command = serde_json::from_str(r#"{"type":"stop"}"#).unwrap();
        assert_eq!(stop, Command::Stop);
    }

    #[test]
    fn ready_event_shape() {
        let event = Event::Ready {
            engine: ENGINE_ID.into(),
            protocol_version: PROTOCOL_VERSION,
            runtime_version: "0.1.0".into(),
            session_id: "s1".into(),
            capabilities: Capabilities {
                files: true,
                shell: true,
                streaming: true,
                thinking: true,
                approvals: true,
                mcp: false,
                skills: false,
                subagents: 0,
                unavailable: vec!["mcp".into(), "skills".into(), "evolution".into()],
                extensions: BTreeMap::new(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ready\""));
        assert!(json.contains("\"engine\":\"nanok3\""));
        assert!(json.contains("\"unavailable\":[\"mcp\",\"skills\",\"evolution\"]"));
    }
}
