//! Fixture-replay tests: parse the recorded Flux traffic captured by
//! scripts/flux-probe. Reads the crate's vendored snapshot (fixtures-flux/,
//! a verbatim copy of shared/fixtures/flux) so standalone checkouts and CI
//! are self-contained — deterministic, no network.

use crate::anthropic_messages::{
    METADATA_CACHE_CONTROL, METADATA_THINKING, build_count_tokens_body,
    build_request_body as build_anthropic_body, parse_count_tokens_body, parse_message_body,
    parse_sse_message_stream,
};
use crate::flux_completions::{
    build_request_body, classify_status, parse_completion_body, parse_sse_completion_stream,
};
use crate::flux_responses::{
    build_request_body as build_responses_body, parse_response_body, parse_sse_responses_stream,
};
use crate::types::{Message, ModelError, ModelEvent, ModelRequest, Role};

fn newest_file(dir: &str, suffix: &str) -> String {
    let full = format!("{}/fixtures-flux/{dir}", env!("CARGO_MANIFEST_DIR"));
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
    assert!(matches!(
        auth,
        ModelError::Auth {
            status: Some(401),
            ..
        }
    ));
    let entitlement = classify_status(402, r#"{"error":{"message":"upgrade required"}}"#.into());
    assert!(matches!(entitlement, ModelError::Entitlement(_)));
    let server = classify_status(503, r#"{"error":{"message":"upstream down"}}"#.into());
    assert!(matches!(server, ModelError::Server { status: 503, .. }));
}

#[test]
fn f_p5_1_500_invalid_request_error_is_terminal_never_retryable() {
    // Live wire (F-P5-1): a malformed request body (e.g. a tool payload a
    // leaf cannot parse) arrives as HTTP 5xx with
    // error.type=="invalid_request_error". That is a FORMAT rejection:
    // terminal InvalidRequest, never the retryable/cascading Server class.
    let body = r#"{"error":{"message":"Invalid parameter: tools[0].function","type":"invalid_request_error","code":"400"}}"#;
    for status in [500, 502, 503] {
        let err = classify_status(status, body.into());
        let ModelError::InvalidRequest { status: s, .. } = &err else {
            panic!("{status} + invalid_request_error must classify as InvalidRequest: {err:?}");
        };
        assert_eq!(*s, status);
        assert_eq!(
            crate::retry::is_retryable(&err),
            None,
            "a format rejection is never retried"
        );
    }
    // The fold applies on the invalid_request_error marker only — a plain
    // internal_error 500 stays the retryable Server class (pinned by
    // non_auth_500_stays_retryable_server).
}

#[test]
fn batch3_badkey_500_auth_error_classifies_as_auth_not_retryable() {
    // Live wire: invalid key → HTTP 500 with error.type=="auth_error"
    // (FINDINGS.md batch 3 §a2). Must surface as Auth and never be retried.
    let path = newest_file("errors", "_cc_badkey_response.json");
    let body = std::fs::read_to_string(&path).unwrap();
    let err = classify_status(500, body);
    let ModelError::Auth { message, status } = &err else {
        panic!("500 auth_error must classify as Auth: {err:?}");
    };
    // A 500-classified auth failure is NOT the 401 seam's concern.
    assert_eq!(*status, Some(500));
    // The message carries only the SHA-256 digest of the presented key
    // (`key=<64 hex chars>`), never the key itself.
    assert!(message.contains("auth") || message.contains("Authentication"));
    assert_eq!(crate::retry::is_retryable(&err), None);
}

#[test]
fn batch3_overlimit_413_classifies_as_context_overflow() {
    // Live wire: max_tokens over the context window → HTTP 413 with
    // error.message=="context_window_exceeded" (FINDINGS.md batch 3 §a).
    let path = newest_file("errors", "_cc_overlimit_response.json");
    let body = std::fs::read_to_string(&path).unwrap();
    let err = classify_status(413, body);
    let ModelError::ContextOverflow(message) = &err else {
        panic!("413 must classify as ContextOverflow: {err:?}");
    };
    assert_eq!(message, "context_window_exceeded");
    assert_eq!(crate::retry::is_retryable(&err), None);
}

#[test]
fn batch3_burst_503_edge_html_classifies_as_retryable_server() {
    // Live wire: burst load saturates the edge with bare nginx 503 HTML
    // (non-JSON, no Retry-After) — never 429 (FINDINGS.md batch 3 §b).
    // This is the retryable failure mode under load.
    let path = newest_file("rate-limit", "_cc_parallel_burst_503_body.html");
    let body = std::fs::read_to_string(&path).unwrap();
    let err = classify_status(503, body);
    let ModelError::Server { status: 503, .. } = &err else {
        panic!("503 edge HTML must classify as Server: {err:?}");
    };
    assert_eq!(crate::retry::is_retryable(&err), Some(None));
}

#[test]
fn non_auth_500_stays_retryable_server() {
    // Only error.type=="auth_error" reclassifies a 500; a plain internal
    // error keeps the generic retryable Server shape.
    let body = r#"{"error":{"message":"upstream exploded","type":"internal_error","code":"500"}}"#;
    let err = classify_status(500, body.into());
    let ModelError::Server { status: 500, .. } = &err else {
        panic!("non-auth 500 must stay Server: {err:?}");
    };
    assert_eq!(crate::retry::is_retryable(&err), Some(None));
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

// ---------------------------------------------------------------------------
// Responses surface (fixtures-flux/responses/, streaming/*_rs_sse.txt)
// ---------------------------------------------------------------------------

#[test]
fn responses_fixture_maps_reasoning_and_incomplete_stop() {
    // Batch-1 fixture: the entire 16-token budget was eaten by the reasoning
    // item — status "incomplete", reason "max_output_tokens", NO message
    // item. Empty visible output is not an error (pinned).
    let path = newest_file("responses", "_response.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_response_body(&text).expect("parse");

    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::ReasoningDelta(t) if t.contains("ok"))),
        "reasoning summary must map: {:?}",
        response.events
    );
    assert!(
        !response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(_))),
        "no message item in fixture — no text events: {:?}",
        response.events
    );
    assert_eq!(response.stop_reason, "max_output_tokens");
    assert_eq!(response.usage.input_tokens, 90);
    assert_eq!(response.usage.output_tokens, 16);
    assert_eq!(response.usage.reasoning_tokens, Some(16));
}

