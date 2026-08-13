//! Session-owned tools and the plan posture (C10): the `todo` tool
//! (journaled journal-first), plan mode (a read-only POSTURE orthogonal to
//! the C2 privilege modes), and the structured-question plumbing shared by
//! `ask_user` and the plan-exit approval.
//!
//! One transition rule, pack-wide: EVERY plan-posture mutation goes through
//! [`set_plan_posture`] — journal-first (`Op::PlanSet` lands durably, and
//! ONLY on append success does the cell flip). The ACP set_mode handler,
//! the TUI /plan (over the wire), and the tool entry path all converge on
//! it, so every entry path produces the identical journal + state. The
//! posture is never restored on session/load: content replays, postures
//! don't.

use nano_agent::loop_protection::ProgressSignals;
use nano_agent::turn::{ApprovalGate, AskOutcome, ToolExecutor, ToolOutcome};
use nano_model::types::ToolCall;
use nano_session::op::{Op, OpEnvelope, TodoItem, TodoStatus};
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The label that approves a plan exit (C10 §3). Any other answer keeps the
/// posture and returns the label as revise feedback.
pub const PLAN_EXIT_APPROVE_LABEL: &str = "Approve plan";
/// The revise-loop label offered beside it.
pub const PLAN_EXIT_REVISE_LABEL: &str = "Keep planning";

/// The session's plan posture and its plan file (C10 §3, Q5 RULED: the plan
/// file lives in the SESSION storage under nano_home —
/// `<sessions_dir>/<session-id>.plan.md`, sibling of the journal — and the
/// write exception is a nano_home-containment check, NEVER a workspace
/// exception).
#[derive(Debug)]
pub struct PlanPosture {
    pub active: bool,
    /// The plan file as joined (display/debug spelling).
    plan_file: PathBuf,
    /// Canonicalized sessions dir (resolved at construction — the journal
    /// is already open there, so it is guaranteed to exist). The leaf is
    /// NEVER canonicalized: it may not exist yet (creation-safe check).
    canonical_parent: PathBuf,
    /// The fixed leaf name (`<session-id>.plan.md`).
    file_name: std::ffi::OsString,
}

impl PlanPosture {
    /// Fail-closed construction: a session whose sessions dir cannot be
    /// canonicalized gets NO plan posture (the caller turns this into a
    /// typed session-creation error).
    pub fn new(sessions_dir: &Path, session_id: &str) -> std::io::Result<Self> {
        let canonical_parent = std::fs::canonicalize(sessions_dir)?;
        let file_name = std::ffi::OsString::from(format!("{session_id}.plan.md"));
        Ok(Self {
            active: false,
            plan_file: sessions_dir.join(&file_name),
            canonical_parent,
            file_name,
        })
    }

    /// The plan file path (for display and the tool-layer policy root).
    pub fn plan_file(&self) -> &Path {
        &self.plan_file
    }

    /// Creation-safe containment check on an ALREADY-RESOLVED absolute
    /// target (the gate and the executor share the same resolve() spelling,
    /// so authorization and mutation validate the exact same path — no
    /// post-gate re-resolution). The parent is canonicalized (resolving
    /// symlinks/reparse points) and compared against the canonical sessions
    /// dir; the leaf is name-compared, never canonicalized. A symlinked or
    /// reparsed parent that escapes nano_home resolves to a different
    /// canonical parent and fails closed; any canonicalize failure denies.
    pub fn is_plan_file(&self, resolved_target: &Path) -> bool {
        if resolved_target.file_name() != Some(self.file_name.as_os_str()) {
            return false;
        }
        let Some(parent) = resolved_target.parent() else {
            return false;
        };
        std::fs::canonicalize(parent)
            .map(|canonical| canonical == self.canonical_parent)
            .unwrap_or(false)
    }
}

/// The ONE plan-posture transition (C10 §3): journal-first —
/// `Op::PlanSet` is appended durably, and ONLY on success does the cell
/// flip. Append failure ⇒ state unchanged and a typed error to the caller.
pub fn set_plan_posture(
    posture: &Arc<Mutex<PlanPosture>>,
    coordinator: &nano_session::JournalCoordinator,
    op_id: String,
    active: bool,
) -> Result<(), String> {
    let envelope = OpEnvelope::new(op_id, "now", Op::PlanSet { active });
    coordinator
        .append(&envelope)
        .map_err(|err| format!("cannot journal plan transition: {err}"))?;
    posture.lock().unwrap_or_else(|p| p.into_inner()).active = active;
    Ok(())
}

