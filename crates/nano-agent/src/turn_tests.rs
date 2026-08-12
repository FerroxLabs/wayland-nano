//! Turn engine tests with a scripted mock driver (no live API).

#[cfg(test)]
mod tests {
    use crate::loop_protection::{ProgressSignals, TurnBudget};
    use crate::turn::{ModelDriver, ToolExecutor, TurnEngine, TurnState};
    use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
    use std::sync::Mutex;

    #[derive(Debug)]
    struct ScriptedModel {
        responses: Mutex<Vec<ModelResponse>>,
        pub requests: Mutex<Vec<ModelRequest>>,
    }

    impl ScriptedModel {
        fn new(responses: Vec<ModelResponse>) -> Self {
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
            self.responses.lock().unwrap().remove(0).pipe_ok()
        }
    }

    trait PipeOk {
        fn pipe_ok(self) -> Result<ModelResponse, ModelError>;
    }
    impl PipeOk for ModelResponse {
        fn pipe_ok(self) -> Result<ModelResponse, ModelError> {
            Ok(self)
        }
    }

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

    fn tool_response(call: ToolCall) -> ModelResponse {
        ModelResponse {
            events: vec![
                ModelEvent::ToolCallComplete(call),
                ModelEvent::Done {
                    stop_reason: "tool_calls".into(),
                },
            ],
            usage: Usage::default(),
            stop_reason: "tool_calls".into(),
        }
    }

    #[derive(Debug)]
    struct RecordingTools {
        pub calls: Mutex<Vec<ToolCall>>,
        pub progress: ProgressSignals,
    }

    #[async_trait::async_trait]
    impl ToolExecutor for RecordingTools {
        async fn execute(&self, call: &ToolCall) -> crate::turn::ToolOutcome {
            self.calls.lock().unwrap().push(call.clone());
            crate::turn::ToolOutcome {
                ok: true,
                output: format!("ran {}", call.name),
                progress: self.progress.clone(),
            }
        }
    }

