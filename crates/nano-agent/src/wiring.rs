//! Production wiring: Flux-backed ModelDriver and tool-backed ToolExecutor.

use crate::loop_protection::ProgressSignals;
use crate::turn::{ModelDriver, ToolExecutor, ToolOutcome};
use nano_model::anthropic_messages::AnthropicMessagesClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::flux_responses::FluxResponsesClient;
use nano_model::types::{ModelError, ModelRequest, ModelResponse, ToolCall, ToolDefinition};
use nano_tools::fs::{FileToken, FsTools, PageCursor, ReadBounds, ReadPage};
use nano_tools::shell::{ShellKind, ShellTool};
use nano_tools::web::{FetchArgs, WebFetchTool, render_fetch_output};

/// One of the three Flux wire surfaces. Completions is the production wire;
/// Responses and Anthropic Messages are selectable compat surfaces (per
/// FINDINGS batch-2 WIRE-2, never the default).
#[derive(Debug)]
enum FluxClient {
    Completions(FluxCompletionsClient),
    Responses(FluxResponsesClient),
    Anthropic(AnthropicMessagesClient),
}

/// ModelDriver over a Flux wire client (default: Completions).
#[derive(Debug)]
pub struct FluxDriver {
    client: FluxClient,
    api_key: String,
}

impl FluxDriver {
    /// Default construction: the production Chat Completions wire.
    pub fn new(client: FluxCompletionsClient, api_key: impl Into<String>) -> Self {
        Self {
            client: FluxClient::Completions(client),
            api_key: api_key.into(),
        }
    }

    /// Explicit opt-in: the Responses surface.
    pub fn responses(client: FluxResponsesClient, api_key: impl Into<String>) -> Self {
        Self {
            client: FluxClient::Responses(client),
            api_key: api_key.into(),
        }
    }

    /// Explicit opt-in: the Anthropic Messages COMPAT surface (thinking/cache
    /// are inert on live Flux — FINDINGS batch-2 WIRE-2).
    pub fn anthropic_compat(client: AnthropicMessagesClient, api_key: impl Into<String>) -> Self {
        Self {
            client: FluxClient::Anthropic(client),
            api_key: api_key.into(),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for FluxDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        match &self.client {
            FluxClient::Completions(client) => client.complete(request, &self.api_key).await,
            FluxClient::Responses(client) => client.complete(request, &self.api_key).await,
            FluxClient::Anthropic(client) => client.complete(request, &self.api_key).await,
        }
    }
}

/// One of the C8 provider wire surfaces (§4): the generalized OpenAI
/// chat-completions client (Flux is the `base_url = FLUX_BASE` special
/// case) or the Anthropic-native messages client. base_url + api_path come
/// from the vendored catalog row — the sole endpoint authority.
#[derive(Debug)]
enum ProviderClient {
    OpenAi(FluxCompletionsClient),
    Anthropic(AnthropicMessagesClient),
}

/// ModelDriver for a session's provider binding (C8 §5): the wire client
/// is constructed from the binding's catalog row; the credential (API key
/// or injected OAuth bearer) is presented per call exactly like FluxDriver
/// presents the Flux key.
#[derive(Debug)]
pub struct ProviderDriver {
    client: ProviderClient,
    credential: String,
}

impl ProviderDriver {
    /// OpenAI chat-completions surface (flux-router and the compat set).
    pub fn openai(client: FluxCompletionsClient, credential: impl Into<String>) -> Self {
        Self {
            client: ProviderClient::OpenAi(client),
            credential: credential.into(),
        }
    }

    /// Anthropic-native messages surface (the `anthropic` provider arm).
    pub fn anthropic(client: AnthropicMessagesClient, credential: impl Into<String>) -> Self {
        Self {
            client: ProviderClient::Anthropic(client),
            credential: credential.into(),
        }
    }
}

#[async_trait::async_trait]
impl ModelDriver for ProviderDriver {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        match &self.client {
            ProviderClient::OpenAi(client) => client.complete(request, &self.credential).await,
            ProviderClient::Anthropic(client) => client.complete(request, &self.credential).await,
        }
    }
}

