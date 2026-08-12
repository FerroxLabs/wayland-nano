//! `wayland-nano exec` (C11 §2) — headless one-shot execution: build or
//! resume a session, run turn(s) to completion, emit the frozen JSONL v1
//! event schema on stdout, exit with the pinned exit-code matrix.
//!
//! Contract (panel-frozen, Q2):
//! - stdout is ALWAYS JSONL (diagnostics go to stderr); every line carries
//!   `v: 1`, `session_id`, and a monotonic `seq` scoped to ONE process
//!   invocation (it restarts at 0 on every start, including `--resume` —
//!   consumers dedupe within one stream only);
//! - exactly seven event types: `session_started`, `text_delta`,
//!   `tool_call`, `tool_result`, `approval_denied`, `turn_completed`,
//!   `error`. Consumers MUST ignore unknown types/fields; producers NEVER
//!   extend v1 (a richer vocabulary is v2);
//! - approvals can never prompt: promptable actions auto-DENY with an
//!   `approval_denied` event naming the tool and the mode (fail-closed —
//!   there is no --yes/--dangerously-* flag, deliberately);
//! - exit codes: 0 completed · 1 model/runtime failure or TURN-level budget
//!   trip · 2 usage/journal/resume error · 3 goal terminal blocked
//!   (including GOAL-level budget trips) · 6 goal paused;
//! - `--output-last-message` writes the final assistant text ONLY on exit 0,
//!   atomically (tempfile + replace-existing rename); a failed run leaves a
//!   pre-existing file untouched and never writes a partial one.

use nano_agent::bootstrap::{BootstrappedSession, SessionSeed, latest_session_id};
use nano_agent::clock::SystemClock;
use nano_agent::goal::{
    GoalDriveOutcome, GoalTurnStop, is_control_tool, journal_goal_transition, validate_objective,
};
use nano_agent::loop_protection::TurnBudget;
use nano_agent::mcp::McpToolExecutor;
use nano_agent::turn::{ApprovalDecision, ApprovalGate, TurnEngine, TurnState};
use nano_agent::wiring::v1_tool_definitions;
use nano_model::types::{ToolCall, Usage};
use nano_protocol::permission_mode::PermissionMode;
use nano_session::op::{GoalBudgets, Op, OpEnvelope};
use nano_session::writer::JournalWriter;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

/// What to resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeTarget {
    Id(String),
    Last,
}

/// Goal spec for `exec --goal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecGoal {
    pub objective: String,
    pub budgets: GoalBudgets,
}

#[derive(Debug, Clone)]
pub struct ExecParams {
    /// The one-shot prompt. May be empty when `--goal` drives the run.
    pub prompt: String,
    pub mode: PermissionMode,
    pub resume: Option<ResumeTarget>,
    pub output_last_message: Option<PathBuf>,
    pub goal: Option<ExecGoal>,
}

/// The frozen exec event vocabulary (v1). Serialized one JSON object per
/// line; the fixture test pins every type.
pub struct ExecEvents<W: Write> {
    out: W,
    session_id: String,
    seq: u64,
}

impl<W: Write> ExecEvents<W> {
    pub fn new(out: W, session_id: String) -> Self {
        Self {
            out,
            session_id,
            seq: 0,
        }
    }

    fn emit(&mut self, body: serde_json::Value) {
        let mut line = serde_json::json!({
            "v": 1,
            "session_id": self.session_id,
            "seq": self.seq,
        });
        line.as_object_mut()
            .expect("object")
            .extend(body.as_object().expect("object").clone());
        self.seq += 1;
        let mut text = serde_json::to_string(&line).unwrap_or_default();
        text.push('\n');
        // A broken stdout (CI teardown) must not panic the exit path.
        let _ = self.out.write_all(text.as_bytes());
        let _ = self.out.flush();
    }

    pub fn session_started(&mut self, cwd: &str, mode: PermissionMode, resumed: bool) {
        self.emit(serde_json::json!({
            "type": "session_started",
            "cwd": cwd,
            "mode": mode.id(),
            "resumed": resumed,
        }));
    }

    pub fn text_delta(&mut self, text: &str) {
        self.emit(serde_json::json!({ "type": "text_delta", "text": text }));
    }

    pub fn tool_call(&mut self, call_id: &str, name: &str, args: &serde_json::Value) {
        self.emit(serde_json::json!({
            "type": "tool_call",
            "call_id": call_id,
            "name": name,
            "args": args,
        }));
    }

