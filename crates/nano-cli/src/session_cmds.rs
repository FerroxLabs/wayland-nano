//! `wayland-nano session fork` and `wayland-nano goal …` (C11 §3.3/§4.4):
//! thin CLI mirrors over the nano-session/nano-agent library APIs — no
//! business logic in the adapter. All transitions are journaled under the
//! SessionGuard, journal-first; JSON on stdout, exit 0 / 2 (usage, not
//! found, busy, io).

use nano_agent::bootstrap::{is_fs_safe_session_id, new_session_id, session_guard_registry};
use nano_agent::goal::{journal_goal_transition, validate_objective};
use nano_protocol::permission_mode::PermissionMode;
use nano_session::op::GoalBudgets;
// (JournalWriter import retired: appends route through JournalCoordinator)
use nano_session::{
    ForkPoint, GoalOutcome, GoalReason, GoalStatusKind, SessionState, fork_journal,
};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

fn journal_path(sessions_dir: &Path, session_id: &str) -> Result<std::path::PathBuf, String> {
    if !is_fs_safe_session_id(session_id) {
        return Err(format!("invalid session id: {session_id}"));
    }
    let path = sessions_dir.join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return Err(format!("session not found: {session_id}"));
    }
    Ok(path)
}

/// Fork core (C11): shared by the CLI mirror and the ACP
/// `_wayland/session/fork` adapter — one implementation, no business logic
/// in either adapter. `parent_owned` is true only when the caller holds the
/// parent session's lifetime ownership lock (F-P4-3: the acp-host forking
/// its own active session); then the OS lock acquisition is skipped — it
/// would self-conflict with the ownership handle — and the SessionGuard's
/// in-process layer still excludes a mid-turn append or cron fire for the
/// whole digest→copy→digest sequence.
pub fn session_fork_core(
    sessions_dir: &Path,
    session_id: &str,
    at_turn: Option<String>,
    parent_owned: bool,
) -> Result<serde_json::Value, String> {
    session_fork_core_with_binding(sessions_dir, session_id, at_turn, parent_owned, |_| {
        Ok(None)
    })
}

/// Fork core plus the authenticated child-binding step. The child journal is
/// created first under the parent lock; an activated caller then binds the
/// child from the already-validated parent authority before success becomes
/// visible. Any binding failure removes the child, so a half-authorized fork
/// cannot later be loaded.
pub fn session_fork_core_with_binding<F>(
    sessions_dir: &Path,
    session_id: &str,
    at_turn: Option<String>,
    parent_owned: bool,
    bind_child: F,
) -> Result<serde_json::Value, String>
where
    F: FnOnce(&str) -> Result<Option<String>, String>,
{
    let parent = journal_path(sessions_dir, session_id)?;
    // fork_journal takes the OS journal lock itself for the whole
    // before-digest → copy → after-digest sequence — that IS the
    // SessionGuard's cross-process layer, and every in-process writer
    // (turn, cron fire) holds the same lock via its guard, so the full
    // contention matrix resolves to typed busy here. (Acquiring the
    // registry guard as well would self-conflict on the second lock.)
    let child_id = new_session_id();
    let child = sessions_dir.join(format!("{child_id}.jsonl"));
    let at = match &at_turn {
        Some(turn_id) => ForkPoint::Turn(turn_id.clone()),
        None => ForkPoint::End,
    };
    let outcome = if parent_owned {
        let _guard = session_guard_registry()
            .try_acquire(&parent)
            .map_err(|err| err.to_string())?;
        nano_session::fork_journal_when_owned(&parent, &child, &child_id, &at)
    } else {
        fork_journal(&parent, &child, &child_id, &at)
    }
    .map_err(|err| err.to_string())?;
    let resume_fingerprint = match bind_child(&outcome.child_session_id) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let rollback = std::fs::remove_file(&outcome.child_path);
            if rollback.is_err() || outcome.child_path.exists() {
                return Err(format!(
                    "{error}; fork child rollback failed: {}",
                    outcome.child_path.display()
                ));
            }
            return Err(error);
        }
    };
    Ok(serde_json::json!({
        "child_session_id": outcome.child_session_id,
        "parent_digest_before": outcome.parent_digest_before,
        "parent_digest_after": outcome.parent_digest_after,
        "imported_ops": outcome.imported_ops,
        "at_turn": at_turn,
        "closed_parent_goal": outcome.closed_parent_goal,
        "resume_fingerprint": resume_fingerprint,
    }))
}

/// `wayland-nano session fork <session_id> [--at-turn <turn_id>]`.
pub fn session_fork(
    sessions_dir: &Path,
    session_id: &str,
    at_turn: Option<String>,
    out: &mut dyn Write,
) -> i32 {
    match session_fork_core(sessions_dir, session_id, at_turn, false) {
        Ok(json) => {
            let _ = writeln!(out, "{json}");
            0
        }
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            2
        }
    }
}

