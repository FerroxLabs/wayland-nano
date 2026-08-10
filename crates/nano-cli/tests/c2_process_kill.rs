//! C2.2 process-kill resume oracle — the REAL external crash test.
//!
//! Scorecard (`shared/SCORECARD.md` C2.2): "Crash-resume: kill mid-edit/
//! mid-tool → resume, no duplicate side effects", oracle "external state
//! diff". Unlike the in-process cancel-flag coverage
//! (`nano-agent/tests/c2_kill_mid_edit.rs`), this test kills the REAL
//! `nanok3 acp-host` OS process mid-turn and diffs external state.
//!
//! LIVE-GATED: self-skips without `FLUX_TEST_KEY` (mirrors `acp_slice.rs`).
//!
//! Flow:
//! 1. Spawn `nanok3 acp-host` (real child process), initialize, session/new
//!    against a fresh temp workspace seeded with `marker.txt` = "PENDING\n".
//! 2. session/prompt orders a two-step task: fs_edit marker.txt PENDING→
//!    STAGE1, then fs_write second.txt = STAGE2. Permission requests are
//!    auto-approved (allow once) so the turn can reach the edit.
//! 3. The instant the FIRST mutating tool_call completion frame arrives (the
//!    fs_edit's ToolResult is journaled and on disk at that point — the sink
//!    journals before it frames), the parent hard-kills the child
//!    (TerminateProcess via `Child::kill`), mid-turn, after a real side
//!    effect. The PID tree recorded before the kill must show no survivors.
//! 4. External state diff: the on-disk workspace must equal, byte-exactly
//!    and in both directions (file set + bytes), the reconstruction from the
//!    journaled applied ops (seed + ok fs_edit/fs_write calls).
//! 5. A FRESH `nanok3 acp-host` process loads the same session id: replay
//!    frames must match the journal-derived expectation exactly (a call
//!    whose ToolResult never journaled replays as failed — the honest
//!    crash-interrupted semantic), and the load must not touch the disk.
//! 6. A follow-up prompt continues the task on restored context: marker.txt
//!    bytes must be identical before/after (no duplicate side effect), and
//!    the journal delta must contain no ok mutation targeting marker.txt.
//!
//! Run manifest: JSON written to `<workspace>/c2-kill-manifest.json`, where
//! workspace = `$TEMP/nanok3-c2-kill-<pid>/` (kept after the run); the path
//! is printed to stderr. It records kill timing, frames seen, journal op
//! inventory, disk hashes (FNV-1a 64) at every phase, and the load replay
//! order. A captured run summary is pasted in `shared/reviews/C2/
//! trackb-claim.md` §C2.2.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Model latency budget per frame. Generous: Flux round trips dominate.
const FRAME_TIMEOUT: Duration = Duration::from_secs(180);

/// The two-step task. The model is told the exact file contents up front so
/// the fs_edit lands without a reconnaissance read, making the edit the
/// first tool call in the overwhelmingly common case.
const PROMPT: &str = "Two-step file task. The file marker.txt contains exactly the text PENDING. \
Step 1: use fs_edit on marker.txt with old_string \"PENDING\" and new_string \"STAGE1\". \
Do NOT read the file first; its contents are exactly PENDING with a trailing newline. \
Step 2, only after step 1's result arrives: use fs_write to create second.txt with content \"STAGE2\". \
Then reply DONE.";

