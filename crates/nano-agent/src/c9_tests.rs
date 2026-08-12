//! C9 turn-engine robustness tests: steer queue integration (journal-first
//! drain, cancel-beats-steer, adversarial approval timing), the one-shot
//! 401 refresh seam, the journaled schema re-ask, and observation
//! forwarding/coalescing.

#[cfg(test)]
mod tests {
    use crate::loop_protection::TurnBudget;
    use crate::steer::{EnqueueAck, SteerHandle, SteerItem};
    use crate::turn::{
        ApprovalDecision, ApprovalGate, ModelDriver, ToolExecutor, TurnEngine, TurnRobustness,
        TurnState,
    };
    use nano_model::auth::{AuthRefresh, RefreshOutcome};
    use nano_model::rate_limits::RateLimitSnapshot;
    use nano_model::types::{
        CallHooks, ModelError, ModelEvent, ModelObservation, ModelRequest, ModelResponse, ToolCall,
        Usage,
    };
    use nano_session::op::Op;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;

    fn text_response(text: &str) -> ModelResponse {
        ModelResponse {
            events: vec![
                ModelEvent::TextDelta(text.into()),
                ModelEvent::Done {
                    stop_reason: "stop".into(),
                },
            ],
            usage: Usage::default(),
            stop_reason: "stop".into(),
        }
    }

    /// A scripted model whose responses can FAIL, and which records every
    /// request it saw. `on_call` runs inside each call — the mid-turn
    /// submission point for steers/cancels.
    struct ScriptedModel {
        outcomes: Mutex<std::collections::VecDeque<Result<ModelResponse, ModelError>>>,
        requests: Mutex<Vec<ModelRequest>>,
        on_call: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl std::fmt::Debug for ScriptedModel {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ScriptedModel").finish_non_exhaustive()
        }
    }

    impl ScriptedModel {
        fn new(outcomes: Vec<Result<ModelResponse, ModelError>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                requests: Mutex::new(Vec::new()),
                on_call: None,
            }
        }

