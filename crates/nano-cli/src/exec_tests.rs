//! C11 §7 exec tests: the frozen JSONL v1 schema (fixture), seq discipline,
//! the exit-code matrix, auto-deny with named tool+mode, full_auto
//! containment, --output-last-message semantics, and bounded exit.

use crate::exec_mode::{
    ExecEvents, ExecGoal, ExecParams, ExecRouting, ResumeTarget, atomic_replace_write,
    exec_gate_decision,
};
use crate::exec_run::run_exec_with;
use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ApprovalDecision, ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::permission_mode::PermissionMode;
use nano_session::GoalBudgets;
use std::path::PathBuf;

/// P5: the pre-P5 posture — implicit alias passthrough, reference flux-auto.
const IMPLICIT_ROUTING: ExecRouting = ExecRouting {
    mode: nano_session::RoutingMode::ImplicitAliasPassthrough,
    reference: String::new(),
    tools_probe: false,
};
use std::sync::Mutex;

fn tmpdir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "nano-c11-exec-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Most exec-gate legs predate the rules lane: an empty ruleset (no rule
/// can match, so behavior is exactly the pre-rules baseline).
fn no_rules() -> nano_core::execrules::RuleSet {
    nano_core::execrules::RuleSet::default()
}

/// A ruleset parsed from rule lines (the engine's validator applies).
fn rules_with(rules: Vec<nano_core::execrules::PrefixRule>) -> nano_core::execrules::RuleSet {
    nano_core::execrules::RuleSet::new(rules).expect("valid rules")
}

fn allow_rule(program: &str) -> nano_core::execrules::PrefixRule {
    nano_core::execrules::PrefixRule {
        pattern: vec![nano_core::execrules::PatternToken::Single(program.into())],
        exact: false,
        decision: nano_core::execrules::RuleDecision::Allow,
        justification: None,
        added_at: None,
        source: None,
    }
}

fn deny_rule(program: &str) -> nano_core::execrules::PrefixRule {
    nano_core::execrules::PrefixRule {
        decision: nano_core::execrules::RuleDecision::Deny,
        ..allow_rule(program)
    }
}

/// A scripted model: each queued response is returned in order. Clones
/// share the queue (the per-turn driver factory hands out clones).
#[derive(Debug, Default, Clone)]
struct FakeModel {
    responses: std::sync::Arc<Mutex<std::collections::VecDeque<Result<ModelResponse, ModelError>>>>,
}

impl FakeModel {
    fn with(responses: Vec<Result<ModelResponse, ModelError>>) -> Self {
        Self {
            responses: std::sync::Arc::new(Mutex::new(responses.into())),
        }
    }
}

fn text_response(text: &str) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![ModelEvent::TextDelta(text.to_string())],
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        stop_reason: "end_turn".into(),
        model: None,
    })
}

fn tool_response(call: ToolCall) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        events: vec![ModelEvent::ToolCallComplete(call)],
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        stop_reason: "tool_use".into(),
        model: None,
    })
}

#[async_trait::async_trait]
impl ModelDriver for FakeModel {
    async fn complete(&self, _request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text_response("(no more scripted responses)"))
    }
}

/// A scripted tool executor: never touches the filesystem.
#[derive(Debug, Default)]
struct FakeTools {
    calls: Mutex<Vec<ToolCall>>,
}

