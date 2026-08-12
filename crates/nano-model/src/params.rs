//! Cross-surface model params (C9 §4): reasoning effort, verbosity, and
//! structured output. The Q3 capability ladder is uniform across all three:
//!
//! 1. **Verified supported** → mapped onto the wire.
//! 2. **Unverified** → OMITTED from the wire + a typed `ParamInert`
//!    observation — never a guessed parameter name on the wire (an
//!    unverified name risks 400s on every request, worse than absent
//!    capability). Nothing is dropped silently.
//! 3. **Known-unsupported** → `ModelError::UnsupportedParam` at
//!    request-build, BEFORE any network I/O; the message is actionable (it
//!    names the setting to clear).
//!
//! The per-(surface, model, param) rungs below are pinned by in-phase
//! `live_smoke` probes (see live_smoke.rs); a probe verdict upgrades a rung
//! with a recorded fixture, never by editing call sites.

use crate::types::{ModelError, ModelObservation, ModelRequest, ReasoningEffort};

/// The three wire surfaces this crate speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Flux Chat Completions — the production wire.
    Completions,
    /// Flux Responses.
    Responses,
    /// Flux Anthropic Messages (COMPAT only — WIRE-2).
    Anthropic,
}

impl Surface {
    pub fn label(&self) -> &'static str {
        match self {
            Surface::Completions => "flux-completions",
            Surface::Responses => "flux-responses",
            Surface::Anthropic => "flux-anthropic-compat",
        }
    }
}

/// A cross-surface param the ladder judges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param {
    ReasoningEffort,
    Verbosity,
    OutputSchema,
}

impl Param {
    pub fn label(&self) -> &'static str {
        match self {
            Param::ReasoningEffort => "reasoning_effort",
            Param::Verbosity => "verbosity",
            Param::OutputSchema => "output_schema",
        }
    }
}

/// The Q3 ladder rung for one (surface, model, param).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// Verified supported: map onto the wire.
    Verified,
    /// Unverified: omit from the wire + typed notice.
    Unverified,
    /// Known-unsupported: reject before network I/O.
    Unsupported,
}

/// Per-model reasoning classification, pinned by the vendored fixture
/// catalog ids (flux_models fixture test carries exactly these tiers).
/// `Some(true)` = reasoning tier, `Some(false)` = known non-reasoning tier,
/// `None` = a model the fixture catalog does not name (unverified).
pub fn reasoning_model_class(model: &str) -> Option<bool> {
    match model {
        "flux-reasoning" => Some(true),
        "flux-auto" | "flux-standard" | "flux-fast" => Some(false),
        _ => None,
    }
}

/// The ladder verdict for one (surface, param, model).
///
/// Recorded rung rationales (live_smoke probe fixtures are the upgrade
/// path for the Unverified rungs — see the module header):
/// - Anthropic effort maps to the documented Anthropic `thinking` block —
///   the MAPPING is verified Anthropic-native semantics — but live Flux
///   accepts-and-IGNORES it (fixture_tests WIRE-2: zero thinking blocks in
///   recorded responses), so every mapped request ALSO emits the loud
///   inert notice.
/// - Anthropic has no verbosity primitive: known-unsupported. There is no
///   system-prompt fakery, ever.
/// - Anthropic non-strict output schema: known-unsupported — schema-in-
///   prompt is prompt engineering, NEVER structured output.
/// - Completions/Responses effort/verbosity: probed live (Q3 in-phase,
///   `probe_reasoning_effort.json` / `probe_verbosity.json` /
///   `probe_validation_sharpness.json`): the wire accepts even BOGUS
///   values (200), i.e. the params are accepted-and-unvalidated — not
///   verifiable, so they stay UNVERIFIED: omitted with a typed notice,
///   never a guessed name relied upon.
/// - Completions response_format json_schema: probed live — the wire
///   REJECTS it (HTTP 400 naming response_format,
///   `probe_response_format.json`). Known-unsupported on this surface:
///   deterministic pre-network rejection, never a 400 on every request.
/// - Responses response_format: unprobed → Unverified (omit + notice; the
///   extracted output is still validated before the caller sees it).
pub fn ladder(surface: Surface, param: Param, model: &str) -> Rung {
    match (surface, param) {
        (Surface::Anthropic, Param::ReasoningEffort) => Rung::Verified,
        (Surface::Anthropic, Param::Verbosity) => Rung::Unsupported,
        (Surface::Anthropic, Param::OutputSchema) => Rung::Verified,
        (Surface::Completions, Param::OutputSchema) => Rung::Unsupported,
        (_, Param::ReasoningEffort) => match reasoning_model_class(model) {
            Some(true) => Rung::Unverified,
            // Effort on a known non-reasoning model is rejected with an
            // actionable config error — never silently downgraded.
            Some(false) => Rung::Unsupported,
            None => Rung::Unverified,
        },
        (_, Param::Verbosity) => Rung::Unverified,
        (_, Param::OutputSchema) => Rung::Unverified,
    }
}