/// The v1 tool surface advertised to the model.
pub fn v1_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "fs_read".into(),
            description: "Read a file in bounded pages. line_offset is 0-BASED; truncated results end with a footer carrying the next cursor (line_offset, or byte_offset_in_line for a hard-cut oversized line) and an advisory file_token. Args: path, optional line_offset, max_lines (clamped to [1,2000]), file_token, byte_offset_in_line."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "line_offset": {"type": "integer"},
                    "max_lines": {"type": "integer"},
                    "file_token": {"type": "string"},
                    "byte_offset_in_line": {"type": "integer"}
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "fs_write".into(),
            description: "Write a file (creating parents). Args: path, content.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "fs_edit".into(),
            description:
                "Exact-replacement edit. Args: path, old_string, new_string, optional replace_all."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["path", "old_string", "new_string"]
            }),
        },
        ToolDefinition {
            name: "shell".into(),
            description: "Run a shell command inside the workspace sandbox. Args: command.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "web_fetch".into(),
            description: "Fetch a URL (GET only) through the deny-by-default egress policy (a SECOND policy domain, separate from the model API allowlist; https only). Args: url, optional max_bytes (clamped to [1,65536], default 32768), timeout_ms (clamped to [1000,30000], default 15000). The body is RAW untrusted remote content — no extraction — capped at max_bytes with a marked truncation footer."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "max_bytes": {"type": "integer"},
                    "timeout_ms": {"type": "integer"}
                },
                "required": ["url"]
            }),
        },
        // ── C10 session tools ───────────────────────────────────────────
        // These are SESSION-owned: the host's session executor wrapper
        // services them (journal-first todo writes, plan posture, the gated
        // question channel). RealToolExecutor never executes them — if one
        // reaches it, the host mis-wired and the error is loud (wcore's
        // ask_user_question lesson #504: never a silent empty result).
        ToolDefinition {
            name: "todo".into(),
            description: "Read or replace the session task list. Args: optional todos — an array of {id, content, status} with status one of pending/in_progress/completed/cancelled. Omitted = read the current list; provided = replace it (journaled). Returns the full list with counts. Unavailable while plan mode is active (use the plan file)."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string"},
                                "content": {"type": "string"},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}
                            },
                            "required": ["id", "content", "status"]
                        }
                    }
                }
            }),
        },
        ToolDefinition {
            name: "ask_user".into(),
            description: "Ask the user a structured mid-turn question. Args: question, optional header, options (2-4 of {label, optional description}), optional timeout_seconds (0 = wait forever; only honored by hosts that KNOW they are interactive — capability-blind hosts normalize it to the 5-minute default). Returns the selected option's label, or a typed error when the user dismisses, the question times out, or the host cannot answer — on any error, proceed without asking."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string"},
                    "header": {"type": "string"},
                    "options": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": {"type": "string"},
                                "description": {"type": "string"}
                            },
                            "required": ["label"]
                        }
                    },
                    "timeout_seconds": {"type": "integer"}
                },
                "required": ["question", "options"]
            }),
        },
        ToolDefinition {
            name: "enter_plan_mode".into(),
            description: "Enter read-only planning posture: writes are restricted to the session's plan file (every other fs_write/fs_edit is denied at the gate in EVERY permission mode), todo is unavailable, and shell stays governed by the session's permission mode. No args.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolDefinition {
            name: "exit_plan_mode".into(),
            description: "Present the plan file to the user for approval and, on approval, exit planning posture. The exit ALWAYS asks the user — even in full_auto. On rejection the posture stays active and the feedback is returned so you can revise. No args.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        },
    ]
}

/// Session-owned tools (C10): serviced by the host's session executor
/// wrapper and the approval gate's question channel, NEVER by
/// RealToolExecutor — and ABSENT from the child tool surface (C6
/// sub-agents carry a DenyAll gate and cannot answer questions or own
/// session state; codex's root-thread-only rule). The integrator filters
/// these out of child tool definitions via [`child_tool_definitions`].
pub const SESSION_TOOL_NAMES: [&str; 4] = ["todo", "ask_user", "enter_plan_mode", "exit_plan_mode"];

/// The v1 tool surface MINUS the session-owned tools — the C6 child-agent
/// contract: a child never sees `todo`, `ask_user`, or the plan-mode tools.
pub fn child_tool_definitions() -> Vec<ToolDefinition> {
    v1_tool_definitions()
        .into_iter()
        .filter(|def| !SESSION_TOOL_NAMES.contains(&def.name.as_str()))
        .collect()
}

/// The loud-defensive error a session tool produces when it reaches
/// RealToolExecutor (mis-wired host — the session wrapper was skipped).
/// Never a silent empty result (wcore lesson #504).
fn miswired_session_tool(name: &str) -> ToolOutcome {
    ToolOutcome {
        ok: false,
        output: format!(
            "{name} reached the base executor: this host failed to route the session-owned tool through its session wrapper/approval gate. This is a host wiring bug; do not retry."
        ),
        progress: ProgressSignals::default(),
    }
}

/// ToolExecutor over the real fs/shell tools, policy-checked and sandboxed.
pub struct RealToolExecutor {
    fs: FsTools,
    shell: ShellTool,
    workspace: std::path::PathBuf,
    /// web_fetch (C4): None = no fetch hosts configured = the tool denies
    /// everything (deny-by-default for the second egress policy domain).
    web_fetch: Option<WebFetchTool>,
    /// C6 kill domain: set ONLY on background-task child executors (via
    /// [`Self::with_task_kill_registry`]). When present, shell calls route
    /// through `ShellTool::run_task` so every command's teardown handle
    /// lands in the task's registry — the construction boundary that makes
    /// the registered spawn the ONLY launch path a child holds (codex r2).
    task_kill: Option<std::sync::Arc<nano_tools::shell::KillRegistry>>,
    /// Dedup state for honest progress signals: re-reading identical content
    /// or re-running an identical command with identical output is NOT new
    /// information (the no-progress detector depends on this truth).
    seen: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// C10 §6: live-wire diff sink. When set, successful fs_write/fs_edit
    /// calls push their (call id, structured before/after pair) here (the
    /// host forwards it as an ACP diff content block on the same tool call).
    /// Live-wire-only: the hook is never journaled, and sensitive-path
    /// targets never produce a diff.
    diff_hook: Option<crate::turn::DiffHook>,
}

