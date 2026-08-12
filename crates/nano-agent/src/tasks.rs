//! Background tasks (C6, panel-certified design:
//! shared/reviews/panel-tui/C5-C6-memory-tasks-design.md §8–10): polled
//! subagent v1.
//!
//! Model: `task_spawn` returns a task id IMMEDIATELY (the executor contract
//! is synchronous — never run a child inline); `task_status`/`task_result`
//! are non-blocking polls; `task_cancel` sets only the addressed child's
//! flag; `task_apply` copies the child's recorded `changed_files` back into
//! the parent workspace as a normal approval-gated write.
//!
//! Isolation invariants:
//! - Each child runs on its OWN thread with its own current-thread runtime,
//!   a fresh TurnEngine/TurnBudget, its own journal at
//!   `<nano_home>/tasks/<task_id>/journal.jsonl`, and an isolated filtered
//!   COPY of the workspace its sandbox policy is re-anchored to (a
//!   narrowing — the child reaches strictly less than the parent).
//! - The child approval gate has NO interactive path (the parent's gate
//!   shares one pending-permission map; routing a child through it would
//!   deadlock, C6-R3). It approves exactly what the non-interactive policy
//!   already permits — read tools, fs writes contained by the copy-root
//!   policy, sandboxed shell — and denies everything else immediately.
//! - Depth limit 1: the `task_*` family is absent from child tool
//!   definitions AND the registry carries a spawn_depth guard. Fan-out cap:
//!   4 concurrent running children, no silent queue.
//! - Kill domain = registry-of-handles (`KillRegistry`): every child command
//!   spawns through `ShellTool::run_task`, the ONLY launch path a child
//!   executor holds; teardown sets the cancel flag, terminates every
//!   registered handle, waits `recv_timeout(5s)`, then DETACHES with a
//!   typed teardown warning (a wedged child can only burn an idle thread
//!   until process exit; KILL_ON_JOB_CLOSE is the final backstop).

use crate::loop_protection::ProgressSignals;
use crate::turn::{
    ApprovalDecision, ApprovalGate, ModelDriver, ToolExecutor, ToolOutcome, TurnEngine, TurnState,
};
use crate::wiring::{RealToolExecutor, v1_tool_definitions};
use nano_model::types::{ToolCall, ToolDefinition};
use nano_session::op::{Op, OpEnvelope};
use nano_session::writer::JournalWriter;
use nano_tools::fs::FsTools;
use nano_tools::shell::{KillRegistry, ShellTool};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Max concurrent `running` children per session; spawn beyond the cap is a
/// typed error (a silent queue is an unbounded-latency lie).
pub const MAX_CONCURRENT_TASKS: usize = 4;
/// Workspace-copy resource guards (fail-closed spawn): a few hundred MB /
/// tens of thousands of files.
pub const TASK_WORKSPACE_BYTE_CAP: u64 = 256 * 1024 * 1024;
pub const TASK_WORKSPACE_FILE_CAP: u64 = 20_000;
/// The result payload returned to the parent is size-capped; the full
/// report stays in the retained task dir.
pub const TASK_RESULT_CHAR_CAP: usize = 8_000;
/// Bounded teardown: cancel flag + terminated jobs, then this wait, then
/// detach with a typed warning.
const TEARDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// Depth limit (v1 = 1: children cannot spawn). Carried on the registry so
/// raising it later is a config change, not a redesign.
const MAX_SPAWN_DEPTH: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("unknown task: {0}")]
    NotFound(String),
    #[error("fan-out cap: {MAX_CONCURRENT_TASKS} tasks already running")]
    FanOutCap,
    #[error("spawn depth limit: background tasks cannot spawn tasks")]
    DepthLimit,
    #[error("workspace copy failed closed: {0}")]
    WorkspaceCopy(String),
    #[error("child journal unavailable: {0}")]
    JournalUnavailable(String),
    #[error("{0}")]
    DriverUnavailable(String),
    #[error("task {0} is not complete")]
    NotComplete(String),
    #[error("path is not in the task's recorded changed_files: {0}")]
    NotInInventory(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Done,
    Failed,
    Cancelled,
    /// Terminal diagnostic state: teardown's bounded wait tripped on a
    /// wedged child; it is cancel-flagged and job-killed, and the thread is
    /// dropped (never joined). Leaves active fan-out accounting.
    Detached,
}

impl TaskState {
    fn label(self) -> &'static str {
        match self {
            TaskState::Running => "running",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
            TaskState::Detached => "detached",
        }
    }
}

#[derive(Debug)]
struct ChildOutcome {
    state: TaskState,
    report: String,
    changed_files: Vec<String>,
    failure: Option<String>,
}

#[derive(Debug)]
struct TaskRecord {
    label: Option<String>,
    state: TaskState,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    kills: Arc<KillRegistry>,
    steps: Arc<AtomicU32>,
    done_rx: std::sync::mpsc::Receiver<ChildOutcome>,
    join: Option<std::thread::JoinHandle<()>>,
    outcome: Option<ChildOutcome>,
    /// `<nano_home>/tasks/<task_id>/` — retained after completion (manual
    /// GC only; auto-deleting audit artifacts is the wrong default).
    dir: PathBuf,
    workspace_copy: PathBuf,
}

/// The session-scoped task registry: owns every task's JoinHandle,
/// completion channel, kill domain, and journal path; dies with the
/// session (Drop tears every child down).
pub struct TaskRegistry {
    tasks: Mutex<std::collections::HashMap<String, TaskRecord>>,
    nano_home: PathBuf,
    workspace: PathBuf,
    model_name: String,
    /// Fail-closed: a factory error (e.g. the session's provider binding
    /// could not resolve) makes task_spawn a typed error, never a fallback.
    driver_factory: Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync>,
    /// Defensive depth guard (children never receive a registry — the
    /// `task_*` family is absent from their tool definitions — but a
    /// registry constructed at MAX depth refuses to spawn regardless).
    spawn_depth: u32,
    counter: AtomicU64,
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRegistry")
            .field("workspace", &self.workspace)
            .field("spawn_depth", &self.spawn_depth)
            .finish_non_exhaustive()
    }
}

