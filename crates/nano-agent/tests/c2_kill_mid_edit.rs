//! C2.2 kill-mid-edit EXTERNAL-STATE-DIFF — crash semantics proven against
//! the file bytes on disk, never internal state. Scripted model driver (no
//! live key), REAL tool executor, REAL append-only journal.
//!
//! Scorecard mapping (`shared/SCORECARD.md` C2.2): kill mid-edit/mid-tool →
//! resume, no duplicate side effects; oracle = external state diff.
//!
//! The kill lands at the engine's documented kill boundary: the streaming
//! sink journals every op the moment the engine records it (production parity
//! with `acp_mode`), and the instant the `fs_edit`'s ToolResult is durable
//! the cancellation flag fires — the exact crash window "edit applied and
//! journaled, process dead before the next step". The engine never cancels
//! mid-tool-execution (`run_turn_cancellable` docs), so this boundary stop IS
//! the mid-edit kill semantics: a side effect already applied stays applied
//! and journaled, and nothing past the boundary may execute.
//!
//! External oracle, both directions:
//! - every journaled applied edit is ON DISK: the expected workspace bytes
//!   are rebuilt purely from the journal's ToolCall ops and compared to the
//!   real bytes;
//! - NO unjournaled mutation exists: the full workspace file set + bytes
//!   must equal that journal reconstruction exactly;
//! - resume does not double-apply: the session/load resume path (journal
//!   fold + transcript rebuild + continued turn on the same journal) leaves
//!   the file hash bit-identical before/after, and the post-resume journal
//!   still reconstructs to the same disk bytes.

use nano_agent::loop_protection::TurnBudget;
use nano_agent::turn::{ApproveAll, ModelDriver, TurnEngine, TurnState};
use nano_agent::wiring::{RealToolExecutor, v1_tool_definitions};
use nano_model::types::{
    ContentBlock, Message, ModelError, ModelEvent, ModelRequest, ModelResponse, Role, ToolCall,
    Usage,
};
use nano_session::op::{Op, OpEnvelope, TurnOutcome};
use nano_session::reader::read_journal;
use nano_session::replay::SessionState;
use nano_session::writer::JournalWriter;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const ORIGINAL: &str =
    "pub fn add(a: i32, b: i32) -> i32 {\n    a - b // BUG: wrong operator here\n}\n";
const EDIT_OLD: &str = "a - b // BUG: wrong operator here";
const EDIT_NEW: &str = "a + b // fixed: correct operator";
const TASK: &str =
    "Read math.rs, fix the inverted operator with fs_edit, then write a summary file.";

// --- scripted model (no live API; mirrors src/turn_tests.rs) -------------------

#[derive(Debug)]
struct ScriptedModel {
    responses: Mutex<Vec<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for ScriptedModel {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(self.responses.lock().unwrap().remove(0))
    }
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::TextDelta(text.into()),
            ModelEvent::Done {
                stop_reason: "stop".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "stop".into(),
    }
}

fn tool_response(call: ToolCall) -> ModelResponse {
    ModelResponse {
        events: vec![
            ModelEvent::ToolCallComplete(call),
            ModelEvent::Done {
                stop_reason: "tool_calls".into(),
            },
        ],
        usage: Usage::default(),
        stop_reason: "tool_calls".into(),
    }
}

// --- external-state helpers ------------------------------------------------------

/// Every file under `root`, relative path → bytes. The file SET is part of
/// the oracle: an unjournaled created or deleted file fails the diff.
fn snapshot_workspace(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("workspace readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("under workspace")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(rel, std::fs::read(&path).expect("file readable"));
            }
        }
    }
    files
}