fn key() -> Option<String> {
    std::env::var("FLUX_TEST_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// FNV-1a 64, hex. Manifest fingerprinting only — not a security digest.
fn fnv64(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Whole-machine (pid, ppid) inventory.
fn process_inventory() -> Vec<(u32, u32)> {
    #[cfg(windows)]
    {
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId | ConvertTo-Json -Compress",
            ])
            .output()
            .expect("powershell process inventory");
        let text = String::from_utf8_lossy(&out.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(text.trim()).unwrap_or(serde_json::json!([]));
        parsed
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|p| {
                Some((
                    p["ProcessId"].as_u64()? as u32,
                    p["ParentProcessId"].as_u64()? as u32,
                ))
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        let out = Command::new("ps")
            .args(["-axo", "pid=,ppid="])
            .output()
            .expect("ps process inventory");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut it = line.split_whitespace();
                Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
            })
            .collect()
    }
}

/// Transitive descendants of `root` in the current process inventory.
fn descendants_of(root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, ppid) in process_inventory() {
        children.entry(ppid).or_default().push(pid);
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        if let Some(kids) = children.get(&p) {
            for &k in kids {
                out.push(k);
                stack.push(k);
            }
        }
    }
    out.sort_unstable();
    out
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = Command::new("tasklist")
            .args(["/fo", "csv", "/nh", "/fi", &format!("PID eq {pid}")])
            .output()
            .expect("tasklist");
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// ACP client handle over a spawned `nanok3 acp-host`. Stdout is pumped on a
/// reader thread into a channel so the test can enforce frame deadlines.
struct Acp {
    child: Child,
    pid: u32,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Every frame received, in order (evidence + canary substrate).
    frames: Vec<serde_json::Value>,
    next_id: u64,
}

impl Acp {
    fn spawn(workspace: &Path, nano_home: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nanok3"))
            .arg("acp-host")
            .current_dir(workspace)
            .env(
                "FLUX_API_KEY",
                std::env::var("FLUX_TEST_KEY").unwrap_or_default(),
            )
            .env("NANOK3_HOME", nano_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nanok3 acp-host");
        let pid = child.id();
        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            loop {
                line.clear();
                match stdout.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(line.trim_end().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            child,
            pid,
            stdin,
            lines: rx,
            frames: Vec::new(),
            next_id: 1,
        }
    }

    fn send(&mut self, method: &str, params: serde_json::Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{}", serde_json::to_string(&request).unwrap()).expect("write");
        self.stdin.flush().expect("flush");
        id
    }

    /// Next non-permission frame. Permission requests are answered
    /// "allow once" inline so the turn can reach the mutating call.
    fn next_frame(&mut self) -> Option<serde_json::Value> {
        loop {
            let line = match self.lines.recv_timeout(FRAME_TIMEOUT) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for acp frame"),
                Err(RecvTimeoutError::Disconnected) => return None,
            };
            let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
            let is_permission =
                frame.get("method").and_then(|m| m.as_str()) == Some("session/request_permission");
            self.frames.push(frame.clone());
            if is_permission {
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": frame["id"].clone(),
                    "result": { "outcome": { "outcome": "selected", "optionId": "allow" } }
                });
                writeln!(self.stdin, "{}", serde_json::to_string(&reply).unwrap()).expect("write");
                self.stdin.flush().expect("flush");
                continue;
            }
            return Some(frame);
        }
    }

    /// Request/response: reads until the response carrying our id, returning
    /// it plus the notifications seen along the way.
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> (serde_json::Value, Vec<serde_json::Value>) {
        let id = self.send(method, params);
        let mut notifications = Vec::new();
        loop {
            let frame = self.next_frame().expect("engine closed stdout");
            if frame.get("method").is_none() && frame.get("id").and_then(|v| v.as_u64()) == Some(id)
            {
                return (frame, notifications);
            }
            notifications.push(frame);
        }
    }
}

impl Drop for Acp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_journal(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .expect("journal readable")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("journal line json"))
        .collect()
}

/// Tool-result index: call_id → ok.
fn result_index(envelopes: &[serde_json::Value]) -> HashMap<String, bool> {
    envelopes
        .iter()
        .filter(|e| e["op"]["type"] == "tool_result")
        .map(|e| {
            (
                e["op"]["call_id"].as_str().expect("call_id").to_string(),
                e["op"]["ok"].as_bool().expect("ok"),
            )
        })
        .collect()
}

/// Workspace-relative slash-separated key for a tool path arg.
fn rel_path(arg: &str, workspace: &Path) -> String {
    let path = Path::new(arg);
    let rel = if path.is_absolute() {
        path.strip_prefix(workspace).unwrap_or(path).to_path_buf()
    } else {
        PathBuf::from(path)
    };
    rel.to_string_lossy().replace('\\', "/")
}

