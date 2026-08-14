//! P5 — Flux Auto routing: the §8.1 deterministic matrix and the §8.2
//! seam/live legs (design: shared/reviews/panel-tui/P5-auto-routing-design.md,
//! REVISED round 3).
//!
//! - §8.1 host-level legs drive `acp_mode::serve` in-process (the C8
//!   harness pattern) with scripted drivers and a REAL journal directory.
//! - §8.1 ladder-level legs and the §8.2 [seam] legs drive the §8
//!   deterministic test seam: injected candidate snapshots plus
//!   per-candidate scripted/loopback transports (never production dispatch,
//!   never an egress or catalog redirect).
//! - §8.2 [live] legs self-skip without FLUX_TEST_KEY (resolved via the
//!   standard flux_key chain at call time; the key value is never logged,
//!   and captured output is canary-scanned before it lands in a fixture).
//!
//! Env-mutating cases run under one file-wide lock and always restore the
//! process env (other test files are separate processes).

use std::collections::VecDeque;
use std::io::Read as _;
use std::sync::{Arc, Mutex, MutexGuard};

use nano_agent::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_cli::acp_mode;
use nano_cli::auto_routing::{
    self, AdmittedCandidate, AttemptOutcome, CandidateTransport, Ladder, LadderCandidate,
    RoutingSink,
};
use nano_cli::provider_router::ProviderRouter;
use nano_model::types::{
    CallHooks, ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage,
};
use nano_protocol::acp::AvailableModel;
use nano_session::op::{
    CandidateKind, LeafProvenance, Op, OpEnvelope, RoutingMode, RoutingOutcome,
};
use nano_session::{JournalCoordinator, SessionState};

// ── env discipline (the c8 pattern) ─────────────────────────────────────

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

const TOUCHED_VARS: &[&str] = &[
    "FLUX_API_KEY",
    "FLUX_TEST_KEY",
    "FLUX_API_KEY_FILE",
    "OPENAI_API_KEY",
    "NVIDIA_API_KEY",
    "NANO_ROUTING_AUTO",
    "NANO_DEFAULT_MODEL",
    "NANO_AUTO_TOOLS_PROBE",
];

fn clear_env() {
    for var in TOUCHED_VARS {
        unsafe { std::env::remove_var(var) };
    }
}

struct EnvRestore(Vec<Option<String>>);

impl EnvRestore {
    fn snapshot() -> Self {
        Self(TOUCHED_VARS.iter().map(|v| std::env::var(v).ok()).collect())
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (var, value) in TOUCHED_VARS.iter().zip(self.0.iter()) {
            match value {
                Some(value) => unsafe { std::env::set_var(var, value) },
                None => unsafe { std::env::remove_var(var) },
            }
        }
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(future)
}

// ── the scripted seam transport ─────────────────────────────────────────

/// §8 seam: a per-candidate transport with a scripted queue of routing
/// attempts. Records every dispatched request; panics when the ladder runs
/// more attempts than scripted (a test bug, never a silent pass).
#[derive(Debug, Default)]
struct ScriptedTransport {
    calls: Arc<Mutex<Vec<ModelRequest>>>,
    steps: Mutex<VecDeque<AttemptOutcome>>,
}

impl ScriptedTransport {
    fn succeeds(model: Option<&str>, usage: Usage) -> Self {
        let transport = Self::default();
        transport.steps.lock().unwrap().push_back(AttemptOutcome {
            result: Ok(ModelResponse {
                events: vec![
                    ModelEvent::TextDelta("ok".into()),
                    ModelEvent::Done {
                        stop_reason: "stop".into(),
                    },
                ],
                usage,
                stop_reason: "stop".into(),
                model: model.map(str::to_string),
            }),
            attempts_consumed: 1,
            failed_usage: None,
        });
        transport
    }

    fn fails(err: ModelError, attempts_consumed: u32, failed_usage: Option<Usage>) -> Self {
        let transport = Self::default();
        transport.steps.lock().unwrap().push_back(AttemptOutcome {
            result: Err(err),
            attempts_consumed,
            failed_usage,
        });
        transport
    }
}

impl CandidateTransport for ScriptedTransport {
    async fn attempt(&self, request: &ModelRequest, _hooks: &CallHooks<'_>) -> AttemptOutcome {
        self.calls.lock().unwrap().push(request.clone());
        self.steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted transport ran out of steps")
    }
}

/// The journal sink that collects in memory (pure ladder legs).
#[derive(Debug, Default)]
struct CollectSink {
    envelopes: Mutex<Vec<OpEnvelope>>,
}

impl RoutingSink for CollectSink {
    fn append(&self, envelope: &OpEnvelope) -> bool {
        self.envelopes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(envelope.clone());
        true
    }
}

impl CollectSink {
    fn ops(&self) -> Vec<Op> {
        self.envelopes
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.op.clone())
            .collect()
    }
}

/// A sink that fails every append (the fail-closed journal leg).
#[derive(Debug, Default)]
struct FailingSink;

impl RoutingSink for FailingSink {
    fn append(&self, _envelope: &OpEnvelope) -> bool {
        false
    }
}

fn candidate(ordinal: u32, provider: &str, leaf: &str, kind: CandidateKind) -> AdmittedCandidate {
    AdmittedCandidate {
        ordinal,
        provider_id: provider.to_string(),
        candidate: leaf.to_string(),
        reference: if provider == "flux-router" {
            leaf.to_string()
        } else {
            format!("{provider}:{leaf}")
        },
        kind,
    }
}

fn ladder_with(
    candidates: Vec<(AdmittedCandidate, ScriptedTransport)>,
    sink: Arc<dyn RoutingSink>,
) -> Ladder<ScriptedTransport> {
    Ladder::new(
        "s-turn-1",
        RoutingMode::AutoClientSide,
        "flux-auto",
        candidates
            .into_iter()
            .map(|(plan, transport)| LadderCandidate { plan, transport })
            .collect(),
        sink,
        None,
        false,
        auto_routing::ATTEMPT_BUDGET,
        0,
    )
}

fn text_request() -> ModelRequest {
    ModelRequest {
        model: "flux-auto".into(),
        messages: vec![nano_model::types::Message::user("hello")],
        ..ModelRequest::default()
    }
}

fn receipts(ops: &[Op]) -> Vec<(u32, RoutingOutcome)> {
    ops.iter()
        .filter_map(|op| match op {
            Op::RoutingReceipt {
                ordinal, outcome, ..
            } => Some((*ordinal, *outcome)),
            _ => None,
        })
        .collect()
}

fn begins(ops: &[Op]) -> Vec<u32> {
    ops.iter()
        .filter_map(|op| match op {
            Op::RoutingAttemptBegin { ordinal, .. } => Some(*ordinal),
            _ => None,
        })
        .collect()
}

// ── §8.1 commit boundary + §8.2 leg 5 (auth terminality) ────────────────

#[test]
fn cascading_429_failover_journals_both_receipts() {
    // §8.2 leg 3 [seam, scripted arm]: 429 on rung 1, success on rung 2 —
    // exactly two attempts, intact request semantics, both receipts.
    let a = ScriptedTransport::fails(
        ModelError::RateLimited {
            retry_after_ms: None,
        },
        1,
        None,
    );
    let a_calls = a.calls.clone();
    let b = ScriptedTransport::succeeds(Some("gpt-a"), Usage::default());
    let b_calls = b.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            ),
            (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
        ],
        sink.clone(),
    );
    let response = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("rung 2 succeeds");
    assert_eq!(response.model.as_deref(), Some("gpt-a"));
    // Exactly one attempt per candidate; the wire model id is the
    // candidate's bare id; every other request field is intact.
    let a_calls = a_calls.lock().unwrap();
    let b_calls = b_calls.lock().unwrap();
    assert_eq!(a_calls.len(), 1);
    assert_eq!(b_calls.len(), 1);
    assert_eq!(a_calls[0].model, "flux-auto");
    assert_eq!(b_calls[0].model, "gpt-a");
    let mut stripped = a_calls[0].clone();
    stripped.model = b_calls[0].model.clone();
    assert_eq!(stripped, b_calls[0], "intact request semantics");
    drop(a_calls);
    drop(b_calls);
    // Journal: begin-0, receipt-0 (cascade, 429), begin-1, receipt-1
    // (committed, selected).
    let ops = sink.ops();
    assert_eq!(begins(&ops), [0, 1]);
    assert_eq!(
        receipts(&ops),
        [
            (0, RoutingOutcome::CascadeFailure),
            (1, RoutingOutcome::Committed)
        ]
    );
    let selected: Vec<u32> = ops
        .iter()
        .filter_map(|op| match op {
            Op::RoutingReceipt {
                ordinal,
                selected: true,
                ..
            } => Some(*ordinal),
            _ => None,
        })
        .collect();
    assert_eq!(selected, [1]);
    // The selected receipt carries the actual-leaf provenance (§6).
    let receipt = ops
        .iter()
        .find_map(|op| match op {
            Op::RoutingReceipt {
                ordinal: 1,
                response_model,
                leaf_identity,
                ..
            } => Some((response_model.clone(), *leaf_identity)),
            _ => None,
        })
        .expect("receipt 1");
    assert_eq!(receipt.0.as_deref(), Some("gpt-a"));
    assert_eq!(receipt.1, LeafProvenance::ProviderReported);
}

#[test]
fn auth_failures_are_terminal_zero_calls_to_later_candidates() {
    // §8.1 never-cascade + §8.2 leg 5 [seam]: 401 and 403 close the ladder.
    for status in [401, 403] {
        let a = ScriptedTransport::fails(
            ModelError::Auth {
                message: "bad key".into(),
                status: Some(status),
            },
            1,
            None,
        );
        let b = ScriptedTransport::succeeds(None, Usage::default());
        let b_calls = b.calls.clone();
        let sink = Arc::new(CollectSink::default());
        let ladder = ladder_with(
            vec![
                (
                    candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                    a,
                ),
                (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
            ],
            sink.clone(),
        );
        let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
            .expect_err("auth is terminal");
        assert!(matches!(err, ModelError::Auth { .. }));
        assert!(
            b_calls.lock().unwrap().is_empty(),
            "zero calls to later candidates on {status}"
        );
        let ops = sink.ops();
        assert_eq!(
            receipts(&ops),
            [(0, RoutingOutcome::TerminalFailure)],
            "{status}"
        );
    }
}

#[test]
fn commit_boundary_partial_stream_and_protocol_are_terminal() {
    // §8.1 commit boundary: partial SSE (MidStream transport) and partial
    // tool-call arguments (Protocol) are post-commit — terminal, zero calls
    // to later candidates.
    for err in [
        ModelError::Transport {
            phase: nano_model::types::TransportPhase::MidStream,
            message: "connection reset mid-body".into(),
        },
        ModelError::Protocol("partial tool-call arguments".into()),
        ModelError::Protocol("malformed success body".into()),
    ] {
        let a = ScriptedTransport::fails(err, 1, None);
        let b = ScriptedTransport::succeeds(None, Usage::default());
        let b_calls = b.calls.clone();
        let sink = Arc::new(CollectSink::default());
        let ladder = ladder_with(
            vec![
                (
                    candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                    a,
                ),
                (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
            ],
            sink,
        );
        assert!(block_on(ladder.complete_observed(&text_request(), &CallHooks::none())).is_err());
        assert!(b_calls.lock().unwrap().is_empty());
    }
}

#[test]
fn post_commit_failure_after_latch_never_reopens_the_ladder() {
    // §4 commit boundary: once a candidate emitted (latched), a LATER
    // failure — e.g. after tool dispatch — is terminal for routing: it
    // surfaces on the latched candidate, never cascades.
    let a = ScriptedTransport::default();
    a.steps.lock().unwrap().extend([
        AttemptOutcome {
            result: Ok(ModelResponse {
                events: vec![ModelEvent::ToolCallComplete(ToolCall {
                    id: "call-1".into(),
                    name: "fs_read".into(),
                    arguments: serde_json::json!({}),
                })],
                usage: Usage::default(),
                stop_reason: "tool_calls".into(),
                model: None,
            }),
            attempts_consumed: 1,
            failed_usage: None,
        },
        AttemptOutcome {
            // The post-tool-dispatch call fails with a CASCADE-CLASS error —
            // still terminal: the ladder is closed.
            result: Err(ModelError::RateLimited {
                retry_after_ms: None,
            }),
            attempts_consumed: 1,
            failed_usage: None,
        },
    ]);
    let b = ScriptedTransport::succeeds(None, Usage::default());
    let b_calls = b.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            ),
            (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
        ],
        sink,
    );
    block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("first call commits");
    let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect_err("post-commit failure surfaces on the latched candidate");
    assert!(matches!(err, ModelError::RateLimited { .. }));
    assert!(
        b_calls.lock().unwrap().is_empty(),
        "the ladder never reopens after commit"
    );
}

// ── §8.1 global three-attempt bound ─────────────────────────────────────