/// The bounded system block injected while the posture is active (C10 §3
/// prompt layer — defense in depth, NEVER the mechanism; the gate is the
/// enforcement). Adapted from codex's plan.md mode rules; shell
/// non-mutation is prompt-level guidance, honestly not enforced (§10).
pub fn plan_mode_instructions(plan_file: &Path) -> String {
    format!(
        "PLAN MODE ACTIVE — read-only planning posture.\n\
         - Explore and read freely. You MUST NOT mutate the workspace: fs_write/fs_edit to any path other than the plan file are denied at the approval gate in every permission mode.\n\
         - Write your plan to the plan file: {}\n\
         - The todo tool is unavailable while planning; the plan file is the checklist.\n\
         - Shell commands remain governed by the session's permission mode — keep them non-mutating (this is guidance, not mechanically enforced).\n\
         - When the plan is ready, call exit_plan_mode: the user is asked to approve it (even in full_auto), and only then does the posture lift.",
        plan_file.display()
    )
}

/// Plan-exit prompt cap (C10 §3, claude NB): 8k head + 8k tail with a
/// deterministic elision marker — a multi-hundred-kB plan file must not
/// flood the session/request_permission frame.
pub const PLAN_EXIT_HEAD_CHARS: usize = 8_000;
pub const PLAN_EXIT_TAIL_CHARS: usize = 8_000;

/// Cap the plan text for presentation: head + tail with an elision marker.
pub fn cap_plan_text(text: &str) -> String {
    let total = text.chars().count();
    if total <= PLAN_EXIT_HEAD_CHARS + PLAN_EXIT_TAIL_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(PLAN_EXIT_HEAD_CHARS).collect();
    let tail: String = text.chars().skip(total - PLAN_EXIT_TAIL_CHARS).collect();
    format!(
        "{head}\n…[elided {} chars]…\n{tail}",
        total - PLAN_EXIT_HEAD_CHARS - PLAN_EXIT_TAIL_CHARS
    )
}

/// Todo re-injection bounds (C10 §2, Q2 RULED): 50 items / 4k chars,
/// deterministic truncation, explicit delimiters marking it as restored
/// session state.
pub const TODO_RESTORE_MAX_ITEMS: usize = 50;
pub const TODO_RESTORE_MAX_CHARS: usize = 4_000;

/// Render the todo list as the bounded, clearly-delimited re-injection
/// block used when context is rebuilt after a resume (C10 §2 Q2). `None`
/// when the list is empty (no block, no noise).
pub fn todo_restore_block(todos: &[TodoItem]) -> Option<String> {
    if todos.is_empty() {
        return None;
    }
    let mut out = String::from(
        "[Restored session todo list — session state rebuilt from the journal, not a new instruction]\n",
    );
    let mut shown = 0usize;
    for item in todos.iter().take(TODO_RESTORE_MAX_ITEMS) {
        let line = format!("- [{}] {}: {}\n", item.status.id(), item.id, item.content);
        if out.chars().count() + line.chars().count() > TODO_RESTORE_MAX_CHARS {
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    let total = todos.len();
    if shown < total {
        out.push_str(&format!("…[{total} items, showing {shown}]\n"));
    }
    out.push_str("[End of restored todo list]");
    Some(out)
}

/// The model-facing full-list rendering (wcore shape: the list + counts).
pub fn render_todo_list(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "todo list is empty".to_string();
    }
    let counts = |pred: fn(TodoStatus) -> bool| todos.iter().filter(|t| pred(t.status)).count();
    let mut out = format!(
        "{} item(s) ({} pending, {} in_progress, {} completed, {} cancelled):",
        todos.len(),
        counts(|s| s == TodoStatus::Pending),
        counts(|s| s == TodoStatus::InProgress),
        counts(|s| s == TodoStatus::Completed),
        counts(|s| s == TodoStatus::Cancelled),
    );
    for item in todos {
        out.push_str(&format!(
            "\n- [{}] {}: {}",
            item.status.id(),
            item.id,
            item.content
        ));
    }
    out
}

/// Validate model-supplied todo args into items (fail-closed: any malformed
/// entry rejects the whole write, never a partial mutation).
pub fn parse_todo_items(value: &serde_json::Value) -> Result<Vec<TodoItem>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| "todos must be an array of {id, content, status}".to_string())?;
    let mut items = Vec::with_capacity(array.len());
    for (index, entry) in array.iter().enumerate() {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("todos[{index}].id must be a non-empty string"))?;
        let content = entry
            .get("content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("todos[{index}].content must be a non-empty string"))?;
        let status_raw = entry
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("todos[{index}].status must be a string"))?;
        let status = TodoStatus::parse(status_raw)
            .ok_or_else(|| format!("todos[{index}].status: unknown status {status_raw:?}"))?;
        items.push(TodoItem {
            id: id.to_string(),
            content: content.to_string(),
            status,
        });
    }
    Ok(items)
}

/// The session-owned tool executor (C10): wraps the turn's real executor
/// and services `todo` / `enter_plan_mode` / `exit_plan_mode` / `ask_user`
/// against the session's cells — everything else delegates. The question
/// channel is the gate's ONE `ask` method (reuse, never parallel
/// machinery): `ask_user` routes through it directly, and the plan-exit
/// approval round-trips through it too.
pub struct SessionTools<'a> {
    inner: &'a dyn ToolExecutor,
    gate: &'a dyn ApprovalGate,
    todos: Arc<Mutex<Vec<TodoItem>>>,
    posture: Arc<Mutex<PlanPosture>>,
    coordinator: Arc<nano_session::JournalCoordinator>,
    session_id: String,
    /// Monotonic per-lifetime counter for TodoSet/PlanSet op ids (the id
    /// also carries nanoseconds, so resumes never collide) — the C2
    /// ModeSet id pattern.
    op_counter: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for SessionTools<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionTools")
            .field("session_id", &self.session_id)
            .field("journal", &self.coordinator.path())
            .finish_non_exhaustive()
    }
}

