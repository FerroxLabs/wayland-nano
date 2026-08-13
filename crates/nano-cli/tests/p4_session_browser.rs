use nano_session::{Op, OpEnvelope};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn write_journal(path: &std::path::Path, session_id: &str, cwd: &str) {
    let first = serde_json::to_string(&OpEnvelope::new(
        format!("begin-{session_id}"),
        "2026-08-13T00:00:00Z",
        Op::SessionBegin {
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
        },
    ))
    .unwrap();
    std::fs::write(path, format!("{first}\n")).unwrap();
}

#[test]
fn headless_sessions_prints_the_shared_summary_fields() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    write_journal(&sessions.join("s1.jsonl"), "s1", "C:/workspace");

    let output = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("sessions")
        .env("NANO_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let fields: Vec<&str> = stdout.trim().split('\t').collect();
    assert_eq!(fields.len(), 5, "id/cwd/mtime/size/status: {stdout}");
    assert_eq!(fields[0], "s1");
    assert_eq!(fields[1], "C:/workspace");
    assert_eq!(fields[4], "Closed");
}

#[test]
fn corrupt_session_load_keeps_existing_journal_unavailable_kind() {
    let Some(flux_key) = std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|key| !key.is_empty())
    else {
        eprintln!("FLUX_TEST_KEY not set — skipping ACP process load assertion");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("corrupt.jsonl"), "not-json\n{}\n").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("acp-host")
        .env("NANO_HOME", home.path())
        .env("FLUX_API_KEY", flux_key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":1,"clientCapabilities":{}}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let initialize: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert!(
        initialize.get("result").is_some(),
        "initialize: {initialize}"
    );

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"session/load",
            "params":{"sessionId":"corrupt","cwd":".","mcpServers":[]}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let load: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        load["error"]["data"]["nanoError"]["kind"],
        "journal_unavailable"
    );

    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// RC2 wiring (P4 session browser §4): `_wayland/session/list` is served
/// with or without an active session (the listing is global), advertised
/// in nanoExtensions, and io failures map to the typed
/// journal_unavailable kind.
#[test]
fn session_list_extension_round_trip_without_an_active_session() {
    let home = tempfile::tempdir().unwrap();
    let sessions = home.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    write_journal(&sessions.join("s1.jsonl"), "s1", "C:/workspace");

    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("acp-host")
        .env("NANO_HOME", home.path())
        // Startup needs SOME resolvable credential (B2 gate); this dummy
        // never leaves the process (no model call is made).
        .env("FLUX_API_KEY", "sk-test-fixture-never-networked")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":1,"clientCapabilities":{}}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let initialize: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        initialize["result"]["agentCapabilities"]["nanoExtensions"]["_wayland/session/list"]["version"],
        1,
        "advertised: {initialize}"
    );

    // No session/new — the listing is global and must still answer.
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"_wayland/session/list","params":{}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let list: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(list["result"]["sessions"][0]["sessionId"], "s1", "{list}");
    assert_eq!(list["result"]["sessions"][0]["status"], "closed", "{list}");
    assert!(
        list["result"]["liveStatusCaveat"]
            .as_str()
            .unwrap()
            .contains("point-in-time"),
        "{list}"
    );

    drop(stdin);
    assert!(child.wait().unwrap().success());

    // io failure (sessions dir replaced by a regular file) ⇒ typed
    // journal_unavailable, never a panic or an empty ok.
    std::fs::remove_dir_all(&sessions).unwrap();
    std::fs::write(&sessions, "not a dir").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .arg("acp-host")
        .env("NANO_HOME", home.path())
        // Startup needs SOME resolvable credential (B2 gate); this dummy
        // never leaves the process (no model call is made).
        .env("FLUX_API_KEY", "sk-test-fixture-never-networked")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        stdin,
        "{}",
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"_wayland/session/list","params":{}
        })
    )
    .unwrap();
    stdin.flush().unwrap();
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let list: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        list["error"]["data"]["nanoError"]["kind"], "journal_unavailable",
        "{list}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
