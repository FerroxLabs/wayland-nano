//! C11 §7 "Both-UIs drive": exec process smoke — spawn the real
//! `wayland-nano exec` binary, assert the JSONL v1 stream and exit code.
//! Live-gated (FLUX_TEST_KEY via the standard resolution chain): self-skips
//! without a key, per the repo's live-test convention. Never embeds the key
//! value anywhere.

use std::io::BufRead;
use std::process::Stdio;

fn flux_key_available() -> bool {
    if std::env::var_os("FLUX_API_KEY").is_some() || std::env::var_os("FLUX_TEST_KEY").is_some() {
        return true;
    }
    std::env::var_os("FLUX_API_KEY_FILE").is_some_and(|path| {
        std::fs::metadata(&path)
            .map(|m| m.is_file())
            .unwrap_or(false)
    })
}

#[test]
fn exec_process_emits_jsonl_v1_and_exits_zero() {
    if !flux_key_available() {
        eprintln!("skipping: no Flux key (live-gated)");
        return;
    }
    let dir = std::env::temp_dir().join(format!("nano-c11-exec-proc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args([
            "exec",
            "--output-last-message",
            dir.join("last.txt").to_str().expect("utf8 path"),
            "Reply with exactly: ok",
        ])
        .env("NANO_HOME", &dir)
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("spawn wayland-nano exec");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let lines: Vec<serde_json::Value> = std::io::BufReader::new(&output.stdout[..])
        .lines()
        .map(|line| serde_json::from_str(&line.expect("line")).expect("json"))
        .collect();
    assert!(
        lines.len() >= 3,
        "session_started + turn events + turn_completed"
    );
    for (index, event) in lines.iter().enumerate() {
        assert_eq!(event["v"], 1);
        assert!(
            event["session_id"]
                .as_str()
                .unwrap()
                .starts_with("wayland-nano-session-")
        );
        assert_eq!(event["seq"].as_u64().unwrap(), index as u64);
    }
    assert_eq!(lines[0]["type"], "session_started");
    assert_eq!(lines.last().unwrap()["type"], "turn_completed");
    // --output-last-message captured the final text.
    let last = std::fs::read_to_string(dir.join("last.txt")).unwrap();
    assert!(!last.trim().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Usage error path (no key needed): a bad resume target exits 2 with
/// NOTHING parseable as events on stdout.
#[test]
fn exec_process_bad_resume_exits_2() {
    let dir = std::env::temp_dir().join(format!("nano-c11-exec-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(["exec", "--resume", "no-such-session", "hello"])
        .env("NANO_HOME", &dir)
        // A bogus key keeps the run offline; the resume failure precedes
        // any model call anyway.
        .env("FLUX_API_KEY", "invalid-test-not-a-key")
        .current_dir(&dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("spawn wayland-nano exec");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "no events without a session: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
