//! Pure strict-ratchet decision core for gated climbs.

use std::collections::BTreeMap;

use crate::{CandidateArtifact, GateEvidence};

pub type ModelId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Probe,
    Ensemble,
    Surgical,
    Consolidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Cheap,
    Escalate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    Budget,
    Solved,
    Plateau,
    Exhausted,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogCode {
    Generated,
    GenerationFailed,
    Gated,
    Accepted,
    Rejected,
    PhaseChanged,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogEntry {
    pub phase: Phase,
    pub score: [i64; 2],
    pub accepted: bool,
    pub code: LogCode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TerminalState {
    Verified,
    CriteriaChecked,
    SelfChecked,
    NeedsEscalation,
    Blocked(String),
    Cancelled,
    TimedOut,
    PermissionDenied,
    CrashedRecovered,
    Superseded,
}

impl TerminalState {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunDeadline {
    pub monotonic_millis: u64,
}

#[derive(Debug, Clone)]
pub struct ClimbConfig {
    pub cheap: Vec<ModelId>,
    pub ladder: Vec<ModelId>,
    pub budget: u32,
    pub seed_n: u32,
    pub deadline: RunDeadline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub artifact: CandidateArtifact,
    pub score: (i64, i64),
    pub fails: Vec<String>,
    pub evidence: GateEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub model: ModelId,
    pub text: String,
    pub artifact: Option<CandidateArtifact>,
    pub score: (i64, i64),
    pub fails: Vec<String>,
    pub evidence: Option<GateEvidence>,
}

#[derive(Debug, Clone)]
pub struct ClimbState {
    pub cfg: ClimbConfig,
    pub calls: u32,
    pub phase: Phase,
    pub best: Option<Candidate>,
    pub tried: BTreeMap<String, Vec<ModelId>>,
    pub wins: BTreeMap<ModelId, u32>,
    pub consolidated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClimbStep {
    Probe {
        model: ModelId,
    },
    Ensemble {
        models: Vec<ModelId>,
    },
    Surgical {
        model: ModelId,
        target: String,
        others: Vec<String>,
        tier: Tier,
    },
    Consolidate {
        model: ModelId,
        fails: Vec<String>,
    },
    Stop {
        reason: StopReason,
    },
}

pub fn better_candidate(candidate: &Candidate, best: Option<&Candidate>) -> bool {
    let Some(best) = best else {
        return true;
    };
    if candidate.score.0 != best.score.0 {
        return candidate.score.0 > best.score.0;
    }
    let candidate_fails = canonical_failures(&candidate.fails);
    let best_fails = canonical_failures(&best.fails);
    candidate_fails.len() < best_fails.len()
        && candidate_fails
            .iter()
            .all(|failure| best_fails.contains(failure))
}
pub fn next_step(state: &ClimbState) -> ClimbStep {
    if state
        .best
        .as_ref()
        .is_some_and(|candidate| candidate.fails.is_empty())
    {
        return ClimbStep::Stop {
            reason: StopReason::Solved,
        };
    }
    if state.calls >= state.cfg.budget {
        return ClimbStep::Stop {
            reason: StopReason::Budget,
        };
    }
    match state.phase {
        Phase::Probe => state.cfg.cheap.first().cloned().map_or(
            ClimbStep::Stop {
                reason: StopReason::Exhausted,
            },
            |model| ClimbStep::Probe { model },
        ),
        Phase::Ensemble => {
            let remaining = state.cfg.budget.saturating_sub(state.calls) as usize;
            let width = usize::try_from(state.cfg.seed_n).unwrap_or(usize::MAX);
            ClimbStep::Ensemble {
                models: state
                    .cfg
                    .cheap
                    .iter()
                    .skip(1)
                    .take(width.saturating_sub(1))
                    .take(remaining)
                    .cloned()
                    .collect(),
            }
        }
        Phase::Surgical | Phase::Consolidation => next_surgical_step(state),
    }
}
pub fn apply_result(state: &ClimbState, step: &ClimbStep, results: &[StepResult]) -> ClimbState {
    let mut next = state.clone();
    next.calls = next
        .calls
        .saturating_add(u32::try_from(results.len()).unwrap_or(u32::MAX));
    match step {
        ClimbStep::Probe { .. } => next.phase = Phase::Ensemble,
        ClimbStep::Ensemble { .. } => next.phase = Phase::Surgical,
        ClimbStep::Surgical { model, target, .. } => {
            let tried = next.tried.entry(target.clone()).or_default();
            if !tried.contains(model) {
                tried.push(model.clone());
            }
        }
        ClimbStep::Consolidate { .. } => next.consolidated = true,
        ClimbStep::Stop { .. } => return next,
    }
    for result in results {
        let Some(candidate) = candidate_from_result(result) else {
            continue;
        };
        if better_candidate(&candidate, next.best.as_ref()) {
            *next.wins.entry(result.model.clone()).or_default() += 1;
            let accepted_fails = canonical_failures(&candidate.fails);
            next.best = Some(candidate);
            if matches!(step, ClimbStep::Consolidate { .. }) {
                next.tried.clear();
            } else {
                next.tried
                    .retain(|failure, _| accepted_fails.contains(failure));
            }
        }
    }
    next
}

fn canonical_failures(fails: &[String]) -> std::collections::BTreeSet<String> {
    fails.iter().cloned().collect()
}

fn candidate_from_result(result: &StepResult) -> Option<Candidate> {
    Some(Candidate {
        text: result.text.clone(),
        artifact: result.artifact.clone()?,
        score: result.score,
        fails: deduplicate_failures(&result.fails),
        evidence: result.evidence.clone()?,
    })
}

fn deduplicate_failures(fails: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    fails
        .iter()
        .filter(|failure| seen.insert((*failure).clone()))
        .cloned()
        .collect()
}

fn next_surgical_step(state: &ClimbState) -> ClimbStep {
    let Some(best) = state.best.as_ref() else {
        return ClimbStep::Stop {
            reason: StopReason::Exhausted,
        };
    };
    for target in &best.fails {
        let tried = state
            .tried
            .get(target)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut cheap: Vec<(usize, &ModelId)> = state
            .cfg
            .cheap
            .iter()
            .enumerate()
            .filter(|(_, model)| !tried.contains(model))
            .collect();
        cheap.sort_by_key(|(order, model)| {
            (
                std::cmp::Reverse(state.wins.get(*model).copied().unwrap_or(0)),
                *order,
            )
        });
        if let Some((_, model)) = cheap.first() {
            return ClimbStep::Surgical {
                model: (*model).clone(),
                target: target.clone(),
                others: best
                    .fails
                    .iter()
                    .filter(|failure| *failure != target)
                    .take(8)
                    .cloned()
                    .collect(),
                tier: Tier::Cheap,
            };
        }
        if let Some(model) = state.cfg.ladder.iter().find(|model| !tried.contains(model)) {
            return ClimbStep::Surgical {
                model: model.clone(),
                target: target.clone(),
                others: best
                    .fails
                    .iter()
                    .filter(|failure| *failure != target)
                    .take(8)
                    .cloned()
                    .collect(),
                tier: Tier::Escalate,
            };
        }
    }
    if state.consolidated {
        return ClimbStep::Stop {
            reason: StopReason::Plateau,
        };
    }
    let Some((_, model)) = state
        .cfg
        .cheap
        .iter()
        .enumerate()
        .max_by_key(|(order, model)| {
            (
                state.wins.get(*model).copied().unwrap_or(0),
                std::cmp::Reverse(*order),
            )
        })
    else {
        return ClimbStep::Stop {
            reason: StopReason::Exhausted,
        };
    };
    ClimbStep::Consolidate {
        model: model.clone(),
        fails: best.fails.iter().take(10).cloned().collect(),
    }
}

#[derive(Clone, serde::Serialize)]
pub struct ClimbOutcome {
    terminal: TerminalState,
    score: [i64; 2],
    fails: Vec<String>,
    rounds_used: u32,
    escalated: bool,
    stop_reason: StopReason,
    cost_usd: Option<f64>,
    log: Vec<LogEntry>,
    #[serde(skip)]
    accepted_artifact: Option<CandidateArtifact>,
    #[serde(skip)]
    _seal: OutcomeSeal,
}

#[derive(Debug, Clone, PartialEq)]
struct OutcomeSeal;

impl std::fmt::Debug for ClimbOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClimbOutcome")
            .field("terminal", &self.terminal)
            .field("score", &self.score)
            .field("fails", &self.fails)
            .field("rounds_used", &self.rounds_used)
            .field("escalated", &self.escalated)
            .field("stop_reason", &self.stop_reason)
            .field("cost_usd", &self.cost_usd)
            .field("log", &self.log)
            .field("accepted_artifact", &self.accepted_artifact.is_some())
            .finish()
    }
}

impl PartialEq for ClimbOutcome {
    fn eq(&self, other: &Self) -> bool {
        self.terminal == other.terminal
            && self.score == other.score
            && self.fails == other.fails
            && self.rounds_used == other.rounds_used
            && self.escalated == other.escalated
            && self.stop_reason == other.stop_reason
            && self.cost_usd == other.cost_usd
            && self.log == other.log
            && self.accepted_artifact == other.accepted_artifact
    }
}

impl ClimbOutcome {
    /// The sole sealed construction path for trusted crate-internal drivers.
    ///
    /// Semantic result fields and the accepted artifact are derived from one immutable
    /// state snapshot so an artifact cannot be paired with another candidate's score,
    /// failures, or call count.
    #[allow(dead_code, reason = "consumed by the Plan 02 trusted driver")]
    pub(crate) fn from_state(
        state: &ClimbState,
        terminal: TerminalState,
        stop_reason: StopReason,
        escalated: bool,
        cost_usd: Option<f64>,
        log: Vec<LogEntry>,
    ) -> Self {
        let (score, fails, accepted_artifact) = state.best.as_ref().map_or_else(
            || ([0, 0], Vec::new(), None),
            |best| {
                (
                    [best.score.0, best.score.1],
                    best.fails.clone(),
                    Some(best.artifact.clone()),
                )
            },
        );
        Self {
            terminal,
            score,
            fails,
            rounds_used: state.calls,
            escalated,
            stop_reason,
            cost_usd,
            log,
            accepted_artifact,
            _seal: OutcomeSeal,
        }
    }

    pub fn terminal(&self) -> &TerminalState {
        &self.terminal
    }
    pub fn score(&self) -> [i64; 2] {
        self.score
    }
    pub fn fails(&self) -> &[String] {
        &self.fails
    }
    pub fn rounds_used(&self) -> u32 {
        self.rounds_used
    }
    pub fn escalated(&self) -> bool {
        self.escalated
    }
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }
    pub fn cost_usd(&self) -> Option<f64> {
        self.cost_usd
    }
    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }
    pub fn accepted_artifact(&self) -> Option<&CandidateArtifact> {
        self.accepted_artifact.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, score: (i64, i64), fails: &[&str]) -> Candidate {
        Candidate {
            text: id.into(),
            artifact: CandidateArtifact::inert(id),
            score,
            fails: fails.iter().map(|s| (*s).into()).collect(),
            evidence: GateEvidence {
                exit_code: Some(0),
                log_digest: Some(format!("log-{id}")),
                artifact_sha256: format!("sha-{id}"),
            },
        }
    }

    fn config(budget: u32) -> ClimbConfig {
        ClimbConfig {
            cheap: vec!["c0".into(), "c1".into()],
            ladder: vec!["l0".into()],
            budget,
            seed_n: 3,
            deadline: RunDeadline {
                monotonic_millis: 1,
            },
        }
    }

    #[test]
    fn ratchet_accepts_strict_score_win_and_strict_subset_only() {
        let best = candidate("best", (2, 4), &["A", "B"]);
        assert!(better_candidate(
            &candidate("score", (3, 4), &["A", "B", "C"]),
            Some(&best)
        ));
        assert!(better_candidate(
            &candidate("subset", (2, 4), &["A"]),
            Some(&best)
        ));
        assert!(!better_candidate(
            &candidate("swap", (2, 4), &["C", "D"]),
            Some(&best)
        ));
        assert!(!better_candidate(
            &candidate("lower", (1, 4), &["A"]),
            Some(&best)
        ));
        assert!(!better_candidate(
            &candidate("equal", (2, 4), &["A", "B"]),
            Some(&best)
        ));
        assert!(!better_candidate(
            &candidate("uncontained", (2, 4), &["C"]),
            Some(&best)
        ));
    }

    #[test]
    fn budget_exhaustion_stops() {
        let state = ClimbState {
            cfg: config(3),
            calls: 3,
            phase: Phase::Surgical,
            best: Some(candidate("best", (1, 2), &["A"])),
            tried: BTreeMap::new(),
            wins: BTreeMap::new(),
            consolidated: false,
        };
        assert_eq!(
            next_step(&state),
            ClimbStep::Stop {
                reason: StopReason::Budget
            }
        );
        let outcome = ClimbOutcome::from_state(
            &state,
            TerminalState::NeedsEscalation,
            StopReason::Budget,
            false,
            None,
            Vec::new(),
        );
        assert_eq!(outcome.terminal(), &TerminalState::NeedsEscalation);
        assert_eq!(outcome.score(), [1, 2]);
        assert_eq!(outcome.fails(), &["A"]);
        assert_eq!(outcome.rounds_used(), 3);
        assert_eq!(outcome.stop_reason(), StopReason::Budget);
        assert_eq!(
            outcome.accepted_artifact(),
            state.best.as_ref().map(|best| &best.artifact)
        );

        let solved_on_last_call = ClimbState {
            cfg: config(1),
            calls: 1,
            phase: Phase::Ensemble,
            best: Some(candidate("green", (2, 2), &[])),
            tried: BTreeMap::new(),
            wins: BTreeMap::new(),
            consolidated: false,
        };
        assert_eq!(
            next_step(&solved_on_last_call),
            ClimbStep::Stop {
                reason: StopReason::Solved
            }
        );

        let attempted = ClimbState {
            cfg: config(4),
            calls: 0,
            phase: Phase::Surgical,
            best: Some(candidate("best", (1, 2), &["A"])),
            tried: BTreeMap::new(),
            wins: BTreeMap::new(),
            consolidated: false,
        };
        let generation_failure = ClimbStep::Surgical {
            model: "c0".into(),
            target: "A".into(),
            others: Vec::new(),
            tier: Tier::Cheap,
        };
        let attempted = apply_result(
            &attempted,
            &generation_failure,
            &[result("c0", None, (0, 2), &[])],
        );
        assert_eq!(attempted.calls, 1);
        assert!(attempted.wins.is_empty());
        let rejected = apply_result(
            &attempted,
            &ClimbStep::Surgical {
                model: "c1".into(),
                target: "A".into(),
                others: Vec::new(),
                tier: Tier::Cheap,
            },
            &[result("c1", Some("reject"), (0, 2), &[])],
        );
        assert_eq!(rejected.calls, 2);
        assert!(rejected.wins.is_empty());
        assert_eq!(rejected.best.as_ref().unwrap().text, "best");
    }

    fn result(model: &str, id: Option<&str>, score: (i64, i64), fails: &[&str]) -> StepResult {
        StepResult {
            model: model.into(),
            text: id.unwrap_or_default().into(),
            artifact: id.map(CandidateArtifact::inert),
            score,
            fails: fails.iter().map(|failure| (*failure).into()).collect(),
            evidence: id.map(|id| GateEvidence {
                exit_code: Some(0),
                log_digest: Some(format!("log-{id}")),
                artifact_sha256: format!("sha-{id}"),
            }),
        }
    }

    #[test]
    fn probe_ensemble_surgical_consolidate_path() {
        let mut cfg = config(20);
        cfg.cheap.push("c2".into());
        cfg.ladder.push("l1".into());
        let mut state = ClimbState {
            cfg,
            calls: 0,
            phase: Phase::Probe,
            best: None,
            tried: BTreeMap::new(),
            wins: BTreeMap::new(),
            consolidated: false,
        };

        let probe = next_step(&state);
        assert_eq!(probe, ClimbStep::Probe { model: "c0".into() });
        state = apply_result(
            &state,
            &probe,
            &[result("c0", Some("probe"), (1, 4), &["A", "B", "C"])],
        );
        let ensemble = next_step(&state);
        assert_eq!(
            ensemble,
            ClimbStep::Ensemble {
                models: vec!["c1".into(), "c2".into()]
            }
        );
        state = apply_result(
            &state,
            &ensemble,
            &[
                result("c1", None, (0, 4), &[]),
                result("c2", Some("ensemble"), (2, 4), &["A", "B"]),
            ],
        );
        assert_eq!(state.calls, 3);
        assert_eq!(state.best.as_ref().unwrap().text, "ensemble");
        assert_eq!(
            state.best.as_ref().unwrap().evidence.artifact_sha256,
            "sha-ensemble"
        );

        state.wins.insert("c1".into(), 4);
        let cheap = next_step(&state);
        assert_eq!(
            cheap,
            ClimbStep::Surgical {
                model: "c1".into(),
                target: "A".into(),
                others: vec!["B".into()],
                tier: Tier::Cheap
            }
        );
        state = apply_result(&state, &cheap, &[result("c1", None, (0, 4), &[])]);
        assert_eq!(state.calls, 4);
        assert_eq!(state.tried["A"], vec!["c1"]);

        state
            .tried
            .insert("A".into(), vec!["c0".into(), "c1".into(), "c2".into()]);
        state.tried.insert("B".into(), vec!["c2".into()]);
        state.wins.insert("l1".into(), 99);
        let ladder = next_step(&state);
        assert_eq!(
            ladder,
            ClimbStep::Surgical {
                model: "l0".into(),
                target: "A".into(),
                others: vec!["B".into()],
                tier: Tier::Escalate
            }
        );
        state = apply_result(
            &state,
            &ladder,
            &[result("l0", Some("surgical"), (2, 4), &["B"])],
        );
        assert_eq!(state.best.as_ref().unwrap().text, "surgical");
        assert!(!state.tried.contains_key("A"));
        assert_eq!(state.tried["B"], vec!["c2"]);

        state.tried.insert(
            "B".into(),
            vec![
                "c0".into(),
                "c1".into(),
                "c2".into(),
                "l0".into(),
                "l1".into(),
            ],
        );
        let consolidate = next_step(&state);
        assert_eq!(
            consolidate,
            ClimbStep::Consolidate {
                model: "c1".into(),
                fails: vec!["B".into()]
            }
        );
        let winner = result("c1", Some("winner"), (3, 4), &[]);
        let winner_artifact = winner.artifact.clone().unwrap();
        let winner_evidence = winner.evidence.clone().unwrap();
        state = apply_result(&state, &consolidate, &[winner]);
        assert!(state.tried.is_empty());
        assert_eq!(state.best.as_ref().unwrap().artifact, winner_artifact);
        assert_eq!(state.best.as_ref().unwrap().evidence, winner_evidence);
        assert_eq!(state.best.as_ref().unwrap().text, "winner");
        assert_eq!(
            next_step(&state),
            ClimbStep::Stop {
                reason: StopReason::Solved
            }
        );

        let mut plateau = state.clone();
        plateau.best = Some(candidate("red", (2, 4), &["B"]));
        plateau.calls = 6;
        plateau.consolidated = true;
        plateau.tried.insert(
            "B".into(),
            vec![
                "c0".into(),
                "c1".into(),
                "c2".into(),
                "l0".into(),
                "l1".into(),
            ],
        );
        assert_eq!(
            next_step(&plateau),
            ClimbStep::Stop {
                reason: StopReason::Plateau
            }
        );

        let mut truncated = plateau;
        truncated.phase = Phase::Ensemble;
        truncated.calls = 19;
        assert_eq!(
            next_step(&truncated),
            ClimbStep::Ensemble {
                models: vec!["c1".into()]
            }
        );

        let equal_wins = ClimbState {
            cfg: ClimbConfig {
                cheap: vec!["z-caller-first".into(), "a-caller-second".into()],
                ladder: Vec::new(),
                budget: 4,
                seed_n: 2,
                deadline: RunDeadline {
                    monotonic_millis: 1,
                },
            },
            calls: 1,
            phase: Phase::Surgical,
            best: Some(candidate("tie", (1, 2), &["T"])),
            tried: BTreeMap::new(),
            wins: BTreeMap::from([("z-caller-first".into(), 2), ("a-caller-second".into(), 2)]),
            consolidated: false,
        };
        assert_eq!(
            next_step(&equal_wins),
            ClimbStep::Surgical {
                model: "z-caller-first".into(),
                target: "T".into(),
                others: Vec::new(),
                tier: Tier::Cheap,
            }
        );

        let ordered = apply_result(
            &ClimbState {
                cfg: config(12),
                calls: 0,
                phase: Phase::Probe,
                best: None,
                tried: BTreeMap::new(),
                wins: BTreeMap::new(),
                consolidated: false,
            },
            &ClimbStep::Probe { model: "c0".into() },
            &[result("c0", Some("ordered"), (1, 3), &["Z", "A", "Z"])],
        );
        assert_eq!(ordered.best.as_ref().unwrap().fails, vec!["Z", "A"]);
    }
}