#[test]
fn global_three_attempt_bound_with_in_candidate_retry_accounting() {
    // §8.1/§8.2 leg 4: a candidate that transport-retries once then fails
    // (2 physical attempts) leaves exactly ONE further candidate attempt —
    // retry + 2 candidates = 3, never 4.
    let a = ScriptedTransport::fails(
        ModelError::RateLimited {
            retry_after_ms: None,
        },
        2, // the retained in-candidate retry, counted against the budget
        None,
    );
    let b = ScriptedTransport::fails(
        ModelError::Server {
            status: 503,
            message: "overloaded".into(),
        },
        1,
        None,
    );
    let c = ScriptedTransport::succeeds(None, Usage::default());
    let c_calls = c.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            ),
            (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
            (candidate(2, "openai", "gpt-b", CandidateKind::Leaf), c),
        ],
        sink.clone(),
    );
    let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect_err("budget exhausted");
    assert!(matches!(err, ModelError::Server { status: 503, .. }));
    assert!(
        c_calls.lock().unwrap().is_empty(),
        "the third candidate is never attempted (2 + 1 = 3)"
    );
    // The final receipt carries the exhaustion outcome.
    let ops = sink.ops();
    let exhaustion = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt {
            ordinal: 1,
            exhaustion,
            attempts_consumed,
            ..
        } => Some((*exhaustion, *attempts_consumed)),
        _ => None,
    });
    assert_eq!(
        exhaustion,
        Some((Some(nano_session::RoutingExhaustion::BudgetExhausted), 1))
    );
    // Attempt accounting on the first receipt: 2 consumed.
    let first = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt {
            ordinal: 0,
            attempts_consumed,
            ..
        } => Some(*attempts_consumed),
        _ => None,
    });
    assert_eq!(first, Some(2));
}

#[test]
fn cancellation_is_terminal_and_pre_dispatch_cancel_dispatches_nothing() {
    // Pre-dispatch cancel: the loop-top check fires before rung 1 — nothing
    // is dispatched, nothing journaled beyond the snapshot.
    {
        let a = ScriptedTransport::succeeds(None, Usage::default());
        let a_calls = a.calls.clone();
        let ladder = ladder_with(
            vec![(
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            )],
            Arc::new(CollectSink::default()),
        );
        let flag = std::sync::atomic::AtomicBool::new(true);
        let hooks = CallHooks {
            cancel: Some(&flag),
            observer: None,
        };
        let err = block_on(ladder.complete_observed(&text_request(), &hooks))
            .expect_err("cancel is terminal");
        assert!(matches!(err, ModelError::Cancelled));
        assert!(a_calls.lock().unwrap().is_empty());
    }
    // Mid-ladder cancel: rung 1 fails cascading AND fires the cancel flag
    // inside its attempt — the ladder stops before rung 2.
    {
        #[derive(Debug)]
        struct CancelArmingTransport {
            inner: ScriptedTransport,
            flag: Arc<std::sync::atomic::AtomicBool>,
        }
        impl CandidateTransport for CancelArmingTransport {
            async fn attempt(
                &self,
                request: &ModelRequest,
                hooks: &CallHooks<'_>,
            ) -> AttemptOutcome {
                self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
                self.inner.attempt(request, hooks).await
            }
        }
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let a = CancelArmingTransport {
            inner: ScriptedTransport::fails(
                ModelError::Server {
                    status: 500,
                    message: String::new(),
                },
                1,
                None,
            ),
            flag: flag.clone(),
        };
        let b = ScriptedTransport::succeeds(None, Usage::default());
        let b_calls = b.calls.clone();
        let sink = Arc::new(CollectSink::default());
        let ladder = Ladder::new(
            "s-turn-cancel",
            RoutingMode::AutoClientSide,
            "flux-auto",
            vec![
                LadderCandidate {
                    plan: candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                    transport: a,
                },
                LadderCandidate {
                    plan: candidate(1, "openai", "gpt-a", CandidateKind::Leaf),
                    transport: CancelArmingTransport {
                        inner: b,
                        flag: flag.clone(),
                    },
                },
            ],
            sink,
            None,
            false,
            auto_routing::ATTEMPT_BUDGET,
            0,
        );
        let err = block_on(ladder.complete_observed(
            &text_request(),
            &CallHooks {
                cancel: Some(flag.as_ref()),
                observer: None,
            },
        ))
        .expect_err("cancel mid-ladder is terminal");
        assert!(matches!(err, ModelError::Cancelled));
        assert!(
            b_calls.lock().unwrap().is_empty(),
            "no cascade after cancel"
        );
    }
}

#[test]
fn no_duplicate_candidate_and_candidates_exhausted() {
    // Every candidate fails cascading: each is attempted at most once, in
    // order, and the last receipt reports candidates_exhausted.
    let fail = || {
        ScriptedTransport::fails(
            ModelError::Server {
                status: 503,
                message: String::new(),
            },
            1,
            None,
        )
    };
    let a = fail();
    let a_calls = a.calls.clone();
    let b = fail();
    let c = fail();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            ),
            (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
            (candidate(2, "openai", "gpt-b", CandidateKind::Leaf), c),
        ],
        sink.clone(),
    );
    assert!(block_on(ladder.complete_observed(&text_request(), &CallHooks::none())).is_err());
    assert_eq!(a_calls.lock().unwrap().len(), 1, "at most one attempt each");
    let ops = sink.ops();
    assert_eq!(begins(&ops), [0, 1, 2]);
    let last = ops.iter().filter_map(|op| match op {
        Op::RoutingReceipt {
            ordinal: 2,
            exhaustion,
            ..
        } => *exhaustion,
        _ => None,
    });
    assert_eq!(
        last.collect::<Vec<_>>(),
        [nano_session::RoutingExhaustion::CandidatesExhausted]
    );
}

// ── §8.1 journal fail-closure ───────────────────────────────────────────

#[test]
fn unjournaled_attempt_never_dispatches() {
    // Journal-first: a failed AttemptBegin append fails the turn CLOSED —
    // the candidate's transport is never invoked.
    let a = ScriptedTransport::succeeds(None, Usage::default());
    let a_calls = a.calls.clone();
    let ladder = ladder_with(
        vec![(
            candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
            a,
        )],
        Arc::new(FailingSink),
    );
    let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect_err("journal failure is terminal");
    assert!(matches!(err, ModelError::Protocol(_)));
    assert!(
        a_calls.lock().unwrap().is_empty(),
        "no dispatch behind an unjournaled begin"
    );
}

// ── §8.1 kill-resume (§4.1) — real journal, simulated process kill ──────
// The kill is simulated at the journal boundary (the c2_kill_mid_edit
// precedent): the journal is the durable oracle — a fresh reader/coordinator
// on the same file plays the resumed process.

/// Writes the pre-kill journal state for an interrupted auto turn:
/// SessionBegin, TurnBegin, RoutingSnapshot, then the given partial ladder
/// ops (begins/receipts).
struct KillFixture {
    // Kept for Drop (the tempdir lives until the fixture drops).
    _dir: tempfile::TempDir,
    journal_path: std::path::PathBuf,
    turn_id: String,
}

fn kill_fixture(boundary: &str) -> KillFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal_path = dir.path().join("sessions").join("s.jsonl");
    let coordinator = JournalCoordinator::open(&journal_path).expect("open journal");
    let turn_id = "s-turn-1".to_string();
    coordinator
        .append(&nano_session::OpEnvelope::new(
            "s-begin-1",
            "now",
            Op::SessionBegin {
                session_id: "s".into(),
                cwd: ".".into(),
            },
        ))
        .expect("session begin");
    coordinator
        .append(&nano_session::OpEnvelope::new(
            format!("{turn_id}-1"),
            "now",
            Op::TurnBegin {
                turn_id: turn_id.clone(),
                input: "resume me".into(),
                input_blocks: Vec::new(),
            },
        ))
        .expect("turn begin");
    let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
    let candidates = vec![
        nano_session::RoutingCandidate {
            provider: "flux-router".into(),
            candidate: "flux-auto".into(),
            kind: CandidateKind::Alias,
            admitted: true,
            rejection: None,
        },
        nano_session::RoutingCandidate {
            provider: "openai".into(),
            candidate: "gpt-a".into(),
            kind: CandidateKind::Leaf,
            admitted: true,
            rejection: None,
        },
        nano_session::RoutingCandidate {
            provider: "openai".into(),
            candidate: "gpt-b".into(),
            kind: CandidateKind::Leaf,
            admitted: true,
            rejection: None,
        },
    ];
    assert!(auto_routing::journal_snapshot(
        &sink,
        &turn_id,
        RoutingMode::AutoClientSide,
        "flux-auto",
        auto_routing::ATTEMPT_BUDGET,
        candidates,
        "ab".repeat(32),
    ));
    let begin = |sink: &auto_routing::CoordinatorRoutingSink, ordinal: u32, candidate: &str| {
        assert!(RoutingSink::append(
            sink,
            &nano_session::OpEnvelope::new(
                format!("{turn_id}-routing-begin-{ordinal}"),
                "now",
                Op::RoutingAttemptBegin {
                    turn_id: turn_id.clone(),
                    ordinal,
                    routing_mode: RoutingMode::AutoClientSide,
                    provider: if ordinal == 0 {
                        "flux-router"
                    } else {
                        "openai"
                    }
                    .into(),
                    candidate: candidate.into(),
                },
            ),
        ));
    };
    let cascade_receipt =
        |sink: &auto_routing::CoordinatorRoutingSink, ordinal: u32, candidate: &str| {
            assert!(RoutingSink::append(
                sink,
                &nano_session::OpEnvelope::new(
                    format!("{turn_id}-routing-receipt-{ordinal}"),
                    "now",
                    Op::RoutingReceipt {
                        turn_id: turn_id.clone(),
                        ordinal,
                        routing_mode: RoutingMode::AutoClientSide,
                        provider: "flux-router".into(),
                        configured_reference: "flux-auto".into(),
                        candidate: candidate.into(),
                        outcome: RoutingOutcome::CascadeFailure,
                        failure: Some(nano_session::RoutingFailureClass::RateLimited),
                        status: Some(429),
                        attempts_consumed: 1,
                        selected: false,
                        response_model: None,
                        leaf_identity: LeafProvenance::Absent,
                        usage: None,
                        exhaustion: None,
                        rejection: None,
                    },
                ),
            ));
        };
    match boundary {
        // Kill before the first dispatch: snapshot only.
        "before_dispatch" => {}
        // Kill mid-attempt: begin 0 without a receipt (in-flight).
        "mid_attempt" => begin(&sink, 0, "flux-auto"),
        // Kill between rungs: receipt 0 journaled, begin 1 in flight.
        "between_rungs" => {
            cascade_receipt(&sink, 0, "flux-auto");
            begin(&sink, 1, "gpt-a");
        }
        other => panic!("unknown boundary {other}"),
    }
    // The "process" dies here: drop the coordinator without a TurnEnd.
    KillFixture {
        _dir: dir,
        journal_path,
        turn_id,
    }
}

/// The resume half: fresh reader + coordinator on the journaled state.
fn resume_fixture(fixture: &KillFixture) -> (Vec<nano_session::OpEnvelope>, SessionState) {
    let report = nano_session::read_journal(&fixture.journal_path).expect("journal reads");
    let folded = SessionState::fold(&report.envelopes);
    (report.envelopes, folded)
}

#[test]
fn kill_before_first_dispatch_resumes_with_full_budget() {
    let fixture = kill_fixture("before_dispatch");
    let (_envelopes, folded) = resume_fixture(&fixture);
    assert!(folded.turn_interrupted);
    let open = folded.open_turn.as_ref().expect("interrupted turn");
    let routing = folded.routing.get(&open.turn_id).expect("routing state");
    // Nothing begun: nothing to reconcile; full budget remains.
    assert!(routing.stranded_ordinals().is_empty());
    let resume = auto_routing::plan_resume(routing).expect("resumable");
    assert_eq!(resume.budget, auto_routing::ATTEMPT_BUDGET);
    assert_eq!(
        resume.remaining.len(),
        3,
        "all candidates replay from the journal"
    );
    assert!(resume.exhaustion.is_none());
}

#[test]
fn kill_mid_attempt_consumes_the_in_flight_attempt_and_charges_estimate() {
    let fixture = kill_fixture("mid_attempt");
    let (_envelopes, folded) = resume_fixture(&fixture);
    let open = folded.open_turn.as_ref().expect("interrupted");
    let routing = folded.routing.get(&open.turn_id).expect("routing");
    assert_eq!(routing.stranded_ordinals(), vec![0]);
    // Reconcile (the session/load path): the in-flight attempt is consumed
    // and charged the §3.5 estimate — reported: false, never zero.
    let coordinator = JournalCoordinator::open(&fixture.journal_path).expect("reopen");
    let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
    let journaled = auto_routing::reconcile_interrupted(&sink, &open.turn_id, routing, &open.input)
        .expect("reconcile");
    assert_eq!(journaled, 1);
    // Re-read: the ConsumedInflight receipt is durable and folded into the
    // session usage totals (a killed attempt is never free).
    let report = nano_session::read_journal(&fixture.journal_path).expect("re-read");
    let receipt = report.envelopes.iter().find_map(|e| match &e.op {
        Op::RoutingReceipt {
            ordinal: 0,
            outcome,
            usage,
            ..
        } => Some((*outcome, *usage)),
        _ => None,
    });
    let (outcome, usage) = receipt.expect("consumed-inflight receipt");
    assert_eq!(outcome, RoutingOutcome::ConsumedInflight);
    let usage = usage.expect("estimate usage");
    assert!(!usage.reported, "§3.5 estimate provenance");
    assert!(
        usage.input_tokens >= 1 && usage.output_tokens >= 1,
        "never zero"
    );
    let folded = SessionState::fold(&report.envelopes);
    assert!(
        folded.session_usage.total_tokens() > 0,
        "the estimate folds into session totals"
    );
    assert_eq!(
        folded.session_usage.usage_source,
        nano_session::UsageSource::Estimated
    );
    // Reconciliation is idempotent (envelope-id dedup): a second pass is a no-op.
    let routing = folded.routing.get(&open.turn_id).expect("routing");
    assert!(routing.stranded_ordinals().is_empty());
    let coordinator = JournalCoordinator::open(&fixture.journal_path).expect("reopen");
    let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
    assert_eq!(
        auto_routing::reconcile_interrupted(&sink, &open.turn_id, routing, &open.input)
            .expect("reconcile"),
        0
    );
    // Budget: 3 - 1 consumed = 2; resume continues at the NEXT admitted
    // candidate (ordinal 1), never replaying ordinal 0.
    let resume = auto_routing::plan_resume(routing).expect("resumable");
    assert_eq!(resume.budget, 2);
    assert_eq!(resume.remaining.len(), 2);
    assert_eq!(resume.remaining[0].candidate, "gpt-a");
    assert_eq!(resume.remaining[0].ordinal, 1);
}

