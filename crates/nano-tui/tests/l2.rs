//! L2 (design doc §6): full-frame integration through the real app logic
//! and the real render path, on a VT100 virtual terminal, against scripted
//! fake acp-hosts whose frames come from RECORDED real-host transcripts
//! (corpus binding C2(b); hand-authored adversarial fixtures pass the C2(a)
//! lint). No Flux key, no sandbox, no network.

mod support;

use crossterm::event::KeyCode;
use nano_tui::event::AppEvent;
use support::World;
use support::lint_fixture;

const FULL_JOURNEY: &str = include_str!("fixtures/full_journey.ndjson");
const PTY_JOURNEY: &str = include_str!("fixtures/pty_journey.ndjson");
const LIFECYCLE: &str = include_str!("fixtures/adversarial/lifecycle_only.ndjson");
const SPLIT_ESCAPE: &str = include_str!("fixtures/adversarial/split_escape.ndjson");
const APPROVAL_SPOOF: &str = include_str!("fixtures/adversarial/approval_spoof.ndjson");
const TORN_RESUME: &str = include_str!("fixtures/adversarial/torn_resume.ndjson");
const CANCEL_MID_TURN: &str = include_str!("fixtures/adversarial/cancel_mid_turn.ndjson");
const COMPACT: &str = include_str!("fixtures/adversarial/compact.ndjson");
const PAGED_READ: &str = include_str!("fixtures/adversarial/paged_read.ndjson");
const WEB_FETCH: &str = include_str!("fixtures/adversarial/web_fetch.ndjson");

/// The recorded full journey is two lifetimes in one file: phase 1 ends at
/// the second `initialize` expectation (the relaunch). Split there.
fn split_journey(text: &str) -> (String, String) {
    let mut phase1 = String::new();
    let mut phase2 = String::new();
    let mut seen_initialize = 0;
    for line in text.lines() {
        if line.contains("\"method\":\"initialize\"") {
            seen_initialize += 1;
        }
        if seen_initialize < 2 {
            phase1.push_str(line);
            phase1.push('\n');
        } else {
            phase2.push_str(line);
            phase2.push('\n');
        }
    }
    assert_eq!(seen_initialize, 2, "journey must contain two lifetimes");
    (phase1, phase2)
}

fn lint_all_adversarial() {
    let recorded = &[FULL_JOURNEY, PTY_JOURNEY];
    for (name, text, extra) in [
        ("lifecycle_only", LIFECYCLE, &[][..]),
        ("split_escape", SPLIT_ESCAPE, &[][..]),
        ("approval_spoof", APPROVAL_SPOOF, &[][..]),
        ("torn_resume", TORN_RESUME, &["plan"][..]),
        ("cancel_mid_turn", CANCEL_MID_TURN, &[][..]),
    ] {
        lint_fixture(text, recorded, extra)
            .unwrap_or_else(|e| panic!("adversarial fixture {name} failed the lint: {e}"));
    }
}

