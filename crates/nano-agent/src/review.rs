//! P4 §3.1/§3.2: the review child's approval gate and tool executor.
//!
//! The review child is a constrained C6 child (tasks.rs `ChildKind::Review`)
//! with four layered constraints: the pinned prompt (review_prompt.rs),
//! FORCED read-only (this module: an executor that structurally holds only
//! fs/search tools — no shell field exists — plus a `ReviewApproval` gate
//! that denies everything not read-only-classified, PermissionMode-
//! independent), no nesting (the definition set excludes task tools; the
//! depth cap is consumed like any C6 child), and a fixed budget
//! ([`REVIEW_MAX_STEPS`], enforced through the engine's TurnBudget).

use crate::loop_protection::ProgressSignals;
use crate::turn::{ApprovalDecision, ApprovalGate, ToolExecutor, ToolOutcome};
use nano_model::types::ToolCall;
use nano_tools::fs::{FsTools, ReadBounds};
use nano_tools::search::SearchTools;
use std::path::{Path, PathBuf};

/// §3.1: the review child's fixed turn cap (8 model round-trips). The
/// token reservation rides the P1 session meter through the shared
/// TurnRobustness meter handle (cloned into every child context by the
/// registry); the wall-time bound rides the host-side watcher
/// (acp_mode `review_watcher`), which cancels past the budget.
pub const REVIEW_MAX_STEPS: u32 = 8;
/// The review child's wall-time budget (a bounded review never runs
/// longer; the host watcher enforces the same bound plus margin, so a
/// replaced-session review can never outlive its watcher).
pub const REVIEW_WALL_TIME: std::time::Duration = std::time::Duration::from_secs(600);

/// §3.1: the review gate. Approves ONLY `is_read_only_tool`-classified
/// names and denies everything else, unconditionally — it holds no
/// permission mode at all, so a `full_auto` parent cannot leak write
/// authority into a review (research risk #1 answered by construction).
/// Self-contained: `AcpApproval`'s mode arms are NOT touched.
#[derive(Debug)]
pub struct ReviewApproval;

impl ApprovalGate for ReviewApproval {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision {
        if crate::tasks::is_read_only_tool(&call.name) {
            ApprovalDecision::Approve
        } else {
            ApprovalDecision::Deny
        }
    }

    fn denial_reason(&self) -> Option<&'static str> {
        Some("review threads are read-only")
    }
}

/// §3.2: the review child's executor — an explicit allow-list surface over
/// `FsTools` + `SearchTools` (policy-filtered, sensitive-path-excluding,
/// link-safe), anchored at the child's workspace COPY root under the
/// `PermissionProfile::read_only()` filesystem policy. There is NO shell
/// field and no web/memory/task/session capability: a call to anything
/// outside {fs_read, search, glob} is a typed denial, and the gate denies
/// it before dispatch regardless (two independent closed doors).
pub struct ReviewToolExecutor {
    fs: FsTools,
    search: SearchTools,
    workspace: PathBuf,
}

// SearchTools has no Debug impl; the executor's is manual (ToolExecutor
// requires Debug).
impl std::fmt::Debug for ReviewToolExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewToolExecutor")
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

impl ReviewToolExecutor {
    pub fn new(fs: FsTools, search: SearchTools, workspace: &Path) -> Self {
        Self {
            fs,
            search,
            workspace: workspace.to_path_buf(),
        }
    }

    fn resolve(&self, raw: &str) -> PathBuf {
        let path = Path::new(raw);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }

    fn arg_str<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
        call.arguments.get(key).and_then(|v| v.as_str())
    }

    /// Non-negative integer arg: negative or non-integer is a TYPED error,
    /// never silently ignored (the wiring.rs discipline).
    fn arg_u64(call: &ToolCall, key: &str) -> Result<Option<u64>, String> {
        match call.arguments.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| format!("{key} must be a non-negative integer")),
        }
    }

    fn arg_error(message: impl Into<String>) -> ToolOutcome {
        Self::fail(message, nano_session::NanoErrorKind::MissingArgs)
    }

    fn fail(message: impl Into<String>, kind: nano_session::NanoErrorKind) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: message.into(),
            progress: ProgressSignals::default(),
            error_kind: Some(kind),
        }
    }

    fn ok(output: impl Into<String>) -> ToolOutcome {
        ToolOutcome {
            ok: true,
            output: output.into(),
            progress: ProgressSignals {
                new_information: true,
                ..Default::default()
            },
            error_kind: None,
        }
    }

    /// Render a path under the workspace root relative when possible (the
    /// reviewer thinks in workspace-relative paths; absolute forms leak
    /// nothing but noise).
    fn display(&self, path: &Path) -> String {
        match path.strip_prefix(&self.workspace) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => path.display().to_string(),
        }
    }
}