impl<'a> SessionTools<'a> {
    pub fn new(
        inner: &'a dyn ToolExecutor,
        gate: &'a dyn ApprovalGate,
        todos: Arc<Mutex<Vec<TodoItem>>>,
        posture: Arc<Mutex<PlanPosture>>,
        coordinator: Arc<nano_session::JournalCoordinator>,
        session_id: String,
    ) -> Self {
        Self {
            inner,
            gate,
            todos,
            posture,
            coordinator,
            session_id,
            op_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn next_op_id(&self, kind: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = self
            .op_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("{}-{kind}-{nanos}-{n}", self.session_id)
    }

    fn error(message: impl Into<String>) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: message.into(),
            progress: ProgressSignals::default(),
            // C7 field; per-kind typing for session-tool errors is a
            // tracked integration follow-up.
            error_kind: None,
        }
    }

    fn plain(output: impl Into<String>) -> ToolOutcome {
        ToolOutcome {
            ok: true,
            output: output.into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }

    fn plan_active(&self) -> bool {
        self.posture
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
    }

    fn plan_file(&self) -> PathBuf {
        self.posture
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .plan_file()
            .to_path_buf()
    }

    /// `todo` (C10 §2): omitted `todos` = read (mutates nothing); provided
    /// = replace, journal-first (validate → durable append → mutate). BOTH
    /// paths are auto-allowed in every C2 mode (the gate handles that); the
    /// write path's only failure mode is the append failing, which leaves
    /// the visible list unchanged. REJECTED under the plan posture (codex
    /// plan.rs:84 precedent) — planning uses the plan file, not the list.
    fn execute_todo(&self, call: &ToolCall) -> ToolOutcome {
        if self.plan_active() {
            return Self::error(
                "todo is unavailable while plan mode is active; the plan file is the checklist",
            );
        }
        let Some(raw) = call.arguments.get("todos").filter(|v| !v.is_null()) else {
            // Read fast-path.
            let todos = self.todos.lock().unwrap_or_else(|p| p.into_inner()).clone();
            return Self::plain(render_todo_list(&todos));
        };
        let items = match parse_todo_items(raw) {
            Ok(items) => items,
            Err(err) => return Self::error(format!("todo rejected: {err}")),
        };
        // Journal-first, accepted-only: the append must be durable before
        // the cell mutates; a failed append leaves the list visibly unchanged.
        let envelope = OpEnvelope::new(
            self.next_op_id("todo"),
            "now",
            Op::TodoSet {
                items: items.clone(),
            },
        );
        if let Err(err) = self.coordinator.append(&envelope) {
            return Self::error(format!("todo write failed (list unchanged): {err}"));
        }
        *self.todos.lock().unwrap_or_else(|p| p.into_inner()) = items;
        let todos = self.todos.lock().unwrap_or_else(|p| p.into_inner()).clone();
        Self::plain(render_todo_list(&todos))
    }

    /// `enter_plan_mode` (C10 §3 tool entry): the same journal-first
    /// transition every other entry path uses.
    fn execute_enter_plan(&self) -> ToolOutcome {
        if self.plan_active() {
            return Self::plain(format!(
                "plan mode is already active; the plan file is {}",
                self.plan_file().display()
            ));
        }
        if let Err(err) = set_plan_posture(
            &self.posture,
            &self.coordinator,
            self.next_op_id("plan"),
            true,
        ) {
            return Self::error(format!("cannot enter plan mode: {err}"));
        }
        Self::plain(format!(
            "plan mode is now active.\n{}",
            plan_mode_instructions(&self.plan_file())
        ))
    }

    /// `exit_plan_mode` (C10 §3): read the plan file, round-trip the host
    /// through the SAME question channel as ask_user, and ONLY on explicit
    /// approval flip the posture off. Even under full_auto the exit prompts
    /// — a plan gate full-auto could blow through is theatre. Every other
    /// exit keeps the posture and returns typed feedback.
    fn execute_exit_plan(&self, call: &ToolCall) -> ToolOutcome {
        if !self.plan_active() {
            return Self::error("plan mode is not active");
        }
        let plan_file = self.plan_file();
        let plan_text = std::fs::read_to_string(&plan_file).unwrap_or_else(|_| {
            "(plan file is empty or missing — write the plan before exiting)".to_string()
        });
        let capped = cap_plan_text(&plan_text);
        let ask_call = ToolCall {
            id: call.id.clone(),
            name: "exit_plan_mode".into(),
            arguments: serde_json::json!({
                "question": format!("Approve this plan and exit plan mode?\n\n{capped}"),
                "header": "Plan approval",
                "options": [
                    { "label": PLAN_EXIT_APPROVE_LABEL },
                    { "label": PLAN_EXIT_REVISE_LABEL }
                ],
                "plan_file": plan_file.display().to_string()
            }),
        };
        match self.gate.ask(&ask_call) {
            AskOutcome::Answered(label) if label == PLAN_EXIT_APPROVE_LABEL => {
                if let Err(err) = set_plan_posture(
                    &self.posture,
                    &self.coordinator,
                    self.next_op_id("plan"),
                    false,
                ) {
                    return Self::error(format!(
                        "plan approved but the posture transition failed: {err}"
                    ));
                }
                Self::plain("plan approved; plan mode is off — proceed with the plan")
            }
            AskOutcome::Answered(label) => Self::error(format!(
                "plan not approved ({label}); plan posture stays active — revise {} and try again",
                plan_file.display()
            )),
            AskOutcome::Denied(reason) => Self::error(format!(
                "plan exit not approved ({reason}); plan posture stays active"
            )),
            AskOutcome::Unavailable => Self::error(
                "this host cannot answer questions, so the plan cannot be approved; plan posture stays active",
            ),
        }
    }

    /// `ask_user` (C10 §5): validate the question shape, then route through
    /// the gate's ask channel. Every failure is a typed error, never an
    /// empty result (wcore lesson #504).
    fn execute_ask_user(&self, call: &ToolCall) -> ToolOutcome {
        if let Err(err) = validate_question_args(&call.arguments) {
            return Self::error(format!("ask_user rejected: {err}"));
        }
        match self.gate.ask(call) {
            AskOutcome::Answered(label) => Self::plain(label),
            AskOutcome::Denied(reason) => {
                Self::error(format!("ask_user: {reason}; proceed without asking"))
            }
            AskOutcome::Unavailable => Self::error(
                "ask_user unavailable: this host cannot answer mid-turn questions; proceed without asking",
            ),
        }
    }
}

/// Validate `{question, options: [{label, ...}]}`: a non-empty question and
/// 2-4 options with non-empty labels (v1 is single-question, single-select,
/// no freeform). Used by the session executor AND the gate's ask.
pub fn validate_question_args(arguments: &serde_json::Value) -> Result<(), String> {
    let question = arguments
        .get("question")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "question must be a non-empty string".to_string())?;
    let _ = question;
    let options = arguments
        .get("options")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "options must be an array of {label, description?}".to_string())?;
    if !(2..=4).contains(&options.len()) {
        return Err(format!("options must number 2-4, got {}", options.len()));
    }
    for (index, option) in options.iter().enumerate() {
        let label = option
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        if label.is_none() {
            return Err(format!("options[{index}].label must be a non-empty string"));
        }
    }
    Ok(())
}

