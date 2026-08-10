//! Adversarial SSE tests: malformed-stream handling in `nano_model::sse` and
//! the high-level `parse_sse_completion_stream`.
//!
//! The parser API is total (`Vec<SseFrame>`, no `Result`), so the contract
//! under test is: hostile input must never panic and never hang, garbage must
//! be contained (dropped), and valid neighboring frames must survive.

use nano_model::flux_completions::parse_sse_completion_stream;
use nano_model::sse::SseParser;
use nano_model::types::ModelEvent;
use std::sync::mpsc;
use std::time::Duration;

/// Run `f` on a worker thread; panic the test if it hangs or crashes.
fn run_guarded<F, T>(label: &str, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(value) => value,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{label}: parser hung on adversarial input")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("{label}: parser panicked on adversarial input")
        }
    }
}

// --- Raw parser robustness ----------------------------------------------------

#[test]
fn truncated_event_mid_frame_is_contained() {
    let mut parser = SseParser::new();
    // Stream cut mid-frame: no terminating blank line ever arrives.
    let frames = parser.feed("data: {\"choices\":[{\"delta\":{\"con");
    assert!(frames.is_empty(), "partial frame must not surface early");
    let tail = parser.finish();
    assert_eq!(tail.len(), 1, "trailing partial frame surfaces via finish");
    assert!(tail[0].data.contains("\"choices\""));
}

#[test]
fn garbage_bytes_mid_stream_do_not_corrupt_neighbors() {
    let mut parser = SseParser::new();
    let frames = run_guarded("garbage mid-stream", move || {
        parser.feed("data: {\"a\":1}\n\n\x00\x01\x02garbage!\n\ndata: {\"b\":2}\n\n")
    });
    assert_eq!(frames.len(), 2, "garbage frame must be dropped, not merged");
    assert_eq!(frames[0].data, "{\"a\":1}");
    assert_eq!(frames[1].data, "{\"b\":2}");
}

#[test]
fn oversized_single_line_neither_panics_nor_hangs() {
    // NOTE: the parser has no line/buffer size cap — a hostile server can
    // grow memory unboundedly. This test pins the current non-crashing
    // behavior; the missing bound is reported as a hardening gap.
    let huge = format!("data: {}\n\n", "x".repeat(4 * 1024 * 1024));
    let frames = run_guarded("oversized line", move || {
        let mut parser = SseParser::new();
        parser.feed(&huge)
    });
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].data.len(), 4 * 1024 * 1024);
}

#[test]
fn flood_of_tiny_frames_completes() {
    // 5k frames in a single chunk: pins completion of burst input.
    let mut chunk = String::new();
    for _ in 0..5000 {
        chunk.push_str("data: x\n\n");
    }
    let frames = run_guarded("tiny-frame flood", move || {
        let mut parser = SseParser::new();
        parser.feed(&chunk)
    });
    assert_eq!(frames.len(), 5000);
}

#[test]
fn pathological_blank_line_stream_is_contained() {
    // A stream of nothing but frame separators must not produce frames,
    // panic, or hang.
    let frames = run_guarded("blank-line flood", || {
        let mut parser = SseParser::new();
        parser.feed(&"\n\n".repeat(10_000))
    });
    assert!(frames.is_empty());
}

#[test]
fn mixed_crlf_lone_cr_and_control_bytes_do_not_panic() {
    let frames = run_guarded("cr/control soup", || {
        let mut parser = SseParser::new();
        let mut all = parser.feed("data: a\r\n\r\ndata: b\r\rdata: c\x07\x1b");
        all.extend(parser.finish());
        all
    });
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].data, "a");
    assert!(frames[1].data.contains('b') && frames[1].data.contains('c'));
}

#[test]
fn event_only_and_comment_frames_are_handled() {
    let mut parser = SseParser::new();
    let frames = parser.feed(": keep-alive comment\n\nevent: ping\n\ndata: x\n\n");
    // Comment-only frame is dropped; event-only and data frames survive.
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].event.as_deref(), Some("ping"));
    assert!(frames[0].data.is_empty());
    assert_eq!(frames[1].data, "x");
}

// --- High-level stream parsing -------------------------------------------------

#[test]
fn high_level_skips_unparseable_garbage_frames() {
    let stream = "data: this is not json at all\n\n\
                  data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n\
                  data: [DONE]\n\n";
    let response = parse_sse_completion_stream(stream).expect("garbage must not error the stream");
    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(t) if t == "hi")),
        "valid frame after garbage must survive: {:?}",
        response.events
    );
}

#[test]
fn missing_done_sentinel_still_completes() {
    // Server hung up without [DONE]: partial content is kept, no error/panic.
    let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
    let response = parse_sse_completion_stream(stream).expect("missing [DONE] must not error");
    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(t) if t == "partial"))
    );
    assert_eq!(response.stop_reason, "stop", "default stop reason");
}

#[test]
fn frames_after_done_sentinel_are_ignored() {
    // Anything a hostile/buggy server appends after [DONE] must not leak into
    // the response.
    let stream = "data: [DONE]\n\n\
                  data: {\"choices\":[{\"delta\":{\"content\":\"after-done\"}}]}\n\n";
    let response = parse_sse_completion_stream(stream).expect("must parse");
    assert!(
        !response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(t) if t.contains("after-done"))),
        "content after [DONE] leaked: {:?}",
        response.events
    );
}

#[test]
fn truncated_json_frame_is_dropped_not_fatal() {
    // Stream cut mid-JSON: the tail frame surfaces via finish() but its JSON
    // is unparseable and must be skipped, not error or panic.
    let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n\
                  data: {\"choices\":[{\"delta\":{\"con";
    let response = run_guarded("truncated json tail", || parse_sse_completion_stream(stream))
        .expect("truncated tail must not error the stream");
    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(t) if t == "ok"))
    );
}
