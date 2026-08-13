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
                error_kind: None,
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
            robustness: Default::default(),
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
    async fn p2b_acceptance_seam_rejects_view_image_without_live_provenance() {
        let model = ScriptedModel::new(vec![
            tool_response(ToolCall {
                id: "img1".into(),
                name: "view_image".into(),
                arguments: serde_json::json!({"path": "fixture.png"}),
            }),
            text_response("continued after typed refusal"),
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
            tool_definitions: crate::wiring::v1_tool_definitions(false, true),
            approval: None,
            compaction: None,
            robustness: Default::default(),
        };
        let result = engine.run_turn("p2b-seam", "read it").await;
        assert_eq!(result.state, TurnState::Complete);
        assert!(result.ops.iter().any(|envelope| matches!(
            &envelope.op,
            nano_session::op::Op::ToolResult {
                call_id,
                ok: false,
                error_kind: Some(nano_session::NanoErrorKind::ImageInvalid),
                image_refs,
                ..
            } if call_id == "img1" && image_refs.is_empty()
        )));
        let second = &model.requests.lock().unwrap()[1];
        assert!(second.messages.iter().flat_map(|message| &message.content).any(|block| {
            matches!(
                block,
                nano_model::types::ContentBlock::ToolResult { content, images, .. }
                    if content.contains("missing or invalid live provenance") && images.is_empty()
            )
        }));
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
            robustness: Default::default(),
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
            robustness: Default::default(),
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
            robustness: Default::default(),
        };

        let result = engine.run_turn("t3", "make no progress").await;

        assert!(
            matches!(result.state, TurnState::Stopped(_)),
            "{:?}",
            result.state
        );
        assert!(
            matches!(result.state, TurnState::Stopped(ref r) if r.detail.contains("no observable progress")),
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
            robustness: Default::default(),
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

    /// C10 §5: the default `ApprovalGate::ask` is `Unavailable` — a gate
    /// that never learned questions (ApproveAll, DenyAll child gates, test
    /// doubles) fails closed to the typed unavailability, never a block.
    #[test]
    fn default_ask_is_unavailable() {
        use crate::turn::{ApprovalGate, AskOutcome};
        let gate = crate::turn::ApproveAll;
        let call = nano_model::types::ToolCall {
            id: "c1".into(),
            name: "ask_user".into(),
            arguments: serde_json::json!({"question": "q", "options": [{"label": "a"}, {"label": "b"}]}),
        };
        assert_eq!(gate.ask(&call), AskOutcome::Unavailable);
    }

    // ── P2a §5.2.1/§6.2: producer plumbing + the pre-dispatch gate ──────

    /// §12 producer plumbing regression: a legacy &str entry journals
    /// TurnBegin with projection == input AND the single-Text manifest, and
    /// the dispatched first user message is byte-identical to pre-P2a.
    #[tokio::test]
    async fn p2a_legacy_text_entry_journals_projection_and_manifest() {
        use nano_session::op::{InputBlock, Op};
        let model = ScriptedModel::new(vec![text_response("done")]);
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
            robustness: Default::default(),
        };
        let result = engine.run_turn("t1", "fix the build").await;
        assert!(matches!(result.state, TurnState::Complete));
        let Op::TurnBegin {
            input,
            input_blocks,
            ..
        } = &result.ops[0].op
        else {
            panic!("first op must be TurnBegin")
        };
        assert_eq!(input, "fix the build");
        assert_eq!(
            *input_blocks,
            vec![InputBlock::Text {
                text: "fix the build".into()
            }]
        );
        let requests = model.requests.lock().unwrap();
        assert_eq!(
            requests[0].messages.last().unwrap().content,
            vec![nano_model::types::ContentBlock::Text {
                text: "fix the build".into()
            }]
        );
    }

    /// §6.2 rung 3: an image-bearing turn against a model that is NOT
    /// vision-proven (the vendored catalog blesses nothing until §13 leg 6)
    /// is WHOLE-REQUEST rejected with typed ModelLacksVision — zero egress
    /// (the scripted driver records no request), no strip-and-transmit. The
    /// journal still carries the audit pair: TurnBegin (projection +
    /// digests-only manifest) then TurnEnd(Failed).
    #[tokio::test]
    async fn p2a_rung3_rejects_image_turn_on_unproven_model_with_zero_egress() {
        use crate::turn_input::{TurnBlock, TurnInput};
        use nano_session::NanoErrorKind;
        use nano_session::op::{ImageRef, InputBlock, Op, OpEnvelope};
        let model = ScriptedModel::new(vec![text_response("never served")]);
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals::default(),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            // An EXCLUDED family (explicit-false posture, §6.3).
            model_name: "flux-pinned-codestral".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: Default::default(),
        };
        let digest = "cd".repeat(32);
        let input = TurnInput {
            blocks: vec![TurnBlock::Image {
                reference: ImageRef {
                    digest: digest.clone(),
                    mime: "image/png".into(),
                    bytes: 5,
                    width: 1,
                    height: 1,
                    normalized_from: None,
                    placeholder: "[Image #1: /tmp/x.png]".into(),
                },
                data: "aGVsbG8".into(),
            }],
        };
        let mut sink = |_: &OpEnvelope| true;
        let result = engine
            .run_turn_streaming_with_context_blocks("t1", input, vec![], None, &mut sink, false)
            .await;
        let TurnState::Failed(err) = &result.state else {
            panic!("expected typed failure, got {:?}", result.state)
        };
        assert_eq!(err.kind, NanoErrorKind::ModelLacksVision);
        assert!(
            model.requests.lock().unwrap().is_empty(),
            "zero egress: no request ever built"
        );
        let Op::TurnBegin {
            input,
            input_blocks,
            ..
        } = &result.ops[0].op
        else {
            panic!("first op must be TurnBegin")
        };
        assert_eq!(input, "[Image #1: /tmp/x.png]");
        assert!(
            matches!(&input_blocks[0], InputBlock::ImageRef(r) if r.digest == digest && r.mime == "image/png")
        );
        // Digest-only: the journaled manifest never carries the live data.
        let journaled = serde_json::to_string(&result.ops[0].op).expect("serialize");
        assert!(!journaled.contains("aGVsbG8"));
        assert!(matches!(
            result.ops.last().unwrap().op,
            Op::TurnEnd {
                outcome: nano_session::op::TurnOutcome::Failed,
                ..
            }
        ));
    }

    // ── P2b §3.6/§7: end-to-end injection-clamp legs ────────────────────────
    //
    // The screenshot-of-instructions threat end-to-end: a real view_image
    // result lands in history (fresh / post-compaction / post-kill-resume)
    // and a subsequent protected trust mutation hits the clamp — the
    // unpromptable gates DENY with the named reason; a prompt-capable gate
    // is consulted with the influence cell SET (the ACP prompt round-trip
    // itself is pinned by p2a_image_influenced_clamp_forces_human_approval).

    #[derive(Debug)]
    struct ApproveImageReads;
    impl nano_tools::image::ImageReadApprover for ApproveImageReads {
        fn request(&self, _canonical: &std::path::Path) -> nano_tools::image::ImageReadApproval {
            nano_tools::image::ImageReadApproval::Approved
        }
    }

    /// A recording gate: prompt-capable gates get the call (and observe the
    /// cell); the clamp short-circuits unpromptable gates before approve().
    #[derive(Debug)]
    struct ClampGate {
        can_prompt: bool,
        cell: std::sync::Arc<std::sync::atomic::AtomicBool>,
        seen: Mutex<Vec<(String, bool)>>,
    }

    impl crate::turn::ApprovalGate for ClampGate {
        fn approve(&self, call: &ToolCall) -> crate::turn::ApprovalDecision {
            self.seen.lock().unwrap().push((
                call.name.clone(),
                self.cell.load(std::sync::atomic::Ordering::SeqCst),
            ));
            crate::turn::ApprovalDecision::Approve
        }
        fn can_prompt_image_clamp(&self) -> bool {
            self.can_prompt
        }
    }

    /// A scripted model that can also serve typed errors (the reactive
    /// ContextOverflow compaction trigger).
    #[derive(Debug)]
    struct FallibleModel {
        responses: Mutex<Vec<Result<ModelResponse, ModelError>>>,
        pub requests: Mutex<Vec<ModelRequest>>,
    }

    #[async_trait::async_trait]
    impl ModelDriver for FallibleModel {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request.clone());
            self.responses.lock().unwrap().remove(0)
        }
    }

    /// A workspace with a real decodable PNG plus a RealToolExecutor with
    /// view_image wired against a temp attachment store.
    fn view_image_fixture() -> (
        tempfile::TempDir,
        crate::wiring::RealToolExecutor,
        std::path::PathBuf,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../nano-tools/fixtures/images/valid.png");
        std::fs::copy(fixture, ws.join("shot.png")).expect("copy fixture");
        let policy = nano_core::permissions::PermissionProfile::workspace_write()
            .file_system_sandbox_policy();
        let fs = nano_tools::fs::FsTools::new(policy.clone(), &ws);
        let shell = nano_tools::shell::ShellTool::new(&home, &ws);
        let store =
            nano_session::attachment_store::AttachmentStore::open(&home).expect("attachment store");
        let view = nano_tools::image::ViewImageTool::new(
            policy,
            &ws,
            std::sync::Arc::new(ApproveImageReads),
        );
        let executor =
            crate::wiring::RealToolExecutor::new(fs, shell, &ws).with_view_image(view, store);
        (tmp, executor, ws)
    }

    /// §7 leg 1: a FRESH, non-evicted view_image result forces the clamp —
    /// the unpromptable gate's protected mutation is DENIED with the named
    /// reason, the journaled result carries image_refs + a real sha256
    /// output digest, and the walker observed the live images (cell set).
    #[tokio::test]
    async fn p2b_fresh_result_image_forces_clamp_in_full_auto() {
        let (_tmp, tools, ws) = view_image_fixture();
        let model = ScriptedModel::new(vec![
            tool_response(ToolCall {
                id: "img1".into(),
                name: "view_image".into(),
                arguments: serde_json::json!({"path": "shot.png"}),
            }),
            tool_response(ToolCall {
                id: "w1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": ws.join("AGENTS.md"), "content": "x"}),
            }),
            text_response("done"),
        ]);
        let cell = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = ClampGate {
            can_prompt: false,
            cell: cell.clone(),
            seen: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: crate::wiring::v1_tool_definitions(false, true),
            approval: Some(&gate),
            compaction: None,
            robustness: crate::turn::TurnRobustness {
                image_influence: Some(cell.clone()),
                ..Default::default()
            },
        };
        let result = engine.run_turn("p2b-clamp", "look then write").await;
        assert_eq!(result.state, TurnState::Complete);
        // The image result landed journaled: image_refs + a REAL sha256
        // (never the len: pseudo-digest).
        assert!(result.ops.iter().any(|envelope| matches!(
            &envelope.op,
            nano_session::op::Op::ToolResult {
                call_id, ok: true, output_digest, image_refs, ..
            } if call_id == "img1"
                && !image_refs.is_empty()
                && output_digest.len() == 64
                && output_digest.chars().all(|c| c.is_ascii_hexdigit())
        )));
        // The walker's input: the SECOND request carried the live images.
        let requests = model.requests.lock().unwrap();
        let second = &requests[1];
        assert!(second.messages.iter().flat_map(|m| &m.content).any(|block| {
            matches!(
                block,
                nano_model::types::ContentBlock::ToolResult { images, .. } if !images.is_empty()
            )
        }));
        assert!(
            cell.load(std::sync::atomic::Ordering::SeqCst),
            "the walker→gate link wrote the session cell"
        );
        // The clamp short-circuited the protected call BEFORE the
        // unpromptable gate (the gate saw only the view_image call itself).
        let seen = gate.seen.lock().unwrap();
        assert!(
            !seen.iter().any(|(name, _)| name == "fs_write"),
            "the protected mutation never reached the unpromptable gate: {seen:?}"
        );
        drop(seen);
        // The denial is journaled AND model-visible with the named reason.
        assert!(result.ops.iter().any(|envelope| matches!(
            &envelope.op,
            nano_session::op::Op::ToolResult {
                call_id,
                ok: false,
                error_kind: Some(nano_session::NanoErrorKind::ApprovalDenied),
                ..
            } if call_id == "w1"
        )));
        let third = &requests[2];
        assert!(third.messages.iter().flat_map(|m| &m.content).any(|block| {
            matches!(
                block,
                nano_model::types::ContentBlock::ToolResult { content, .. }
                    if content.contains(
                        "image-influenced protected mutation requires interactive approval"
                    )
            )
        }));
        drop(requests);
    }

    /// §7 leg 1b: with a PROMPT-CAPABLE gate the protected call is NOT
    /// pre-denied — the gate is consulted with the cell SET (the gate then
    /// forces the human prompt; the ACP forcing is pinned separately).
    #[tokio::test]
    async fn p2b_prompt_capable_gate_sees_protected_call_with_cell_set() {
        let (_tmp, tools, ws) = view_image_fixture();
        let model = ScriptedModel::new(vec![
            tool_response(ToolCall {
                id: "img1".into(),
                name: "view_image".into(),
                arguments: serde_json::json!({"path": "shot.png"}),
            }),
            tool_response(ToolCall {
                id: "w1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": ws.join("AGENTS.md"), "content": "x"}),
            }),
            text_response("done"),
        ]);
        let cell = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = ClampGate {
            can_prompt: true,
            cell: cell.clone(),
            seen: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: crate::wiring::v1_tool_definitions(false, true),
            approval: Some(&gate),
            compaction: None,
            robustness: crate::turn::TurnRobustness {
                image_influence: Some(cell.clone()),
                ..Default::default()
            },
        };
        let result = engine.run_turn("p2b-clamp-prompt", "look then write").await;
        assert_eq!(result.state, TurnState::Complete);
        let seen = gate.seen.lock().unwrap();
        assert!(
            seen.iter()
                .any(|(name, influenced)| name == "fs_write" && *influenced),
            "the gate observed the protected call with the influence cell set: {seen:?}"
        );
    }

    /// §7 leg 3 (post-kill-resume, manifest-presence path), pinned at the
    /// posture the honesty rule forces pre-proving: with NO vision-proven
    /// leaf in the vendored catalog, an image-influenced resumed context is
    /// rejected WHOLE at the rung-3 pre-dispatch gate — typed
    /// ModelLacksVision, ZERO model calls (zero egress). The
    /// manifest-presence → flag fold is pinned by
    /// p2b_image_influenced_fold_counts_result_image_manifests and the flag →
    /// gate clamp by p2a_image_influenced_clamp_forces_human_approval; the
    /// composed engine leg lands when the §8 live-proving flips a leaf.
    #[tokio::test]
    async fn p2b_post_kill_resume_image_influenced_is_fail_closed_zero_egress() {
        let model = ScriptedModel::new(vec![
            tool_response(ToolCall {
                id: "w1".into(),
                name: "fs_write".into(),
                arguments: serde_json::json!({"path": "AGENTS.md", "content": "x"}),
            }),
            text_response("done"),
        ]);
        let tools = RecordingTools {
            calls: Mutex::new(Vec::new()),
            progress: ProgressSignals::default(),
        };
        let cell = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = ClampGate {
            can_prompt: false,
            cell: cell.clone(),
            seen: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: Some(&gate),
            compaction: None,
            robustness: crate::turn::TurnRobustness {
                image_influence: Some(cell.clone()),
                ..Default::default()
            },
        };
        let mut sink = |_: &nano_session::op::OpEnvelope| true;
        let result = engine
            .run_turn_streaming_with_context_blocks(
                "p2b-resume",
                crate::turn_input::TurnInput::text("resume and write"),
                vec![],
                None,
                &mut sink,
                true, // the resume fold's sticky-OR output: an image was here
            )
            .await;
        assert!(
            matches!(
                &result.state,
                TurnState::Failed(err) if err.kind == nano_session::NanoErrorKind::ModelLacksVision
            ),
            "image-influenced resume on an unproven leaf fails closed: {:?}",
            result.state
        );
        assert!(
            model.requests.lock().unwrap().is_empty(),
            "zero model calls — the request was never transmitted"
        );
    }

    /// §7 leg 2 (post-compaction): the result image is evicted by a reactive
    /// compaction — the summarizer never sees the pixels, the journaled
    /// CompactionComplete carries image_influenced=true, the cell stays SET
    /// (sticky fetch_or), and a later protected mutation is STILL clamped.
    #[tokio::test]
    async fn p2b_post_compaction_protected_mutation_stays_clamped() {
        let (_tmp, tools, ws) = view_image_fixture();
        let model = FallibleModel {
            responses: Mutex::new(vec![
                Ok(tool_response(ToolCall {
                    id: "img1".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({"path": "shot.png"}),
                })),
                Err(ModelError::ContextOverflow("too big".into())),
                Ok(text_response("compact summary")),
                Ok(tool_response(ToolCall {
                    id: "w1".into(),
                    name: "fs_write".into(),
                    arguments: serde_json::json!({"path": ws.join("AGENTS.md"), "content": "x"}),
                })),
                Ok(text_response("done")),
            ]),
            requests: Mutex::new(Vec::new()),
        };
        let cell = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = ClampGate {
            can_prompt: false,
            cell: cell.clone(),
            seen: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: crate::wiring::v1_tool_definitions(false, true),
            approval: Some(&gate),
            compaction: Some(crate::compact::CompactionConfig {
                context_window: 1_000_000,
                auto_compact_limit: 900_000,
            }),
            robustness: crate::turn::TurnRobustness {
                image_influence: Some(cell.clone()),
                ..Default::default()
            },
        };
        let result = engine.run_turn("p2b-compact", "look then write").await;
        assert_eq!(result.state, TurnState::Complete);
        // The eviction provenance is journaled.
        assert!(result.ops.iter().any(|envelope| matches!(
            &envelope.op,
            nano_session::op::Op::CompactionComplete {
                image_influenced: true,
                ..
            }
        )));
        // The summarizer never saw the result pixels; the post-compaction
        // retry carried the placeholder, not pixels.
        let requests = model.requests.lock().unwrap();
        let summarize = &requests[2];
        assert!(!summarize.messages.iter().flat_map(|m| &m.content).any(|block| {
            matches!(
                block,
                nano_model::types::ContentBlock::ToolResult { images, .. } if !images.is_empty()
            )
        }));
        let retry = &requests[3];
        assert!(
            !retry.messages.iter().flat_map(|m| &m.content).any(|block| {
                matches!(
                    block,
                    nano_model::types::ContentBlock::ToolResult { images, .. } if !images.is_empty()
                )
            })
        );
        // The cell stayed set across compaction and the clamp still fired.
        assert!(cell.load(std::sync::atomic::Ordering::SeqCst));
        assert!(result.ops.iter().any(|envelope| matches!(
            &envelope.op,
            nano_session::op::Op::ToolResult {
                call_id,
                ok: false,
                error_kind: Some(nano_session::NanoErrorKind::ApprovalDenied),
                ..
            } if call_id == "w1"
        )));
    }
}