/// Rebuilds the expected workspace bytes from the journal ALONE: every
/// mutating ToolCall whose ToolResult journaled ok is applied, in journal
/// order, to the pre-turn snapshot. Compared against the real disk bytes this
/// catches both directions of divergence — a journaled edit missing from
/// disk, and a disk mutation the journal never recorded.
fn journaled_workspace(
    envelopes: &[OpEnvelope],
    before: &BTreeMap<String, Vec<u8>>,
) -> BTreeMap<String, Vec<u8>> {
    let applied: HashSet<&str> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.op {
            Op::ToolResult {
                call_id, ok: true, ..
            } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    let mut shadow = before.clone();
    for envelope in envelopes {
        let Op::ToolCall {
            call_id,
            name,
            args,
            ..
        } = &envelope.op
        else {
            continue;
        };
        if !applied.contains(call_id.as_str()) {
            continue; // no journaled ok result → must have left no trace
        }
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .expect("mutating call carries a path");
        match name.as_str() {
            "fs_write" => {
                let content = args
                    .get("content")
                    .and_then(|v| v.as_str())
                    .expect("fs_write carries content");
                shadow.insert(path.to_string(), content.as_bytes().to_vec());
            }
            "fs_edit" => {
                let old = args.get("old_string").and_then(|v| v.as_str()).unwrap();
                let new = args.get("new_string").and_then(|v| v.as_str()).unwrap();
                let replace_all = args
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let current = String::from_utf8(
                    shadow
                        .get(path)
                        .unwrap_or_else(|| panic!("journaled edit targets unknown file {path}"))
                        .clone(),
                )
                .expect("utf-8 fixture");
                assert!(
                    current.contains(old),
                    "journaled edit's old_string missing from reconstructed {path}"
                );
                let edited = if replace_all {
                    current.replace(old, new)
                } else {
                    current.replacen(old, new, 1)
                };
                shadow.insert(path.to_string(), edited.into_bytes());
            }
            // fs_read/shell model no fs mutation here; if a shell call DID
            // mutate, the disk ⇄ shadow equality below fails loudly.
            _ => {}
        }
    }
    shadow
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Mirrors `acp_mode::messages_from_envelopes` (the session/load resume
/// path): tool payloads are digest-only in the journal, so a restored result
/// carries an elision marker, never a fabricated payload.
fn resume_context_from(envelopes: &[OpEnvelope]) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut assistant: Vec<ContentBlock> = Vec::new();
    let mut seen = HashSet::new();
    let flush = |messages: &mut Vec<Message>, assistant: &mut Vec<ContentBlock>| {
        if !assistant.is_empty() {
            messages.push(Message {
                role: Role::Assistant,
                content: std::mem::take(assistant),
            });
        }
    };
    for envelope in envelopes {
        if !seen.insert(envelope.id.clone()) {
            continue; // idempotent fold: duplicate ids never double-apply
        }
        match &envelope.op {
            Op::TurnBegin { input, .. } => {
                flush(&mut messages, &mut assistant);
                messages.push(Message::user(input.clone()));
            }
            Op::AssistantText { text, .. } => {
                assistant.push(ContentBlock::Text { text: text.clone() });
            }
            Op::ToolCall {
                call_id,
                name,
                args,
                ..
            } => {
                assistant.push(ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: name.clone(),
                    input: args.clone(),
                });
            }
            Op::ToolResult {
                call_id,
                ok,
                output_digest,
                ..
            } => {
                flush(&mut messages, &mut assistant);
                messages.push(Message {
                    role: Role::Tool,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: call_id.clone(),
                        content: format!(
                            "[tool output elided from journal: ok={ok}, digest={output_digest}]"
                        ),
                        is_error: !ok,
                    }],
                });
            }
            Op::TurnEnd { .. } => flush(&mut messages, &mut assistant),
            _ => {}
        }
    }
    flush(&mut messages, &mut assistant);
    messages
}

fn fixture(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nano-c2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(root.join("nano-home")).unwrap();
    std::fs::write(ws.join("math.rs"), ORIGINAL).unwrap();
    root
}

