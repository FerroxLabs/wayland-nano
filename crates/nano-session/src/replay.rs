//! Reducer-fold replay: envelopes → executable session state.
//!
//! Restore invariants (Kimi-derived):
//! - a stranded `TurnBegin` (no `TurnEnd`) marks the turn `Interrupted`: the
//!   pending user input is preserved, tool calls that already returned keep
//!   their results, and no tool call is re-executed on resume;
//! - a stranded `CompactionBegin` (no Complete/Cancel) resets to `Idle`;
//! - duplicate envelope ids never double-apply;
//! - `Unknown` ops are skipped without failing the fold;
//! - covered-by-compaction ops keep their durable effects (changed files)
//!   but drop from the pending-execution surface.
//!
//! C11 fork/goal/cron replay rules:
//! - session identity comes ONLY from the genesis `SessionBegin` at stream
//!   position 0; any later `SessionBegin` (e.g. inside a forked journal's
//!   imported prefix) is inert lineage data;
//! - `ForkedFrom` opens an imported region of declared length: inside it,
//!   context and turn-structure ops fold exactly as the parent's replay
//!   folds them, while CONTROL ops (all goal ops, `CronFired`) are
//!   suppressed from live state into an audit-only namespace at EVERY fold
//!   step — there is no transient application followed by correction;
//! - the live goal machine only tracks goals begun by a CHILD-AUTHORED
//!   `GoalBegin`; `GoalStatus`/`GoalEnd` naming any other goal_id fold as
//!   audit-only no-ops;
//! - a goal whose last state is `active` at the tail normalizes to `paused`
//!   (an interrupted goal never silently resumes driving turns);
//! - a declared `imported_ops` count that overruns the stream is a typed
//!   replay error (fail-closed), never a silent short fold.

use crate::op::GoalBudgets;
use crate::op::GoalReason;
use crate::op::GoalStatusKind;
use crate::op::Op;
use crate::op::OpEnvelope;
use crate::op::TurnOutcome;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionPhase {
    Idle,
    Running { compaction_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenToolCall {
    pub turn_id: String,
    pub call_id: String,
    pub name: String,
    pub args: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenTurn {
    pub turn_id: String,
    pub input: String,
}

/// The live (child-authored) goal machine state (C11). Terminal states stay
/// visible for status reporting; `is_terminal` distinguishes them.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalLive {
    pub goal_id: String,
    pub status: GoalStatusKind,
    pub reason: GoalReason,
    pub objective: String,
    pub budgets: GoalBudgets,
}

impl GoalLive {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            GoalStatusKind::Blocked | GoalStatusKind::Complete
        )
    }
}

/// Typed replay failures (fail-closed). Kept separate from journal
/// integrity errors (the reader owns those): this is about a well-formed
/// envelope stream whose fork lineage declaration does not match reality.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    /// The `ForkedFrom` op declared more imported envelopes than the stream
    /// actually carries — the replay boundary is self-describing, and a
    /// count that overruns the file means the journal was truncated or the
    /// lineage op forged. Never guess.
    #[error("fork lineage declares {declared} imported ops but only {actual} envelopes follow")]
    ImportedRegionOverrun { declared: u64, actual: u64 },
}

#[derive(Debug, Default)]
pub struct SessionState {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    /// The in-progress (or crash-interrupted) turn, if any.
    pub open_turn: Option<OpenTurn>,
    pub turn_interrupted: bool,
    pub open_tool_calls: Vec<OpenToolCall>,
    pub changed_files: BTreeSet<String>,
    pub compaction: Option<CompactionPhase>,
    pub last_compaction_summary: Option<String>,
    /// Live goal state (C11): folds ONLY child-authored goal ops — ops
    /// inside an imported region, and ops naming a goal this session never
    /// began, are audit-only (see `suppressed_control_ops`).
    pub goal: Option<GoalLive>,
    /// Journaled cron occurrence ids fired into THIS session (child-authored
    /// region only) — the idempotency set for the §5.4 fire transaction.
    pub cron_fired_occurrences: BTreeSet<String>,
    /// Latest scheduled-fire instant journaled per cron job (child-authored
    /// only), extracted from occurrence ids. The journal-authoritative
    /// `last_fired` source for runner reconciliation.
    pub cron_last_fired: BTreeMap<String, String>,
    /// Audit-only namespace: control ops that were seen but NEVER folded
    /// into live state (imported goal/cron ops, goal ops naming a goal this
    /// session did not begin). Reader-visible proof that suppression
    /// happened; never a mutation channel.
    pub suppressed_control_ops: Vec<OpEnvelope>,
    /// Set when the fold hit a structural failure (see [`ReplayError`]);
    /// `fold` records it, `fold_strict` returns it.
    pub integrity_error: Option<ReplayError>,
    seen_ids: HashSet<String>,
    /// Envelopes applied so far — position 0 carries the session identity.
    applied: u64,
    /// Remaining envelopes in a fork's imported region (C11), when inside one.
    imported_remaining: u64,
    /// The declared length of the current imported region (for typed errors).
    imported_declared: u64,
}