        fn with_on_call(mut self, on_call: Box<dyn Fn() + Send + Sync>) -> Self {
            self.on_call = Some(on_call);
            self
        }
    }

    #[async_trait::async_trait]
    impl ModelDriver for ScriptedModel {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request.clone());
            if let Some(on_call) = &self.on_call {
                on_call();
            }
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted outcome")
        }
    }

    /// A model that speaks the hooked protocol: emits scripted observations
    /// through the CallHooks observer before returning.
    #[derive(Debug)]
    struct ObservingModel {
        observations: Vec<ModelObservation>,
        inner: ScriptedModel,
    }

    #[async_trait::async_trait]
    impl ModelDriver for ObservingModel {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.inner.complete(request).await
        }

        async fn complete_observed(
            &self,
            request: &ModelRequest,
            hooks: &CallHooks<'_>,
        ) -> Result<ModelResponse, ModelError> {
            for observation in &self.observations {
                hooks.observe(observation.clone());
            }
            self.inner.complete(request).await
        }
    }

    #[derive(Debug)]
    struct NoopTools;
    #[async_trait::async_trait]
    impl ToolExecutor for NoopTools {
        async fn execute(&self, call: &ToolCall) -> crate::turn::ToolOutcome {
            crate::turn::ToolOutcome {
                ok: true,
                output: format!("ran {}", call.name),
                progress: Default::default(),
            }
        }
    }

    fn engine<'a>(model: &'a dyn ModelDriver, robustness: TurnRobustness<'a>) -> TurnEngine<'a> {
        TurnEngine {
            model,
            tools: &NoopTools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness,
        }
    }

    fn snapshot(ms: u64) -> RateLimitSnapshot {
        RateLimitSnapshot {
            captured_at_ms: ms,
            scope: Some("test".into()),
            requests_limit: Some(100),
            requests_remaining: Some(90),
            requests_reset_ms: None,
            tokens_limit: None,
            tokens_remaining: None,
            tokens_reset_ms: None,
        }
    }

    // ── Steer: drain, journal-first, lifecycle ───────────────────────────

    #[tokio::test]
    async fn steer_drains_at_loop_top_journaled_before_history_mutation() {
        let handle = SteerHandle::silent("t1");
        let steer = handle.clone();
        // The steer arrives MID-TURN: submitted from inside the first model
        // call (the only safe simulation of a concurrent submitter).
        let fired = std::sync::Arc::new(AtomicBool::new(false));
        let model = ScriptedModel::new(vec![
            Ok(text_response("working on it")),
            Ok(text_response("done with the steer in view")),
        ])
        .with_on_call(Box::new(move || {
            if fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return; // submit exactly once, on the first call
            }
            assert_eq!(
                steer.enqueue(
                    "submitter-1".into(),
                    "actually, also check the tests".into()
                ),
                EnqueueAck::Queued { position: 1 }
            );
        }));
        let engine = engine(
            &model,
            TurnRobustness {
                steer: Some(handle),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "fix the build").await;
        assert_eq!(result.state, TurnState::Complete);
        // The pending steer made the turn CONTINUE past the no-tool-calls
        // point (codex turn.rs:395 parity): two model calls happened.
        assert_eq!(model.requests.lock().unwrap().len(), 2);
        // Journal-first: Op::SteerInput is in the ops BEFORE the model saw
        // the text — and the second request carries it as a user message
        // appended AFTER the completed assistant text.
        let steers: Vec<&str> = result
            .ops
            .iter()
            .filter_map(|e| match &e.op {
                Op::SteerInput { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(steers, vec!["actually, also check the tests"]);
        let second = &model.requests.lock().unwrap()[1];
        let last = second.messages.last().expect("steer message");
        assert_eq!(last.role, nano_model::types::Role::User);
        assert!(
            matches!(&last.content[0], nano_model::types::ContentBlock::Text { text } if text == "actually, also check the tests")
        );
        // The queue was closed at turn end.
        assert_eq!(
            engine
                .robustness
                .steer
                .as_ref()
                .unwrap()
                .enqueue("late".into(), "too late".into()),
            EnqueueAck::RejectedClosed
        );
    }

    #[tokio::test]
    async fn steer_journal_append_failure_aborts_fail_closed_history_unmutated() {
        let handle = SteerHandle::silent("t1");
        let steer = handle.clone();
        let model = ScriptedModel::new(vec![Ok(text_response("step one"))]).with_on_call(Box::new(
            move || {
                steer.enqueue("s".into(), "mid-turn steer".into());
            },
        ));
        let engine = engine(
            &model,
            TurnRobustness {
                steer: Some(handle),
                ..Default::default()
            },
        );
        // The sink refuses the SteerInput append (journal failure).
        let mut sink = |envelope: &nano_session::op::OpEnvelope| -> bool {
            !matches!(envelope.op, Op::SteerInput { .. })
        };
        let result = engine.run_turn_streaming("t1", "go", None, &mut sink).await;
        let TurnState::Failed(reason) = &result.state else {
            panic!("fail-closed abort expected: {:?}", result.state)
        };
        assert!(reason.contains("steer journal append failed"), "{reason}");
        // History was left unmutated: the model was never re-called.
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancel_beats_steer_with_per_submitter_drop_notification() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let drops: std::sync::Arc<Mutex<Vec<SteerItem>>> =
            std::sync::Arc::new(Mutex::new(Vec::new()));
        let handle = SteerHandle::new("t1", 8, {
            let drops = drops.clone();
            std::sync::Arc::new(move |item| drops.lock().unwrap().push(item))
        });
        let steer = handle.clone();
        let model = ScriptedModel::new(vec![Ok(text_response("step"))]).with_on_call({
            let cancel = cancel.clone();
            Box::new(move || {
                // A steer lands mid-turn AND the cancel fires: cancel wins.
                steer.enqueue("wire-req-7".into(), "never seen".into());
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        });
        let engine = engine(
            &model,
            TurnRobustness {
                steer: Some(handle),
                ..Default::default()
            },
        );
        let result = engine
            .run_turn_cancellable("t1", "go", Some(cancel.as_ref()))
            .await;
        assert!(
            matches!(result.state, TurnState::Stopped(_)),
            "cancel wins: {result:?}"
        );
        assert!(
            result
                .ops
                .iter()
                .any(|e| matches!(&e.op, Op::TurnEnd { outcome, .. } if *outcome == nano_session::op::TurnOutcome::Cancelled))
        );
        // The pending steer was NEVER journaled — the model never saw it,
        // and replay can never resurrect it.
        assert!(
            !result
                .ops
                .iter()
                .any(|e| matches!(e.op, Op::SteerInput { .. }))
        );
        // …and its submitter got exactly one drop notification.
        let drops = drops.lock().unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].submitter, "wire-req-7");
        assert_eq!(drops[0].text, "never seen");
    }

    /// The adversarial case (design §3.2): a steer submitted WHILE a
    /// permission request is outstanding is accepted into the queue but
    /// CANNOT take effect until the batch completes and the loop returns
    /// to the top — structurally guaranteed by the single drain point.
    #[tokio::test]
    async fn steer_during_approval_never_drains_mid_batch() {
        #[derive(Debug)]
        struct SteeringGate {
            steer: SteerHandle,
        }
        impl ApprovalGate for SteeringGate {
            fn approve(&self, _call: &ToolCall) -> ApprovalDecision {
                // The steer arrives while this approval is outstanding.
                assert_eq!(
                    self.steer
                        .enqueue("s".into(), "while you were deciding".into()),
                    EnqueueAck::Queued { position: 1 }
                );
                ApprovalDecision::Approve
            }
        }
        let handle = SteerHandle::silent("t1");
        let gate = SteeringGate {
            steer: handle.clone(),
        };
        let model = ScriptedModel::new(vec![
            Ok(ModelResponse {
                events: vec![
                    ModelEvent::ToolCallComplete(ToolCall {
                        id: "c1".into(),
                        name: "fs_write".into(),
                        arguments: serde_json::json!({"path": "a", "content": "b"}),
                    }),
                    ModelEvent::Done {
                        stop_reason: "tool_calls".into(),
                    },
                ],
                usage: Usage::default(),
                stop_reason: "tool_calls".into(),
            }),
            Ok(text_response("batch done, steer seen")),
        ]);
        let tools = NoopTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: Some(&gate),
            compaction: None,
            robustness: TurnRobustness {
                steer: Some(handle),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        let kind = |e: &nano_session::op::OpEnvelope| match &e.op {
            Op::ToolCall { .. } => "tool_call",
            Op::ToolResult { .. } => "tool_result",
            Op::SteerInput { .. } => "steer",
            _ => "other",
        };
        let kinds: Vec<&str> = result.ops.iter().map(kind).collect();
        let tool_result = kinds.iter().position(|k| *k == "tool_result").unwrap();
        let steer = kinds.iter().position(|k| *k == "steer").unwrap();
        assert!(
            tool_result < steer,
            "no drain until batch completion: {kinds:?}"
        );
        // Adjacency intact on the wire: the steered user message lands
        // AFTER the fully paired batch (tool_use → tool_result).
        let second = &model.requests.lock().unwrap()[1];
        let roles: Vec<&str> = second
            .messages
            .iter()
            .map(|m| match m.role {
                nano_model::types::Role::User => "user",
                nano_model::types::Role::Assistant => "assistant",
                nano_model::types::Role::Tool => "tool",
                nano_model::types::Role::System => "system",
            })
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "tool", "user"]);
    }

    // ── 401 seam ─────────────────────────────────────────────────────────

    #[derive(Debug)]
    struct MockRefresh {
        outcome: RefreshOutcome,
        calls: Mutex<u32>,
    }

    #[async_trait::async_trait]
    impl AuthRefresh for MockRefresh {
        async fn refresh(&self) -> RefreshOutcome {
            *self.calls.lock().unwrap() += 1;
            self.outcome.clone()
        }
    }

    fn auth_401() -> ModelError {
        ModelError::Auth {
            message: "expired".into(),
            status: Some(401),
        }
    }

    #[tokio::test]
    async fn static_key_401_takes_zero_retries() {
        let model = ScriptedModel::new(vec![Err(auth_401())]);
        let engine = engine(&model, TurnRobustness::default());
        let result = engine.run_turn("t1", "go").await;
        assert!(matches!(result.state, TurnState::Failed(_)));
        assert_eq!(model.requests.lock().unwrap().len(), 1, "zero retries");
    }

    #[tokio::test]
    async fn refreshable_provider_gets_exactly_one_retry_after_refresh() {
        let model = ScriptedModel::new(vec![Err(auth_401()), Ok(text_response("recovered"))]);
        let refresh = MockRefresh {
            outcome: RefreshOutcome::Refreshed,
            calls: Mutex::new(0),
        };
        let engine = engine(
            &model,
            TurnRobustness {
                auth_refresh: Some(&refresh),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(*refresh.calls.lock().unwrap(), 1);
        assert_eq!(model.requests.lock().unwrap().len(), 2, "exactly one retry");
    }

    #[tokio::test]
    async fn second_401_after_successful_refresh_is_terminal() {
        let model = ScriptedModel::new(vec![Err(auth_401()), Err(auth_401())]);
        let refresh = MockRefresh {
            outcome: RefreshOutcome::Refreshed,
            calls: Mutex::new(0),
        };
        let engine = engine(
            &model,
            TurnRobustness {
                auth_refresh: Some(&refresh),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "go").await;
        assert!(matches!(result.state, TurnState::Failed(_)));
        assert_eq!(*refresh.calls.lock().unwrap(), 1, "one refresh only");
        assert_eq!(model.requests.lock().unwrap().len(), 2, "no third call");
    }

    #[tokio::test]
    async fn forbidden_403_never_retries_and_never_refreshes() {
        let model = ScriptedModel::new(vec![Err(ModelError::Auth {
            message: "forbidden".into(),
            status: Some(403),
        })]);
        let refresh = MockRefresh {
            outcome: RefreshOutcome::Refreshed,
            calls: Mutex::new(0),
        };
        let engine = engine(
            &model,
            TurnRobustness {
                auth_refresh: Some(&refresh),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "go").await;
        assert!(matches!(result.state, TurnState::Failed(_)));
        assert_eq!(*refresh.calls.lock().unwrap(), 0);
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn refresh_failure_and_not_refreshable_are_terminal() {
        for outcome in [
            RefreshOutcome::Failed("oauth down".into()),
            RefreshOutcome::NotRefreshable,
        ] {
            let model = ScriptedModel::new(vec![Err(auth_401())]);
            let refresh = MockRefresh {
                outcome,
                calls: Mutex::new(0),
            };
            let engine = engine(
                &model,
                TurnRobustness {
                    auth_refresh: Some(&refresh),
                    ..Default::default()
                },
            );
            let result = engine.run_turn("t1", "go").await;
            assert!(matches!(result.state, TurnState::Failed(_)));
            assert_eq!(model.requests.lock().unwrap().len(), 1, "zero retries");
        }
    }

    #[tokio::test]
    async fn one_shot_state_resets_per_turn() {
        let model = ScriptedModel::new(vec![
            Err(auth_401()),
            Ok(text_response("turn one")),
            Err(auth_401()),
            Ok(text_response("turn two")),
        ]);
        let refresh = MockRefresh {
            outcome: RefreshOutcome::Refreshed,
            calls: Mutex::new(0),
        };
        let engine = engine(
            &model,
            TurnRobustness {
                auth_refresh: Some(&refresh),
                ..Default::default()
            },
        );
        assert_eq!(engine.run_turn("t1", "go").await.state, TurnState::Complete);
        assert_eq!(
            engine.run_turn("t2", "again").await.state,
            TurnState::Complete
        );
        assert_eq!(
            *refresh.calls.lock().unwrap(),
            2,
            "each turn gets its own one-shot"
        );
    }

    // ── Schema re-ask ────────────────────────────────────────────────────

    fn schema_request() -> Option<serde_json::Value> {
        Some(serde_json::json!({"type": "object"}))
    }

    #[tokio::test]
    async fn schema_failure_reasks_once_journaled_with_literal_feedback() {
        let feedback = "Your previous response did not satisfy the requested JSON schema (bad). Respond again with output that validates against the schema exactly.";
        let model = ScriptedModel::new(vec![
            Err(ModelError::OutputSchema(feedback.into())),
            Ok(text_response("{\"ok\": true}")),
        ]);
        let engine = engine(
            &model,
            TurnRobustness {
                output_schema: schema_request(),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "give me json").await;
        assert_eq!(result.state, TurnState::Complete);
        // Journaled with the LITERAL feedback text (byte-fidelity never
        // hinges on a template).
        let reasks: Vec<&str> = result
            .ops
            .iter()
            .filter_map(|e| match &e.op {
                Op::SchemaReask { feedback, .. } => Some(feedback.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(reasks, vec![feedback]);
        // The model SAW the feedback as a user message on the retry.
        let second = &model.requests.lock().unwrap()[1];
        assert!(
            matches!(second.messages.last().map(|m| &m.content[0]), Some(nano_model::types::ContentBlock::Text { text }) if text == feedback)
        );
        // Budget accounting: the re-ask is a new sampling step.
        assert_eq!(result.steps, 2);
    }

    #[tokio::test]
    async fn second_schema_failure_is_terminal() {
        let model = ScriptedModel::new(vec![
            Err(ModelError::OutputSchema("first".into())),
            Err(ModelError::OutputSchema("second".into())),
        ]);
        let engine = engine(
            &model,
            TurnRobustness {
                output_schema: schema_request(),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "go").await;
        assert!(matches!(result.state, TurnState::Failed(_)));
        assert_eq!(model.requests.lock().unwrap().len(), 2, "one re-ask only");
    }

    // ── Observation forwarding / coalescing ──────────────────────────────

    #[tokio::test]
    async fn reconnect_observations_forward_immediately_rate_limits_coalesce() {
        let received: Mutex<Vec<ModelObservation>> = Mutex::new(Vec::new());
        let sink = |obs: ModelObservation| received.lock().unwrap().push(obs);
        let model = ObservingModel {
            observations: vec![
                ModelObservation::Reconnecting {
                    attempt: 1,
                    next_delay_ms: 5000,
                    deadline_remaining_ms: 295_000,
                },
                ModelObservation::RateLimit(snapshot(1000)),
                ModelObservation::RateLimit(snapshot(2000)),
            ],
            inner: ScriptedModel::new(vec![Ok(text_response("done"))]),
        };
        let engine = engine(
            &model,
            TurnRobustness {
                observer: Some(&sink),
                ..Default::default()
            },
        );
        let result = engine.run_turn("t1", "go").await;
        assert_eq!(result.state, TurnState::Complete);
        let received = received.lock().unwrap();
        // The reconnect observation forwarded as-is; the two rate-limit
        // snapshots coalesced latest-wins into ONE forward.
        assert_eq!(received.len(), 2, "{received:?}");
        assert!(matches!(
            received[0],
            ModelObservation::Reconnecting { attempt: 1, .. }
        ));
        assert!(
            matches!(&received[1], ModelObservation::RateLimit(s) if s.captured_at_ms == 2000),
            "latest-wins: {received:?}"
        );
    }
}