#[test]
fn kill_between_rungs_preserves_remaining_budget_and_order() {
    let fixture = kill_fixture("between_rungs");
    let (_envelopes, folded) = resume_fixture(&fixture);
    let open = folded.open_turn.as_ref().expect("interrupted");
    let routing = folded.routing.get(&open.turn_id).expect("routing");
    // ordinal 0 receipted (1 attempt), ordinal 1 stranded (1 attempt).
    let coordinator = JournalCoordinator::open(&fixture.journal_path).expect("reopen");
    let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
    auto_routing::reconcile_interrupted(&sink, &open.turn_id, routing, &open.input)
        .expect("reconcile");
    let report = nano_session::read_journal(&fixture.journal_path).expect("re-read");
    let folded = SessionState::fold(&report.envelopes);
    let routing = folded.routing.get(&open.turn_id).expect("routing");
    let resume = auto_routing::plan_resume(routing).expect("resumable");
    assert_eq!(resume.budget, 1, "3 - 1 receipted - 1 in-flight");
    assert_eq!(resume.remaining.len(), 1);
    assert_eq!(resume.remaining[0].candidate, "gpt-b");
    assert_eq!(resume.remaining[0].ordinal, 2);
    // The resumed ladder runs ONLY the remaining journaled candidate against
    // the remaining budget — resume replays, never rediscovers. (No router,
    // no catalog, no discovery inputs exist in this test at all.)
    let transport = ScriptedTransport::succeeds(Some("gpt-b"), Usage::default());
    let calls = transport.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = Ladder::new(
        "s-turn-1-resume",
        RoutingMode::AutoClientSide,
        &resume.configured_reference,
        vec![LadderCandidate {
            plan: resume.remaining[0].clone(),
            transport,
        }],
        sink,
        None,
        false,
        resume.budget,
        0,
    );
    let response = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("resumed rung succeeds");
    assert_eq!(response.model.as_deref(), Some("gpt-b"));
    assert_eq!(calls.lock().unwrap()[0].model, "gpt-b");
}

// ── F-P5-1 / F-P5-2 adversarial regression legs [seam] ──────────────────

#[test]
fn adv_500_with_format_body_must_be_terminal() {
    // F-P5-1: a 500 whose body carries error.type=="invalid_request_error"
    // is a FORMAT rejection — terminal, zero calls to rung 2. The failing
    // error is the REAL ModelError the production adapter's classify_status
    // produces on that wire shape (never a synthetic signal map).
    let err = nano_model::flux_common::classify_status(
        500,
        "{\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad tools payload\"}}"
            .to_string(),
    );
    let a = ScriptedTransport::fails(err, 1, None);
    let b = ScriptedTransport::succeeds(Some("gpt-a"), Usage::default());
    let b_calls = b.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            ),
            (candidate(1, "openai", "gpt-a", CandidateKind::Leaf), b),
        ],
        sink.clone(),
    );
    let outcome = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()));
    assert!(
        matches!(outcome, Err(ModelError::InvalidRequest { status: 500, .. })),
        "the terminal typed error surfaces unchanged: {outcome:?}"
    );
    assert!(
        b_calls.lock().unwrap().is_empty(),
        "terminal: zero rung-2 calls"
    );
    let ops = sink.ops();
    let receipt = ops
        .iter()
        .find_map(|op| match op {
            Op::RoutingReceipt {
                ordinal: 0,
                outcome,
                failure,
                status,
                ..
            } => Some((*outcome, *failure, *status)),
            _ => None,
        })
        .expect("rung-1 receipt journaled");
    assert_eq!(
        receipt,
        (
            RoutingOutcome::TerminalFailure,
            Some(nano_session::RoutingFailureClass::FormatRejected),
            Some(500)
        ),
        "journaled as terminal format rejection with the wire status"
    );
}

#[test]
fn adv_double_kill_budget_leak() {
    // F-P5-2: chained kill-resume must conserve the global 3-attempt budget.
    // Chain: turn 1 is killed mid-attempt (1 consumed) → the resume arm
    // re-journals turn 2 with the TRUE remainder (2), spends one cascading
    // attempt, and is killed again → turn 3 may spend AT MOST 1. Total
    // physical attempts across one logical routed turn: 3, never 4.
    let dir = tempfile::tempdir().expect("tempdir");
    let journal_path = dir.path().join("sessions").join("s.jsonl");
    let candidates = vec![
        nano_session::RoutingCandidate {
            provider: "flux-router".into(),
            candidate: "flux-auto".into(),
            kind: CandidateKind::Alias,
            admitted: true,
            rejection: None,
        },
        nano_session::RoutingCandidate {
            provider: "openai".into(),
            candidate: "gpt-a".into(),
            kind: CandidateKind::Leaf,
            admitted: true,
            rejection: None,
        },
        nano_session::RoutingCandidate {
            provider: "openai".into(),
            candidate: "gpt-b".into(),
            kind: CandidateKind::Leaf,
            admitted: true,
            rejection: None,
        },
    ];
    let digest = "ab".repeat(32);
    // ── Turn 1: snapshot(budget 3), begin 0, killed mid-attempt.
    {
        let coordinator = JournalCoordinator::open(&journal_path).expect("open journal");
        coordinator
            .append(&nano_session::OpEnvelope::new(
                "s-begin-1",
                "now",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: ".".into(),
                },
            ))
            .expect("session begin");
        coordinator
            .append(&nano_session::OpEnvelope::new(
                "s-turn-1-1",
                "now",
                Op::TurnBegin {
                    turn_id: "s-turn-1".into(),
                    input: "resume me".into(),
                    input_blocks: Vec::new(),
                },
            ))
            .expect("turn begin");
        let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
        assert!(auto_routing::journal_snapshot(
            &sink,
            "s-turn-1",
            RoutingMode::AutoClientSide,
            "flux-auto",
            auto_routing::ATTEMPT_BUDGET,
            candidates.clone(),
            digest.clone(),
        ));
        assert!(RoutingSink::append(
            &sink,
            &nano_session::OpEnvelope::new(
                "s-turn-1-routing-begin-0",
                "now",
                Op::RoutingAttemptBegin {
                    turn_id: "s-turn-1".into(),
                    ordinal: 0,
                    routing_mode: RoutingMode::AutoClientSide,
                    provider: "flux-router".into(),
                    candidate: "flux-auto".into(),
                },
            ),
        ));
        // The "process" dies here: dropped without a receipt for ordinal 0.
    }
    // ── Resume #1 (the ACP resume arm): reconcile the stranded attempt,
    //    plan from the journal, re-journal turn 2 with the TRUE remainder,
    //    spend one cascading attempt, killed again.
    {
        let report = nano_session::read_journal(&journal_path).expect("read");
        let folded = SessionState::fold(&report.envelopes);
        let routing = folded.routing.get("s-turn-1").expect("turn-1 routing");
        let coordinator = JournalCoordinator::open(&journal_path).expect("reopen");
        let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
        auto_routing::reconcile_interrupted(&sink, "s-turn-1", routing, "resume me")
            .expect("reconcile");
        let report = nano_session::read_journal(&journal_path).expect("re-read");
        let folded = SessionState::fold(&report.envelopes);
        let routing = folded.routing.get("s-turn-1").expect("turn-1 routing");
        let resume = auto_routing::plan_resume(routing).expect("resumable");
        assert_eq!(resume.budget, 2, "3 - 1 consumed in-flight");
        assert_eq!(resume.remaining.len(), 2);
        // The resume arm re-journals through journal_snapshot — the F-P5-2
        // fix passes the remainder instead of hardcoding ATTEMPT_BUDGET.
        let coordinator = JournalCoordinator::open(&journal_path).expect("reopen");
        let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
        assert!(RoutingSink::append(
            &sink,
            &nano_session::OpEnvelope::new(
                "s-turn-2-1",
                "now",
                Op::TurnBegin {
                    turn_id: "s-turn-2".into(),
                    input: "resume me".into(),
                    input_blocks: Vec::new(),
                },
            ),
        ));
        assert!(auto_routing::journal_snapshot(
            &sink,
            "s-turn-2",
            RoutingMode::AutoClientSide,
            &resume.configured_reference,
            resume.budget,
            resume.snapshot_candidates.clone(),
            resume.catalog_digest.clone(),
        ));
        // Turn 2 spends ONE cascading attempt on ordinal 1, then is killed.
        assert!(RoutingSink::append(
            &sink,
            &nano_session::OpEnvelope::new(
                "s-turn-2-routing-begin-1",
                "now",
                Op::RoutingAttemptBegin {
                    turn_id: "s-turn-2".into(),
                    ordinal: 1,
                    routing_mode: RoutingMode::AutoClientSide,
                    provider: "openai".into(),
                    candidate: "gpt-a".into(),
                },
            ),
        ));
        assert!(RoutingSink::append(
            &sink,
            &nano_session::OpEnvelope::new(
                "s-turn-2-routing-receipt-1",
                "now",
                Op::RoutingReceipt {
                    turn_id: "s-turn-2".into(),
                    ordinal: 1,
                    routing_mode: RoutingMode::AutoClientSide,
                    provider: "openai".into(),
                    configured_reference: "flux-auto".into(),
                    candidate: "gpt-a".into(),
                    outcome: RoutingOutcome::CascadeFailure,
                    failure: Some(nano_session::RoutingFailureClass::RateLimited),
                    status: Some(429),
                    attempts_consumed: 1,
                    selected: false,
                    response_model: None,
                    leaf_identity: LeafProvenance::Absent,
                    usage: None,
                    exhaustion: None,
                    rejection: None,
                },
            ),
        ));
    }
    // The journaled turn-2 snapshot carries the remainder, NOT a fresh 3 —
    // this is the crux of the leak (pre-fix it re-journaled 3).
    {
        let report = nano_session::read_journal(&journal_path).expect("read");
        let journaled_budget = report.envelopes.iter().find_map(|e| match &e.op {
            Op::RoutingSnapshot {
                turn_id,
                attempt_budget,
                ..
            } if turn_id == "s-turn-2" => Some(*attempt_budget),
            _ => None,
        });
        assert_eq!(
            journaled_budget,
            Some(2),
            "the resume arm journals the true remainder"
        );
    }
    // ── Resume #2: budget = journaled remainder (2) - consumed (1) = 1.
    let resume2 = {
        let report = nano_session::read_journal(&journal_path).expect("read");
        let folded = SessionState::fold(&report.envelopes);
        let routing = folded.routing.get("s-turn-2").expect("turn-2 routing");
        auto_routing::plan_resume(routing).expect("resumable")
    };
    assert_eq!(resume2.budget, 1, "the chain conserves the global budget");
    assert_eq!(resume2.remaining.len(), 1);
    assert_eq!(resume2.remaining[0].candidate, "gpt-b");
    // The resumed ladder spends AT MOST that one attempt (the transport has
    // two scripted steps: a second dispatch would also fail the call count).
    let transport = ScriptedTransport::fails(
        ModelError::Server {
            status: 503,
            message: String::new(),
        },
        1,
        None,
    );
    transport.steps.lock().unwrap().push_back(AttemptOutcome {
        result: Err(ModelError::Server {
            status: 503,
            message: String::new(),
        }),
        attempts_consumed: 1,
        failed_usage: None,
    });
    let calls = transport.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = Ladder::new(
        "s-turn-3",
        RoutingMode::AutoClientSide,
        &resume2.configured_reference,
        vec![LadderCandidate {
            plan: resume2.remaining[0].clone(),
            transport,
        }],
        sink,
        None,
        false,
        resume2.budget,
        0,
    );
    let outcome = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()));
    assert!(outcome.is_err(), "the last candidate fails the turn");
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "global budget conserved: 1 + 1 + 1 = 3 physical attempts across the chain"
    );
}

// ── §8.1 usage rollup + receipt completeness + canary ───────────────────

