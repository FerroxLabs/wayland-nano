use nano_cli::exec_mode::{ExecParams, ResumeTarget};
use nano_protocol::permission_mode::PermissionMode;
#[cfg(target_os = "linux")]
use std::io::Write as _;
#[cfg(any(windows, target_os = "linux"))]
use std::process::{Command, Stdio};

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
fn admin_bootstrap_rejects_confirmation_in_argv_before_state_or_key_access() {
    let home = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    let args = vec!["admin-bootstrap".into(), "--confirm".into(), "yes".into()];
    assert_eq!(
        nano_cli::activation::run_activation_command(home.path(), &args, &mut output),
        2
    );
    assert!(output.is_empty());
    assert!(!home.path().join("activation").exists());
}

#[test]
#[cfg(any(windows, target_os = "linux"))]
fn admin_bootstrap_refuses_detached_process_before_creating_authority() {
    let home = tempfile::tempdir().unwrap();
    let args = bootstrap_process_args(home.path());
    #[cfg(target_os = "linux")]
    let output = Command::new("setsid")
        .arg(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(&args)
        .env("NANO_HOME", home.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    #[cfg(windows)]
    let output = {
        use std::os::windows::process::CommandExt as _;
        Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
            .args(&args)
            .env("NANO_HOME", home.path())
            .stdin(Stdio::null())
            .creation_flags(0x0800_0000)
            .output()
            .unwrap()
    };
    assert_eq!(output.status.code(), Some(2));
    let refusal = String::from_utf8_lossy(&output.stderr);
    assert!(
        [
            "requires controlling",
            "requires attached console",
            "requires owning console",
            "requires local process session",
        ]
        .iter()
        .any(|reason| refusal.contains(reason)),
        "{refusal}"
    );
    assert!(!home.path().join("activation/authority.jsonl").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn real_binary_foreground_pty_bootstrap_binds_keys_and_replays_receipt() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::var_os("HOME").unwrap())
        .unwrap();
    provision_bootstrap_keys(home.path());
    let args = bootstrap_process_args(home.path());
    let command = std::iter::once(shell_quote(env!("CARGO_BIN_EXE_wayland-nano")))
        .chain(args.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    let run = || {
        let mut child = Command::new("script")
            .args(["-qfec", &command, "/dev/null"])
            .env("NANO_HOME", home.path())
            .env_remove("SSH_CLIENT")
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"BOOTSTRAP owner-1\n")
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(first_stdout.contains("wayland.nano.admin-bootstrap-receipt/v1"));
    let journal_before = std::fs::read(home.path().join("activation/authority.jsonl")).unwrap();
    let second = run();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        journal_before,
        std::fs::read(home.path().join("activation/authority.jsonl")).unwrap()
    );
    let store = nano_activation::store::AuthorityStore::open(home.path()).unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.admin_id, "owner-1");
    assert_eq!(snapshot.admin_epoch, 1);
    nano_activation::admin::verify_bootstrap_receipt(
        store.bootstrap_receipt().unwrap(),
        &snapshot.receipt_signer_public_key.unwrap(),
    )
    .unwrap();
    let receipt = String::from_utf8_lossy(store.bootstrap_receipt().unwrap());
    assert!(!receipt.contains("SECRET-CANARY"));
    assert!(!receipt.contains(".seed"));
    assert!(!receipt.contains("keyref"));
}

#[cfg(target_os = "linux")]
#[test]
fn remote_environment_marker_is_not_session_authority() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::var_os("HOME").unwrap())
        .unwrap();
    let args = bootstrap_process_args(home.path());
    provision_bootstrap_keys(home.path());
    let command = std::iter::once(shell_quote(env!("CARGO_BIN_EXE_wayland-nano")))
        .chain(args.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut child = Command::new("script")
        .args(["-qfec", &command, "/dev/null"])
        .env("NANO_HOME", home.path())
        .env("SSH_CONNECTION", "127.0.0.1 1 127.0.0.1 2")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"BOOTSTRAP owner-1\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(home.path().join("activation/authority.jsonl").exists());
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

fn bootstrap_process_args(home: &std::path::Path) -> Vec<String> {
    vec![
        "admin".into(),
        "bootstrap".into(),
        "--admin-id".into(),
        "owner-1".into(),
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

#[cfg(target_os = "linux")]
fn provision_bootstrap_keys(home: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    for (name, role, byte) in [
        ("admin", "admin_root", 1u8),
        ("recovery", "recovery_root", 2u8),
        ("receipt", "receipt_signer", 3u8),
        ("cli", "local_cli_issuer", 4u8),
    ] {
        let seed = home.join(format!("{name}.seed"));
        std::fs::write(&seed, [byte; 32]).unwrap();
        std::fs::set_permissions(&seed, std::fs::Permissions::from_mode(0o600)).unwrap();
        let reference = home.join(format!("{name}.keyref"));
        std::fs::write(
            &reference,
            serde_json::to_vec(&serde_json::json!({
                "provider": "file",
                "reference": seed,
                "role": role
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&reference, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