#[test]
fn responses_sse_stream_assembles_lifecycle() {
    let path = newest_file("streaming", "_rs_sse.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_sse_responses_stream(&text).expect("parse stream");
    let reasoning: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::ReasoningDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasoning,
        "We need answer simple. Count 1 to 3 one per line."
    );

    let text_deltas: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, "1\n2\n3");

    // Terminal usage from response.completed.
    assert_eq!(response.usage.input_tokens, 96);
    assert_eq!(response.usage.output_tokens, 21);
    assert_eq!(response.usage.reasoning_tokens, Some(15));
}

#[test]
fn responses_request_body_uses_responses_shape() {
    let request = ModelRequest {
        model: "flux-fast".into(),
        messages: vec![Message::user("hi")],
        max_tokens: Some(16),
        stream: false,
        tools: vec![crate::types::ToolDefinition {
            name: "get_weather".into(),
            description: "Get weather for a city".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }],
        ..Default::default()
    };
    let body = build_responses_body(&request);
    assert_eq!(body["max_output_tokens"], 16);
    assert!(
        body.get("max_tokens").is_none(),
        "completions field must not leak"
    );
    // Lone user text message → the bare-string input form the probe sent.
    assert_eq!(body["input"], "hi");
    // Flattened Responses tool shape — no nested `function` object.
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "get_weather");
    assert!(body["tools"][0].get("function").is_none());
    assert_eq!(body["tool_choice"], "auto");
}

#[test]
fn responses_request_maps_system_to_instructions() {
    let request = ModelRequest {
        model: "flux-fast".into(),
        system: Some("You are concise.".into()),
        messages: vec![Message::user("hi")],
        ..Default::default()
    };
    let body = build_responses_body(&request);
    assert_eq!(body["instructions"], "You are concise.");
    assert!(body.get("system").is_none());
}

// ---------------------------------------------------------------------------
// Anthropic Messages surface (COMPAT — never the default route)
// ---------------------------------------------------------------------------

