//! C7 proof-lane PTY drive (agent-30): the REAL nano-tui under ConPTY
//! against the scripted fake host, fixture `c7_typed_error.ndjson`:
//! prompt 1 streams a partial answer then fails with a typed
//! `model_rate_limited` error response — the partial text must stay and the
//! table-titled error cell must append; prompt 2 fails with a FUTURE
//! nanoError.kind — the TUI must render the generic terminal cell, never
//! the wire message text. Modeled on tests/pty.rs (same DSR handling).

use std::io::Read as _;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/c7_typed_error.ndjson"
);
const ATTEMPTS: u32 = 3;
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(45);

fn attempt() -> Result<(), String> {
    let tui = env!("CARGO_BIN_EXE_nano-tui");
    let fake_host = env!("CARGO_BIN_EXE_nano-tui-fake-host");

    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let mut cmd = portable_pty::CommandBuilder::new(tui);
    cmd.env("NANO_TUI_ACP_HOST", fake_host);
    cmd.env("NANO_TUI_FAKE_HOST_SCRIPT", SCRIPT);
    cmd.env(
        "NANO_HOME",
        std::env::temp_dir().join(format!("nano-tui-c7pty-{}", std::process::id())),
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
    // 0: wait ready → prompt "first"; 1: wait partial text + typed cell →
    // prompt "second"; 2: wait generic terminal cell → /quit; 3: exit.
    while Instant::now() < deadline {
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
        if stage < 3
            && let Ok(Some(status)) = child.try_wait()
        {
            return Err(format!(
                "nano-tui exited early at stage {stage} ({status:?}); screen:\n{screen}"
            ));
        }
        match stage {
            0 if screen.contains("session ready") => {
                writer
                    .write_all(b"first\r")
                    .and_then(|()| writer.flush())
                    .map_err(|e| format!("pty write prompt 1: {e}"))?;
                stage = 1;
            }
            1 if screen.contains("Partial answer before the failure.")
                && screen.contains("Rate limited") =>
            {
                // Partial-stream-then-fail on the TUI: the partial text is
                // kept and the typed error cell carries the TABLE title.
                writer
                    .write_all(b"second\r")
                    .and_then(|()| writer.flush())
                    .map_err(|e| format!("pty write prompt 2: {e}"))?;
                stage = 2;
            }
            2 if screen.contains("Request failed") => {
                // The future kind rendered the generic terminal cell. The
                // hostile wire text must be nowhere on the screen.
                if screen.contains("FUTURE-KIND-WIRE-TEXT") {
                    return Err(format!("wire free-text reached the screen:\n{screen}"));
                }
                writer
                    .write_all(b"/quit\r")
                    .and_then(|()| writer.flush())
                    .map_err(|e| format!("pty write /quit: {e}"))?;
                stage = 3;
            }
            3 => {
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
fn c7_pty_typed_error_and_future_kind() {
    let mut last_err = String::new();
    for attempt_n in 1..=ATTEMPTS {
        match attempt() {
            Ok(()) => return,
            Err(err) => {
                eprintln!("c7 pty attempt {attempt_n}/{ATTEMPTS} failed: {err}");
                last_err = err;
                std::thread::sleep(Duration::from_millis(250 * attempt_n as u64));
            }
        }
    }
    panic!("c7 pty typed-error drive failed after {ATTEMPTS} attempts: {last_err}");
}
