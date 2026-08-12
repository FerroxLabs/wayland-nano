//! C10 AGENT-UX PACK — TUI adversarial proof (proof-owner file, post-build).
//! Leg 8 TUI drive of shared/reviews/panel-tui/C10-proof-plan.md: the real
//! App + real render path on a VT100 terminal against scripted fake hosts
//! (hand-authored fixtures, C2(a)-linted against the recorded corpus):
//!
//! - todo lifecycle visible: a `todo` tool_call frame is tracked, the status
//!   line shows the open count, and /todo prints the list;
//! - diff rendering: a tool_call_update diff content block renders as the
//!   human-facing ± block;
//! - ask_user question round-trip: the minted-options request opens the
//!   modal, the user's selection rides back as the minted optionId;
//! - F-C10-2 pinned: the modal's scroll window is item-indexed while each
//!   option renders as two rows, so the 4th option (Dismiss) is clipped out
//!   of the viewport — selectable blind, and Esc always maps to reject;
//! - OSC 9: sink-level emission is sanitized; the NANO_TUI_NOTIFY off-switch
//!   emits nothing (terminal-level capture is impossible in a VT100 harness
//!   — notify_terminal gates on stderr.is_terminal() by design).
//!
//! (/plan entry + exit over the wire is already driven end-to-end in
//! l2.rs's l2_acceptance_full_journey over the RECORDED host transcript —
//! step (d2). This file covers the surfaces that journey does not.)
//!
//! HARNESS NOTE: the fake host remaps every response frame after a client
//! permission ANSWER to id null (fake_host.rs:159-163 passes Value::Null as
//! the request id), so the trailing prompt response of a question turn is
//! dropped by the app as malformed and the turn never completes — a
//! pre-existing harness quirk shared with the recorded full_journey (whose
//! approval turn's prompt response is nulled the same way; l2 simply never
//! depends on it). The question fixtures below therefore end at the answer
//! effects; turn COMPLETION after a question is proven against the real
//! host in nano-cli's c10 suites (ask_user_* finish_prompt round-trips).

mod support;

use crossterm::event::KeyCode;
use nano_tui::notify::{Notifier, NotifyKind};
use support::{World, lint_fixture};

const RECORDED: &str = include_str!("fixtures/full_journey.ndjson");

/// todo tracking + /todo + diff rendering (no permission requests, so turn
/// lifecycle is intact in this fixture).
const TODO_DIFF_JOURNEY: &str = r#"
{"dir":">","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"wayland-nano","version":"0.1.0"},"agentCapabilities":{"loadSession":true,"promptCapabilities":{"text":true,"image":false,"embeddedContext":false},"mcpCapabilities":{"http":false,"sse":false}}}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/recorded-workspace","mcpServers":[]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"wayland-nano-session-recorded","models":{"availableModels":[{"modelId":"flux-auto","name":"Flux Auto"}],"currentModelId":"flux-auto"},"modes":{"availableModes":[{"id":"read_only","name":"Read Only"},{"id":"default","name":"Default"},{"id":"full_auto","name":"Full Auto"},{"id":"plan","name":"Plan"}],"currentModeId":"default"}}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"wayland-nano-session-recorded","prompt":[{"type":"text","text":"track tasks"}]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call","toolCallId":"td1","title":"todo","status":"in_progress","rawInput":{"todos":[{"id":"t1","content":"write tests","status":"in_progress"},{"id":"t2","content":"ship it","status":"pending"}]}}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call_update","toolCallId":"td1","status":"completed","rawOutput":"2 item(s)"}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Tracked two tasks."}}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{"sessionId":"wayland-nano-session-recorded","prompt":[{"type":"text","text":"edit it"}]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call","toolCallId":"e1","title":"fs_edit","status":"in_progress","kind":"edit","rawInput":{"path":"src/main.rs","old_string":"old line","new_string":"new line"}}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call_update","toolCallId":"e1","status":"completed","rawOutput":"len:16","content":[{"type":"diff","path":"src/main.rs","oldText":"old line","newText":"new line"}]}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Edited."}}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}}
"#;