#[test]
fn anthropic_fixture_tolerates_empty_text_block() {
    // Batch-1 quirk: an empty text block with stop_reason "max_tokens" is a
    // normal truncation outcome, not an error.
    let path = newest_file("anthropic-messages", "_response.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_message_body(&text).expect("parse");

    assert!(
        !response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(_))),
        "empty text block must yield no TextDelta: {:?}",
        response.events
    );
    assert_eq!(response.stop_reason, "max_tokens");
    assert_eq!(response.usage.input_tokens, 14);
    assert_eq!(response.usage.output_tokens, 16);
    assert_eq!(response.usage.cached_input_tokens, Some(0));
}

#[test]
fn anthropic_tool_call_id_is_opaque_recorded_artifact() {
    // FINDINGS batch 2: the anthropic surface is a translation layer — the
    // recorded tool_use.id is call_* (OpenAI-style), NOT toolu_*. This pins
    // the artifact as recorded; ids round-trip verbatim either way.
    let path = newest_file("tool-calls", "_am_tool.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_message_body(&text).expect("parse");

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
    assert!(
        tool_call.id.starts_with("call_"),
        "recorded translation artifact: {}",
        tool_call.id
    );
    assert_eq!(response.stop_reason, "tool_use");
}

#[test]
fn anthropic_sse_stream_assembles_lifecycle() {
    let path = newest_file("streaming", "_am_sse.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_sse_message_stream(&text).expect("parse stream");

    let text_deltas: String = response
        .events
        .iter()
        .filter_map(|e| match e {
            ModelEvent::TextDelta(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, "1\n2\n3");
    assert_eq!(response.stop_reason, "end_turn");
}

#[test]
fn anthropic_request_body_uses_native_shape() {
    let request = ModelRequest {
        model: "flux-auto".into(),
        messages: vec![
            Message::user("What is the weather in Paris? Use the tool."),
            Message {
                role: Role::Assistant,
                content: vec![crate::types::ContentBlock::ToolUse {
                    id: "call_abc".into(),
                    name: "get_weather".into(),
                    input: serde_json::json!({"city": "Paris"}),
                }],
            },
            Message {
                role: Role::Tool,
                content: vec![crate::types::ContentBlock::ToolResult {
                    tool_use_id: "call_abc".into(),
                    content: "sunny".into(),
                    is_error: false,
                    images: vec![],
                }],
            },
        ],
        max_tokens: Some(1024),
        tools: vec![crate::types::ToolDefinition {
            name: "get_weather".into(),
            description: "Get weather for a city".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }],
        ..Default::default()
    };
    let body = build_anthropic_body(&request);
    assert_eq!(body["max_tokens"], 1024);
    // Anthropic-native tool shape — no `type:"function"` wrapper.
    assert_eq!(body["tools"][0]["name"], "get_weather");
    assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    assert!(body["tools"][0].get("function").is_none());
    // tool_use / tool_result round-trip into the Anthropic message shape.
    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(body["messages"][1]["content"][0]["id"], "call_abc");
    assert_eq!(body["messages"][2]["role"], "user");
    assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "call_abc");
}

#[test]
fn anthropic_max_tokens_defaults_when_unset() {
    // max_tokens is REQUIRED on this surface (unlike completions); the
    // adapter supplies a documented default rather than omitting it.
    let request = ModelRequest {
        model: "flux-auto".into(),
        messages: vec![Message::user("hi")],
        ..Default::default()
    };
    let body = build_anthropic_body(&request);
    assert_eq!(
        body["max_tokens"],
        crate::anthropic_messages::DEFAULT_MAX_TOKENS
    );
}

// ---------------------------------------------------------------------------
// Pinned-inert compat: thinking/cache serialize faithfully, live Flux drops
// them server-side (FINDINGS batch-2 WIRE-2). Both halves are asserted so
// nothing is dropped silently.
// ---------------------------------------------------------------------------

#[test]
fn thinking_passthrough_is_inert_on_live_flux_compat() {
    // FINDINGS batch-2 WIRE-2: thinking:{type:"enabled",budget_tokens} is
    // accepted but the response contains NO thinking block — on flux-auto
    // (routed qwen-plus) AND on flux-pinned-claude-sonnet (routed
    // claude-sonnet-5, a real Claude). The adapter serializes faithfully;
    // the drop is server-side. Both halves pinned here.
    let mut request = ModelRequest {
        model: "flux-auto".into(),
        messages: vec![Message::user("What is 17*23? Think briefly.")],
        max_tokens: Some(2048),
        ..Default::default()
    };
    request.metadata.insert(
        METADATA_THINKING.into(),
        serde_json::json!({"type": "enabled", "budget_tokens": 1024}),
    );
    let body = build_anthropic_body(&request);
    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "enabled", "budget_tokens": 1024}),
        "thinking must serialize faithfully onto the wire"
    );

    for suffix in ["_am_thinking.json", "_am_thinking_pinned_claude.json"] {
        let path = newest_file("thinking", suffix);
        let text = std::fs::read_to_string(&path).unwrap();
        let response = parse_message_body(&text).expect("parse");
        assert!(
            !response
                .events
                .iter()
                .any(|e| matches!(e, ModelEvent::ReasoningDelta(_))),
            "live Flux drops thinking (WIRE-2): zero ReasoningDelta in {path}"
        );
    }
}

