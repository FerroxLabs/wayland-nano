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
