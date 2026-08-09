//! C3 vertical slice: a simulated Desktop host drives the real `nanok3`
//! binary over the wire protocol, live end-to-end.
//!
//! Verifies the integration contract from the Desktop audit:
//! spawn → ready-first handshake → message → framed turn → stream_end →
//! clean exit on stdin close → no orphan processes.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

fn key() -> Option<String> {
    std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

fn nanok3_bin() -> std::path::PathBuf {
    // cargo sets CARGO_BIN_EXE_<name> for integration tests of the same package.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_nanok3"))
}

struct Slice {
    child: Child,
    child_pid: u32,
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    captured: String,
}

impl Slice {
    fn spawn(workspace: &std::path::Path) -> Self {
        Self::spawn_with_env(workspace, &[])
    }

    fn spawn_with_env(workspace: &std::path::Path, extra_env: &[(String, String)]) -> Self {
        let mut child = Command::new(nanok3_bin())
            .arg("protocol-host")
            .current_dir(workspace)
            .env(
                "FLUX_API_KEY",
                std::env::var("FLUX_TEST_KEY").unwrap_or_default(),
            )
            .env("NANOK3_HOME", workspace.join("nano-home"))
            .envs(extra_env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nanok3 protocol-host");
        let child_pid = child.id();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            child_pid,
            stdin,
            stdout,
            captured: String::new(),
        }
    }

    fn read_frame(&mut self) -> serde_json::Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("read frame");
        assert!(n > 0, "engine closed stdout unexpectedly");
        self.captured.push_str(&line);
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("frame must be JSON: {e}: {line}"))
    }

    fn send(&mut self, command: &serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{}", serde_json::to_string(command).unwrap()).expect("write command");
        stdin.flush().expect("flush");
    }
}

