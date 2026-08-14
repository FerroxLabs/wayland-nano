//! Session fork (C11 §3): copy a journal prefix under lock, never touch the
//! parent.
//!
//! Child journal shape — genesis + lineage + imported prefix:
//! 1. one fresh `SessionBegin` for the child (stream position 0 — the ONLY
//!    envelope replay takes identity from);
//! 2. one `ForkedFrom` lineage op declaring the imported-region length;
//! 3. the parent envelopes through the fork point, copied BYTE-VERBATIM
//!    (including the parent's own `SessionBegin`, which is inert on replay);
//! 4. when the imported prefix ends with a non-terminal goal, goal close-out
//!    ops (`GoalStatus{blocked, cancelled}` + `GoalEnd`) referencing the
//!    PARENT goal id — under the replay suppression algorithm these fold as
//!    audit-only no-ops against a live state that never imported the goal;
//!    they are the durable record that the goal did not cross the fork.
//!
//! The parent proof: SHA-256 before and after the copy, returned in the
//! outcome and asserted equal — stability against all COOPERATING writers,
//! with the copy running under the parent's OS file lock (the SessionGuard's
//! cross-process layer). A fork that cannot prove the parent untouched fails
//! closed.

use crate::lock::FileLock;
use crate::lock::LockError;
use crate::op::GoalOutcome;
use crate::op::GoalReason;
use crate::op::GoalStatusKind;
use crate::op::Op;
use crate::op::OpEnvelope;
use crate::replay::SessionState;
use sha2::Digest;
use sha2::Sha256;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

