//! C1 §9 tests: token accounting, trigger math, window config, loop guard,
//! the canonical builder, the repair pass, the commit protocol, the two
//! source-tear regressions, and the scripted-model integration paths.

use super::*;
use crate::loop_protection::{ProgressSignals, TurnBudget};
use crate::turn::{ModelDriver, ToolExecutor, TurnEngine, TurnState};
use nano_model::types::{ContentBlock, Message, ModelEvent, ModelResponse, ToolCall, Usage};
use nano_session::op::{CompactionCancelReason, Op};
use std::sync::Mutex;

// ── fakes ────────────────────────────────────────────────────────────────

/// A scripted model that recognizes the summarization call (last message is
/// the SUMMARIZATION_PROMPT) and serves it from a separate queue.
#[derive(Debug)]
struct FakeModel {
    responses: Mutex<Vec<Result<ModelResponse, ModelError>>>,
    summaries: Mutex<Vec<Result<ModelResponse, ModelError>>>,
    pub requests: Mutex<Vec<ModelRequest>>,
}

impl FakeModel {
    fn new(
        responses: Vec<Result<ModelResponse, ModelError>>,
        summaries: Vec<Result<ModelResponse, ModelError>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses),
            summaries: Mutex::new(summaries),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for FakeModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        let is_summary = request
            .messages
            .last()
            .and_then(|m| m.content.first())
            .is_some_and(
                |b| matches!(b, ContentBlock::Text { text } if text == SUMMARIZATION_PROMPT),
            );
        let queue = if is_summary {
            &self.summaries
        } else {
            &self.responses
        };
        queue.lock().unwrap().remove(0).map_err(|e| match e {
            ModelError::ContextOverflow(m) => ModelError::ContextOverflow(m),
            other => other,
        })
    }
}

fn text_response(text: &str, input_tokens: u64) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::Done {
                stop_reason: "stop".into(),
            },
        ],
        usage: Usage {
            input_tokens,
            ..Default::default()
        },
        stop_reason: "stop".into(),
    })
}

fn tool_response(calls: Vec<ToolCall>, input_tokens: u64) -> Result<ModelResponse, ModelError> {
    let mut events: Vec<ModelEvent> = calls
        .into_iter()
        .map(ModelEvent::ToolCallComplete)
        .collect();
    events.push(ModelEvent::Done {
        stop_reason: "tool_calls".into(),
    });
    Ok(ModelResponse {
        events,
        usage: Usage {
            input_tokens,
            ..Default::default()
        },
        stop_reason: "tool_calls".into(),
    })
}

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({"path": "x"}),
    }
}

#[derive(Debug)]
struct OkTools;

#[async_trait::async_trait]
impl ToolExecutor for OkTools {
    async fn execute(&self, call: &ToolCall) -> crate::turn::ToolOutcome {
        crate::turn::ToolOutcome {
            ok: true,
            output: format!("ran {}", call.name),
            progress: ProgressSignals {
                files_changed: true,
                ..Default::default()
            },
            error_kind: None,
        }
    }
}

fn engine<'a>(
    model: &'a dyn ModelDriver,
    tools: &'a dyn ToolExecutor,
    compaction: Option<CompactionConfig>,
) -> TurnEngine<'a> {
    TurnEngine {
        model,
        tools,
        budget: TurnBudget::default(),
        model_name: "mock".into(),
        tool_definitions: vec![],
        approval: None,
        compaction,
        robustness: Default::default(),
    }
}

/// Every ToolUse in every recorded request has a ToolResult and vice versa.
fn assert_no_torn_pairs(model: &FakeModel) {
    for request in model.requests.lock().unwrap().iter() {
        let uses: Vec<&str> = request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let results: Vec<&str> = request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            uses.len(),
            results.len(),
            "torn pair in request: uses {uses:?} vs results {results:?}"
        );
        for id in &uses {
            assert!(results.contains(id), "missing result for {id}");
        }
    }
}

fn op_kinds(result: &crate::turn::TurnResult) -> Vec<&'static str> {
    result
        .ops
        .iter()
        .map(|e| match &e.op {
            Op::TurnBegin { .. } => "TurnBegin",
            Op::ToolCall { .. } => "ToolCall",
            Op::ToolResult { .. } => "ToolResult",
            Op::AssistantText { .. } => "AssistantText",
            Op::TurnEnd { .. } => "TurnEnd",
            Op::CompactionBegin { .. } => "CompactionBegin",
            Op::CompactionComplete { .. } => "CompactionComplete",
            Op::CompactionCancel { .. } => "CompactionCancel",
            _ => "Other",
        })
        .collect()
}

// ── token accounting ─────────────────────────────────────────────────────

#[test]
fn accounting_server_sample_plus_inflated_tail() {
    let mut tracker = TokenTracker::default();
    let messages = vec![
        Message::user("a".repeat(400)), // 100 tokens
        Message::user("b".repeat(400)), // 100 tokens
    ];
    // No sample yet: whole-history heuristic.
    assert_eq!(tracker.estimate(&messages), 200);
    tracker.record_usage(
        &Usage {
            input_tokens: 1000,
            ..Default::default()
        },
        1, // the sample covered only the first message
    );
    // Tail = second message (100 tokens) inflated 25% → 125.
    assert_eq!(tracker.estimate(&messages), 1125);
    // A zero sample is no sample (scripted drivers report 0).
    tracker.record_usage(&Usage::default(), 2);
    assert_eq!(tracker.estimate(&messages), 1125);
    // Post-compaction recompute drops the stale sample.
    tracker.reset();
    assert_eq!(tracker.estimate(&messages), 200);
}