#[test]
fn failed_and_successful_attempt_usage_rolls_up_with_provenance() {
    // §8.1/§8.2 leg 8 [seam]: a metered FAILED attempt (provider-reported
    // usage) then a successful attempt naming a leaf — per-attempt records
    // with distinct provenance, priced against the actual leaf only.
    let pricing = nano_model::pricing::PricingCatalog::from_toml_str(
        "[openai.gpt-a]\ninput_per_mtok_usd = 1.0\noutput_per_mtok_usd = 2.0\n",
    )
    .expect("pricing parses");
    let a = ScriptedTransport::fails(
        ModelError::Server {
            status: 503,
            message: String::new(),
        },
        1,
        Some(Usage {
            input_tokens: 100,
            output_tokens: 0,
            ..Usage::default()
        }),
    );
    let b = ScriptedTransport::succeeds(
        Some("gpt-a"),
        Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Usage::default()
        },
    );
    let sink = Arc::new(CollectSink::default());
    let ladder = Ladder::new(
        "s-turn-meter",
        RoutingMode::AutoClientSide,
        "flux-auto",
        vec![
            LadderCandidate {
                plan: candidate(0, "openai", "gpt-a", CandidateKind::Leaf),
                transport: a,
            },
            LadderCandidate {
                plan: candidate(1, "openai", "gpt-a", CandidateKind::Leaf),
                transport: b,
            },
        ],
        sink.clone(),
        Some(Arc::new(pricing)),
        false,
        auto_routing::ATTEMPT_BUDGET,
        0,
    );
    block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("rung 2 succeeds");
    let ops = sink.ops();
    // Failed rung: provider-reported usage retained (never free), unpriced
    // (no actual-leaf signal from a failed attempt).
    let failed = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt {
            ordinal: 0, usage, ..
        } => *usage,
        _ => None,
    });
    let failed = failed.expect("failed rung carries usage");
    assert!(failed.reported);
    assert_eq!(failed.input_tokens, 100);
    assert!(
        !failed.priced && failed.microcents == 0,
        "unpriced, never fake $0"
    );
    // Successful rung: actual-leaf pricing against (openai, gpt-a).
    let success = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt {
            ordinal: 1,
            usage,
            leaf_identity,
            ..
        } => Some((*usage, *leaf_identity)),
        _ => None,
    });
    let (usage, identity) = success.expect("success receipt");
    let usage = usage.expect("success usage");
    assert_eq!(identity, LeafProvenance::ProviderReported);
    assert!(usage.priced && usage.microcents > 0, "actual-leaf pricing");
}

/// F-18: a provider 404 arrives from the adapter as the TYPED
/// ModelError::ModelNotFound — the ladder journals failure model_not_found
/// (status 404) and closes TERMINAL (§4: a stale snapshot fails closed,
/// never cascades); the kind-keyed record is what fallback/retirement
/// logic consumes.
#[test]
fn provider_404_journals_model_not_found_and_closes_terminal() {
    let a = ScriptedTransport::fails(
        nano_model::flux_common::classify_status(
            404,
            r#"{"error":{"message":"model retired upstream"}}"#.to_string(),
        ),
        1,
        None,
    );
    let a_calls = a.calls.clone();
    let b = ScriptedTransport::succeeds(Some("gpt-b"), Usage::default());
    let b_calls = b.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (candidate(0, "openai", "gpt-a", CandidateKind::Leaf), a),
            (candidate(1, "openai", "gpt-b", CandidateKind::Leaf), b),
        ],
        sink.clone(),
    );
    let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect_err("the 404 fails the turn");
    assert!(
        matches!(err, ModelError::ModelNotFound { status: 404, .. }),
        "the surfaced error is the typed model-not-found: {err:?}"
    );
    assert_eq!(a_calls.lock().unwrap().len(), 1);
    assert!(
        b_calls.lock().unwrap().is_empty(),
        "terminal class: the next candidate is never dispatched"
    );
    let ops = sink.ops();
    let receipt = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt {
            outcome,
            failure,
            status,
            ..
        } => Some((*outcome, *failure, *status)),
        _ => None,
    });
    assert_eq!(
        receipt,
        Some((
            RoutingOutcome::TerminalFailure,
            Some(nano_session::RoutingFailureClass::ModelNotFound),
            Some(404)
        )),
        "the journaled receipt carries the model_not_found kind"
    );
}

#[test]
fn receipt_completeness_and_secret_canary() {
    // §8.1: every receipt carries the mandated fields; no credential ever
    // lands in the journal. The canary string rides the (scripted) error
    // message — receipts journal the CLASS and status, never the message.
    let canary = "sk-p5-canary-never-journaled";
    let a = ScriptedTransport::fails(
        ModelError::Auth {
            message: format!("auth failed for {canary}"),
            status: Some(401),
        },
        1,
        None,
    );
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![(
            candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
            a,
        )],
        sink.clone(),
    );
    assert!(block_on(ladder.complete_observed(&text_request(), &CallHooks::none())).is_err());
    let ops = sink.ops();
    // Completeness: begin + receipt with turn id, ordinal, routing_mode,
    // provider, configured reference, candidate, classified failure, status,
    // exhaustion-free terminal outcome.
    let begin = ops.iter().find_map(|op| match op {
        Op::RoutingAttemptBegin {
            turn_id,
            ordinal,
            routing_mode,
            provider,
            candidate,
        } => Some((
            turn_id.clone(),
            *ordinal,
            *routing_mode,
            provider.clone(),
            candidate.clone(),
        )),
        _ => None,
    });
    let (turn_id, ordinal, mode, provider, cand) = begin.expect("begin");
    assert_eq!((ordinal, mode), (0, RoutingMode::AutoClientSide));
    assert_eq!(
        (provider.as_str(), cand.as_str()),
        ("flux-router", "flux-auto")
    );
    let receipt = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt { turn_id: t, .. } if *t == turn_id => Some(op.clone()),
        _ => None,
    });
    match receipt.expect("receipt") {
        Op::RoutingReceipt {
            routing_mode,
            configured_reference,
            outcome,
            failure,
            status,
            attempts_consumed,
            selected,
            leaf_identity,
            ..
        } => {
            assert_eq!(routing_mode, RoutingMode::AutoClientSide);
            assert_eq!(configured_reference, "flux-auto");
            assert_eq!(outcome, RoutingOutcome::TerminalFailure);
            assert_eq!(failure, Some(nano_session::RoutingFailureClass::Auth));
            assert_eq!(status, Some(401));
            assert_eq!(attempts_consumed, 1);
            assert!(!selected);
            assert_eq!(leaf_identity, LeafProvenance::Absent);
        }
        other => panic!("expected receipt, got {other:?}"),
    }
    // Canary: serialize the whole op stream — the credential is absent.
    let serialized = serde_json::to_string(&ops).expect("serialize");
    assert!(
        !serialized.contains(canary),
        "no credential in any journal op"
    );
}

// ── §8.2 [seam] loopback wire legs (legs 3/4/5 over real HTTP) ──────────
// The deterministic fault-injection harness: loopback MockServers plus the
// production client stack (egress loopback policy + with_base_url +
// single-attempt retry) — synthetic endpoints are NOT live proof; only the
// FLUX_TEST_KEY-gated legs prove wire behavior.