/// The option labels of a validated question call, in order (the gate mints
/// `opt_{i}` wire ids from these).
pub fn question_labels(arguments: &serde_json::Value) -> Vec<String> {
    arguments
        .get("options")
        .and_then(|v| v.as_array())
        .map(|options| {
            options
                .iter()
                .filter_map(|o| o.get("label").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl ToolExecutor for SessionTools<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        match call.name.as_str() {
            "todo" => self.execute_todo(call),
            "enter_plan_mode" => self.execute_enter_plan(),
            "exit_plan_mode" => self.execute_exit_plan(call),
            "ask_user" => self.execute_ask_user(call),
            _ => self.inner.execute(call).await,
        }
    }

    /// P1: thread the turn's cancel flag through to the inner executor
    /// (web_search's in-flight cancellation); the session-owned arms are
    /// serviced here and complete at the loop's boundary checks.
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> ToolOutcome {
        match call.name.as_str() {
            "todo" | "enter_plan_mode" | "exit_plan_mode" | "ask_user" => self.execute(call).await,
            _ => self.inner.execute_cancellable(call, cancel).await,
        }
    }

    fn take_image_result(&self, call_id: &str) -> Option<nano_agent::turn::LiveImageToolResult> {
        self.inner.take_image_result(call_id)
    }

    fn image_results_backed(&self) -> bool {
        self.inner.image_results_backed()
    }
}

/// The session-owned PTY executor (P4 §4.3/§4.4): routes the five `pty_*`
/// tools to the session's [`PtySessionManager`] and defers everything else
/// (the TaskToolExecutor wrapper pattern). Approval is the GATE's job —
/// `pty_spawn` always prompts there; the four follow-ups are not re-gated
/// (the spawn was the gated action). Ownership is by construction: the
/// manager is session-scoped, so a foreign/child session id is simply
/// unknown here (typed `PtySessionGone`).
pub struct PtyToolExecutor<'a> {
    inner: &'a dyn ToolExecutor,
    pty: Arc<nano_tools::pty::PtySessionManager>,
}

impl std::fmt::Debug for PtyToolExecutor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyToolExecutor").finish_non_exhaustive()
    }
}