#[test]
fn trigger_is_ninety_percent_of_window() {
    assert_eq!(default_auto_compact_limit(128_000), 115_200);
    assert_eq!(default_auto_compact_limit(1_000_000), 900_000);
}

#[test]
fn unknown_model_window_defaults_to_128k_pinned() {
    // Catalog policy (codex r3 build-brief): pinned so a catalog change
    // cannot silently widen the unknown-model fallback.
    assert_eq!(
        nano_model::flux_common::DEFAULT_CONTEXT_WINDOW_TOKENS,
        128_000
    );
    assert_eq!(
        nano_model::flux_common::context_window_for("no-such-model", &[]),
        128_000
    );
    let catalog = vec![nano_model::flux_models::FluxModel {
        id: "flux-auto".into(),
        name: "flux-auto".into(),
        max_input_tokens: Some(1_000_000),
    }];
    assert_eq!(
        nano_model::flux_common::context_window_for("flux-auto", &catalog),
        1_000_000
    );
    // A catalog entry WITHOUT max_input_tokens still gets the default.
    let unclassified = vec![nano_model::flux_models::FluxModel {
        id: "flux-voice".into(),
        name: "flux-voice".into(),
        max_input_tokens: None,
    }];
    assert_eq!(
        nano_model::flux_common::context_window_for("flux-voice", &unclassified),
        128_000
    );
}

#[test]
fn config_overrides_are_downward_only() {
    let resolved = resolve_compaction_config(1_000_000, None, None).unwrap();
    assert_eq!(resolved.context_window, 1_000_000);
    assert_eq!(resolved.auto_compact_limit, 900_000);
    // Downward overrides accepted.
    let resolved = resolve_compaction_config(1_000_000, Some(500_000), Some(100_000)).unwrap();
    assert_eq!(resolved.context_window, 500_000);
    assert_eq!(resolved.auto_compact_limit, 100_000);
    // Upward overrides rejected with typed errors.
    assert!(matches!(
        resolve_compaction_config(1_000_000, Some(2_000_000), None),
        Err(CompactionConfigError::WindowOverrideUpward { .. })
    ));
    assert!(matches!(
        resolve_compaction_config(1_000_000, None, Some(900_001)),
        Err(CompactionConfigError::LimitOverrideUpward { .. })
    ));
    assert!(matches!(
        resolve_compaction_config(1_000_000, Some(0), None),
        Err(CompactionConfigError::WindowOverrideZero)
    ));
    assert!(matches!(
        resolve_compaction_config(1_000_000, None, Some(0)),
        Err(CompactionConfigError::LimitOverrideZero)
    ));
    // The window override narrows the derived limit ceiling too.
    assert!(matches!(
        resolve_compaction_config(1_000_000, Some(100_000), Some(500_000)),
        Err(CompactionConfigError::LimitOverrideUpward { .. })
    ));
}

#[test]
fn loop_guard_semantics() {
    let mut guard = AutoCompactGuard::default();
    // First ineffective compaction: no trip.
    assert!(!guard.record_ineffective());
    // A re-baseline below the trigger re-arms.
    guard.reset();
    assert!(!guard.record_ineffective());
    // Two IN A ROW trip the guard.
    assert!(guard.record_ineffective());
}

// ── canonical builder ────────────────────────────────────────────────────

fn assistant_with_use(id: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: "fs_read".into(),
            input: serde_json::json!({}),
        }],
    }
}

#[test]
fn builder_keeps_user_messages_drops_the_rest() {
    let messages = vec![
        Message::user("first question"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "answer".into(),
            }],
        },
        assistant_with_use("c1"),
        Message::tool_result("c1", "payload", false),
        Message::system("repeat-protection nag"),
        Message::user("second question"),
        Message::user(format!("{SUMMARY_PREFIX}\nan older summary")),
    ];
    let built = build_compacted_history(messages, "the new summary");
    assert_eq!(built.len(), 3);
    assert_eq!(built[0], Message::user("first question"));
    assert_eq!(built[1], Message::user("second question"));
    // The summary is the final user message, marked; prior summaries and
    // mid-turn system nags are gone, assistant/tool messages wholesale-removed.
    assert_eq!(built[2].role, Role::User);
    let ContentBlock::Text { text } = &built[2].content[0] else {
        panic!("text block")
    };
    assert!(text.starts_with(SUMMARY_PREFIX));
    assert!(text.contains("the new summary"));
    assert!(!text.contains("an older summary"));
}

#[test]
fn builder_truncates_at_utf8_boundary_over_cap() {
    // Force a tiny cap by constructing messages that exceed it: the 20k-token
    // cap is 80k bytes; one 90k-byte message crosses it mid-message.
    let mut big = "ü".repeat(45_000); // 90_000 bytes, 2 bytes per char
    big.push_str("tail");
    let messages = vec![Message::user("older"), Message::user(big.clone())];
    let built = build_compacted_history(messages, "s");
    // Newest-first: the big (newest) message fits in the 80k budget only
    // partially — wait: 90k > 80k, so it is THE overflow message, truncated
    // to its tail; the older message is never reached.
    assert_eq!(built.len(), 2);
    let ContentBlock::Text { text } = &built[0].content[0] else {
        panic!("text")
    };
    assert!(text.len() <= 80_000, "capped: {}", text.len());
    assert!(text.is_char_boundary(0));
    assert!(text.ends_with("tail"), "keeps the tail nearest the present");
    assert!(!text.contains("older"));
}