/// A one-shot loopback HTTP server: answers each accepted connection with
/// the scripted (status, body), recording raw request bytes.
struct MockServer {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    /// `responses`: one per expected connection. `close_without_answer`
    /// simulates connection loss after accept. The accept loop is
    /// nonblocking with a shutdown flag: a candidate the ladder NEVER
    /// contacts (the point of several legs) must not hang `finish`.
    fn start(responses: Vec<MockResponse>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink = requests.clone();
        let stop = shutdown.clone();
        let join = std::thread::spawn(move || {
            for response in responses {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                // Poll accept until a connection arrives or shutdown lands.
                let mut stream = loop {
                    if stop.load(std::sync::atomic::Ordering::SeqCst) {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => return,
                    }
                };
                // BSD/macOS: accept()ed sockets INHERIT the listener's
                // O_NONBLOCK (unlike Linux) — force the stream blocking
                // before the read loop, or WouldBlock tears the request
                // read and the client sees a transport reset (CI-proven on
                // macos-15-intel).
                stream
                    .set_nonblocking(false)
                    .expect("accepted stream blocking");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .ok();
                // Read the request head + body (content-length).
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let mut head_done = false;
                let mut content_length = 0usize;
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if !head_done
                                && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                head_done = true;
                                let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                                for line in head.lines() {
                                    if let Some(value) =
                                        line.to_ascii_lowercase().strip_prefix("content-length:")
                                    {
                                        content_length = value.trim().parse().unwrap_or(0);
                                    }
                                }
                            }
                            if head_done {
                                let head_end =
                                    buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                                if buf.len() >= head_end + content_length {
                                    break;
                                }
                            }
                        }
                    }
                }
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                match response {
                    MockResponse::Close => {
                        drop(stream); // connection loss before any response byte
                    }
                    MockResponse::Answer { status, body } => {
                        let bytes = format!(
                            "HTTP/1.1 {status} X\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        use std::io::Write as _;
                        let _ = stream.write_all(bytes.as_bytes());
                        let _ = stream.flush();
                    }
                    MockResponse::PartialThenClose { head_bytes } => {
                        // A truncated success frame: claims a longer body
                        // than it delivers, then drops mid-stream.
                        use std::io::Write as _;
                        let bytes = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: application/json\r\n\r\n{}",
                            head_bytes.len() + 1024,
                            head_bytes
                        );
                        let _ = stream.write_all(bytes.as_bytes());
                        let _ = stream.flush();
                        drop(stream);
                    }
                }
            }
        });
        Self {
            port,
            requests,
            shutdown,
            join: Some(join),
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn bodies(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|raw| {
                raw.split_once("\r\n\r\n")
                    .map(|(_, body)| body.to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn finish(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

enum MockResponse {
    Answer { status: u16, body: String },
    Close,
    PartialThenClose { head_bytes: String },
}

/// A ladder candidate riding the PRODUCTION client stack against a loopback
/// mock (single-attempt posture — every physical attempt is visible).
fn loopback_candidate(
    ordinal: u32,
    provider: &str,
    leaf: &str,
    server: &MockServer,
) -> LadderCandidate<auto_routing::DriverTransport<nano_agent::wiring::ProviderDriver>> {
    let client = nano_model::flux_completions::FluxCompletionsClient::new(
        nano_egress::client::EgressClient::new(
            nano_egress::policy::EgressPolicy::new().allow_host_with_http("127.0.0.1"),
        ),
    )
    .with_base_url(server.base_url())
    .with_retry_config(nano_model::retry::RetryConfig::single_attempt());
    let driver = nano_agent::wiring::ProviderDriver::openai(client, "sk-loopback-not-a-key");
    LadderCandidate {
        plan: candidate(ordinal, provider, leaf, CandidateKind::Leaf),
        transport: auto_routing::DriverTransport(driver),
    }
}

fn success_body(model: &str) -> String {
    serde_json::json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ok"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 1}
    })
    .to_string()
}

#[test]
fn loopback_429_then_success_two_attempts_intact_semantics() {
    // §8.2 leg 3 [seam, wire arm]: the FIRST controlled endpoint returns
    // 429, the second succeeds — exactly two attempts, intact request
    // semantics, both receipts.
    let mut server_a = MockServer::start(vec![MockResponse::Answer {
        status: 429,
        body: r#"{"error":{"message":"slow down","type":"rate_limit"}}"#.into(),
    }]);
    let mut server_b = MockServer::start(vec![MockResponse::Answer {
        status: 200,
        body: success_body("gpt-b"),
    }]);
    let sink = Arc::new(CollectSink::default());
    let ladder = Ladder::new(
        "s-turn-loop",
        RoutingMode::AutoClientSide,
        "flux-auto",
        vec![
            loopback_candidate(0, "openai", "gpt-a", &server_a),
            loopback_candidate(1, "openai", "gpt-b", &server_b),
        ],
        sink.clone(),
        None,
        false,
        auto_routing::ATTEMPT_BUDGET,
        0,
    );
    let response = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("rung 2 succeeds");
    assert_eq!(response.model.as_deref(), Some("gpt-b"));
    server_a.finish();
    server_b.finish();
    assert_eq!(server_a.request_count(), 1);
    assert_eq!(server_b.request_count(), 1);
    // Intact request semantics: identical bodies except the wire model id.
    let body_a: serde_json::Value =
        serde_json::from_str(&server_a.bodies()[0]).expect("json body a");
    let body_b: serde_json::Value =
        serde_json::from_str(&server_b.bodies()[0]).expect("json body b");
    assert_eq!(body_a["model"], "gpt-a");
    assert_eq!(body_b["model"], "gpt-b");
    let mut stripped = body_a.clone();
    *stripped.get_mut("model").expect("model") = body_b["model"].clone();
    assert_eq!(stripped, body_b, "intact request semantics over the wire");
    let ops = sink.ops();
    assert_eq!(
        receipts(&ops),
        [
            (0, RoutingOutcome::CascadeFailure),
            (1, RoutingOutcome::Committed)
        ]
    );
    // The 429 receipt carries the classified failure + nonsecret status.
    let failure = ops.iter().find_map(|op| match op {
        Op::RoutingReceipt {
            ordinal: 0,
            failure,
            status,
            ..
        } => Some((*failure, *status)),
        _ => None,
    });
    assert_eq!(
        failure,
        Some((
            Some(nano_session::RoutingFailureClass::RateLimited),
            Some(429)
        ))
    );
}

#[test]
fn loopback_503_and_connection_loss_cascade_within_the_ceiling() {
    // §8.2 leg 4 [seam]: 503 then connection loss then success — three
    // physical attempts total, never a fourth.
    let mut server_a = MockServer::start(vec![MockResponse::Answer {
        status: 503,
        body: "<html>bad gateway</html>".into(),
    }]);
    let mut server_b = MockServer::start(vec![MockResponse::Close]);
    let mut server_c = MockServer::start(vec![MockResponse::Answer {
        status: 200,
        body: success_body("gpt-c"),
    }]);
    let mut server_d = MockServer::start(vec![MockResponse::Answer {
        status: 200,
        body: success_body("gpt-d"),
    }]);
    let sink = Arc::new(CollectSink::default());
    let ladder = Ladder::new(
        "s-turn-loop4",
        RoutingMode::AutoClientSide,
        "flux-auto",
        vec![
            loopback_candidate(0, "openai", "gpt-a", &server_a),
            loopback_candidate(1, "openai", "gpt-b", &server_b),
            loopback_candidate(2, "openai", "gpt-c", &server_c),
            loopback_candidate(3, "openai", "gpt-d", &server_d),
        ],
        sink,
        None,
        false,
        auto_routing::ATTEMPT_BUDGET,
        0,
    );
    let response = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("rung 3 succeeds");
    assert_eq!(response.model.as_deref(), Some("gpt-c"));
    server_a.finish();
    server_b.finish();
    server_c.finish();
    server_d.finish();
    assert_eq!(
        server_d.request_count(),
        0,
        "the three-attempt ceiling holds"
    );
}

#[test]
fn loopback_auth_terminality_zero_calls_downstream() {
    // §8.2 leg 5 [seam, wire arm]: 401/403 on the first rung — zero calls
    // to later candidates, over real HTTP through the production client.
    for status in [401, 403] {
        let mut server_a = MockServer::start(vec![MockResponse::Answer {
            status,
            body: r#"{"error":{"message":"nope","type":"auth_error"}}"#.into(),
        }]);
        let mut server_b = MockServer::start(vec![MockResponse::Answer {
            status: 200,
            body: success_body("gpt-b"),
        }]);
        let ladder = Ladder::new(
            "s-turn-auth",
            RoutingMode::AutoClientSide,
            "flux-auto",
            vec![
                loopback_candidate(0, "openai", "gpt-a", &server_a),
                loopback_candidate(1, "openai", "gpt-b", &server_b),
            ],
            Arc::new(CollectSink::default()),
            None,
            false,
            auto_routing::ATTEMPT_BUDGET,
            0,
        );
        let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
            .expect_err("auth is terminal");
        assert!(matches!(err, ModelError::Auth { .. }), "{status}");
        server_a.finish();
        server_b.finish();
        assert_eq!(
            server_b.request_count(),
            0,
            "{status}: zero downstream calls"
        );
    }
}

#[test]
fn loopback_malformed_success_body_is_terminal() {
    // §8.1: a malformed success body is terminal (never cascaded).
    let mut server_a = MockServer::start(vec![MockResponse::Answer {
        status: 200,
        body: "this is not json".into(),
    }]);
    let mut server_b = MockServer::start(vec![MockResponse::Answer {
        status: 200,
        body: success_body("gpt-b"),
    }]);
    let ladder = Ladder::new(
        "s-turn-bad",
        RoutingMode::AutoClientSide,
        "flux-auto",
        vec![
            loopback_candidate(0, "openai", "gpt-a", &server_a),
            loopback_candidate(1, "openai", "gpt-b", &server_b),
        ],
        Arc::new(CollectSink::default()),
        None,
        false,
        auto_routing::ATTEMPT_BUDGET,
        0,
    );
    let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect_err("malformed success is terminal");
    assert!(matches!(err, ModelError::Protocol(_)), "{err:?}");
    server_a.finish();
    server_b.finish();
    assert_eq!(server_b.request_count(), 0);
}

#[test]
fn loopback_truncated_stream_is_post_commit_terminal() {
    // §8.1 commit boundary over the wire: a truncated body (bytes flowed,
    // then the connection died) is post-commit — terminal.
    let mut server_a = MockServer::start(vec![MockResponse::PartialThenClose {
        head_bytes: r#"{"id":"chatcmpl-1","choices":["#.into(),
    }]);
    let mut server_b = MockServer::start(vec![MockResponse::Answer {
        status: 200,
        body: success_body("gpt-b"),
    }]);
    let ladder = Ladder::new(
        "s-turn-trunc",
        RoutingMode::AutoClientSide,
        "flux-auto",
        vec![
            loopback_candidate(0, "openai", "gpt-a", &server_a),
            loopback_candidate(1, "openai", "gpt-b", &server_b),
        ],
        Arc::new(CollectSink::default()),
        None,
        false,
        auto_routing::ATTEMPT_BUDGET,
        0,
    );
    let err = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect_err("truncated stream is terminal");
    let class = auto_routing::classify_attempt(&auto_routing::signals_of_model_error(&err));
    assert!(
        !class.cascades(),
        "truncated stream resolves terminal, got {class:?} ({err:?})"
    );
    server_a.finish();
    server_b.finish();
    assert_eq!(server_b.request_count(), 0);
}

// ── §8.1 host-level legs: the in-process ACP harness (C8 pattern) ───────

mod acp_harness {
    use super::*;
    use nano_agent::loop_protection::ProgressSignals;
    use std::io::{BufRead, Read, Write};

    pub struct ChannelReader {
        rx: std::sync::mpsc::Receiver<String>,
        buf: Vec<u8>,
        pos: usize,
    }

    impl Read for ChannelReader {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = {
                let avail = self.fill_buf()?;
                let n = avail.len().min(out.len());
                out[..n].copy_from_slice(&avail[..n]);
                n
            };
            self.consume(n);
            Ok(n)
        }
    }

    impl BufRead for ChannelReader {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            while self.pos >= self.buf.len() {
                match self.rx.recv() {
                    Ok(line) => {
                        self.buf = line.into_bytes();
                        self.pos = 0;
                    }
                    Err(_) => return Ok(&[]),
                }
            }
            Ok(&self.buf[self.pos..])
        }

        fn consume(&mut self, amt: usize) {
            self.pos += amt;
        }
    }

    pub struct ChannelWriter {
        tx: std::sync::mpsc::Sender<String>,
        buf: Vec<u8>,
    }

    impl Write for ChannelWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                self.tx
                    .send(String::from_utf8_lossy(&line).into_owned())
                    .map_err(std::io::Error::other)?;
            }
            Ok(())
        }
    }

    /// Records the wire model id of every dispatched turn.
    #[derive(Debug, Clone, Default)]
    pub struct CapturingDriver {
        pub seen_models: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl ModelDriver for CapturingDriver {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.seen_models.lock().unwrap().push(request.model.clone());
            Ok(ModelResponse {
                events: vec![
                    ModelEvent::TextDelta("ok".into()),
                    ModelEvent::Done {
                        stop_reason: "stop".into(),
                    },
                ],
                usage: Usage::default(),
                stop_reason: "stop".into(),
                model: None,
            })
        }
    }

    #[derive(Debug, Clone, Default)]
    pub struct MockTools;

    #[async_trait::async_trait]
    impl ToolExecutor for MockTools {
        async fn execute(&self, call: &ToolCall) -> ToolOutcome {
            ToolOutcome {
                ok: true,
                output: format!("ran {}", call.name),
                progress: ProgressSignals::default(),
                error_kind: None,
            }
        }
    }

    pub struct Host {
        pub sessions_dir: std::path::PathBuf,
        to_host: Option<std::sync::mpsc::Sender<String>>,
        frames: std::sync::mpsc::Receiver<String>,
        handle: Option<std::thread::JoinHandle<std::io::Result<i32>>>,
        next_id: u64,
        pub seen_models: Arc<Mutex<Vec<String>>>,
    }

    impl Host {
        pub fn spawn(
            router: ProviderRouter,
            available: Vec<AvailableModel>,
            default_model: &str,
            routing: &'static auto_routing::RoutingConfig,
        ) -> Self {
            let (in_tx, in_rx) = std::sync::mpsc::channel::<String>();
            let (out_tx, out_rx) = std::sync::mpsc::channel::<String>();
            let sessions_dir = std::env::temp_dir().join(format!(
                "nano-p5-sessions-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&sessions_dir).expect("sessions dir");
            let driver = CapturingDriver::default();
            let seen_models = driver.seen_models.clone();
            let default_model = default_model.to_string();
            let sessions_dir_thread = sessions_dir.clone();
            let handle = std::thread::spawn(move || {
                block_on(async move {
                    let sandbox_probe = || true;
                    let memory_config = acp_mode::MemoryHostConfig {
                        dir: sessions_dir_thread.parent().expect("root").join("memory"),
                        write_enabled: false,
                        block_cap: nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
                    };
                    let vision_catalog = nano_model::vision_catalog::VisionCatalog::vendored()
                        .expect("vendored vision catalog parses");
                    let attachment_home = sessions_dir_thread.parent().expect("root");
                    let config = acp_mode::ServeConfig {
                        sessions_dir: &sessions_dir_thread,
                        default_model: &default_model,
                        available_models: &available,
                        env_mcp_specs: &[],
                        catalog: &[],
                        window_override: None,
                        limit_override: None,
                        sandbox_probe: &sandbox_probe,
                        router: &router,
                        journal_append_failer: None,
                        memory: &memory_config,
                        reasoning_effort: None,
                        verbosity: None,
                        cron_home: None,
                        search: None,
                        search_meter: None,
                        pricing: None,
                        budget_cap: None,
                        vision_catalog: &vision_catalog,
                        attachment_home,
                        routing,
                    };
                    acp_mode::serve(
                        ChannelReader {
                            rx: in_rx,
                            buf: Vec::new(),
                            pos: 0,
                        },
                        ChannelWriter {
                            tx: out_tx,
                            buf: Vec::new(),
                        },
                        &config,
                        move |_| driver.clone(),
                        move |_, _, _, _, _, _| {
                            (
                                MockTools,
                                nano_core::permissions::PermissionProfile::workspace_write()
                                    .file_system_sandbox_policy(),
                            )
                        },
                    )
                    .await
                })
            });
            Self {
                sessions_dir,
                to_host: Some(in_tx),
                frames: out_rx,
                handle: Some(handle),
                next_id: 1,
                seen_models,
            }
        }

        pub fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
            let id = self.next_id;
            self.next_id += 1;
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            });
            self.to_host
                .as_ref()
                .expect("stdin open")
                .send(format!("{}\n", serde_json::to_string(&frame).unwrap()))
                .expect("send to host");
            loop {
                let line = self
                    .frames
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("host frame");
                let value: serde_json::Value = serde_json::from_str(&line).expect("json frame");
                if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                    return value;
                }
            }
        }

        pub fn new_session(&mut self) -> String {
            let response = self.request(
                "session/new",
                serde_json::json!({"cwd": self.sessions_dir.parent().expect("root")}),
            );
            response["result"]["sessionId"]
                .as_str()
                .expect("session id")
                .to_string()
        }

        pub fn prompt(&mut self, session_id: &str, text: &str) -> serde_json::Value {
            self.request(
                "session/prompt",
                serde_json::json!({
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": text}],
                }),
            )
        }

        /// The journaled routing ops of the session's journal.
        pub fn routing_ops(&self, session_id: &str) -> Vec<Op> {
            let journal = self.sessions_dir.join(format!("{session_id}.jsonl"));
            // The writer may hold a buffered tail — read AFTER the turn's
            // terminal frame arrived (journal-first discipline makes the
            // ops durable by then).
            nano_session::read_journal(&journal)
                .expect("journal reads")
                .envelopes
                .iter()
                .map(|e| e.op.clone())
                .filter(|op| {
                    matches!(
                        op,
                        Op::RoutingSnapshot { .. }
                            | Op::RoutingAttemptBegin { .. }
                            | Op::RoutingReceipt { .. }
                    )
                })
                .collect()
        }

        pub fn seen(&self) -> Vec<String> {
            self.seen_models.lock().unwrap().clone()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            drop(self.to_host.take());
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    pub fn flux_available() -> Vec<AvailableModel> {
        vec![AvailableModel {
            id: "flux-auto".into(),
            name: "flux-auto".into(),
        }]
    }
}

use acp_harness::Host;

static NO_AUTO: auto_routing::RoutingConfig = auto_routing::RoutingConfig {
    auto_opt_in: false,
    configured_default: None,
    tools_probe: false,
};

static AUTO_OPT_IN: auto_routing::RoutingConfig = auto_routing::RoutingConfig {
    auto_opt_in: true,
    configured_default: None,
    tools_probe: false,
};

fn configured_default(reference: &str) -> &'static auto_routing::RoutingConfig {
    // Leak a per-test config — tests are process-lifetime bounded.
    Box::leak(Box::new(auto_routing::RoutingConfig {
        auto_opt_in: false,
        configured_default: Some(reference.to_string()),
        tools_probe: false,
    }))
}

fn snapshot_mode(ops: &[Op]) -> Option<RoutingMode> {
    ops.iter().find_map(|op| match op {
        Op::RoutingSnapshot { routing_mode, .. } => Some(*routing_mode),
        _ => None,
    })
}

