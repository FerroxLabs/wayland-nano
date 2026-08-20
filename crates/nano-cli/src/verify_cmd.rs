#[cfg(test)]
mod tests {
    use super::{VerifyMode, parse_args};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
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