impl std::fmt::Debug for RealToolExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealToolExecutor")
            .field("workspace", &self.workspace)
            .field("web_fetch", &self.web_fetch.is_some())
            .field("diff_hook", &self.diff_hook.is_some())
            .finish_non_exhaustive()
    }
}

impl RealToolExecutor {
    pub fn new(fs: FsTools, shell: ShellTool, workspace: &std::path::Path) -> Self {
        Self {
            fs,
            shell,
            workspace: workspace.to_path_buf(),
            web_fetch: None,
            task_kill: None,
            seen: std::sync::Mutex::new(std::collections::HashMap::new()),
            diff_hook: None,
        }
    }

    /// Attach the live-wire diff sink (C10 §6).
    pub fn with_diff_hook(mut self, hook: crate::turn::DiffHook) -> Self {
        self.diff_hook = Some(hook);
        self
    }

    /// Emit a successful mutation's diff: capped per side, and SUPPRESSED
    /// outright for sensitive-path targets (the same deny-list the read
    /// path enforces, applied to a new exfil surface).
    fn emit_diff(
        &self,
        call_id: &str,
        path: &std::path::Path,
        old_text: Option<String>,
        new_text: String,
    ) {
        let Some(hook) = &self.diff_hook else { return };
        if nano_tools::fs::is_sensitive_path(path) {
            return;
        }
        hook(
            call_id,
            &crate::turn::FileDiff::capped(path.to_path_buf(), old_text, new_text),
        );
    }

    /// Attach the web_fetch tool with its own (second-domain) egress client
    /// built from configured fetch hosts.
    pub fn with_web_fetch(mut self, tool: WebFetchTool) -> Self {
        self.web_fetch = Some(tool);
        self
    }

    /// C6: mark this executor as a background-task child's — every shell
    /// command registers with the task's kill domain at spawn time.
    pub fn with_task_kill_registry(
        mut self,
        registry: std::sync::Arc<nano_tools::shell::KillRegistry>,
    ) -> Self {
        self.task_kill = Some(registry);
        self
    }

    /// Returns true when this (kind:key) content digest was NOT seen before.
    fn mark_and_check_novelty(&self, kind: &str, key: &str, digest_input: &str) -> bool {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        digest_input.hash(&mut hasher);
        let digest = hasher.finish();
        let mut seen = self.seen.lock().unwrap();
        seen.insert(format!("{kind}:{key}"), digest) != Some(digest)
    }

    fn arg_str<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
        call.arguments.get(key).and_then(|v| v.as_str())
    }

    fn resolve(&self, raw: &str) -> std::path::PathBuf {
        let path = std::path::Path::new(raw);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }

    /// Non-negative integer arg: negative or non-integer is a TYPED error,
    /// never silently ignored (C3 §2.4).
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
        ToolOutcome {
            ok: false,
            output: message.into(),
            progress: ProgressSignals::default(),
        }
    }
}