#[test]
fn routing_mode_separation_four_journaled_values() {
    // §8.1 routing-mode separation: implicit default, explicit pin,
    // configured default, and the explicit opt-in produce the four distinct
    // journaled routing_mode values — and only the fourth admits the
    // client-side ladder.
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    clear_env();
    unsafe { std::env::set_var("FLUX_API_KEY", "sk-p5-modes") };

    // 1. implicit_alias_passthrough: no pin, no opt-in.
    let mut host = Host::spawn(
        ProviderRouter::default(),
        acp_harness::flux_available(),
        "flux-auto",
        &NO_AUTO,
    );
    let session = host.new_session();
    let response = host.prompt(&session, "hi");
    assert!(
        response.get("result").is_some(),
        "implicit prompt completes: {response}"
    );
    let ops = host.routing_ops(&session);
    assert_eq!(
        snapshot_mode(&ops),
        Some(RoutingMode::ImplicitAliasPassthrough)
    );
    assert_eq!(host.seen(), ["flux-auto"], "alias passthrough to Flux only");
    drop(host);

    // 2. explicit_alias_pin: session/set_model("flux-auto") is a PIN even
    // with the opt-in live — never client-side routing (§1).
    let mut host = Host::spawn(
        ProviderRouter::default(),
        acp_harness::flux_available(),
        "flux-auto",
        &AUTO_OPT_IN,
    );
    let session = host.new_session();
    let ack = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session, "modelId": "flux-auto"}),
    );
    assert!(ack.get("result").is_some(), "set_model ack: {ack}");
    let response = host.prompt(&session, "hi");
    assert!(
        response.get("result").is_some(),
        "pinned prompt completes: {response}"
    );
    let ops = host.routing_ops(&session);
    assert_eq!(snapshot_mode(&ops), Some(RoutingMode::ExplicitAliasPin));
    assert_eq!(host.seen(), ["flux-auto"]);
    drop(host);

    // 3. configured_default_alias: the configured default pin beats the
    // opt-in too — and the default is flux-auto here.
    let mut host = Host::spawn(
        ProviderRouter::default(),
        acp_harness::flux_available(),
        "flux-auto",
        configured_default("flux-auto"),
    );
    let session = host.new_session();
    let response = host.prompt(&session, "hi");
    assert!(
        response.get("result").is_some(),
        "configured prompt completes: {response}"
    );
    let ops = host.routing_ops(&session);
    assert_eq!(
        snapshot_mode(&ops),
        Some(RoutingMode::ConfiguredDefaultAlias)
    );
    assert_eq!(host.seen(), ["flux-auto"]);
    drop(host);

    // 4. auto_client_side: the opt-in admits the ladder. Post-S1 the
    // vendored catalog blesses the flux-auto alias for tools, so the
    // tool-bearing turn dispatches rung 1 (the §5 capability gate is
    // satisfied by recorded proof — the pre-S1 capability_empty refusal is
    // superseded, never weakened: an UNPROVEN candidate still rejects).
    let mut host = Host::spawn(
        ProviderRouter::default(),
        acp_harness::flux_available(),
        "flux-auto",
        &AUTO_OPT_IN,
    );
    let session = host.new_session();
    let response = host.prompt(&session, "hi");
    assert!(
        response.get("result").is_some(),
        "the auto turn rides the blessed alias: {response}"
    );
    let ops = host.routing_ops(&session);
    assert_eq!(snapshot_mode(&ops), Some(RoutingMode::AutoClientSide));
    // The journaled snapshot shows the alias rung ADMITTED (blessed for
    // tools) and the turn committed on it.
    let admitted = ops.iter().any(|op| {
        matches!(
            op,
            Op::RoutingSnapshot { candidates, .. }
                if candidates.iter().any(|c| c.admitted
                    && c.kind == CandidateKind::Alias
                    && c.candidate == "flux-auto")
        )
    });
    assert!(admitted, "the alias rung is admitted: {ops:?}");
    assert_eq!(host.seen(), ["flux-auto"], "dispatch reached rung 1 only");
    drop(host);
}

#[test]
fn pin_terminality_matrix_no_fall_through() {
    // §8.1 pin terminality: unknown, unadvertised, unproven, and
    // uncredentialed explicit references each fail terminally with NO
    // fall-through to flux-auto, the fallback, or the ladder.
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    clear_env();
    unsafe { std::env::set_var("FLUX_API_KEY", "sk-p5-pins") };
    let router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-a"],"hasKey":true},
            {"provider":"nvidia","models":["nv-alpha"],"hasKey":true}
        ]"#,
    ))
    .expect("payload validates");
    let mut available = acp_harness::flux_available();
    available.extend(router.advertised_models());
    let mut host = Host::spawn(router, available, "flux-auto", &NO_AUTO);
    let session = host.new_session();

    // Unknown namespaced provider → typed model_not_found.
    let response = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session, "modelId": "ghost:model"}),
    );
    assert!(
        format!("{}", response["error"]).contains("model_not_found"),
        "{response}"
    );
    // Unadvertised model on a known provider → model_not_found.
    let response = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session, "modelId": "openai:ghost"}),
    );
    assert!(
        format!("{}", response["error"]).contains("model_not_found"),
        "{response}"
    );
    // Advertised but UNPROVEN provider → provider_unproven.
    let response = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session, "modelId": "nvidia:nv-alpha"}),
    );
    assert_eq!(
        response["error"]["data"]["kind"], "provider_unproven",
        "{response}"
    );
    // Advertised, proven, but UNCREDENTIALED → provider_key_missing.
    let response = host.request(
        "session/set_model",
        serde_json::json!({"sessionId": session, "modelId": "openai:gpt-a"}),
    );
    assert_eq!(
        response["error"]["data"]["kind"], "provider_key_missing",
        "{response}"
    );
    // Terminality: the session model never moved; the next prompt still
    // dispatches the implicit default, and nothing ever rerouted.
    let response = host.prompt(&session, "hi");
    assert!(response.get("result").is_some(), "{response}");
    assert_eq!(host.seen(), ["flux-auto"], "no fall-through on any failure");
    drop(host);

    // A misconfigured CONFIGURED default fails loudly at dispatch (typed
    // binding error), never silently reroutes to flux-auto.
    let mut available = acp_harness::flux_available();
    available.push(AvailableModel {
        id: "openai:gpt-a".into(),
        name: "gpt-a".into(),
    });
    let router = ProviderRouter::from_payload(Some(
        r#"[{"provider":"openai","models":["gpt-a"],"hasKey":true}]"#,
    ))
    .expect("payload");
    let mut host = Host::spawn(
        router,
        available,
        "openai:gpt-a",
        configured_default("openai:gpt-a"),
    );
    let session = host.new_session();
    let response = host.prompt(&session, "hi");
    // openai is advertised but has NO credential in this test → the typed
    // pin failure, and NO flux-auto dispatch despite the Flux key.
    assert_eq!(
        response["error"]["data"]["kind"], "provider_key_missing",
        "{response}"
    );
    let ops = host.routing_ops(&session);
    assert_eq!(
        snapshot_mode(&ops),
        Some(RoutingMode::ConfiguredDefaultAlias),
        "the configured default pin is journaled even on failure: {ops:?}"
    );
    assert!(host.seen().is_empty(), "a failing pin never falls through");
    drop(host);
}

#[test]
fn no_config_no_cross_provider_dispatch() {
    // §8.1 no-config no-cross-provider assertion: no explicit/configured
    // model, a resolved Flux credential, and multiple credentialed direct
    // providers — absent the explicit opt-in the turn is alias passthrough
    // to Flux ONLY.
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    clear_env();
    unsafe {
        std::env::set_var("FLUX_API_KEY", "sk-p5-nocross");
        std::env::set_var("OPENAI_API_KEY", "sk-p5-openai");
        std::env::set_var("NVIDIA_API_KEY", "sk-p5-nvidia");
    };
    let router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-a"],"hasKey":true},
            {"provider":"nvidia","models":["nv-alpha"],"hasKey":true}
        ]"#,
    ))
    .expect("payload validates");
    let mut available = acp_harness::flux_available();
    available.extend(router.advertised_models());
    let mut host = Host::spawn(router, available, "flux-auto", &NO_AUTO);
    let session = host.new_session();
    let response = host.prompt(&session, "hi");
    assert!(response.get("result").is_some(), "{response}");
    assert_eq!(
        host.seen(),
        ["flux-auto"],
        "no client-side cross-provider dispatch without the opt-in"
    );
    let ops = host.routing_ops(&session);
    let singleton = ops.iter().any(|op| {
        matches!(
            op,
            Op::RoutingSnapshot { candidates, routing_mode, .. }
                if *routing_mode == RoutingMode::ImplicitAliasPassthrough
                    && candidates.len() == 1
                    && candidates[0].kind == CandidateKind::Alias
        )
    });
    assert!(
        singleton,
        "the implicit turn journals a singleton alias snapshot: {ops:?}"
    );
    drop(host);

    // With the opt-in (post-S1: the vendored catalog blesses the flux-auto
    // alias for tools), the tool-bearing turn is admitted on rung 1 ONLY —
    // the ladder dispatches the alias, never a cross-provider leaf (no
    // approved-leaf manifest exists in v1, so rungs 2/3 are empty).
    let router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-a"],"hasKey":true},
            {"provider":"nvidia","models":["nv-alpha"],"hasKey":true}
        ]"#,
    ))
    .expect("payload validates");
    let mut available = acp_harness::flux_available();
    available.extend(router.advertised_models());
    let mut host = Host::spawn(router, available, "flux-auto", &AUTO_OPT_IN);
    let session = host.new_session();
    let response = host.prompt(&session, "hi");
    assert!(
        response.get("result").is_some(),
        "the opted-in tool-bearing turn rides the blessed alias: {response}"
    );
    assert_eq!(
        host.seen(),
        ["flux-auto"],
        "rung 1 only — opt-in never dispatches cross-provider"
    );
    let ops = host.routing_ops(&session);
    let admits_alias_only = ops.iter().any(|op| {
        matches!(
            op,
            Op::RoutingSnapshot { routing_mode, candidates, .. }
                if *routing_mode == RoutingMode::AutoClientSide
                    && candidates.len() == 1
                    && candidates[0].admitted
                    && candidates[0].kind == CandidateKind::Alias
                    && candidates[0].candidate == "flux-auto"
        )
    });
    assert!(
        admits_alias_only,
        "the auto snapshot admits exactly the alias rung: {ops:?}"
    );
    let committed = ops.iter().any(|op| {
        matches!(
            op,
            Op::RoutingReceipt { outcome, selected, .. }
                if *outcome == RoutingOutcome::Committed && *selected
        )
    });
    assert!(committed, "rung 1 committed: {ops:?}");
    drop(host);
}

#[test]
fn malformed_auto_env_is_a_typed_config_error() {
    // §1: a malformed NANO_ROUTING_AUTO value is a typed config error,
    // never a silent default.
    let err =
        auto_routing::parse_auto_opt_in(Some("enabled".into())).expect_err("malformed is typed");
    assert!(err.contains("NANO_ROUTING_AUTO"), "{err}");
}

// ── §8.2 leg 2 [seam]: Auto correctness — deterministic order ───────────

#[test]
fn auto_correctness_selected_leaf_matches_deterministic_order() {
    // Inject a controlled candidate catalog: alias rung + two proven leaves.
    // The first two rungs fail cascading; the selected leaf is the THIRD —
    // the deterministic construction order — and the whole path is journaled.
    let a = ScriptedTransport::fails(
        ModelError::RateLimited {
            retry_after_ms: None,
        },
        1,
        None,
    );
    let b = ScriptedTransport::fails(
        ModelError::Server {
            status: 500,
            message: String::new(),
        },
        1,
        None,
    );
    let c = ScriptedTransport::succeeds(Some("flux-pinned-gpt-5"), Usage::default());
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(
        vec![
            (
                candidate(0, "flux-router", "flux-auto", CandidateKind::Alias),
                a,
            ),
            (
                candidate(1, "flux-router", "flux-pinned-opus", CandidateKind::Leaf),
                b,
            ),
            (
                candidate(2, "flux-router", "flux-pinned-gpt-5", CandidateKind::Leaf),
                c,
            ),
        ],
        sink.clone(),
    );
    let response = block_on(ladder.complete_observed(&text_request(), &CallHooks::none()))
        .expect("rung 3 succeeds");
    assert_eq!(response.model.as_deref(), Some("flux-pinned-gpt-5"));
    let ops = sink.ops();
    assert_eq!(begins(&ops), [0, 1, 2], "deterministic order");
    let selected: Vec<u32> = ops
        .iter()
        .filter_map(|op| match op {
            Op::RoutingReceipt {
                ordinal,
                selected: true,
                ..
            } => Some(*ordinal),
            _ => None,
        })
        .collect();
    assert_eq!(selected, [2], "the selected leaf is journaled");
}

// ── §8.2 leg 6 [seam]: vision gating ────────────────────────────────────