#[test]
fn builder_is_deterministic_byte_identical() {
    let messages = vec![
        Message::user("q1"),
        assistant_with_use("c1"),
        Message::tool_result("c1", "out", false),
        Message::user("q2"),
    ];
    let a = build_compacted_history(messages.clone(), "summary");
    let b = build_compacted_history(messages, "summary");
    assert_eq!(a, b);
}

// ── repair pass (synthetic tears only; the pre-fix turn.rs tears are
// covered by their own regression tests below) ────────────────────────────

#[test]
fn repair_pass_synthesizes_missing_results_and_drops_orphans() {
    let torn = vec![
        Message::user("go"),
        assistant_with_use("c1"), // no result: torn
        Message::tool_result("c-orphan", "orphaned", false), // no use: orphan
        Message::tool_result("c1-extra", "also orphaned", false),
    ];
    let repaired = normalize_history(torn);
    let uses: Vec<&str> = repaired
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let results: Vec<&str> = repaired
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(uses, vec!["c1"]);
    assert_eq!(results, vec!["c1"], "orphan results dropped, tear paired");
    // The synthesized result uses the shared encoding and marks the failure.
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = &repaired
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("synthesized result")
        .content[0]
    else {
        panic!("tool result")
    };
    assert!(content.contains("did not complete"));
    assert!(is_error);
}

// ── source-tear regressions (turn.rs Remind / ForceStop arms) ────────────

#[tokio::test]
async fn remind_arm_pairs_the_skipped_call() {
    // Three identical calls in one batch: the third hits the reminder streak
    // (REMIND_AT = 3) and is skipped — pre-fix it was never paired.
    let identical: Vec<ToolCall> = (0..3).map(|i| call(&format!("c{i}"), "fs_read")).collect();
    let model = FakeModel::new(
        vec![tool_response(identical, 0), text_response("done", 0)],
        vec![],
    );
    let tools = OkTools;
    let engine = engine(&model, &tools, None);
    let result = engine.run_turn("t-remind", "repeat").await;
    assert_eq!(result.state, TurnState::Complete);
    assert_no_torn_pairs(&model);
}

#[tokio::test]
async fn force_stop_arm_pairs_current_and_remaining_calls() {
    // Thirteen identical calls in one batch: streak 12 force-stops, leaving
    // the 12th AND 13th unexecuted — pre-fix both were stranded.
    let identical: Vec<ToolCall> = (0..13).map(|i| call(&format!("c{i}"), "fs_read")).collect();
    let model = FakeModel::new(vec![tool_response(identical, 0)], vec![]);
    let tools = OkTools;
    let engine = engine(&model, &tools, None);
    let result = engine.run_turn("t-stop", "repeat").await;
    assert!(matches!(result.state, TurnState::Stopped(_)));
    // The turn stopped, so no further model call carries the torn tail —
    // assert on the op stream instead: every journaled ToolCall has its
    // ToolResult. (Skipped calls are paired in the LIVE context only.)
    let mut open: Vec<&str> = Vec::new();
    for envelope in &result.ops {
        match &envelope.op {
            Op::ToolCall { call_id, .. } => open.push(call_id),
            Op::ToolResult { call_id, .. } => open.retain(|id| id != call_id),
            _ => {}
        }
    }
    assert!(open.is_empty(), "journaled calls all resolved: {open:?}");
}

// ── summarization escalation ─────────────────────────────────────────────

#[tokio::test]
async fn summarize_escalates_pair_preserving_then_succeeds() {
    let model = FakeModel::new(
        vec![],
        vec![
            Err(ModelError::ContextOverflow("too big".into())),
            text_response("compact summary", 0),
        ],
    );
    let history = vec![
        Message::user("u1"),
        assistant_with_use("c1"),
        Message::tool_result("c1", "out", false),
        Message::user("u2"),
    ];
    let summary = summarize(&model, "mock", &history).await.unwrap();
    assert_eq!(summary, "compact summary");
    // The second summarization request dropped the oldest message; the pair
    // went with it when the orphan result reached the head.
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].messages.len(), 5); // 4 history + prompt
    let second = &requests[1].messages;
    assert!(second.len() < 5, "escalation shrank the history");
    // No orphan tool result may remain at the head of the retried history.
    let mut saw_use = false;
    for message in second.iter().take(second.len() - 1) {
        for block in &message.content {
            match block {
                ContentBlock::ToolUse { .. } => saw_use = true,
                ContentBlock::ToolResult { .. } => assert!(saw_use, "no orphan-first result"),
                _ => {}
            }
        }
    }
}

#[tokio::test]
async fn summarize_gives_up_at_one_item_typed() {
    let model = FakeModel::new(
        vec![],
        vec![
            Err(ModelError::ContextOverflow("x".into())),
            Err(ModelError::ContextOverflow("x".into())),
            Err(ModelError::ContextOverflow("x".into())),
        ],
    );
    let history = vec![
        Message::user("u1"),
        Message::user("u2"),
        Message::user("u3"),
    ];
    let err = summarize(&model, "mock", &history).await.unwrap_err();
    assert_eq!(err, CompactionError::OverflowEscalationExhausted);
}

// ── commit protocol (compact_messages) ───────────────────────────────────

