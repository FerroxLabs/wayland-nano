use std::path::{Path, PathBuf};

const DEFAULT_DEADLINE_MS: u64 = 600_000;
const MAX_DEADLINE_MS: u64 = 3_600_000;
const MAX_ESCALATION_MODELS: usize = 4;
const USAGE: &str = "usage: wayland-nano verify --requirement <id> [--gate <gate-id>] [--task <text>] [--budget N] --cheap-model <id> --escalation-model <id> [--escalation-model <id> ...] [--receipt-out <path>] [--deadline-ms N] [--json] | verify --verify-receipt <path> [--json]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyMode {
    Mint {
        requirement: String,
        gate: Option<String>,
        task: Option<String>,
        budget: Option<u32>,
        cheap_model: String,
        escalation_models: Vec<String>,
        deadline_ms: u64,
        receipt_out: Option<PathBuf>,
        json: bool,
    },
    CheckReceipt {
        path: PathBuf,
        json: bool,
    },
    RunOnly {
        gate: String,
        deadline_ms: u64,
        json: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyParams {
    pub mode: VerifyMode,
}

#[derive(Default)]
struct ParsedArgs {
    requirement: Option<String>,
    gate: Option<String>,
    task: Option<String>,
    budget: Option<u32>,
    cheap_model: Option<String>,
    escalation_models: Vec<String>,
    deadline_ms: Option<u64>,
    receipt_out: Option<PathBuf>,
    verify_receipt: Option<PathBuf>,
    run_only: bool,
    json: bool,
}

pub fn parse_args(args: &[String]) -> Result<VerifyParams, i32> {
    parse_args_inner(args).map_err(|()| {
        eprintln!("{USAGE}");
        2
    })
}

fn parse_args_inner(args: &[String]) -> Result<VerifyParams, ()> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while let Some(flag) = args.get(index) {
        let take = |index: &mut usize| -> Result<String, ()> {
            *index += 1;
            let value = args.get(*index).ok_or(())?;
            if value.is_empty() {
                return Err(());
            }
            Ok(value.clone())
        };
        match flag.as_str() {
            "--requirement" if parsed.requirement.is_none() => {
                parsed.requirement = Some(take(&mut index)?);
            }
            "--gate" if parsed.gate.is_none() => parsed.gate = Some(take(&mut index)?),
            "--task" if parsed.task.is_none() => parsed.task = Some(take(&mut index)?),
            "--budget" if parsed.budget.is_none() => {
                let value = take(&mut index)?.parse::<u32>().map_err(|_| ())?;
                if value == 0 {
                    return Err(());
                }
                parsed.budget = Some(value);
            }
            "--cheap-model" if parsed.cheap_model.is_none() => {
                parsed.cheap_model = Some(take(&mut index)?);
            }
            "--escalation-model" => {
                let value = take(&mut index)?;
                if parsed.escalation_models.len() == MAX_ESCALATION_MODELS
                    || parsed.escalation_models.contains(&value)
                {
                    return Err(());
                }
                parsed.escalation_models.push(value);
            }
            "--deadline-ms" if parsed.deadline_ms.is_none() => {
                let value = take(&mut index)?.parse::<u64>().map_err(|_| ())?;
                if value == 0 || value > MAX_DEADLINE_MS {
                    return Err(());
                }
                parsed.deadline_ms = Some(value);
            }
            "--receipt-out" if parsed.receipt_out.is_none() => {
                parsed.receipt_out = Some(PathBuf::from(take(&mut index)?));
            }
            "--verify-receipt" if parsed.verify_receipt.is_none() => {
                parsed.verify_receipt = Some(PathBuf::from(take(&mut index)?));
            }
            "--run-only" if !parsed.run_only => parsed.run_only = true,
            "--json" if !parsed.json => parsed.json = true,
            _ => return Err(()),
        }
        index += 1;
    }

    if let Some(path) = parsed.verify_receipt {
        if parsed.requirement.is_some()
            || parsed.gate.is_some()
            || parsed.task.is_some()
            || parsed.budget.is_some()
            || parsed.cheap_model.is_some()
            || !parsed.escalation_models.is_empty()
            || parsed.deadline_ms.is_some()
            || parsed.receipt_out.is_some()
            || parsed.run_only
        {
            return Err(());
        }
        return Ok(VerifyParams {
            mode: VerifyMode::CheckReceipt {
                path,
                json: parsed.json,
            },
        });
    }

    let deadline_ms = parsed.deadline_ms.unwrap_or(DEFAULT_DEADLINE_MS);
    if parsed.run_only {
        if parsed.requirement.is_some()
            || parsed.task.is_some()
            || parsed.budget.is_some()
            || parsed.cheap_model.is_some()
            || !parsed.escalation_models.is_empty()
            || parsed.receipt_out.is_some()
        {
            return Err(());
        }
        return Ok(VerifyParams {
            mode: VerifyMode::RunOnly {
                gate: parsed.gate.ok_or(())?,
                deadline_ms,
                json: parsed.json,
            },
        });
    }

    Ok(VerifyParams {
        mode: VerifyMode::Mint {
            requirement: parsed.requirement.ok_or(())?,
            gate: parsed.gate,
            task: parsed.task,
            budget: parsed.budget,
            cheap_model: parsed.cheap_model.ok_or(())?,
            escalation_models: if parsed.escalation_models.is_empty() {
                return Err(());
            } else {
                parsed.escalation_models
            },
            deadline_ms,
            receipt_out: parsed.receipt_out,
            json: parsed.json,
        },
    })
}