/// The ask_user question round-trip (ends at the answer effects — see the
/// harness note above).
const QUESTION_JOURNEY: &str = r#"
{"dir":">","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"wayland-nano","version":"0.1.0"},"agentCapabilities":{"loadSession":true,"promptCapabilities":{"text":true,"image":false,"embeddedContext":false},"mcpCapabilities":{"http":false,"sse":false}}}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/recorded-workspace","mcpServers":[]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"wayland-nano-session-recorded","models":{"availableModels":[{"modelId":"flux-auto","name":"Flux Auto"}],"currentModelId":"flux-auto"},"modes":{"availableModes":[{"id":"read_only","name":"Read Only"},{"id":"default","name":"Default"},{"id":"full_auto","name":"Full Auto"},{"id":"plan","name":"Plan"}],"currentModeId":"default"}}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"wayland-nano-session-recorded","prompt":[{"type":"text","text":"ask me"}]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call","toolCallId":"q1","title":"ask_user","status":"in_progress","rawInput":{"question":"Which approach?","options":[{"label":"Refactor"},{"label":"Rewrite"},{"label":"Defer"}]}}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":50,"method":"session/request_permission","params":{"sessionId":"wayland-nano-session-recorded","toolCall":{"toolCallId":"q1","title":"Which approach?","rawInput":{"question":"Which approach?","options":[{"label":"Refactor"},{"label":"Rewrite"},{"label":"Defer"}]}},"options":[{"optionId":"opt_0","kind":"allow_once","name":"Refactor"},{"optionId":"opt_1","kind":"allow_once","name":"Rewrite"},{"optionId":"opt_2","kind":"allow_once","name":"Defer"},{"optionId":"reject","kind":"reject_once","name":"Dismiss"}]}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":50,"result":{"outcome":{"outcome":"selected","optionId":"opt_1"}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call_update","toolCallId":"q1","status":"completed","rawOutput":"Rewrite"}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"You chose Rewrite."}}}}}
"#;

/// The four-option question for the F-C10-2 viewport pin (ends at the
/// answer — see the harness note above).
const DISMISS_JOURNEY: &str = r#"
{"dir":">","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentInfo":{"name":"wayland-nano","version":"0.1.0"},"agentCapabilities":{"loadSession":true,"promptCapabilities":{"text":true,"image":false,"embeddedContext":false},"mcpCapabilities":{"http":false,"sse":false}}}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/recorded-workspace","mcpServers":[]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"wayland-nano-session-recorded","models":{"availableModels":[{"modelId":"flux-auto","name":"Flux Auto"}],"currentModelId":"flux-auto"},"modes":{"availableModes":[{"id":"read_only","name":"Read Only"},{"id":"default","name":"Default"},{"id":"full_auto","name":"Full Auto"},{"id":"plan","name":"Plan"}],"currentModeId":"default"}}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"wayland-nano-session-recorded","prompt":[{"type":"text","text":"ask again"}]}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call","toolCallId":"q2","title":"ask_user","status":"in_progress","rawInput":{"question":"Pick a color?","options":[{"label":"Red"},{"label":"Blue"},{"label":"Green"}]}}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":51,"method":"session/request_permission","params":{"sessionId":"wayland-nano-session-recorded","toolCall":{"toolCallId":"q2","title":"Pick a color?","rawInput":{"question":"Pick a color?","options":[{"label":"Red"},{"label":"Blue"},{"label":"Green"}]}},"options":[{"optionId":"opt_0","kind":"allow_once","name":"Red"},{"optionId":"opt_1","kind":"allow_once","name":"Blue"},{"optionId":"opt_2","kind":"allow_once","name":"Green"},{"optionId":"reject","kind":"reject_once","name":"Dismiss"}]}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":51,"result":{"outcome":{"outcome":"selected","optionId":"reject"}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"wayland-nano-session-recorded","update":{"sessionUpdate":"tool_call_update","toolCallId":"q2","status":"failed","rawOutput":"ask_user: dismissed by the user; proceed without asking"}}}}
"#;

/// todo lifecycle visible + diff rendering after an edit.
#[test]
fn c10_tui_todo_lifecycle_and_diff_rendering() {
    lint_fixture(TODO_DIFF_JOURNEY, &[RECORDED], &[])
        .expect("fixture kinds bound to the recorded corpus");
    let mut world = World::new(TODO_DIFF_JOURNEY, 100, 30, None);
    let screen = world.screen();
    assert!(screen.contains("session ready"), "handshake: {screen}");

    // ── todo lifecycle visible ─────────────────────────────────────────
    world.type_and_submit("track tasks");
    let screen = world.screen();
    assert!(screen.contains("todo"), "todo tool card: {screen}");
    // The status line tracks the open count from the wire frames (both
    // items are pending/in_progress → 2 open).
    let status = screen.lines().last().unwrap_or_default();
    assert!(
        status.contains("todo: 2 open"),
        "status-line todo count: {status}"
    );
    // /todo prints the tracked list.
    world.type_and_submit("/todo");
    let screen = world.screen();
    assert!(screen.contains("todo list (2 item(s))"), "/todo: {screen}");
    assert!(screen.contains("write tests"), "item 1: {screen}");
    assert!(screen.contains("ship it"), "item 2: {screen}");

    // ── diff rendering after an edit ───────────────────────────────────
    world.type_and_submit("edit it");
    let screen = world.screen();
    assert!(
        screen.contains("± src/main.rs (+1/-1)"),
        "diff summary line: {screen}"
    );
    assert!(screen.contains("- old line"), "removed line: {screen}");
    assert!(screen.contains("+ new line"), "added line: {screen}");

    world.finish();
}