#[tokio::test]
async fn protocol_journals_begin_complete_then_swaps() {
    let model = FakeModel::new(vec![], vec![text_response("clean summary", 0)]);
    let mut messages = vec![Message::user("work on the thing")];
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-1",
        vec!["op-1".into()],
        vec!["src/main.rs".into()],
        false,
        &mut emit,
    )
    .await
    .unwrap();
    assert!(matches!(journaled[0], Op::CompactionBegin { .. }));
    assert!(matches!(journaled[1], Op::CompactionComplete { .. }));
    assert_eq!(journaled.len(), 2);
    // The swap happened: history is user messages + marked summary.
    assert_eq!(messages.len(), 2);
    let ContentBlock::Text { text } = &messages[1].content[0] else {
        panic!("text")
    };
    assert!(text.starts_with(SUMMARY_PREFIX));
}

#[tokio::test]
async fn canary_summary_is_never_persisted() {
    // The model embeds a synthetic canary; the gate must reject, journal a
    // bounded-reason Cancel, and leave the original history intact.
    let model = FakeModel::new(
        vec![],
        vec![text_response(
            "notes: the key was wayland-nano-canary-a1b2c3d4 ok",
            0,
        )],
    );
    let original = vec![Message::user("secret-bearing work")];
    let mut messages = original.clone();
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    let err = compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-2",
        vec![],
        vec![],
        false,
        &mut emit,
    )
    .await
    .unwrap_err();
    assert_eq!(err, CompactionError::RedactionHit);
    assert_eq!(messages, original, "history retained untouched");
    assert_eq!(journaled.len(), 2);
    let Op::CompactionCancel { reason, .. } = &journaled[1] else {
        panic!("second op must be the cancel")
    };
    assert_eq!(*reason, CompactionCancelReason::RedactionHit);
    // Nothing carrying the summary was journaled.
    assert!(
        !journaled
            .iter()
            .any(|op| matches!(op, Op::CompactionComplete { .. }))
    );
}

#[tokio::test]
async fn redactor_error_fails_closed() {
    // An oversized summary trips the scanner's internal bound — a scanner
    // limitation, not a hit — and must STILL fail closed.
    let huge = "s".repeat(nano_session::redaction::MAX_SCAN_BYTES + 1);
    let model = FakeModel::new(vec![], vec![text_response(&huge, 0)]);
    let mut messages = vec![Message::user("work")];
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    let err = compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-3",
        vec![],
        vec![],
        false,
        &mut emit,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CompactionError::RedactorError(_)));
    let Op::CompactionCancel { reason, .. } = &journaled[1] else {
        panic!("cancel expected")
    };
    assert_eq!(*reason, CompactionCancelReason::RedactorError);
}

#[tokio::test]
async fn journal_failure_blocks_the_swap() {
    let model = FakeModel::new(vec![], vec![text_response("summary", 0)]);
    let original = vec![Message::user("work")];
    let mut messages = original.clone();
    // Fail ONLY the CompactionComplete append.
    let mut emit = |op: Op| !matches!(op, Op::CompactionComplete { .. });
    let err = compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-4",
        vec![],
        vec![],
        false,
        &mut emit,
    )
    .await
    .unwrap_err();
    assert_eq!(err, CompactionError::JournalWrite);
    assert_eq!(messages, original, "no swap without a durable Complete");
    // Begin failing blocks even the summarization call.
    let calls_before = model.requests.lock().unwrap().len();
    let mut messages2 = original.clone();
    let mut deny_all = |_: Op| false;
    let err = compact_messages(
        &model,
        "mock",
        &mut messages2,
        "c-5",
        vec![],
        vec![],
        false,
        &mut deny_all,
    )
    .await
    .unwrap_err();
    assert_eq!(err, CompactionError::JournalWrite);
    assert_eq!(messages2, original);
    assert_eq!(
        model.requests.lock().unwrap().len(),
        calls_before,
        "no summarization call without a journaled Begin"
    );
}

// ── integration: scripted long session crossing the threshold ────────────

#[tokio::test]
async fn long_session_compacts_at_loop_top_and_continues() {
    let config = CompactionConfig {
        context_window: 1_000,
        auto_compact_limit: 900,
    };
    let model = FakeModel::new(
        vec![
            tool_response(vec![call("c1", "fs_edit")], 950),
            text_response("fixed it", 100),
        ],
        vec![text_response(
            "user wants the build fixed; edited main.rs",
            0,
        )],
    );
    let tools = OkTools;
    let engine = engine(&model, &tools, Some(config));
    let result = engine.run_turn("t-auto", "fix the build").await;
    assert_eq!(result.state, TurnState::Complete);
    assert_eq!(result.final_text, "fixed it");
    let kinds = op_kinds(&result);
    let begin = kinds.iter().position(|k| *k == "CompactionBegin").unwrap();
    let complete = kinds
        .iter()
        .position(|k| *k == "CompactionComplete")
        .unwrap();
    assert!(begin < complete, "Begin precedes Complete");
    assert_no_torn_pairs(&model);
    // The continuation call sees the compacted history: the journaled
    // summary is its final message.
    let requests = model.requests.lock().unwrap();
    let continuation = &requests[2];
    let ContentBlock::Text { text } = &continuation.messages.last().unwrap().content[0] else {
        panic!("text")
    };
    assert!(text.contains("user wants the build fixed"));
    assert!(text.starts_with(SUMMARY_PREFIX));
}