/// The closed production entry is wired by later WP-3 plans. Until then every
/// parsed request fails as usage rather than performing partial effects.
pub async fn run(_home: &Path, _workspace: &Path, _params: &VerifyParams) -> i32 {
    2
}

#[cfg(test)]
mod tests {
    use super::{VerifyMode, parse_args};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn landed_contract_import_probe() {
        use nano_verify::{
            ArtifactWorkspace, BaselineGateEvidence, BaselineGateExecution, CandidateArtifact,
            CheckVerdict, ClimbConfig, Effects, ExpectedChangeManifest, FailingRun, GateClosure,
            GateEvidence, GateExecution, GateInvocation, GateRegistry, LogEntry, Receipt,
            RunDeadline, TerminalState, VerifyVerdict, canonical_receipt,
            create_artifact_workspace, derive_expected_changes, gate_for_requirement,
            load_registry, mint_receipt, parse_candidate_diff, preflight_receipt, read_receipt,
            run_climb, run_gate_baseline_execution, run_gate_execution, write_receipt,
        };
        let imported_types = [
            std::any::type_name::<Receipt>(), std::any::type_name::<CheckVerdict>(),
            std::any::type_name::<VerifyVerdict>(), std::any::type_name::<TerminalState>(),
            std::any::type_name::<LogEntry>(), std::any::type_name::<RunDeadline>(),
            std::any::type_name::<ClimbConfig>(), std::any::type_name::<GateClosure>(),
            std::any::type_name::<GateRegistry>(), std::any::type_name::<GateInvocation>(),
            std::any::type_name::<GateExecution>(), std::any::type_name::<GateEvidence>(),
            std::any::type_name::<BaselineGateExecution>(),
            std::any::type_name::<BaselineGateEvidence>(),
            std::any::type_name::<CandidateArtifact>(), std::any::type_name::<ArtifactWorkspace>(),
            std::any::type_name::<ExpectedChangeManifest>(), std::any::type_name::<FailingRun>(),
        ];
        assert!(imported_types.iter().all(|name| name.starts_with("nano_verify::")));
        let _ = (
            canonical_receipt, create_artifact_workspace, derive_expected_changes,
            gate_for_requirement, load_registry, mint_receipt, parse_candidate_diff,
            preflight_receipt, read_receipt, run_climb::<ProbeEffects>,
            run_gate_baseline_execution, run_gate_execution, write_receipt,
        );

        struct ProbeEffects;
        impl Effects for ProbeEffects {
            async fn generate(&self, _: &str, _: &str) -> Result<String, nano_verify::VerifyError> { unreachable!() }
            fn emit_event(&self, _: nano_verify::EngineEvent) {}
            fn now_millis(&self) -> u64 { 0 }
            fn cancellation_requested(&self) -> bool { false }
        }
    }

    #[test]
    fn parse_accepts_all_three_closed_modes_and_defaults() {
        let mint = parse_args(&args(&[
            "--requirement", "CLI-01", "--cheap-model", "cheap", "--escalation-model",
            "strong",
        ]))
        .unwrap();
        assert!(matches!(mint.mode, VerifyMode::Mint { budget: None, deadline_ms: 600_000, json: false, .. }));

        let check = parse_args(&args(&["--verify-receipt", "receipt.json", "--json"])).unwrap();
        assert!(matches!(check.mode, VerifyMode::CheckReceipt { json: true, .. }));

        let run = parse_args(&args(&[
            "--gate", "demo", "--run-only", "--deadline-ms", "42", "--json",
        ]))
        .unwrap();
        assert!(matches!(run.mode, VerifyMode::RunOnly { deadline_ms: 42, json: true, .. }));
    }