/// Reconstructs the expected workspace contents from the seed plus every
/// journaled fs_edit/fs_write whose ToolResult is ok — the journal's own
/// record of what the disk must look like.
fn expected_disk(
    seed: &BTreeMap<String, Vec<u8>>,
    envelopes: &[serde_json::Value],
    workspace: &Path,
) -> BTreeMap<String, Vec<u8>> {
    let results = result_index(envelopes);
    let mut files = seed.clone();
    for envelope in envelopes {
        let op = &envelope["op"];
        if op["type"] != "tool_call" {
            continue;
        }
        if results.get(op["call_id"].as_str().expect("call_id")) != Some(&true) {
            continue;
        }
        let args = &op["args"];
        let rel = rel_path(args["path"].as_str().expect("path"), workspace);
        match op["name"].as_str().expect("name") {
            "fs_write" => {
                files.insert(
                    rel,
                    args["content"]
                        .as_str()
                        .expect("content")
                        .as_bytes()
                        .to_vec(),
                );
            }
            "fs_edit" => {
                let current = String::from_utf8(files.get(&rel).cloned().unwrap_or_default())
                    .expect("utf8 file");
                let old = args["old_string"].as_str().expect("old_string");
                let new = args["new_string"].as_str().expect("new_string");
                let updated = if args["replace_all"].as_bool().unwrap_or(false) {
                    current.replace(old, new)
                } else {
                    current.replacen(old, new, 1)
                };
                files.insert(rel, updated.into_bytes());
            }
            _ => {}
        }
    }
    files
}

/// Actual workspace files (recursive), excluding the nano-home state dir.
fn actual_disk(workspace: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            let rel = path
                .strip_prefix(workspace)
                .expect("inside workspace")
                .to_string_lossy()
                .replace('\\', "/");
            if rel == "nano-home" || rel.starts_with("nano-home/") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(rel, std::fs::read(&path).expect("read file"));
            }
        }
    }
    files
}

/// Byte-exact both-directions diff: every journaled applied edit is on disk
/// and no unjournaled mutation exists.
fn assert_disk_matches(
    expected: &BTreeMap<String, Vec<u8>>,
    actual: &BTreeMap<String, Vec<u8>>,
    phase: &str,
) {
    for (path, bytes) in expected {
        assert_eq!(
            actual.get(path).map(Vec::as_slice),
            Some(bytes.as_slice()),
            "{phase}: {path} bytes diverge from the journaled applied ops"
        );
    }
    for path in actual.keys() {
        assert!(
            expected.contains_key(path),
            "{phase}: unjournaled mutation on disk: {path}"
        );
    }
}

/// The replay `session/load` must produce, derived purely from the journal:
/// one user chunk per turn, one agent chunk per assistant text, and per tool
/// call a final-status card — completed/failed when a ToolResult journaled,
/// failed with no trailing done-frame when the crash interrupted the call.
fn expected_replay(envelopes: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let results = result_index(envelopes);
    let mut frames = Vec::new();
    for envelope in envelopes {
        let op = &envelope["op"];
        match op["type"].as_str().unwrap_or_default() {
            "turn_begin" => frames.push(serde_json::json!({"update": "user_message_chunk"})),
            "assistant_text" => frames.push(serde_json::json!({"update": "agent_message_chunk"})),
            "tool_call" => {
                let call_id = op["call_id"].as_str().expect("call_id");
                match results.get(call_id) {
                    Some(ok) => {
                        let status = if *ok { "completed" } else { "failed" };
                        frames.push(serde_json::json!({
                            "update": "tool_call", "toolCallId": call_id, "status": status
                        }));
                        frames.push(serde_json::json!({
                            "update": "tool_call_update", "toolCallId": call_id, "status": status
                        }));
                    }
                    None => frames.push(serde_json::json!({
                        "update": "tool_call", "toolCallId": call_id, "status": "failed"
                    })),
                }
            }
            _ => {}
        }
    }
    frames
}