    pub fn tool_result(&mut self, call_id: &str, ok: bool, output_digest: &str) {
        self.emit(serde_json::json!({
            "type": "tool_result",
            "call_id": call_id,
            "ok": ok,
            "output_digest": output_digest,
        }));
    }

    pub fn approval_denied(&mut self, call_id: &str, tool: &str, mode: PermissionMode) {
        self.emit(serde_json::json!({
            "type": "approval_denied",
            "call_id": call_id,
            "tool": tool,
            "mode": mode.id(),
        }));
    }

    pub fn turn_completed(&mut self, turn_id: &str, stop_reason: &str, usage: &Usage) {
        self.emit(serde_json::json!({
            "type": "turn_completed",
            "turn_id": turn_id,
            "stop_reason": stop_reason,
            "usage": {
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
            },
        }));
    }

    pub fn error(&mut self, message: &str) {
        self.emit(serde_json::json!({ "type": "error", "message": message }));
    }
}

/// The non-interactive approval gate (§2.2): read-only tools and the
/// session-internal `goal_complete` control channel auto-approve in every
/// mode; `full_auto` auto-approves CONTAINED fs writes and sandboxed shell
/// exactly like the C2 gate; everything a mode would PROMPT for is
/// auto-DENIED with an `approval_denied` event naming the tool and the
/// mode. There is deliberately no flag that turns prompts into silent
/// approvals.
pub struct ExecApproval<W: Write + Send> {
    pub mode: PermissionMode,
    pub policy: nano_core::permissions::FileSystemSandboxPolicy,
    pub cwd: PathBuf,
    pub sandbox_available: bool,
    pub events: Arc<Mutex<ExecEvents<W>>>,
}

impl<W: Write + Send> std::fmt::Debug for ExecApproval<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecApproval")
            .field("mode", &self.mode)
            .field("sandbox_available", &self.sandbox_available)
            .finish_non_exhaustive()
    }
}

/// The gate decision core, split from the event sink so tests (and the cron
/// fire path, which shares the auto-deny discipline) can drive it without
/// stdout.
pub fn exec_gate_decision(
    call: &ToolCall,
    mode: PermissionMode,
    policy: &nano_core::permissions::FileSystemSandboxPolicy,
    cwd: &Path,
    sandbox_available: bool,
) -> ApprovalDecision {
    if crate::acp_mode::is_read_only_tool(&call.name) || is_control_tool(&call.name) {
        return ApprovalDecision::Approve;
    }
    match mode {
        PermissionMode::ReadOnly => ApprovalDecision::Deny,
        PermissionMode::Default => ApprovalDecision::Deny, // would prompt → auto-deny
        PermissionMode::FullAuto => match call.name.as_str() {
            "fs_write" | "fs_edit" => {
                let contained = call
                    .arguments
                    .get("path")
                    .and_then(|value| value.as_str())
                    .is_some_and(|path| policy.can_write_path_with_cwd(Path::new(path), cwd));
                if contained {
                    ApprovalDecision::Approve
                } else {
                    ApprovalDecision::Deny
                }
            }
            "shell" => {
                if sandbox_available {
                    ApprovalDecision::Approve
                } else {
                    ApprovalDecision::Deny
                }
            }
            // cronjob create prompts even in full_auto (§5.5) — and exec can
            // never prompt, so it auto-denies. mcp__* likewise.
            _ => ApprovalDecision::Deny,
        },
    }
}

impl<W: Write + Send> ApprovalGate for ExecApproval<W> {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision {
        let decision = exec_gate_decision(
            call,
            self.mode,
            &self.policy,
            &self.cwd,
            self.sandbox_available,
        );
        if decision == ApprovalDecision::Deny {
            self.events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .approval_denied(&call.id, &call.name, self.mode);
        }
        decision
    }

    fn denial_reason(&self) -> Option<&'static str> {
        match self.mode {
            PermissionMode::ReadOnly => Some("session is in read_only mode"),
            _ => Some("exec is non-interactive: actions that would prompt are denied"),
        }
    }
}

/// One executed turn's essentials back to the exec driver.
pub struct ExecTurnOutcome {
    pub state: TurnState,
    pub final_text: String,
    pub usage: Usage,
}

/// Atomic, Windows-safe replace-existing write for `--output-last-message`
/// (and the pinned primitive cron store writes mirror): tempfile in the
/// same directory, flush + close, then rename over the target. Called ONLY
/// on the success path — a failed turn never produces a new or partial
/// file.
pub fn atomic_replace_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
        std::process::id()
    ));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_data()?;
    }
    std::fs::rename(&tmp, path)
}