/// The normative combined acceptance scenario (design doc §6/D3), over the
/// RECORDED real-host transcript:
/// (a) prompt → streamed agent_message_chunk renders incrementally;
/// (b) session/request_permission → approved via the modal → tool result;
/// (c) /model → picker → session/set_model → status line updates;
/// (d) /mode → picker → session/set_mode ×2 → status line updates (C2);
/// (e) kill + relaunch → session/load replay renders the prior transcript,
///     with the mode back at `default` (never resurrected, C2 panel Q5).
#[test]
fn l2_acceptance_full_journey() {
    lint_all_adversarial();
    let (phase1, phase2) = split_journey(FULL_JOURNEY);

    // ── lifetime 1 ────────────────────────────────────────────────────
    let mut world = World::new(&phase1, 80, 24, None);
    let screen = world.screen();
    assert!(screen.contains("session ready"), "handshake: {screen}");
    assert!(screen.contains("flux-auto"), "status line model: {screen}");

    // (a) compose → send → streamed chunk renders incrementally.
    for c in "Say hello.".chars() {
        world.key(KeyCode::Char(c));
    }
    world.event_no_pump(AppEvent::Key(crossterm::event::KeyEvent::new(
        KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    )));
    // The chunk frame arrives BEFORE the prompt response: first the live
    // streaming cell...
    assert!(world.pump_one(), "chunk frame queued");
    let screen = world.screen();
    assert!(
        screen.contains("Hello from Wayland Nano."),
        "chunk: {screen}"
    );
    assert!(screen.contains('▌'), "active streaming marker: {screen}");
    // ...then the response commits it.
    assert!(world.pump_one(), "prompt response queued");
    let screen = world.screen();
    assert!(
        screen.contains("Hello from Wayland Nano."),
        "committed: {screen}"
    );
    assert!(!screen.contains('▌'), "marker gone after commit: {screen}");
    assert!(screen.contains("Say hello."), "user echo: {screen}");

    // (b) the approval flow: modal → explicit decision → tool result.
    world.type_and_submit("Write note.txt.");
    let screen = world.screen();
    assert!(
        screen.contains("Approve? fs_write"),
        "approval modal: {screen}"
    );
    assert!(screen.contains("Allow once"), "wire options: {screen}");
    world.key(KeyCode::Enter); // select "Allow once"
    let screen = world.screen();
    assert!(
        screen.contains("approval decision: allow"),
        "decision note: {screen}"
    );
    assert!(
        screen.contains("fs_write [completed]"),
        "tool card: {screen}"
    );
    assert!(screen.contains("File written."), "final answer: {screen}");
    let decisions = &world.conn.host.decisions;
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].option_id, "allow");

    // (c) /model → picker → session/set_model → status line updates.
    world.type_and_submit("/model");
    let screen = world.screen();
    assert!(screen.contains("Switch model"), "picker: {screen}");
    assert!(screen.contains("Flux Fast"), "catalog: {screen}");
    world.key(KeyCode::Down);
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(
        screen.contains("model switched to flux-fast"),
        "note: {screen}"
    );
    assert!(
        screen
            .lines()
            .last()
            .is_some_and(|l| l.contains("flux-fast")),
        "status line updated: {screen}"
    );

    // (d) /mode (C2): picker over the advertised availableModes →
    // session/set_mode → status line mode slot updates; flip default →
    // full_auto → default. The picker preselects the CURRENT mode, so one
    // Down from `default` lands on `full_auto`, one Up lands back.
    world.type_and_submit("/mode");
    let screen = world.screen();
    assert!(
        screen.contains("Switch permission mode"),
        "picker: {screen}"
    );
    assert!(screen.contains("Full Auto"), "advertised modes: {screen}");
    assert!(
        screen.contains("Default (current)"),
        "current mode: {screen}"
    );
    world.key(KeyCode::Down);
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(
        screen.contains("mode switched to full_auto"),
        "note: {screen}"
    );
    assert!(
        screen
            .lines()
            .last()
            .is_some_and(|l| l.contains("full_auto")),
        "status line mode slot: {screen}"
    );
    world.type_and_submit("/mode");
    world.key(KeyCode::Up);
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(
        screen.contains("mode switched to default"),
        "note: {screen}"
    );

    // (d2) /plan (C10): sends session/set_mode {modeId:"plan"} over the
    // wire (the TUI is an ACP client — no local-only channel); the ack's
    // modes block flips the status slot to "plan" and the plan file path
    // prints for discoverability (Q5).
    world.type_and_submit("/plan");
    let screen = world.screen();
    assert!(screen.contains("mode switched to plan"), "note: {screen}");
    assert!(
        screen.contains("plan file:") && screen.contains(".plan.md"),
        "plan file path printed: {screen}"
    );
    // Back out via /mode → default: the modes list is
    // [read_only, default, full_auto, plan] with "plan" current, so two
    // Ups land on default.
    world.type_and_submit("/mode");
    world.key(KeyCode::Up);
    world.key(KeyCode::Up);
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(
        screen.contains("mode switched to default"),
        "plan exit re-advertises the privilege mode: {screen}"
    );

    // The recorded phase-1 tail (a bare session/cancel between turns) is
    // not reproducible through TUI inputs — the TUI only cancels mid-turn;
    // that path has its own scenario (l2_cancel_mid_turn).
    let remaining = world.finish_partial();
    assert_eq!(remaining, ["Notification(\"session/cancel\")".to_string()]);

    // ── lifetime 2: kill + relaunch → session/load replay (e) ─────────
    let world = World::new(
        &phase2,
        80,
        24,
        Some("wayland-nano-session-recorded".to_string()),
    );
    let screen = world.screen();
    for expected in [
        "Say hello.",
        "Hello from Wayland Nano.",
        "Write note.txt.",
        "fs_write [completed]",
        "File written.",
        "session resumed",
    ] {
        assert!(
            screen.contains(expected),
            "replay missing {expected:?}: {screen}"
        );
    }
    // C2 (panel Q5): the resumed session is back in `default` — the mode
    // was journaled as audit history, never resurrected.
    assert!(
        screen
            .lines()
            .last()
            .is_some_and(|l| l.contains("| default |")),
        "status line mode after resume: {screen}"
    );
    world.finish();
}

