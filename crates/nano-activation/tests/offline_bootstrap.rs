#![cfg(windows)]

use nano_activation::admin::{BootstrapError, attest_interactive_owner};
use nano_activation::run_phase2_offline_bootstrap_command;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ADMIN_ID: &str = "phase2-owner";

fn key_argv(root: &Path) -> Vec<String> {
    vec![
        "--admin-root-keyref".into(),
        root.join("admin.keyref").display().to_string(),
        "--recovery-root-keyref".into(),
        root.join("recovery.keyref").display().to_string(),
        "--receipt-signer-keyref".into(),
        root.join("receipt.keyref").display().to_string(),
        "--local-cli-keyref".into(),
        root.join("cli.keyref").display().to_string(),
    ]
}

fn unused_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nano-offline-{label}-must-not-exist-{}",
        std::process::id()
    ))
}

#[test]
fn offline_facade_rejects_incomplete_wrong_document_and_relative_argv_before_io() {
    let root = unused_root("argv");
    let mut output = Vec::new();

    for args in [
        vec!["offline-bootstrap-challenge".into()],
        {
            let mut args = vec!["offline-bootstrap-challenge".into()];
            args.extend(key_argv(&root));
            args.extend(["--output".into(), "relative.json".into()]);
            args
        },
        {
            let mut args = vec!["offline-bootstrap-challenge".into()];
            args.extend(key_argv(&root));
            args.extend([
                "--authorization".into(),
                root.join("authorization.json").display().to_string(),
            ]);
            args
        },
        {
            let mut args = vec!["offline-bootstrap-apply".into()];
            args.extend(key_argv(&root));
            args.extend(["--authorization".into(), "relative.json".into()]);
            args
        },
    ] {
        output.clear();
        assert_eq!(
            run_phase2_offline_bootstrap_command(&root.join("home"), &args, &mut output),
            2
        );
        assert!(output.is_empty());
        assert!(!root.exists());
    }
}

#[test]
fn offline_facade_rejects_duplicate_unknown_and_fallback_argv_before_io() {
    let root = unused_root("authority-argv");
    let document = root.join("authorization.json").display().to_string();
    let mut output = Vec::new();

    let mut duplicate = vec!["offline-bootstrap-apply".into()];
    duplicate.extend(key_argv(&root));
    duplicate.extend([
        "--authorization".into(),
        document.clone(),
        "--authorization".into(),
        document.clone(),
    ]);
    let mut unknown = vec!["offline-bootstrap-apply".into()];
    unknown.extend(key_argv(&root));
    unknown.extend([
        "--authorization".into(),
        document.clone(),
        "--admin-id".into(),
        ADMIN_ID.into(),
    ]);
    let mut fallback = vec!["offline-bootstrap-challenge".into()];
    fallback.extend(key_argv(&root));
    fallback.extend([
        "--output".into(),
        root.join("challenge.json").display().to_string(),
        "--stdin".into(),
        "true".into(),
    ]);

    for args in [duplicate, unknown, fallback, vec!["bootstrap".into()]] {
        output.clear();
        assert_eq!(
            run_phase2_offline_bootstrap_command(&root.join("home"), &args, &mut output),
            2
        );
        assert!(output.is_empty());
        assert!(!root.exists());
    }
}

#[test]
fn ordinary_interactive_bootstrap_still_refuses_detached_or_rdp_execution() {
    const CHILD: &str = "NANO_OFFLINE_BOOTSTRAP_RDP_CHILD";
    if std::env::var_os(CHILD).is_some() {
        std::process::exit(
            if matches!(
                attest_interactive_owner(ADMIN_ID),
                Err(BootstrapError::NoControllingTty | BootstrapError::RemoteSession)
            ) {
                0
            } else {
                9
            },
        );
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("ordinary_interactive_bootstrap_still_refuses_detached_or_rdp_execution")
        .arg("--nocapture")
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}