/// P4 §8: the tool-result-boundary kind mapping. `PtySessionGone` gets its
/// dedicated kind; spawn/sandbox unavailability and invalid params ride the
/// existing kinds. Capacity is deliberately UNTYPED — the §4.4 capacity
/// refusal follows the `TaskError::FanOutCap` precedent (a bounded
/// tool-level message, not a table entry).
fn kind_of_pty(err: &nano_tools::pty::PtyError) -> Option<nano_session::NanoErrorKind> {
    use nano_session::NanoErrorKind;
    match err {
        nano_tools::pty::PtyError::PtySessionGone { .. } => Some(NanoErrorKind::PtySessionGone),
        nano_tools::pty::PtyError::SandboxUnavailable(_) | nano_tools::pty::PtyError::Spawn(_) => {
            Some(NanoErrorKind::SandboxUnavailable)
        }
        nano_tools::pty::PtyError::InvalidParams(_) => Some(NanoErrorKind::InvalidParams),
        nano_tools::pty::PtyError::Capacity => None,
    }
}

impl<'a> PtyToolExecutor<'a> {
    pub fn new(inner: &'a dyn ToolExecutor, pty: Arc<nano_tools::pty::PtySessionManager>) -> Self {
        Self { inner, pty }
    }

    fn parse<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, ToolOutcome> {
        serde_json::from_value(call.arguments.clone()).map_err(|err| ToolOutcome {
            ok: false,
            output: format!("bad pty arguments: {err}"),
            progress: ProgressSignals::default(),
            error_kind: Some(nano_session::NanoErrorKind::MissingArgs),
        })
    }

    fn outcome(result: Result<impl serde::Serialize, nano_tools::pty::PtyError>) -> ToolOutcome {
        match result {
            Ok(response) => ToolOutcome {
                ok: true,
                output: serde_json::to_string(&response)
                    .unwrap_or_else(|_| "pty response serialization failed".into()),
                progress: ProgressSignals::default(),
                error_kind: None,
            },
            Err(err) => ToolOutcome {
                ok: false,
                output: err.to_string(),
                progress: ProgressSignals::default(),
                error_kind: kind_of_pty(&err),
            },
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for PtyToolExecutor<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        use nano_tools::pty::{PtyKillRequest, PtyReadRequest, PtySpawnRequest, PtyWriteRequest};
        match call.name.as_str() {
            "pty_spawn" => match Self::parse::<PtySpawnRequest>(call) {
                Ok(request) => Self::outcome(self.pty.spawn(request)),
                Err(outcome) => outcome,
            },
            "pty_write" => match Self::parse::<PtyWriteRequest>(call) {
                Ok(request) => Self::outcome(self.pty.write(request)),
                Err(outcome) => outcome,
            },
            "pty_read" => match Self::parse::<PtyReadRequest>(call) {
                Ok(request) => Self::outcome(self.pty.read(request)),
                Err(outcome) => outcome,
            },
            "pty_kill" => match Self::parse::<PtyKillRequest>(call) {
                Ok(request) => Self::outcome(self.pty.kill(request)),
                Err(outcome) => outcome,
            },
            "pty_list" => Self::outcome(Ok::<_, nano_tools::pty::PtyError>(self.pty.list())),
            _ => self.inner.execute(call).await,
        }
    }

    /// The pty arms are synchronous manager operations (pty_read's wait is
    /// the bounded yield, not a cancellable stream); everything else
    /// threads the turn's cancel flag through (the web_search discipline).
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> ToolOutcome {
        match call.name.as_str() {
            name if nano_agent::wiring::PTY_TOOL_NAMES.contains(&name) => self.execute(call).await,
            _ => self.inner.execute_cancellable(call, cancel).await,
        }
    }

    fn take_image_result(&self, call_id: &str) -> Option<nano_agent::turn::LiveImageToolResult> {
        self.inner.take_image_result(call_id)
    }

    fn image_results_backed(&self) -> bool {
        self.inner.image_results_backed()
    }
}

/// The protocol-host approval gate (C10 §3): the historical host gate is
/// ApproveAll (the protocol host is a trust-all dev surface with no C2
/// modes), but the plan posture is NOT theatre — while it is active,
/// fs_write/fs_edit to anything but the plan file deny at the gate, exactly
/// like the ACP gate's posture arm. Questions are `Unavailable` (the
/// protocol has no question channel in v1), so ask_user and plan exit fail
/// closed with the typed unavailability error.
#[derive(Debug)]
pub struct PlanAwareApproval {
    posture: Arc<Mutex<PlanPosture>>,
    /// The session workspace (relative call paths resolve against it,
    /// exactly as the executor's resolve() does).
    workspace: PathBuf,
}

impl PlanAwareApproval {
    pub fn new(posture: Arc<Mutex<PlanPosture>>, workspace: &Path) -> Self {
        Self {
            posture,
            workspace: workspace.to_path_buf(),
        }
    }
}

/// Resolve a call's `path` argument the way the executor's resolve() does
/// (absolute kept, relative joined to the workspace) — the gate validates
/// the exact spelling the mutation consumes.
pub fn resolve_call_path(call: &ToolCall, workspace: &Path) -> Option<PathBuf> {
    let raw = call.arguments.get("path")?.as_str()?;
    let path = Path::new(raw);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    })
}