#[tokio::test]
async fn compaction_never_fires_mid_batch() {
    // Usage crosses the limit while a multi-call batch runs; the seam is
    // loop-top only, so compaction must wait for the batch to finish — and
    // no approval can be pending when it fires (the batch is complete).
    let config = CompactionConfig {
        context_window: 1_000,
        auto_compact_limit: 900,
    };
    let model = FakeModel::new(
        vec![
            tool_response(vec![call("c1", "fs_edit"), call("c2", "fs_edit")], 950),
            text_response("done", 100),
        ],
        vec![text_response("summary", 0)],
    );
    let tools = OkTools;
    let engine = engine(&model, &tools, Some(config));
    let result = engine.run_turn("t-seam", "work").await;
    assert_eq!(result.state, TurnState::Complete);
    // In the op stream, no Compaction op may sit between a ToolCall and its
    // ToolResult (the pending-approval impossibility proxy: the seam only
    // exists where nothing is outstanding).
    let mut open: Vec<&str> = Vec::new();
    for envelope in &result.ops {
        match &envelope.op {
            Op::ToolCall { call_id, .. } => open.push(call_id),
            Op::ToolResult { call_id, .. } => open.retain(|id| id != call_id),
            Op::CompactionBegin { .. } | Op::CompactionComplete { .. } => {
                assert!(open.is_empty(), "compaction fired with calls outstanding")
            }
            _ => {}
        }
    }
    assert_no_torn_pairs(&model);
}

#[tokio::test]
async fn reactive_overflow_routes_into_compaction_once() {
    let config = CompactionConfig {
        context_window: 1_000_000,
        auto_compact_limit: 900_000,
    };
    // First call overflows (heuristic undershot), compaction runs, retry ok.
    let model = FakeModel::new(
        vec![
            Err(ModelError::ContextOverflow(
                "context_window_exceeded".into(),
            )),
            text_response("recovered", 100),
        ],
        vec![text_response("summary so far", 0)],
    );
    let tools = OkTools;
    let engine = engine(&model, &tools, Some(config));
    let result = engine.run_turn("t-reactive", "big context").await;
    assert_eq!(result.state, TurnState::Complete);
    assert_eq!(result.final_text, "recovered");
    assert_eq!(
        op_kinds(&result)
            .iter()
            .filter(|k| **k == "CompactionComplete")
            .count(),
        1
    );
}

#[tokio::test]
async fn overflow_again_after_reactive_compaction_fails_typed() {
    let config = CompactionConfig {
        context_window: 1_000_000,
        auto_compact_limit: 900_000,
    };
    let model = FakeModel::new(
        vec![
            Err(ModelError::ContextOverflow("x".into())),
            Err(ModelError::ContextOverflow("x".into())),
        ],
        vec![text_response("summary", 0)],
    );
    let tools = OkTools;
    let engine = engine(&model, &tools, Some(config));
    let result = engine.run_turn("t-reactive-2", "big context").await;
    let TurnState::Failed(reason) = &result.state else {
        panic!("must fail typed, got {:?}", result.state)
    };
    assert!(reason.detail.contains("context overflow"), "{reason:?}");
    // Exactly ONE compaction happened between the two overflows.
    assert_eq!(
        op_kinds(&result)
            .iter()
            .filter(|k| **k == "CompactionComplete")
            .count(),
        1
    );
}

#[tokio::test]
async fn loop_guard_trips_on_repeated_ineffective_compactions() {
    let config = CompactionConfig {
        context_window: 1_000,
        auto_compact_limit: 900,
    };
    // Every summary is huge (5000 bytes ≈ 1250 estimated tokens ≥ 900), so
    // no compaction ever re-baselines below the trigger. The turn keeps
    // working between compactions (usage 950 ≥ 900 each time), so the guard
    // sees two INEFFECTIVE compactions in a row and trips.
    let big_summary = "s".repeat(5_000);
    let model = FakeModel::new(
        vec![
            tool_response(vec![call("c1", "fs_edit")], 950),
            tool_response(vec![call("c2", "fs_edit")], 950),
        ],
        vec![
            text_response(&big_summary, 0),
            text_response(&big_summary, 0),
        ],
    );
    let tools = OkTools;
    let engine = engine(&model, &tools, Some(config));
    let result = engine.run_turn("t-guard", "work").await;
    let TurnState::Failed(reason) = &result.state else {
        panic!("guard must fail the turn, got {:?}", result.state)
    };
    assert!(reason.detail.contains("loop guard"), "{reason:?}");
    assert_eq!(
        op_kinds(&result)
            .iter()
            .filter(|k| **k == "CompactionComplete")
            .count(),
        2,
        "two ineffective compactions, then the trip"
    );
}