/// §8 escape-sequence injection, including sequences SPLIT ACROSS streamed
/// frames: nothing reaches the virtual terminal but the visible text.
#[test]
fn l2_split_frame_escape_injection() {
    lint_all_adversarial();
    let mut world = World::new(SPLIT_ESCAPE, 80, 24, None);
    world.type_and_submit("try to inject");
    let screen = world.screen();
    assert!(
        screen.contains("answer:  safe text done"),
        "visible text survives, escapes do not: {screen}"
    );
    for hostile in ["spoof title", "[38;5", "9m", "unterminated dcs"] {
        assert!(
            !screen.contains(hostile),
            "hostile payload leaked: {hostile:?}"
        );
    }
    world.finish();
}

/// §8 approval spoofing: malformed, duplicate-while-open, and replayed
/// permission requests are all auto-denied; only the one explicitly
/// approved request is allowed.
#[test]
fn l2_approval_spoofing() {
    lint_all_adversarial();
    let mut world = World::new(APPROVAL_SPOOF, 80, 24, None);
    world.type_and_submit("write it");
    // Malformed (empty options) auto-denied; valid request opened the modal;
    // duplicate auto-denied. (C7: the denial surfaces as a typed error cell.)
    let screen = world.screen();
    assert!(
        screen.contains("Malformed permission request — auto-denied"),
        "{screen}"
    );
    assert!(screen.contains("Approve? fs_write"), "modal open: {screen}");
    assert!(screen.contains("duplicate permission request"), "{screen}");
    // The explicit human decision on the one legitimate request.
    world.key(KeyCode::Enter); // Allow once
    let screen = world.screen();
    assert!(screen.contains("fs_write [completed]"), "{screen}");
    let options: Vec<&str> = world
        .conn
        .host
        .decisions
        .iter()
        .map(|d| d.option_id.as_str())
        .collect();
    assert_eq!(
        options,
        ["deny", "deny", "allow", "deny"],
        "malformed, duplicate, explicit allow, replay-deny"
    );
    world.finish();
}

/// §8 paste bombs / control injection: a bracketed paste carrying escapes
/// and C0 controls lands sanitized; the sanitized text is what ships.
#[test]
fn l2_paste_bomb() {
    lint_all_adversarial();
    let mut world = World::new(PTY_JOURNEY, 80, 24, None);
    let bomb = format!(
        "payload\u{1b}[2J\u{1b}]0;pwned\u{7}\n{}\u{0}\u{c}",
        "x".repeat(20_000)
    );
    world.event(AppEvent::Paste(bomb));
    let composer = world.app.composer.text();
    assert!(composer.starts_with("payload\n"));
    assert!(composer.contains(&"x".repeat(20_000)));
    assert!(
        !composer
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'),
        "composer must hold no controls: {:?}",
        &composer[..40]
    );
    // Submitting the pasted prompt drives the recorded streamed answer.
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(screen.contains("Hello from Wayland Nano."), "{screen}");
    world.finish();
}

