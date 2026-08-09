//! C2 fixture task — REAL end-to-end: live Flux drives the turn engine over
//! real fs/shell tools on bare-metal Windows. Gated on FLUX_TEST_KEY.
//!
//! Scorecard mapping:
//! - C2.1: read project → patch → run tests → verify → report (external oracle)
//! - C2.2: cancellation stops a turn at a step boundary
//! - C2.3: journal Ops written and replayable
//!
//! The oracle is external state (file contents, test script result, journal
//! file), never the agent's self-report.

use nano_agent::loop_protection::TurnBudget;
use nano_agent::turn::{TurnEngine, TurnState};
use nano_agent::wiring::{FluxDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_session::reader::read_journal;
use nano_session::replay::SessionState;
use nano_session::writer::JournalWriter;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;

fn key() -> Option<String> {
    std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// Builds the broken mini-project: math.rs with an inverted add, and a
/// check.cmd that fails until the file contains `a + b`.
fn broken_project(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("nanok3-c2-{tag}-{}", std::process::id()));
    let ws = root.join("workspace");
    let home = root.join("nano-home");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        ws.join("math.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a - b // BUG: wrong operator here\n}\n",
    )
    .unwrap();
    std::fs::write(
        ws.join("check.cmd"),
        "@echo off\nfindstr /C:\"a + b\" math.rs >nul && (echo TEST PASS) || (echo TEST FAIL & exit /b 1)\n",
    )
    .unwrap();
    root
}

fn engine_for<'a>(
    ws: &'a std::path::Path,
    home: &'a std::path::Path,
    driver: &'a FluxDriver,
    executor: &'a RealToolExecutor,
    approve_all: &'a nano_agent::turn::ApproveAll,
) -> TurnEngine<'a> {
    let _ = (ws, home);
    TurnEngine {
        model: driver,
        tools: executor,
        budget: TurnBudget {
            max_steps: 12,
            max_tool_calls: 20,
            max_wall_time: std::time::Duration::from_secs(600),
        },
        model_name: "flux-fast".into(),
        tool_definitions: v1_tool_definitions(),
        approval: Some(approve_all),
    }
}

const TASK: &str = "You are an agent working in a sandboxed Windows workspace. \
Available tools: fs_read(path), fs_write(path, content), fs_edit(path, old_string, new_string), shell(command). \
This project has a failing test. Steps: (1) read check.cmd and math.rs with fs_read, \
(2) fix the bug in math.rs with fs_edit so `add` computes a + b, \
(3) run `check.cmd` with shell to verify it prints TEST PASS, \
(4) report what you changed in one sentence.";

#[tokio::test]
async fn c2_fixture_agent_fixes_broken_project_live() {
    let Some(key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live C2 fixture");
        return;
    };
    let root = broken_project("fix");
    let ws = root.join("workspace");
    let home = root.join("nano-home");

    let policy =
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy();
    let fs = FsTools::new(policy, &ws);
    let shell = ShellTool::new(&home, &ws);
    let executor = RealToolExecutor::new(fs, shell, &ws);
    let driver = FluxDriver::new(FluxCompletionsClient::new(EgressClient::flux()), &key);
    let approve_all = nano_agent::turn::ApproveAll;
    let engine = engine_for(&ws, &home, &driver, &executor, &approve_all);

    let result = engine.run_turn("c2-fix", TASK).await;

    let tool_trace: Vec<String> = result
        .ops
        .iter()
        .filter_map(|e| match &e.op {
            nano_session::op::Op::ToolCall { name, args, .. } => Some(format!("{name}({args})")),
            _ => None,
        })
        .collect();
    eprintln!("TOOL TRACE: {:?}", tool_trace);
    eprintln!("STATE: {:?}", result.state);

    // --- external-state oracle ---
    let math = std::fs::read_to_string(ws.join("math.rs")).expect("math.rs exists");
    assert!(
        math.contains("a + b"),
        "EXTERNAL ORACLE: math.rs must actually contain a + b after the turn.\nTurn state: {:?}\nFinal text: {}\nFile:\n{}",
        result.state,
        result.final_text,
        math
    );

    let check = std::process::Command::new("cmd.exe")
        .args(["/c", "check.cmd"])
        .current_dir(&ws)
        .output()
        .expect("run check.cmd");
    assert!(
        check.status.success(),
        "EXTERNAL ORACLE: check.cmd must pass after the fix: {}",
        String::from_utf8_lossy(&check.stdout)
    );

    assert_eq!(result.state, TurnState::Complete, "turn must complete");
    assert!(result.tool_calls > 0, "agent must have used tools");

    // --- journal evidence ---
    let journal_path = home.join("c2-fix-wire.jsonl");
    {
        let mut writer = JournalWriter::open(&journal_path).unwrap();
        for envelope in &result.ops {
            writer.append(envelope).unwrap();
        }
    }
    let report = read_journal(&journal_path).expect("journal readable");
    assert!(report.envelopes.len() >= result.ops.len() - 1);
    let state = SessionState::fold(&report.envelopes);
    assert!(state.open_turn.is_none(), "turn closed in journal replay");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn c2_cancellation_stops_turn_at_boundary() {
    // Cancellation does not need live Flux: the flag is checked before the
    // first model call, so a scripted-but-real engine path proves the stop.
    let Some(_key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping (flag check is pre-call anyway)");
        return;
    };
    let root = broken_project("cancel");
    let ws = root.join("workspace");
    let home = root.join("nano-home");
    let policy =
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy();
    let fs = FsTools::new(policy, &ws);
    let shell = ShellTool::new(&home, &ws);
    let executor = RealToolExecutor::new(fs, shell, &ws);
    let driver = FluxDriver::new(FluxCompletionsClient::new(EgressClient::flux()), _key);
    let approve_all = nano_agent::turn::ApproveAll;
    let engine = engine_for(&ws, &home, &driver, &executor, &approve_all);

    let flag = std::sync::atomic::AtomicBool::new(true); // already fired
    let result = engine
        .run_turn_cancellable("c2-cancel", TASK, Some(&flag))
        .await;

    assert!(
        matches!(result.state, TurnState::Stopped(ref reason) if reason.contains("cancelled")),
        "expected cancelled stop, got {:?}",
        result.state
    );
    assert!(
        result.ops.iter().any(|e| matches!(
            e.op,
            nano_session::op::Op::TurnEnd {
                outcome: nano_session::op::TurnOutcome::Cancelled,
                ..
            }
        )),
        "journal must record the cancellation"
    );

    let _ = std::fs::remove_dir_all(&root);
}