#[tokio::test]
async fn flush_before_swap_is_observable_at_the_sink() {
    // The post-compaction model call must observe CompactionComplete already
    // durable at the sink (journal-first, then swap).
    use std::sync::Arc;
    let journaled: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_by_model: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));

    #[derive(Debug)]
    struct ObservingModel {
        inner: FakeModel,
        journaled: Arc<Mutex<Vec<&'static str>>>,
        seen: Arc<Mutex<Vec<bool>>>,
    }
    #[async_trait::async_trait]
    impl ModelDriver for ObservingModel {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let complete_journaled = self
                .journaled
                .lock()
                .unwrap()
                .contains(&"CompactionComplete");
            self.seen.lock().unwrap().push(complete_journaled);
            self.inner.complete(request).await
        }
    }

    let config = CompactionConfig {
        context_window: 1_000,
        auto_compact_limit: 900,
    };
    let inner = FakeModel::new(
        vec![
            tool_response(vec![call("c1", "fs_edit")], 950),
            text_response("done", 100),
        ],
        vec![text_response("summary", 0)],
    );
    let model = ObservingModel {
        inner,
        journaled: journaled.clone(),
        seen: seen_by_model.clone(),
    };
    let tools = OkTools;
    let engine = engine(&model, &tools, Some(config));
    let mut sink = |envelope: &nano_session::op::OpEnvelope| {
        journaled.lock().unwrap().push(match &envelope.op {
            Op::CompactionComplete { .. } => "CompactionComplete",
            _ => "Other",
        });
        true
    };
    let result = engine
        .run_turn_streaming("t-flush", "work", None, &mut sink)
        .await;
    assert_eq!(result.state, TurnState::Complete);
    let seen = seen_by_model.lock().unwrap();
    // Calls: 1 = normal (no compaction yet), 2 = summarization (Begin
    // journaled, Complete not yet), 3 = post-compaction (Complete durable).
    assert_eq!(seen.len(), 3);
    assert!(!seen[0] && !seen[1]);
    assert!(
        seen[2],
        "the post-compaction call observed a durable Complete"
    );
}

// ── canonical-builder live/replay equality (engine side; the journal-fold
// half is pinned in nano-cli) ─────────────────────────────────────────────

#[test]
fn builder_output_matches_rule_by_rule() {
    let summary = "S";
    let messages = vec![
        Message::system("nag"),
        Message::user("keep me"),
        Message::user(format!("{SUMMARY_PREFIX}\nold")),
        assistant_with_use("c1"),
        Message::tool_result("c1", "out", false),
    ];
    let built = build_compacted_history(messages, summary);
    let expected = vec![
        Message::user("keep me"),
        Message::user(format!("{SUMMARY_PREFIX}\n{summary}")),
    ];
    assert_eq!(built, expected, "byte-identical by construction");
}

// ── P2a §8: image-aware compaction ───────────────────────────────────────

fn p2a_image_message(data: &str) -> Message {
    Message::user_blocks(vec![
        ContentBlock::Text { text: "see".into() },
        ContentBlock::Image {
            mime: "image/png".into(),
            data: data.to_string(),
        },
    ])
}

/// §8 part 3: the Image arm of message_bytes returns EXACTLY 6,000 bytes
/// (1,500 tokens at BYTES_PER_TOKEN = 4) — never the base64 length.
#[test]
fn p2a_message_bytes_image_arm_is_6000() {
    let message = Message::user_blocks(vec![ContentBlock::Image {
        mime: "image/png".into(),
        data: "A".repeat(100_000),
    }]);
    assert_eq!(message_bytes(&message), 6_000);
    let mixed = Message::user_blocks(vec![
        ContentBlock::Text {
            text: "abcd".into(),
        },
        ContentBlock::Image {
            mime: "image/png".into(),
            data: "A".repeat(10),
        },
    ]);
    assert_eq!(message_bytes(&mixed), 4 + 6_000);
}

/// §8 part 1: the pre-summary transform replaces every Image IN PLACE with
/// the structural placeholder and reports whether anything was evicted.
#[test]
fn p2a_evict_images_with_placeholders_in_place() {
    let mut messages = vec![
        Message::user_blocks(vec![
            ContentBlock::Text { text: "a".into() },
            ContentBlock::Image {
                mime: "image/png".into(),
                data: "x".into(),
            },
            ContentBlock::Image {
                mime: "image/png".into(),
                data: "y".into(),
            },
        ]),
        Message::user("plain"),
    ];
    assert!(evict_images_with_placeholders(&mut messages));
    let content = &messages[0].content;
    assert!(matches!(&content[0], ContentBlock::Text { text } if text == "a"));
    assert!(
        matches!(&content[1], ContentBlock::Text { text } if text == COMPACT_IMAGE_PLACEHOLDER)
    );
    assert!(
        matches!(&content[2], ContentBlock::Text { text } if text == COMPACT_IMAGE_PLACEHOLDER)
    );
    assert!(!evict_images_with_placeholders(&mut messages));
}

/// §8 parts 1+2: the summarize call receives ZERO Image blocks (pixels
/// never reach the summarizer), and the journaled CompactionComplete
/// carries image_influenced=true (any-image-evicted arm).
#[tokio::test]
async fn p2a_summarizer_never_sees_pixels_and_provenance_journaled() {
    let model = FakeModel::new(vec![], vec![text_response("clean summary", 0)]);
    let mut messages = vec![
        p2a_image_message(&"A".repeat(1000)),
        Message::user("follow-up"),
    ];
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-img",
        vec![],
        vec![],
        false,
        &mut emit,
    )
    .await
    .unwrap();
    // The captured summarize request held ZERO image blocks.
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let saw_image = requests[0]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .any(|b| matches!(b, ContentBlock::Image { .. }));
    assert!(!saw_image, "the summarizer must never see pixels");
    let saw_placeholder = requests[0]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .any(|b| matches!(b, ContentBlock::Text { text } if text == COMPACT_IMAGE_PLACEHOLDER));
    assert!(
        saw_placeholder,
        "the transform left the explicit placeholder"
    );
    drop(requests);
    // Provenance: before=false, but an image was evicted → true.
    let Op::CompactionComplete {
        image_influenced, ..
    } = &journaled[1]
    else {
        panic!("complete expected")
    };
    assert!(image_influenced);
    // The LIVE history kept its (under-budget) image block.
    assert!(
        messages
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|b| matches!(b, ContentBlock::Image { .. }))
    );
}