/// Result-set cap for search/glob (bounded, sorted, deterministic — the
/// SearchTools contract).
const REVIEW_SEARCH_MAX_RESULTS: usize = 200;

#[async_trait::async_trait]
impl ToolExecutor for ReviewToolExecutor {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        match call.name.as_str() {
            "fs_read" => {
                let Some(path) = Self::arg_str(call, "path") else {
                    return Self::arg_error("missing path");
                };
                let line_offset = match Self::arg_u64(call, "line_offset") {
                    Ok(v) => v.map(|v| v as usize),
                    Err(e) => return Self::arg_error(e),
                };
                let max_lines = match Self::arg_u64(call, "max_lines") {
                    Ok(v) => v.map(|v| v.clamp(1, 2000) as usize).unwrap_or(1000),
                    Err(e) => return Self::arg_error(e),
                };
                let byte_offset_in_line = match Self::arg_u64(call, "byte_offset_in_line") {
                    Ok(v) => v.map(|v| v as usize),
                    Err(e) => return Self::arg_error(e),
                };
                let bounds = ReadBounds {
                    line_offset,
                    max_lines,
                    byte_offset_in_line,
                    ..Default::default()
                };
                match self.fs.read_file(&self.resolve(path), &bounds) {
                    Ok(page) => Self::ok(crate::wiring::render_read_output(
                        line_offset.unwrap_or(0),
                        max_lines,
                        &page,
                    )),
                    Err(err) => Self::fail(err.to_string(), crate::error_map::kind_of_tool(&err)),
                }
            }
            "glob" => {
                let Some(pattern) = Self::arg_str(call, "pattern") else {
                    return Self::arg_error("missing pattern");
                };
                let root = Self::arg_str(call, "path")
                    .map(|p| self.resolve(p))
                    .unwrap_or_else(|| self.workspace.clone());
                let max_results = match Self::arg_u64(call, "max_results") {
                    Ok(v) => v
                        .map(|v| v.clamp(1, REVIEW_SEARCH_MAX_RESULTS as u64) as usize)
                        .unwrap_or(50),
                    Err(e) => return Self::arg_error(e),
                };
                match self.search.glob_files(&root, pattern, max_results) {
                    Ok(paths) => {
                        if paths.is_empty() {
                            return Self::ok("no matches");
                        }
                        let lines: Vec<String> = paths.iter().map(|p| self.display(p)).collect();
                        let mut output = lines.join("\n");
                        if paths.len() == max_results {
                            output.push_str("\n…[truncated: max_results reached]");
                        }
                        Self::ok(output)
                    }
                    Err(err) => Self::fail(err.to_string(), crate::error_map::kind_of_tool(&err)),
                }
            }
            "search" => {
                let Some(query) = Self::arg_str(call, "query") else {
                    return Self::arg_error("missing query");
                };
                let root = Self::arg_str(call, "path")
                    .map(|p| self.resolve(p))
                    .unwrap_or_else(|| self.workspace.clone());
                let max_results = match Self::arg_u64(call, "max_results") {
                    Ok(v) => v
                        .map(|v| v.clamp(1, REVIEW_SEARCH_MAX_RESULTS as u64) as usize)
                        .unwrap_or(50),
                    Err(e) => return Self::arg_error(e),
                };
                match self.search.search_content(&root, query, max_results) {
                    Ok(matches) => {
                        if matches.is_empty() {
                            return Self::ok("no matches");
                        }
                        let lines: Vec<String> = matches
                            .iter()
                            .map(|(path, line, text)| {
                                format!("{}:{line}: {}", self.display(path), text.trim_end())
                            })
                            .collect();
                        let mut output = lines.join("\n");
                        if matches.len() == max_results {
                            output.push_str("\n…[truncated: max_results reached]");
                        }
                        Self::ok(output)
                    }
                    Err(err) => Self::fail(err.to_string(), crate::error_map::kind_of_tool(&err)),
                }
            }
            // The second closed door (the gate is the first): anything not
            // on the review allow-list is denied, never executed. Gate
            // denials ride ApprovalDenied (§8).
            other => Self::fail(
                format!(
                    "{other} is not available in a review thread: review threads are read-only"
                ),
                nano_session::NanoErrorKind::ApprovalDenied,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_model::types::ToolCall;

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
        }
    }