/// §8 resume abuse: torn/unknown replay frames render partial transcripts
/// without panic (a hanging in_progress card, an unknown update kind, a
/// truncated escape in a chunk).
#[test]
fn l2_torn_resume() {
    lint_all_adversarial();
    let world_script = TORN_RESUME;
    let mut world = World::new(
        world_script,
        80,
        24,
        Some("wayland-nano-session-recorded".to_string()),
    );
    world.pump();
    let screen = world.screen();
    assert!(screen.contains("do the thing"), "{screen}");
    assert!(screen.contains("working on it"), "{screen}");
    assert!(
        screen.contains("fs_write [in_progress]"),
        "hanging card: {screen}"
    );
    assert!(screen.contains("partial answer"), "{screen}");
    assert!(
        !screen.contains("[31m"),
        "truncated escape leaked: {screen}"
    );
    assert!(screen.contains("session resumed"), "{screen}");
    world.finish();
}

/// §8 resize storms / zero-size terminal: no panic, redraw deferred.
#[test]
fn l2_resize_storm() {
    lint_all_adversarial();
    let mut world = World::new(LIFECYCLE, 80, 24, None);
    for (w, h) in [(0, 0), (1, 1), (200, 60), (0, 24), (80, 0), (80, 24)] {
        world.event(AppEvent::Resize(w, h));
    }
    // The app still works after the storm.
    world.type_and_submit("/status");
    let screen = world.screen();
    assert!(screen.contains("model:   flux-auto"), "{screen}");
    // Teardown: the status command fired a doctor subprocess thread — that
    // is fine for the world (its event lands on a bus nobody drains here),
    // but the script itself must be clean.
    let remaining = world.finish_partial();
    assert!(remaining.is_empty(), "{remaining:?}");
}

/// Esc mid-turn sends session/cancel; the cancelled response ends the turn.
#[test]
fn l2_cancel_mid_turn() {
    lint_all_adversarial();
    let mut world = World::new(CANCEL_MID_TURN, 80, 24, None);
    world.type_and_submit("long task");
    let screen = world.screen();
    assert!(screen.contains("partial"), "chunk rendered: {screen}");
    assert!(screen.contains("turn running"), "{screen}");
    world.key(KeyCode::Esc);
    let screen = world.screen();
    assert!(screen.contains("cancel requested"), "{screen}");
    assert!(screen.contains("turn ended: cancelled"), "{screen}");
    world.finish();
}

/// §8 key-leak canary: no fixture and no rendered screen may carry a
/// credential-shaped value; if a real key is in the environment, it must
/// appear in none of the fixtures.
#[test]
fn l2_fixtures_carry_no_credentials() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut scanned = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).expect("adversarial dir") {
                let path = entry.expect("entry").path();
                scan_fixture(&path);
                scanned += 1;
            }
        } else {
            scan_fixture(&path);
            scanned += 1;
        }
    }
    assert!(scanned >= 6, "expected fixtures to scan, got {scanned}");
}

fn scan_fixture(path: &std::path::Path) {
    let text = std::fs::read_to_string(path).expect("fixture readable");
    for marker in ["FLUX_API_KEY", "FLUX_TEST_KEY", "FLUX_API_KEY_FILE", "sk-"] {
        assert!(
            !text.contains(marker),
            "fixture {} carries credential-shaped marker {marker:?}",
            path.display()
        );
    }
    for env in ["FLUX_API_KEY", "FLUX_TEST_KEY"] {
        if let Ok(value) = std::env::var(env)
            && !value.is_empty()
        {
            assert!(
                !text.contains(&value),
                "fixture {} leaks the live {env} value",
                path.display()
            );
        }
    }
}

