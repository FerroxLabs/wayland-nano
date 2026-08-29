use nano_cli::exec_mode::{ExecParams, ResumeTarget};
use nano_protocol::permission_mode::PermissionMode;

#[test]
fn unauthenticated_resume_refuses_before_nano_home_state() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let params = ExecParams {
        prompt: "resume".into(),
        mode: PermissionMode::Default,
        resume: Some(ResumeTarget::Id("session-a".into())),
        output_last_message: None,
        goal: None,
        model: None,
        auto: false,
        activation_request: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(nano_cli::exec_run::run(
            home.path(),
            workspace.path(),
            &params,
        )),
        2
    );
    assert!(!home.path().join("sessions").exists());
    assert!(!home.path().join("activation").exists());
}

#[test]
fn unauthenticated_fresh_exec_refuses_before_nano_home_state() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let params = ExecParams {
        prompt: "fresh".into(),
        mode: PermissionMode::Default,
        resume: None,
        output_last_message: None,
        goal: None,
        model: None,
        auto: false,
        activation_request: None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(nano_cli::exec_run::run(
            home.path(),
            workspace.path(),
            &params,
        )),
        2
    );
    assert!(!home.path().join("sessions").exists());
    assert!(!home.path().join("activation").exists());
}

#[test]
fn activation_admin_surface_rejects_incomplete_arguments_without_state() {
    let home = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    assert_eq!(
        nano_cli::activation::run_activation_command(
            home.path(),
            &[
                "admin-apply".into(),
                "--request".into(),
                "request.json".into()
            ],
            &mut output,
        ),
        2
    );
    assert!(output.is_empty());
    assert!(!home.path().join("activation").exists());
}

#[test]
fn offline_receipt_verification_never_requires_nano_home_or_private_key() {
    let home = tempfile::tempdir().unwrap();
    let receipt = home.path().join("receipt.json");
    std::fs::write(&receipt, b"{}").unwrap();
    let mut output = Vec::new();
    let args = vec![
        "receipt-verify".into(),
        "--receipt".into(),
        receipt.display().to_string(),
        "--public-key".into(),
        "00".repeat(32),
    ];
    assert_eq!(
        nano_cli::activation::run_activation_command(home.path(), &args, &mut output),
        2
    );
    assert!(output.is_empty());
    assert!(!home.path().join("activation").exists());
}