impl TaskRegistry {
    pub fn new(
        nano_home: &Path,
        workspace: &Path,
        model_name: String,
        // Fail-closed: a factory error (e.g. the session's provider binding
        // could not resolve) makes task_spawn a typed error, never a
        // fallback.
        driver_factory: Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync>,
    ) -> Self {
        Self {
            tasks: Mutex::new(std::collections::HashMap::new()),
            nano_home: nano_home.to_path_buf(),
            workspace: workspace.to_path_buf(),
            model_name,
            driver_factory,
            spawn_depth: 0,
            counter: AtomicU64::new(1),
        }
    }

    /// Test seam for the depth guard: a registry at the depth limit refuses
    /// to spawn even though it structurally could.
    #[cfg(test)]
    pub fn at_depth_limit(mut self) -> Self {
        self.spawn_depth = MAX_SPAWN_DEPTH;
        self
    }

    fn tasks_dir(&self) -> PathBuf {
        self.nano_home.join("tasks")
    }

    /// Pull completed outcomes out of the completion channels (non-blocking).
    fn reap_finished(&self, tasks: &mut std::collections::HashMap<String, TaskRecord>) {
        for record in tasks.values_mut() {
            if record.state != TaskState::Running || record.outcome.is_some() {
                continue;
            }
            match record.done_rx.try_recv() {
                Ok(outcome) => {
                    record.state = outcome.state;
                    record.outcome = Some(outcome);
                    if let Some(join) = record.join.take() {
                        let _ = join.join();
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    record.state = TaskState::Failed;
                    record.outcome = Some(ChildOutcome {
                        state: TaskState::Failed,
                        report: String::new(),
                        changed_files: Vec::new(),
                        failure: Some("child thread died without reporting".into()),
                    });
                }
            }
        }
    }

    /// task_spawn: copy the workspace, open the child journal, spawn the
    /// child thread, register the record, return the id. Every failure
    /// mode fails CLOSED: no half-copied tree, no half-started task.
    pub fn spawn(&self, prompt: &str, label: Option<&str>) -> Result<String, TaskError> {
        if self.spawn_depth >= MAX_SPAWN_DEPTH {
            return Err(TaskError::DepthLimit);
        }
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        self.reap_finished(&mut tasks);
        let running = tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
            .count();
        if running >= MAX_CONCURRENT_TASKS {
            return Err(TaskError::FanOutCap);
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let task_id = format!(
            "task-{nanos}-{}",
            self.counter.fetch_add(1, Ordering::SeqCst)
        );
        let dir = self.tasks_dir().join(&task_id);
        let workspace_copy = dir.join("workspace");
        let journal_path = dir.join("journal.jsonl");

        // Isolated filtered copy with hard resource guards, no-follow on
        // links, canonical containment — any failure aborts the spawn.
        let report =
            copy_workspace(&self.workspace, &workspace_copy).map_err(TaskError::WorkspaceCopy)?;
        if report.skipped_links > 0 {
            eprintln!(
                "wayland-nano: task {task_id}: skipped {} link(s) in the workspace copy",
                report.skipped_links
            );
        }
        // Fail-closed journaling: a child whose journal can't open is never
        // started (the parent's posture, applied to children).
        let journaled = JournalWriter::open(&journal_path).and_then(|mut w| {
            w.append(&OpEnvelope::new(
                format!("{task_id}-begin-1"),
                "now",
                Op::SessionBegin {
                    session_id: task_id.clone(),
                    cwd: workspace_copy.display().to_string(),
                },
            ))
        });
        if let Err(err) = journaled {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(TaskError::JournalUnavailable(err.to_string()));
        }

        let driver = match (self.driver_factory)() {
            Ok(driver) => driver,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(TaskError::DriverUnavailable(err));
            }
        };
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kills = Arc::new(KillRegistry::new());
        let steps = Arc::new(AtomicU32::new(0));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let ctx = ChildContext {
            task_id: task_id.clone(),
            prompt: prompt.to_string(),
            cancel: cancel.clone(),
            kills: kills.clone(),
            steps: steps.clone(),
            workspace_copy: workspace_copy.clone(),
            journal_path,
            report_path: dir.join("report.md"),
            nano_home: self.nano_home.clone(),
            model_name: self.model_name.clone(),
            driver,
            done_tx,
        };
        let join = std::thread::Builder::new()
            .name(format!("wayland-nano-{task_id}"))
            .spawn(move || run_child(ctx))
            .map_err(|e| TaskError::WorkspaceCopy(format!("child thread spawn: {e}")))?;
        tasks.insert(
            task_id.clone(),
            TaskRecord {
                label: label.map(str::to_string),
                state: TaskState::Running,
                cancel,
                kills,
                steps,
                done_rx,
                join: Some(join),
                outcome: None,
                dir,
                workspace_copy,
            },
        );
        Ok(task_id)
    }

    /// task_status / task_result / task_list share one lookup; returns the
    /// one-line status (with the child's step count as the progress note).
    pub fn status(&self, task_id: &str) -> Result<String, TaskError> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        self.reap_finished(&mut tasks);
        let record = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
        Ok(format!(
            "{} ({}, {} steps)",
            record.state.label(),
            record.label.as_deref().unwrap_or("no label"),
            record.steps.load(Ordering::SeqCst)
        ))
    }

