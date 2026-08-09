//! Fixture-replay tests: parse the recorded Flux traffic captured by
//! scripts/flux-probe (shared/fixtures/flux/) — deterministic, no network.

use crate::flux_completions::{
    build_request_body, classify_status, parse_completion_body, parse_sse_completion_stream,
};
use crate::types::{Message, ModelError, ModelEvent, ModelRequest, Role};

fn newest_file(dir: &str, suffix: &str) -> String {
    let full = format!("../../../shared/fixtures/flux/{dir}");
    let mut files: Vec<_> = std::fs::read_dir(&full)
        .unwrap_or_else(|e| panic!("fixture dir {full}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(suffix))
        .collect();
    files.sort();
    files
        .pop()
        .unwrap_or_else(|| panic!("no fixture ending {suffix} in {full}"))
        .to_string_lossy()
        .to_string()
}

#[test]
fn parses_batch1_completion_with_reasoning_and_cost() {
    let path = newest_file("chat-completions", "_response.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_completion_body(&text).expect("parse");

    // Batch-1 fixture: reasoning_content present, content "ok", cost_usd set.
    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::ReasoningDelta(_))),
        "reasoning must map: {:?}",
        response.events
    );
    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(t) if t.contains("ok"))),
        "text must map: {:?}",
        response.events
    );
    assert!(response.usage.cost_usd.is_some(), "cost must map");
    assert_eq!(response.stop_reason, "length");
    assert!(response.usage.output_tokens > 0);
}

#[test]
fn parses_tool_call_fixture_into_complete_tool_call() {
    let path = newest_file("tool-calls", "_cc_tool.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_completion_body(&text).expect("parse");

    let tool_call = response
        .events
        .iter()
        .find_map(|e| match e {
            ModelEvent::ToolCallComplete(tc) => Some(tc),
            _ => None,
        })
        .expect("tool call present");
    assert_eq!(tool_call.name, "get_weather");
    assert_eq!(tool_call.arguments["city"], "Paris");
    assert_eq!(response.stop_reason, "tool_calls");
}

#[test]
fn parses_streaming_sse_fixture_into_deltas() {
    let path = newest_file("streaming", "_cc_sse.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_sse_completion_stream(&text).expect("parse stream");

    let reasoning: Vec<_> = response
        .events
        .iter()
        .filter(|e| matches!(e, ModelEvent::ReasoningDelta(_)))
        .collect();
    assert!(!reasoning.is_empty(), "stream reasoning deltas must map");
    let text_deltas: Vec<_> = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let joined = text_deltas.join("");
    assert!(joined.contains('1'), "stream text must assemble: {joined}");
}

#[test]
fn request_body_uses_completions_shape() {
    let request = ModelRequest {
        model: "flux-fast".into(),
        messages: vec![Message::user("hi")],
        max_tokens: Some(64),
        stream: true,
        ..Default::default()
    };
    let body = build_request_body(&request);
    assert_eq!(body["model"], "flux-fast");
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "hi");
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["stream"], true);
    // No anthropic fields on the completions wire.
    assert!(body.get("system").is_none());
}

#[test]
fn error_classification_maps_flux_shapes() {
    let auth = classify_status(401, r#"{"error":{"message":"bad key"}}"#.into());
    assert!(matches!(auth, ModelError::Auth(_)));
    let entitlement = classify_status(402, r#"{"error":{"message":"upgrade required"}}"#.into());
    assert!(matches!(entitlement, ModelError::Entitlement(_)));
    let server = classify_status(503, r#"{"error":{"message":"upstream down"}}"#.into());
    assert!(matches!(server, ModelError::Server { status: 503, .. }));
}

#[test]
fn message_roles_wire_correctly() {
    let request = ModelRequest {
        model: "m".into(),
        messages: vec![Message::system("rules"), Message::user("work")],
        ..Default::default()
    };
    let body = build_request_body(&request);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(Role::Assistant, Role::Assistant);
}
