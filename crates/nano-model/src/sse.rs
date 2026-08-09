//! SSE frame parsing for the three Flux inference wires.
//!
//! Tolerates what live fixtures showed: `event:` lines (anthropic/responses),
//! data-only frames (completions), `[DONE]` sentinels, CRLF, and trailing
//! partial frames (stream cut mid-frame).

#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser: feed chunks, drain complete frames.
#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<SseFrame> {
        let normalized = chunk.replace("\r\n", "\n");
        self.buffer.push_str(&normalized);
        let mut frames = Vec::new();
        while let Some(pos) = self.buffer.find("\n\n") {
            let raw = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();
            if let Some(frame) = parse_frame(&raw) {
                frames.push(frame);
            }
        }
        frames
    }

    pub fn finish(&mut self) -> Vec<SseFrame> {
        let raw = std::mem::take(&mut self.buffer);
        parse_frame(&raw).into_iter().collect()
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
        let frames = p.feed("data: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "{\"a\":1}");
        assert_eq!(frames[0].event, None);
    }

    #[test]
    fn parses_event_and_multiline_data() {
        let mut p = SseParser::new();
        let frames = p.feed("event: message\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("message"));
        assert!(frames[0].data.contains("jsonrpc"));
    }

    #[test]
    fn tolerates_crlf_and_partial_tail() {
        let mut p = SseParser::new();
        let frames = p.feed("data: ok\r\n\r\ndata: partial");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "ok");
        let tail = p.finish();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].data, "partial");
    }

    #[test]
    fn done_sentinel_passes_through() {
        let mut p = SseParser::new();
        let frames = p.feed("data: [DONE]\n\n");
        assert_eq!(frames[0].data, "[DONE]");
    }
}
