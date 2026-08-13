//! `repo_map` tool (P4 design §5.5) — aider-style LEXICAL symbol query
//! over the session workspace. Read-only at every gate; never escapes
//! read policy; never auto-injected into context (tool-only in v1).
//!
//! The heavy lifting lives in `nano-repomap`; this module is the tool
//! shell: argument validation (typed `InvalidParams`), result clamping
//! (`max_results` default 20, cap 50), deterministic text rendering.
//!
//! WIRING POINTS — owned by other lanes, deliberately NOT wired here
//! (P4 lane split). To activate the tool:
//! 1. `nano-agent/src/wiring.rs` `v1_tool_definitions` (:149): append
//!    [`repo_map_tool_definition`] to the list.
//! 2. `nano-agent/src/wiring.rs` `RealToolExecutor::dispatch` (:600):
//!    add a `"repo_map"` arm calling [`RepoMapTool::execute`]; the
//!    executor gains a `repo_map: Option<RepoMapTool>` slot built from
//!    the session policy + cwd (same construction as `FsTools`).
//! 3. `nano-cli/src/acp_mode.rs` `is_read_only_tool` (:3183-3192): add
//!    `name.starts_with("repo_map")` — the read-only fast-path.
//! 4. `nano-agent/src/tasks.rs` `is_read_only_tool` (:1243-1245, the
//!    child's private copy): SAME addition — children may use the
//!    repomap (§5.5: workspace-read-only, their sandbox bounds reads).
//! 5. `nano-cli/src/exec_mode.rs` (:191-193): picks the tool up
//!    automatically through the shared predicate once (3) lands.
//! 6. The §5.5 regression test asserting all three predicates agree on
//!    `"repo_map"` lands WITH the wiring (it cannot compile green
//!    before then).

use std::path::Path;
use std::sync::Mutex;

use globset::GlobBuilder;
use nano_core::permissions::FileSystemSandboxPolicy;
use nano_repomap::{QueryResult, ReadPolicy, RepoMap};

use crate::fs::ToolError;

/// Default and hard cap for `max_results` (§5.5).
pub const DEFAULT_MAX_RESULTS: usize = 20;
pub const MAX_RESULTS_CAP: usize = 50;

/// The advertised tool definition (§5.5). Returned as a plain value so
/// the wiring lane maps it into `nano_model::types::ToolDefinition`
/// without this crate depending on the model crate.
pub fn repo_map_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "repo_map",
        "description": "Lexical symbol query over the workspace (aider-style repo map). Read-only, policy-filtered (denied-read/sensitive paths are never indexed or returned), never auto-injected. Matching is token-AND over symbol names + path substrings — `RepoMap::new` queries as the tokens `repomap` AND `new`. Args: optional query (free text), optional path_glob (git-style glob over root-relative paths, e.g. `src/**`), optional max_results (clamped to [1,50], default 20). At least one of query/path_glob is required. Returns matches as path:line kind name plus index_stats { files, symbols, last_refresh_age_ms, refreshed_files, skipped_denied } and an explicit truncated flag. Freshness: edits are picked up on the next refresh pass (throttled to one per 2s; a full hash pass runs at least once per 30s).",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "path_glob": {"type": "string"},
                "max_results": {"type": "integer"}
            }
        }
    })
}

/// The session-scoped tool handle. Holds the per-session in-memory
/// store behind a `Mutex` so query-triggered refresh works through the
/// executor's shared references. Constructed LAZILY — the first query
/// performs the initial index.
pub struct RepoMapTool {
    map: Mutex<RepoMap>,
}

impl RepoMapTool {
    pub fn new(policy: &FileSystemSandboxPolicy, cwd: &Path) -> Result<Self, ToolError> {
        let map = RepoMap::new(
            cwd,
            nano_repomap::IndexOptions::default(),
            ReadPolicy::new(policy, cwd),
        )
        .map_err(|e| match e {
            nano_repomap::RepoMapError::Root { source, .. } => ToolError::Io(source),
            // #[non_exhaustive] seam: future variants surface as typed
            // IO failures, never panics.
            other => ToolError::Io(std::io::Error::other(other)),
        })?;
        Ok(Self {
            map: Mutex::new(map),
        })
    }