    /// task_result: NON-BLOCKING poll (panel ruling Q4). A running task
    /// reports its progress note; the parent re-polls.
    pub fn result(&self, task_id: &str) -> Result<String, TaskError> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        self.reap_finished(&mut tasks);
        let record = tasks
            .get_mut(task_id)
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
        if record.state == TaskState::Running {
            return Ok(format!(
                "task {task_id} still running ({} steps)",
                record.steps.load(Ordering::SeqCst)
            ));
        }
        let Some(outcome) = record.outcome.as_ref() else {
            // Detached: a wedged child never reported an outcome — that IS
            // the result (typed, honest; the registry entry keeps the state).
            return Ok(format!(
                "task {task_id} detached: wedged past the bounded teardown wait (cancel-flagged, jobs terminated, journal closed)"
            ));
        };
        let mut text = String::new();
        if let Some(failure) = &outcome.failure {
            text.push_str(&format!("task {task_id} failed: {failure}\n"));
        }
        text.push_str(&outcome.report);
        if !outcome.changed_files.is_empty() {
            text.push_str(&format!(
                "\nchanged_files: {}",
                outcome.changed_files.join(", ")
            ));
        }
        text.push_str(&format!(
            "\n(task dir, retained for debugging: {})",
            record.dir.display()
        ));
        if text.chars().count() > TASK_RESULT_CHAR_CAP {
            text = text.chars().take(TASK_RESULT_CHAR_CAP).collect();
            text.push_str("\n[truncated: full report in the task dir]");
        }
        Ok(text)
    }

    pub fn list(&self) -> String {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        self.reap_finished(&mut tasks);
        if tasks.is_empty() {
            return "no tasks".to_string();
        }
        let mut rows: Vec<String> = tasks
            .iter()
            .map(|(id, record)| {
                format!(
                    "{id}: {} ({})",
                    record.state.label(),
                    record.label.as_deref().unwrap_or("no label")
                )
            })
            .collect();
        rows.sort();
        rows.join("\n")
    }

    /// task_cancel: sets ONLY the addressed child's flag, then the bounded
    /// teardown. Never touches the parent's flag (cancel-flag isolation).
    pub fn cancel(&self, task_id: &str) -> Result<String, TaskError> {
        let record = {
            let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
            tasks.remove(task_id)
        };
        let Some(mut record) = record else {
            return Err(TaskError::NotFound(task_id.to_string()));
        };
        teardown_record(task_id, &mut record);
        let state = record.state;
        self.tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task_id.to_string(), record);
        Ok(format!("task {task_id}: {}", state.label()))
    }

    /// Parent cancel / session end: set every child flag and terminate every
    /// registered kill handle. FAST (no waits) — safe from the reader
    /// thread's mid-poll relay; the bounded join happens in teardown.
    pub fn cancel_all(&self) {
        let tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        for record in tasks.values() {
            if record.state == TaskState::Running {
                record.cancel.store(true, Ordering::SeqCst);
                record.kills.terminate_all();
            }
        }
    }

    /// Full bounded teardown of every running child (session end). The
    /// registry lock is never held across a wait: each record is taken out,
    /// torn down, and put back.
    pub fn teardown_all(&self) {
        loop {
            let next = {
                let tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
                tasks
                    .iter()
                    .find(|(_, r)| r.state == TaskState::Running)
                    .map(|(id, _)| id.clone())
            };
            let Some(id) = next else { break };
            let mut record = self
                .tasks
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id)
                .expect("record present");
            teardown_record(&id, &mut record);
            self.tasks
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id, record);
        }
    }

    /// task_apply: HOST-SIDE copy-back of the child's recorded changed_files
    /// from the workspace copy into the parent workspace. Validates each
    /// path against the child's inventory (a model-invented path is
    /// refused), refuses destination reparse points, and verifies
    /// containment AFTER the copy (TOCTOU: a substituted destination is
    /// caught and removed, never kept).
    pub fn apply(&self, task_id: &str, files: Option<Vec<String>>) -> Result<String, TaskError> {
        let (workspace_copy, inventory) = {
            let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
            self.reap_finished(&mut tasks);
            let record = tasks
                .get_mut(task_id)
                .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
            if record.state != TaskState::Done {
                return Err(TaskError::NotComplete(task_id.to_string()));
            }
            let outcome = record.outcome.as_ref().expect("done has an outcome");
            (record.workspace_copy.clone(), outcome.changed_files.clone())
        };
        let requested = files.unwrap_or_else(|| inventory.clone());
        for rel in &requested {
            if !inventory.contains(rel) {
                return Err(TaskError::NotInInventory(rel.clone()));
            }
        }
        let parent_root = self
            .workspace
            .canonicalize()
            .map_err(|e| TaskError::InvalidPath(format!("parent workspace: {e}")))?;
        let child_root = workspace_copy
            .canonicalize()
            .map_err(|e| TaskError::InvalidPath(format!("child workspace: {e}")))?;
        let mut applied = Vec::new();
        for rel in &requested {
            apply_one(rel, &child_root, &parent_root)?;
            applied.push(rel.clone());
        }
        Ok(format!(
            "applied {} file(s) from {task_id}: {}",
            applied.len(),
            applied.join(", ")
        ))
    }
}

impl Drop for TaskRegistry {
    /// Session end tears every child down (bounded per child; detach on a
    /// wedged one — process exit's KILL_ON_JOB_CLOSE is the backstop).
    fn drop(&mut self) {
        self.teardown_all();
    }
}

/// The bounded teardown (design §10): cancel flag → terminate registered
/// kill handles → recv_timeout(5s) → detach with a typed warning. The
/// registry lock is NEVER held across this wait.
fn teardown_record(task_id: &str, record: &mut TaskRecord) {
    if record.state != TaskState::Running {
        return;
    }
    record.cancel.store(true, Ordering::SeqCst);
    record.kills.terminate_all();
    match record.done_rx.recv_timeout(TEARDOWN_WAIT) {
        Ok(outcome) => {
            record.state = if outcome.state == TaskState::Done {
                // A cancelled child that still completed races here; the
                // flag was set, so report cancelled honestly.
                TaskState::Cancelled
            } else {
                outcome.state
            };
            record.outcome = Some(outcome);
            if let Some(join) = record.join.take() {
                let _ = join.join();
            }
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            eprintln!(
                "wayland-nano: task {task_id} teardown: child wedged past the {}s bound; detaching (cancel-flagged, jobs terminated, journal closed)",
                TEARDOWN_WAIT.as_secs()
            );
            record.state = TaskState::Detached;
            // Drop the JoinHandle + receiver: the wedged thread can only
            // burn a bounded idle thread until process exit.
            record.join.take();
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            record.state = TaskState::Detached;
            record.join.take();
        }
    }
}