#[test]
fn vision_gating_exact_proven_leaves_only_and_no_bytes_to_rejected() {
    // Image-bearing Auto with mixed proven/unproven leaves: only exact
    // proven leaves are eligible; rejected providers never enter the ladder
    // (no transport is ever built for them — no bytes can flow).
    let router = ProviderRouter::from_payload(Some(
        r#"[
            {"provider":"openai","models":["gpt-vision","gpt-plain"],"hasKey":true}
        ]"#,
    ))
    .expect("payload validates");
    let get_env = |name: &str| (name == "OPENAI_API_KEY").then(|| "sk-test".to_string());
    let vision = nano_model::vision_catalog::VisionCatalog::from_json_str(
        r#"{"version": 1, "entries": {
            "openai:gpt-vision": { "image_in": true, "proven": "shared/fixtures/flux/vision/probe.json" },
            "openai:gpt-plain": { "image_in": false, "proven": null }
        }}"#,
    )
    .expect("vision catalog parses");
    let inputs = auto_routing::CandidateInputs {
        router: &router,
        get_env: &get_env,
        now_unix_secs: 0,
        flux_credentialed: true,
        flux_advertised: &[],
        vision: &vision,
        tools: &auto_routing::EmptyToolCapabilityCatalog,
        approved_leaves: &[
            "openai:gpt-vision".to_string(),
            "openai:gpt-plain".to_string(),
        ],
        requirements: auto_routing::TurnRequirements {
            images: true,
            tools: false,
        },
    };
    let plan = auto_routing::construct_candidates(&inputs);
    let admitted: Vec<&str> = plan.admitted.iter().map(|c| c.reference.as_str()).collect();
    assert_eq!(
        admitted,
        ["openai:gpt-vision"],
        "the alias rung (never vision-blessed) and the unproven leaf are excluded"
    );
    let plain = plan
        .candidates
        .iter()
        .find(|c| c.candidate == "gpt-plain")
        .expect("journaled rejection");
    assert_eq!(
        plain.rejection,
        Some(nano_session::CandidateRejection::CapabilityUnproven)
    );
    // The ladder is built from the admitted subset ONLY: rejected providers
    // hold no transport, so no bytes can reach them — by construction.
    let proven = ScriptedTransport::succeeds(Some("gpt-vision"), Usage::default());
    let proven_calls = proven.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(vec![(plan.admitted[0].clone(), proven)], sink);
    let mut request = text_request();
    request.messages = vec![nano_model::types::Message::user_blocks(vec![
        nano_model::types::ContentBlock::Image {
            mime: "image/png".into(),
            data: "aA".into(),
        },
    ])];
    block_on(ladder.complete_observed(&request, &CallHooks::none())).expect("proven leaf serves");
    // The image content rides the admitted candidate unchanged (no
    // stripping, no conversion).
    assert!(matches!(
        proven_calls.lock().unwrap()[0].messages[0].content[0],
        nano_model::types::ContentBlock::Image { .. }
    ));

    // No eligible leaf at all: the pre-dispatch refusal (typed, distinct).
    let refusal_plan = auto_routing::construct_candidates(&auto_routing::CandidateInputs {
        router: &router,
        get_env: &get_env,
        now_unix_secs: 0,
        flux_credentialed: true,
        flux_advertised: &[],
        vision: &vision,
        tools: &auto_routing::EmptyToolCapabilityCatalog,
        approved_leaves: &["openai:gpt-plain".to_string()],
        requirements: auto_routing::TurnRequirements {
            images: true,
            tools: false,
        },
    });
    assert!(
        refusal_plan.admitted.is_empty(),
        "no eligible leaf → empty admitted set"
    );
    // The refusal error is the DISTINCT capability-empty kind (§5).
    let err = nano_cli::provider_router::ProviderError::capability_empty("image_in");
    assert_eq!(err.kind, nano_cli::provider_router::KIND_CAPABILITY_EMPTY);
    assert!(!err.retryable);
    let nocred = nano_cli::provider_router::ProviderError::no_credential("FLUX_API_KEY");
    assert_eq!(nocred.kind, nano_cli::provider_router::KIND_NO_CREDENTIAL);
    assert_ne!(err.kind, nocred.kind, "distinct typed empty states");
}

// ── §8.2 leg 7 [seam]: tool/MCP gating ──────────────────────────────────

#[test]
fn tool_gating_preserves_schema_and_admits_only_proven_leaves() {
    // Tool-bearing Auto with mixed capability leaves (injected catalog):
    // only proven leaves are eligible, and the tool schema/content is
    // preserved byte-identically on the wire — no downgrade.
    let router = ProviderRouter::from_payload(Some(
        r#"[{"provider":"openai","models":["gpt-tools","gpt-plain"],"hasKey":true}]"#,
    ))
    .expect("payload validates");
    let get_env = |name: &str| (name == "OPENAI_API_KEY").then(|| "sk-test".to_string());
    let vision = nano_model::vision_catalog::VisionCatalog::from_json_str(
        r#"{"version": 1, "entries": {}}"#,
    )
    .expect("empty vision catalog parses");
    let tool_catalog = |provider: &str, leaf: &str| provider == "openai" && leaf == "gpt-tools";
    let inputs = auto_routing::CandidateInputs {
        router: &router,
        get_env: &get_env,
        now_unix_secs: 0,
        flux_credentialed: true,
        flux_advertised: &[],
        vision: &vision,
        tools: &tool_catalog,
        approved_leaves: &[
            "openai:gpt-tools".to_string(),
            "openai:gpt-plain".to_string(),
        ],
        requirements: auto_routing::TurnRequirements {
            images: false,
            tools: true,
        },
    };
    let plan = auto_routing::construct_candidates(&inputs);
    let admitted: Vec<&str> = plan.admitted.iter().map(|c| c.reference.as_str()).collect();
    assert_eq!(admitted, ["openai:gpt-tools"]);
    let plain = plan
        .candidates
        .iter()
        .find(|c| c.candidate == "gpt-plain")
        .expect("journaled");
    assert_eq!(
        plain.rejection,
        Some(nano_session::CandidateRejection::CapabilityUnproven)
    );
    // The tool schema is preserved exactly through the ladder.
    let transport = ScriptedTransport::succeeds(Some("gpt-tools"), Usage::default());
    let calls = transport.calls.clone();
    let sink = Arc::new(CollectSink::default());
    let ladder = ladder_with(vec![(plan.admitted[0].clone(), transport)], sink);
    let mut request = text_request();
    request.tools = vec![nano_model::types::ToolDefinition {
        name: "mcp__server__search".into(),
        description: "search things".into(),
        input_schema: serde_json::json!({"type": "object", "properties": {"q": {"type": "string"}}}),
    }];
    let expected_tools = request.tools.clone();
    block_on(ladder.complete_observed(&request, &CallHooks::none())).expect("proven leaf serves");
    let dispatched = calls.lock().unwrap();
    assert_eq!(dispatched[0].tools, expected_tools, "no schema downgrade");
    assert_eq!(dispatched[0].model, "gpt-tools", "bare leaf id on the wire");
}

// ── §8.2 leg 10 [seam]: replay + canary ─────────────────────────────────

#[test]
fn replay_consumes_recorded_receipts_without_discovery_and_keeps_canary() {
    // Replay recorded receipts with NO discovery and NO network: the fold
    // consumes the journaled ops and reconstructs the ladder state.
    let fixture = kill_fixture("between_rungs");
    let coordinator = JournalCoordinator::open(&fixture.journal_path).expect("reopen");
    let sink = auto_routing::CoordinatorRoutingSink(Arc::new(coordinator));
    // Reconcile, then append a terminal receipt (the resumed rung's success).
    {
        let report = nano_session::read_journal(&fixture.journal_path).expect("read");
        let folded = SessionState::fold(&report.envelopes);
        let open = folded.open_turn.as_ref().expect("interrupted");
        let routing = folded.routing.get(&open.turn_id).expect("routing");
        auto_routing::reconcile_interrupted(&sink, &open.turn_id, routing, &open.input)
            .expect("reconcile");
    }
    // Replay: read + fold only — this test has no router/catalog/discovery
    // inputs at all, proving replay never rediscovers.
    let report = nano_session::read_journal(&fixture.journal_path).expect("replay reads");
    let folded = SessionState::fold(&report.envelopes);
    let routing = folded
        .routing
        .get(&fixture.turn_id)
        .expect("folded routing");
    assert_eq!(routing.attempts_consumed(), 2, "receipted + reconciled");
    let resume = auto_routing::plan_resume(routing).expect("resumable");
    assert_eq!(resume.budget, 1);
    assert_eq!(resume.remaining.len(), 1);
    // Canary: no credential appears in any frame, journal entry, or fixture.
    // The pre-kill fixture carried a canary-bearing error message in the
    // (unjournaled) wire exchange; the journal must be clean.
    let raw = std::fs::read_to_string(&fixture.journal_path).expect("journal bytes");
    for marker in ["sk-p5", "sk-loopback", "sk-test", "authorization"] {
        assert!(
            !raw.to_lowercase().contains(marker),
            "journal carries no credential material ({marker})"
        );
    }
}

// ── §8.2 legs 1 + 8 [live]: self-skipping without FLUX_TEST_KEY ─────────
// Live proof against real Flux. The key resolves via the standard chain at
// call time (the operator exports FLUX_TEST_KEY from the documented
// secrets path); the value is never logged, and every captured byte is
// canary-scanned before it lands in a fixture.

fn live_flux_key() -> Option<String> {
    // The standard Flux resolution chain (FLUX_API_KEY, FLUX_TEST_KEY,
    // FLUX_API_KEY_FILE) — never a hardcoded path read here.
    nano_cli::flux_key::flux_api_key()
}

#[test]
fn live_leg1_alias_identity() {
    // §8.2 leg 1 [live]: distinct prompt shapes through each Flux alias;
    // record requested alias + response-reported actual model. This leg is
    // the evidence path that could lift the §6 provenance-only rule.
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    let Some(key) = live_flux_key() else {
        eprintln!("p5 live leg 1: FLUX_TEST_KEY not set — self-skipping");
        return;
    };
    let client = nano_model::flux_completions::FluxCompletionsClient::new(
        nano_egress::client::EgressClient::flux(),
    )
    .with_retry_config(nano_model::retry::RetryConfig::single_attempt());
    let mut records = Vec::new();
    for (alias, prompt) in [
        ("flux-auto", "Reply with exactly: 17"),
        (
            "flux-standard",
            "What is 6 * 7? Answer with the number only.",
        ),
        ("flux-fast", "Say: ok"),
        ("flux-reasoning", "Compute 13*13, digit by digit."),
    ] {
        let request = ModelRequest {
            model: alias.to_string(),
            messages: vec![nano_model::types::Message::user(prompt)],
            max_tokens: Some(64),
            ..ModelRequest::default()
        };
        let outcome = block_on(client.complete(&request, &key));
        let record = match outcome {
            Ok(response) => serde_json::json!({
                "requested_alias": alias,
                "prompt_sha256_len": prompt.len(),
                "outcome": "ok",
                "response_model": response.model,
                "input_tokens": response.usage.input_tokens,
                "output_tokens": response.usage.output_tokens,
            }),
            Err(err) => serde_json::json!({
                "requested_alias": alias,
                "prompt_sha256_len": prompt.len(),
                "outcome": "error",
                "error_kind": format!("{err:?}").split(['(', ' ']).next().unwrap_or("unknown"),
                "response_model": null,
            }),
        };
        records.push(record);
    }
    // At least the flux-auto call must succeed for the evidence to mean
    // anything (a total outage is a live failure, not a pass).
    assert!(
        records
            .iter()
            .any(|r| r["requested_alias"] == "flux-auto" && r["outcome"] == "ok"),
        "flux-auto live call failed: {records:?}"
    );
    let fixture = serde_json::json!({
        "leg": "p5-auto-routing-8.2-leg1-alias-identity",
        "captured_at": "live",
        "records": records,
    });
    let text = serde_json::to_string_pretty(&fixture).expect("fixture serializes");
    // Canary: the credential NEVER lands in the fixture.
    assert!(!text.contains(&key), "canary: no key in the fixture");
    // F-P5-4: ancestor-walked fixture root (no hardcoded `..` count).
    let dir = shared_flux_fixture_dir().join("auto-routing");
    std::fs::create_dir_all(&dir).expect("fixture dir");
    std::fs::write(dir.join("alias-identity.json"), text).expect("fixture write");
}

#[test]
fn live_leg8_metering_provenance() {
    // §8.2 leg 8 [live arm]: a real flux-auto attempt whose response may
    // name a leaf — the metering rules apply: provenance recorded, unpriced
    // without the §6 evidence path.
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    let Some(key) = live_flux_key() else {
        eprintln!("p5 live leg 8: FLUX_TEST_KEY not set — self-skipping");
        return;
    };
    let client = nano_model::flux_completions::FluxCompletionsClient::new(
        nano_egress::client::EgressClient::flux(),
    )
    .with_retry_config(nano_model::retry::RetryConfig::single_attempt());
    let request = ModelRequest {
        model: "flux-auto".to_string(),
        messages: vec![nano_model::types::Message::user("Say: ok")],
        max_tokens: Some(16),
        ..ModelRequest::default()
    };
    let response = block_on(client.complete(&request, &key)).expect("live flux-auto call");
    let pricing = nano_model::pricing::PricingCatalog::load_default().expect("bundled pricing");
    let metering = auto_routing::meter_success(
        CandidateKind::Alias,
        "flux-router",
        &[],
        response.model.as_deref(),
        &response.usage,
        false, // v1: the evidence path is NOT established
        Some(&pricing),
    );
    // Whatever the wire reported: alias passthrough is never priced.
    assert!(
        !metering.usage.priced,
        "alias passthrough carries no pricing attribution"
    );
    assert!(metering.usage.reported);
    // A concrete reported leaf is provenance-only evidence (not a mismatch).
    if let Some(leaf) = &metering.response_model {
        assert!(
            !auto_routing::is_flux_alias(leaf) || metering.provenance == LeafProvenance::Absent
        );
    }
}