#[test]
fn cache_control_serializes_but_write_is_never_recorded_compat() {
    // FINDINGS batch-2 WIRE-2: cache_control:{type:"ephemeral"} on a system
    // block serializes faithfully, but cache_creation_input_tokens stays 0 —
    // the requested write is never recorded server-side.
    let mut request = ModelRequest {
        model: "flux-auto".into(),
        system: Some("You are concise.".into()),
        messages: vec![Message::user("Say ok")],
        max_tokens: Some(64),
        ..Default::default()
    };
    request.metadata.insert(
        METADATA_CACHE_CONTROL.into(),
        serde_json::json!({"type": "ephemeral"}),
    );
    let body = build_anthropic_body(&request);
    // Block-array system form with the cache_control marker (the recorded
    // cache fixture's request shape).
    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["text"], "You are concise.");
    assert_eq!(
        body["system"][0]["cache_control"],
        serde_json::json!({"type": "ephemeral"})
    );

    // Write fixture: requested write never recorded (raw creation count is
    // 0); the read count (128) is ambient infra caching, not request-driven
    // — asserted as mapped without claiming request causation.
    let path = newest_file("cache", "_am_cache_write.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["usage"]["cache_creation_input_tokens"], 0);
    let response = parse_message_body(&text).expect("parse");
    assert_eq!(response.usage.cached_input_tokens, Some(128));

    // Read fixture: both halves zero.
    let path = newest_file("cache", "_am_cache_read.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let response = parse_message_body(&text).expect("parse");
    assert_eq!(response.usage.cached_input_tokens, Some(0));
}

// ---------------------------------------------------------------------------
// count_tokens (messages-only body — no max_tokens, per fixture)
// ---------------------------------------------------------------------------

#[test]
fn count_tokens_fixture_parses_input_count() {
    let path = newest_file("anthropic-count-tokens", "_response.json");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(parse_count_tokens_body(&text).expect("parse"), 14);
}

#[test]
fn count_tokens_request_body_has_no_max_tokens() {
    let request = ModelRequest {
        model: "flux-auto".into(),
        messages: vec![Message::user("Reply with exactly the word: ok")],
        max_tokens: Some(512),
        ..Default::default()
    };
    let body = build_count_tokens_body(&request);
    assert!(body.get("max_tokens").is_none(), "fixture-pinned shape");
    assert_eq!(body["model"], "flux-auto");
    assert_eq!(body["messages"][0]["role"], "user");
    // Error path reuses the shared classification (single path, all wires).
    let err = classify_status(
        500,
        r#"{"error":{"message":"x","type":"auth_error"}}"#.into(),
    );
    assert!(matches!(
        err,
        ModelError::Auth {
            status: Some(500),
            ..
        }
    ));
}

/// C9 §5: a rate-limit payload riding a Responses stream frame is parsed
/// MID-STREAM into the observation channel — the dedicated typed path,
/// never a ModelEvent.
#[test]
fn responses_stream_rate_limit_frame_emits_mid_stream_observation() {
    let text = concat!(
        "data: {\"type\": \"response.rate_limits\", \"rate_limits\": {\"scope\": \"account\", \"tokens_limit\": 100000, \"tokens_remaining\": 99500}}\n\n",
        "data: {\"type\": \"response.output_text.delta\", \"delta\": \"hi\"}\n\n",
        "data: [DONE]\n\n"
    );
    let mut observed = Vec::new();
    let response =
        crate::flux_responses::parse_sse_responses_stream_observed(text, &mut |snapshot| {
            observed.push(snapshot);
        })
        .expect("parse stream");
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].scope.as_deref(), Some("account"));
    assert_eq!(observed[0].tokens_limit, Some(100_000));
    // The stream itself is unaffected (no ModelEvent pollution).
    assert!(
        response
            .events
            .iter()
            .any(|e| matches!(e, ModelEvent::TextDelta(t) if t == "hi"))
    );
}

