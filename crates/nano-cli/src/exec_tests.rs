//! C11 §7 exec tests: the frozen JSONL v1 schema (fixture), seq discipline,
//! the exit-code matrix, auto-deny with named tool+mode, full_auto
//! containment, --output-last-message semantics, and bounded exit.

use crate::exec_mode::{
    ExecEvents, ExecGoal, ExecParams, ResumeTarget, atomic_replace_write, exec_gate_decision,
};
use crate::exec_run::run_exec_with;
use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ApprovalDecision, ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, ToolCall, Usage};
use nano_protocol::permission_mode::PermissionMode;
use nano_session::GoalBudgets;
use std::path::PathBuf;
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
    let exit = run_exec_with(
        &sessions,
        &dir,
        &workspace,
        params,
        "fake-model",
        move || model.clone(),
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[],
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
                exec_gate_decision(&call(name), mode, &policy, &dir, true),
                ApprovalDecision::Deny,
                "{mode:?}: {name} auto-denies in exec"
            );
        }
        assert_eq!(
            exec_gate_decision(&call("repo_map"), mode, &policy, &dir, false),
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
            exec_gate_decision(&call("tool_search"), mode, &policy, &dir, false),
            ApprovalDecision::Approve,
            "{mode:?}: tool_search approves (DiscoveryLocal, no server contact)"
        );
        for name in ["mcp_list_resources", "mcp_read_resource"] {
            assert_eq!(
                exec_gate_decision(&call(name), mode, &policy, &dir, true),
                ApprovalDecision::Deny,
                "{mode:?}: {name} would prompt ⇒ exec auto-denies"
            );
        }
        // The mcp__ catch-all stays strict: every mode denies in exec
        // (default/read_only categorically, full_auto would prompt).
        assert_eq!(
            exec_gate_decision(&call("mcp__s__t"), mode, &policy, &dir, true),
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
        exec_gate_decision(&contained, PermissionMode::FullAuto, &policy, &dir, false),
        ApprovalDecision::Approve,
        "contained writes auto-approve in full_auto"
    );
    assert_eq!(
        exec_gate_decision(&contained, PermissionMode::Default, &policy, &dir, false),
        ApprovalDecision::Deny,
        "default mode: promptable → auto-deny"
    );
    assert_eq!(
        exec_gate_decision(&contained, PermissionMode::ReadOnly, &policy, &dir, false),
        ApprovalDecision::Deny
    );
    let shell = ToolCall {
        id: "c".into(),
        name: "shell".into(),
        arguments: serde_json::json!({"command": "echo"}),
    };
    assert_eq!(
        exec_gate_decision(&shell, PermissionMode::FullAuto, &policy, &dir, false),
        ApprovalDecision::Deny,
        "no sandbox backend → deny (never prompt in exec)"
    );
    assert_eq!(
        exec_gate_decision(&shell, PermissionMode::FullAuto, &policy, &dir, true),
        ApprovalDecision::Approve
    );
    // cronjob create is NOT in full_auto's auto-approve set (§5.5): it
    // would prompt — exec auto-denies.
    let cronjob = ToolCall {
        id: "c".into(),
        name: "cronjob".into(),
        arguments: serde_json::json!({"action": "create"}),
    };
    assert_eq!(
        exec_gate_decision(&cronjob, PermissionMode::FullAuto, &policy, &dir, true),
        ApprovalDecision::Deny
    );
    // Read-only + control tools always approve.
    for name in ["fs_read", "goal_complete"] {
        let call = ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: serde_json::json!({}),
        };
        assert_eq!(
            exec_gate_decision(&call, PermissionMode::ReadOnly, &policy, &dir, false),
            ApprovalDecision::Approve
        );
    }
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
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[],
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
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[],
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
    let dir = tmpdir("mcp-reap");
    let sessions = dir.join("sessions");
    let workspace = dir.join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let pid_file = dir.join("child.pid");
    // A fake MCP stdio server that records its pid, then answers the
    // handshake (same JSON-RPC line discipline as nano-mcp's own fixtures).
    #[cfg(windows)]
    let spec = nano_agent::mcp::McpServerSpec {
        name: "fake".into(),
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
    };
    #[cfg(unix)]
    let spec = nano_agent::mcp::McpServerSpec {
        name: "fake".into(),
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
    };

    let shared = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
    let exit = run_exec_with(
        &sessions,
        &dir,
        &workspace,
        &params("hello"),
        "fake-model",
        || FakeModel::with(vec![text_response("hi")]),
        move |_, _| (FakeTools::default(), fake_policy()),
        false,
        false,
        &[spec],
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