    /// Execute one `repo_map` call. Read-only: performs NO filesystem
    /// writes (§5.4); the only IO is the bounded refresh read.
    pub fn execute(&self, arguments: &serde_json::Value) -> Result<String, ToolError> {
        let query = arguments.get("query").and_then(|v| v.as_str());
        let path_glob = arguments.get("path_glob").and_then(|v| v.as_str());
        if query.is_none() && path_glob.is_none() {
            return Err(ToolError::InvalidParams(
                "repo_map needs at least one of query / path_glob".into(),
            ));
        }
        let max_results = match arguments.get("max_results") {
            None => DEFAULT_MAX_RESULTS,
            Some(v) => {
                let n = v.as_u64().ok_or_else(|| {
                    ToolError::InvalidParams("max_results must be an integer".into())
                })?;
                usize::try_from(n)
                    .unwrap_or(MAX_RESULTS_CAP)
                    .clamp(1, MAX_RESULTS_CAP)
            }
        };
        let glob = match path_glob {
            None => None,
            Some(pattern) => Some(
                GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .allow_unclosed_class(true)
                    .build()
                    .map_err(|e| {
                        ToolError::InvalidParams(format!("invalid path_glob `{pattern}`: {e}"))
                    })?
                    .compile_matcher(),
            ),
        };

        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        let result = map.query(query, glob.as_ref(), max_results);
        Ok(render_result(&result))
    }
}