/// Render one fs_read page: content plus the structured footer (C3 §2.3).
/// The `…[truncated]` / `…[eof:` prefix convention is preserved so consumers
/// pattern-matching on the legacy marker keep working. No footer when the
/// page is not truncated (byte-identical to pre-C3 output).
fn render_read_output(line_offset: usize, max_lines: usize, page: &ReadPage) -> String {
    let token = page.file_token;
    match page.cursor {
        PageCursor::Eof { total_lines } => {
            if page.content.is_empty() && line_offset > 0 && line_offset >= total_lines {
                // Typed out-of-range: real total, never a fabricated page.
                format!(
                    "…[eof: line_offset {line_offset} >= total_lines {total_lines}; file_token={token}]"
                )
            } else {
                page.content.clone()
            }
        }
        PageCursor::Lines { next_line_offset } => {
            let last = next_line_offset.saturating_sub(1);
            let total_lines = page
                .total_lines
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".into());
            format!(
                "{}\n…[truncated: showing lines {line_offset}-{last} (0-based); total_bytes={}; total_lines={total_lines}; next: fs_read(path, line_offset={next_line_offset}, max_lines={max_lines}); file_token={token}]",
                page.content, page.total_bytes
            )
        }
        PageCursor::LineTruncated {
            line_offset: line,
            byte_offset_in_line,
        } => {
            format!(
                "{}\n…[truncated: line {line} cut at byte {byte_offset_in_line} (0-based intra-line); next: fs_read(path, line_offset={line}, byte_offset_in_line={byte_offset_in_line}); file_token={token}]",
                page.content
            )
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for RealToolExecutor {
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
                    // C3 §2.4: clamp to [1, 2000].
                    Ok(v) => v.map(|v| v.clamp(1, 2000) as usize).unwrap_or(1000),
                    Err(e) => return Self::arg_error(e),
                };
                let byte_offset_in_line = match Self::arg_u64(call, "byte_offset_in_line") {
                    Ok(v) => v.map(|v| v as usize),
                    Err(e) => return Self::arg_error(e),
                };
                let file_token = Self::arg_str(call, "file_token");
                let resolved = self.resolve(path);
                // Advisory freshness token (C3 §2.3): validated when PAGING
                // (line_offset > 0 or an intra-line resume) and a token was
                // supplied. Mismatch is a typed error, never silent. Calls
                // without a token page at their own risk (back-compat).
                if line_offset.unwrap_or(0) > 0 || byte_offset_in_line.unwrap_or(0) > 0 {
                    if let Some(token) = file_token {
                        let Ok(expected) = token.parse::<FileToken>() else {
                            return Self::arg_error(
                                "malformed file_token: expected m:<mtime_secs>-l:<len>",
                            );
                        };
                        match self.fs.stat_token(&resolved) {
                            Ok(current) if current == expected => {}
                            Ok(_) => {
                                return Self::arg_error(
                                    "stale_file_token: file changed since the token was issued; re-read from line_offset=0",
                                );
                            }
                            Err(err) => return Self::arg_error(err.to_string()),
                        }
                    }
                }
                let bounds = ReadBounds {
                    line_offset,
                    max_lines,
                    byte_offset_in_line,
                    ..Default::default()
                };
                match self.fs.read_file(&resolved, &bounds) {
                    Ok(page) => {
                        let novel = self.mark_and_check_novelty("read", path, &page.content);
                        ToolOutcome {
                            ok: true,
                            output: render_read_output(line_offset.unwrap_or(0), max_lines, &page),
                            progress: ProgressSignals {
                                new_information: novel,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "web_fetch" => {
                let Some(tool) = &self.web_fetch else {
                    return Self::arg_error(
                        "web_fetch denied: no fetch hosts configured (the fetch egress policy is a separate domain; set NANO_WEB_FETCH_HOSTS)",
                    );
                };
                let args = match FetchArgs::parse(&call.arguments) {
                    Ok(args) => args,
                    Err(err) => return Self::arg_error(err.to_string()),
                };
                match tool.fetch(&args).await {
                    Ok(outcome) => {
                        let body_text = String::from_utf8_lossy(&outcome.body);
                        let novel = self.mark_and_check_novelty("fetch", &args.url, &body_text);
                        ToolOutcome {
                            ok: true,
                            output: render_fetch_output(&outcome),
                            progress: ProgressSignals {
                                new_information: novel,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "fs_write" => {
                let (Some(path), Some(content)) =
                    (Self::arg_str(call, "path"), Self::arg_str(call, "content"))
                else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing path or content".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                let resolved = self.resolve(path);
                match self.fs.write_file_with_diff(&resolved, content) {
                    Ok(diff) => {
                        self.emit_diff(&call.id, &resolved, diff.old_text, diff.new_text);
                        ToolOutcome {
                            ok: true,
                            output: "written".into(),
                            progress: ProgressSignals {
                                files_changed: true,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "fs_edit" => {
                let (Some(path), Some(old), Some(new)) = (
                    Self::arg_str(call, "path"),
                    Self::arg_str(call, "old_string"),
                    Self::arg_str(call, "new_string"),
                ) else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing path/old_string/new_string".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                let replace_all = call
                    .arguments
                    .get("replace_all")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let resolved = self.resolve(path);
                match self
                    .fs
                    .edit_file_with_diff(&resolved, old, new, replace_all)
                {
                    Ok(diff) => {
                        let n = diff.replacements;
                        self.emit_diff(&call.id, &resolved, diff.old_text, diff.new_text);
                        ToolOutcome {
                            ok: true,
                            output: format!("{n} replacement(s)"),
                            progress: ProgressSignals {
                                files_changed: true,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            "shell" => {
                let Some(command) = Self::arg_str(call, "command") else {
                    return ToolOutcome {
                        ok: false,
                        output: "missing command".into(),
                        progress: ProgressSignals::default(),
                    };
                };
                // C6: a child executor's commands register with the task's
                // kill domain (no-breakaway jobs on Windows, process groups
                // on unix); the parent's executor runs the plain path.
                let shell_result = match &self.task_kill {
                    Some(registry) => self.shell.run_task(
                        ShellKind::platform_default(),
                        command,
                        Some(std::time::Duration::from_secs(120)),
                        registry,
                    ),
                    None => self.shell.run(
                        ShellKind::platform_default(),
                        command,
                        Some(std::time::Duration::from_secs(120)),
                    ),
                };
                match shell_result {
                    Ok(out) => {
                        let digest = format!("{}|{}|{}", out.exit_code, out.stdout, out.stderr);
                        let novel = self.mark_and_check_novelty("shell", command, &digest);
                        ToolOutcome {
                            ok: out.exit_code == 0,
                            output: format!(
                                "exit={}\nstdout:\n{}\nstderr:\n{}",
                                out.exit_code, out.stdout, out.stderr
                            ),
                            progress: ProgressSignals {
                                process_outcome_changed: novel,
                                new_information: novel,
                                ..Default::default()
                            },
                        }
                    }
                    Err(err) => ToolOutcome {
                        ok: false,
                        output: err.to_string(),
                        progress: ProgressSignals::default(),
                    },
                }
            }
            // C10: session-owned tools must never reach the base executor.
            // A host that skipped its session wrapper gets the loud
            // mis-wiring error, never a silent empty or "unknown" result.
            name if SESSION_TOOL_NAMES.contains(&name) => miswired_session_tool(name),
            other => ToolOutcome {
                ok: false,
                output: format!("unknown tool: {other}"),
                progress: ProgressSignals::default(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C2 (claude concern 3, prospective pin): the full_auto gate approves
    /// `shell` on sandbox-backend availability ALONE, blind to arguments.
    /// That is only sound while the shell tool's schema carries no
    /// sandbox-relaxing argument surface (no escalation/unsandboxed-request
    /// analogue). This test pins the schema: ANY change to shell's argument
    /// surface must update this test AND re-audit the gate's
    /// argument-blindness (design §4 invariant — a relaxing argument must
    /// be inspected-and-rejected by the gate or ignored by the tool under
    /// ALL modes).
    #[test]
    fn shell_schema_has_no_sandbox_relaxing_arguments() {
        let defs = v1_tool_definitions();
        let shell = defs
            .iter()
            .find(|d| d.name == "shell")
            .expect("shell tool advertised");
        let properties = shell.input_schema["properties"]
            .as_object()
            .expect("shell properties");
        let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["command"],
            "shell gained an argument — re-audit the full_auto gate's argument-blindness"
        );
        assert_eq!(
            shell.input_schema["required"],
            serde_json::json!(["command"])
        );
    }

    /// C2 (claude concern 2): the full_auto gate extracts `path` from
    /// fs_write/fs_edit arguments for its containment check. If either tool
    /// renames that argument, full_auto silently degrades to default (every
    /// write prompts) — the per-mode matrix in nano-cli asserts a contained
    /// fs_edit auto-approves, and THIS pin catches the rename at the source.
    #[test]
    fn fs_write_and_fs_edit_take_a_path_argument() {
        let defs = v1_tool_definitions();
        for name in ["fs_write", "fs_edit"] {
            let def = defs.iter().find(|d| d.name == name).expect(name);
            assert!(
                def.input_schema["properties"].get("path").is_some(),
                "{name} lost its `path` argument"
            );
            assert!(
                def.input_schema["required"]
                    .as_array()
                    .expect("required")
                    .iter()
                    .any(|r| r == "path"),
                "{name} no longer requires `path`"
            );
        }
    }

    // --- C3/C4 executor-level battery -----------------------------------------

    use nano_model::types::ToolCall as _ToolCall;

    fn executor_fixture() -> (tempfile::TempDir, RealToolExecutor, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let policy = nano_core::permissions::PermissionProfile::workspace_write()
            .file_system_sandbox_policy();
        let fs = FsTools::new(policy, &ws);
        let shell = ShellTool::new(&home, &ws);
        let executor = RealToolExecutor::new(fs, shell, &ws);
        (tmp, executor, ws)
    }

    fn call(name: &str, arguments: serde_json::Value) -> _ToolCall {
        _ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments,
        }
    }

    fn extract_token(output: &str) -> String {
        output
            .split("file_token=")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("footer carries file_token")
            .to_string()
    }

    /// Back-compat: a small-file default read is byte-identical to pre-C3
    /// output (content only, no footer).
    #[tokio::test]
    async fn fs_read_small_file_output_is_byte_identical() {
        let (_tmp, executor, ws) = executor_fixture();
        std::fs::write(ws.join("note.txt"), "hello\nworld\n").unwrap();
        let outcome = executor
            .execute(&call("fs_read", serde_json::json!({"path": "note.txt"})))
            .await;
        assert!(outcome.ok);
        assert_eq!(outcome.output, "hello\nworld");
    }

    /// Truncated page footer: 0-based line range, real total_bytes, total
    /// lines unknown (no EOF), next cursor, and the freshness token.
    #[tokio::test]
    async fn fs_read_truncated_footer_carries_cursor_and_token() {
        let (_tmp, executor, ws) = executor_fixture();
        let body = (0..3000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(ws.join("big.txt"), &body).unwrap();
        let outcome = executor
            .execute(&call("fs_read", serde_json::json!({"path": "big.txt"})))
            .await;
        assert!(outcome.ok);
        assert!(outcome.output.starts_with("line 0\n"));
        assert!(
            outcome
                .output
                .contains("…[truncated: showing lines 0-999 (0-based); total_bytes="),
            "footer: {}",
            &outcome.output[outcome.output.len() - 200..]
        );
        assert!(outcome.output.contains("total_lines=unknown"));
        assert!(
            outcome
                .output
                .contains("next: fs_read(path, line_offset=1000, max_lines=1000)")
        );
        assert!(outcome.output.contains("file_token=m:"));
    }

    /// max_lines clamps to [1, 2000]; non-integer/negative args are typed
    /// errors (C3 §2.4).
    #[tokio::test]
    async fn fs_read_clamps_max_lines_and_types_bad_args() {
        let (_tmp, executor, ws) = executor_fixture();
        let body = (0..3000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(ws.join("big.txt"), &body).unwrap();
        let outcome = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "big.txt", "max_lines": 1_000_000_000_u64}),
            ))
            .await;
        assert!(outcome.ok);
        assert_eq!(outcome.output.matches('\n').count(), 2000 - 1 + 1); // 2000 lines + footer
        assert!(
            outcome
                .output
                .contains("next: fs_read(path, line_offset=2000, max_lines=2000)")
        );
        let outcome = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "big.txt", "max_lines": 0}),
            ))
            .await;
        assert!(outcome.ok);
        assert!(
            outcome
                .output
                .starts_with("line 0\n…[truncated: showing lines 0-0")
        );
        let outcome = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "big.txt", "line_offset": -1}),
            ))
            .await;
        assert!(!outcome.ok, "negative offset is a typed error");
        assert!(
            outcome
                .output
                .contains("line_offset must be a non-negative integer")
        );
    }

    /// Out-of-range offset: typed EOF footer with the REAL total (ok: true,
    /// empty content), never a fabricated page.
    #[tokio::test]
    async fn fs_read_out_of_range_is_typed_eof() {
        let (_tmp, executor, ws) = executor_fixture();
        std::fs::write(ws.join("short.txt"), "a\nb\nc\n").unwrap();
        let outcome = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "short.txt", "line_offset": 5000}),
            ))
            .await;
        assert!(outcome.ok);
        assert_eq!(
            outcome.output,
            format!(
                "…[eof: line_offset 5000 >= total_lines 3; file_token={}]",
                extract_token(&outcome.output)
            )
        );
        assert!(
            outcome
                .output
                .starts_with("…[eof: line_offset 5000 >= total_lines 3;")
        );
    }

    /// Oversized line: the footer carries the intra-line byte cursor, and
    /// the resume call continues from it.
    #[tokio::test]
    async fn fs_read_oversized_line_footer_and_byte_resume() {
        let (_tmp, executor, ws) = executor_fixture();
        let long_line = "a".repeat(150 * 1024);
        std::fs::write(ws.join("long.txt"), format!("{long_line}\ntail")).unwrap();
        let outcome = executor
            .execute(&call("fs_read", serde_json::json!({"path": "long.txt"})))
            .await;
        assert!(outcome.ok);
        assert!(
            outcome.output.contains(
                "…[truncated: line 0 cut at byte 102400 (0-based intra-line); next: fs_read(path, line_offset=0, byte_offset_in_line=102400)"
            ),
            "footer: {}",
            &outcome.output[outcome.output.len() - 250..]
        );
        let token = extract_token(&outcome.output);
        let outcome = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({
                    "path": "long.txt",
                    "line_offset": 0,
                    "byte_offset_in_line": 102400,
                    "file_token": token,
                }),
            ))
            .await;
        assert!(outcome.ok, "resume: {}", outcome.output);
        assert!(outcome.output.ends_with("tail"));
    }

    /// Freshness token (C3 §2.3): an edit between reads invalidates a paged
    /// call carrying the old token (typed error); the same flow WITHOUT a
    /// token pages at its own risk; re-reading from 0 issues a fresh token
    /// that pages coherently.
    #[tokio::test]
    async fn fs_read_stale_token_is_typed_error() {
        let (_tmp, executor, ws) = executor_fixture();
        let body = (0..3000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(ws.join("fresh.txt"), &body).unwrap();
        let page1 = executor
            .execute(&call("fs_read", serde_json::json!({"path": "fresh.txt"})))
            .await;
        let token = extract_token(&page1.output);

        // Edit between reads (length change — the case the advisory token
        // CAN detect; coarse-mtime + same-length edits are documented out
        // of scope for the token).
        std::fs::write(ws.join("fresh.txt"), format!("{body}\nextra")).unwrap();

        let stale = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "fresh.txt", "line_offset": 1000, "file_token": token}),
            ))
            .await;
        assert!(!stale.ok);
        assert_eq!(
            stale.output,
            "stale_file_token: file changed since the token was issued; re-read from line_offset=0"
        );

        // Documented at-risk path: no token → pages anyway.
        let no_token = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "fresh.txt", "line_offset": 1000}),
            ))
            .await;
        assert!(no_token.ok);
        assert!(no_token.output.starts_with("line 1000\n"));

        // Re-read from 0 issues a fresh token that pages coherently.
        let fresh = executor
            .execute(&call("fs_read", serde_json::json!({"path": "fresh.txt"})))
            .await;
        let fresh_token = extract_token(&fresh.output);
        let page2 = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "fresh.txt", "line_offset": 1000, "file_token": fresh_token}),
            ))
            .await;
        assert!(page2.ok, "fresh token must page: {}", page2.output);
        assert!(page2.output.starts_with("line 1000\n"));

        // Malformed token: typed error, never silently ignored.
        let malformed = executor
            .execute(&call(
                "fs_read",
                serde_json::json!({"path": "fresh.txt", "line_offset": 1000, "file_token": "bogus"}),
            ))
            .await;
        assert!(!malformed.ok);
        assert!(malformed.output.contains("malformed file_token"));
    }

    /// web_fetch with no configured fetch hosts: typed denial — the second
    /// egress domain is deny-by-default even though the tool is compiled in.
    #[tokio::test]
    async fn web_fetch_unconfigured_is_typed_denial() {
        let (_tmp, executor, _ws) = executor_fixture();
        let outcome = executor
            .execute(&call(
                "web_fetch",
                serde_json::json!({"url": "https://example.com/"}),
            ))
            .await;
        assert!(!outcome.ok);
        assert!(
            outcome
                .output
                .contains("web_fetch denied: no fetch hosts configured")
        );
    }

    /// Loop protection: fetching the same URL twice with an identical body
    /// scores non-novel (the same digest path as fs_read).
    #[tokio::test]
    async fn web_fetch_identical_refetch_scores_non_novel() {
        use nano_egress::client::{FetchDriver, FetchHop};
        #[derive(Debug)]
        struct FakeDriver;
        #[async_trait::async_trait]
        impl FetchDriver for FakeDriver {
            async fn resolve(
                &self,
                _host: &str,
                _port: u16,
            ) -> std::io::Result<Vec<std::net::IpAddr>> {
                Ok(vec!["93.184.216.34".parse().unwrap()])
            }
            async fn send(
                &self,
                _host: &str,
                _port: u16,
                _addrs: &[std::net::IpAddr],
                _url: &str,
                _timeout: std::time::Duration,
            ) -> Result<FetchHop, nano_egress::client::EgressError> {
                Ok(FetchHop {
                    status: 200,
                    location: None,
                    content_type: Some("text/plain".into()),
                    content_length: Some(11),
                    body: Box::pin(futures_util::stream::iter(vec![Ok::<
                        Vec<u8>,
                        nano_egress::client::EgressError,
                    >(
                        b"hello world".to_vec()
                    )])),
                })
            }
        }
        let (_tmp, executor, _ws) = executor_fixture();
        let client = nano_egress::client::EgressClient::new(
            nano_egress::policy::EgressPolicy::new().allow_host("example.com"),
        )
        .with_fetch_driver_for_tests(std::sync::Arc::new(FakeDriver));
        let executor = executor.with_web_fetch(nano_tools::web::WebFetchTool::new(client));

        let first = executor
            .execute(&call(
                "web_fetch",
                serde_json::json!({"url": "https://example.com/"}),
            ))
            .await;
        assert!(first.ok, "first fetch: {}", first.output);
        assert!(first.progress.new_information);
        assert!(first.output.contains("status: 200"));
        assert!(first.output.contains("hello world"));
        assert!(first.output.contains("declared_bytes: 11"));
        assert!(first.output.contains("untrusted remote content"));

        let second = executor
            .execute(&call(
                "web_fetch",
                serde_json::json!({"url": "https://example.com/"}),
            ))
            .await;
        assert!(second.ok);
        assert!(
            !second.progress.new_information,
            "identical refetch must score non-novel"
        );
    }

    // ── C10 tests ───────────────────────────────────────────────────────

    /// The v1 surface advertises the session tools; the C6 child surface
    /// (child_tool_definitions) excludes ALL of them — children cannot own
    /// session state or answer questions (codex root-thread-only rule).
    #[test]
    fn session_tools_advertised_but_absent_from_the_child_surface() {
        let v1 = v1_tool_definitions();
        for name in SESSION_TOOL_NAMES {
            assert!(v1.iter().any(|d| d.name == name), "{name} advertised");
        }
        let child = child_tool_definitions();
        for name in SESSION_TOOL_NAMES {
            assert!(
                !child.iter().any(|d| d.name == name),
                "{name} must be absent from the child tool surface"
            );
        }
        // The child surface is otherwise intact.
        for name in ["fs_read", "fs_write", "fs_edit", "shell", "web_fetch"] {
            assert!(child.iter().any(|d| d.name == name), "{name} kept");
        }
    }

    /// A session tool that reaches the BASE executor (mis-wired host)
    /// returns the loud-defensive typed error — never a silent empty
    /// result (wcore lesson #504).
    #[tokio::test]
    async fn session_tools_at_the_base_executor_fail_loud() {
        let (_tmp, executor, _ws) = executor_fixture();
        for name in SESSION_TOOL_NAMES {
            let outcome = executor.execute(&call(name, serde_json::json!({}))).await;
            assert!(!outcome.ok);
            assert!(
                outcome.output.contains("reached the base executor"),
                "{name}: {}",
                outcome.output
            );
        }
    }

    /// C10 §6: write/edit diffs flow through the hook — whole-file add
    /// (old_text None) on create, before/after on overwrite and edit —
    /// while the model-facing output strings stay byte-identical.
    #[tokio::test]
    async fn write_and_edit_emit_structured_diffs() {
        let (_tmp, executor, ws) = executor_fixture();
        let diffs: std::sync::Arc<std::sync::Mutex<Vec<crate::turn::FileDiff>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = diffs.clone();
        let executor = executor.with_diff_hook(std::sync::Arc::new(move |_id, diff| {
            sink.lock().unwrap().push(diff.clone());
        }));

        // Create: whole-file add.
        let outcome = executor
            .execute(&call(
                "fs_write",
                serde_json::json!({"path": "a.txt", "content": "v1"}),
            ))
            .await;
        assert!(outcome.ok);
        assert_eq!(outcome.output, "written", "model-facing output unchanged");
        {
            let diffs = diffs.lock().unwrap();
            assert_eq!(diffs.len(), 1);
            assert_eq!(diffs[0].path, ws.join("a.txt"));
            assert_eq!(diffs[0].old_text, None, "create is a whole-file add");
            assert_eq!(diffs[0].new_text, "v1");
        }
        // Overwrite: read-before diff.
        executor
            .execute(&call(
                "fs_write",
                serde_json::json!({"path": "a.txt", "content": "v2"}),
            ))
            .await;
        // Edit: region old/new.
        let outcome = executor
            .execute(&call(
                "fs_edit",
                serde_json::json!({"path": "a.txt", "old_string": "v2", "new_string": "v3"}),
            ))
            .await;
        assert!(outcome.ok);
        assert_eq!(outcome.output, "1 replacement(s)", "output unchanged");
        let diffs = diffs.lock().unwrap();
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[1].old_text.as_deref(), Some("v1"));
        assert_eq!(diffs[1].new_text, "v2");
        assert_eq!(diffs[2].old_text.as_deref(), Some("v2"));
        assert_eq!(diffs[2].new_text, "v3");
    }

    /// C10 §6 egress discipline: a sensitive-path target emits NO diff
    /// (the write still succeeds when the caller holds the sensitive
    /// override — only the exfil surface is closed), and each diff side is
    /// capped at 32k chars with the deterministic elision marker.
    #[tokio::test]
    async fn sensitive_paths_emit_no_diff_and_sides_are_capped() {
        let (_tmp, executor, ws) = executor_fixture();
        let diffs: std::sync::Arc<std::sync::Mutex<Vec<crate::turn::FileDiff>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = diffs.clone();
        let executor = executor.with_diff_hook(std::sync::Arc::new(move |_id, diff| {
            sink.lock().unwrap().push(diff.clone());
        }));
        // The executor fixture's FsTools denies sensitive paths at the
        // write layer; the DIFF-suppression rule is a separate, additional
        // guard on the exfil surface. Exercise it through a path the write
        // policy allows: write a NON-sensitive file, then verify the
        // suppression predicate directly on a sensitive spelling.
        assert!(nano_tools::fs::is_sensitive_path(std::path::Path::new(
            ".env"
        )));
        assert!(nano_tools::fs::is_sensitive_path(std::path::Path::new(
            "id_rsa"
        )));
        assert!(!nano_tools::fs::is_sensitive_path(std::path::Path::new(
            "ok.txt"
        )));

        // Cap: an 80k write truncates each side with the elision marker.
        let big = "x".repeat(80 * 1024);
        let outcome = executor
            .execute(&call(
                "fs_write",
                serde_json::json!({"path": "big.txt", "content": big}),
            ))
            .await;
        assert!(outcome.ok);
        let diffs = diffs.lock().unwrap();
        assert_eq!(diffs.len(), 1);
        let new_text = &diffs[0].new_text;
        assert!(new_text.contains("…[elided"), "{new_text}");
        assert!(
            new_text.chars().count() <= crate::turn::FileDiff::MAX_SIDE_CHARS + 64,
            "capped: {} chars",
            new_text.chars().count()
        );
        // Deterministic.
        let again = crate::turn::FileDiff::capped(ws.join("big.txt"), None, "x".repeat(80 * 1024));
        assert_eq!(&diffs[0].new_text, &again.new_text);
        let _ = ws;
    }

    /// C10 §6: with the sensitive OVERRIDE held (the write is allowed), the
    /// diff is STILL suppressed from the wire hook — the egress rule is
    /// independent of the write authorization.
    #[tokio::test]
    async fn sensitive_write_with_override_still_emits_no_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let policy = nano_core::permissions::PermissionProfile::workspace_write()
            .file_system_sandbox_policy();
        let fs = FsTools::new(policy, &ws).with_sensitive_override();
        let shell = ShellTool::new(&home, &ws);
        let diffs: std::sync::Arc<std::sync::Mutex<Vec<crate::turn::FileDiff>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = diffs.clone();
        let executor = RealToolExecutor::new(fs, shell, &ws).with_diff_hook(std::sync::Arc::new(
            move |_id, diff| {
                sink.lock().unwrap().push(diff.clone());
            },
        ));
        let outcome = executor
            .execute(&call(
                "fs_write",
                serde_json::json!({"path": ".env", "content": "SECRET=x"}),
            ))
            .await;
        assert!(outcome.ok, "override permits the write: {}", outcome.output);
        assert!(
            diffs.lock().unwrap().is_empty(),
            "sensitive-path targets never emit a diff"
        );
    }
}