/// Maps the turn end state to the exec exit code (§2.2): 0 completed; 1 for
/// model/runtime failures AND turn-level TurnBudget trips.
pub fn exit_code_for_turn(state: &TurnState) -> i32 {
    match state {
        TurnState::Complete => 0,
        _ => 1,
    }
}

/// Maps a goal drive outcome to the exec exit code: Complete → 0, Blocked →
/// 3, Paused → 6, engine error / turn-level budget trip → 1 (the failure
/// class disambiguation CI scripts key on).
pub fn exit_code_for_goal(outcome: &GoalDriveOutcome) -> i32 {
    match outcome {
        GoalDriveOutcome::Complete { .. } => 0,
        GoalDriveOutcome::Blocked { .. } => 3,
        GoalDriveOutcome::Paused => 6,
        GoalDriveOutcome::EngineError | GoalDriveOutcome::TurnBudgetTrip => 1,
    }
}

/// Resolves the bootstrap seed from exec params (resume target or new).
pub fn resolve_seed(
    sessions_dir: &Path,
    resume: &Option<ResumeTarget>,
) -> Result<(SessionSeed, bool), String> {
    match resume {
        None => Ok((SessionSeed::New, false)),
        Some(ResumeTarget::Id(id)) => Ok((SessionSeed::Resume(id.clone()), true)),
        Some(ResumeTarget::Last) => match latest_session_id(sessions_dir) {
            Ok(Some(id)) => Ok((SessionSeed::Resume(id), true)),
            Ok(None) => Err("no session to resume (sessions directory is empty)".into()),
            Err(err) => Err(format!("cannot scan sessions directory: {err}")),
        },
    }
}

/// Runs one turn of an exec session: journal-first sink feeding the JSONL
/// event stream. Shared by the plain path and the goal driver's per-turn
/// closure.
#[allow(clippy::too_many_arguments)]
pub async fn run_exec_turn<D, T, W>(
    driver: &D,
    tools: &T,
    gate: &dyn ApprovalGate,
    model_name: &str,
    _session: &BootstrappedSession,
    turn_id: &str,
    input: &str,
    context: Vec<nano_model::types::Message>,
    journal: Arc<Mutex<JournalWriter>>,
    events: Arc<Mutex<ExecEvents<W>>>,
    extra_tool_definitions: &[nano_model::types::ToolDefinition],
    // P1: the advertised surface mirrors the executor's resolved backend
    // (design §2.3) — false keeps the pre-P1 surface byte-identical.
    web_search_backed: bool,
) -> ExecTurnOutcome
where
    D: nano_agent::turn::ModelDriver,
    T: nano_agent::turn::ToolExecutor,
    W: Write + Send,
{
    let mut tool_definitions = v1_tool_definitions(web_search_backed);
    tool_definitions.extend(extra_tool_definitions.iter().cloned());
    let engine = TurnEngine {
        model: driver,
        tools,
        budget: TurnBudget::default(),
        model_name: model_name.to_string(),
        tool_definitions,
        approval: Some(gate),
        compaction: None,
        // Headless exec runs the pre-C9 engine posture: no steer queue, no
        // observer channel, no sticky params (a CI-grade run takes its
        // instructions up front).
        robustness: nano_agent::turn::TurnRobustness::default(),
    };
    let mut sink = |envelope: &OpEnvelope| -> bool {
        // Journal first: the durable record leads the live event, never the
        // other way round.
        let journaled = match journal
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .append(envelope)
        {
            Ok(_) => true,
            Err(err) => {
                eprintln!("wayland-nano: session journal append failed: {err}");
                false
            }
        };
        let mut events = events.lock().unwrap_or_else(|p| p.into_inner());
        match &envelope.op {
            Op::AssistantText { text, .. } => events.text_delta(text),
            Op::ToolCall {
                call_id,
                name,
                args,
                ..
            } => events.tool_call(call_id, name, args),
            Op::ToolResult {
                call_id,
                ok,
                output_digest,
                ..
            } => events.tool_result(call_id, *ok, output_digest),
            _ => {}
        }
        journaled
    };
    let result = engine
        .run_turn_streaming_with_context(turn_id, input, context, None, &mut sink)
        .await;
    // C7 wire vocabulary: genuine cancels stay "cancelled"; every other
    // stop/failure surfaces as its typed kind's snake_case wire id (never
    // the logs-side detail).
    let kind_wire_id = |kind: nano_session::NanoErrorKind| {
        serde_json::to_string(&kind)
            .unwrap_or_else(|_| "\"unknown\"".into())
            .trim_matches('"')
            .to_string()
    };
    let stop_reason = match &result.state {
        TurnState::Complete => "end_turn".to_string(),
        TurnState::Stopped(info) if info.kind == nano_session::NanoErrorKind::UserCancelled => {
            "cancelled".to_string()
        }
        TurnState::Stopped(info) => kind_wire_id(info.kind),
        TurnState::Failed(err) => kind_wire_id(err.kind),
        other => other.label().to_lowercase(),
    };
    events
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .turn_completed(turn_id, &stop_reason, &result.usage);
    ExecTurnOutcome {
        state: result.state,
        final_text: result.final_text,
        usage: result.usage,
    }
}