/// Where the child's imported prefix ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkPoint {
    /// After the last complete envelope (the default).
    End,
    /// Immediately after the named turn's `TurnEnd` envelope — an exact,
    /// deterministic boundary (no rollout-scan heuristics).
    Turn(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkOutcome {
    pub child_session_id: String,
    pub child_path: PathBuf,
    pub parent_digest_before: String,
    pub parent_digest_after: String,
    /// Envelopes in the imported prefix (the self-describing replay boundary).
    pub imported_ops: u64,
    /// Id of the last imported parent envelope (the fork point).
    pub parent_op_id: String,
    /// Whether goal close-out ops were appended (prefix ended non-terminal).
    pub closed_parent_goal: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ForkError {
    #[error("parent session journal not found: {0}")]
    ParentNotFound(PathBuf),
    #[error("parent journal integrity error: {0}")]
    ParentCorrupt(String),
    /// Fork-at-turn named a turn with a `TurnBegin` but no `TurnEnd` — a
    /// crashed turn. Typed error, never a silent truncation.
    #[error("cannot fork at crashed turn (no TurnEnd journaled): {turn_id}")]
    CrashedTurn { turn_id: String },
    #[error("cannot fork at unknown turn: {turn_id}")]
    TurnNotFound { turn_id: String },
    /// The parent digest changed between the before/after reads — a writer
    /// bypassed the lock. Fail closed; the child is removed.
    #[error("parent journal mutated during fork copy")]
    ParentMutatedDuringCopy,
    #[error("child journal already exists: {0}")]
    ChildExists(PathBuf),
    /// The parent's journal lock is held (a turn, another fork, or a cron
    /// fire is in progress) — typed busy, never a silent queue.
    #[error("session busy: journal lock is held")]
    Busy,
    #[error("fork io error: {0}")]
    Io(#[from] io::Error),
}

impl From<LockError> for ForkError {
    fn from(err: LockError) -> Self {
        match err {
            LockError::Busy => ForkError::Busy,
            LockError::Io(err) => ForkError::Io(err),
        }
    }
}

/// Forks the parent journal at `parent_path` into a NEW child journal at
/// `child_path` with fresh identity `child_id`. See module docs for the
/// child shape and the parent-stability proof. The whole before-digest →
/// copy → after-digest sequence runs under the parent's OS file lock; the
/// caller layers the in-process SessionGuard mutex on top.
pub fn fork_journal(
    parent_path: &Path,
    child_path: &Path,
    child_id: &str,
    at: &ForkPoint,
) -> Result<ForkOutcome, ForkError> {
    if !parent_path.exists() {
        return Err(ForkError::ParentNotFound(parent_path.to_path_buf()));
    }
    // Cross-process exclusion for the whole before→copy→after sequence.
    let _lock = FileLock::try_acquire(parent_path)?;
    fork_journal_locked(parent_path, child_path, child_id, at)
}

/// F-P4-3: fork a parent journal the CALLER already owns — the owning host
/// holds the session's lifetime OS lock (single-writer ownership), so this
/// variant skips the lock acquisition that would otherwise self-conflict.
/// Contract: the caller must hold the exclusive journal lock on
/// `parent_path` (plus the in-process SessionGuard layer) for the whole
/// call; the before/after digest proof still fails closed against any
/// writer that bypassed ownership.
pub fn fork_journal_when_owned(
    parent_path: &Path,
    child_path: &Path,
    child_id: &str,
    at: &ForkPoint,
) -> Result<ForkOutcome, ForkError> {
    if !parent_path.exists() {
        return Err(ForkError::ParentNotFound(parent_path.to_path_buf()));
    }
    fork_journal_locked(parent_path, child_path, child_id, at)
}

/// The lock-free body: everything after exclusion is established (either by
/// a freshly acquired [`FileLock`] or by the caller's lifetime ownership).
fn fork_journal_locked(
    parent_path: &Path,
    child_path: &Path,
    child_id: &str,
    at: &ForkPoint,
) -> Result<ForkOutcome, ForkError> {
    let before_bytes = std::fs::read(parent_path)?;
    let digest_before = sha256_hex(&before_bytes);

    // Strict line scan: every non-final line must parse (integrity errors
    // fail the fork); a crash-torn final line is excluded from the prefix
    // (same tolerance as the reader). Each parsed envelope is paired with
    // the byte offset of the end of its line (newline-inclusive), so a
    // turn-boundary cut stays byte-exact.
    let lines = split_complete_lines(&before_bytes);
    let mut parsed: Vec<(OpEnvelope, usize)> = Vec::new();
    for (index, (span, text)) in lines.iter().enumerate() {
        match serde_json::from_str::<OpEnvelope>(text) {
            Ok(envelope) => parsed.push((envelope, *span)),
            Err(_err) if index == lines.len() - 1 => break, // torn tail: excluded
            Err(err) => {
                return Err(ForkError::ParentCorrupt(format!(
                    "line {}: {err}",
                    index + 1
                )));
            }
        }
    }

    // Resolve the fork point to a prefix cut.
    let (imported, prefix_len): (Vec<OpEnvelope>, usize) = match at {
        ForkPoint::End => {
            let cut = parsed.last().map(|(_, span)| *span).unwrap_or(0);
            (
                parsed.into_iter().map(|(envelope, _)| envelope).collect(),
                cut,
            )
        }
        ForkPoint::Turn(turn_id) => {
            let begin_at = parsed.iter().position(
                |(e, _)| matches!(&e.op, Op::TurnBegin { turn_id: id, .. } if id == turn_id),
            );
            let end_at = parsed.iter().position(
                |(e, _)| matches!(&e.op, Op::TurnEnd { turn_id: id, .. } if id == turn_id),
            );
            match (begin_at, end_at) {
                (Some(_), Some(end)) => {
                    let cut = parsed[end].1;
                    (
                        parsed[..=end]
                            .iter()
                            .map(|(envelope, _)| envelope.clone())
                            .collect(),
                        cut,
                    )
                }
                (Some(_), None) => {
                    return Err(ForkError::CrashedTurn {
                        turn_id: turn_id.clone(),
                    });
                }
                (None, _) => {
                    return Err(ForkError::TurnNotFound {
                        turn_id: turn_id.clone(),
                    });
                }
            }
        }
    };
    if imported.is_empty() {
        return Err(ForkError::ParentCorrupt("no complete envelopes".into()));
    }
    let parent_op_id = imported.last().expect("non-empty").id.clone();
    let imported_ops = imported.len() as u64;

    // The byte-identical-parent proof: re-read and compare digests.
    let after_bytes = std::fs::read(parent_path)?;
    let digest_after = sha256_hex(&after_bytes);
    if digest_before != digest_after {
        return Err(ForkError::ParentMutatedDuringCopy);
    }

    // Does the imported prefix end with a non-terminal goal? (Fold without
    // the post-fold normalize: a goal left `active` OR `paused` is
    // non-terminal either way.)
    let imported_state = SessionState::fold(&imported);
    let open_goal = imported_state.goal.filter(|goal| !goal.is_terminal());

    // ── Write the child: genesis → lineage → verbatim prefix → close-out ──
    if let Some(parent) = child_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut child = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(child_path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            return Err(ForkError::ChildExists(child_path.to_path_buf()));
        }
        Err(err) => return Err(ForkError::Io(err)),
    };
    let write_result = (|| -> io::Result<()> {
        let genesis = OpEnvelope::new(
            format!("{child_id}-begin-1"),
            "now",
            Op::SessionBegin {
                session_id: child_id.to_string(),
                // The child inherits the parent's cwd at the fork point.
                cwd: imported_state.cwd.clone().unwrap_or_default(),
            },
        );
        write_line(&mut child, &genesis)?;
        let lineage = OpEnvelope::new(
            format!("{child_id}-fork-1"),
            "now",
            Op::ForkedFrom {
                parent_session_id: imported_state.session_id.clone().unwrap_or_default(),
                parent_op_id: parent_op_id.clone(),
                at_turn: match at {
                    ForkPoint::End => None,
                    ForkPoint::Turn(turn_id) => Some(turn_id.clone()),
                },
                parent_digest_before: digest_before.clone(),
                parent_digest_after: digest_after.clone(),
                imported_ops,
            },
        );
        write_line(&mut child, &lineage)?;
        // Byte-verbatim imported prefix — never rewritten, re-stamped, or
        // re-ordered (rewriting would void the digest proof and the C1
        // replay-equivalence argument).
        child.write_all(&before_bytes[..prefix_len])?;
        // Goal close-out (kimi GOAL_FORK_CLEARED equivalent): the durable
        // record that the parent's goal did not cross. These reference the
        // PARENT goal id, so replay folds them as audit-only no-ops against
        // the child's (empty) live goal state.
        if let Some(goal) = &open_goal {
            let close_status = OpEnvelope::new(
                format!("{child_id}-fork-close-1"),
                "now",
                Op::GoalStatus {
                    goal_id: goal.goal_id.clone(),
                    status: GoalStatusKind::Blocked,
                    reason: GoalReason::Cancelled,
                },
            );
            write_line(&mut child, &close_status)?;
            let close_end = OpEnvelope::new(
                format!("{child_id}-fork-close-2"),
                "now",
                Op::GoalEnd {
                    goal_id: goal.goal_id.clone(),
                    outcome: GoalOutcome::Blocked,
                },
            );
            write_line(&mut child, &close_end)?;
        }
        child.sync_data()
    })();
    if let Err(err) = write_result {
        drop(child);
        let _ = std::fs::remove_file(child_path); // never leave a half child
        return Err(ForkError::Io(err));
    }

    Ok(ForkOutcome {
        child_session_id: child_id.to_string(),
        child_path: child_path.to_path_buf(),
        parent_digest_before: digest_before,
        parent_digest_after: digest_after,
        imported_ops,
        parent_op_id,
        closed_parent_goal: open_goal.is_some(),
    })
}

/// Byte spans (end offsets, newline-inclusive) and text of each complete
/// line. A final line without a trailing newline is included (the caller
/// applies torn-tail rules); empty lines are skipped by the parser stage.
fn split_complete_lines(bytes: &[u8]) -> Vec<(usize, String)> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in text.split('\n') {
        let line_len = line.len() + 1;
        let trimmed = line.trim_end_matches('\r');
        if !trimmed.is_empty() {
            out.push((offset + line_len, trimmed.to_string()));
        }
        offset += line_len;
    }
    out
}

fn write_line(file: &mut std::fs::File, envelope: &OpEnvelope) -> io::Result<()> {
    let mut line = serde_json::to_vec(envelope)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    line.push(b'\n');
    file.write_all(&line)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
