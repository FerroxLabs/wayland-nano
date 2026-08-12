//! L3 (design doc §6/D3): the REAL nano-tui binary under portable-pty
//! (ConPTY on Windows) against the scripted fake-host binary — the intended
//! acceptance environment for the stock-crossterm Windows delta (named
//! risk C3: any stock-vs-fork behavioral delta on Windows surfaces here
//! first).
//!
//! Gate: the binaries are present (CI always builds them — the gate is NOT
//! an optional skip; a missing binary is a loud failure). ConPTY timing
//! flakes are handled by retry-with-backoff, never by skipping.

use std::io::Read as _;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use portable_pty::CommandBuilder;
use portable_pty::PtySize;
use portable_pty::native_pty_system;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/pty_journey.ndjson"
);
const ATTEMPTS: u32 = 3;
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(45);

fn require_binary(path: &str) {
    assert!(
        std::path::Path::new(path).exists(),
        "L3 gate: binary {path} must exist (CI builds it) — this is a loud failure, not a skip"
    );
}

/// One full attempt: spawn → compose → send → streamed answer → /quit →
/// clean exit. Returns the failure reason on flake.
fn attempt() -> Result<(), String> {
    let tui = env!("CARGO_BIN_EXE_nano-tui");
    let fake_host = env!("CARGO_BIN_EXE_nano-tui-fake-host");
    require_binary(tui);
    require_binary(fake_host);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let mut cmd = CommandBuilder::new(tui);
    cmd.env("NANO_TUI_ACP_HOST", fake_host);
    cmd.env("NANO_TUI_FAKE_HOST_SCRIPT", SCRIPT);
    cmd.env(
        "NANO_HOME",
        std::env::temp_dir().join(format!("nano-tui-l3-{}", std::process::id())),
    );
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn nano-tui: {e}"))?;
    drop(pair.slave);

    let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&output);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("pty reader: {e}"))?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(&buf[..n]),
            }
        }
    });
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("pty writer: {e}"))?;

    let screen_text = |parser: &mut vt100::Parser| -> String {
        let bytes = output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        *parser = vt100::Parser::new(24, 80, 0);
        parser.process(&bytes);
        parser.screen().contents()
    };

    let deadline = Instant::now() + ATTEMPT_TIMEOUT;
    let mut parser = vt100::Parser::new(24, 80, 0);
    let mut stage = 0;
    let mut dsr_answered = 0usize;
    // Stage machine: 0 wait for session ready → type prompt; 1 wait for the
    // streamed answer → type /quit; 2 wait for exit.
    while Instant::now() < deadline {
        // ConPTY emits a Device Status Report (ESC[6n) at startup and blocks
        // the child until the "terminal" answers (real terminals answer
        // instantly; under the test WE are the terminal).
        let pending_dsr = {
            let bytes = output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bytes.windows(4).filter(|w| *w == b"\x1b[6n").count()
        };
        while dsr_answered < pending_dsr {
            writer
                .write_all(b"\x1b[1;1R")
                .and_then(|()| writer.flush())
                .map_err(|e| format!("pty DSR answer: {e}"))?;
            dsr_answered += 1;
        }
        let screen = screen_text(&mut parser);
        // Fast fail with diagnostics when the TUI dies before finishing.
        if stage < 2
            && let Ok(Some(status)) = child.try_wait()
        {
            return Err(format!(
                "nano-tui exited early at stage {stage} ({status:?}); screen:\n{screen}"
            ));
        }
        match stage {
            0 if screen.contains("session ready") => {
                writer
                    .write_all(b"Say hello.\r")
                    .and_then(|()| writer.flush())
                    .map_err(|e| format!("pty write prompt: {e}"))?;
                stage = 1;
            }
            1 if screen.contains("Hello from Wayland Nano.") => {
                writer
                    .write_all(b"/quit\r")
                    .and_then(|()| writer.flush())
                    .map_err(|e| format!("pty write /quit: {e}"))?;
                stage = 2;
            }
            2 => {
                if let Ok(Some(status)) = child.try_wait() {
                    if status.success() {
                        return Ok(());
                    }
                    return Err(format!("nano-tui exited {status:?}: {screen}"));
                }
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let screen = screen_text(&mut parser);
    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "timed out at stage {stage}; final screen:\n{screen}"
    ))
}

#[test]
fn l3_pty_smoke_compose_send_stream_quit() {
    let mut last_err = String::new();
    for attempt_n in 1..=ATTEMPTS {
        match attempt() {
            Ok(()) => return,
            Err(err) => {
                // Retry-with-backoff for ConPTY timing flakes — loudly.
                eprintln!("l3 attempt {attempt_n}/{ATTEMPTS} failed: {err}");
                last_err = err;
                std::thread::sleep(Duration::from_millis(250 * attempt_n as u64));
            }
        }
    }
    panic!("l3 pty smoke failed after {ATTEMPTS} attempts: {last_err}");
}
