//! NDJSON codec: one JSON value per line, malformed-tolerant.
//!
//! Rules: exactly one event per stdout line (a raw newline inside a payload
//! would corrupt the stream, so encoding rejects embedded newlines in the
//! serialized form — serde_json escapes them by construction). Malformed
//! input lines yield `ProtocolError::Malformed` for the host loop to convert
//! into `error` frames; they never panic the engine.

use crate::messages::{Command, Event};

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("malformed frame at byte {offset}: {message}")]
    Malformed { offset: u64, message: String },
}

pub fn encode_event(event: &Event) -> String {
    let mut line = serde_json::to_string(event).expect("event serializes");
    debug_assert!(
        !line.contains('\n'),
        "serde_json must escape newlines inside strings"
    );
    line.push('\n');
    line
}

/// Decodes NDJSON command input. Valid lines parse; a malformed FINAL
/// (possibly incomplete) line is held back in `remainder` for the next chunk;
/// a malformed COMPLETE middle line is an error the host loop turns into an
/// `error` frame while continuing to run.
pub fn decode_commands(input: &str) -> (Vec<Result<Command, ProtocolError>>, String) {
    let mut out = Vec::new();
    let mut offset: u64 = 0;
    let mut remainder = String::new();
    let lines: Vec<&str> = input.split('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        let is_last = index == lines.len() - 1;
        let trimmed = line.trim_end_matches('\r');
        if trimmed.is_empty() {
            offset += line.len() as u64 + 1;
            continue;
        }
        if is_last {
            remainder = trimmed.to_string();
            break;
        }
        match serde_json::from_str::<Command>(trimmed) {
            Ok(cmd) => out.push(Ok(cmd)),
            Err(err) => out.push(Err(ProtocolError::Malformed {
                offset,
                message: err.to_string(),
            })),
        }
        offset += line.len() as u64 + 1;
    }
    (out, remainder)
}

/// Decodes a complete buffer (no remainder expected) — used by tests and by
/// line-oriented drivers.
pub fn decode_complete(input: &str) -> Vec<Result<Command, ProtocolError>> {
    let (mut frames, remainder) = decode_commands(input);
    if !remainder.is_empty() {
        match serde_json::from_str::<Command>(&remainder) {
            Ok(cmd) => frames.push(Ok(cmd)),
            Err(err) => frames.push(Err(ProtocolError::Malformed {
                offset: 0,
                message: err.to_string(),
            })),
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{Capabilities, Event, ENGINE_ID, PROTOCOL_VERSION};
    use std::collections::BTreeMap;

    fn ready() -> Event {
        Event::Ready {
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
                unavailable: vec![],
                extensions: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn encode_is_single_line_ndjson() {
        let encoded = encode_event(&ready());
        assert!(encoded.ends_with('\n'));
        assert_eq!(encoded.matches('\n').count(), 1);
        assert!(encoded.len() < 8192, "ready frame must stay small");
    }

    #[test]
    fn decode_valid_and_malformed_mixed_stream() {
        let input = "{\"type\":\"ping\"}\nnot json\n{\"type\":\"stop\"}\n";
        let frames = decode_complete(input);
        assert_eq!(frames.len(), 3);
        assert!(matches!(frames[0], Ok(Command::Ping)));
        assert!(matches!(frames[1], Err(ProtocolError::Malformed { .. })));
        assert!(matches!(frames[2], Ok(Command::Stop)));
    }

    #[test]
    fn partial_final_line_held_as_remainder() {
        let input = "{\"type\":\"ping\"}\n{\"type\":\"mes";
        let (frames, remainder) = decode_commands(input);
        assert_eq!(frames.len(), 1);
        assert_eq!(remainder, "{\"type\":\"mes");
    }

    #[test]
    fn crlf_tolerated() {
        let frames = decode_complete("{\"type\":\"ping\"}\r\n");
        assert_eq!(frames.len(), 1);
        assert!(matches!(frames[0], Ok(Command::Ping)));
    }
}