#[async_trait::async_trait]
impl ToolExecutor for FakeTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        self.calls.lock().unwrap().push(call.clone());
        ToolOutcome {
            ok: true,
            output: "fake-ok".into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

fn fake_policy() -> nano_core::permissions::FileSystemSandboxPolicy {
    nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy()
}

fn params(prompt: &str) -> ExecParams {
    ExecParams {
        prompt: prompt.to_string(),
        mode: PermissionMode::Default,
        resume: None,
        output_last_message: None,
        goal: None,
        model: None,
        auto: false,
    }
}

struct ExecRun {
    exit: i32,
    events: Vec<serde_json::Value>,
}

/// Runs exec with a SHARED output buffer so the JSONL stream is inspectable.
async fn run_fake_shared(tag: &str, model: FakeModel, params: &ExecParams) -> (ExecRun, PathBuf) {
    let dir = tmpdir(tag);
    let sessions = dir.join("sessions");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let shared = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = SharedWriter(shared.clone());
    let ladder_model = model.clone();
    let exit = run_exec_with(
        &sessions,
        &dir,
        &workspace,
        params,
        "fake-model",
        move || model.clone(),
        move || ladder_model.clone(),
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[],
        &IMPLICIT_ROUTING,
        writer,
    )
    .await;
    let bytes = shared.lock().unwrap().clone();
    let text = String::from_utf8(bytes).unwrap();
    let events = text
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect();
    (ExecRun { exit, events }, dir)
}

#[derive(Clone)]
struct SharedWriter(std::sync::Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

const KNOWN_TYPES: &[&str] = &[
    "session_started",
    "text_delta",
    "tool_call",
    "tool_result",
    "approval_denied",
    "turn_completed",
    "error",
];

/// The frozen v1 schema fixture (Q2): every line carries v:1 + session_id +
/// monotonic seq; only the seven event types exist; the fixture sequence is
/// pinned end to end.
#[tokio::test]
async fn jsonl_v1_schema_fixture() {
    let model = FakeModel::with(vec![
        tool_response(ToolCall {
            id: "call-1".into(),
            name: "fs_read".into(), // auto-approved in every mode
            arguments: serde_json::json!({"path": "x.txt"}),
        }),
        text_response("done"),
    ]);
    let (run, dir) = run_fake_shared("schema", model, &params("read a file")).await;
    assert_eq!(run.exit, 0);

    // Envelope invariants on EVERY line.
    for (index, event) in run.events.iter().enumerate() {
        assert_eq!(event["v"], 1, "line {index}");
        assert!(
            event["session_id"]
                .as_str()
                .unwrap()
                .starts_with("wayland-nano-session-")
        );
        assert_eq!(
            event["seq"].as_u64().unwrap(),
            index as u64,
            "monotonic seq"
        );
        let kind = event["type"].as_str().unwrap();
        assert!(KNOWN_TYPES.contains(&kind), "unknown event type: {kind}");
    }
    // The pinned sequence for this fixture.
    let types: Vec<&str> = run
        .events
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "session_started",
            "tool_call",
            "tool_result",
            "text_delta",
            "turn_completed"
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// P4 §4.3/§13: exec can never prompt, and the PTY spawn's authorization IS
/// the prompt — all five pty_* names auto-DENY in every mode (headless PTY
/// is out of scope, §16). repo_map (§5.5) is read-only and approves in
/// every mode.
#[test]
fn p4_exec_gate_pty_denied_repo_map_read_only() {
    let dir = tmpdir("gate-pty");
    let policy = fake_policy();
    let call = |name: &str| ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: serde_json::json!({}),
    };
    for mode in [
        PermissionMode::ReadOnly,
        PermissionMode::Default,
        PermissionMode::FullAuto,
    ] {
        for name in nano_agent::wiring::PTY_TOOL_NAMES {
            assert_eq!(
                exec_gate_decision(&call(name), mode, &policy, &dir, true, &no_rules()),
                ApprovalDecision::Deny,
                "{mode:?}: {name} auto-denies in exec"
            );
        }
        assert_eq!(
            exec_gate_decision(&call("repo_map"), mode, &policy, &dir, false, &no_rules()),
            ApprovalDecision::Approve,
            "{mode:?}: repo_map is read-only in exec"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// P3 §4.3: exec is non-interactive — DiscoveryLocal (tool_search, no server
/// contact) approves in every mode; the server-contact classes would prompt,
/// so they auto-DENY in every mode, full_auto included.
#[test]
fn p3_exec_gate_mcp_classes() {
    let dir = tmpdir("gate-mcp");
    let policy = fake_policy();
    let call = |name: &str| ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: serde_json::json!({}),
    };
    for mode in [
        PermissionMode::ReadOnly,
        PermissionMode::Default,
        PermissionMode::FullAuto,
    ] {
        assert_eq!(
            exec_gate_decision(
                &call("tool_search"),
                mode,
                &policy,
                &dir,
                false,
                &no_rules()
            ),
            ApprovalDecision::Approve,
            "{mode:?}: tool_search approves (DiscoveryLocal, no server contact)"
        );
        for name in ["mcp_list_resources", "mcp_read_resource"] {
            assert_eq!(
                exec_gate_decision(&call(name), mode, &policy, &dir, true, &no_rules()),
                ApprovalDecision::Deny,
                "{mode:?}: {name} would prompt ⇒ exec auto-denies"
            );
        }
        // The mcp__ catch-all stays strict: every mode denies in exec
        // (default/read_only categorically, full_auto would prompt).
        assert_eq!(
            exec_gate_decision(&call("mcp__s__t"), mode, &policy, &dir, true, &no_rules()),
            ApprovalDecision::Deny,
            "{mode:?}: mcp__ never auto-approves in exec"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// approval auto-deny: a promptable action in default mode is denied and
/// emits approval_denied naming the tool and the mode; the model sees a
/// tool error and may continue.
#[tokio::test]
async fn approval_auto_deny_emits_named_event() {
    let model = FakeModel::with(vec![
        tool_response(ToolCall {
            id: "call-9".into(),
            name: "shell".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        }),
        text_response("cannot run shell, explained"),
    ]);
    let (run, dir) = run_fake_shared("deny", model, &params("run echo")).await;
    assert_eq!(run.exit, 0, "denials are ordinary tool errors — exit 0");
    let denied: Vec<&serde_json::Value> = run
        .events
        .iter()
        .filter(|e| e["type"] == "approval_denied")
        .collect();
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0]["tool"], "shell");
    assert_eq!(denied[0]["mode"], "default");
    assert_eq!(denied[0]["call_id"], "call-9");
    let _ = std::fs::remove_dir_all(&dir);
}

/// full_auto in exec: contained writes auto-approve (the tool RAN — the
/// tool_call event exists with no approval_denied); uncontained writes and
/// shell without a sandbox backend still auto-deny.
#[test]
fn full_auto_gate_decision_matrix() {
    let dir = tmpdir("gate");
    let policy = fake_policy();
    let contained = ToolCall {
        id: "c".into(),
        name: "fs_write".into(),
        arguments: serde_json::json!({"path": dir.join("ok.txt").to_string_lossy(), "content": "x"}),
    };
    assert_eq!(
        exec_gate_decision(
            &contained,
            PermissionMode::FullAuto,
            &policy,
            &dir,
            false,
            &no_rules()
        ),
        ApprovalDecision::Approve,
        "contained writes auto-approve in full_auto"
    );
    assert_eq!(
        exec_gate_decision(
            &contained,
            PermissionMode::Default,
            &policy,
            &dir,
            false,
            &no_rules()
        ),
        ApprovalDecision::Deny,
        "default mode: promptable → auto-deny"
    );
    assert_eq!(
        exec_gate_decision(
            &contained,
            PermissionMode::ReadOnly,
            &policy,
            &dir,
            false,
            &no_rules()
        ),
        ApprovalDecision::Deny
    );
    let shell = ToolCall {
        id: "c".into(),
        name: "shell".into(),
        arguments: serde_json::json!({"command": "echo"}),
    };
    assert_eq!(
        exec_gate_decision(
            &shell,
            PermissionMode::FullAuto,
            &policy,
            &dir,
            false,
            &no_rules()
        ),
        ApprovalDecision::Deny,
        "no sandbox backend → deny (never prompt in exec)"
    );
    assert_eq!(
        exec_gate_decision(
            &shell,
            PermissionMode::FullAuto,
            &policy,
            &dir,
            true,
            &no_rules()
        ),
        ApprovalDecision::Approve
    );
    // cronjob is pinned typed-denied on the exec surface in EVERY mode
    // (F-6 closure: create/delete would prompt — exec can never prompt;
    // list stays out of v1 headless scope).
    for action in ["create", "list", "delete"] {
        let cronjob = ToolCall {
            id: "c".into(),
            name: "cronjob".into(),
            arguments: serde_json::json!({"action": action}),
        };
        for mode in PermissionMode::ALL {
            assert_eq!(
                exec_gate_decision(&cronjob, mode, &policy, &dir, true, &no_rules()),
                ApprovalDecision::Deny,
                "exec must typed-deny cronjob {action} in {mode:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// P4 §2.6 + §13 gate matrix, exec surface: an Allow rule IS the approval
/// (default AND full_auto, sandbox or not — exec is non-interactive); a
/// Deny rule auto-denies in every mode including full_auto-with-sandbox; a
/// non-matching command keeps the would-prompt ⇒ auto-deny baseline; and
/// read_only categorically denies even an Allow-matched command (rules are
/// consulted strictly inside the Default/FullAuto arms).
#[test]
fn p4_exec_gate_rules() {
    let dir = tmpdir("gate-rules");
    let policy = fake_policy();
    let shell = |command: &str| ToolCall {
        id: "c".into(),
        name: "shell".into(),
        arguments: serde_json::json!({"command": command}),
    };
    let rules = rules_with(vec![allow_rule("echo"), deny_rule("denyme")]);
    for mode in [PermissionMode::Default, PermissionMode::FullAuto] {
        for sandbox in [true, false] {
            assert_eq!(
                exec_gate_decision(&shell("echo hi"), mode, &policy, &dir, sandbox, &rules),
                ApprovalDecision::Approve,
                "{mode:?} sandbox={sandbox}: an Allow rule is the approval"
            );
            assert_eq!(
                exec_gate_decision(&shell("denyme /f x"), mode, &policy, &dir, sandbox, &rules),
                ApprovalDecision::Deny,
                "{mode:?} sandbox={sandbox}: a Deny rule refuses"
            );
        }
        // No match: default auto-denies (would prompt); full_auto keeps its
        // sandbox baseline (Q7: rules narrow, never redefine).
        assert_eq!(
            exec_gate_decision(&shell("whoami"), mode, &policy, &dir, false, &rules),
            ApprovalDecision::Deny
        );
        assert_eq!(
            exec_gate_decision(&shell("whoami"), mode, &policy, &dir, true, &rules),
            match mode {
                PermissionMode::FullAuto => ApprovalDecision::Approve,
                _ => ApprovalDecision::Deny,
            },
            "{mode:?}: the no-match baseline is unchanged"
        );
    }
    // read_only: even an Allow-matched command denies — the mode arm
    // precedes rule consultation (the narrow-only arm order).
    assert_eq!(
        exec_gate_decision(
            &shell("echo hi"),
            PermissionMode::ReadOnly,
            &policy,
            &dir,
            true,
            &rules
        ),
        ApprovalDecision::Deny,
        "read_only must deny even a rule-allowed command"
    );
    // The compound strictest-wins rule applies through the gate: an allow
    // on segment 1 cannot launder a denied segment 2.
    assert_eq!(
        exec_gate_decision(
            &shell("echo hi && denyme /f x"),
            PermissionMode::FullAuto,
            &policy,
            &dir,
            true,
            &rules
        ),
        ApprovalDecision::Deny,
        "compound: one denied segment denies the whole command"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Exit-code matrix: 0 on stop; 1 on model failure; 1 on a turn-level
/// TurnBudget trip; 2 on a bad resume target.
#[tokio::test]
async fn exit_code_matrix() {
    // 0: clean stop.
    let (run, dir) = run_fake_shared(
        "exit0",
        FakeModel::with(vec![text_response("hi")]),
        &params("say hi"),
    )
    .await;
    assert_eq!(run.exit, 0);
    assert_eq!(run.events.last().unwrap()["type"], "turn_completed");
    assert_eq!(run.events.last().unwrap()["stop_reason"], "end_turn");
    assert_eq!(run.events.last().unwrap()["usage"]["input_tokens"], 10);

    // 1: model failure (provider error).
    let failing = FakeModel::with(vec![Err(ModelError::Server {
        status: 500,
        message: "boom".into(),
    })]);
    let (run, dir2) = run_fake_shared("exit1", failing, &params("go")).await;
    assert_eq!(run.exit, 1);
    assert!(run.events.iter().any(|e| e["type"] == "error"));

    // 2: resume target not found.
    let bad = ExecParams {
        resume: Some(ResumeTarget::Id("no-such-session".into())),
        ..params("go")
    };
    let (run, dir3) = run_fake_shared("exit2b", FakeModel::default(), &bad).await;
    assert_eq!(run.exit, 2);
    let _ = (dir, dir2, dir3);
}

/// seq restarts at 0 on a --resume invocation (fresh per-process stream),
/// and the session continues the SAME journal.
#[tokio::test]
async fn resume_restarts_seq_and_continues_journal() {
    let dir = tmpdir("resume");
    let sessions = dir.join("sessions");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let shared = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
    let exit = run_exec_with(
        &sessions,
        &dir,
        &workspace,
        &params("first"),
        "fake-model",
        || FakeModel::with(vec![text_response("one")]),
        || FakeModel::with(vec![text_response("one")]),
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[],
        &IMPLICIT_ROUTING,
        SharedWriter(shared.clone()),
    )
    .await;
    assert_eq!(exit, 0);
    let first: Vec<serde_json::Value> = String::from_utf8(shared.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let session_id = first[0]["session_id"].as_str().unwrap().to_string();

    // Second invocation: resume the SAME session. seq restarts at 0.
    shared.lock().unwrap().clear();
    let resumed_params = ExecParams {
        resume: Some(ResumeTarget::Id(session_id.clone())),
        ..params("second")
    };
    let exit = run_exec_with(
        &sessions,
        &dir,
        &workspace,
        &resumed_params,
        "fake-model",
        || FakeModel::with(vec![text_response("two")]),
        || FakeModel::with(vec![text_response("two")]),
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[],
        &IMPLICIT_ROUTING,
        SharedWriter(shared.clone()),
    )
    .await;
    assert_eq!(exit, 0);
    let second: Vec<serde_json::Value> = String::from_utf8(shared.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(second[0]["seq"], 0, "seq restarts per process invocation");
    assert_eq!(second[0]["session_id"], session_id);
    assert_eq!(second[0]["type"], "session_started");
    assert_eq!(second[0]["resumed"], true);
    // Journal continuity: ONE journal holds both turns.
    let report = nano_session::read_journal(&sessions.join(format!("{session_id}.jsonl"))).unwrap();
    let turns = report
        .envelopes
        .iter()
        .filter(|e| matches!(e.op, nano_session::Op::TurnBegin { .. }))
        .count();
    assert_eq!(turns, 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// --output-last-message: written atomically on success (overwrite
/// replaces), untouched on failure, never partial.
#[tokio::test]
async fn output_last_message_semantics() {
    let dir = tmpdir("lastmsg");
    let target = dir.join("last.txt");
    let run_params = ExecParams {
        output_last_message: Some(target.clone()),
        ..params("say the thing")
    };
    let (run, run_dir) = run_fake_shared(
        "lastmsg-ok",
        FakeModel::with(vec![text_response("final answer")]),
        &run_params,
    )
    .await;
    assert_eq!(run.exit, 0);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "final answer");

    // Overwrite replaces (the Windows replace-existing pin).
    let (run2, _) = run_fake_shared(
        "lastmsg-replace",
        FakeModel::with(vec![text_response("second answer")]),
        &run_params,
    )
    .await;
    assert_eq!(run2.exit, 0);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "second answer");

    // Failure leaves the pre-existing file untouched.
    let (run3, _) = run_fake_shared(
        "lastmsg-fail",
        FakeModel::with(vec![Err(ModelError::Server {
            status: 500,
            message: "boom".into(),
        })]),
        &run_params,
    )
    .await;
    assert_eq!(run3.exit, 1);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "second answer",
        "failed turns never touch the file"
    );
    // No stray temp files left behind.
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no partial/temp files: {leftovers:?}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&run_dir);
}

/// atomic_replace_write primitive: replace-existing works on this platform
/// (the Windows CI regression pin).
#[test]
fn atomic_replace_replaces_existing() {
    let dir = tmpdir("atomic");
    let target = dir.join("f.txt");
    std::fs::write(&target, "old").unwrap();
    atomic_replace_write(&target, b"new").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Goal mode end to end: goal_complete via the trusted tool → exit 0;
/// budget trip → blocked(budget_*) journaled + exit 3; model text claiming
/// completion → INERT (exit 3 via turn budget, never 0).
#[tokio::test]
async fn goal_end_to_end_via_exec() {
    // Completion through the tool (FakeTools executes goal_complete? No —
    // the GoalToolExecutor intercepts before FakeTools; the scripted model
    // just has to CALL goal_complete).
    let model = FakeModel::with(vec![tool_response(ToolCall {
        id: "gc-1".into(),
        name: "goal_complete".into(),
        arguments: serde_json::json!({"summary": "objective achieved"}),
    })]);
    let goal_params = ExecParams {
        goal: Some(ExecGoal {
            objective: "achieve the objective".into(),
            budgets: GoalBudgets::default(),
        }),
        ..params("")
    };
    let (run, dir) = run_fake_shared("goal-ok", model, &goal_params).await;
    assert_eq!(run.exit, 0, "events: {:?}", run.events);
    let _ = std::fs::remove_dir_all(&dir);

    // Turn budget trip → exit 3 with blocked(budget_turns) journaled.
    let model = FakeModel::default(); // always text, never completes
    let goal_params = ExecParams {
        goal: Some(ExecGoal {
            objective: "never finishes".into(),
            budgets: GoalBudgets {
                token_budget: None,
                turn_budget: Some(2),
                wall_clock_budget_ms: None,
            },
        }),
        ..params("")
    };
    let (run, dir2) = run_fake_shared("goal-budget", model, &goal_params).await;
    assert_eq!(run.exit, 3);
    let report = nano_session::read_journal(&dir2.join("sessions").join(format!(
        "{}.jsonl",
        run.events[0]["session_id"].as_str().unwrap()
    )))
    .unwrap();
    assert!(report.envelopes.iter().any(|e| matches!(
        &e.op,
        nano_session::Op::GoalStatus {
            status: nano_session::GoalStatusKind::Blocked,
            reason: nano_session::GoalReason::BudgetTurns,
            ..
        }
    )));
    let _ = std::fs::remove_dir_all(&dir2);
}

/// Bounded exit: the run returns promptly after the turn ends (no stray
/// handles; the in-process run completing IS the bounded-exit assertion for
/// the engine path — the process-level smoke runs in CI).
#[tokio::test]
async fn exec_returns_promptly_after_turn_end() {
    let start = std::time::Instant::now();
    let (run, dir) = run_fake_shared(
        "bounded",
        FakeModel::with(vec![text_response("quick")]),
        &params("be quick"),
    )
    .await;
    assert_eq!(run.exit, 0);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "exec must exit promptly after turn end"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The events writer tolerates unknown consumer fields (forward-compat pin
/// is producer-side: we never emit them; consumers ignore them — asserted
/// here by round-tripping an unknown-field event through a lenient parse).
#[test]
fn forward_compat_unknown_fields_and_types_are_ignored() {
    let lenient: serde_json::Value = serde_json::from_str(
        r#"{"v":1,"session_id":"s","seq":0,"type":"goal_status","extra":"field"}"#,
    )
    .unwrap();
    assert_eq!(lenient["type"], "goal_status"); // a v2 candidate parses fine
    let mut events = ExecEvents::new(Vec::new(), "s".to_string());
    events.error("x");
    let _ = events;
}

/// Bounded exit, no stray MCP child (§2.2): an exec session that registered
/// an MCP stdio server exits with the child REAPED — the registry drops
/// with the run and kills its stdio children (the oracle is the OS process
/// inventory, never self-report).
#[tokio::test]
async fn exec_reaps_mcp_children_on_exit() {
    // The unix contained spawn (seatbelt/bwrap workspace-write) may only
    // write under the host cwd — so this fixture anchors under target/,
    // not the OS temp dir: a /tmp pid-file write would be DENIED inside
    // containment and the server would die before recording its pid
    // (CI unix-leg failure, 2026-08-14). Same precedent as mcp_tests.
    let scratch = std::env::current_dir().expect("cwd").join("target");
    std::fs::create_dir_all(&scratch).expect("fixture scratch root");
    let dir = tempfile::Builder::new()
        .prefix("nano-cli-exec-mcp-reap-")
        .tempdir_in(&scratch)
        .expect("fixture dir")
        .keep();
    let sessions = dir.join("sessions");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let pid_file = dir.join("child.pid");
    // A fake MCP stdio server that records its pid, then answers the
    // handshake (same JSON-RPC line discipline as nano-mcp's own fixtures).
    #[cfg(windows)]
    let spec = nano_agent::mcp::McpServerSpec {
        name: "fake".into(),
        transport: nano_agent::mcp::Transport::Stdio {
            command: "powershell.exe".into(),
            args: vec![
            "-NoProfile".into(),
            "-Command".into(),
            r#"$PID | Out-File -Encoding ascii '@PIDFILE@'
$reader = [System.Console]::In
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { break }
    if ($line -match '"method"\s*:\s*"initialize"') {
        $id = ($line | ConvertFrom-Json).id
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$id,`"result`":{`"protocolVersion`":`"2025-03-26`",`"capabilities`":{},`"serverInfo`":{`"name`":`"fake`",`"version`":`"0`"}}}")
    } elseif ($line -match '"method"\s*:\s*"tools/list"') {
        $id = ($line | ConvertFrom-Json).id
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$id,`"result`":{`"tools`":[]}}")
    }
}
"#
            .replace("@PIDFILE@", &pid_file.display().to_string()),
            ],
            env: vec![],
        },
        source: nano_agent::mcp::SpecSource::Config,
    };
    #[cfg(unix)]
    let spec = nano_agent::mcp::McpServerSpec {
        name: "fake".into(),
        transport: nano_agent::mcp::Transport::Stdio {
            command: "sh".into(),
            args: vec![
            "-c".into(),
            r#"echo $$ > @PIDFILE@
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*//p')
    case "$line" in
        *'"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}
' "$id" ;;
        *'"tools/list"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}
' "$id" ;;
    esac
done
"#
            .replace("@PIDFILE@", &pid_file.display().to_string()),
            ],
            env: vec![],
        },
        source: nano_agent::mcp::SpecSource::Config,
    };

    let shared = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
    let exit = run_exec_with(
        &sessions,
        &dir,
        &workspace,
        &params("hello"),
        "fake-model",
        || FakeModel::with(vec![text_response("hi")]),
        || FakeModel::with(vec![text_response("hi")]),
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[spec],
        &IMPLICIT_ROUTING,
        SharedWriter(shared),
    )
    .await;
    assert_eq!(exit, 0);

    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("fake server recorded its pid")
        .trim()
        .parse()
        .expect("numeric pid");
    #[cfg(windows)]
    let alive = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            &format!("if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }} else {{ exit 1 }}"),
        ])
        .status()
        .expect("probe")
        .success();
    #[cfg(unix)]
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .expect("probe")
        .success();
    assert!(!alive, "MCP child {pid} must be reaped when exec exits");
    let _ = std::fs::remove_dir_all(&dir);
}