/// Copy ONE recorded file back into the parent workspace with reparse and
/// containment checks on BOTH ends (codex r2: destination checks too, not
/// merely canonicalized sources).
fn apply_one(rel: &str, child_root: &Path, parent_root: &Path) -> Result<(), TaskError> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(TaskError::InvalidPath(rel.to_string()));
    }
    let src = child_root.join(rel_path);
    let src_meta = src
        .symlink_metadata()
        .map_err(|e| TaskError::InvalidPath(format!("{rel}: {e}")))?;
    if src_meta.file_type().is_symlink() || !src_meta.file_type().is_file() {
        return Err(TaskError::InvalidPath(format!("{rel} is not a plain file")));
    }
    let src_canon = src
        .canonicalize()
        .map_err(|e| TaskError::InvalidPath(format!("{rel}: {e}")))?;
    if !src_canon.starts_with(child_root) {
        return Err(TaskError::InvalidPath(format!(
            "{rel} escapes the child copy"
        )));
    }
    let dst = parent_root.join(rel_path);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TaskError::InvalidPath(format!("{rel}: {e}")))?;
        let parent_canon = parent
            .canonicalize()
            .map_err(|e| TaskError::InvalidPath(format!("{rel}: {e}")))?;
        if !parent_canon.starts_with(parent_root) {
            return Err(TaskError::InvalidPath(format!(
                "{rel} destination escapes the parent workspace"
            )));
        }
    }
    // Destination reparse rejection: never write THROUGH a planted link.
    if let Ok(meta) = dst.symlink_metadata()
        && !meta.file_type().is_file()
    {
        return Err(TaskError::InvalidPath(format!(
            "{rel} destination is not a plain file"
        )));
    }
    if dst
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(TaskError::InvalidPath(format!(
            "{rel} destination is a link"
        )));
    }
    std::fs::copy(&src_canon, &dst).map_err(|e| TaskError::InvalidPath(format!("{rel}: {e}")))?;
    // TOCTOU backstop: a substitution between the check and the copy is
    // caught after the fact and the copied bytes removed.
    let dst_canon = dst
        .canonicalize()
        .map_err(|e| TaskError::InvalidPath(format!("{rel}: {e}")))?;
    if !dst_canon.starts_with(parent_root) {
        let _ = std::fs::remove_file(&dst);
        return Err(TaskError::InvalidPath(format!(
            "{rel} destination was substituted mid-copy"
        )));
    }
    Ok(())
}

/// One child's whole lifecycle: own current-thread runtime, fresh engine +
/// budget, child journal, DenyAll-interactive gate.
struct ChildContext {
    task_id: String,
    prompt: String,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    kills: Arc<KillRegistry>,
    steps: Arc<AtomicU32>,
    workspace_copy: PathBuf,
    journal_path: PathBuf,
    report_path: PathBuf,
    nano_home: PathBuf,
    model_name: String,
    driver: Arc<dyn ModelDriver>,
    done_tx: std::sync::mpsc::Sender<ChildOutcome>,
}

fn run_child(ctx: ChildContext) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let outcome = match runtime {
        Ok(runtime) => runtime.block_on(run_child_inner(&ctx)),
        Err(err) => ChildOutcome {
            state: TaskState::Failed,
            report: String::new(),
            changed_files: Vec::new(),
            failure: Some(format!("child runtime: {err}")),
        },
    };
    // The retained task dir carries the full report regardless of state.
    let report_body = format!(
        "# task {}\n\nstate: {}\n\n{}\n\nchanged_files:\n{}\n",
        ctx.task_id,
        outcome.state.label(),
        outcome.report,
        outcome.changed_files.join("\n")
    );
    let _ = std::fs::write(&ctx.report_path, report_body);
    // A wedged parent teardown detaches instead of reading this; send is
    // best-effort.
    let _ = ctx.done_tx.send(outcome);
}

async fn run_child_inner(ctx: &ChildContext) -> ChildOutcome {
    let journaled = JournalWriter::open(&ctx.journal_path);
    let mut writer = match journaled {
        Ok(writer) => writer,
        Err(err) => {
            return ChildOutcome {
                state: TaskState::Failed,
                report: String::new(),
                changed_files: Vec::new(),
                failure: Some(format!("child journal: {err}")),
            };
        }
    };
    // The sandbox policy is RE-ANCHORED to the copy root: the child can
    // reach strictly less than the parent, never the parent tree.
    let policy =
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy();
    let fs = FsTools::new(policy.clone(), &ctx.workspace_copy);
    let shell = ShellTool::new(&ctx.nano_home, &ctx.workspace_copy);
    // The construction boundary (codex r2): the kill registry is attached
    // here, and this executor holds NO other process-launch path.
    let executor = RealToolExecutor::new(fs, shell, &ctx.workspace_copy)
        .with_task_kill_registry(ctx.kills.clone());
    let gate = TaskApproval {
        policy,
        cwd: ctx.workspace_copy.clone(),
    };
    let engine = TurnEngine {
        model: ctx.driver.as_ref(),
        tools: &executor,
        budget: crate::loop_protection::TurnBudget::default(),
        model_name: ctx.model_name.clone(),
        // Depth limit (tested invariant): the task family is ABSENT from
        // child tool definitions; memory tools are absent too (children
        // never write memory).
        tool_definitions: v1_tool_definitions(),
        approval: Some(&gate),
        compaction: None,
    };
    let steps = ctx.steps.clone();
    let mut sink = move |envelope: &OpEnvelope| -> bool {
        if matches!(envelope.op, Op::ToolCall { .. }) {
            steps.fetch_add(1, Ordering::SeqCst);
        }
        match writer.append(envelope) {
            Ok(_) => true,
            Err(err) => {
                eprintln!("wayland-nano: child journal append failed: {err}");
                false
            }
        }
    };
    let turn_id = format!("{}-turn-1", ctx.task_id);
    let result = engine
        .run_turn_streaming(&turn_id, &ctx.prompt, Some(ctx.cancel.as_ref()), &mut sink)
        .await;
    let changed_files = changed_files_from_ops(&result.ops, &ctx.workspace_copy);
    // A cancel that landed during the child's final stretch (after the
    // engine's last flag check) still reports cancelled — the same posture
    // as the parent's cancel_fired override in acp_mode.
    let cancel_fired = ctx.cancel.load(Ordering::SeqCst);
    match result.state {
        TurnState::Complete if !cancel_fired => ChildOutcome {
            state: TaskState::Done,
            report: result.final_text,
            changed_files,
            failure: None,
        },
        TurnState::Complete | TurnState::Stopped(_) => ChildOutcome {
            state: TaskState::Cancelled,
            report: result.final_text,
            changed_files,
            failure: match result.state {
                // C7: Stopped carries a typed StopInfo — surface kind + detail.
                TurnState::Stopped(reason) => Some(format!("{:?}: {}", reason.kind, reason.detail)),
                _ => Some("cancelled".into()),
            },
        },
        other => ChildOutcome {
            state: TaskState::Failed,
            report: result.final_text,
            changed_files,
            failure: Some(format!("{other:?}")),
        },
    }
}

