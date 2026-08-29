use nano_activation::admin::{BootstrapError, BootstrapRequest, bootstrap};
use nano_activation::authority::KeyRole;
use nano_activation::key_provider::{KeyProviderError, load_key_reference};
use std::process::Command;

#[test]
fn key_reference_is_role_bound_and_owner_only() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("admin-root.keyref");
    std::fs::write(&path, br#"{"provider":"file","reference":"opaque-admin-root","role":"admin_root"}"#).unwrap();
    secure(&path);
    let reference = load_key_reference(&path, KeyRole::AdminRoot).unwrap();
    assert_eq!(reference.reference(), "opaque-admin-root");
    assert!(matches!(load_key_reference(&path, KeyRole::ReceiptSigner), Err(KeyProviderError::RoleMismatch)));
}

#[test]
fn symlink_or_reparse_reference_is_refused_in_real_child_process() {
    if std::env::var_os("NANO_ACTIVATION_KEYREF_CHILD").is_some() {
        let path = std::env::var_os("NANO_ACTIVATION_KEYREF_PATH").unwrap();
        std::process::exit(if load_key_reference(std::path::Path::new(&path), KeyRole::AdminRoot).is_err() { 0 } else { 9 });
    }
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("target.keyref");
    std::fs::write(&target, br#"{"provider":"file","reference":"opaque","role":"admin_root"}"#).unwrap();
    secure(&target);
    let alias = home.path().join("alias.keyref");
    if !make_alias(&target, &alias) { return; }
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact").arg("symlink_or_reparse_reference_is_refused_in_real_child_process")
        .arg("--nocapture")
        .env("NANO_ACTIVATION_KEYREF_CHILD", "1")
        .env("NANO_ACTIVATION_KEYREF_PATH", &alias).status().unwrap();
    assert!(status.success());
}

#[test]
fn bootstrap_requires_confirmation_tty_and_empty_store() {
    let home = tempfile::tempdir().unwrap();
    let request = BootstrapRequest::for_test([1; 32], "root-1");
    assert!(matches!(bootstrap(home.path(), request.clone(), false), Err(BootstrapError::ConfirmationRequired)));
    bootstrap(home.path(), request.clone(), true).unwrap();
    assert!(matches!(bootstrap(home.path(), request, true), Err(BootstrapError::AlreadyBootstrapped)));
}

#[cfg(unix)]
fn secure(path: &std::path::Path) { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap(); }
#[cfg(windows)]
fn secure(_path: &std::path::Path) {}

#[cfg(unix)]
fn make_alias(target: &std::path::Path, alias: &std::path::Path) -> bool { std::os::unix::fs::symlink(target, alias).is_ok() }
#[cfg(windows)]
fn make_alias(target: &std::path::Path, alias: &std::path::Path) -> bool { std::os::windows::fs::symlink_file(target, alias).is_ok() }