/// The actionable known-unsupported error (rung 3). Names the setting to
/// clear so a sticky effort config + a mid-session set_model switch to a
/// non-reasoning model yields an actionable error, not an opaque failure.
fn unsupported(surface: Surface, param: Param, message: String) -> ModelError {
    ModelError::UnsupportedParam {
        param: param.label().to_string(),
        surface: surface.label().to_string(),
        message,
    }
}

/// The loud inert/omitted notice (rung 2, and the Anthropic effort
/// recorded-inert caveat). Typed fields; UIs render, never parse.
fn inert_notice(surface: Surface, param: Param, detail: impl Into<String>) -> ModelObservation {
    ModelObservation::ParamInert {
        param: param.label().to_string(),
        surface: surface.label().to_string(),
        detail: detail.into(),
    }
}

/// Anthropic `thinking` budget per effort level (the verified Anthropic
/// mapping; recorded-inert on live Flux — WIRE-2). Anthropic requires
/// budget_tokens >= 1024 and < max_tokens; the caller owns max_tokens.
pub fn thinking_budget_tokens(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Low => 1024,
        ReasoningEffort::Medium => 4096,
        ReasoningEffort::High => 16384,
    }
}

/// Apply the C9 params to a built request body, per the ladder. Returns the
/// typed notices the caller MUST surface through the observation channel.
/// Rung-3 verdicts error here — before any network I/O.
///
/// `build` produced the base body; this function only ADDS verified fields,
/// so an unverified param can never leak a guessed name onto the wire.
pub fn apply_params(
    surface: Surface,
    body: &mut serde_json::Value,
    request: &ModelRequest,
) -> Result<Vec<ModelObservation>, ModelError> {
    let mut notices = Vec::new();

    if let Some(effort) = request.reasoning_effort {
        match ladder(surface, Param::ReasoningEffort, &request.model) {
            Rung::Verified => match surface {
                Surface::Anthropic => {
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": thinking_budget_tokens(effort),
                    });
                    // Loud, always: the mapping is honest passthrough but
                    // live Flux drops thinking (WIRE-2 fixture-pinned).
                    notices.push(inert_notice(
                        surface,
                        Param::ReasoningEffort,
                        "thinking is mapped but recorded-inert on live Flux (WIRE-2: zero thinking blocks in recorded responses)",
                    ));
                }
                // Verified Completions/Responses mapping lands here once a
                // live_smoke probe verifies the wire name; until then the
                // ladder never returns Verified for these surfaces.
                _ => unreachable!("ladder gates Verified to Anthropic effort"),
            },
            Rung::Unverified => notices.push(inert_notice(
                surface,
                Param::ReasoningEffort,
                format!(
                    "reasoning_effort={} omitted from the wire: unverified on this surface/model",
                    effort.as_str()
                ),
            )),
            Rung::Unsupported => {
                return Err(unsupported(
                    surface,
                    Param::ReasoningEffort,
                    format!(
                        "model '{}' has no reasoning tier; clear `reasoning_effort` in config (NANO_REASONING_EFFORT) or switch back to a reasoning model",
                        request.model
                    ),
                ));
            }
        }
    }

    if let Some(verbosity) = request.verbosity {
        match ladder(surface, Param::Verbosity, &request.model) {
            Rung::Verified => unreachable!("no verbosity mapping is verified"),
            Rung::Unverified => notices.push(inert_notice(
                surface,
                Param::Verbosity,
                format!(
                    "verbosity={} omitted from the wire: unverified on this surface/model",
                    verbosity.as_str()
                ),
            )),
            Rung::Unsupported => {
                return Err(unsupported(
                    surface,
                    Param::Verbosity,
                    "this surface has no verbosity primitive; clear `verbosity` in config (NANO_VERBOSITY) — no system-prompt fakery is ever substituted".into(),
                ));
            }
        }
    }

    if let Some(schema) = &request.output_schema {
        if surface == Surface::Anthropic && !request.output_schema_strict {
            // Schema-in-prompt is prompt engineering, NEVER structured
            // output: it cannot satisfy the fail-closed schema guarantee.
            return Err(unsupported(
                surface,
                Param::OutputSchema,
                "non-strict output_schema is not structured output on this surface (schema-in-prompt is never accepted); request strict mode or clear `output_schema`".into(),
            ));
        }
        match ladder(surface, Param::OutputSchema, &request.model) {
            Rung::Verified => match surface {
                Surface::Anthropic => {
                    // Strict mode: a forced single tool whose input_schema
                    // IS the schema; the extracted value is the forced tool
                    // call's input, never the final text.
                    let tools = body["tools"].as_array_mut();
                    let forced = serde_json::json!({
                        "name": crate::structured::FORCED_OUTPUT_TOOL,
                        "description": "Emit the structured output. Its input is validated against the caller-supplied JSON schema.",
                        "input_schema": schema,
                    });
                    match tools {
                        Some(tools) => tools.push(forced),
                        None => body["tools"] = serde_json::json!([forced]),
                    }
                    body["tool_choice"] = serde_json::json!({
                        "type": "tool",
                        "name": crate::structured::FORCED_OUTPUT_TOOL,
                    });
                }
                _ => unreachable!("ladder gates Verified schema to Anthropic"),
            },
            Rung::Unverified => notices.push(inert_notice(
                surface,
                Param::OutputSchema,
                "response_format omitted from the wire: unverified on this surface; the extracted output is still validated against the schema before the caller sees it",
            )),
            Rung::Unsupported => {
                return Err(unsupported(
                    surface,
                    Param::OutputSchema,
                    "the live wire rejects response_format (in-phase probe: HTTP 400 naming the argument); clear `output_schema` or use a surface with verified structured output".into(),
                ));
            }
        }
    }

    Ok(notices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Verbosity;

    fn request_with(model: &str) -> ModelRequest {
        ModelRequest {
            model: model.into(),
            ..Default::default()
        }
    }

    #[test]
    fn effort_on_known_non_reasoning_model_is_an_actionable_typed_error() {
        let mut request = request_with("flux-fast");
        request.reasoning_effort = Some(ReasoningEffort::High);
        let mut body = serde_json::json!({"model": "flux-fast"});
        let err = apply_params(Surface::Completions, &mut body, &request).unwrap_err();
        let ModelError::UnsupportedParam {
            param,
            surface,
            message,
        } = &err
        else {
            panic!("typed UnsupportedParam expected: {err:?}");
        };
        assert_eq!(param, "reasoning_effort");
        assert_eq!(surface, "flux-completions");
        // Actionable: names the setting to clear AND the alternative.
        assert!(message.contains("reasoning_effort"), "{message}");
        assert!(message.contains("NANO_REASONING_EFFORT"), "{message}");
        assert!(message.contains("flux-fast"), "{message}");
        // Nothing reached the body.
        assert_eq!(body, serde_json::json!({"model": "flux-fast"}));
    }

    #[test]
    fn unverified_effort_is_omitted_with_a_typed_notice_never_guessed() {
        let mut request = request_with("flux-reasoning");
        request.reasoning_effort = Some(ReasoningEffort::Low);
        let mut body = serde_json::json!({"model": "flux-reasoning"});
        let notices = apply_params(Surface::Completions, &mut body, &request).unwrap();
        assert_eq!(body, serde_json::json!({"model": "flux-reasoning"}));
        assert_eq!(notices.len(), 1);
        let ModelObservation::ParamInert { param, detail, .. } = &notices[0] else {
            panic!("typed inert notice expected: {:?}", notices[0]);
        };
        assert_eq!(param, "reasoning_effort");
        assert!(detail.contains("omitted"));
    }

    #[test]
    fn unknown_model_effort_is_unverified_not_unsupported() {
        let mut request = request_with("some-future-model");
        request.reasoning_effort = Some(ReasoningEffort::Low);
        let mut body = serde_json::json!({});
        let notices = apply_params(Surface::Completions, &mut body, &request).unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(body, serde_json::json!({}));
    }

    #[test]
    fn anthropic_effort_maps_thinking_with_the_loud_inert_notice() {
        let mut request = request_with("flux-auto");
        request.reasoning_effort = Some(ReasoningEffort::Medium);
        let mut body = serde_json::json!({"model": "flux-auto"});
        let notices = apply_params(Surface::Anthropic, &mut body, &request).unwrap();
        assert_eq!(
            body["thinking"],
            serde_json::json!({"type": "enabled", "budget_tokens": 4096})
        );
        assert_eq!(notices.len(), 1);
        let ModelObservation::ParamInert { detail, .. } = &notices[0] else {
            panic!()
        };
        assert!(detail.contains("recorded-inert"), "{detail}");
    }

    #[test]
    fn anthropic_verbosity_is_rejected_pre_network() {
        let mut request = request_with("flux-auto");
        request.verbosity = Some(Verbosity::Low);
        let mut body = serde_json::json!({});
        let err = apply_params(Surface::Anthropic, &mut body, &request).unwrap_err();
        assert!(matches!(err, ModelError::UnsupportedParam { .. }));
        assert_eq!(body, serde_json::json!({}));
    }

    #[test]
    fn anthropic_non_strict_schema_is_rejected_pre_network() {
        let mut request = request_with("flux-auto");
        request.output_schema = Some(serde_json::json!({"type": "object"}));
        request.output_schema_strict = false;
        let mut body = serde_json::json!({});
        let err = apply_params(Surface::Anthropic, &mut body, &request).unwrap_err();
        let ModelError::UnsupportedParam { message, .. } = &err else {
            panic!("{err:?}")
        };
        assert!(message.contains("never accepted"), "{message}");
    }

    #[test]
    fn anthropic_strict_schema_maps_to_the_forced_tool() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"]
        });
        let mut request = request_with("flux-auto");
        request.output_schema = Some(schema.clone());
        let mut body = serde_json::json!({"model": "flux-auto"});
        let notices = apply_params(Surface::Anthropic, &mut body, &request).unwrap();
        assert!(notices.is_empty());
        let tools = body["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], crate::structured::FORCED_OUTPUT_TOOL);
        assert_eq!(tools[0]["input_schema"], schema);
        assert_eq!(
            body["tool_choice"],
            serde_json::json!({"type": "tool", "name": crate::structured::FORCED_OUTPUT_TOOL})
        );
    }

    #[test]
    fn completions_schema_is_known_unsupported_probed_400() {
        // In-phase live probe (probe_response_format.json): the wire
        // REJECTS response_format with a 400 → rung 3, deterministic
        // pre-network rejection, never a 400 on every request.
        let mut request = request_with("flux-auto");
        request.output_schema = Some(serde_json::json!({"type": "object"}));
        let mut body = serde_json::json!({"model": "flux-auto"});
        let err = apply_params(Surface::Completions, &mut body, &request).unwrap_err();
        let ModelError::UnsupportedParam { message, .. } = &err else {
            panic!("{err:?}")
        };
        assert!(message.contains("output_schema"), "{message}");
        assert!(!body.as_object().unwrap().contains_key("response_format"));
    }

    #[test]
    fn responses_schema_is_unverified_omitted_with_notice() {
        let mut request = request_with("flux-auto");
        request.output_schema = Some(serde_json::json!({"type": "object"}));
        let mut body = serde_json::json!({"model": "flux-auto"});
        let notices = apply_params(Surface::Responses, &mut body, &request).unwrap();
        assert!(!body.as_object().unwrap().contains_key("response_format"));
        assert_eq!(notices.len(), 1);
    }
}