    /// §13: the gate approves exactly the read-only classification and
    /// denies everything else with the pinned reason — mode-independent.
    #[test]
    fn review_gate_matrix() {
        let gate = ReviewApproval;
        for approved in ["fs_read", "search", "glob"] {
            assert_eq!(
                gate.approve(&call(approved, serde_json::json!({}))),
                ApprovalDecision::Approve,
                "{approved}"
            );
        }
        // §3.1: the note's predicate is the shared is_read_only_tool —
        // repo_map classifies read-only; it is unreachable anyway (not in
        // the review definition set, denied by the executor).
        assert_eq!(
            gate.approve(&call("repo_map", serde_json::json!({}))),
            ApprovalDecision::Approve
        );
        for denied in [
            "fs_write",
            "fs_edit",
            "shell",
            "web_fetch",
            "web_search",
            "task_spawn",
            "task_cancel",
            "task_apply",
            "memory_save",
            "view_image",
            "pty_spawn",
            "mcp__server__tool",
        ] {
            assert_eq!(
                gate.approve(&call(denied, serde_json::json!({}))),
                ApprovalDecision::Deny,
                "{denied}"
            );
        }
        assert_eq!(
            gate.denial_reason(),
            Some("review threads are read-only"),
            "§3.1 pinned denial reason"
        );
    }

    /// §13: a write attempt by the reviewer (the prompt-injection leg's
    /// unit half) is denied at the gate AND fails typed at the executor —
    /// the review continues read-only.
    #[tokio::test]
    async fn write_attempt_denied_gate_and_executor() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("code.rs"), "fn main() {}\n").unwrap();
        let policy =
            nano_core::permissions::PermissionProfile::read_only().file_system_sandbox_policy();
        let executor = ReviewToolExecutor::new(
            FsTools::new(policy.clone(), &ws),
            SearchTools::new(policy, &ws),
            &ws,
        );
        // Gate denial (what the engine sees before dispatch).
        let gate = ReviewApproval;
        let write = call(
            "fs_write",
            serde_json::json!({"path": "pwned.txt", "content": "x"}),
        );
        assert_eq!(gate.approve(&write), ApprovalDecision::Deny);
        // Forced invocation past the gate: typed denial at the executor.
        let outcome = executor.execute(&write).await;
        assert!(!outcome.ok);
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::ApprovalDenied)
        );
        assert!(outcome.output.contains("read-only"), "{}", outcome.output);
        assert!(!ws.join("pwned.txt").exists(), "nothing was written");
    }

    /// The three serviced tools work against a fixture workspace.
    #[tokio::test]
    async fn read_search_glob_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(ws.join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
        let policy =
            nano_core::permissions::PermissionProfile::read_only().file_system_sandbox_policy();
        let executor = ReviewToolExecutor::new(
            FsTools::new(policy.clone(), &ws),
            SearchTools::new(policy, &ws),
            &ws,
        );
        let read = executor
            .execute(&call("fs_read", serde_json::json!({"path": "src/lib.rs"})))
            .await;
        assert!(read.ok, "{}", read.output);
        assert!(read.output.contains("helper"), "{}", read.output);
        let glob = executor
            .execute(&call("glob", serde_json::json!({"pattern": "**/*.rs"})))
            .await;
        assert!(glob.ok, "{}", glob.output);
        assert!(glob.output.contains("src/lib.rs"), "{}", glob.output);
        let search = executor
            .execute(&call("search", serde_json::json!({"query": "helper"})))
            .await;
        assert!(search.ok, "{}", search.output);
        assert!(search.output.contains("src/lib.rs:1"), "{}", search.output);
        // Bad args are typed, never silent.
        let bad = executor.execute(&call("glob", serde_json::json!({}))).await;
        assert!(!bad.ok);
        assert_eq!(
            bad.error_kind,
            Some(nano_session::NanoErrorKind::MissingArgs)
        );
    }
}