/// C1 §7/§9 (TUI scripted drive): /compact sends session/compact and the
/// host's compaction notices render as system notes; a notice carrying
/// escape sequences is sanitized on the render path.
#[test]
fn l2_compact_command_sends_request_and_renders_notices() {
    lint_fixture(COMPACT, &[FULL_JOURNEY, PTY_JOURNEY], &["compaction"][..])
        .unwrap_or_else(|e| panic!("compact fixture failed the lint: {e}"));
    let mut world = World::new(COMPACT, 80, 24, None);
    assert!(world.screen().contains("session ready"));
    world.type_and_submit("/compact");
    let screen = world.screen();
    assert!(
        screen.contains("context compaction: begin"),
        "begin notice renders: {screen}"
    );
    assert!(
        screen.contains("context compaction: complete"),
        "complete notice renders: {screen}"
    );
    // The hostile-status notice rendered its text but no escape sequence
    // survived the sanitizer into the terminal grid.
    assert!(screen.contains("forged"), "sanitized text kept: {screen}");
    assert!(!screen.contains('\u{1b}'), "no raw ESC on screen");
    world.finish();
}

/// C3 coverage (routed from the C3+C4 proof): a paged-read continuation
/// renders as a second fs_read tool card whose detail line carries the
/// paging cursor (line_offset + file_token) — the continuation must be
/// visible as its own card, not silently merged into the first page.
#[test]
fn l2_paged_read_continuation() {
    lint_fixture(PAGED_READ, &[FULL_JOURNEY, PTY_JOURNEY], &[][..])
        .unwrap_or_else(|e| panic!("paged_read fixture failed the lint: {e}"));
    let mut world = World::new(PAGED_READ, 80, 24, None);
    world.type_and_submit("read big.txt");
    let screen = world.screen();
    assert!(
        screen.contains("fs_read [completed]"),
        "first page card: {screen}"
    );
    assert!(
        screen.contains("\"line_offset\":1000"),
        "continuation cursor visible: {screen}"
    );
    assert!(
        screen.contains("file_token"),
        "freshness token visible: {screen}"
    );
    assert!(screen.contains("Read both pages of big.txt."), "{screen}");
    world.finish();
}

/// C4 coverage (routed from the C3+C4 proof): web_fetch allow/deny through
/// the permission modal — the allowed fetch renders a completed fetch card;
/// the denied fetch renders NO card (a gate denial never emits tool frames)
/// and the decision note names the denial.
#[test]
fn l2_web_fetch_allow_and_deny() {
    lint_fixture(WEB_FETCH, &[FULL_JOURNEY, PTY_JOURNEY], &[][..])
        .unwrap_or_else(|e| panic!("web_fetch fixture failed the lint: {e}"));
    let mut world = World::new(WEB_FETCH, 80, 24, None);
    world.type_and_submit("fetch two urls");
    let screen = world.screen();
    assert!(screen.contains("Approve? web_fetch"), "modal: {screen}");
    // Allow the first fetch.
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(
        screen.contains("approval decision: allow"),
        "decision note: {screen}"
    );
    assert!(
        screen.contains("web_fetch [completed]"),
        "allowed fetch card: {screen}"
    );
    assert!(
        screen.contains("https://example.com/"),
        "fetch url detail: {screen}"
    );
    // The second fetch parks at its own modal; deny it (Down → Deny).
    assert!(
        screen.contains("Approve? web_fetch"),
        "second modal: {screen}"
    );
    world.key(KeyCode::Down);
    world.key(KeyCode::Enter);
    let screen = world.screen();
    assert!(
        screen.contains("approval decision: deny"),
        "denial note: {screen}"
    );
    assert!(
        !screen.contains("tracker.invalid"),
        "a denied call emits no tool frames — no card, no url: {screen}"
    );
    assert!(
        screen.contains("Fetched example.com; the second fetch was denied."),
        "final answer: {screen}"
    );
    let options: Vec<&str> = world
        .conn
        .host
        .decisions
        .iter()
        .map(|d| d.option_id.as_str())
        .collect();
    assert_eq!(options, ["allow", "deny"]);
    world.finish();
}
