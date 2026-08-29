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