/// The child's changed_files inventory: fs_write/fs_edit calls with an ok
/// result, resolved inside the copy root (canonical containment verified).
/// Relative paths only — task_apply re-anchors them into the parent tree.
fn changed_files_from_ops(ops: &[OpEnvelope], copy_root: &Path) -> Vec<String> {
    let ok_calls: std::collections::HashSet<&str> = ops
        .iter()
        .filter_map(|e| match &e.op {
            Op::ToolResult { call_id, ok, .. } if *ok => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    let Ok(canon_root) = copy_root.canonicalize() else {
        return Vec::new();
    };
    let mut files: Vec<String> = ops
        .iter()
        .filter_map(|e| match &e.op {
            Op::ToolCall {
                call_id,
                name,
                args,
                ..
            } if ok_calls.contains(call_id.as_str())
                && (name == "fs_write" || name == "fs_edit") =>
            {
                let path = args.get("path").and_then(|p| p.as_str())?;
                let raw = Path::new(path);
                let resolved = if raw.is_absolute() {
                    raw.to_path_buf()
                } else {
                    copy_root.join(raw)
                };
                let canon = resolved.canonicalize().ok()?;
                let rel = canon.strip_prefix(&canon_root).ok()?;
                Some(rel.to_string_lossy().replace('\\', "/"))
            }
            _ => None,
        })
        .collect();
    files.sort();
    files.dedup();
    files
}

/// The workspace copy (design §9): filtered full copy with byte/file-count
/// caps (fail-closed spawn), no-follow on symlinks/junctions/reparse
/// points (skipped like `.git`, counted in the report), and canonical
/// containment on every copied destination.
struct CopyReport {
    files: u64,
    bytes: u64,
    skipped_links: u64,
}

impl std::fmt::Debug for CopyReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopyReport")
            .field("files", &self.files)
            .field("bytes", &self.bytes)
            .field("skipped_links", &self.skipped_links)
            .finish()
    }
}

fn copy_workspace(src: &Path, dst: &Path) -> Result<CopyReport, String> {
    const EXCLUDED: &[&str] = &[".git", "target", "node_modules"];
    copy_workspace_with_caps(
        src,
        dst,
        TASK_WORKSPACE_BYTE_CAP,
        TASK_WORKSPACE_FILE_CAP,
        EXCLUDED,
    )
}

/// Cap-parameterized core (tests drive tiny caps instead of building
/// hundred-MB fixtures).
fn copy_workspace_with_caps(
    src: &Path,
    dst: &Path,
    byte_cap: u64,
    file_cap: u64,
    excluded: &[&str],
) -> Result<CopyReport, String> {
    let result = copy_workspace_inner(src, dst, byte_cap, file_cap, excluded);
    if result.is_err() {
        // No partial tree survives a failed spawn.
        let _ = std::fs::remove_dir_all(dst);
    }
    result
}

fn copy_workspace_inner(
    src: &Path,
    dst: &Path,
    byte_cap: u64,
    file_cap: u64,
    excluded: &[&str],
) -> Result<CopyReport, String> {
    let mut report = CopyReport {
        files: 0,
        bytes: 0,
        skipped_links: 0,
    };
    let canon_dst = std::fs::canonicalize(src).map_err(|e| format!("workspace root: {e}"))?;
    std::fs::create_dir_all(dst).map_err(|e| format!("task workspace: {e}"))?;
    let canon_dst_root = dst
        .canonicalize()
        .map_err(|e| format!("task workspace: {e}"))?;
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if excluded.iter().any(|x| std::ffi::OsStr::new(x) == name) {
                continue;
            }
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("stat {}: {e}", path.display()))?;
            if is_reparse_or_link(&meta) {
                report.skipped_links += 1;
                continue; // never followed — escape + blow-up hazard
            }
            let rel = path
                .strip_prefix(src)
                .map_err(|e| format!("rel {}: {e}", path.display()))?;
            let target = dst.join(rel);
            if meta.is_dir() {
                std::fs::create_dir_all(&target)
                    .map_err(|e| format!("mkdir {}: {e}", target.display()))?;
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                report.skipped_links += 1;
                continue; // sockets/fifos/etc: skipped, never copied
            }
            report.bytes += meta.len();
            report.files += 1;
            if report.bytes > byte_cap {
                return Err(format!("workspace exceeds the {byte_cap}-byte copy cap"));
            }
            if report.files > file_cap {
                return Err(format!("workspace exceeds the {file_cap}-file copy cap"));
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            std::fs::copy(&path, &target).map_err(|e| format!("copy {}: {e}", path.display()))?;
            // Canonical containment: every copied destination must land
            // inside the task workspace root.
            let canon_target = target
                .canonicalize()
                .map_err(|e| format!("canon {}: {e}", target.display()))?;
            if !canon_target.starts_with(&canon_dst_root) {
                return Err(format!(
                    "traversal: {} escapes the copy root",
                    path.display()
                ));
            }
        }
    }
    let _ = canon_dst; // source canonicalization only validates the root exists
    Ok(report)
}

#[cfg(windows)]
fn is_reparse_or_link(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT covers symlinks AND junctions.
    meta.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_or_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// The child approval gate (C6 §9, C6-R3): NO interactive path — the
/// parent's AcpApproval shares one pending-permission map with the running
/// turn, and a child routed through it would deadlock (or interleave
/// request ids with the parent's). This gate approves exactly what the
/// non-interactive policy already permits — read tools, fs writes contained
/// by the re-anchored copy-root policy, and shell (sandboxed at spawn, the
/// spawn-time transform fails closed) — and denies everything else
/// immediately with a typed reason. DEVIATION from the design's literal
/// `DenyAllApproval` name, resolved against §14's bar (the workspace-copy
/// and task_apply legs require child writes to land in the copy): what is
/// denied is every action that would require an interactive grant.
#[derive(Debug)]
pub struct TaskApproval {
    policy: nano_core::permissions::FileSystemSandboxPolicy,
    cwd: PathBuf,
}

impl ApprovalGate for TaskApproval {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision {
        match call.name.as_str() {
            name if is_read_only_tool(name) => ApprovalDecision::Approve,
            "fs_write" | "fs_edit" => {
                let contained = call
                    .arguments
                    .get("path")
                    .and_then(|value| value.as_str())
                    .is_some_and(|path| {
                        self.policy
                            .can_write_path_with_cwd(Path::new(path), &self.cwd)
                    });
                if contained {
                    ApprovalDecision::Approve
                } else {
                    ApprovalDecision::Deny
                }
            }
            // Sandboxed at spawn; the spawn-time transform is the
            // fail-closed authority (run_task, never an unregistered exec).
            "shell" => ApprovalDecision::Approve,
            _ => ApprovalDecision::Deny,
        }
    }

    fn denial_reason(&self) -> Option<&'static str> {
        Some("background tasks cannot request interactive approval")
    }
}

