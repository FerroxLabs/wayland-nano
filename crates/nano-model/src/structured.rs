//! Structured output (C9 §4.3): ONE canonical extracted JSON value per
//! response, validated identically on every surface by the named
//! `jsonschema` dependency (JSON Schema Draft 2020-12) before the caller
//! sees the response.
//!
//! Extraction is per-surface; validation is shared:
//! - Completions / Responses: the final text parsed as JSON.
//! - Anthropic strict: the FORCED TOOL CALL'S input (strict mode returns
//!   tool input, not final text). A response without the forced tool call
//!   is a protocol error. The forced tool call is STRIPPED from the
//!   response events after extraction so the agent loop never executes the
//!   pseudo-tool.
//! - Anthropic non-strict: rejected at request-build (params.rs) —
//!   schema-in-prompt is NEVER structured output.
//!
//! Fail-closed: invalid JSON or a schema violation is a typed
//! `ModelError::OutputSchema` carrying the LITERAL re-ask feedback text
//! (the turn loop journals exactly this string via Op::SchemaReask, so
//! replay byte-fidelity never hinges on template wording). Unvalidated
//! output never passes through to the caller.

use crate::params::Surface;
use crate::types::{ModelError, ModelEvent, ModelResponse};

/// The forced-tool name for Anthropic-surface strict structured output.
pub const FORCED_OUTPUT_TOOL: &str = "nano_structured_output";

/// The literal re-ask feedback the model sees (and the journal records
/// verbatim via Op::SchemaReask). Built once, here, so the error text IS
/// the feedback text — byte-faithful by construction.
pub fn reask_feedback(detail: &str) -> String {
    format!(
        "Your previous response did not satisfy the requested JSON schema ({detail}). Respond again with output that validates against the schema exactly."
    )
}

/// Validate one extracted value against the schema (the shared validator —
/// the same check runs for every surface's extraction).
pub fn validate_value(schema: &serde_json::Value, value: &serde_json::Value) -> Result<(), String> {
    match jsonschema::validate(schema, value) {
        Ok(()) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// Extract and validate the canonical structured output from a response.
/// `response` is mutated ONLY on the Anthropic-strict path (the forced
/// tool call is stripped once its input is extracted).
pub fn extract_and_validate(
    surface: Surface,
    request: &crate::types::ModelRequest,
    response: &mut ModelResponse,
) -> Result<serde_json::Value, ModelError> {
    let Some(schema) = &request.output_schema else {
        return Err(ModelError::Protocol(
            "extract_and_validate called without an output_schema".into(),
        ));
    };
    let extracted: serde_json::Value = match surface {
        Surface::Completions | Surface::Responses => {
            let text: String = response
                .events
                .iter()
                .filter_map(|event| match event {
                    ModelEvent::TextDelta(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            serde_json::from_str(&text).map_err(|err| {
                ModelError::OutputSchema(reask_feedback(&format!(
                    "output was not valid JSON: {err}"
                )))
            })?
        }
        Surface::Anthropic => {
            // Non-strict never reaches here (request-build rejection).
            if !request.output_schema_strict {
                return Err(ModelError::UnsupportedParam {
                    param: "output_schema".into(),
                    surface: surface.label().into(),
                    message: "non-strict output_schema is not structured output on this surface"
                        .into(),
                });
            }
            let position = response
                .events
                .iter()
                .position(|event| {
                    matches!(event, ModelEvent::ToolCallComplete(call) if call.name == FORCED_OUTPUT_TOOL)
                })
                .ok_or_else(|| {
                    ModelError::Protocol(
                        "structured output: response carried no forced tool call".into(),
                    )
                })?;
            let ModelEvent::ToolCallComplete(call) = response.events.remove(position) else {
                unreachable!("position matched ToolCallComplete")
            };
            call.arguments
        }
    };
    validate_value(schema, &extracted)
        .map_err(|detail| ModelError::OutputSchema(reask_feedback(&detail)))?;
    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModelRequest, ToolCall, Usage};

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        })
    }

    fn request(strict: bool) -> ModelRequest {
        ModelRequest {
            model: "flux-auto".into(),
            output_schema: Some(schema()),
            output_schema_strict: strict,
            ..Default::default()
        }
    }

    fn text_response(text: &str) -> ModelResponse {
        ModelResponse {
            events: vec![ModelEvent::TextDelta(text.into())],
            usage: Usage::default(),
            stop_reason: "stop".into(),
            model: None,
        }
    }

    #[test]
    fn completions_valid_text_validates() {
        let mut response = text_response(r#"{"answer": "yes"}"#);
        let value =
            extract_and_validate(Surface::Completions, &request(true), &mut response).unwrap();
        assert_eq!(value["answer"], "yes");
    }

    #[test]
    fn completions_invalid_json_is_a_typed_schema_error_with_literal_feedback() {
        let mut response = text_response("not json at all");
        let err =
            extract_and_validate(Surface::Completions, &request(true), &mut response).unwrap_err();
        let ModelError::OutputSchema(feedback) = &err else {
            panic!("{err:?}")
        };
        assert!(feedback.contains("not valid JSON"), "{feedback}");
        assert!(
            feedback.contains("validates against the schema exactly"),
            "{feedback}"
        );
    }

    #[test]
    fn schema_violation_is_a_typed_schema_error() {
        let mut response = text_response(r#"{"answer": 42}"#);
        let err =
            extract_and_validate(Surface::Completions, &request(true), &mut response).unwrap_err();
        let ModelError::OutputSchema(feedback) = &err else {
            panic!("{err:?}")
        };
        assert!(feedback.contains("did not satisfy"), "{feedback}");
    }

    #[test]
    fn anthropic_strict_extracts_the_forced_tool_input_and_strips_it() {
        let mut response = ModelResponse {
            events: vec![
                ModelEvent::ToolCallComplete(ToolCall {
                    id: "call_1".into(),
                    name: FORCED_OUTPUT_TOOL.into(),
                    arguments: serde_json::json!({"answer": "yes"}),
                }),
                ModelEvent::Done {
                    stop_reason: "tool_use".into(),
                },
            ],
            usage: Usage::default(),
            stop_reason: "tool_use".into(),
            model: None,
        };
        let value =
            extract_and_validate(Surface::Anthropic, &request(true), &mut response).unwrap();
        assert_eq!(value["answer"], "yes");
        // The pseudo-tool never reaches the agent loop.
        assert!(
            !response
                .events
                .iter()
                .any(|e| matches!(e, ModelEvent::ToolCallComplete(_)))
        );
    }

    #[test]
    fn anthropic_strict_without_the_forced_tool_is_a_protocol_error() {
        let mut response = text_response(r#"{"answer": "yes"}"#);
        let err =
            extract_and_validate(Surface::Anthropic, &request(true), &mut response).unwrap_err();
        assert!(matches!(err, ModelError::Protocol(_)));
    }

    #[test]
    fn anthropic_strict_forced_input_still_validates() {
        let mut response = ModelResponse {
            events: vec![ModelEvent::ToolCallComplete(ToolCall {
                id: "call_1".into(),
                name: FORCED_OUTPUT_TOOL.into(),
                arguments: serde_json::json!({"answer": 42}),
            })],
            usage: Usage::default(),
            stop_reason: "tool_use".into(),
            model: None,
        };
        let err =
            extract_and_validate(Surface::Anthropic, &request(true), &mut response).unwrap_err();
        assert!(matches!(err, ModelError::OutputSchema(_)));
    }
}
