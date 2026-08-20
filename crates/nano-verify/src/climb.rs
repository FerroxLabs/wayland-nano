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

pub fn better_candidate(_candidate: &Candidate, _best: Option<&Candidate>) -> bool {
    false
}
pub fn next_step(_state: &ClimbState) -> ClimbStep {
    ClimbStep::Stop {
        reason: StopReason::Error,
    }
}
pub fn apply_result(state: &ClimbState, _step: &ClimbStep, _results: &[StepResult]) -> ClimbState {
    state.clone()
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

impl ClimbOutcome {
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
    }
}
