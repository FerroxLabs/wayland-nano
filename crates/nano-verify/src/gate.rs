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
        match self {
            Self::Green { verdicts } | Self::Red { verdicts } => {
                let passed = verdicts.iter().filter(|verdict| verdict.passed).count();
                (
                    i64::try_from(passed).unwrap_or(i64::MAX),
                    i64::try_from(verdicts.len()).unwrap_or(i64::MAX),
                )
            }
            Self::FailClosed(_) => (0, 1),
        }
    }

    pub fn fails(&self) -> Vec<String> {
        match self {
            Self::Green { .. } => Vec::new(),
            Self::Red { verdicts } => verdicts
                .iter()
                .filter(|verdict| !verdict.passed)
                .map(|verdict| format!("{} {}", verdict.id, verdict.category.as_str()))
                .collect(),
            Self::FailClosed(reason) => vec![reason.sentinel().to_owned()],
        }
    }
}

/// PURE. stdout + the card's check inventory → outcome. Never panics.
pub fn parse_gate_output(stdout: &str, inventory: &[(String, FailCategory)]) -> GateOutcome {
    let mut summary = None;
    let mut failures: Vec<(&str, FailCategory)> = Vec::new();

    for line in stdout.lines() {
        if let Some(parsed) = scan_summary(line) {
            summary = Some(parsed);
        }
        if let Some(rest) = line.strip_prefix("FAIL ") {
            let mut fields = rest.split_whitespace();
            let id = fields.next().unwrap_or("");
            let Some(category) = fields.next().and_then(FailCategory::parse) else {
                return GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(bounded_id(id)));
            };
            if !valid_check_id(id) || fields.next().is_some() {
                return GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(bounded_id(id)));
            }
            let Some((_, declared_category)) = inventory.iter().find(|(known, _)| known == id)
            else {
                return GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(id.to_owned()));
            };
            if *declared_category != category
                || failures.iter().any(|(failed_id, _)| *failed_id == id)
            {
                let (passed, total) = summary.unwrap_or((0, 0));
                return GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                    passed,
                    total,
                });
            }
            failures.push((id, category));
        }
    }

    let Some((passed, total)) = summary else {
        return GateOutcome::FailClosed(FailClosedReason::NoGateOutput);
    };
    let inventory_total = u64::try_from(inventory.len()).unwrap_or(u64::MAX);
    let fail_count = u64::try_from(failures.len()).unwrap_or(u64::MAX);
    if inventory.is_empty()
        || total != inventory_total
        || passed != total.checked_sub(fail_count).unwrap_or(u64::MAX)
    {
        return GateOutcome::FailClosed(FailClosedReason::InconsistentSummary { passed, total });
    }

    let verdicts = inventory
        .iter()
        .map(|(id, category)| CheckVerdict {
            id: id.clone(),
            category: *category,
            passed: !failures.iter().any(|(failed_id, _)| failed_id == id),
        })
        .collect();
    if failures.is_empty() {
        GateOutcome::Green { verdicts }
    } else {
        GateOutcome::Red { verdicts }
    }
}

impl FailCategory {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "structure" => Some(Self::Structure),
            "value" => Some(Self::Value),
            "relation" => Some(Self::Relation),
            "grounding" => Some(Self::Grounding),
            "execution" => Some(Self::Execution),
            "security" => Some(Self::Security),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Value => "value",
            Self::Relation => "relation",
            Self::Grounding => "grounding",
            Self::Execution => "execution",
            Self::Security => "security",
        }
    }
}

impl FailClosedReason {
    const fn sentinel(&self) -> &'static str {
        match self {
            Self::NoGateOutput => "<no gate output>",
            Self::Timeout => "<gate timeout>",
            Self::SpawnError(_) => "<gate spawn error>",
            Self::InconsistentSummary { .. } => "<inconsistent gate summary>",
            Self::UnknownCheckId(_) => "<unknown check id>",
        }
    }
}

fn scan_summary(line: &str) -> Option<(u64, u64)> {
    let line = line.trim_start();
    let colon = line.find(':')?;
    let label = &line[..colon];
    if !label.ends_with("gate")
        || label.is_empty()
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }

    let mut rest = line[colon + 1..].trim_start();
    let passed_end = rest.bytes().take_while(u8::is_ascii_digit).count();
    if passed_end == 0 {
        return None;
    }
    let passed = rest[..passed_end].parse().ok()?;
    rest = rest[passed_end..].trim_start();
    rest = rest.strip_prefix('/')?.trim_start();
    let total_end = rest.bytes().take_while(u8::is_ascii_digit).count();
    if total_end == 0 {
        return None;
    }
    let total = rest[..total_end].parse().ok()?;
    Some((passed, total))
}

fn valid_check_id(id: &str) -> bool {
    let Some((prefix, digits)) = id.split_once('-') else {
        return false;
    };
    (2..=4).contains(&prefix.len())
        && prefix.bytes().all(|byte| byte.is_ascii_uppercase())
        && digits.len() == 2
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn bounded_id(id: &str) -> String {
    if valid_check_id(id) {
        id.to_owned()
    } else {
        "<malformed>".to_owned()
    }
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
        let actual = parse_gate_output("gate: 4/4\nFAIL TG-02 execution\ngate: 3/4", &inventory());
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
        let actual = parse_gate_output("FAIL   TG-03   structure  \ngate: 3 / 4", &inventory());
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