#[tokio::test]
async fn c2_kill_mid_edit_journal_disk_equivalence_and_resume_no_double_apply() {
    let root = fixture("kill");
    let ws = root.join("workspace");
    let home = root.join("nano-home");
    let math = ws.join("math.rs");
    let before = snapshot_workspace(&ws);

    let policy =
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy();
    let fs = FsTools::new(policy, &ws);
    let shell = ShellTool::new(&home, &ws);
    let executor = RealToolExecutor::new(fs, shell, &ws);
    let approve_all = ApproveAll; // headless C2-era seam; see trackb-claim.md C2.1
    let budget = TurnBudget {
        max_steps: 12,
        max_tool_calls: 20,
        max_wall_time: std::time::Duration::from_secs(600),
    };

    // Step 1 reads, step 2 applies the real edit. Responses 3-4 are the work
    // the kill must prevent: the instant e1's result is journaled the flag
    // fires, so the fs_write is never even requested by the engine.
    let model = ScriptedModel::new(vec![
        tool_response(ToolCall {
            id: "r1".into(),
            name: "fs_read".into(),
            arguments: serde_json::json!({"path": "math.rs"}),
        }),
        tool_response(ToolCall {
            id: "e1".into(),
            name: "fs_edit".into(),
            arguments: serde_json::json!({
                "path": "math.rs",
                "old_string": EDIT_OLD,
                "new_string": EDIT_NEW,
            }),
        }),
        tool_response(ToolCall {
            id: "x1".into(),
            name: "fs_write".into(),
            arguments: serde_json::json!({
                "path": "should_not_exist.txt",
                "content": "unjournaled mutation",
            }),
        }),
        text_response("never reached"),
    ]);
    let engine = TurnEngine {
        model: &model,
        tools: &executor,
        budget: budget.clone(),
        model_name: "scripted".into(),
        tool_definitions: v1_tool_definitions(),
        approval: Some(&approve_all),
        compaction: None,
        robustness: Default::default(),
    };

    let journal_path = home.join("c2-kill-wire.jsonl");
    let kill = AtomicBool::new(false);
    let result = {
        let mut writer = JournalWriter::open(&journal_path).unwrap();
        let kill_ref = &kill;
        let mut sink = move |envelope: &OpEnvelope| {
            writer.append(envelope).expect("journal append");
            // The kill: the process dies the instant the edit's result is
            // durable. The engine stops at the next boundary — before any
            // further model call or tool execution.
            if matches!(&envelope.op, Op::ToolResult { call_id, ok, .. } if call_id == "e1" && *ok)
            {
                kill_ref.store(true, Ordering::SeqCst);
            }
            true
        };
        engine
            .run_turn_streaming("c2-kill", TASK, Some(&kill), &mut sink)
            .await
    };
    eprintln!("STATE: {:?}", result.state);

    assert!(
        matches!(result.state, TurnState::Stopped(ref r) if r.detail.contains("cancelled")),
        "kill boundary must stop the turn typed, got {:?}",
        result.state
    );
    assert!(
        !ws.join("should_not_exist.txt").exists(),
        "no side effect may land past the kill boundary"
    );

    // --- EXTERNAL-STATE DIFF: journal ⇄ disk, both directions ---
    let report = read_journal(&journal_path).expect("journal readable after kill");
    assert!(
        matches!(
            report.envelopes.last().map(|e| &e.op),
            Some(Op::TurnEnd {
                outcome: TurnOutcome::Cancelled,
                ..
            })
        ),
        "last durable op must be the typed cancellation"
    );
    let edit_calls = report
        .envelopes
        .iter()
        .filter(|e| matches!(&e.op, Op::ToolCall { name, .. } if name == "fs_edit"))
        .count();
    assert_eq!(edit_calls, 1, "exactly one edit may be journaled");
    let shadow = journaled_workspace(&report.envelopes, &before);
    let on_disk = snapshot_workspace(&ws);
    assert_eq!(
        on_disk, shadow,
        "EXTERNAL ORACLE: on-disk workspace must equal the journaled reconstruction \
         exactly — every journaled edit on disk, no unjournaled mutation"
    );

    let state = SessionState::fold(&report.envelopes);
    assert!(state.open_turn.is_none(), "cancelled turn is closed");
    assert!(state.open_tool_calls.is_empty(), "no stranded tool calls");

    // --- RESUME: the session/load path must not double-apply ---
    let bytes_before = std::fs::read(&math).unwrap();
    let hash_before = hash_bytes(&bytes_before);
    let prior = resume_context_from(&report.envelopes);
    let resume_model = ScriptedModel::new(vec![text_response("resumed: fix already on disk")]);
    let resume_engine = TurnEngine {
        model: &resume_model,
        tools: &executor,
        budget,
        model_name: "scripted".into(),
        tool_definitions: v1_tool_definitions(),
        approval: Some(&approve_all),
        compaction: None,
        robustness: Default::default(),
    };
    let resumed = {
        let mut writer = JournalWriter::open(&journal_path).unwrap();
        let mut sink = move |envelope: &OpEnvelope| {
            writer.append(envelope).expect("journal append");
            true
        };
        resume_engine
            .run_turn_streaming_with_context("c2-kill-resume", "continue", prior, None, &mut sink)
            .await
    };
    assert_eq!(resumed.state, TurnState::Complete);
    assert!(
        resumed
            .ops
            .iter()
            .all(|e| !matches!(e.op, Op::ToolCall { .. })),
        "resume must continue from the journal, never re-execute journaled tools"
    );

    let bytes_after = std::fs::read(&math).unwrap();
    let hash_after = hash_bytes(&bytes_after);
    eprintln!("RESUME HASH: {hash_before:#x} -> {hash_after:#x}");
    assert_eq!(
        hash_before, hash_after,
        "EXTERNAL ORACLE: resume double-applied a side effect (file hash changed)"
    );
    assert_eq!(bytes_before, bytes_after, "file bytes identical");

    // The resumed turn really did rebuild the interrupted turn from the
    // journal: the model saw e1's ToolUse block in its context.
    let requests = resume_model.requests.lock().unwrap();
    let saw_edit = requests[0]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "e1"));
    assert!(
        saw_edit,
        "resume context must carry the interrupted turn's journaled tool calls"
    );
    drop(requests);

    // Post-resume journal ⇄ disk still equivalent, and still exactly one
    // applied edit on record — no double-apply in the durable log either.
    let final_report = read_journal(&journal_path).expect("journal readable after resume");
    let final_shadow = journaled_workspace(&final_report.envelopes, &before);
    assert_eq!(
        snapshot_workspace(&ws),
        final_shadow,
        "EXTERNAL ORACLE: post-resume journal must still reconstruct the disk exactly"
    );
    let total_edits = final_report
        .envelopes
        .iter()
        .filter(|e| matches!(&e.op, Op::ToolCall { name, .. } if name == "fs_edit"))
        .count();
    assert_eq!(
        total_edits, 1,
        "exactly one applied edit across kill + resume — no double-apply"
    );

    let _ = std::fs::remove_dir_all(&root);
}