// ---------------------------------------------------------------------------
// P2a §2.2: pinned wire-JSON fixtures for ContentBlock::Image emission (one
// per codec, pinned BEFORE the producers land — the C10 diff-block
// precedent). `aGVsbG8` is base64("hello") — synthetic bytes only.
// ---------------------------------------------------------------------------

fn image_request() -> ModelRequest {
    use crate::types::ContentBlock;
    ModelRequest {
        model: "flux-pinned-gpt-5".into(),
        messages: vec![Message::user_blocks(vec![
            ContentBlock::Text {
                text: "before".into(),
            },
            ContentBlock::Image {
                mime: "image/png".into(),
                data: "aGVsbG8".into(),
            },
            ContentBlock::Text {
                text: "after".into(),
            },
        ])],
        ..Default::default()
    }
}

#[test]
fn completions_image_blocks_emit_image_url_parts_in_order() {
    let body = build_request_body(&image_request());
    // Exactly ONE image_url part, data:<sniffed-mime>;base64, prefix, part
    // order preserved (text parts are NOT the bare-string fast path).
    assert_eq!(
        body["messages"][0],
        serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "before"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,aGVsbG8"}},
                {"type": "text", "text": "after"}
            ]
        })
    );
    // No `detail` field in P2a (Q7 RULED, §16).
    assert!(
        body["messages"][0]["content"][1]["image_url"]
            .get("detail")
            .is_none()
    );
}

#[test]
fn anthropic_image_blocks_emit_base64_source_parts_in_order() {
    let body = build_anthropic_body(&image_request());
    assert_eq!(
        body["messages"][0]["content"],
        serde_json::json!([
            {"type": "text", "text": "before"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGVsbG8"}},
            {"type": "text", "text": "after"}
        ])
    );
}

#[test]
fn responses_image_blocks_emit_input_image_parts_in_order() {
    let body = build_responses_body(&image_request());
    assert_eq!(
        body["input"],
        serde_json::json!([
            {"role": "user", "content": [
                {"type": "input_text", "text": "before"},
                {"type": "input_image", "image_url": "data:image/png;base64,aGVsbG8"},
                {"type": "input_text", "text": "after"}
            ]}
        ])
    );
}

/// Regression: a lone text block keeps the historical bare-string /
/// joined-text shapes on all three codecs (no array form without images).
#[test]
fn text_only_messages_keep_legacy_wire_shapes() {
    let request = ModelRequest {
        model: "m".into(),
        messages: vec![Message::user("hi")],
        ..Default::default()
    };
    assert_eq!(build_request_body(&request)["messages"][0]["content"], "hi");
    assert_eq!(
        build_anthropic_body(&request)["messages"][0]["content"],
        "hi"
    );
    assert_eq!(build_responses_body(&request)["input"], "hi");
}

// ---------------------------------------------------------------------------
// P2b §3.4/§7: image-bearing ToolResult — the Anthropic native arm pinned
// with NON-EMPTY images on a complete request (parallel view_image +
// fs_read turn, mixed text/image results), and the completions/responses
// rung-3 refusals proven zero-egress. `b25l`/`dHdv` are base64("one")/
// base64("two") — synthetic bytes only.
// ---------------------------------------------------------------------------

fn p2b_image_result_request() -> (ModelRequest, crate::image_result::ImageToolResultParts) {
    use crate::image_result::{OrderedImage, build_image_tool_result};
    use crate::types::ContentBlock;
    use sha2::{Digest, Sha256};
    let ordered = |bytes: &[u8]| OrderedImage {
        bytes: bytes.to_vec(),
        mime: "image/png".into(),
        digest: format!("{:x}", Sha256::digest(bytes)),
        width: 1,
        height: 1,
        normalized_from: None,
    };
    let (parts, provenance) =
        build_image_tool_result("t1", "view_image", vec![ordered(b"one"), ordered(b"two")])
            .expect("canonical builder");
    // The live acceptance seam consumes the token; the codec test needs only
    // the parts.
    drop(provenance);
    let request = ModelRequest {
        model: "flux-pinned-claude".into(),
        messages: vec![
            Message::user("look at these"),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "view_image".into(),
                        input: serde_json::json!({"path": "a.png"}),
                    },
                    ContentBlock::ToolUse {
                        id: "t2".into(),
                        name: "fs_read".into(),
                        input: serde_json::json!({"path": "b.txt"}),
                    },
                ],
            },
            Message {
                role: Role::Tool,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_use_id: "t1".into(),
                        content: parts.content.clone(),
                        is_error: false,
                        images: parts.images.clone(),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: "t2".into(),
                        content: "file body".into(),
                        is_error: false,
                        images: vec![],
                    },
                ],
            },
        ],
        ..Default::default()
    };
    (request, parts)
}

