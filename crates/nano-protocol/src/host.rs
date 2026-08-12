//! Stdio host loop: reads NDJSON commands, drives turns, emits events.
//!
//! Invariants (Desktop integration contract):
//! - `ready` is emitted FIRST, before any other frame;
//! - malformed input produces an `error` frame and the loop CONTINUES;
//! - `ping` answers `pong` immediately (heartbeats do not affect turns);
//! - stdin close shuts down cleanly with no unterminated frames;
//! - turn events are framed as stream_start → deltas/tool frames → stream_end
//!   so a host-side stall watchdog always sees turn-scoped frames.

use crate::codec::{ProtocolError, decode_commands, encode_event};
use crate::messages::{Command, ErrorBody, Event, NanoCapabilities, UsageFrame};
use crate::profile::v1_capabilities;
#[cfg(test)]
use nano_agent::turn::TurnState;
use nano_model::types::Usage;
use std::io::{BufRead, Write};

pub struct HostConfig {
    pub runtime_version: String,
    pub session_id: String,
    pub capabilities: NanoCapabilities,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            session_id: uuidish(),
            capabilities: v1_capabilities(),
        }
    }
}

fn uuidish() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("wayland-nano-{:x}", t.as_nanos())
}

/// What the host loop concluded with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostExit {
    StdinClosed,
    ShutdownCommand,
    Fatal(String),
}

/// Emits the ready frame first, then processes commands until stdin closes
/// or a shutdown command arrives. Returns the exit reason.
///
/// `turns` is called for each well-formed `message` command with its msg_id
/// and content; the host handles framing (start/end) around it.
pub async fn run_host_loop<R, W, F, Fut>(
    reader: &mut R,
    writer: &mut W,
    config: &HostConfig,
    mut run_turn: F,
) -> std::io::Result<HostExit>
where
    R: BufRead,
    W: Write,
    F: FnMut(String, String) -> Fut,
    Fut: std::future::Future<Output = (Vec<Event>, Option<Usage>, String)>,
{
    write_frame(
        writer,
        &Event::Ready {
            capabilities: config.capabilities.clone(),
            session_id: config.session_id.clone(),
            version: config.runtime_version.clone(),
        },
    )?;

    let mut buffer = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            // stdin closed: flush any remainder interpretation and exit clean.
            if !buffer.trim().is_empty() {
                let (frames, _) = decode_commands(&buffer);
                for frame in frames {
                    if let Err(err) = frame {
                        write_error_frame(writer, &err)?;
                    }
                }
            }
            return Ok(HostExit::StdinClosed);
        }
        buffer.push_str(&line);

        let (frames, remainder) = decode_commands(&buffer);
        buffer = remainder;
        for frame in frames {
            match frame {
                Ok(Command::Ping) => write_frame(writer, &Event::Pong)?,
                Ok(Command::Stop) => {
                    // v1: stop is advisory between turns (the turn engine's
                    // cancellation is plumbed at the driver level).
                    write_frame(
                        writer,
                        &Event::Error {
                            error: ErrorBody {
                                code: "advisory".into(),
                                message: "stop acknowledged between turns".into(),
                                retryable: true,
                            },
                            msg_id: String::new(),
                        },
                    )?;
                }
                Ok(Command::Shutdown) => return Ok(HostExit::ShutdownCommand),
                Ok(Command::ToolApprove { .. }) | Ok(Command::ToolDeny { .. }) => {
                    // Approval results are routed to the pending turn driver;
                    // v1 headless turns use policy gates, so these are logged
                    // as recoverable no-ops.
                    write_frame(
                        writer,
                        &Event::Error {
                            error: ErrorBody {
                                code: "advisory".into(),
                                message: "approval received with no pending request".into(),
                                retryable: true,
                            },
                            msg_id: String::new(),
                        },
                    )?;
                }
                Ok(Command::ApprovalResume { .. }) => {
                    write_frame(
                        writer,
                        &Event::Error {
                            error: ErrorBody {
                                code: "advisory".into(),
                                message: "approval resume received with no pending request".into(),
                                retryable: true,
                            },
                            msg_id: String::new(),
                        },
                    )?;
                }
                Ok(Command::Message {
                    msg_id, content, ..
                }) => {
                    write_frame(
                        writer,
                        &Event::StreamStart {
                            msg_id: msg_id.clone(),
                        },
                    )?;
                    let (events, usage, stop_reason) = run_turn(msg_id.clone(), content).await;
                    for event in events {
                        write_frame(writer, &event)?;
                    }
                    let usage_frame = usage.map(|u| UsageFrame {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cache_read_tokens: u.cached_input_tokens,
                        cache_write_tokens: None,
                        cost_usd: u.cost_usd,
                        // P1 §5: this host predates meter wiring; the
                        // optional session-total fields stay absent (older
                        // and newer hosts both parse the frame).
                        session_total_input_tokens: None,
                        session_total_output_tokens: None,
                        cost_microcents: None,
                        priced: None,
                    });
                    write_frame(
                        writer,
                        &Event::StreamEnd {
                            finish_reason: stop_reason,
                            msg_id,
                            usage: usage_frame.clone().unwrap_or_default(),
                            usage_delta: usage_frame.unwrap_or_default(),
                            agent_run_id: None,
                        },
                    )?;
                }
                Err(err) => write_error_frame(writer, &err)?,
            }
        }
    }
}