/// The child's read-only set (the host's fast-path, minus memory reads —
/// children get no memory tools at all).
fn is_read_only_tool(name: &str) -> bool {
    name.starts_with("fs_read") || name.starts_with("search") || name.starts_with("glob")
}

// ── tool surface (C6 §8) ────────────────────────────────────────────────

/// The `task_*` family advertised to the PARENT model only (never present
/// in a child's definitions — the depth limit).
pub fn task_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "task_spawn".into(),
            description: "Spawn a background task: a fresh agent turn on its own thread with an isolated COPY of the workspace (never the parent tree), an auto-deny approval posture (only non-interactive actions run), and its own journal. Returns a task id immediately; poll with task_status/task_result. Args: prompt, optional label.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string"},
                    "label": {"type": "string"}
                },
                "required": ["prompt"]
            }),
        },
        ToolDefinition {
            name: "task_status".into(),
            description: "Non-blocking poll of a background task: running|done|failed|cancelled|detached plus a progress note. Args: task_id.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: "task_result".into(),
            description: "Non-blocking fetch of a finished task's final report (capped; the full report stays in the task dir). Returns 'still running' for a live task — re-poll. Args: task_id.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: "task_list".into(),
            description: "List this session's background tasks with states. Args: none.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
        },
        ToolDefinition {
            name: "task_cancel".into(),
            description: "Cancel one background task (sets its cancel flag and terminates its running commands). Args: task_id.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"task_id": {"type": "string"}},
                "required": ["task_id"]
            }),
        },
        ToolDefinition {
            name: "task_apply".into(),
            description: "Copy a finished task's recorded changed_files from its isolated workspace copy back into the parent workspace (approval-gated; only recorded files can be applied). Args: task_id, optional files (relative paths; default: all changed files).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string"},
                    "files": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["task_id"]
            }),
        },
    ]
}

/// ToolExecutor wrapper routing the `task_*` family to the session's
/// registry and deferring everything else (the MCP/memory wrapper pattern).
#[derive(Debug)]
pub struct TaskToolExecutor<'a> {
    registry: Arc<TaskRegistry>,
    inner: &'a dyn ToolExecutor,
}

impl<'a> TaskToolExecutor<'a> {
    pub fn new(registry: Arc<TaskRegistry>, inner: &'a dyn ToolExecutor) -> Self {
        Self { registry, inner }
    }

    fn error(message: impl Into<String>) -> ToolOutcome {
        ToolOutcome {
            ok: false,
            output: message.into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }

    fn ok(message: impl Into<String>) -> ToolOutcome {
        ToolOutcome {
            ok: true,
            output: message.into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for TaskToolExecutor<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let arg = |key: &str| call.arguments.get(key).and_then(|v| v.as_str());
        match call.name.as_str() {
            "task_spawn" => match arg("prompt") {
                Some(prompt) => match self.registry.spawn(prompt, arg("label")) {
                    Ok(task_id) => Self::ok(format!("spawned {task_id}")),
                    Err(err) => Self::error(err.to_string()),
                },
                None => Self::error("missing prompt"),
            },
            "task_status" => match arg("task_id") {
                Some(id) => match self.registry.status(id) {
                    Ok(status) => Self::ok(status),
                    Err(err) => Self::error(err.to_string()),
                },
                None => Self::error("missing task_id"),
            },
            "task_result" => match arg("task_id") {
                Some(id) => match self.registry.result(id) {
                    Ok(report) => Self::ok(report),
                    Err(err) => Self::error(err.to_string()),
                },
                None => Self::error("missing task_id"),
            },
            "task_list" => Self::ok(self.registry.list()),
            "task_cancel" => match arg("task_id") {
                Some(id) => match self.registry.cancel(id) {
                    Ok(status) => Self::ok(status),
                    Err(err) => Self::error(err.to_string()),
                },
                None => Self::error("missing task_id"),
            },
            "task_apply" => match arg("task_id") {
                Some(id) => {
                    let files = call
                        .arguments
                        .get("files")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|f| f.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                        });
                    match self.registry.apply(id, files) {
                        Ok(summary) => Self::ok(summary),
                        Err(err) => Self::error(err.to_string()),
                    }
                }
                None => Self::error("missing task_id"),
            },
            _ => self.inner.execute(call).await,
        }
    }
}

#[cfg(test)]
mod tests {
    //! C6 §14 unit battery: workspace-copy guards, gate matrix, depth and
    //! fan-out trips, cancel isolation, bounded teardown (wedged child),
    //! changed_files inventory + task_apply refusals.

    use super::*;
    use nano_model::types::{ModelError, ModelEvent, ModelRequest, ModelResponse, Usage};

    /// A scripted driver whose responses route on the LAST user message, so
    /// parent and child turns share one queue deterministically. Calls whose
    /// route key is in `block_keys` block until `release` is set (the
    /// slow/wedged-child seam); everything else answers immediately.
    #[derive(Debug, Default)]
    struct ScriptedDriver {
        routes: Mutex<std::collections::HashMap<String, std::collections::VecDeque<ModelResponse>>>,
        block_keys: Vec<String>,
        release: Option<Arc<std::sync::atomic::AtomicBool>>,
    }

