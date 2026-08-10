//! ACP slice: a simulated ACP host (Desktop's role) drives the real `nanok3
//! acp-host` binary over stdio JSON-RPC, live end-to-end.
//!
//! Verifies: initialize → session/new → session/prompt (streamed updates +
//! final response) → stdin-close clean exit → zero orphans.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn key() -> Option<String> {
    std::env::var("FLUX_TEST_KEY").ok().filter(|k| !k.is_empty())
}

struct AcpSlice {
    child: Child,
    child_pid: u32,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    captured: String,
    next_id: u64,
}

impl AcpSlice {
    fn spawn(workspace: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nanok3"))
            .arg("acp-host")
            .current_dir(workspace)
            .env(
                "FLUX_API_KEY",
                std::env::var("FLUX_TEST_KEY").unwrap_or_default(),
            )
            .env("NANOK3_HOME", workspace.join("nano-home"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nanok3 acp-host");
        let child_pid = child.id();
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            child_pid,
            stdin,
            stdout,
            captured: String::new(),
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).expect("write");
        stdin.flush().expect("flush");

        // Read lines until the response carrying our id arrives; notifications
        // (no id / method-bearing) are collected as evidence, not matched.
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read");
            assert!(n > 0, "engine closed stdout");
            self.captured.push_str(&line);
            let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
            if frame.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return frame;
            }
        }
    }
}

impl Drop for AcpSlice {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn acp_slice_live_prompt_through_desktop_protocol() {
    if key().is_none() {
        eprintln!("FLUX_TEST_KEY not set — skipping ACP slice");
        return;
    }
    let workspace = std::env::temp_dir().join(format!("nanok3-acp-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("note.txt"), "acp slice marker\n").unwrap();

    let mut slice = AcpSlice::spawn(&workspace);

    // 1. initialize — protocol v1, nanok3 identity, prompt text capability.
    let init = slice.request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
        }),
    );
    let result = &init["result"];
    assert_eq!(result["protocolVersion"], 1, "init: {init}");
    assert_eq!(result["agentInfo"]["name"], "nanok3");
    assert_eq!(result["agentCapabilities"]["promptCapabilities"]["text"], true);

    // 2. session/new → sessionId.
    let created = slice.request(
        "session/new",
        serde_json::json!({ "cwd": workspace.to_string_lossy(), "mcpServers": [] }),
    );
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();
    assert!(session_id.starts_with("nanok3-session-"));

    // 3. session/prompt → streamed updates captured + final stopReason.
    let answer = slice.request(
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": "Read note.txt with fs_read and reply with its exact contents. Do not use any other tool." }]
        }),
    );
    assert_eq!(answer["result"]["stopReason"], "end_turn", "answer: {answer}");

    let captured = slice.captured.clone();
    assert!(
        captured.contains("\"sessionUpdate\":\"tool_call\""),
        "tool call must stream as ACP update: {captured}"
    );
    assert!(
        captured.contains("agent_message_chunk"),
        "final text must stream: {captured}"
    );
    assert!(
        captured.contains("acp slice marker"),
        "model must report the file contents it read"
    );

    // 4. canary: key never appears in any frame.
    let key = std::env::var("FLUX_TEST_KEY").unwrap_or_default();
    assert!(
        key.is_empty() || !captured.contains(&key),
        "CANARY VIOLATION: credential leaked into ACP frames"
    );

    // 5. stdin close → clean exit, own PID reaped.
    drop(slice.stdin.take());
    let status = slice.child.wait().expect("wait");
    assert!(status.success(), "clean exit: {status}");
    let probe = Command::new("tasklist")
        .args(["/fo", "csv", "/nh", "/fi", &format!("PID eq {}", slice.child_pid)])
        .output()
        .expect("tasklist");
    assert!(
        !String::from_utf8_lossy(&probe.stdout).contains(&slice.child_pid.to_string()),
        "acp child must be dead after exit"
    );

    let _ = std::fs::remove_dir_all(&workspace);
}
