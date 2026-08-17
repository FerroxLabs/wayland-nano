//! Gate invocation and fail-closed output parsing primitives.

/// One gate invocation. argv ONLY — no shell string ever reaches the OS
/// (gate-runner.cts:116-123). Artifact path is appended as the final argv token at
/// spawn time (gate-runner.cts:98,119); it is NOT part of the closure digest.
#[derive(Debug, Clone)]
pub struct GateInvocation {
    pub argv: Vec<std::ffi::OsString>,
    pub cwd: std::path::PathBuf, // materialized from CwdPolicy by the caller
    /// Declared env from the closure; spawn = env_clear + baseline allowlist + this.
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub timeout: std::time::Duration, // default 400s (gate-runner.cts:33-34)
    pub gate_id: String,              // registry key this invocation was built from
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailCategory {
    Structure,
    Value,
    Relation,
    Grounding,
    Execution,
    Security,
}

/// Per-check verdict for the FULL inventory — not just failures
/// (audit seam #1, spec-coldread-audit.md:263; WP3 assumption SPEC-WP3:12).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckVerdict {
    pub id: String,
    pub category: FailCategory,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailClosedReason {
    NoGateOutput,
    Timeout,
    SpawnError(String),
    InconsistentSummary { passed: u64, total: u64 },
    UnknownCheckId(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    Green { verdicts: Vec<CheckVerdict> },
    Red { verdicts: Vec<CheckVerdict> },
    FailClosed(FailClosedReason),
}

impl GateOutcome {
    pub fn score(&self) -> (i64, i64) {
        (0, 1)
    }

    pub fn fails(&self) -> Vec<String> {
        vec!["<gate parser unavailable>".to_owned()]
    }
}

/// PURE. stdout + the card's check inventory → outcome. Never panics.
pub fn parse_gate_output(_stdout: &str, _inventory: &[(String, FailCategory)]) -> GateOutcome {
    GateOutcome::FailClosed(FailClosedReason::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> Vec<(String, FailCategory)> {
        vec![
            ("TG-01".into(), FailCategory::Value),
            ("TG-02".into(), FailCategory::Execution),
            ("TG-03".into(), FailCategory::Structure),
            ("TG-04".into(), FailCategory::Security),
        ]
    }

    fn verdicts(failed: &[&str]) -> Vec<CheckVerdict> {
        inventory()
            .into_iter()
            .map(|(id, category)| CheckVerdict {
                passed: !failed.contains(&id.as_str()),
                id,
                category,
            })
            .collect()
    }

    #[test]
    fn parse_summary_last_match_wins() {
        let actual = parse_gate_output("gate: 4/4\nnoise\ngate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-02"])
            }
        );
        assert_eq!(actual.score(), (3, 4));
    }

    #[test]
    fn parse_prefixed_slug_summary() {
        let actual =
            parse_gate_output("gate :3/4\nFAIL TG-04 security\nmy-gate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-04"])
            }
        );
    }

    #[test]
    fn parse_no_summary_fails_closed() {
        assert_eq!(
            parse_gate_output("FAIL TG-03 structure", &inventory()),
            GateOutcome::FailClosed(FailClosedReason::NoGateOutput)
        );
    }

    #[test]
    fn parse_empty_stdout_fails_closed() {
        let actual = parse_gate_output(" \r\n\t", &inventory());
        assert_eq!(
            actual,
            GateOutcome::FailClosed(FailClosedReason::NoGateOutput)
        );
        assert_eq!(actual.score(), (0, 1));
        assert_eq!(actual.fails(), vec!["<no gate output>"]);
    }

    #[test]
    fn parse_fail_v2_canonical() {
        let actual = parse_gate_output("FAIL TG-03 structure\ngate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-03"])
            }
        );
        assert_eq!(actual.fails(), vec!["TG-03 structure"]);
    }

    #[test]
    fn parse_fail_v2_whitespace_collapses() {
        let actual = parse_gate_output("FAIL\t TG-03   structure  \ngate: 3 / 4", &inventory());
        assert_eq!(actual.fails(), vec!["TG-03 structure"]);
    }

    #[test]
    fn parse_unknown_fail_id_fails_closed() {
        let actual = parse_gate_output("FAIL ZZ-99 value\ngate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::FailClosed(FailClosedReason::UnknownCheckId("ZZ-99".into()))
        );
        assert_eq!(actual.fails(), vec!["<unknown check id>"]);
    }

    #[test]
    fn parse_reconstructs_full_verdict_inventory() {
        let green = parse_gate_output("gate: 4/4", &inventory());
        assert_eq!(
            green,
            GateOutcome::Green {
                verdicts: verdicts(&[])
            }
        );
        assert_eq!(green.score(), (4, 4));
        assert!(green.fails().is_empty());

        let red = parse_gate_output(
            "FAIL TG-01 value\nFAIL TG-04 security\ngate: 2/4",
            &inventory(),
        );
        assert_eq!(
            red,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-01", "TG-04"])
            }
        );
        assert_eq!(red.score(), (2, 4));
        assert_eq!(red.fails(), vec!["TG-01 value", "TG-04 security"]);
    }

    #[test]
    fn summary_inventory_mismatch_fails_closed() {
        assert_eq!(
            parse_gate_output("gate: 4/5", &inventory()),
            GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                passed: 4,
                total: 5
            })
        );
        assert_eq!(
            parse_gate_output("FAIL TG-01 value\ngate: 4/4", &inventory()),
            GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                passed: 4,
                total: 4
            })
        );
        assert_eq!(
            parse_gate_output("gate: 0/0", &[]),
            GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                passed: 0,
                total: 0
            })
        );
    }
}