/// §3.4 native arm: the tool_result content becomes an ARRAY — one text part
/// (the projection) then one base64 image part per image, order and count
/// pinned; the sibling text-only result keeps the plain-string form.
#[test]
fn p2b_anthropic_tool_result_images_native_arm_pinned() {
    let (request, _parts) = p2b_image_result_request();
    let body = build_anthropic_body(&request);
    let labels =
        "[Image #1 from tool view_image — 1x1 png]\n[Image #2 from tool view_image — 1x1 png]";
    assert_eq!(
        body["messages"],
        serde_json::json!([
            {"role": "user", "content": "look at these"},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "view_image", "input": {"path": "a.png"}},
                {"type": "tool_use", "id": "t2", "name": "fs_read", "input": {"path": "b.txt"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": [
                    {"type": "text", "text": labels},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "b25l"}},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "dHdv"}}
                ], "is_error": false},
                {"type": "tool_result", "tool_use_id": "t2", "content": "file body", "is_error": false}
            ]}
        ])
    );
}

/// §3.4 rung 3 (RC2 RULED): an image-bearing result is REFUSED on the
/// completions and responses surfaces BEFORE any network I/O — the error is
/// the typed refusal, never a connect/transport error, so the refusal is
/// provably zero-egress (no listener, no fixture server, no packet).
#[tokio::test]
async fn p2b_completions_and_responses_refuse_tool_result_images_with_zero_egress() {
    let (request, _parts) = p2b_image_result_request();
    let completions = crate::flux_completions::FluxCompletionsClient::new(
        nano_egress::client::EgressClient::flux(),
    );
    let err = completions
        .complete_with_hooks(&request, "sk-test", &crate::types::CallHooks::none())
        .await
        .expect_err("completions surface refuses image-bearing results");
    assert!(
        matches!(
            &err,
            ModelError::UnsupportedParam { param, surface, .. }
                if param == "tool_result_images" && surface == "flux-completions"
        ),
        "typed rung-3 refusal, not a transport error: {err:?}"
    );
    let responses =
        crate::flux_responses::FluxResponsesClient::new(nano_egress::client::EgressClient::flux());
    let err = responses
        .complete_with_hooks(&request, "sk-test", &crate::types::CallHooks::none())
        .await
        .expect_err("responses surface refuses image-bearing results");
    assert!(
        matches!(
            &err,
            ModelError::UnsupportedParam { param, surface, .. }
                if param == "tool_result_images" && surface == "flux-responses"
        ),
        "typed rung-3 refusal, not a transport error: {err:?}"
    );
}
