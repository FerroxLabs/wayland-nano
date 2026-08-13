//! P1 economy engine tests (design §8): turn-sum journaling, reservation
//! clamp/hard-stop, missing-usage conservative charge + provenance, warn,
//! grant kill-resume determinism, and the grounding-attribution seam.

#[cfg(test)]
mod tests {
    use crate::cost::CostMeter;
    use crate::loop_protection::{ProgressSignals, TurnBudget};
    use crate::turn::{ModelDriver, ToolExecutor, TurnEngine, TurnRobustness, TurnState};
    use nano_model::pricing::PricingCatalog;
    use nano_model::types::{
        ModelError, ModelEvent, ModelObservation, ModelRequest, ModelResponse, ToolCall, Usage,
    };
    use nano_session::NanoErrorKind;
    use nano_session::op::{
        ESTIMATION_METHOD_VERSION, Op, OpEnvelope, TurnOutcome, TurnUsage, UsageSource,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct ScriptedModel {
        responses: Mutex<Vec<Result<ModelResponse, ModelError>>>,
        pub requests: Mutex<Vec<ModelRequest>>,
    }

    impl ScriptedModel {
        fn new(responses: Vec<Result<ModelResponse, ModelError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelDriver for ScriptedModel {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request.clone());
            self.responses.lock().unwrap().remove(0)
        }
    }

    fn text_response(text: &str, input: u64, output: u64) -> ModelResponse {
        ModelResponse {
            events: vec![
                ModelEvent::TextDelta(text.into()),
                ModelEvent::Done {
                    stop_reason: "stop".into(),
                },
            ],
            usage: Usage {
                input_tokens: input,
                output_tokens: output,
                ..Default::default()
            },
            stop_reason: "stop".into(),
            model: None,
        }
    }

    fn tool_response(call: ToolCall, input: u64, output: u64) -> ModelResponse {
        ModelResponse {
            events: vec![
                ModelEvent::ToolCallComplete(call),
                ModelEvent::Done {
                    stop_reason: "tool_calls".into(),
                },
            ],
            usage: Usage {
                input_tokens: input,
                output_tokens: output,
                ..Default::default()
            },
            stop_reason: "tool_calls".into(),
            model: None,
        }
    }

    /// A response carrying NO usage (the §3.5 missing-usage case).
    fn no_usage_response(text: &str) -> ModelResponse {
        ModelResponse {
            events: vec![
                ModelEvent::TextDelta(text.into()),
                ModelEvent::Done {
                    stop_reason: "stop".into(),
                },
            ],
            usage: Usage::default(),
            stop_reason: "stop".into(),
            model: None,
        }
    }

    #[derive(Debug, Default)]
    struct NoopTools;

    #[async_trait::async_trait]
    impl ToolExecutor for NoopTools {
        async fn execute(&self, _call: &ToolCall) -> crate::turn::ToolOutcome {
            crate::turn::ToolOutcome {
                ok: true,
                output: "ran".into(),
                progress: ProgressSignals::default(),
                error_kind: None,
            }
        }
    }

    /// A tool executor that sets the cancel flag mid-turn (drives the
    /// loop-top cancel arm AFTER one usage-bearing response).
    #[derive(Debug)]
    struct CancellingTools {
        flag: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for CancellingTools {
        async fn execute(&self, _call: &ToolCall) -> crate::turn::ToolOutcome {
            self.flag.store(true, Ordering::SeqCst);
            crate::turn::ToolOutcome {
                ok: true,
                output: "ran".into(),
                progress: ProgressSignals::default(),
                error_kind: None,
            }
        }
    }

    fn catalog() -> Arc<PricingCatalog> {
        Arc::new(
            PricingCatalog::from_toml_str(
                "[metered.mock]\ninput_per_mtok_usd = 1.0\noutput_per_mtok_usd = 2.0\n",
            )
            .unwrap(),
        )
    }

    fn make_engine<'a>(
        model: &'a ScriptedModel,
        tools: &'a (dyn ToolExecutor + Sync),
        meter: Option<CostMeter>,
    ) -> TurnEngine<'a> {
        TurnEngine {
            model,
            tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                meter,
                ..Default::default()
            },
        }
    }

    fn turn_end_usage(result: &crate::turn::TurnResult) -> Option<TurnUsage> {
        result.ops.iter().find_map(|envelope| match &envelope.op {
            Op::TurnEnd { usage, .. } => Some(usage.clone()),
            _ => None,
        })?
    }

    fn tool_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "fs_read".into(),
            arguments: serde_json::json!({"path": "note.txt"}),
        }
    }

    /// §8 sum≠last fixture: a multi-step turn where last_usage ≠ sum proves
    /// the journaled payload is the turn-scoped SUM [r2 claude-F3].
    #[tokio::test]
    async fn turn_end_usage_is_the_sum_not_the_last_response() {
        let model = ScriptedModel::new(vec![
            Ok(tool_response(tool_call("c1"), 100, 50)),
            Ok(text_response("done", 10, 5)),
        ]);
        let tools = NoopTools;
        let meter = CostMeter::new("metered", catalog(), None);
        let engine = make_engine(&model, &tools, Some(meter.clone()));
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        // last-response-only is 10/5; the journaled sum is 110/55.
        assert_eq!(result.usage.input_tokens, 10);
        let usage = turn_end_usage(&result).expect("usage journaled");
        assert_eq!(usage.input_tokens, 110, "the SUM, not the last response");
        assert_eq!(usage.output_tokens, 55);
        assert_eq!(usage.usage_source, UsageSource::ProviderReported);
        // Metered cost: 110 in @ $1/Mtok + 55 out @ $2/Mtok → 22000 µ¢.
        assert_eq!(usage.microcents, 22_000);
        assert!(usage.priced);
        // Live meter == journaled sum (replay reconstruction equal).
        assert_eq!(meter.session_usage().total_tokens(), 165);
        // TurnResult carries the same sum (the C6 rollup payload).
        assert_eq!(result.turn_usage, Some(usage));
    }

    /// §3.4: partial usage is journaled for EVERY terminal outcome —
    /// Cancelled and Failed included; nothing consumed is silently dropped.
    #[tokio::test]
    async fn partial_usage_journaled_for_cancelled_and_failed() {
        // Cancelled: one usage-bearing response, then the flag fires.
        let flag = Arc::new(AtomicBool::new(false));
        let model = ScriptedModel::new(vec![Ok(tool_response(tool_call("c1"), 100, 50))]);
        let tools = CancellingTools { flag: flag.clone() };
        let engine = make_engine(&model, &tools, None);
        let result = engine
            .run_turn_cancellable("t1", "go", Some(flag.as_ref()))
            .await;
        let cancelled = turn_end_usage(&result).expect("cancelled turn journals usage");
        assert_eq!(cancelled.input_tokens, 100);
        assert_eq!(cancelled.output_tokens, 50);
        assert!(matches!(
            result.ops.iter().map(|e| &e.op).next_back(),
            Some(Op::TurnEnd {
                outcome: TurnOutcome::Cancelled,
                ..
            })
        ));

        // Failed: one usage-bearing response, then a journaled failure arm
        // (a ToolCall append failure fails the turn journal_unavailable —
        // one of the §3.4 TurnEnd append points; the generic model-error
        // break intentionally journals NO TurnEnd — a stranded TurnBegin
        // replays as Interrupted, the pre-existing C1 semantics).
        let model = ScriptedModel::new(vec![Ok(tool_response(tool_call("c1"), 100, 50))]);
        let tools = NoopTools;
        let engine = make_engine(&model, &tools, None);
        let mut sink = |envelope: &OpEnvelope| !matches!(envelope.op, Op::ToolCall { .. });
        let result = engine.run_turn_streaming("t1", "go", None, &mut sink).await;
        let failed = turn_end_usage(&result).expect("failed turn journals usage");
        assert_eq!(failed.input_tokens, 100);
        assert_eq!(failed.output_tokens, 50);
        assert!(matches!(result.state, TurnState::Failed(_)));
        assert!(matches!(
            result.ops.iter().map(|e| &e.op).next_back(),
            Some(Op::TurnEnd {
                outcome: TurnOutcome::Failed,
                ..
            })
        ));
    }

    /// §3.5 (Q4 + codex-F4): a response with NO usage takes the conservative
    /// charge (input estimate + FULL reserved output, no refund) and the
    /// journaled sum carries the full provenance.
    #[tokio::test]
    async fn missing_usage_charges_conservatively_with_provenance() {
        let model = ScriptedModel::new(vec![Ok(no_usage_response("done"))]);
        let tools = NoopTools;
        let meter = CostMeter::new("metered", catalog(), Some(1_000_000));
        let engine = make_engine(&model, &tools, Some(meter.clone()));
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        let usage = turn_end_usage(&result).expect("usage journaled");
        assert_eq!(usage.usage_source, UsageSource::Estimated);
        assert_eq!(
            usage.estimation_method_version,
            Some(ESTIMATION_METHOD_VERSION)
        );
        let req_in = usage.request_estimate_input.expect("input estimate");
        let req_out = usage.request_estimate_output.expect("reserved output");
        assert_eq!(req_out, 4096, "FULL reserved output, no refund");
        assert_eq!(usage.applied_estimate, Some(req_in + req_out));
        // The meter charged the same — never zero.
        assert_eq!(
            meter.session_usage().total_tokens(),
            req_in + req_out,
            "an under-reporting provider must not become a budget bypass"
        );
    }

    /// §4.2 + §8 clamp test: remaining allowance 10 → the reservation
    /// grants 10 and `max_tokens` is 10 on the wire at the single
    /// ModelRequest build site (asserted in the captured request); the
    /// clamp is logged via the typed BudgetClamp observation.
    #[tokio::test]
    async fn clamped_reservation_bounds_max_tokens_on_the_wire() {
        let meter = CostMeter::new("metered", catalog(), Some(110));
        // Pre-charge 100 of the 110 allowance: remaining is 10.
        meter.record_usage(
            "mock",
            &Usage {
                input_tokens: 60,
                output_tokens: 40,
                ..Default::default()
            },
        );
        let model = ScriptedModel::new(vec![Ok(text_response("done", 3, 2))]);
        let tools = NoopTools;
        let observations: Arc<Mutex<Vec<ModelObservation>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = observations.clone();
        let observer = move |obs: ModelObservation| captured.lock().unwrap().push(obs);
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                observer: Some(&observer),
                meter: Some(meter.clone()),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].max_tokens,
            Some(10),
            "clamped to the reserved allowance at the build site"
        );
        drop(requests);
        let clamped = observations.lock().unwrap();
        assert!(
            clamped.iter().any(|obs| matches!(
                obs,
                ModelObservation::BudgetClamp {
                    requested: 4096,
                    granted: 10
                }
            )),
            "typed clamp notice, never silent: {clamped:?}"
        );
    }

    /// §4.1: once the meter crosses the cap, the next turn hard-stops with
    /// the typed budget_exceeded — never a zero-token request.
    #[tokio::test]
    async fn zero_grant_hard_stops_typed_budget_exceeded() {
        let meter = CostMeter::new("metered", catalog(), Some(50));
        let model = ScriptedModel::new(vec![Ok(text_response("done", 30, 30))]);
        let tools = NoopTools;
        let engine = make_engine(&model, &tools, Some(meter.clone()));
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(meter.budget_state().unwrap().observed, 60);

        // Turn 2: the reservation grants zero → hard stop, typed.
        let model = ScriptedModel::new(vec![Ok(text_response("never sent", 1, 1))]);
        let engine = make_engine(&model, &tools, Some(meter));
        let result = engine.run_turn("t2", "go").await;
        match result.state {
            TurnState::Stopped(ref info) => {
                assert_eq!(info.kind, NanoErrorKind::BudgetExceeded);
                assert!(info.detail.contains("budget_exceeded"), "{info:?}");
                assert!(info.detail.contains("limit=50"), "{info:?}");
            }
            other => panic!("expected typed budget stop, got {other:?}"),
        }
        // No request ever left the build site.
        assert!(model.requests.lock().unwrap().is_empty());
        // The stop journaled TurnEnd with the turn's (empty) usage omitted.
        assert!(matches!(
            result.ops.iter().map(|e| &e.op).next_back(),
            Some(Op::TurnEnd {
                outcome: TurnOutcome::Failed,
                usage: None,
                ..
            })
        ));
    }

    /// §8 input-overshoot honesty test [r2 claude-F4]: a boundary turn
    /// whose INPUT alone crosses the cap completes (input unclampable), the
    /// meter records the overshoot, and the next turn hard-stops — pinning
    /// the output-bounded, input-best-effort guarantee.
    #[tokio::test]
    async fn input_overshoot_is_recorded_and_the_next_turn_stops() {
        let meter = CostMeter::new("metered", catalog(), Some(100));
        let model = ScriptedModel::new(vec![Ok(text_response("done", 150, 20))]);
        let tools = NoopTools;
        let engine = make_engine(&model, &tools, Some(meter.clone()));
        let result = engine.run_turn("t1", "go").await;
        // The turn COMPLETES: output was clamped (max_tokens 100 on the
        // wire), input 150 was unclampable.
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(model.requests.lock().unwrap()[0].max_tokens, Some(100));
        // The meter records the overshoot honestly (170 > 100).
        assert_eq!(meter.budget_state().unwrap().observed, 170);

        let model = ScriptedModel::new(vec![Ok(text_response("never sent", 1, 1))]);
        let engine = make_engine(&model, &tools, Some(meter));
        let result = engine.run_turn("t2", "go").await;
        assert!(matches!(
            result.state,
            TurnState::Stopped(ref info) if info.kind == NanoErrorKind::BudgetExceeded
        ));
    }

    /// §4.1: the 80% crossing emits the typed BudgetWarn observation
    /// `{limit, observed, pct_used}` once per crossing.
    #[tokio::test]
    async fn budget_warn_fires_at_eighty_percent() {
        let meter = CostMeter::new("metered", catalog(), Some(100));
        let model = ScriptedModel::new(vec![Ok(text_response("done", 50, 40))]);
        let tools = NoopTools;
        let observations: Arc<Mutex<Vec<ModelObservation>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = observations.clone();
        let observer = move |obs: ModelObservation| captured.lock().unwrap().push(obs);
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                observer: Some(&observer),
                meter: Some(meter),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        let captured = observations.lock().unwrap();
        assert!(
            captured.iter().any(|obs| matches!(
                obs,
                ModelObservation::BudgetWarn {
                    limit: 100,
                    observed: 90,
                    pct_used: 90
                }
            )),
            "typed BudgetWarn at the crossing: {captured:?}"
        );
    }

    /// §8 scripted cap battery: session_tokens = 100, 60-token turns →
    /// turn 2 warns (80%), turn 3 hard-stops typed; the grant resumes.
    #[tokio::test]
    async fn scripted_cap_warn_stop_grant_resume() {
        let meter = CostMeter::new("metered", catalog(), Some(100));
        let tools = NoopTools;
        // Turn 1: 60 tokens. Turn 2: 60 more → 120 total, warn crossed.
        for turn in ["t1", "t2"] {
            let model = ScriptedModel::new(vec![Ok(text_response("done", 40, 20))]);
            let engine = make_engine(&model, &tools, Some(meter.clone()));
            let result = engine.run_turn(turn, "go").await;
            assert_eq!(result.state, TurnState::Complete);
        }
        assert_eq!(meter.budget_state().unwrap().observed, 120);
        // Turn 3: hard stop.
        let model = ScriptedModel::new(vec![Ok(text_response("never", 1, 1))]);
        let engine = make_engine(&model, &tools, Some(meter.clone()));
        let result = engine.run_turn("t3", "go").await;
        assert!(matches!(
            result.state,
            TurnState::Stopped(ref info) if info.kind == NanoErrorKind::BudgetExceeded
        ));
        // Without the grant every turn hard-stops; with it, turns resume.
        assert_eq!(meter.apply_grant(200), Some(300));
        let model = ScriptedModel::new(vec![Ok(text_response("done", 40, 20))]);
        let engine = make_engine(&model, &tools, Some(meter));
        let result = engine.run_turn("t4", "go").await;
        assert_eq!(result.state, TurnState::Complete);
    }

    /// §4.3 grant kill-resume determinism: replay folds TurnEnd.usage +
    /// BudgetGrant; a reseeded meter reconstructs the exact effective limit
    /// and position; a retried (duplicate-envelope) grant folds once.
    #[test]
    fn grant_and_usage_replay_reconstruct_the_budget_position() {
        let usage = || {
            let mut u = TurnUsage::default();
            u.add_provider_reported(40, 20, 0, 0, 0, true);
            u
        };
        let envelopes = vec![
            OpEnvelope::new(
                "s-1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "C:\\repo".into(),
                },
            ),
            OpEnvelope::new(
                "s-2",
                "now",
                Op::TurnEnd {
                    turn_id: "t1".into(),
                    outcome: TurnOutcome::Completed,
                    usage: Some(usage()),
                },
            ),
            OpEnvelope::new(
                "s-3",
                "now",
                Op::TurnEnd {
                    turn_id: "t2".into(),
                    outcome: TurnOutcome::Cancelled,
                    usage: Some(usage()),
                },
            ),
            OpEnvelope::new(
                "s-4",
                "now",
                Op::BudgetGrant {
                    grant_id: "s-grant-1".into(),
                    tokens: 200,
                    after_limit: 300,
                },
            ),
            // A retried grant append (same envelope id): folds once.
            OpEnvelope::new(
                "s-4",
                "now",
                Op::BudgetGrant {
                    grant_id: "s-grant-1".into(),
                    tokens: 200,
                    after_limit: 300,
                },
            ),
        ];
        let folded = nano_session::SessionState::fold(&envelopes);
        assert_eq!(folded.session_usage.total_tokens(), 120);
        assert_eq!(folded.budget_granted_tokens, 200);
        assert_eq!(folded.budget_after_limit, Some(300));

        // The resumed session's meter: cap 100 + journaled grant 200.
        let meter = CostMeter::new("metered", catalog(), Some(100));
        meter.reseed(&folded.session_usage, folded.budget_granted_tokens);
        let state = meter.budget_state().unwrap();
        assert_eq!(state.limit, 300);
        assert_eq!(state.observed, 120);
        // Reservations account against the reconstructed position.
        let res = meter.reserve_output(200);
        assert_eq!(res.granted(), 180);
    }

    /// r3 codex-F1: externally-attributed (grounding) usage routed through
    /// the turn's extra-usage cell lands in the journaled turn sum BEFORE
    /// terminal journaling — live meter == journaled sum == replay,
    /// searches included.
    #[tokio::test]
    async fn grounding_usage_routes_into_the_owning_turns_sum() {
        let model = ScriptedModel::new(vec![Ok(text_response("done", 10, 5))]);
        let tools = NoopTools;
        let extra: Arc<Mutex<TurnUsage>> = Arc::new(Mutex::new(TurnUsage::default()));
        // The search lane's backend records the grounding round trip here
        // (and into the meter through the UsageSink handle).
        extra
            .lock()
            .unwrap()
            .add_provider_reported(30, 20, 0, 0, 0, false);
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                extra_usage: Some(extra),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "go").await;
        let usage = turn_end_usage(&result).expect("usage journaled");
        assert_eq!(usage.input_tokens, 40, "10 model + 30 grounding");
        assert_eq!(usage.output_tokens, 25, "5 model + 20 grounding");
    }
}
