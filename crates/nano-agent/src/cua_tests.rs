//! S9 CUA seam tests (design S9-BROWSER-CUA-DESIGN.md §2–§5, §7.1): the
//! turn engine's digest-only CuaAction/CuaResult journaling, the gate
//! interplay, the cancel race, panic containment, and the §4.2 resume rule —
//! all headless over `nano_cua::mock::MockBackend` (no live desktop).

#[cfg(test)]
mod tests {
    use crate::cua::{CuaSession, cua_registration};
    use crate::loop_protection::TurnBudget;
    use crate::turn::{
        ApprovalDecision, ApprovalGate, ModelDriver, ToolExecutor, TurnEngine, TurnRobustness,
        TurnState,
    };
    use nano_cua::mock::{MockBackend, MockBehavior};
    use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
    use nano_session::NanoErrorKind;
    use nano_session::op::{CuaOutcome, Op};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    // ── fixtures ─────────────────────────────────────────────────────────

    #[derive(Debug)]
    struct ScriptModel {
        responses: Mutex<VecDeque<ModelResponse>>,
        requests: Mutex<Vec<ModelRequest>>,
    }

    impl ScriptModel {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelDriver for ScriptModel {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted response"))
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
            model: None,
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
            model: None,
        }
    }

    fn cua_call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments,
        }
    }

    /// CUA calls are serviced by the engine's bridge — never by the tool
    /// executor. A delegation here is a wiring bug and fails loudly.
    #[derive(Debug)]
    struct NoTools;

    #[async_trait::async_trait]
    impl ToolExecutor for NoTools {
        async fn execute(&self, call: &ToolCall) -> crate::turn::ToolOutcome {
            panic!(
                "{} reached the tool executor: CUA calls ride the bridge",
                call.name
            );
        }
    }

    /// Records every approve() call — the always-prompt proof is that the
    /// gate was CONSULTED per CUA op (the per-mode prompt behavior itself is
    /// pinned at the ACP gate in nano-cli).
    #[derive(Debug)]
    struct RecordingGate {
        decision: ApprovalDecision,
        calls: Mutex<Vec<String>>,
    }

    impl ApprovalGate for RecordingGate {
        fn approve(&self, call: &ToolCall) -> ApprovalDecision {
            self.calls.lock().unwrap().push(call.name.clone());
            self.decision
        }
    }

    fn cua_fixture(
        mock: &MockBackend,
        home: &std::path::Path,
        needs_rescreenshot: bool,
    ) -> CuaSession {
        let store = nano_session::AttachmentStore::open(home).expect("attachment store");
        CuaSession::new(
            Arc::new(mock.clone()),
            nano_cua::CuaPolicy::default(),
            store,
            needs_rescreenshot,
        )
    }

    fn cua_ops(result: &crate::turn::TurnResult) -> Vec<&Op> {
        result
            .ops
            .iter()
            .map(|envelope| &envelope.op)
            .filter(|op| matches!(op, Op::CuaAction { .. } | Op::CuaResult { .. }))
            .collect()
    }

    fn cua_results(result: &crate::turn::TurnResult) -> Vec<(CuaOutcome, Option<NanoErrorKind>)> {
        result
            .ops
            .iter()
            .filter_map(|envelope| match &envelope.op {
                Op::CuaResult {
                    outcome,
                    error_kind,
                    ..
                } => Some((*outcome, *error_kind)),
                _ => None,
            })
            .collect()
    }

    // ── posture matrix (§3) ──────────────────────────────────────────────

    #[test]
    fn posture_matrix_is_strictest_wins() {
        // Registration requires a wired session bridge, no plan posture, and
        // a CUA-registering mode.
        assert!(!cua_registration("read_only", false, true));
        assert!(cua_registration("default", false, true));
        assert!(cua_registration("full_auto", false, true));
        // Plan mode: not registered in ANY mode.
        for mode in ["read_only", "default", "full_auto"] {
            assert!(!cua_registration(mode, true, true), "{mode} + plan");
        }
        // No backend (no probe pass / unsupported platform): not registered.
        assert!(!cua_registration("full_auto", false, false));
        // Unknown mode id: fail closed.
        assert!(!cua_registration("yolo", false, true));
    }

    #[test]
    fn children_never_see_cua_tools() {
        for (search, vision) in [(false, false), (true, true)] {
            let children = crate::wiring::child_tool_definitions(search, vision);
            assert!(
                !children
                    .iter()
                    .any(|def| crate::cua::is_cua_tool(&def.name)),
                "child surface must never carry cua_* tools (§8 non-goal 8)"
            );
        }
        // The definitions themselves are the eight-op v1 surface.
        let defs = crate::cua::cua_tool_definitions();
        assert_eq!(defs.len(), 8);
        for def in &defs {
            assert!(crate::cua::is_cua_tool(&def.name), "{}", def.name);
        }
    }

    // ── engine path ──────────────────────────────────────────────────────

    /// A completed mutating op: CuaAction BEFORE CuaResult, pre/post shots in
    /// the attachment store, the gate consulted, no ToolCall/ToolResult ops,
    /// and the backend observed pre-shot → click → post-shot order.
    #[tokio::test(flavor = "current_thread")]
    async fn completed_click_journals_digest_only_pair() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new().with_frontmost("notepad.exe");
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call(
                "cua_left_click",
                serde_json::json!({"x": 10, "y": 20}),
            )),
            text_response("clicked"),
        ]);
        let tools = NoTools;
        let gate = RecordingGate {
            decision: ApprovalDecision::Approve,
            calls: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: Some(&gate),
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "click the button").await;
        assert_eq!(result.state, TurnState::Complete);

        // The gate was consulted for the CUA op (always-prompt: the
        // per-mode behavior is pinned at the ACP gate in nano-cli).
        assert_eq!(gate.calls.lock().unwrap().as_slice(), ["cua_left_click"]);

        // The journaled pair, in order, digest-only.
        let pair = cua_ops(&result);
        assert_eq!(pair.len(), 2);
        let Op::CuaAction {
            turn_id,
            call_id,
            op_kind,
            args_digest,
            frontmost_app,
            pre_shot,
        } = pair[0]
        else {
            panic!("first op is the action")
        };
        assert_eq!(turn_id, "t1");
        assert_eq!(call_id, "call-cua_left_click");
        assert_eq!(op_kind, "left_click");
        assert_eq!(frontmost_app.as_deref(), Some("notepad.exe"));
        use sha2::Digest as _;
        let expected_digest = format!(
            "{:x}",
            sha2::Sha256::digest(
                serde_json::to_vec(&serde_json::json!({"x": 10, "y": 20})).unwrap()
            )
        );
        assert_eq!(args_digest, &expected_digest);
        let (pre, post) = match pair[1] {
            Op::CuaResult {
                outcome: CuaOutcome::Completed,
                post_shot: Some(post),
                error_kind: None,
                ..
            } => (pre_shot.clone().expect("pre-shot journaled"), post.clone()),
            other => panic!("completed result with post-shot, got {other:?}"),
        };
        // Both shots rehydrate from the digest-keyed store.
        let store = nano_session::AttachmentStore::open(home.path()).unwrap();
        assert!(store.read_verified(&pre).is_ok());
        assert!(store.read_verified(&post).is_ok());

        // No generic ToolCall/ToolResult frames for the CUA call.
        assert!(
            !result
                .ops
                .iter()
                .any(|e| matches!(e.op, Op::ToolCall { .. } | Op::ToolResult { .. })),
            "CUA calls never journal raw args"
        );

        // Backend order: pre-shot, the click (approved frontmost passed),
        // post-shot.
        let dispatched = mock.dispatched();
        assert_eq!(dispatched.len(), 3);
        assert!(matches!(dispatched[0], nano_cua::CuaOp::Screenshot { .. }));
        assert!(matches!(
            dispatched[1],
            nano_cua::CuaOp::LeftClick { x: 10, y: 20, .. }
        ));
        assert!(matches!(dispatched[2], nano_cua::CuaOp::Screenshot { .. }));
    }

    /// §4.1 journal-first: the CuaAction append lands BEFORE the backend
    /// dispatch — proven live: the sink marks the append, the mock records
    /// what it observed at dispatch entry.
    #[tokio::test(flavor = "current_thread")]
    async fn cua_action_is_journaled_before_dispatch() {
        let home = tempfile::tempdir().unwrap();
        let journaled = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let mock = MockBackend::new().with_on_dispatch({
            let journaled = journaled.clone();
            let observed = observed.clone();
            Arc::new(move || {
                // The wait is non-mutating (no pre-shot): the ONLY dispatch
                // is the action itself, which must observe the append.
                observed.store(journaled.load(Ordering::SeqCst), Ordering::SeqCst);
            })
        });
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call("cua_wait", serde_json::json!({"duration_ms": 5}))),
            text_response("done"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let mut sink = |envelope: &nano_session::op::OpEnvelope| -> bool {
            if matches!(envelope.op, Op::CuaAction { .. }) {
                journaled.store(true, Ordering::SeqCst);
            }
            true
        };
        let result = engine
            .run_turn_streaming("t1", "wait", None, &mut sink)
            .await;
        assert_eq!(result.state, TurnState::Complete);
        assert!(
            observed.load(Ordering::SeqCst),
            "dispatch observed the journaled CuaAction"
        );
    }

    /// Digest-only invariant (§4.1): a cua_type call's raw text and a
    /// cua_left_click call's raw coordinates NEVER appear in any journaled
    /// op — only their sha256 digest does.
    #[tokio::test(flavor = "current_thread")]
    async fn journal_frames_never_carry_raw_text_or_coords() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new().with_frontmost("app.exe");
        let session = cua_fixture(&mock, home.path(), false);
        let secret = "hunter2-cua-payload";
        let model = ScriptModel::new(vec![
            tool_response(cua_call("cua_type", serde_json::json!({"text": secret}))),
            tool_response(cua_call(
                "cua_left_click",
                serde_json::json!({"x": 1234, "y": 5678}),
            )),
            text_response("done"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "type the thing").await;
        assert_eq!(result.state, TurnState::Complete);
        for envelope in &result.ops {
            let serialized = serde_json::to_string(envelope).unwrap();
            assert!(
                !serialized.contains(secret),
                "raw typed text must never journal: {serialized}"
            );
            assert!(
                !serialized.contains("1234") && !serialized.contains("5678"),
                "raw coordinates must never journal: {serialized}"
            );
        }
        // The digests ARE journaled (one action per call).
        assert_eq!(
            result
                .ops
                .iter()
                .filter(|e| matches!(e.op, Op::CuaAction { .. }))
                .count(),
            2
        );
    }

    /// A gate denial journals the pair as denied/approval_denied and never
    /// dispatches the action (the pre-shot evidence capture in prepare is
    /// the only backend contact).
    #[tokio::test(flavor = "current_thread")]
    async fn gate_denial_journals_denied_pair_without_dispatch() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new().with_frontmost("notepad.exe");
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call(
                "cua_left_click",
                serde_json::json!({"x": 1, "y": 2}),
            )),
            text_response("ok, not clicking"),
        ]);
        let tools = NoTools;
        let gate = RecordingGate {
            decision: ApprovalDecision::Deny,
            calls: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: Some(&gate),
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "click").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(
            cua_results(&result),
            vec![(CuaOutcome::Denied, Some(NanoErrorKind::ApprovalDenied))]
        );
        let dispatched = mock.dispatched();
        assert_eq!(dispatched.len(), 1, "only the pre-shot evidence capture");
        assert!(matches!(dispatched[0], nano_cua::CuaOp::Screenshot { .. }));
    }

    /// A failed CuaAction append is turn-fatal (§4.1, the ToolCall
    /// precedent): the action NEVER dispatches and the turn fails with
    /// journal_unavailable.
    #[tokio::test(flavor = "current_thread")]
    async fn failed_action_append_is_turn_fatal() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new();
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call("cua_wait", serde_json::json!({"duration_ms": 1}))),
            text_response("unreachable"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let mut sink = |envelope: &nano_session::op::OpEnvelope| -> bool {
            !matches!(envelope.op, Op::CuaAction { .. }) // the append "fails"
        };
        let result = engine
            .run_turn_streaming("t1", "wait", None, &mut sink)
            .await;
        let TurnState::Failed(err) = &result.state else {
            panic!("turn must fail, got {:?}", result.state)
        };
        assert_eq!(err.kind, NanoErrorKind::JournalUnavailable);
        assert!(
            mock.dispatched().is_empty(),
            "a failed action append never dispatches"
        );
    }

    /// §2.5 cancel race: a cancel firing mid-dispatch aborts the in-flight
    /// backend call and journals a typed cancelled result.
    #[tokio::test(flavor = "current_thread")]
    async fn cancel_races_dispatch_to_typed_cancelled() {
        let home = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mock = MockBackend::new().with_on_dispatch({
            let cancel = cancel.clone();
            Arc::new(move || cancel.store(true, Ordering::SeqCst))
        });
        mock.push_behavior(MockBehavior::Hang);
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![tool_response(cua_call(
            "cua_wait",
            serde_json::json!({"duration_ms": 60_000}),
        ))]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine
            .run_turn_cancellable("t1", "wait", Some(cancel.as_ref()))
            .await;
        // The cancelled op journaled its pair; the loop-top cancel then
        // stopped the turn (the F-17 semantics).
        assert_eq!(
            cua_results(&result),
            vec![(CuaOutcome::Cancelled, Some(NanoErrorKind::UserCancelled))]
        );
        assert!(matches!(
            result.state,
            TurnState::Stopped(ref info) if info.kind == NanoErrorKind::UserCancelled
        ));
    }

    /// §2.5 panic containment: a backend panic surfaces as a typed
    /// cua_backend tool error — never a process abort — and the turn
    /// continues (no retry: the next model step is a fresh decision).
    #[tokio::test(flavor = "current_thread")]
    async fn backend_panic_becomes_typed_error() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new();
        mock.push_behavior(MockBehavior::Panic);
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call("cua_wait", serde_json::json!({"duration_ms": 1}))),
            text_response("recovered"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "wait").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(
            cua_results(&result),
            vec![(CuaOutcome::Failed, Some(NanoErrorKind::CuaBackend))]
        );
        // The model SAW the typed failure (bounded text, "panicked" named).
        let requests = model.requests.lock().unwrap();
        let saw = requests[1]
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|block| match block {
                nano_model::types::ContentBlock::ToolResult { content, .. } => {
                    content.contains("panicked")
                }
                _ => false,
            });
        assert!(saw, "the typed panic error reached the model");
    }

    /// §5.1: the frontmost app changed between approval and dispatch —
    /// typed focus loss, the op NOT dispatched as approved.
    #[tokio::test(flavor = "current_thread")]
    async fn focus_loss_between_approval_and_dispatch_is_typed() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new().with_frontmost("a.exe");
        mock.set_frontmost(Some("a.exe".into()));
        let shifter = mock.clone();
        let mock = mock.with_on_dispatch(Arc::new(move || {
            shifter.set_frontmost(Some("b.exe".into()));
        }));
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call("cua_wait", serde_json::json!({"duration_ms": 1}))),
            text_response("focus moved"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "wait").await;
        assert_eq!(
            cua_results(&result),
            vec![(CuaOutcome::Failed, Some(NanoErrorKind::CuaFocusLost))]
        );
        // The action journaled the app the approval was issued against.
        let action = result
            .ops
            .iter()
            .find_map(|e| match &e.op {
                Op::CuaAction { frontmost_app, .. } => Some(frontmost_app.clone()),
                _ => None,
            })
            .expect("action journaled");
        assert_eq!(action.as_deref(), Some("a.exe"));
    }

    /// §2.3: a forbidden combo (the built-in secure-attention denylist) is
    /// policy-denied BEFORE the gate — journaled denied/cua_policy_denied,
    /// no dispatch, no approval prompt.
    #[tokio::test(flavor = "current_thread")]
    async fn forbidden_combo_is_policy_denied_pre_gate() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new().with_frontmost("any.exe");
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![
            tool_response(cua_call(
                "cua_key",
                serde_json::json!({"keys": "ctrl+alt+del"}),
            )),
            text_response("understood"),
        ]);
        let tools = NoTools;
        let gate = RecordingGate {
            decision: ApprovalDecision::Approve,
            calls: Mutex::new(Vec::new()),
        };
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: Some(&gate),
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "press it").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(
            cua_results(&result),
            vec![(CuaOutcome::Denied, Some(NanoErrorKind::CuaPolicyDenied))]
        );
        assert!(mock.dispatched().is_empty());
        assert!(
            gate.calls.lock().unwrap().is_empty(),
            "a policy reject never reaches the approval gate"
        );
    }

    /// §4.2 resume rule: with an ambiguous tail armed, every non-screenshot
    /// CUA op is policy-denied until ONE screenshot succeeds — then the
    /// surface re-arms.
    #[tokio::test(flavor = "current_thread")]
    async fn resumed_turn_must_screenshot_before_any_other_cua_op() {
        let home = tempfile::tempdir().unwrap();
        let mock = MockBackend::new().with_frontmost("app.exe");
        let session = cua_fixture(&mock, home.path(), true);
        let model = ScriptModel::new(vec![
            tool_response(cua_call(
                "cua_left_click",
                serde_json::json!({"x": 1, "y": 1}),
            )),
            tool_response(cua_call("cua_screenshot", serde_json::json!({}))),
            tool_response(cua_call(
                "cua_left_click",
                serde_json::json!({"x": 1, "y": 1}),
            )),
            text_response("recovered"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let result = engine.run_turn("t1", "continue").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(
            cua_results(&result),
            vec![
                (CuaOutcome::Denied, Some(NanoErrorKind::CuaPolicyDenied)),
                (CuaOutcome::Completed, None),
                (CuaOutcome::Completed, None),
            ]
        );
        // Dispatch order: the denied click never touched the backend; then
        // the mandatory screenshot; then the re-armed click with its
        // pre/post shots.
        let kinds: Vec<&'static str> = mock.dispatched().iter().map(|op| op.kind_tag()).collect();
        assert_eq!(
            kinds,
            vec!["screenshot", "screenshot", "left_click", "screenshot"]
        );
        // The denial told the model to screenshot first.
        let requests = model.requests.lock().unwrap();
        let told = requests[1]
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|block| match block {
                nano_model::types::ContentBlock::ToolResult { content, .. } => {
                    content.contains("cua_screenshot")
                }
                _ => false,
            });
        assert!(told, "the resume-rule denial names the mandatory op");
    }

    /// §4.2 kill proof, end to end: a kill BETWEEN the CuaAction append and
    /// the dispatch return (the turn future dropped mid-dispatch — the
    /// process-death analogue) leaves the ambiguous tail in the durable
    /// journal, and replay marks it interrupted.
    #[tokio::test(flavor = "current_thread")]
    async fn kill_mid_dispatch_leaves_an_interrupted_ambiguous_tail() {
        let home = tempfile::tempdir().unwrap();
        let journal_path = home.path().join("session.jsonl");
        let coordinator = nano_session::JournalCoordinator::open(&journal_path).unwrap();
        let dispatch_started = Arc::new(AtomicBool::new(false));
        let mock = MockBackend::new().with_on_dispatch({
            let started = dispatch_started.clone();
            Arc::new(move || started.store(true, Ordering::SeqCst))
        });
        mock.push_behavior(MockBehavior::Hang);
        let session = cua_fixture(&mock, home.path(), false);
        let model = ScriptModel::new(vec![tool_response(cua_call(
            "cua_wait",
            serde_json::json!({"duration_ms": 60_000}),
        ))]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness {
                cua: Some(&session),
                ..Default::default()
            },
        };
        let mut sink = |envelope: &nano_session::op::OpEnvelope| -> bool {
            coordinator.append(envelope).is_ok()
        };
        let turn = engine.run_turn_streaming("t1", "wait", None, &mut sink);
        tokio::pin!(turn);
        tokio::select! {
            _ = &mut turn => panic!("the hung dispatch must not complete"),
            _ = async {
                while !dispatch_started.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            } => {}
        }
        // The kill: never poll the turn again — the in-flight dispatch has no
        // chance to journal CuaResult or TurnEnd (the pinned future and its
        // sink borrow die at scope end; the journal is already durable).
        let _ = &turn;

        let report = nano_session::reader::read_journal(&journal_path).unwrap();
        let state = nano_session::SessionState::fold(&report.envelopes);
        assert!(state.turn_interrupted, "the stranded turn is interrupted");
        assert_eq!(state.interrupted_cua.len(), 1);
        assert_eq!(state.interrupted_cua[0].op_kind, "wait");
        assert_eq!(state.interrupted_cua[0].call_id, "call-cua_wait");
        // Exactly the ambiguous shape: an action, NO result.
        assert!(
            report
                .envelopes
                .iter()
                .any(|e| matches!(e.op, Op::CuaAction { .. }))
        );
        assert!(
            !report
                .envelopes
                .iter()
                .any(|e| matches!(e.op, Op::CuaResult { .. }))
        );
    }

    /// Fail-closed: a cua_* call reaching an engine with NO wired bridge
    /// (unregistered host; stale/hallucinated call) journals a failed pair
    /// and never dispatches.
    #[tokio::test(flavor = "current_thread")]
    async fn unwired_bridge_fails_closed_without_dispatch() {
        let model = ScriptModel::new(vec![
            tool_response(cua_call("cua_wait", serde_json::json!({"duration_ms": 1}))),
            text_response("ok"),
        ]);
        let tools = NoTools;
        let engine = TurnEngine {
            model: &model,
            tools: &tools,
            budget: TurnBudget::default(),
            model_name: "mock".into(),
            tool_definitions: vec![],
            approval: None,
            compaction: None,
            robustness: TurnRobustness::default(),
        };
        let result = engine.run_turn("t1", "wait").await;
        assert_eq!(result.state, TurnState::Complete);
        assert_eq!(
            cua_results(&result),
            vec![(
                CuaOutcome::Failed,
                Some(NanoErrorKind::CuaBackendUnavailable)
            )]
        );
    }
}