/// Under the plan posture, is this call permitted? Shared by both gates:
/// fs_write/fs_edit pass ONLY for the plan file; every other tool name is
/// unaffected by the posture (shell stays governed by the underlying mode).
pub fn posture_allows(
    posture: &Arc<Mutex<PlanPosture>>,
    call: &ToolCall,
    workspace: &Path,
) -> Option<bool> {
    let guard = posture.lock().unwrap_or_else(|p| p.into_inner());
    if !guard.active {
        return None;
    }
    match call.name.as_str() {
        "fs_write" | "fs_edit" => Some(match resolve_call_path(call, workspace) {
            Some(resolved) => guard.is_plan_file(&resolved),
            None => false, // unparseable path: deny under the posture
        }),
        _ => None,
    }
}

impl nano_agent::turn::ApprovalGate for PlanAwareApproval {
    fn approve(&self, call: &ToolCall) -> nano_agent::turn::ApprovalDecision {
        match posture_allows(&self.posture, call, &self.workspace) {
            Some(true) => nano_agent::turn::ApprovalDecision::Approve,
            Some(false) => nano_agent::turn::ApprovalDecision::Deny,
            None => nano_agent::turn::ApprovalDecision::Approve,
        }
    }

    fn denial_reason(&self) -> Option<&'static str> {
        if self
            .posture
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
        {
            Some("plan mode is active: writes are restricted to the session's plan file")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn posture_fixture() -> (tempfile::TempDir, Arc<Mutex<PlanPosture>>) {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let posture = PlanPosture::new(&sessions, "test-session").unwrap();
        (tmp, Arc::new(Mutex::new(posture)))
    }

    #[test]
    fn plan_file_containment_is_creation_safe() {
        let (tmp, posture) = posture_fixture();
        let posture = posture.lock().unwrap();
        let sessions = tmp.path().join("sessions");
        // The nonexistent leaf passes (creation-safe: name + canonical parent).
        let target = sessions.join("test-session.plan.md");
        assert!(!target.exists());
        assert!(posture.is_plan_file(&target));
        // Wrong leaf name in the right dir: no.
        assert!(!posture.is_plan_file(&sessions.join("other.md")));
        // Right leaf name in the WRONG dir: no.
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert!(!posture.is_plan_file(&elsewhere.join("test-session.plan.md")));
        // Unresolvable parent: fail closed.
        assert!(!posture.is_plan_file(&tmp.path().join("nope").join("test-session.plan.md")));
    }

    #[test]
    fn symlinked_parent_that_escapes_fails_closed() {
        let (tmp, posture) = posture_fixture();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let link = tmp.path().join("sessions-link");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();
        #[cfg(windows)]
        let linked = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .expect("spawn mklink")
            .status
            .success();
        if !linked {
            panic!("directory link creation refused on this host");
        }
        // The link's canonical parent resolves OUTSIDE the sessions dir.
        let posture = posture.lock().unwrap();
        assert!(!posture.is_plan_file(&link.join("test-session.plan.md")));
    }

    #[test]
    fn posture_transition_is_journal_first() {
        let (tmp, posture) = posture_fixture();
        let journal = tmp.path().join("sessions").join("test-session.jsonl");
        // Open failure (journal parent is a regular FILE, so the writer's
        // create_dir_all fails) is typed at coordinator construction — a
        // session that cannot journal never starts (fail closed).
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "not a dir").unwrap();
        let bad = blocker.join("x.jsonl");
        assert!(nano_session::JournalCoordinator::open(&bad).is_err());
        assert!(!posture.lock().unwrap().active);
        // Successful append flips the cell and lands the op.
        let coordinator = nano_session::JournalCoordinator::open(&journal).unwrap();
        set_plan_posture(&posture, &coordinator, "op-2".into(), true).unwrap();
        assert!(posture.lock().unwrap().active);
        let report = nano_session::reader::read_journal(&journal).unwrap();
        assert!(
            report
                .envelopes
                .iter()
                .any(|e| matches!(e.op, Op::PlanSet { active: true }))
        );
        // And replay does NOT restore the posture (postures don't replay).
        let folded = nano_session::SessionState::fold(&report.envelopes);
        let _ = folded; // SessionState has no posture field at all — by design.
    }

    #[test]
    fn todo_restore_block_bounds_and_delimits() {
        let items: Vec<TodoItem> = (0..60)
            .map(|i| TodoItem {
                id: format!("t{i}"),
                content: format!("task {i}"),
                status: TodoStatus::Pending,
            })
            .collect();
        let block = todo_restore_block(&items).unwrap();
        assert!(block.starts_with("[Restored session todo list"));
        assert!(block.ends_with("[End of restored todo list]"));
        assert!(block.contains("60 items, showing 50"));
        assert!(block.chars().count() <= TODO_RESTORE_MAX_CHARS + 200);
        assert!(todo_restore_block(&[]).is_none());
    }