impl SessionState {
    pub fn fold(envelopes: &[OpEnvelope]) -> Self {
        let mut state = Self::new();
        for envelope in envelopes {
            state.apply(envelope);
        }
        state.restore_invariants();
        state
    }

    /// Fail-closed fold: structural replay failures (e.g. a fork lineage
    /// count overrunning the stream) come back as typed errors.
    pub fn fold_strict(envelopes: &[OpEnvelope]) -> Result<Self, ReplayError> {
        let state = Self::fold(envelopes);
        match &state.integrity_error {
            Some(err) => Err(err.clone()),
            None => Ok(state),
        }
    }

    /// A fresh fold accumulator (the same initial state `fold` starts from):
    /// compaction phase known-idle, everything else empty. Public so tests
    /// and hosts can fold incrementally and assert per-step invariants.
    pub fn new() -> Self {
        SessionState {
            compaction: Some(CompactionPhase::Idle),
            ..Default::default()
        }
    }

    /// Folds one envelope. Public for per-step invariant assertions (C11 §7
    /// fork replay-namespace tests); callers folding a whole stream should
    /// prefer [`SessionState::fold`] / [`SessionState::fold_strict`], which
    /// also apply the post-fold restore invariants.
    pub fn apply(&mut self, envelope: &OpEnvelope) {
        if !self.seen_ids.insert(envelope.id.clone()) {
            return; // idempotent: duplicate ids never double-apply
        }
        let position = self.applied;
        self.applied += 1;

        // ── C11 imported-region classification ─────────────────────────
        // Inside an imported region, envelopes fold BY CLASS: context and
        // turn-structure ops fold exactly as the parent's replay folds them;
        // control ops (goal/cron/lineage) are suppressed from live state
        // into the audit namespace at THIS fold step — never applied and
        // corrected later. A nested `ForkedFrom` is lineage audit data: it
        // counts against the OUTER region but never re-opens one.
        if self.imported_remaining > 0 {
            self.imported_remaining -= 1;
            match &envelope.op {
                Op::GoalBegin { .. }
                | Op::GoalStatus { .. }
                | Op::GoalEnd { .. }
                | Op::CronFired { .. }
                | Op::ForkedFrom { .. } => {
                    self.suppressed_control_ops.push(envelope.clone());
                    return;
                }
                _ => {}
            }
        }

        match &envelope.op {
            Op::SessionBegin { session_id, cwd } => {
                // Identity comes ONLY from the genesis envelope at stream
                // position 0. Any later SessionBegin — a resume refresh, or
                // the parent's begin inside an imported prefix — is inert
                // lineage/audit data and folds into no state.
                if position == 0 {
                    self.session_id = Some(session_id.clone());
                    self.cwd = Some(cwd.clone());
                }
            }
            Op::TurnBegin { turn_id, input } => {
                self.open_turn = Some(OpenTurn {
                    turn_id: turn_id.clone(),
                    input: input.clone(),
                });
                self.turn_interrupted = false;
            }
            Op::ToolCall {
                turn_id,
                call_id,
                name,
                args,
            } => {
                self.open_tool_calls.push(OpenToolCall {
                    turn_id: turn_id.clone(),
                    call_id: call_id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                });
            }
            Op::ToolResult {
                call_id,
                changed_files,
                ..
            } => {
                self.open_tool_calls.retain(|call| &call.call_id != call_id);
                self.changed_files.extend(changed_files.iter().cloned());
            }
            Op::TurnEnd { outcome, .. } => {
                self.open_turn = None;
                self.turn_interrupted = matches!(outcome, TurnOutcome::Interrupted);
            }
            Op::CompactionBegin { compaction_id } => {
                self.compaction = Some(CompactionPhase::Running {
                    compaction_id: compaction_id.clone(),
                });
            }
            Op::CompactionComplete {
                summary,
                changed_files,
                ..
            } => {
                self.compaction = Some(CompactionPhase::Idle);
                self.last_compaction_summary = Some(summary.clone());
                self.changed_files.extend(changed_files.iter().cloned());
            }
            Op::CompactionCancel { .. } => {
                self.compaction = Some(CompactionPhase::Idle);
            }
            // Assistant text is transcript, not execution state: replay cares
            // about open work and durable effects, not the wording of replies.
            Op::AssistantText { .. } => {}
            // Mode changes are audit history only (C2): context-neutral on
            // replay, never restored into session state.
            Op::ModeSet { .. } => {}
            Op::ForkedFrom { imported_ops, .. } => {
                // Opens the imported region (child-authored region: only a
                // genesis-positioned lineage op reaches this arm — nested
                // ones are consumed by the classification above). The
                // boundary is the DECLARED count, never content-sniffed.
                self.imported_remaining = *imported_ops;
                self.imported_declared = *imported_ops;
            }
            Op::GoalBegin {
                goal_id,
                objective,
                budgets,
            } => {
                // One current goal per session. The writer side rejects a
                // second non-terminal goal; on replay the journal is
                // authoritative, so a later GoalBegin supersedes.
                self.goal = Some(GoalLive {
                    goal_id: goal_id.clone(),
                    status: GoalStatusKind::Active,
                    reason: GoalReason::Unspecified,
                    objective: objective.clone(),
                    budgets: *budgets,
                });
            }
            Op::GoalStatus {
                goal_id,
                status,
                reason,
            } => match &mut self.goal {
                // Only goals THIS session began are live; ops naming any
                // other goal_id (e.g. a fork's close-out ops referencing the
                // parent goal) fold as audit-only no-ops.
                Some(goal) if goal.goal_id == *goal_id => {
                    goal.status = *status;
                    goal.reason = *reason;
                }
                _ => self.suppressed_control_ops.push(envelope.clone()),
            },
            Op::GoalEnd { goal_id, outcome } => match &mut self.goal {
                Some(goal) if goal.goal_id == *goal_id => {
                    goal.status = match outcome {
                        crate::op::GoalOutcome::Complete => GoalStatusKind::Complete,
                        _ => GoalStatusKind::Blocked,
                    };
                }
                _ => self.suppressed_control_ops.push(envelope.clone()),
            },
            Op::CronFired {
                job_id,
                occurrence_id,
                ..
            } => {
                self.cron_fired_occurrences.insert(occurrence_id.clone());
                // occurrence_id = "{job_id}:{scheduled_fire_time}" (RFC3339
                // UTC, minute resolution — lexicographic == chronological).
                // Malformed identities are REJECTED: they must never
                // influence last_fired.
                if let Some((job, scheduled)) = occurrence_id.split_once(':')
                    && job == job_id
                    && is_rfc3339_utc_minute(scheduled)
                {
                    let entry = self.cron_last_fired.entry(job_id.clone()).or_default();
                    if scheduled > entry.as_str() {
                        *entry = scheduled.to_string();
                    }
                }
            }
            Op::Unknown => {}
        }
    }

