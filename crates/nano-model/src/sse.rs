//! SSE frame parsing for the three Flux inference wires.
//!
//! Tolerates what live fixtures showed: `event:` lines (anthropic/responses),
//! data-only frames (completions), `[DONE]` sentinels, CRLF, and trailing
//! partial frames (stream cut mid-frame).
//!
//! Fail-closed bounds: a single event buffer may not exceed
//! [`MAX_SSE_FRAME_BYTES`], and the accumulated unconsumed tail (a partial
//! frame still awaiting its terminating blank line) may not exceed
//! [`MAX_SSE_BUFFER_BYTES`]. Crossing either bound yields a typed
//! [`SseError`] — never a panic, never silent truncation — and poisons the
//! parser so every subsequent `feed`/`finish` fails closed with the same
//! error.

/// Largest single SSE event buffer accepted, in bytes.
///
/// Generous-but-sane: the largest legitimate recorded stream fixture
/// (`shared/fixtures/flux/streaming/`) is under 8 KiB end to end; 8 MiB
/// leaves three orders of magnitude of headroom for a max-size completion
/// event while still capping hostile growth.
pub const MAX_SSE_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// Largest accumulated unconsumed buffer, in bytes: a partial frame that
/// never receives its terminating blank line cannot grow past this.
pub const MAX_SSE_BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// Typed integrity error for hostile or broken SSE streams.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SseError {
    /// A single event buffer exceeded [`MAX_SSE_FRAME_BYTES`].
    #[error("sse event buffer exceeds {limit}-byte cap")]
    FrameTooLarge {
        /// The cap that was exceeded, in bytes.
        limit: usize,
    },
    /// The accumulated unconsumed tail exceeded [`MAX_SSE_BUFFER_BYTES`].
    #[error("sse unconsumed buffer exceeds {limit}-byte cap")]
    BufferTooLarge {
        /// The cap that was exceeded, in bytes.
        limit: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser: feed chunks, drain complete frames.
#[derive(Default)]
pub struct SseParser {
    buffer: String,
    poisoned: Option<SseError>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) -> Result<Vec<SseFrame>, SseError> {
        if let Some(err) = &self.poisoned {
            return Err(err.clone());
        }
        let normalized = chunk.replace("\r\n", "\n");
        self.buffer.push_str(&normalized);
        let mut frames = Vec::new();
        while let Some(pos) = self.buffer.find("\n\n") {
            if pos > MAX_SSE_FRAME_BYTES {
                return Err(self.poison(SseError::FrameTooLarge {
                    limit: MAX_SSE_FRAME_BYTES,
                }));
            }
            let raw = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();
            if let Some(frame) = parse_frame(&raw) {
                frames.push(frame);
            }
        }
        if self.buffer.len() > MAX_SSE_BUFFER_BYTES {
            return Err(self.poison(SseError::BufferTooLarge {
                limit: MAX_SSE_BUFFER_BYTES,
            }));
        }
        Ok(frames)
    }

    pub fn finish(&mut self) -> Result<Vec<SseFrame>, SseError> {
        if let Some(err) = &self.poisoned {
            return Err(err.clone());
        }
        let raw = std::mem::take(&mut self.buffer);
        Ok(parse_frame(&raw).into_iter().collect())
    }

    fn poison(&mut self, err: SseError) -> SseError {
        self.poisoned = Some(err.clone());
        err
    }
}

fn parse_frame(raw: &str) -> Option<SseFrame> {
    let mut event: Option<String> = None;
    let mut data_lines: Vec<&str> = Vec::new();
    let mut saw_any = false;
    for line in raw.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        saw_any = true;
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if !saw_any && raw.trim().is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    if data.is_empty() && event.is_none() {
        return None;
    }
    Some(SseFrame { event, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_only_frames() {
        let mut p = SseParser::new();
        let frames = p
            .feed("data: {\"a\":1}\n\ndata: {\"b\":2}\n\n")
            .expect("small frames must parse");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "{\"a\":1}");
        assert_eq!(frames[0].event, None);
    }

    #[test]
    fn parses_event_and_multiline_data() {
        let mut p = SseParser::new();
        let frames = p
            .feed("event: message\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1}\n\n")
            .expect("small frame must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("message"));
        assert!(frames[0].data.contains("jsonrpc"));
    }

    #[test]
    fn tolerates_crlf_and_partial_tail() {
        let mut p = SseParser::new();
        let frames = p.feed("data: ok\r\n\r\ndata: partial").expect("must parse");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "ok");
        let tail = p.finish().expect("unpoisoned finish must succeed");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].data, "partial");
    }

    #[test]
    fn done_sentinel_passes_through() {
        let mut p = SseParser::new();
        let frames = p.feed("data: [DONE]\n\n").expect("must parse");
        assert_eq!(frames[0].data, "[DONE]");
    }

    #[test]
    fn oversized_frame_poisons_parser_fail_closed() {
        let mut p = SseParser::new();
        let huge = format!("data: {}\n\n", "x".repeat(MAX_SSE_FRAME_BYTES + 1));
        let err = p.feed(&huge).expect_err("oversized frame must error");
        assert_eq!(
            err,
            SseError::FrameTooLarge {
                limit: MAX_SSE_FRAME_BYTES
            }
        );
        // Fail-closed: after a cap violation every later call reports the
        // same typed error instead of resuming on a hostile stream.
        let again = p.feed("data: ok\n\n").expect_err("poisoned feed");
        assert_eq!(again, err);
        let finish_err = p.finish().expect_err("poisoned finish");
        assert_eq!(finish_err, err);
    }
}