    /// C10 §2: replace/read round-trip through the wrapper, journal-first
    /// ordering (append failure ⇒ the visible list is unchanged), and a
    /// typed validation error on bad status vocabulary.
    #[test]
    fn todo_replace_read_round_trip_journal_first() {
        use nano_agent::turn::{ApprovalDecision, ApprovalGate, ToolExecutor};

        #[derive(Debug)]
        struct NoopGate;
        impl ApprovalGate for NoopGate {
            fn approve(&self, _call: &nano_model::types::ToolCall) -> ApprovalDecision {
                ApprovalDecision::Approve
            }
        }
        #[derive(Debug)]
        struct NoopExec;
        #[async_trait::async_trait]
        impl ToolExecutor for NoopExec {
            async fn execute(&self, _call: &nano_model::types::ToolCall) -> ToolOutcome {
                unreachable!("session tools never delegate session names")
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tmp, posture) = posture_fixture();
        let journal = tmp.path().join("sessions").join("test-session.jsonl");
        let todos = Arc::new(Mutex::new(Vec::new()));
        let gate = NoopGate;
        let inner = NoopExec;
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal).unwrap());
        let tools = SessionTools::new(
            &inner,
            &gate,
            todos.clone(),
            posture,
            coordinator,
            "test-session".into(),
        );
        let call = |args: serde_json::Value| nano_model::types::ToolCall {
            id: "c1".into(),
            name: "todo".into(),
            arguments: args,
        };

        // Read of the empty list.
        let outcome = rt.block_on(tools.execute(&call(serde_json::json!({}))));
        assert!(outcome.ok);
        assert_eq!(outcome.output, "todo list is empty");

        // Replace: journaled, then visible.
        let outcome = rt.block_on(tools.execute(&call(serde_json::json!({
            "todos": [{"id": "t1", "content": "write tests", "status": "in_progress"}]
        }))));
        assert!(outcome.ok, "{}", outcome.output);
        assert!(outcome.output.contains("1 item(s)"));
        assert!(outcome.output.contains("1 in_progress"));
        assert_eq!(todos.lock().unwrap().len(), 1);
        let report = nano_session::reader::read_journal(&journal).unwrap();
        assert!(
            report
                .envelopes
                .iter()
                .any(|e| matches!(e.op, Op::TodoSet { ref items } if items.len() == 1)),
            "TodoSet journaled"
        );

        // Bad status vocabulary: typed error, nothing journaled.
        let outcome = rt.block_on(tools.execute(&call(serde_json::json!({
            "todos": [{"id": "t2", "content": "x", "status": "done"}]
        }))));
        assert!(!outcome.ok);
        assert!(outcome.output.contains("unknown status"));
        assert_eq!(
            todos.lock().unwrap().len(),
            1,
            "rejected write mutates nothing"
        );

        // P3 §3.3: appends route through the session's single
        // JournalCoordinator, opened at session construction — so the
        // fail-closed boundary moves to OPEN time: a journal whose parent
        // is unwritable fails construction typed, before any tool runs.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "file").unwrap();
        assert!(nano_session::JournalCoordinator::open(blocker.join("x.jsonl")).is_err());