    /// Post-fold restore rules: anything left `running` at the tail of the
    /// journal is a crash artifact and resets to a safe state.
    fn restore_invariants(&mut self) {
        if self.open_turn.is_some() {
            self.turn_interrupted = true;
        }
        if matches!(self.compaction, Some(CompactionPhase::Running { .. })) {
            self.compaction = Some(CompactionPhase::Idle);
        }
        // kimi normalizeAfterReplay: an interrupted goal never silently
        // resumes driving turns — it comes back paused until an explicit
        // resume.
        if let Some(goal) = &mut self.goal
            && goal.status == GoalStatusKind::Active
        {
            goal.status = GoalStatusKind::Paused;
        }
        // Fail-closed lineage check: the declared imported region must be
        // fully covered by the stream.
        if self.imported_remaining > 0 {
            self.integrity_error = Some(ReplayError::ImportedRegionOverrun {
                declared: self.imported_declared,
                actual: self.imported_declared - self.imported_remaining,
            });
        }
    }
}

/// RFC3339 UTC at minute resolution (`YYYY-MM-DDTHH:MM:00Z`) — the exact
/// shape cron occurrence ids carry. Structural check only (length, fixed
/// punctuation, digits, zeroed seconds): this is the rejection gate for
/// malformed occurrence identities, not a calendar validator.
pub fn is_rfc3339_utc_minute(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 || !value.ends_with('Z') {
        return false;
    }
    let digit = |index: usize| bytes[index].is_ascii_digit();
    let punct = matches!(
        (bytes[4], bytes[7], bytes[10], bytes[13], bytes[16]),
        (b'-', b'-', b'T', b':', b':')
    );
    punct
        && (0..=1).all(&digit)
        && (2..=3).all(&digit)
        && (5..=6).all(&digit)
        && (8..=9).all(&digit)
        && (11..=12).all(&digit)
        && (14..=15).all(&digit)
        && &value[17..19] == "00"
}