/// §8 part 2: the session-side sticky flag alone forces provenance true
/// (transitivity), and with neither source the flag journals false.
#[tokio::test]
async fn p2a_provenance_flag_sources() {
    let model = FakeModel::new(vec![], vec![text_response("s", 0)]);
    let mut messages = vec![Message::user("plain work")];
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-a",
        vec![],
        vec![],
        true,
        &mut emit,
    )
    .await
    .unwrap();
    let Op::CompactionComplete {
        image_influenced, ..
    } = &journaled[1]
    else {
        panic!("complete expected")
    };
    assert!(*image_influenced, "sticky session flag propagates");

    let model = FakeModel::new(vec![], vec![text_response("s", 0)]);
    let mut messages = vec![Message::user("plain work")];
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-b",
        vec![],
        vec![],
        false,
        &mut emit,
    )
    .await
    .unwrap();
    let Op::CompactionComplete {
        image_influenced, ..
    } = &journaled[1]
    else {
        panic!("complete expected")
    };
    assert!(!*image_influenced, "no images, no flag → false");
}

/// §8 parts 3+4: images are COUNTED against the retained-history budget;
/// under-budget images survive intact, over-budget images leave the
/// placeholder at BOTH the tail-cut crosser and wholesale-dropped older
/// messages, and an image-only message is never silently dropped.
#[test]
fn p2a_builder_counts_and_evicts_images_with_placeholders() {
    // Under budget: image retained verbatim.
    let history = vec![Message::user("older"), p2a_image_message(&"A".repeat(500))];
    let built = build_compacted_history(history, "sum");
    assert!(
        built
            .iter()
            .flat_map(|m| m.content.iter())
            .any(|b| matches!(b, ContentBlock::Image { .. })),
        "under-budget image retained: {built:?}"
    );

    // Budget is 80,000 bytes. Build: oldest = text 30,000 + image (36,000),
    // then an image-only message (6,000), then two more text+image
    // (36,000 each). Newest-first: 36,000 + 36,000 = 72,000 fit intact;
    // the image-only (6,000) crosses (78,000 > 80,000? no — 72,000+6,000
    // = 78,000 fits); the oldest (36,000) crosses → its image becomes a
    // placeholder and its text tail fills the remaining 2,000 bytes.
    let make = || {
        vec![
            Message::user_blocks(vec![
                ContentBlock::Text {
                    text: "a".repeat(30_000),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "A".repeat(100),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::Image {
                mime: "image/png".into(),
                data: "B".repeat(100),
            }]),
            Message::user_blocks(vec![
                ContentBlock::Text {
                    text: "c".repeat(30_000),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "C".repeat(100),
                },
            ]),
            Message::user_blocks(vec![
                ContentBlock::Text {
                    text: "d".repeat(30_000),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "D".repeat(100),
                },
            ]),
        ]
    };
    let built = build_compacted_history(make(), "sum");
    // Determinism: same input → identical output.
    assert_eq!(built, build_compacted_history(make(), "sum"));
    // The two newest image messages survived intact (72,000 ≤ 80,000), the
    // image-only one fit (78,000 ≤ 80,000), the oldest crossed: its image
    // was evicted with the explicit placeholder and its text tail survived.
    let images = built
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| matches!(b, ContentBlock::Image { .. }))
        .count();
    let placeholders = built
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| matches!(b, ContentBlock::Text { text } if text == COMPACT_IMAGE_PLACEHOLDER))
        .count();
    assert_eq!(images, 3, "under-budget images retained: {built:?}");
    assert_eq!(placeholders, 1, "one placeholder for the one dropped image");
    // The image-only message was never silently dropped: it is present with
    // its image block (it fit under the cap).
    // The crosser's text tail: 80,000 − 78,000 = 2,000 bytes of tail.
    let crosser = &built[0];
    let tail_len: usize = crosser
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } if text != COMPACT_IMAGE_PLACEHOLDER => text.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(tail_len, 2_000, "crosser keeps its tail within budget");
}

/// §8 part 5: enforce_image_budget is oldest-first, deterministic, and
/// leaves the explicit placeholder; under-budget histories are untouched.
/// (The aggregate cap constant is lane-A-owned — §4.2, 50 MiB.)
#[test]
fn p2a_enforce_image_budget_oldest_first() {
    // Under the cap: untouched.
    let mut messages = vec![p2a_image_message(&"A".repeat(1000))];
    enforce_image_budget(&mut messages);
    assert!(
        matches!(messages[0].content[1], ContentBlock::Image { .. }),
        "under-budget history untouched"
    );

    // Over the cap: two images whose RAW estimate exceeds 50 MiB
    // (data.len()/4*3 per image = 27 MiB → aggregate 54 MiB > 50 MiB):
    // the OLDEST is evicted, the newest survives.
    let big = "A".repeat(36_000_000);
    let mut messages = vec![
        Message::user_blocks(vec![ContentBlock::Image {
            mime: "image/png".into(),
            data: big.clone(),
        }]),
        Message::user_blocks(vec![ContentBlock::Image {
            mime: "image/png".into(),
            data: big,
        }]),
    ];
    enforce_image_budget(&mut messages);
    assert!(
        matches!(&messages[0].content[0], ContentBlock::Text { text } if text == COMPACT_IMAGE_PLACEHOLDER),
        "oldest evicted with placeholder"
    );
    assert!(
        matches!(messages[1].content[0], ContentBlock::Image { .. }),
        "newest survives once under budget"
    );
}