/// The ask_user question round-trip: the minted options render in the
/// modal and the selected option's MINTED ID rides back on the wire.
#[test]
fn c10_tui_question_modal_round_trip() {
    lint_fixture(QUESTION_JOURNEY, &[RECORDED], &[])
        .expect("fixture kinds bound to the recorded corpus");
    let mut world = World::new(QUESTION_JOURNEY, 100, 30, None);
    world.type_and_submit("ask me");
    let screen = world.screen();
    assert!(screen.contains("Which approach?"), "question modal: {screen}");
    assert!(screen.contains("Refactor"), "option 1: {screen}");
    assert!(screen.contains("Rewrite"), "option 2: {screen}");
    assert!(screen.contains("Defer"), "option 3: {screen}");
    // Select the SECOND option: its minted id rides back on the wire.
    world.key(KeyCode::Down);
    world.key(KeyCode::Enter);
    let decisions = &world.conn.host.decisions;
    assert_eq!(decisions.len(), 1, "one answer sent");
    assert_eq!(
        decisions[0].option_id, "opt_1",
        "the minted id of the selected label rode the answer channel"
    );
    let screen = world.screen();
    assert!(
        screen.contains("approval decision: opt_1"),
        "decision note: {screen}"
    );
    assert!(
        screen.contains("You chose Rewrite."),
        "the resolved label flowed into the turn: {screen}"
    );
    world.finish();
}

/// FINDING F-C10-2 (pinned, minor): the question modal's scroll window is
/// computed in ITEMS (render.rs:303-307, `skip(start).take(visible)` with
/// `start` derived from the selected index) while each option renders as
/// TWO rows (name + kind description), so with 4 options (3 minted +
/// Dismiss) the Dismiss row is clipped out of the 5-row viewport and never
/// becomes visible — even when selected. The option still WORKS blind (and
/// Esc maps to the reject id), proven here: Down×3 selects the invisible
/// Dismiss and the reject id rides the wire.
#[test]
fn c10_tui_question_dismiss_viewport_pin() {
    lint_fixture(DISMISS_JOURNEY, &[RECORDED], &[])
        .expect("fixture kinds bound to the recorded corpus");
    let mut world = World::new(DISMISS_JOURNEY, 100, 30, None);
    world.type_and_submit("ask again");
    let screen = world.screen();
    assert!(screen.contains("Pick a color?"), "modal: {screen}");
    assert!(screen.contains("Red"), "option 1: {screen}");
    world.key(KeyCode::Down);
    world.key(KeyCode::Down);
    world.key(KeyCode::Down);
    let screen = world.screen();
    assert!(
        !screen.contains("Dismiss"),
        "F-C10-2 pinned: the 4th option never renders in the viewport"
    );
    world.key(KeyCode::Enter); // blind-select the invisible Dismiss
    let decisions = &world.conn.host.decisions;
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].option_id, "reject",
        "the invisible Dismiss still answers correctly"
    );
    let screen = world.screen();
    assert!(
        screen.contains("approval decision: reject"),
        "dismiss decision note: {screen}"
    );
    world.finish();
}

// ── OSC 9 (leg 7) ────────────────────────────────────────────────────────

/// The emission path end-to-end at sink level: a hostile payload is
/// sanitized (no BEL/ESC smuggled through), the sequence lands on the sink,
/// and the NANO_TUI_NOTIFY off-switch emits nothing. Terminal-level capture
/// is impossible in a VT100 harness — notify_terminal gates on
/// stderr.is_terminal() by design. One test owns the env mutations (no
/// parallel-reader race: nothing else in the tree reads these vars).
#[test]
fn osc9_emission_sanitizes_and_the_off_switch_emits_nothing() {
    unsafe {
        std::env::remove_var("NANO_TUI_NOTIFY");
        std::env::remove_var("TMUX");
    }
    let notifier = Notifier::from_env();
    let mut sink: Vec<u8> = Vec::new();
    notifier.notify(
        &mut sink,
        NotifyKind::TurnComplete,
        "done\x07\x1b]8;;evil\x1b\\",
    );
    let text = String::from_utf8(sink).expect("utf8");
    assert!(text.starts_with("\x1b]9;"), "OSC 9 opener: {text:?}");
    assert_eq!(text.matches('\x07').count(), 1, "one BEL terminator");
    assert_eq!(text.matches('\x1b').count(), 1, "no smuggled ESC");
    assert!(!text.contains("evil"), "hostile payload stripped");

    // The off-switch: NANO_TUI_NOTIFY=0|off|false emits nothing.
    for value in ["0", "off", "false"] {
        unsafe { std::env::set_var("NANO_TUI_NOTIFY", value) };
        let notifier = Notifier::from_env();
        assert!(
            notifier.encode(NotifyKind::TurnComplete, "x").is_empty(),
            "NANO_TUI_NOTIFY={value} emits nothing"
        );
        let mut sink: Vec<u8> = Vec::new();
        notifier.notify(&mut sink, NotifyKind::PermissionPending, "x");
        assert!(sink.is_empty());
    }
    unsafe { std::env::remove_var("NANO_TUI_NOTIFY") };
    let notifier = Notifier::from_env();
    assert!(
        !notifier.encode(NotifyKind::TurnComplete, "x").is_empty(),
        "default is enabled"
    );
}