// ── S1 [live]: tool-bearing Auto turns through the flux-auto alias ──────
// Self-skips without the Flux key (standard chain at call time). Arm A
// drives the §4 ladder in-process with a real Flux driver (3 alias-stability
// trials, fixtures under shared/fixtures/flux/tools/); arm B runs one
// integrated `exec --auto` turn end-to-end. The probe arm
// (NANO_AUTO_TOOLS_PROBE) admits rung 1 for evidence capture BEFORE the
// vendored catalog blessing lands; post-bless the vendored `true` admits
// it and the probe is unnecessary.

/// Locate `shared/fixtures/flux` by walking ancestors of the manifest dir —
/// robust across the main-checkout and worktree layouts (the F-P5-4 class
/// of hardcoded-`..` bug).
fn shared_flux_fixture_dir() -> std::path::PathBuf {
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("shared/fixtures/flux");
        if candidate.is_dir() {
            return candidate;
        }
        if !dir.pop() {
            panic!("shared/fixtures/flux not found above CARGO_MANIFEST_DIR");
        }
    }
}

#[test]
fn live_leg_tool_bearing_auto_alias() {
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    let Some(key) = live_flux_key() else {
        eprintln!("p5 live tools leg: FLUX_TEST_KEY not set — self-skipping");
        return;
    };
    // Production candidate construction: the vendored catalog layered with
    // the probe arm as the process env carries it.
    let probe =
        auto_routing::parse_tools_probe(std::env::var(auto_routing::AUTO_TOOLS_PROBE_ENV).ok())
            .expect("typed probe parse");
    let vendored = nano_model::tool_capability::ToolCapabilityCatalog::vendored()
        .expect("vendored catalog parses");
    let tools = auto_routing::ProbeToolCatalog {
        inner: vendored,
        probe,
    };
    let vision = nano_model::vision_catalog::VisionCatalog::vendored().expect("vision parses");
    let router = ProviderRouter::default();
    let inputs = auto_routing::CandidateInputs {
        router: &router,
        get_env: &|_| None,
        now_unix_secs: 0,
        flux_credentialed: true,
        flux_advertised: &[],
        vision: &vision,
        tools: &tools,
        approved_leaves: &[],
        requirements: auto_routing::TurnRequirements {
            images: false,
            tools: true,
        },
    };
    let plan = auto_routing::construct_candidates(&inputs);
    assert_eq!(
        plan.admitted.len(),
        1,
        "rung 1 (the alias) is the one admitted candidate: {:?}",
        plan.candidates
    );
    assert_eq!(plan.admitted[0].candidate, "flux-auto");
    assert_eq!(plan.admitted[0].kind, CandidateKind::Alias);

    let tool_def = nano_model::types::ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get weather for a city".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
    };
    let request_json = serde_json::json!({
        "model": "flux-auto",
        "messages": [{"role": "user", "content": "What is the weather in Paris? Use the get_weather tool."}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather for a city",
                "parameters": tool_def.input_schema,
            }
        }],
        "tool_choice": "auto",
        "max_tokens": 256,
        "stream": false,
    });
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Arm A: three alias-stability trials through the real ladder.
    let mut trials = Vec::new();
    for trial in 0..3u32 {
        let client = nano_model::flux_completions::FluxCompletionsClient::new(
            nano_egress::client::EgressClient::flux(),
        )
        .with_retry_config(nano_model::retry::RetryConfig::single_attempt());
        let driver = nano_agent::wiring::FluxDriver::new(client, key.clone());
        let sink = Arc::new(CollectSink::default());
        let ladder = Ladder::new(
            &format!("s1-tools-trial-{trial}"),
            RoutingMode::AutoClientSide,
            "flux-auto",
            vec![LadderCandidate {
                plan: plan.admitted[0].clone(),
                transport: auto_routing::DriverTransport(driver),
            }],
            sink.clone(),
            None,
            false,
            auto_routing::ATTEMPT_BUDGET,
            0,
        );
        let request = ModelRequest {
            model: "flux-auto".to_string(),
            messages: vec![nano_model::types::Message::user(
                "What is the weather in Paris? Use the get_weather tool.",
            )],
            tools: vec![tool_def.clone()],
            max_tokens: Some(256),
            ..ModelRequest::default()
        };
        let response = block_on(ladder.complete_observed(&request, &CallHooks::none()))
            .expect("alias tool trial succeeds");
        let tool_calls: Vec<serde_json::Value> = response
            .events
            .iter()
            .filter_map(|event| match event {
                ModelEvent::ToolCallComplete(call) => Some(serde_json::json!({
                    "name": call.name,
                    "arguments": call.arguments,
                })),
                _ => None,
            })
            .collect();
        assert!(
            !tool_calls.is_empty(),
            "trial {trial}: the response carries tool_calls: {:?}",
            response.events
        );
        // The journaled ladder run: exactly one begin, committed on rung 1.
        let ops = sink.ops();
        assert_eq!(begins(&ops), [0], "one attempt on the alias rung");
        let committed = ops.iter().any(|op| {
            matches!(
                op,
                Op::RoutingReceipt {
                    ordinal: 0,
                    outcome: RoutingOutcome::Committed,
                    selected: true,
                    candidate,
                    ..
                } if candidate == "flux-auto"
            )
        });
        assert!(committed, "rung-1 committed receipt journaled: {ops:?}");
        trials.push(serde_json::json!({
            "trial": trial,
            "requested_alias": "flux-auto",
            "response_model": response.model,
            "tool_calls": tool_calls,
            "input_tokens": response.usage.input_tokens,
            "output_tokens": response.usage.output_tokens,
        }));
    }
    let manifest = serde_json::json!({
        "leg": "s1-tool-bearing-auto-alias",
        "captured_at_unix": stamp,
        "surface": "chat_completions",
        "request": request_json,
        "trials": trials,
    });
    let text = serde_json::to_string_pretty(&manifest).expect("fixture serializes");
    // Canary: the credential NEVER lands in the fixture.
    assert!(!text.contains(&key), "canary: no key in the fixture");
    let dir = shared_flux_fixture_dir().join("tools/flux-auto");
    std::fs::create_dir_all(&dir).expect("fixture dir");
    std::fs::write(dir.join(format!("{stamp}_manifest.json")), &text)
        .expect("timestamped manifest");
    // The stable citation the vendored catalog names in `proven`.
    std::fs::write(dir.join("manifest.json"), &text).expect("stable manifest");

    // Arm B: one integrated `exec --auto` tool turn end-to-end.
    let home = std::env::temp_dir().join(format!(
        "nano-p5-exec-live-{}-{}",
        std::process::id(),
        stamp
    ));
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(
        workspace.join("hello.txt"),
        "hello from the s1 live fixture\n",
    )
    .expect("seed file");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"));
    command
        .args([
            "exec",
            "--auto",
            "Read the file hello.txt with the fs_read tool, then reply with its exact contents.",
        ])
        .current_dir(&workspace)
        .env("NANO_HOME", &home)
        .env("FLUX_API_KEY", &key)
        // Hermetic: no inherited provider payload/credentials.
        .env_remove("WAYLAND_NANO_PROVIDERS");
    if let Ok(probe_value) = std::env::var(auto_routing::AUTO_TOOLS_PROBE_ENV) {
        command.env(auto_routing::AUTO_TOOLS_PROBE_ENV, probe_value);
    }
    let output = command.output().expect("spawn exec");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Canary: the key never appears on either stream.
    assert!(
        !stdout.contains(&key) && !stderr.contains(&key),
        "canary: no key"
    );
    assert!(
        !stdout.contains("capability_empty"),
        "the tool-bearing auto turn is NOT refused: {stdout}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "the tool-bearing auto turn completes: {stdout} {stderr}"
    );
    assert!(
        stdout.contains("\"tool_call\""),
        "a tool_call event streamed: {stdout}"
    );
    // The journaled snapshot admits rung 1 (the alias candidate).
    let sessions = home.join("sessions");
    let journal = std::fs::read_dir(&sessions)
        .expect("sessions dir")
        .next()
        .expect("one journal")
        .expect("entry")
        .path();
    let ops: Vec<Op> = nano_session::read_journal(&journal)
        .expect("journal reads")
        .envelopes
        .iter()
        .map(|e| e.op.clone())
        .collect();
    let admits_rung1 = ops.iter().any(|op| {
        matches!(
            op,
            Op::RoutingSnapshot {
                routing_mode,
                candidates,
                ..
            } if *routing_mode == RoutingMode::AutoClientSide
                && candidates.iter().any(|c| c.admitted
                    && c.kind == CandidateKind::Alias
                    && c.candidate == "flux-auto")
        )
    });
    assert!(
        admits_rung1,
        "the journaled snapshot admits rung 1: {ops:?}"
    );
    let raw = std::fs::read_to_string(&journal).expect("journal bytes");
    assert!(!raw.contains(&key), "canary: no key in the journal");
}

// ── §8.1 exec surface: --auto / --model / NANO_ROUTING_AUTO ─────────────
// Process-level legs against the real binary (the c11 pattern). The usage
// errors fire BEFORE any dispatch; the auto leg dispatches rung 1 (the
// S1-blessed alias) and fails closed on its fake credential — one physical
// attempt, never committed, whether the edge answers or is unreachable.

#[test]
fn exec_auto_admits_alias_and_bad_credential_fails_closed() {
    // Post-S1: the vendored tool-capability catalog blesses the flux-auto
    // alias, so `exec --auto` ADMITS rung 1 and dispatches. A bad
    // credential (or an unreachable edge) still fails the turn closed —
    // exactly one physical attempt, never committed, exit 1. (Pre-S1 this
    // leg asserted the pre-dispatch capability_empty refusal; the
    // capability gate moved from "nothing proven" to "proven on the
    // alias", and the fail-closed property now lives at the wire.)
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    clear_env();
    let home = std::env::temp_dir().join(format!(
        "nano-p5-exec-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&home).expect("home");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(["exec", "--auto", "hi"])
        .env("NANO_HOME", &home)
        .env("FLUX_API_KEY", "sk-p5-exec-auto")
        // Hermetic: no inherited provider payload/credentials.
        .env_remove("WAYLAND_NANO_PROVIDERS")
        .output()
        .expect("spawn exec");
    assert_eq!(
        output.status.code(),
        Some(1),
        "the admitted auto turn fails closed on a bad credential: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("capability_empty"),
        "the blessed alias is not capability-refused: {stdout}"
    );
    assert!(
        !stdout.contains("sk-p5-exec-auto"),
        "canary: no key on stdout"
    );
    // The journaled snapshot carries auto_client_side with the ADMITTED
    // alias rung; exactly one begin and one non-committed receipt (terminal
    // auth on the live wire, or cascade-exhausted when the edge is
    // unreachable — both fail closed within the budget).
    let sessions = home.join("sessions");
    let journal = std::fs::read_dir(&sessions)
        .expect("sessions dir")
        .next()
        .expect("one journal")
        .expect("entry")
        .path();
    let ops: Vec<Op> = nano_session::read_journal(&journal)
        .expect("journal reads")
        .envelopes
        .iter()
        .map(|e| e.op.clone())
        .collect();
    let snapshot = ops.iter().find_map(|op| match op {
        Op::RoutingSnapshot {
            routing_mode,
            candidates,
            attempt_budget,
            ..
        } => Some((*routing_mode, candidates.clone(), *attempt_budget)),
        _ => None,
    });
    let (mode, candidates, budget) = snapshot.expect("snapshot journaled");
    assert_eq!(mode, RoutingMode::AutoClientSide, "{ops:?}");
    assert_eq!(budget, auto_routing::ATTEMPT_BUDGET);
    assert!(
        candidates
            .iter()
            .any(|c| c.admitted && c.kind == CandidateKind::Alias && c.candidate == "flux-auto"),
        "rung 1 admitted: {candidates:?}"
    );
    assert_eq!(
        begins(&ops).len(),
        1,
        "exactly one physical attempt: {ops:?}"
    );
    let receipt_outcomes = receipts(&ops);
    assert_eq!(receipt_outcomes.len(), 1, "one receipt: {ops:?}");
    assert_ne!(
        receipt_outcomes[0].1,
        RoutingOutcome::Committed,
        "never committed on a bad credential"
    );
    let raw = std::fs::read_to_string(&journal).expect("journal bytes");
    assert!(
        !raw.contains("sk-p5-exec-auto"),
        "canary: no key in the journal"
    );
}

#[test]
fn exec_model_namespaced_is_a_typed_usage_error() {
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    clear_env();
    let home = std::env::temp_dir().join(format!(
        "nano-p5-exec-ns-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(["exec", "--model", "openai:gpt-5", "hi"])
        .env("NANO_HOME", &home)
        .env("FLUX_API_KEY", "sk-p5-exec-pin")
        .env_remove("WAYLAND_NANO_PROVIDERS")
        .output()
        .expect("spawn exec");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bare Flux model ids only"), "{stderr}");
}

#[test]
fn exec_malformed_routing_auto_env_is_exit_2() {
    let _guard = env_lock();
    let _restore = EnvRestore::snapshot();
    clear_env();
    let home = std::env::temp_dir().join(format!(
        "nano-p5-exec-env-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(["exec", "hi"])
        .env("NANO_HOME", &home)
        .env("FLUX_API_KEY", "sk-p5-exec-env")
        .env("NANO_ROUTING_AUTO", "enabled")
        .env_remove("WAYLAND_NANO_PROVIDERS")
        .output()
        .expect("spawn exec");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NANO_ROUTING_AUTO"), "{stderr}");
}
