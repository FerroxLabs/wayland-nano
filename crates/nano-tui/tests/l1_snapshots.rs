//! L1 (design doc §6): widget-level assertions on ratatui's TestBackend
//! with insta snapshots of rendered frames (codex mandates insta for UI
//! changes — the snapshot files are the reviewable UI diff).

mod support;

use crossterm::event::KeyCode;
use nano_tui::app::App;
use nano_tui::render;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use support::World;

const LIFECYCLE: &str = include_str!("fixtures/adversarial/lifecycle_only.ndjson");

/// Render the app into a TestBackend frame and return its text.
fn render_to_test_backend(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render::render(frame, app))
        .expect("draw");
    terminal.backend().to_string()
}

#[test]
fn snapshot_transcript_cells() {
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    world.app.transcript.push_user("read the README");
    world.app.transcript.push_agent_chunk("The README says…");
    world.app.transcript.commit_active();
    world.app.transcript.push_tool_call(
        "call-1",
        "fs_read",
        "in_progress",
        "{\"path\":\"README.md\"}",
    );
    world
        .app
        .transcript
        .push_tool_result("call-1", "completed", "# Wayland Nano", None);
    world.app.transcript.push_note("doctor: 0 fail, 1 warn");
    insta::assert_snapshot!(render_to_test_backend(&world.app, 80, 24));
    world.finish();
}

#[test]
fn snapshot_streaming_active_cell() {
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    world
        .app
        .transcript
        .push_agent_chunk("streaming answer so far");
    insta::assert_snapshot!(render_to_test_backend(&world.app, 60, 12));
    world.finish();
}

#[test]
fn snapshot_approval_modal() {
    let script = format!(
        "{}{}",
        LIFECYCLE,
        concat!(
            "{\"dir\":\">\",\"frame\":{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{}}}\n",
            "{\"dir\":\"<\",\"frame\":{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/request_permission\",\"params\":{\"sessionId\":\"wayland-nano-session-recorded\",\"toolCall\":{\"toolCallId\":\"call-1\",\"title\":\"fs_write\",\"rawInput\":{\"path\":\"note.txt\",\"content\":\"hello\"}},\"options\":[{\"optionId\":\"allow\",\"kind\":\"allow_once\",\"name\":\"Allow once\"},{\"optionId\":\"deny\",\"kind\":\"reject_once\",\"name\":\"Deny\"}]}}}\n",
            "{\"dir\":\">\",\"frame\":{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"outcome\":{\"outcome\":\"selected\",\"optionId\":\"deny\"}}}}\n",
            "{\"dir\":\"<\",\"frame\":{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"stopReason\":\"end_turn\"}}}\n",
        )
    );
    let mut world = World::new(&script, 80, 24, None);
    world.type_and_submit("write note");
    insta::assert_snapshot!(render_to_test_backend(&world.app, 80, 24));
    // Esc = deny (explicit decision), keyed by the request id.
    world.key(KeyCode::Esc);
    assert_eq!(world.conn.host.decisions.len(), 1);
    assert_eq!(world.conn.host.decisions[0].option_id, "deny");
    world.finish();
}

#[test]
fn snapshot_model_picker() {
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    world.type_and_submit("/model");
    insta::assert_snapshot!(render_to_test_backend(&world.app, 80, 24));
    // Esc cancels the picker without a set_model.
    world.key(KeyCode::Esc);
    assert!(!world.app.modal_open());
    world.finish();
}

#[test]
fn snapshot_pager_scrolled_up() {
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    for i in 0..30 {
        world.app.transcript.push_note(&format!("line {i}"));
    }
    world.key(KeyCode::PageUp);
    insta::assert_snapshot!(render_to_test_backend(&world.app, 60, 12));
    world.finish();
}

/// A malformed slash command renders the help note, never executes.
#[test]
fn unknown_slash_command_is_a_note() {
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    world.type_and_submit("/bogus");
    let screen = render_to_test_backend(&world.app, 80, 24);
    assert!(screen.contains("unknown command: /bogus"), "{screen}");
    world.finish();
}

/// C7: typed error cells — one icon per class (✖ terminal / ↻ retryable /
/// ⛔ policy-denial), table title + hint, code label.
#[test]
fn snapshot_error_cells() {
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    world.app.transcript.push_error(
        "Rate limited",
        "Retrying automatically; wait a moment",
        "-32603",
        Some(nano_session::NanoErrorKind::ModelRateLimited),
        true,
    );
    world.app.transcript.push_error(
        "Denied by user",
        "",
        "-32603",
        Some(nano_session::NanoErrorKind::ApprovalDenied),
        false,
    );
    // Unknown/future kind: generic terminal cell, static title (design §4).
    world
        .app
        .transcript
        .push_error("Request failed", "", "-32603", None, false);
    insta::assert_snapshot!(render_to_test_backend(&world.app, 80, 24));
    world.finish();
}