/// `wayland-nano goal set <session_id> <objective> [--token-budget N
/// --turn-budget N --wall-clock-budget MS]`.
pub fn goal_set_core(
    sessions_dir: &Path,
    session_id: &str,
    objective: &str,
    budgets: &GoalBudgets,
) -> Result<serde_json::Value, String> {
    validate_objective(objective)?;
    let journal = journal_path(sessions_dir, session_id)?;
    let _guard = session_guard_registry()
        .try_acquire(&journal)
        .map_err(|err| err.to_string())?;
    let report = nano_session::read_journal(&journal).map_err(|err| err.to_string())?;
    let state = SessionState::fold_strict(&report.envelopes).map_err(|err| err.to_string())?;
    if let Some(goal) = &state.goal
        && !goal.is_terminal()
    {
        return Err(format!(
            "session already has a non-terminal goal: {} ({:?})",
            goal.goal_id, goal.status
        ));
    }
    let sequence = AtomicU64::new(1);
    let goal_id = crate::exec_mode::begin_goal(
        &Arc::new(nano_session::JournalCoordinator::open(&journal).map_err(|err| err.to_string())?),
        &sequence,
        session_id,
        objective,
        budgets,
    )?;
    Ok(serde_json::json!({ "goal_id": goal_id, "status": "active" }))
}

/// `wayland-nano goal set <session_id> <objective> [--budgets]`.
pub fn goal_set(
    sessions_dir: &Path,
    session_id: &str,
    objective: &str,
    budgets: &GoalBudgets,
    out: &mut dyn Write,
) -> i32 {
    match goal_set_core(sessions_dir, session_id, objective, budgets) {
        Ok(json) => {
            let _ = writeln!(out, "{json}");
            0
        }
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            2
        }
    }
}

/// Goal status core (shared CLI/ACP): a thin journal reader.
pub fn goal_status_core(
    sessions_dir: &Path,
    session_id: &str,
) -> Result<serde_json::Value, String> {
    let journal = journal_path(sessions_dir, session_id)?;
    let report = nano_session::read_journal(&journal).map_err(|err| err.to_string())?;
    let state = SessionState::fold_strict(&report.envelopes).map_err(|err| err.to_string())?;
    Ok(match &state.goal {
        Some(goal) => serde_json::json!({
            "goal_id": goal.goal_id,
            "status": serde_json::to_value(goal.status).unwrap_or_default(),
            "reason": serde_json::to_value(goal.reason).unwrap_or_default(),
            "objective": goal.objective,
            "budgets": goal.budgets,
        }),
        None => serde_json::json!({ "goal": serde_json::Value::Null }),
    })
}

/// `wayland-nano goal status <session_id>` — a thin journal reader.
pub fn goal_status(sessions_dir: &Path, session_id: &str, out: &mut dyn Write) -> i32 {
    match goal_status_core(sessions_dir, session_id) {
        Ok(json) => {
            let _ = writeln!(out, "{json}");
            0
        }
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            2
        }
    }
}

/// `wayland-nano goal pause|resume|cancel <session_id>`. Journal-first
/// transitions; replay normalization keeps a killed goal paused until an
/// explicit resume.
pub fn goal_transition_core(
    sessions_dir: &Path,
    session_id: &str,
    action: &str,
) -> Result<serde_json::Value, String> {
    let journal = journal_path(sessions_dir, session_id)?;
    let _guard = session_guard_registry()
        .try_acquire(&journal)
        .map_err(|err| err.to_string())?;
    let report = nano_session::read_journal(&journal).map_err(|err| err.to_string())?;
    let state = SessionState::fold_strict(&report.envelopes).map_err(|err| err.to_string())?;
    let Some(goal) = state.goal.filter(|goal| !goal.is_terminal()) else {
        return Err("no non-terminal goal in this session".to_string());
    };
    let (status, reason, terminal) = match action {
        "pause" => (GoalStatusKind::Paused, GoalReason::Unspecified, None),
        "resume" => (GoalStatusKind::Active, GoalReason::Unspecified, None),
        "cancel" => (
            GoalStatusKind::Blocked,
            GoalReason::Cancelled,
            Some(GoalOutcome::Blocked),
        ),
        other => return Err(format!("unknown goal action: {other}")),
    };
    let coordinator =
        nano_session::JournalCoordinator::open(&journal).map_err(|err| err.to_string())?;
    journal_goal_transition(
        &coordinator,
        session_id,
        &AtomicU64::new(1),
        &goal.goal_id,
        status,
        reason,
        terminal,
    )
    .map_err(|err| err.to_string())?;
    Ok(serde_json::json!({
        "goal_id": goal.goal_id,
        "status": serde_json::to_value(status).unwrap_or_default(),
    }))
}

/// `wayland-nano goal pause|resume|cancel <session_id>`. Journal-first
/// transitions; replay normalization keeps a killed goal paused until an
/// explicit resume.
pub fn goal_transition(
    sessions_dir: &Path,
    session_id: &str,
    action: &str,
    out: &mut dyn Write,
) -> i32 {
    match goal_transition_core(sessions_dir, session_id, action) {
        Ok(json) => {
            let _ = writeln!(out, "{json}");
            0
        }
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            2
        }
    }
}

/// Parses `--mode` values onto the C2 vocabulary (typed error otherwise).
pub fn parse_mode(raw: &str) -> Result<PermissionMode, String> {
    PermissionMode::parse(raw).ok_or_else(|| format!("unknown mode: {raw}"))
}
