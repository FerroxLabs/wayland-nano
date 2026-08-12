//! nano-tui-fake-host — test-support binary (NOT shipped runtime): a
//! scripted fake acp-host speaking NDJSON over stdio. Used by the L3 PTY
//! smoke (tests/pty.rs) so the real nano-tui binary can run its full
//! terminal stack (ConPTY on Windows) without a Flux key or network.
//!
//! Script source: $NANO_TUI_FAKE_HOST_SCRIPT (path), or argv[1].
//! Exit 0 on clean EOF with the script exhausted; 2 on a script violation
//! (loud — a drift between TUI and fixture must never pass silently).

use std::io::BufRead as _;
use std::io::Write as _;

use nano_tui::fake_host::FakeHost;

fn main() {
    let code = run();
    std::process::exit(code);
}

fn run() -> i32 {
    let script_path = std::env::var_os("NANO_TUI_FAKE_HOST_SCRIPT")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::args().nth(1).map(std::path::PathBuf::from));
    let Some(script_path) = script_path else {
        eprintln!("nano-tui-fake-host: no script (NANO_TUI_FAKE_HOST_SCRIPT or argv[1])");
        return 2;
    };
    let script = match std::fs::read_to_string(&script_path) {
        Ok(script) => script,
        Err(err) => {
            eprintln!(
                "nano-tui-fake-host: cannot read {}: {err}",
                script_path.display()
            );
            return 2;
        }
    };
    let mut host = match FakeHost::from_script(&script) {
        Ok(host) => host,
        Err(err) => {
            eprintln!("nano-tui-fake-host: bad script: {err}");
            return 2;
        }
    };

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let frame: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(frame) => frame,
            Err(err) => {
                eprintln!("nano-tui-fake-host: client sent invalid json: {err}");
                return 2;
            }
        };
        match host.feed(&frame) {
            Ok(replies) => {
                for reply in replies {
                    if writeln!(stdout, "{reply}")
                        .and_then(|()| stdout.flush())
                        .is_err()
                    {
                        return 0; // client gone
                    }
                }
            }
            Err(err) => {
                eprintln!("nano-tui-fake-host: script violation: {err}");
                return 2;
            }
        }
    }
    if !host.is_exhausted() {
        eprintln!(
            "nano-tui-fake-host: stdin closed with unplayed expectations: {:?}",
            host.remaining()
        );
        return 3;
    }
    0
}