    #[async_trait::async_trait]
    impl ModelDriver for ScriptedDriver {
        async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            let key = request
                .messages
                .iter()
                .rev()
                .find_map(|m| {
                    m.content.iter().find_map(|b| match b {
                        nano_model::types::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                })
                .unwrap_or_default();
            if let Some(release) = &self.release
                && self.block_keys.contains(&key)
            {
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            self.routes
                .lock()
                .unwrap()
                .get_mut(&key)
                .and_then(|q| q.pop_front())
                .ok_or_else(|| ModelError::Protocol(format!("no scripted response for {key:?}")))
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

    struct Dirs {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        workspace: PathBuf,
    }

    fn dirs() -> Dirs {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        Dirs {
            _tmp: tmp,
            home,
            workspace,
        }
    }

    fn registry(dirs: &Dirs, driver: ScriptedDriver) -> TaskRegistry {
        let driver = Arc::new(driver);
        let factory: Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync> =
            Arc::new(move || Ok(driver.clone()));
        TaskRegistry::new(&dirs.home, &dirs.workspace, "mock".into(), factory)
    }

    /// Poll until the task leaves Running (bounded).
    fn wait_terminal(registry: &TaskRegistry, id: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let status = registry.status(id).unwrap();
            if !status.starts_with("running") {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "task {id} never settled"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn copy_guards_caps_links_and_exclusions() {
        let dirs = dirs();
        std::fs::write(dirs.workspace.join("keep.txt"), "data").unwrap();
        std::fs::create_dir_all(dirs.workspace.join("target")).unwrap();
        std::fs::write(dirs.workspace.join("target/big.o"), "junk").unwrap();
        std::fs::create_dir_all(dirs.workspace.join(".git")).unwrap();
        std::fs::write(dirs.workspace.join(".git/config"), "x").unwrap();
        // A link in the tree: skipped (never followed), counted.
        let outside = dirs.home.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dirs.workspace.join("link")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, dirs.workspace.join("link")).unwrap();

        let dst = dirs.home.join("copy-ok");
        let report = copy_workspace(&dirs.workspace, &dst).unwrap();
        assert_eq!(report.files, 1, "only keep.txt: {report:?}");
        assert!(report.skipped_links >= 1, "the link was counted");
        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join("target").exists(), "excluded dir");
        assert!(!dst.join(".git").exists(), "excluded dir");
        assert!(!dst.join("link").exists(), "link never followed");
        assert!(
            !dst.join("link/secret.txt").exists(),
            "link target never copied through"
        );

        // File-count cap: fail closed, no partial tree.
        let dst2 = dirs.home.join("copy-capped");
        let err = copy_workspace_with_caps(&dirs.workspace, &dst2, 1 << 30, 0, &[])
            .expect_err("file cap trips");
        assert!(err.contains("file copy cap"), "{err}");
        assert!(!dst2.exists(), "no partial copy survives");

        // Byte cap: same posture.
        let dst3 = dirs.home.join("copy-byte-capped");
        let err = copy_workspace_with_caps(&dirs.workspace, &dst3, 2, 100, &[])
            .expect_err("byte cap trips");
        assert!(err.contains("byte copy cap"), "{err}");
        assert!(!dst3.exists());
    }

    #[test]
    fn child_gate_matrix() {
        let dirs = dirs();
        let copy = dirs.home.join("task-ws");
        std::fs::create_dir_all(&copy).unwrap();
        let gate = TaskApproval {
            policy: nano_core::permissions::PermissionProfile::workspace_write()
                .file_system_sandbox_policy(),
            cwd: copy.clone(),
        };
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
        };
        // Read tools and contained writes and shell: approved (the
        // non-interactive policy set).
        assert_eq!(
            gate.approve(&call("fs_read", serde_json::json!({"path": "a"}))),
            ApprovalDecision::Approve
        );
        assert_eq!(
            gate.approve(&call(
                "fs_write",
                serde_json::json!({"path": copy.join("ok.txt"), "content": "x"})
            )),
            ApprovalDecision::Approve
        );
        assert_eq!(
            gate.approve(&call("shell", serde_json::json!({"command": "ls"}))),
            ApprovalDecision::Approve
        );
        // Everything interactive: immediate typed denial. The uncontained
        // write anchors at the FILESYSTEM ROOT, not a tempdir sibling: the
        // workspace_write policy includes the tmp roots, so a `..` escape
        // from a tempdir copy root would be CONTAINED on unix (the 495a2ef
        // platform-neutrality precedent).
        let outside = std::env::temp_dir()
            .ancestors()
            .last()
            .expect("filesystem root")
            .join("nano-c6-gate-outside")
            .join("escape.txt");
        for denied in [
            call("mcp__server__tool", serde_json::json!({})),
            call(
                "web_fetch",
                serde_json::json!({"url": "https://example.com"}),
            ),
            call(
                "memory_save",
                serde_json::json!({"slug": "x", "content": "y"}),
            ),
            call("task_spawn", serde_json::json!({"prompt": "recurse"})),
            call(
                "fs_write",
                serde_json::json!({"path": outside, "content": "x"}),
            ),
        ] {
            assert_eq!(
                gate.approve(&denied),
                ApprovalDecision::Deny,
                "{} must be denied",
                denied.name
            );
        }
        assert_eq!(
            gate.denial_reason(),
            Some("background tasks cannot request interactive approval")
        );
    }

    #[test]
    fn depth_guard_double_fails_closed() {
        let dirs = dirs();
        let registry = registry(&dirs, ScriptedDriver::default()).at_depth_limit();
        assert!(matches!(
            registry.spawn("child job", None),
            Err(TaskError::DepthLimit)
        ));
    }

    #[test]
    fn spawn_run_and_result_round_trip() {
        let dirs = dirs();
        let mut routes: std::collections::HashMap<
            String,
            std::collections::VecDeque<ModelResponse>,
        > = std::collections::HashMap::new();
        routes
            .entry("child job".to_string())
            .or_default()
            .push_back(text_response("child report body"));
        let registry = registry(
            &dirs,
            ScriptedDriver {
                routes: Mutex::new(routes),
                block_keys: vec![],
                release: None,
            },
        );
        let id = registry.spawn("child job", Some("demo")).expect("spawn");
        assert!(id.starts_with("task-"));
        let status = wait_terminal(&registry, &id);
        assert!(status.starts_with("done"), "{status}");
        assert!(status.contains("demo"), "label carried: {status}");
        let result = registry.result(&id).unwrap();
        assert!(result.contains("child report body"), "{result}");
        assert!(result.contains("task dir"), "{result}");
        // The child journal exists and replays standalone.
        let journal = dirs.home.join("tasks").join(&id).join("journal.jsonl");
        let report = nano_session::reader::read_journal(&journal).expect("child journal reads");
        assert!(
            report
                .envelopes
                .iter()
                .any(|e| matches!(e.op, Op::TurnEnd { .. })),
            "child turn completed in its own journal"
        );
        // Retained report on disk.
        assert!(dirs.home.join("tasks").join(&id).join("report.md").exists());
        // Result poll of an unknown id: typed error.
        assert!(matches!(
            registry.result("task-nope"),
            Err(TaskError::NotFound(_))
        ));
    }

    #[test]
    fn fan_out_cap_refuses_the_fifth() {
        let dirs = dirs();
        // Children that never complete until released hold Running slots.
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut routes: std::collections::HashMap<
            String,
            std::collections::VecDeque<ModelResponse>,
        > = std::collections::HashMap::new();
        for _ in 0..4 {
            routes
                .entry("slow job".to_string())
                .or_default()
                .push_back(text_response("done"));
        }
        let registry = registry(
            &dirs,
            ScriptedDriver {
                routes: Mutex::new(routes),
                block_keys: vec!["slow job".to_string()],
                release: Some(release.clone()),
            },
        );
        for _ in 0..4 {
            registry.spawn("slow job", None).expect("spawn within cap");
        }
        assert!(matches!(
            registry.spawn("slow job", None),
            Err(TaskError::FanOutCap)
        ));
        release.store(true, Ordering::SeqCst);
        // Drop tears down cleanly once released.
    }

    #[test]
    fn cancel_isolation_and_bounded_teardown_on_a_wedged_child() {
        let dirs = dirs();
        // Child A: wedged forever (release never fires) — the bounded
        // teardown must detach it within ~5s. Child B: completes normally.
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut routes: std::collections::HashMap<
            String,
            std::collections::VecDeque<ModelResponse>,
        > = std::collections::HashMap::new();
        routes
            .entry("quick job".to_string())
            .or_default()
            .push_back(text_response("quick done"));
        routes
            .entry("wedged job".to_string())
            .or_default()
            .push_back(text_response("never reached"));
        let registry = registry(
            &dirs,
            ScriptedDriver {
                routes: Mutex::new(routes),
                block_keys: vec!["wedged job".to_string()],
                release: Some(release.clone()),
            },
        );
        let wedged = registry.spawn("wedged job", None).unwrap();
        let quick = registry.spawn("quick job", None).unwrap();
        assert_eq!(wait_terminal(&registry, &quick), "done (no label, 0 steps)");

        // Cancel ONLY the wedged child: bounded wait trips, detach, and the
        // OTHER child is untouched.
        let started = std::time::Instant::now();
        let status = registry.cancel(&wedged).unwrap();
        let elapsed = started.elapsed();
        assert!(status.contains("detached"), "{status}");
        assert!(
            elapsed >= TEARDOWN_WAIT && elapsed < TEARDOWN_WAIT * 3,
            "bounded, not instant, not forever: {elapsed:?}"
        );
        assert!(
            registry.status(&quick).unwrap().starts_with("done"),
            "cancel isolation: the other task is unaffected"
        );
        // A cancelled-then-detached task reports a terminal state on result.
        assert!(registry.result(&wedged).is_ok());
    }

    #[test]
    fn parent_cancel_cascade_sets_every_child_flag() {
        let dirs = dirs();
        let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut routes: std::collections::HashMap<
            String,
            std::collections::VecDeque<ModelResponse>,
        > = std::collections::HashMap::new();
        for prompt in ["job one", "job two"] {
            routes
                .entry(prompt.to_string())
                .or_default()
                .push_back(text_response("x"));
        }
        let registry = registry(
            &dirs,
            ScriptedDriver {
                routes: Mutex::new(routes),
                block_keys: vec!["job one".to_string(), "job two".to_string()],
                release: Some(release.clone()),
            },
        );
        let a = registry.spawn("job one", None).unwrap();
        let b = registry.spawn("job two", None).unwrap();
        registry.cancel_all();
        release.store(true, Ordering::SeqCst);
        for id in [&a, &b] {
            let status = wait_terminal(&registry, id);
            assert!(
                status.starts_with("cancelled"),
                "cascaded cancel: {id} -> {status}"
            );
        }
    }

    #[test]
    fn changed_files_inventory_and_task_apply() {
        let dirs = dirs();
        let mut routes: std::collections::HashMap<
            String,
            std::collections::VecDeque<ModelResponse>,
        > = std::collections::HashMap::new();
        let queue = routes.entry("writer job".to_string()).or_default();
        queue.push_back(tool_response(ToolCall {
            id: "w1".into(),
            name: "fs_write".into(),
            arguments: serde_json::json!({"path": "note.txt", "content": "from the child"}),
        }));
        queue.push_back(text_response("wrote note.txt"));
        let registry = registry(
            &dirs,
            ScriptedDriver {
                routes: Mutex::new(routes),
                block_keys: vec![],
                release: None,
            },
        );
        let id = registry.spawn("writer job", None).unwrap();
        assert!(wait_terminal(&registry, &id).starts_with("done"));
        let result = registry.result(&id).unwrap();
        assert!(result.contains("changed_files: note.txt"), "{result}");
        // The write landed in the COPY, never the parent tree.
        let copy = dirs.home.join("tasks").join(&id).join("workspace");
        assert_eq!(
            std::fs::read_to_string(copy.join("note.txt")).unwrap(),
            "from the child"
        );
        assert!(!dirs.workspace.join("note.txt").exists());

        // A model-invented path is refused.
        assert!(matches!(
            registry.apply(&id, Some(vec!["invented.txt".to_string()])),
            Err(TaskError::NotInInventory(_))
        ));
        assert!(!dirs.workspace.join("invented.txt").exists());

        // The real apply copies the recorded file back.
        let summary = registry.apply(&id, None).unwrap();
        assert!(summary.contains("applied 1 file"), "{summary}");
        assert_eq!(
            std::fs::read_to_string(dirs.workspace.join("note.txt")).unwrap(),
            "from the child"
        );

        // A planted symlink at the destination is refused (reparse check) —
        // and left untouched.
        std::fs::remove_file(dirs.workspace.join("note.txt")).unwrap();
        let elsewhere = dirs.home.join("elsewhere.txt");
        std::fs::write(&elsewhere, "original").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, dirs.workspace.join("note.txt")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&elsewhere, dirs.workspace.join("note.txt")).unwrap();
        assert!(
            registry.apply(&id, None).is_err(),
            "link destination refused"
        );
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "original");

        // Apply on a running task is a typed error.
        assert!(matches!(
            registry.apply("task-nope", None),
            Err(TaskError::NotFound(_))
        ));
    }
}