// ── P2b §3.5: result-carried images in accounting, eviction, and pairing ──

/// A ToolResult message carrying `count` 1x1 images (base64 payload sized by
/// the caller) — the P2b shape under test.
fn p2b_result_message(call_id: &str, content: &str, count: usize, data: &str) -> Message {
    Message {
        role: nano_model::types::Role::Tool,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: call_id.into(),
            content: content.into(),
            is_error: false,
            images: (0..count)
                .map(|_| nano_model::types::ImageData {
                    mime: "image/png".into(),
                    data: data.to_string(),
                })
                .collect(),
        }],
    }
}

/// §3.5: the ToolResult arm of message_bytes counts `6,000 × images` ON TOP
/// OF the id + projection text — no result image survives uncounted, and the
/// base64 length never leaks into the estimate.
#[test]
fn p2b_message_bytes_tool_result_arm_counts_6000_per_image() {
    let bare = p2b_result_message("c1", "abcd", 0, "A");
    assert_eq!(message_bytes(&bare), 2 + 4);
    let two = p2b_result_message("c1", "abcd", 2, &"A".repeat(100_000));
    assert_eq!(message_bytes(&two), 2 + 4 + 2 * IMAGE_BLOCK_BYTES);
}

/// §3.5 eviction: over the §4.2 per-prompt aggregate budget, result-carried
/// images are evicted OLDEST-FIRST with the explicit placeholder appended to
/// the projection, and the pairing key (`tool_use_id`) is structurally
/// untouched — `normalize_history` sees the same pairing before and after.
#[test]
fn p2b_enforce_image_budget_evicts_result_images_oldest_first_pairing_intact() {
    let over_cap_data = "A".repeat(((MAX_PROMPT_IMAGE_AGGREGATE_BYTES / 3 * 4) + 4096) as usize);
    let mut messages = vec![
        Message {
            role: nano_model::types::Role::Assistant,
            content: vec![
                ContentBlock::ToolUse {
                    id: "old".into(),
                    name: "view_image".into(),
                    input: serde_json::json!({}),
                },
                ContentBlock::ToolUse {
                    id: "new".into(),
                    name: "view_image".into(),
                    input: serde_json::json!({}),
                },
            ],
        },
        p2b_result_message(
            "old",
            "[Image #1 from tool view_image — 1x1 png]",
            1,
            &over_cap_data,
        ),
        p2b_result_message(
            "new",
            "[Image #1 from tool view_image — 1x1 png]",
            1,
            "aGVsbG8",
        ),
    ];
    enforce_image_budget(&mut messages);
    // Oldest evicted (placeholder appended, images cleared); newest kept.
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        images,
        ..
    } = &messages[1].content[0]
    else {
        panic!("result block")
    };
    assert_eq!(tool_use_id, "old", "pairing key untouched");
    assert!(images.is_empty(), "oldest result image evicted");
    assert!(
        content.contains("[Image #1 from tool view_image")
            && content.contains(COMPACT_IMAGE_PLACEHOLDER),
        "label preserved + explicit placeholder appended: {content}"
    );
    let ContentBlock::ToolResult { images, .. } = &messages[2].content[0] else {
        panic!("result block")
    };
    assert_eq!(images.len(), 1, "newest image survives");
    // Pairing intact: the repair pass sees the same tool_use_id set, so the
    // post-eviction history introduces NO orphans — normalize_history drops
    // nothing.
    let normalized = normalize_history(messages.clone());
    assert_eq!(
        normalized, messages,
        "eviction left pairing intact (no orphaned results)"
    );
}

/// §3.5 provenance: the summarizer never sees result pixels (the pre-summary
/// transform clears ToolResult.images and appends the placeholder), and the
/// journaled CompactionComplete carries image_influenced=true for an evicted
/// RESULT image exactly as for an intake image.
#[tokio::test]
async fn p2b_summarizer_never_sees_result_pixels_and_provenance_journaled() {
    let model = FakeModel::new(vec![], vec![text_response("clean summary", 0)]);
    let mut messages = vec![
        p2b_result_message(
            "c1",
            "[Image #1 from tool view_image — 1x1 png]",
            1,
            &"A".repeat(1000),
        ),
        Message::user("follow-up"),
    ];
    let mut journaled: Vec<Op> = Vec::new();
    let mut emit = |op: Op| {
        journaled.push(op);
        true
    };
    compact_messages(
        &model,
        "mock",
        &mut messages,
        "c-res-img",
        vec![],
        vec![],
        false,
        &mut emit,
    )
    .await
    .unwrap();
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let saw_result_pixels = requests[0]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .any(|b| matches!(b, ContentBlock::ToolResult { images, .. } if !images.is_empty()));
    assert!(
        !saw_result_pixels,
        "the summarizer must never see result pixels"
    );
    let saw_placeholder = requests[0]
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .any(|b| matches!(b, ContentBlock::ToolResult { content, .. } if content.contains(COMPACT_IMAGE_PLACEHOLDER)));
    assert!(
        saw_placeholder,
        "the transform left the explicit placeholder"
    );
    drop(requests);
    let Op::CompactionComplete {
        image_influenced, ..
    } = &journaled[1]
    else {
        panic!("complete expected")
    };
    assert!(
        image_influenced,
        "an evicted result image sets the provenance flag"
    );
}