/// Deterministic text rendering (sorted by the query's ranking; paths
/// root-relative with `/` separators on every platform).
fn render_result(result: &QueryResult) -> String {
    let mut out = String::new();
    if result.truncated {
        out.push_str(&format!(
            "{} matches (truncated at cap):\n",
            result.matches.len()
        ));
    } else {
        out.push_str(&format!("{} matches:\n", result.matches.len()));
    }
    for m in &result.matches {
        let path = m.path.to_string_lossy().replace('\\', "/");
        out.push_str(&format!(
            "{}:{} {} {}\n",
            path,
            m.line,
            m.kind.as_str(),
            m.name
        ));
    }
    let age = result
        .stats
        .last_refresh_age_ms
        .map(|a| a.to_string())
        .unwrap_or_else(|| "never".into());
    out.push_str(&format!(
        "index: files={} symbols={} last_refresh_age_ms={} refreshed_files={} skipped_denied={}",
        result.stats.files,
        result.stats.symbols,
        age,
        result.stats.refreshed_files,
        result.stats.skipped_denied,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use nano_core::abs::AbsolutePathBuf;
    use nano_core::permissions::{
        FileSystemAccessMode, FileSystemPath, FileSystemSandboxEntry, FileSystemSpecialPath,
    };

    fn policy(ws: &Path, extra: &[(&str, FileSystemAccessMode)]) -> FileSystemSandboxPolicy {
        let mut entries = vec![
            FileSystemSandboxEntry::new(
                FileSystemPath::Special {
                    value: FileSystemSpecialPath::Root,
                },
                FileSystemAccessMode::Read,
            ),
            FileSystemSandboxEntry::new(
                FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(ws).unwrap(),
                },
                FileSystemAccessMode::Write,
            ),
        ];
        for (rel, mode) in extra {
            entries.push(FileSystemSandboxEntry::new(
                FileSystemPath::Path {
                    path: AbsolutePathBuf::from_absolute_path(ws.join(rel)).unwrap(),
                },
                *mode,
            ));
        }
        FileSystemSandboxPolicy::restricted(entries)
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("src/lib.rs"),
            "pub fn surface_query() {}\npub struct SurfaceMap;\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.join("denied")).unwrap();
        std::fs::write(ws.join("denied/hidden.rs"), "fn hidden_surface() {}\n").unwrap();
        (tmp, ws)
    }

    #[test]
    fn definition_shape_is_the_locked_surface() {
        let def = repo_map_tool_definition();
        assert_eq!(def["name"], "repo_map");
        let props = def["input_schema"]["properties"].as_object().unwrap();
        for key in ["query", "path_glob", "max_results"] {
            assert!(props.contains_key(key), "missing {key}");
        }
        // All optional: no required array naming any of them (§5.5).
        assert!(def["input_schema"].get("required").is_none());
    }

    #[test]
    fn query_renders_matches_and_stats() {
        let (_tmp, ws) = fixture();
        let tool = RepoMapTool::new(&policy(&ws, &[]), &ws).unwrap();
        let out = tool
            .execute(&serde_json::json!({"query": "surface_query"}))
            .unwrap();
        assert!(
            out.contains("src/lib.rs:1 function surface_query"),
            "out = {out}"
        );
        assert!(out.contains("index: files="), "out = {out}");
        assert!(out.contains("skipped_denied=0"), "out = {out}");
    }

    #[test]
    fn no_query_no_glob_is_typed_invalid_params() {
        let (_tmp, ws) = fixture();
        let tool = RepoMapTool::new(&policy(&ws, &[]), &ws).unwrap();
        let err = tool
            .execute(&serde_json::json!({}))
            .expect_err("unservable");
        assert!(matches!(err, ToolError::InvalidParams(_)));
    }

    #[test]
    fn malformed_glob_is_typed_invalid_params() {
        let (_tmp, ws) = fixture();
        let tool = RepoMapTool::new(&policy(&ws, &[]), &ws).unwrap();
        let err = tool
            .execute(&serde_json::json!({"path_glob": "src/[z-a].rs"}))
            .expect_err("bad glob");
        assert!(matches!(err, ToolError::InvalidParams(_)), "err = {err:?}");
    }

    #[test]
    fn max_results_clamped_to_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        let body = (0..80)
            .map(|i| format!("pub fn clamped_{i}() {{}}\n"))
            .collect::<String>();
        std::fs::write(ws.join("src/all.rs"), body).unwrap();
        let tool = RepoMapTool::new(&policy(&ws, &[]), &ws).unwrap();
        let out = tool
            .execute(&serde_json::json!({"query": "clamped", "max_results": 500}))
            .unwrap();
        assert!(
            out.starts_with("50 matches (truncated at cap):"),
            "out = {out}"
        );
        let out = tool
            .execute(&serde_json::json!({"query": "clamped"}))
            .unwrap();
        assert!(
            out.starts_with("20 matches (truncated at cap):"),
            "out = {out}"
        );
    }

    #[test]
    fn denied_paths_never_appear_in_rendered_results() {
        let (_tmp, ws) = fixture();
        let tool =
            RepoMapTool::new(&policy(&ws, &[("denied", FileSystemAccessMode::Deny)]), &ws).unwrap();
        let out = tool
            .execute(&serde_json::json!({"query": "hidden_surface"}))
            .unwrap();
        assert!(out.starts_with("0 matches:"), "out = {out}");
        // The denied path/symbol is never enumerated (the
        // `skipped_denied` COUNT in the stats line is the honest signal).
        assert!(
            !out.contains("hidden"),
            "denied content never enumerated: {out}"
        );
        assert!(out.contains("skipped_denied=1"), "counted honestly: {out}");
    }

    #[test]
    fn path_glob_scopes_results() {
        let (_tmp, ws) = fixture();
        let tool = RepoMapTool::new(&policy(&ws, &[]), &ws).unwrap();
        let out = tool
            .execute(&serde_json::json!({"path_glob": "src/**"}))
            .unwrap();
        assert!(
            out.contains("src/lib.rs:2 struct SurfaceMap"),
            "out = {out}"
        );
    }
}