    #[test]
    fn parse_preserves_mint_values_and_escalation_order() {
        let parsed = parse_args(&args(&[
            "--requirement", "CLI-01", "--gate", "demo", "--task", "repair", "--budget",
            "7", "--cheap-model", "cheap", "--escalation-model", "one",
            "--escalation-model", "two", "--escalation-model", "three",
            "--escalation-model", "four", "--deadline-ms", "3600000", "--receipt-out",
            "out.json", "--json",
        ]))
        .unwrap();
        let VerifyMode::Mint { requirement, gate, task, budget, cheap_model, escalation_models, deadline_ms, receipt_out, json } = parsed.mode else { panic!("mint") };
        assert_eq!(requirement, "CLI-01");
        assert_eq!(gate.as_deref(), Some("demo"));
        assert_eq!(task.as_deref(), Some("repair"));
        assert_eq!(budget, Some(7));
        assert_eq!(cheap_model, "cheap");
        assert_eq!(escalation_models, ["one", "two", "three", "four"]);
        assert_eq!(deadline_ms, 3_600_000);
        assert_eq!(receipt_out.unwrap().to_string_lossy(), "out.json");
        assert!(json);
    }

    #[test]
    fn parse_rejects_missing_values_unknowns_and_duplicate_singletons() {
        for bad in [
            vec![], vec!["--unknown"], vec!["positional"], vec!["--requirement"],
            vec!["--requirement", ""], vec!["--requirement", "R", "--requirement", "R2"],
            vec!["--gate", "g", "--gate", "h", "--run-only"],
            vec!["--task", "a", "--task", "b"], vec!["--budget", "1", "--budget", "2"],
            vec!["--cheap-model", "a", "--cheap-model", "b"],
            vec!["--receipt-out", "a", "--receipt-out", "b"],
            vec!["--verify-receipt", "a", "--verify-receipt", "b"],
            vec!["--run-only", "--run-only"], vec!["--json", "--json"],
        ] {
            assert_eq!(parse_args(&args(&bad)), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn parse_rejects_invalid_budget_deadline_and_ladder_values() {
        for bad in [
            vec!["--requirement", "R", "--budget", "0", "--cheap-model", "c", "--escalation-model", "e"],
            vec!["--requirement", "R", "--budget", "4294967296", "--cheap-model", "c", "--escalation-model", "e"],
            vec!["--requirement", "R", "--deadline-ms", "0", "--cheap-model", "c", "--escalation-model", "e"],
            vec!["--requirement", "R", "--deadline-ms", "3600001", "--cheap-model", "c", "--escalation-model", "e"],
            vec!["--requirement", "R", "--deadline-ms", "18446744073709551616", "--cheap-model", "c", "--escalation-model", "e"],
            vec!["--requirement", "R", "--deadline-ms", "1", "--deadline-ms", "2", "--cheap-model", "c", "--escalation-model", "e"],
            vec!["--requirement", "R", "--cheap-model", "", "--escalation-model", "e"],
            vec!["--requirement", "R", "--cheap-model", "c", "--escalation-model", ""],
            vec!["--requirement", "R", "--cheap-model", "c", "--escalation-model", "e", "--escalation-model", "e"],
            vec!["--requirement", "R", "--cheap-model", "c", "--escalation-model", "1", "--escalation-model", "2", "--escalation-model", "3", "--escalation-model", "4", "--escalation-model", "5"],
        ] {
            assert_eq!(parse_args(&args(&bad)), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn parse_rejects_cross_mode_flags_and_incomplete_modes() {
        for bad in [
            vec!["--requirement", "R", "--cheap-model", "c"],
            vec!["--requirement", "R", "--escalation-model", "e"],
            vec!["--cheap-model", "c", "--escalation-model", "e"],
            vec!["--verify-receipt", "r", "--requirement", "R"],
            vec!["--verify-receipt", "r", "--gate", "g"],
            vec!["--verify-receipt", "r", "--task", "t"],
            vec!["--verify-receipt", "r", "--budget", "1"],
            vec!["--verify-receipt", "r", "--cheap-model", "c"],
            vec!["--verify-receipt", "r", "--escalation-model", "e"],
            vec!["--verify-receipt", "r", "--receipt-out", "o"],
            vec!["--verify-receipt", "r", "--deadline-ms", "1"],
            vec!["--verify-receipt", "r", "--run-only"],
            vec!["--run-only"], vec!["--gate", "g"],
            vec!["--gate", "g", "--run-only", "--requirement", "R"],
            vec!["--gate", "g", "--run-only", "--task", "t"],
            vec!["--gate", "g", "--run-only", "--budget", "1"],
            vec!["--gate", "g", "--run-only", "--cheap-model", "c"],
            vec!["--gate", "g", "--run-only", "--escalation-model", "e"],
            vec!["--gate", "g", "--run-only", "--receipt-out", "o"],
        ] {
            assert_eq!(parse_args(&args(&bad)), Err(2), "{bad:?}");
        }
    }
}