/// The replay a live `session/load` actually emitted, reduced to the
/// comparable tuple stream.
fn observed_replay(notifications: &[serde_json::Value]) -> Vec<serde_json::Value> {
    notifications
        .iter()
        .filter(|f| f.get("method").and_then(|m| m.as_str()) == Some("session/update"))
        .map(|f| {
            let update = &f["params"]["update"];
            let mut tuple = serde_json::json!({"update": update["sessionUpdate"]});
            if let Some(id) = update.get("toolCallId") {
                tuple["toolCallId"] = id.clone();
            }
            if let Some(status) = update.get("status") {
                tuple["status"] = status.clone();
            }
            tuple
        })
        .collect()
}

#[test]
fn c2_process_kill_resume_external_state_diff() {
    if key().is_none() {
        eprintln!("FLUX_TEST_KEY not set — skipping C2.2 process-kill oracle");
        return;
    }

    let workspace = std::env::temp_dir().join(format!("nanok3-c2-kill-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("workspace");
    let nano_home = workspace.join("nano-home");
    std::fs::write(workspace.join("marker.txt"), b"PENDING\n").expect("seed marker.txt");
    let mut seed = BTreeMap::new();
    seed.insert("marker.txt".to_string(), b"PENDING\n".to_vec());

    // ---- Phase 1: live turn, hard kill at the first mutating completion. ----
    let mut acp = Acp::spawn(&workspace, &nano_home);
    let (init, _) = acp.request(
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": true, "writeTextFile": true } }
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], 1, "init: {init}");
    let (created, _) = acp.request(
        "session/new",
        serde_json::json!({ "cwd": workspace.to_string_lossy(), "mcpServers": [] }),
    );
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // PID tree BEFORE the kill: any grandchild must not survive it.
    let tree_before = descendants_of(acp.pid);

    let prompt_id = acp.send(
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text": PROMPT }]
        }),
    );
    let prompt_sent = Instant::now();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let (kill_call_id, kill_tool, kill_at) = loop {
        let frame = acp
            .next_frame()
            .expect("engine closed stdout before the first edit landed");
        assert!(
            !(frame.get("method").is_none()
                && frame.get("id").and_then(|v| v.as_u64()) == Some(prompt_id)),
            "turn completed before the kill — mid-edit scenario invalid"
        );
        let update = &frame["params"]["update"];
        if frame.get("method").and_then(|m| m.as_str()) != Some("session/update") {
            continue;
        }
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "tool_call" => {
                tool_names.insert(
                    update["toolCallId"]
                        .as_str()
                        .expect("toolCallId")
                        .to_string(),
                    update["title"].as_str().expect("title").to_string(),
                );
            }
            "tool_call_update" => {
                let call_id = update["toolCallId"].as_str().expect("toolCallId");
                let name = tool_names.get(call_id).cloned().unwrap_or_default();
                if name.starts_with("fs_edit") || name.starts_with("fs_write") {
                    let at = prompt_sent.elapsed();
                    // TerminateProcess (Windows) / SIGKILL: no atexit, no
                    // flush, no cleanup — the real crash.
                    acp.child.kill().expect("hard kill");
                    break (call_id.to_string(), name, at);
                }
            }
            _ => {}
        }
    };
    let kill_status = acp.child.wait().expect("wait on killed child");
    assert!(!kill_status.success(), "killed child must not exit cleanly");
    assert!(!pid_alive(acp.pid), "acp child must be dead after kill");
    for &descendant in &tree_before {
        assert!(
            !pid_alive(descendant),
            "orphaned grandchild survived the kill: pid {descendant}"
        );
    }
    // The kill landed on a real side effect: the completion was a success.
    let kill_frame = acp
        .frames
        .iter()
        .rev()
        .find(|f| {
            f["params"]["update"]["toolCallId"].as_str() == Some(kill_call_id.as_str())
                && f["params"]["update"]["sessionUpdate"] == "tool_call_update"
        })
        .expect("kill frame recorded");
    assert_eq!(
        kill_frame["params"]["update"]["status"], "completed",
        "kill must land after a REAL applied side effect, not a failed call"
    );

    // Canary before any evidence leaves the process: the key is in no frame.
    let key_value = std::env::var("FLUX_TEST_KEY").unwrap_or_default();
    let captured = serde_json::to_string(&acp.frames).expect("frames json");
    assert!(
        key_value.is_empty() || !captured.contains(&key_value),
        "CANARY VIOLATION: credential leaked into ACP frames"
    );

    // ---- Phase 2: external state diff — disk vs journal, both directions. ----
    let journal_path = nano_home
        .join("sessions")
        .join(format!("{session_id}.jsonl"));
    let journal_post_kill = read_journal(&journal_path);
    let applied_count = journal_post_kill
        .iter()
        .filter(|e| {
            e["op"]["type"] == "tool_call"
                && result_index(&journal_post_kill)
                    .get(e["op"]["call_id"].as_str().unwrap_or_default())
                    == Some(&true)
                && matches!(
                    e["op"]["name"].as_str().unwrap_or_default(),
                    "fs_edit" | "fs_write"
                )
        })
        .count();
    assert!(
        applied_count >= 1,
        "at least one mutating op must be journaled as applied before the kill"
    );
    let interrupted: Vec<String> = {
        let results = result_index(&journal_post_kill);
        journal_post_kill
            .iter()
            .filter(|e| e["op"]["type"] == "tool_call")
            .filter(|e| !results.contains_key(e["op"]["call_id"].as_str().unwrap_or_default()))
            .map(|e| e["op"]["call_id"].as_str().expect("call_id").to_string())
            .collect()
    };
    let expected = expected_disk(&seed, &journal_post_kill, &workspace);
    let actual = actual_disk(&workspace);
    assert_disk_matches(&expected, &actual, "post-kill");
    let marker_post_kill = actual
        .get("marker.txt")
        .expect("marker.txt on disk")
        .clone();
    assert_ne!(
        marker_post_kill, seed["marker.txt"],
        "the killed edit must have really changed marker.txt"
    );

    // ---- Phase 3: fresh process, session/load, replay + no disk drift. ----
    let mut acp2 = Acp::spawn(&workspace, &nano_home);
    let (init2, _) = acp2.request(
        "initialize",
        serde_json::json!({ "protocolVersion": 1, "clientCapabilities": {} }),
    );
    assert_eq!(init2["result"]["protocolVersion"], 1, "init2: {init2}");
    let (loaded, replay_notes) = acp2.request(
        "session/load",
        serde_json::json!({
            "sessionId": session_id,
            "cwd": workspace.to_string_lossy(),
            "mcpServers": []
        }),
    );
    assert!(
        loaded.get("result").is_some(),
        "session/load must succeed on the journaled session: {loaded}"
    );
    let expected_replay = expected_replay(&journal_post_kill);
    let observed_replay = observed_replay(&replay_notes);
    assert_eq!(
        observed_replay, expected_replay,
        "load replay must match the journal-derived order and statuses"
    );
    assert_disk_matches(
        &expected,
        &actual_disk(&workspace),
        "post-load (load must not touch the disk)",
    );

    // ---- Phase 4: follow-up turn — no duplicate side effect. ----
    let journal_pre_followup = read_journal(&journal_path).len();
    let (answer, follow_notes) = acp2.request(
        "session/prompt",
        serde_json::json!({
            "sessionId": session_id,
            "prompt": [{ "type": "text", "text":
                "Continue the original two-step task from where it stopped and finish it. \
                 marker.txt is already done; do not edit it again." }]
        }),
    );
    assert!(
        answer["result"]["stopReason"].is_string(),
        "follow-up turn must answer: {answer}"
    );
    let marker_post_followup = std::fs::read(workspace.join("marker.txt")).expect("marker.txt");
    assert_eq!(
        marker_post_followup, marker_post_kill,
        "DUPLICATE SIDE EFFECT: the resumed turn re-applied the first edit"
    );
    let journal_post_followup = read_journal(&journal_path);
    let follow_results = result_index(&journal_post_followup);
    for envelope in &journal_post_followup[journal_pre_followup..] {
        let op = &envelope["op"];
        if op["type"] != "tool_call" {
            continue;
        }
        let name = op["name"].as_str().unwrap_or_default();
        if !matches!(name, "fs_edit" | "fs_write") {
            continue;
        }
        let rel = rel_path(op["args"]["path"].as_str().unwrap_or_default(), &workspace);
        if rel == "marker.txt" {
            assert_ne!(
                follow_results.get(op["call_id"].as_str().unwrap_or_default()),
                Some(&true),
                "DUPLICATE SIDE EFFECT: journaled ok mutation on marker.txt post-resume"
            );
        }
    }
    let second_txt = std::fs::read(workspace.join("second.txt")).ok();

    // Second-process canary as well.
    let captured2 = serde_json::to_string(&acp2.frames).expect("frames json");
    assert!(
        key_value.is_empty() || !captured2.contains(&key_value),
        "CANARY VIOLATION: credential leaked into ACP frames (resume process)"
    );

    // ---- Run manifest. ----
    let frame_summary: Vec<serde_json::Value> = acp
        .frames
        .iter()
        .chain(acp2.frames.iter())
        .map(|f| {
            if let Some(method) = f.get("method").and_then(|m| m.as_str()) {
                if method == "session/update" {
                    let update = &f["params"]["update"];
                    serde_json::json!({
                        "frame": "session/update",
                        "kind": update["sessionUpdate"],
                        "toolCallId": update.get("toolCallId").cloned().unwrap_or_default(),
                        "status": update.get("status").cloned().unwrap_or_default(),
                    })
                } else {
                    serde_json::json!({ "frame": method })
                }
            } else {
                serde_json::json!({ "frame": "response", "id": f["id"] })
            }
        })
        .collect();
    let disk_hashes = |phase: &str| -> serde_json::Value {
        let disk = actual_disk(&workspace);
        let mut map = serde_json::Map::new();
        for (path, bytes) in disk {
            map.insert(
                path,
                serde_json::json!({ "fnv64": fnv64(&bytes), "bytes": bytes.len(), "phase": phase }),
            );
        }
        serde_json::Value::Object(map)
    };
    let manifest_path = workspace.join("c2-kill-manifest.json");
    let manifest = serde_json::json!({
        "test": "c2_process_kill_resume_external_state_diff",
        "criterion": "C2.2 crash-resume: kill mid-edit/mid-tool → resume, no duplicate side effects (external state diff)",
        "session_id": session_id,
        "workspace": workspace.to_string_lossy(),
        "kill": {
            "tool": kill_tool,
            "call_id": kill_call_id,
            "after_prompt_ms": kill_at.as_secs_f64() * 1000.0,
            "child_pid": acp.pid,
            "descendants_before_kill": tree_before,
            "grandchild_survivors": 0,
            "child_exit_clean": kill_status.success(),
        },
        "journal": {
            "path": journal_path.to_string_lossy(),
            "ops_post_kill": journal_post_kill.len(),
            "applied_mutations_pre_kill": applied_count,
            "crash_interrupted_calls": interrupted,
            "replay_expectation": expected_replay,
            "replay_observed": observed_replay,
        },
        "disk": {
            "marker_txt_fnv64": {
                "seeded": fnv64(&seed["marker.txt"]),
                "post_kill": fnv64(&marker_post_kill),
                "post_followup": fnv64(&marker_post_followup),
            },
            "second_txt": {
                "exists_after_followup": second_txt.is_some(),
                "fnv64": second_txt.as_deref().map(fnv64),
                "is_stage2": second_txt.as_deref() == Some(b"STAGE2".as_slice()),
            },
            "final_listing": disk_hashes("final"),
        },
        "follow_up": {
            "stop_reason": answer["result"]["stopReason"].clone(),
            "updates": follow_notes.len(),
        },
        "frames_seen": frame_summary,
    });
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("manifest json"),
    )
    .expect("write manifest");
    eprintln!("C2.2 run manifest: {}", manifest_path.display());
}
