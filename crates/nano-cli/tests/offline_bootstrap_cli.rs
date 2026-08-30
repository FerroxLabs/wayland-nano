use nano_cli::activation::run_admin_command;

fn key_args(home: &std::path::Path) -> Vec<String> {
    vec![
        "--admin-root-keyref".into(),
        home.join("admin.keyref").display().to_string(),
        "--recovery-root-keyref".into(),
        home.join("recovery.keyref").display().to_string(),
        "--receipt-signer-keyref".into(),
        home.join("receipt.keyref").display().to_string(),
        "--local-cli-keyref".into(),
        home.join("cli.keyref").display().to_string(),
    ]
}

fn assert_zero_state(home: &std::path::Path) {
    assert!(!home.join("activation/authority.jsonl").exists());
    assert!(!home.join("activation/offline-bootstrap-v1.jsonl").exists());
}

#[test]
fn challenge_requires_only_literal_complete_argv_before_state() {
    let home = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    let mut args = vec!["offline-bootstrap-challenge".into()];
    args.extend(key_args(home.path()));
    args.extend(["--output".into(), "relative.json".into()]);
    assert_eq!(run_admin_command(home.path(), &args, &mut output), 2);
    assert!(output.is_empty());
    assert_zero_state(home.path());

    let mut args = vec!["offline-bootstrap-challenge".into()];
    args.extend(key_args(home.path()));
    args.extend([
        "--authorization".into(),
        home.path().join("authorization.json").display().to_string(),
    ]);
    assert_eq!(run_admin_command(home.path(), &args, &mut output), 2);
    assert_zero_state(home.path());
}

#[test]
fn apply_rejects_missing_duplicate_and_relative_inputs_before_state() {
    let home = tempfile::tempdir().unwrap();
    let mut output = Vec::new();

    let mut missing = vec!["offline-bootstrap-apply".into()];
    missing.extend(key_args(home.path()));
    assert_eq!(run_admin_command(home.path(), &missing, &mut output), 2);
    assert_zero_state(home.path());

    let mut relative = vec!["offline-bootstrap-apply".into()];
    relative.extend(key_args(home.path()));
    relative.extend(["--authorization".into(), "authorization.json".into()]);
    assert_eq!(run_admin_command(home.path(), &relative, &mut output), 2);
    assert_zero_state(home.path());

    let authorization = home.path().join("authorization.json");
    let mut unavailable = vec!["offline-bootstrap-apply".into()];
    unavailable.extend(key_args(home.path()));
    unavailable.extend([
        "--authorization".into(),
        authorization.display().to_string(),
    ]);
    assert_eq!(run_admin_command(home.path(), &unavailable, &mut output), 2);
    assert_zero_state(home.path());

    let mut duplicate = vec!["offline-bootstrap-apply".into()];
    duplicate.extend(key_args(home.path()));
    duplicate.extend([
        "--authorization".into(),
        authorization.display().to_string(),
        "--authorization".into(),
        authorization.display().to_string(),
    ]);
    assert_eq!(run_admin_command(home.path(), &duplicate, &mut output), 2);
    assert!(output.is_empty());
    assert_zero_state(home.path());
}

#[test]
fn offline_commands_reject_caller_authority_and_fallback_flags() {
    let home = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    for forbidden in ["--admin-id", "--confirm", "--request", "--stdin"] {
        let mut args = vec!["offline-bootstrap-challenge".into()];
        args.extend(key_args(home.path()));
        args.extend([
            "--output".into(),
            home.path().join("challenge.json").display().to_string(),
            forbidden.into(),
            "phase2-owner".into(),
        ]);
        assert_eq!(run_admin_command(home.path(), &args, &mut output), 2);
        assert_zero_state(home.path());
    }
}

#[test]
fn interactive_bootstrap_command_is_not_reinterpreted_as_offline() {
    let home = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    let mut args = vec![
        "bootstrap".into(),
        "--admin-id".into(),
        "phase2-owner".into(),
    ];
    args.extend(key_args(home.path()));
    assert_eq!(run_admin_command(home.path(), &args, &mut output), 2);
    assert!(output.is_empty());
    assert_zero_state(home.path());
}