        // MID-FLIGHT append failure (P3 review): the journal file is
        // replaced by a DIRECTORY after the coordinator opened it — the
        // coordinator's torn-path guard fails the append typed and the
        // visible list stays unchanged (journal-first).
        std::fs::remove_file(&journal).unwrap();
        std::fs::create_dir(&journal).unwrap();
        let outcome = rt.block_on(tools.execute(&call(serde_json::json!({
            "todos": [{"id": "t3", "content": "y", "status": "pending"}]
        }))));
        assert!(!outcome.ok);
        assert!(
            outcome.output.contains("list unchanged"),
            "{}",
            outcome.output
        );
        assert_eq!(
            todos.lock().unwrap().len(),
            1,
            "journal-first: mid-flight append failure leaves the list visibly unchanged"
        );
        std::fs::remove_dir(&journal).unwrap();
    }

    /// The protocol-host gate: trust-all at baseline, but the plan posture
    /// is NOT theatre — under it, workspace writes deny and the plan file
    /// passes.
    #[test]
    fn plan_aware_approval_enforces_the_posture() {
        use nano_agent::turn::{ApprovalDecision, ApprovalGate};
        let (tmp, posture) = posture_fixture();
        let gate = PlanAwareApproval::new(posture.clone(), tmp.path());
        let write = |path: &str| nano_model::types::ToolCall {
            id: "c".into(),
            name: "fs_write".into(),
            arguments: serde_json::json!({"path": path, "content": "x"}),
        };
        // Baseline: everything approves (protocol-host parity with the
        // historical ApproveAll).
        assert_eq!(
            gate.approve(&write("src/main.rs")),
            ApprovalDecision::Approve
        );
        // Posture on: workspace write denies, plan file approves, shell is
        // unaffected (governed by the underlying host policy, not the
        // posture).
        posture.lock().unwrap().active = true;
        assert_eq!(gate.approve(&write("src/main.rs")), ApprovalDecision::Deny);
        assert!(gate.denial_reason().is_some());
        let plan = posture.lock().unwrap().plan_file().to_path_buf();
        assert_eq!(
            gate.approve(&write(plan.to_str().unwrap())),
            ApprovalDecision::Approve
        );
        let shell = nano_model::types::ToolCall {
            id: "s".into(),
            name: "shell".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        assert_eq!(gate.approve(&shell), ApprovalDecision::Approve);
    }

    #[test]
    fn question_args_validation() {
        let good =
            serde_json::json!({"question": "q", "options": [{"label": "a"}, {"label": "b"}]});
        assert!(validate_question_args(&good).is_ok());
        assert_eq!(question_labels(&good), vec!["a", "b"]);
        for bad in [
            serde_json::json!({"options": [{"label": "a"}, {"label": "b"}]}),
            serde_json::json!({"question": "", "options": [{"label": "a"}, {"label": "b"}]}),
            serde_json::json!({"question": "q", "options": [{"label": "a"}]}),
            serde_json::json!({"question": "q", "options": [{"label": "a"}, {"label": "b"}, {"label": "c"}, {"label": "d"}, {"label": "e"}]}),
            serde_json::json!({"question": "q", "options": [{"label": "a"}, {"name": "b"}]}),
        ] {
            assert!(validate_question_args(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cap_plan_text_is_head_plus_tail() {
        let big = format!("{}{}", "h".repeat(9_000), "t".repeat(9_000));
        let capped = cap_plan_text(&big);
        assert!(capped.starts_with(&"h".repeat(100)));
        assert!(capped.ends_with(&"t".repeat(100)));
        assert!(capped.contains("…[elided 2000 chars]…"));
        let small = "small plan";
        assert_eq!(cap_plan_text(small), small);
    }

    /// P4 §4.4/§8: the session-PTY executor routes the five names to the
    /// session's manager with the design's kind mapping; non-pty names
    /// delegate. No real PTY is spawned here — the platform spawn legs live
    /// in nano-tools' pty.rs tests.
    #[test]
    fn pty_executor_dispatch_and_kind_mapping() {
        use nano_agent::turn::{ToolExecutor, ToolOutcome};

        #[derive(Debug)]
        struct RecordingExec;
        #[async_trait::async_trait]
        impl ToolExecutor for RecordingExec {
            async fn execute(&self, _call: &nano_model::types::ToolCall) -> ToolOutcome {
                ToolOutcome {
                    ok: true,
                    output: "delegated".into(),
                    progress: ProgressSignals::default(),
                    error_kind: None,
                }
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let manager = Arc::new(nano_tools::pty::PtySessionManager::new(workspace.path()));
        let inner = RecordingExec;
        let executor = PtyToolExecutor::new(&inner, manager);
        let call = |name: &str, args: serde_json::Value| nano_model::types::ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
        };

        // Unknown session: typed PtySessionGone (never retryable).
        let outcome = rt.block_on(executor.execute(&call(
            "pty_write",
            serde_json::json!({"session_id": "pty_9", "chars": "x"}),
        )));
        assert!(!outcome.ok);
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::PtySessionGone)
        );
        let outcome = rt.block_on(executor.execute(&call(
            "pty_read",
            serde_json::json!({"session_id": "pty_9"}),
        )));
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::PtySessionGone)
        );
        let outcome = rt.block_on(executor.execute(&call(
            "pty_kill",
            serde_json::json!({"session_id": "pty_9"}),
        )));
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::PtySessionGone)
        );

        // Malformed arguments: MissingArgs; schema-valid but bounded-out
        // values: InvalidParams.
        let outcome = rt.block_on(executor.execute(&call("pty_spawn", serde_json::json!({}))));
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::MissingArgs)
        );
        let outcome = rt.block_on(executor.execute(&call(
            "pty_read",
            serde_json::json!({"session_id": "pty_9", "max_bytes": 0}),
        )));
        assert_eq!(
            outcome.error_kind,
            Some(nano_session::NanoErrorKind::InvalidParams)
        );

        // pty_list on the empty manager succeeds with a JSON payload.
        let outcome = rt.block_on(executor.execute(&call("pty_list", serde_json::json!({}))));
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.output, "[]");

        // Other names delegate untouched.
        let outcome =
            rt.block_on(executor.execute(&call("fs_read", serde_json::json!({"path": "a"}))));
        assert!(outcome.ok);
        assert_eq!(outcome.output, "delegated");

        // The §4.4 capacity refusal is a bounded tool-level error with NO
        // table kind (the TaskError::FanOutCap precedent).
        assert_eq!(kind_of_pty(&nano_tools::pty::PtyError::Capacity), None);
    }
}