/// Classifies a finished turn for the goal driver's per-turn contract.
pub fn goal_turn_stop(state: &TurnState) -> GoalTurnStop {
    match state {
        TurnState::Complete => GoalTurnStop::Complete,
        TurnState::Stopped(info) if info.kind == nano_session::NanoErrorKind::BudgetExhausted => {
            GoalTurnStop::TurnBudget
        }
        TurnState::Stopped(info) if info.kind == nano_session::NanoErrorKind::UserCancelled => {
            GoalTurnStop::Cancelled
        }
        TurnState::Stopped(_) => GoalTurnStop::Failed,
        TurnState::Failed(_) => GoalTurnStop::Failed,
        _ => GoalTurnStop::Failed,
    }
}

/// Journals a new goal's activation (GoalBegin + GoalStatus{active}),
/// journal-first, fail-closed. Returns the goal id.
pub fn begin_goal(
    journal: &Arc<Mutex<JournalWriter>>,
    journal_sequence: &AtomicU64,
    session_id: &str,
    objective: &str,
    budgets: &GoalBudgets,
) -> Result<String, String> {
    validate_objective(objective)?;
    // Nanos in ids: never collide across processes/resumes (the journal
    // writer dedupes repeated ids — a collision would silently drop ops).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let goal_id = format!("{session_id}-goal-{nanos}");
    {
        let mut writer = journal.lock().unwrap_or_else(|p| p.into_inner());
        writer
            .append(&OpEnvelope::new(
                format!("{session_id}-goalbegin-{nanos}"),
                "now",
                Op::GoalBegin {
                    goal_id: goal_id.clone(),
                    objective: objective.to_string(),
                    budgets: *budgets,
                },
            ))
            .and_then(|_| writer.sync())
            .map_err(|err| format!("cannot journal goal begin: {err}"))?;
    }
    journal_goal_transition(
        journal,
        session_id,
        journal_sequence,
        &goal_id,
        nano_session::GoalStatusKind::Active,
        nano_session::GoalReason::Unspecified,
        None,
    )
    .map_err(|err| format!("cannot journal goal activation: {err}"))?;
    Ok(goal_id)
}

/// Re-activates a paused goal on explicit resume (a resumed session's goal
/// normalized to `paused`; `exec --goal` is the explicit resume).
pub fn resume_goal(
    journal: &Arc<Mutex<JournalWriter>>,
    journal_sequence: &AtomicU64,
    session_id: &str,
    goal_id: &str,
) -> Result<(), String> {
    journal_goal_transition(
        journal,
        session_id,
        journal_sequence,
        goal_id,
        nano_session::GoalStatusKind::Active,
        nano_session::GoalReason::Unspecified,
        None,
    )
    .map_err(|err| format!("cannot journal goal resume: {err}"))
}

/// The default clock for exec (wall-clock budget deadlines).
pub fn system_clock() -> SystemClock {
    SystemClock
}

/// Marker trait alias so `run_exec_turn`'s generics stay readable at call
/// sites.
pub trait ExecDriver: nano_agent::turn::ModelDriver {}
impl<T: nano_agent::turn::ModelDriver> ExecDriver for T {}

/// Helper: the MCP-merged executor for an exec turn. Specs come from the
/// caller (production: NANO_MCP_SERVERS — exec has no Desktop param
/// channel; tests inject doubles). The registry drops with the executor —
/// its stdio children are killed, so exec never strands an MCP child.
pub fn mcp_executor_for<'a, T: nano_agent::turn::ToolExecutor>(
    inner: &'a T,
    specs: &[nano_agent::mcp::McpServerSpec],
) -> McpToolExecutor<'a> {
    McpToolExecutor::new(crate::mcp_specs::register_all(specs.to_vec()), inner)
}