    #[tokio::test]
    async fn full_turn_act_observe_verify_complete() {
        let model = ScriptedModel::new(vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_edit".into(),
                arguments: serde_json::json!({"path": "main.rs"}),
            }),
            text_response("fixed the build"),
        ]);
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals {
                files_changed: true,
                ..Default::default()
            },
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
        };

        let result = engine.run_turn("t1", "fix the build").await;

        assert_eq!(result.state, TurnState::Complete);
        // The plan: every state is testable — assert the full path.
        let labels: Vec<String> = result.history.iter().map(|s| s.label()).collect();
        assert_eq!(
            labels,
            vec![
                "RECEIVE",
                "UNDERSTAND",
                "PLAN",
                "ACT",
                "OBSERVE",
                "UNDERSTAND",
                "PLAN",
                "VERIFY",
                "COMPLETE",
            ]
        );
        assert_eq!(result.final_text, "fixed the build");
        assert_eq!(tools.calls.lock().unwrap().len(), 1);
        let op_types: Vec<String> = result
            .ops
            .iter()
            .map(|e| {
                format!("{:?}", e.op)
                    .split(' ')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        assert!(op_types.iter().any(|t| t.contains("TurnBegin")));
        assert!(op_types.iter().any(|t| t.contains("ToolCall")));
        assert!(op_types.iter().any(|t| t.contains("ToolResult")));
        assert!(op_types.iter().any(|t| t.contains("TurnEnd")));
    }

    #[tokio::test]
    async fn repeat_breaker_force_stops_identical_calls() {
        let same_call = ToolCall {
            id: "c1".into(),
            name: "fs_read".into(),
            arguments: serde_json::json!({"path": "x"}),
        };
        let model = ScriptedModel::new(vec![
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call.clone()),
            tool_response(same_call),
            text_response("never reached"),
        ]);
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals::default(),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
        };

        let result = engine.run_turn("t2", "loop forever").await;

        assert!(
            matches!(result.state, TurnState::Stopped(_)),
            "{:?}",
            result.state
        );
    }

    #[tokio::test]
    async fn streaming_sink_sees_each_op_as_it_lands() {
        use std::sync::Arc;

        // The second model call asserts the sink already saw step 1's
        // ToolCall/ToolResult — impossible under after-the-fact replay.
        #[derive(Debug)]
        struct SinkAssertingModel {
            seen: Arc<Mutex<Vec<String>>>,
            calls: Mutex<u32>,
        }
        #[async_trait::async_trait]
        impl ModelDriver for SinkAssertingModel {
            async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                if *calls == 1 {
                    Ok(tool_response(ToolCall {
                        id: "c1".into(),
                        name: "fs_read".into(),
                        arguments: serde_json::json!({"path": "x"}),
                    }))
                } else {
                    let seen = self.seen.lock().unwrap();
                    assert!(
                        seen.iter().any(|t| t.contains("ToolCall")),
                        "sink must see step-1 ToolCall before step 2 starts: {seen:?}"
                    );
                    assert!(
                        seen.iter().any(|t| t.contains("ToolResult")),
                        "sink must see step-1 ToolResult before step 2 starts: {seen:?}"
                    );
                    drop(seen);
                    Ok(text_response("done"))
                }
            }
        }

        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let model = SinkAssertingModel {
            seen: seen.clone(),
            calls: Mutex::new(0),
        };
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals::default(),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
        };

        let sink_seen = seen.clone();
        let mut sink = move |envelope: &nano_session::op::OpEnvelope| {
            let label = format!("{:?}", envelope.op)
                .split([' ', '('])
                .next()
                .unwrap_or("")
                .to_string();
            sink_seen.lock().unwrap().push(label);
            true
        };
        let result = engine
            .run_turn_streaming("t4", "read then reply", None, &mut sink)
            .await;

        assert_eq!(result.state, TurnState::Complete);
        let seen = seen.lock().unwrap();
        // Every journaled op also went through the sink, in order.
        assert_eq!(seen.len(), result.ops.len());
        assert!(seen.iter().any(|t| t.contains("TurnBegin")));
        assert!(seen.iter().any(|t| t.contains("TurnEnd")));
        // Sanity: labels really came from ops, not an empty stream.
        assert!(seen.iter().any(|t| t.contains("ToolCall")));
    }

    #[tokio::test]
    async fn no_progress_stops_turn() {
        let call = ToolCall {
            id: "c1".into(),
            name: "fs_read".into(),
            arguments: serde_json::json!({"path": "x"}),
        };
        let model = ScriptedModel::new(vec![
            tool_response(call.clone()),
            tool_response(ToolCall {
                id: "c2".into(),
                ..call.clone()
            }),
            tool_response(ToolCall {
                id: "c3".into(),
                ..call.clone()
            }),
            tool_response(ToolCall {
                id: "c4".into(),
                ..call.clone()
            }),
            tool_response(ToolCall {
                id: "c5".into(),
                ..call.clone()
            }),
            tool_response(ToolCall {
                id: "c6".into(),
                ..call
            }),
            text_response("never reached"),
        ]);
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals::default(), // zero progress every step
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
        };

        let result = engine.run_turn("t3", "make no progress").await;

        assert!(
            matches!(result.state, TurnState::Stopped(_)),
            "{:?}",
            result.state
        );
        assert!(
            matches!(result.state, TurnState::Stopped(ref r) if r.contains("no observable progress")),
            "{:?}",
            result.state
        );
    }

    #[tokio::test]
    async fn gate_denial_reason_reaches_the_model_context() {
        use crate::turn::{ApprovalDecision, ApprovalGate};

        /// C2: a gate that categorically denies (the read_only mode arm)
        /// explains WHY, and the engine must put that reason in the denial
        /// tool-result the model sees — otherwise it retries variants in a
        /// loop.
        #[derive(Debug)]
        struct ModeDenyGate;
        impl ApprovalGate for ModeDenyGate {
            fn approve(&self, _call: &ToolCall) -> ApprovalDecision {
                ApprovalDecision::Deny
            }
            fn denial_reason(&self) -> Option<&'static str> {
                Some("session is in read_only mode")
            }
        }

        let model = ScriptedModel::new(vec![
            tool_response(ToolCall {
                id: "c1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "main.rs", "content": "x"}),
            }),
            text_response("cannot write in read_only mode"),
        ]);
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals::default(),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: Some(&ModeDenyGate),
            compaction: None,
        };

        let result = engine.run_turn("t5", "write the file").await;

        assert_eq!(result.state, TurnState::Complete);
        // The denied call never executed...
        assert!(tools.calls.lock().unwrap().is_empty());
        // ...and the NEXT model request carried the reasoned denial text.
        let requests = model.requests.lock().unwrap();
        let carried = requests[1].messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    nano_model::types::ContentBlock::ToolResult { content, .. }
                    if content.contains("denied by approval gate: session is in read_only mode")
                )
            })
        });
        assert!(
            carried,
            "denial must name the mode: {:?}",
            requests[1].messages
        );
    }
}