fn write_frame<W: Write>(writer: &mut W, event: &Event) -> std::io::Result<()> {
    writer.write_all(encode_event(event).as_bytes())?;
    writer.flush()
}

fn write_error_frame<W: Write>(writer: &mut W, err: &ProtocolError) -> std::io::Result<()> {
    write_frame(
        writer,
        &Event::Error {
            error: ErrorBody {
                code: "protocol".into(),
                message: err.to_string(),
                retryable: true,
            },
            msg_id: String::new(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn config() -> HostConfig {
        HostConfig {
            runtime_version: "test".into(),
            session_id: "s-test".into(),
            capabilities: v1_capabilities(),
        }
    }

    async fn fake_turn(msg_id: String, _content: String) -> (Vec<Event>, Option<Usage>, String) {
        (
            vec![Event::TextDelta {
                msg_id,
                text: "done".into(),
            }],
            None,
            "stop".into(),
        )
    }

    #[tokio::test]
    async fn ready_first_then_frames_then_clean_exit() {
        let input =
            "{\"type\":\"ping\"}\n{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"hi\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut output: Vec<u8> = Vec::new();

        let exit = run_host_loop(&mut reader, &mut output, &config(), fake_turn)
            .await
            .unwrap();

        assert_eq!(exit, HostExit::StdinClosed);
        let text = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].contains("\"type\":\"ready\""),
            "ready must be first: {text}"
        );
        assert!(text.contains("\"type\":\"pong\""));
        assert!(text.contains("\"type\":\"stream_start\""));
        assert!(text.contains("\"type\":\"text_delta\""));
        assert!(text.contains("\"type\":\"stream_end\""));
        assert!(
            lines[1].contains("\"type\":\"pong\""),
            "pong answers ping promptly"
        );
    }

    #[tokio::test]
    async fn malformed_input_produces_error_frame_and_continues() {
        let input = "garbage\n{\"type\":\"ping\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut output: Vec<u8> = Vec::new();

        let exit = run_host_loop(&mut reader, &mut output, &config(), fake_turn)
            .await
            .unwrap();

        assert_eq!(exit, HostExit::StdinClosed);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("\"type\":\"error\""));
        assert!(
            text.contains("\"type\":\"pong\""),
            "loop continued after malformed"
        );
    }

    #[tokio::test]
    async fn shutdown_command_exits_with_reason() {
        let input = "{\"type\":\"shutdown\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut output: Vec<u8> = Vec::new();

        let exit = run_host_loop(&mut reader, &mut output, &config(), fake_turn)
            .await
            .unwrap();

        assert_eq!(exit, HostExit::ShutdownCommand);
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("stream_end"), "no partial turn frames");
    }

    /// Frame cadence contract (G-C2-1): a streamed turn emits frames in the
    /// documented order — `ready` first, then `stream_start`, the turn's
    /// events in scripted order, and the terminal `stream_end` last — and no
    /// two frames are written without an intervening flush boundary.
    #[tokio::test]
    async fn streamed_turn_cadence_orders_frames_and_flushes_between_them() {
        /// Writer that records the write/flush op stream so the test can
        /// observe flush boundaries between frames.
        #[derive(Default)]
        struct FlushProbe {
            bytes: Vec<u8>,
            ops: Vec<&'static str>,
            chunks: Vec<String>,
        }
        impl Write for FlushProbe {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.ops.push("write");
                self.bytes.extend_from_slice(buf);
                self.chunks.push(String::from_utf8_lossy(buf).into_owned());
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.ops.push("flush");
                Ok(())
            }
        }

        async fn scripted_turn(
            msg_id: String,
            _content: String,
        ) -> (Vec<Event>, Option<Usage>, String) {
            (
                vec![
                    Event::TextDelta {
                        msg_id: msg_id.clone(),
                        text: "part one".into(),
                    },
                    Event::ToolRunning {
                        call_id: "c1".into(),
                        msg_id: msg_id.clone(),
                        tool_name: "fs_read".into(),
                    },
                    Event::ToolResult {
                        call_id: "c1".into(),
                        msg_id: msg_id.clone(),
                        output: "ok".into(),
                        output_type: Some("text".into()),
                        status: "success".into(),
                        tool_name: "fs_read".into(),
                        metadata: None,
                    },
                    Event::TextDelta {
                        msg_id,
                        text: "part two".into(),
                    },
                ],
                None,
                "stop".into(),
            )
        }

        let input = "{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"go\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut probe = FlushProbe::default();

        let exit = run_host_loop(&mut reader, &mut probe, &config(), scripted_turn)
            .await
            .unwrap();
        assert_eq!(exit, HostExit::StdinClosed);

        // Contract order: ready first, per-event frames in scripted order,
        // terminal stream_end last with nothing after it.
        let text = String::from_utf8(probe.bytes).unwrap();
        let types: Vec<String> = text
            .lines()
            .map(|line| {
                let frame: serde_json::Value = serde_json::from_str(line).expect("frame json");
                frame["type"].as_str().expect("type tag").to_string()
            })
            .collect();
        assert_eq!(
            types,
            vec![
                "ready",
                "stream_start",
                "text_delta",
                "tool_running",
                "tool_result",
                "text_delta",
                "stream_end",
            ],
            "frame cadence: {text}"
        );

        // Flush boundaries: every frame write is followed by a flush before
        // the next write — no two frames coalesce unflushed.
        assert_eq!(probe.ops.len() % 2, 0, "each write must pair with a flush");
        for pair in probe.ops.chunks_exact(2) {
            assert_eq!(pair, ["write", "flush"], "unflushed frame writes");
        }
        assert_eq!(
            probe.chunks.len(),
            types.len(),
            "one flushed chunk per frame"
        );
        for chunk in &probe.chunks {
            assert_eq!(
                chunk.matches('\n').count(),
                1,
                "each flushed chunk is exactly one NDJSON frame: {chunk:?}"
            );
        }
    }

    #[test]
    fn turn_state_labels_cover_the_machine() {
        let states = [
            TurnState::Receive,
            TurnState::Understand,
            TurnState::Plan,
            TurnState::Act,
            TurnState::Observe,
            TurnState::Verify,
            TurnState::Complete,
        ];
        let labels: Vec<String> = states.iter().map(|s| s.label()).collect();
        assert_eq!(labels.len(), 7);
    }
}