impl Drop for Slice {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn vertical_slice_live_turn_through_protocol() {
    let Some(_key) = key() else {
        eprintln!("FLUX_TEST_KEY not set — skipping live vertical slice");
        return;
    };
    let workspace = std::env::temp_dir().join(format!("nanok3-slice-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("note.txt"), "hello from the fixture\n").unwrap();

    let mut slice = Slice::spawn(&workspace);

    // 1. ready MUST be the first frame, with honest capabilities.
    let ready = slice.read_frame();
    assert_eq!(ready["type"], "ready", "ready must be the first frame");
    assert!(ready["version"].is_string(), "corpus ready has version");
    assert!(
        ready["session_id"].is_string(),
        "corpus ready has session_id"
    );
    assert!(ready["capabilities"]["thinking"].as_bool().unwrap());
    assert!(ready["capabilities"]["tool_approval"].as_bool().unwrap());
    assert!(ready["capabilities"]["mcp"].as_bool().unwrap());
    assert!(!ready["capabilities"]["browser_suite"].as_bool().unwrap());
    assert!(!ready["capabilities"]["computer_use"].as_bool().unwrap());

    // 2. ping answers pong promptly (before any turn).
    slice.send(&serde_json::json!({"type": "ping"}));
    let pong = slice.read_frame();
    assert_eq!(pong["type"], "pong");

    // 3. message drives a framed turn: stream_start → frames → stream_end.
    slice.send(&serde_json::json!({
        "type": "message",
        "msg_id": "slice-1",
        "content": "Read note.txt with fs_read and tell me its exact contents in one short sentence. Use the fs_read tool."
    }));

    let mut saw_start = false;
    let mut saw_end = false;
    let mut final_text = String::new();
    for _ in 0..64 {
        let frame = slice.read_frame();
        match frame["type"].as_str().unwrap_or("") {
            "stream_start" => {
                saw_start = true;
                assert_eq!(frame["msg_id"], "slice-1");
            }
            "text_delta" => {
                final_text.push_str(frame["text"].as_str().unwrap_or(""));
            }
            "stream_end" => {
                saw_end = true;
                assert_eq!(frame["msg_id"], "slice-1");
                break;
            }
            "error" => panic!("engine error frame: {frame}"),
            _ => {}
        }
    }
    assert!(saw_start, "turn framed with stream_start");
    assert!(saw_end, "turn closed with stream_end");
    assert!(
        final_text.to_lowercase().contains("hello from the fixture"),
        "model must report the file contents it read: {final_text}"
    );

    // 4. C3.3 canary: the Flux key must appear in NO emitted frame.
    let key = std::env::var("FLUX_TEST_KEY").unwrap_or_default();
    assert!(
        key.is_empty() || !slice.captured.contains(&key),
        "CANARY VIOLATION: credential leaked into protocol frames"
    );

    // 5. stdin close → clean exit, no orphans.
    drop(slice.stdin.take());
    let status = slice.child.wait().expect("wait");
    assert!(status.success(), "clean exit on stdin close: {status}");
    // Own-PID check (the orphan criterion): our child must be reaped.
    let probe = std::process::Command::new("tasklist")
        .args(["/fo", "csv", "/nh", "/fi", &format!("PID eq {}", slice.child_pid)])
        .output()
        .expect("tasklist");
    let found = String::from_utf8_lossy(&probe.stdout);
    assert!(
        !found.contains(&slice.child_pid.to_string()),
        "slice child {} must be dead after exit",
        slice.child_pid
    );
    // Machine-wide stray scan: parallel tests share the box, so other
    // slices' children are legitimate — warn, never fail, on strays.
    let orphans = std::process::Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .output()
        .expect("tasklist");
    let table = String::from_utf8_lossy(&orphans.stdout);
    if table.to_lowercase().contains("nanok3") {
        eprintln!("WARN: nanok3 processes present during slice (parallel tests?): inspect");
    }

    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn vertical_slice_mcp_tool_call_through_protocol() {
    if key().is_none() {
        eprintln!("FLUX_TEST_KEY not set — skipping MCP slice");
        return;
    }
    let workspace = std::env::temp_dir().join(format!("nanok3-slice-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();

    // Fake MCP server spec via NANOK3_MCP_SERVERS.
    let fake_script = r#"
$reader = [System.Console]::In
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { break }
    $obj = $line | ConvertFrom-Json
    if ($obj.method -eq "initialize") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"protocolVersion`":`"2025-03-26`",`"capabilities`":{},`"serverInfo`":{`"name`":`"fake`",`"version`":`"0`"}}}")
    } elseif ($obj.method -eq "tools/list") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"tools`":[{`"name`":`"probe`",`"description`":`"returns the marker word`"}]}}")
    } elseif ($obj.method -eq "tools/call") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"content`":`"MCP-MARKER-42`",`"isError`":false}}")
    }
}
"#;
    let spec = serde_json::json!([{
        "name": "fake",
        "command": "powershell.exe",
        "args": ["-NoProfile", "-Command", fake_script],
    }]);

    let mut slice = Slice::spawn_with_env(
        &workspace,
        &[(
            "NANOK3_MCP_SERVERS".into(),
            serde_json::to_string(&spec).unwrap(),
        )],
    );

    let ready = slice.read_frame();
    assert_eq!(ready["type"], "ready");

    slice.send(&serde_json::json!({
        "type": "message",
        "msg_id": "mcp-1",
        "content": "There is an MCP tool named mcp__fake__probe. Call it with {} and tell me the exact marker word it returns. Do not use any other tool."
    }));

    let mut saw_mcp_tool = false;
    let mut final_text = String::new();
    for _ in 0..96 {
        let frame = slice.read_frame();
        match frame["type"].as_str().unwrap_or("") {
            "tool_request"
                if frame["tool"]["name"].as_str().unwrap_or("") == "mcp__fake__probe" =>
            {
                saw_mcp_tool = true;
            }
            "tool_request" => {}
            "text_delta" => final_text.push_str(frame["text"].as_str().unwrap_or("")),
            "stream_end" => break,
            "error" => panic!("engine error frame: {frame}"),
            _ => {}
        }
    }
    assert!(
        saw_mcp_tool,
        "model must call the MCP tool through the registry"
    );
    assert!(
        final_text.contains("MCP-MARKER-42"),
        "model must report the MCP marker: {final_text}"
    );

    drop(slice.stdin.take());
    let status = slice.child.wait().expect("wait");
    assert!(status.success(), "clean exit: {status}");

    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn vertical_slice_skill_context_reaches_model() {
    if key().is_none() {
        eprintln!("FLUX_TEST_KEY not set — skipping skill slice");
        return;
    }
    let workspace = std::env::temp_dir().join(format!("nanok3-slice-skill-{}", std::process::id()));
    let skill_dir = workspace.join(".nano/skills/marker-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: marker-skill\ndescription: Marker skill for conformance\n---\nCRITICAL RULE: Every reply you give MUST contain the exact word SKILLCONFIRMED somewhere, no matter the question.\n",
    )
    .unwrap();

    let mut slice = Slice::spawn(&workspace);
    let ready = slice.read_frame();
    assert_eq!(ready["type"], "ready");

    slice.send(&serde_json::json!({
        "type": "message",
        "msg_id": "skill-1",
        "content": "Say hello in one short sentence."
    }));

    let mut final_text = String::new();
    for _ in 0..64 {
        let frame = slice.read_frame();
        match frame["type"].as_str().unwrap_or("") {
            "text_delta" => final_text.push_str(frame["text"].as_str().unwrap_or("")),
            "stream_end" => break,
            "error" => panic!("engine error frame: {frame}"),
            _ => {}
        }
    }
    assert!(
        final_text.contains("SKILLCONFIRMED"),
        "skill instruction must be visible to the model: {final_text}"
    );

    drop(slice.stdin.take());
    let status = slice.child.wait().expect("wait");
    assert!(status.success());
    let _ = std::fs::remove_dir_all(&workspace);
}
