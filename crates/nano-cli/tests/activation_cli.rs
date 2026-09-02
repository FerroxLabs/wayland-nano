use nano_cli::exec_mode::{ExecParams, LocalActivationParams, ResumeTarget};
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
fn foreground_pty_without_affirmative_utmp_row_fails_closed() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::var_os("HOME").unwrap())
        .unwrap();
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
    let output = run();
    assert!(!output.status.success());
    assert!(!home.path().join("activation/authority.jsonl").exists());
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
    assert!(!output.status.success(), "{output:?}");
    assert!(!home.path().join("activation/authority.jsonl").exists());
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

#[cfg(any(windows, target_os = "linux"))]
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
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 03-02/D3-04: the CLI exec mint surface (activation.rs:186-253) is the one
/// place local carriers originate, and it can only pin `fresh` /
/// `session_resume` with `fallback: none`. `memory_recall` arrives only via
/// signed external carriers (admission-matrix rows prove the gate side).
#[test]
fn local_cli_mint_pins_fresh_or_session_resume_with_fallback_none() {
    let home = tempfile::tempdir().unwrap();
    let seed = home.path().canonicalize().unwrap().join("cli.seed");
    std::fs::write(&seed, [5_u8; 32]).unwrap();
    secure(&seed);
    let keyref = home.path().join("cli.keyref");
    std::fs::write(
        &keyref,
        serde_json::to_vec(&serde_json::json!({
            "provider": "file",
            "reference": seed,
            "role": "local_cli_issuer"
        }))
        .unwrap(),
    )
    .unwrap();
    secure(&keyref);
    bootstrap_with_local_cli(home.path());

    let fresh_params = LocalActivationParams {
        key_reference: keyref.clone(),
        issuer_id: "desktop".into(),
        key_id: "desktop-key-1".into(),
        project_id: "project-a".into(),
        session_id: None,
        resume_fingerprint: None,
    };
    let fresh = mint_continuity(home.path(), &fresh_params);
    assert_eq!(fresh["strategy"], "fresh");
    assert_eq!(fresh["fallback"], "none");
    assert!(fresh["resume_fingerprint"].is_null());

    let resume_params = LocalActivationParams {
        key_reference: keyref,
        issuer_id: "desktop".into(),
        key_id: "desktop-key-1".into(),
        project_id: "project-a".into(),
        session_id: Some("session-a".into()),
        resume_fingerprint: Some("a".repeat(64)),
    };
    let resumed = mint_continuity(home.path(), &resume_params);
    assert_eq!(resumed["strategy"], "session_resume");
    assert_eq!(resumed["fallback"], "none");
    assert_eq!(resumed["resume_fingerprint"], "a".repeat(64));
}

fn mint_continuity(home: &std::path::Path, params: &LocalActivationParams) -> serde_json::Value {
    let frame = nano_cli::activation::mint_local_cli_request(home, params, PermissionMode::Default)
        .unwrap();
    let frame: serde_json::Value = serde_json::from_slice(&frame).unwrap();
    frame["params"]["_meta"]["waylandNanoActivation"]["continuity"].clone()
}

struct TestReceiptSigner(ed25519_dalek::SigningKey);
impl nano_activation::receipt::ReceiptSigner for TestReceiptSigner {
    fn key_id(&self) -> &str {
        "receipt-test-1"
    }
    fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
    fn preflight(&self) -> Result<(), nano_activation::receipt::ReceiptError> {
        Ok(())
    }
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], nano_activation::receipt::ReceiptError> {
        Ok(ed25519_dalek::Signer::sign(&self.0, message).to_bytes())
    }
}

fn bootstrap_with_local_cli(home: &std::path::Path) {
    use ed25519_dalek::SigningKey;
    use nano_activation::authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority};
    use std::collections::{BTreeMap, BTreeSet};

    let cli_key = SigningKey::from_bytes(&[5; 32]);
    let receipt = SigningKey::from_bytes(&[9; 32]);
    let mut keys = BTreeMap::new();
    keys.insert(
        "desktop-key-1".into(),
        AuthorityKey {
            public_key: cli_key.verifying_key().to_bytes(),
            epoch: 1,
            revoked: false,
        },
    );
    let mut projects = BTreeSet::new();
    projects.insert("project-a".into());
    let mut snapshot = AuthoritySnapshot::empty("root", [7; 32]).with_service_keys(
        receipt.verifying_key().to_bytes(),
        cli_key.verifying_key().to_bytes(),
    );
    snapshot.issuers.insert(
        "desktop".into(),
        IssuerAuthority {
            subject_id: "main".into(),
            principal_id: "main".into(),
            epoch: 1,
            revoked: false,
            keys,
            projects,
        },
    );
    let root = home.join("activation");
    std::fs::create_dir_all(&root).unwrap();
    let bootstrap_receipt = nano_activation::admin::sign_bootstrap_receipt(
        &snapshot,
        &TestReceiptSigner(SigningKey::from_bytes(&receipt.to_bytes())),
    )
    .unwrap();
    let mut bytes = serde_jcs::to_vec(&nano_activation::journal::AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot: snapshot.clone(),
    })
    .unwrap();
    bytes.push(b'\n');
    bytes.extend_from_slice(
        &serde_jcs::to_vec(
            &nano_activation::journal::AuthorityRecord::BootstrapReceipt {
                sequence: 2,
                receipt: String::from_utf8(bootstrap_receipt).unwrap(),
            },
        )
        .unwrap(),
    );
    bytes.push(b'\n');
    std::fs::write(root.join("authority.jsonl"), bytes).unwrap();
}

#[cfg(unix)]
fn secure(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(windows)]
fn secure(path: &std::path::Path) {
    let script = r#"
$file = [System.IO.FileInfo]::new($env:NANO_TEST_SECURE_FILE)
$acl = $file.GetAccessControl()
$acl.SetAccessRuleProtection($true, $false)
foreach ($rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleSpecific($rule) }
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$owner = $acl.GetOwner([System.Security.Principal.SecurityIdentifier])
if ($owner -ne $sid) { $acl.SetOwner($sid) }
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
  $sid,
  [System.Security.AccessControl.FileSystemRights]::FullControl,
  [System.Security.AccessControl.AccessControlType]::Allow)
[void]$acl.AddAccessRule($rule)
$file.SetAccessControl($acl)
"#;
    assert!(
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", script])
            .env("NANO_TEST_SECURE_FILE", path)
            .status()
            .unwrap()
            .success()
    );
}
