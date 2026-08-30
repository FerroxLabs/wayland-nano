use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::admin::{AdminError, BootstrapError, apply_signed_admin};
use nano_activation::authority::{AuthorityCommand, AuthoritySnapshot, KeyRole};
use nano_activation::journal::AuthorityRecord;
use nano_activation::key_provider::{KeyProviderError, load_key_reference};
use nano_activation::receipt::{ReceiptError, ReceiptSigner};
use nano_activation::store::AuthorityStore;
use serde_json::json;
use std::process::Command;

struct BootstrapSigner(SigningKey);
impl ReceiptSigner for BootstrapSigner {
    fn key_id(&self) -> &str {
        "test-bootstrap"
    }
    fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
    fn preflight(&self) -> Result<(), ReceiptError> {
        Ok(())
    }
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], ReceiptError> {
        Ok(self.0.sign(message).to_bytes())
    }
}

#[test]
fn key_reference_is_role_bound_and_owner_only() {
    let home = tempfile::tempdir().unwrap();
    let path = home
        .path()
        .canonicalize()
        .unwrap()
        .join("admin-root.keyref");
    std::fs::write(
        &path,
        br#"{"provider":"file","reference":"opaque-admin-root","role":"admin_root"}"#,
    )
    .unwrap();
    secure(&path);
    let reference = load_key_reference(&path, KeyRole::AdminRoot).unwrap();
    assert_eq!(reference.reference(), "opaque-admin-root");
    assert!(matches!(
        load_key_reference(&path, KeyRole::ReceiptSigner),
        Err(KeyProviderError::RoleMismatch)
    ));
}

#[test]
fn symlink_or_reparse_reference_is_refused_in_real_child_process() {
    if std::env::var_os("NANO_ACTIVATION_KEYREF_CHILD").is_some() {
        let path = std::env::var_os("NANO_ACTIVATION_KEYREF_PATH").unwrap();
        std::process::exit(
            if load_key_reference(std::path::Path::new(&path), KeyRole::AdminRoot).is_err() {
                0
            } else {
                9
            },
        );
    }
    let home = tempfile::tempdir().unwrap();
    let root = home.path().canonicalize().unwrap();
    let target = root.join("target.keyref");
    std::fs::write(
        &target,
        br#"{"provider":"file","reference":"opaque","role":"admin_root"}"#,
    )
    .unwrap();
    secure(&target);
    let alias = root.join("alias.keyref");
    if !make_alias(&target, &alias) {
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("symlink_or_reparse_reference_is_refused_in_real_child_process")
        .arg("--nocapture")
        .env("NANO_ACTIVATION_KEYREF_CHILD", "1")
        .env("NANO_ACTIVATION_KEYREF_PATH", &alias)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn broad_permissions_are_refused_in_real_child_process() {
    if std::env::var_os("NANO_ACTIVATION_BROAD_ACL_CHILD").is_some() {
        let path = std::env::var_os("NANO_ACTIVATION_KEYREF_PATH").unwrap();
        std::process::exit(
            if matches!(
                load_key_reference(std::path::Path::new(&path), KeyRole::AdminRoot),
                Err(KeyProviderError::InsecurePermissions)
            ) {
                0
            } else {
                9
            },
        );
    }
    let home = tempfile::tempdir().unwrap();
    let path = home.path().canonicalize().unwrap().join("broad.keyref");
    std::fs::write(
        &path,
        br#"{"provider":"file","reference":"opaque","role":"admin_root"}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("broad_permissions_are_refused_in_real_child_process")
        .arg("--nocapture")
        .env("NANO_ACTIVATION_BROAD_ACL_CHILD", "1")
        .env("NANO_ACTIVATION_KEYREF_PATH", &path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn bootstrap_requires_confirmation_tty_and_empty_store() {
    if std::env::var_os("NANO_ACTIVATION_DETACHED_CHILD").is_some() {
        std::process::exit(
            if matches!(
                nano_activation::admin::attest_interactive_owner("root-1"),
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
        .arg("bootstrap_requires_confirmation_tty_and_empty_store")
        .arg("--nocapture")
        .env("NANO_ACTIVATION_DETACHED_CHILD", "1")
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn signed_admin_lifecycle_checks_epoch_digests_nonce_and_root_recovery() {
    let home = tempfile::tempdir().unwrap();
    let root = SigningKey::from_bytes(&[3; 32]);
    let recovery = SigningKey::from_bytes(&[6; 32]);
    let receipt_signer = BootstrapSigner(SigningKey::from_bytes(&[8; 32]));
    let snapshot = AuthoritySnapshot::empty("root-1", root.verifying_key().to_bytes())
        .with_recovery_key(recovery.verifying_key().to_bytes())
        .with_service_keys(receipt_signer.public_key(), [9; 32]);
    let activation = home.path().join("activation");
    std::fs::create_dir_all(&activation).unwrap();
    let mut bytes = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot: snapshot.clone(),
    })
    .unwrap();
    bytes.push(b'\n');
    let receipt =
        nano_activation::admin::sign_bootstrap_receipt(&snapshot, &receipt_signer).unwrap();
    bytes.extend_from_slice(
        &serde_jcs::to_vec(&AuthorityRecord::BootstrapReceipt {
            sequence: 2,
            receipt: String::from_utf8(receipt).unwrap(),
        })
        .unwrap(),
    );
    bytes.push(b'\n');
    std::fs::write(activation.join("authority.jsonl"), bytes).unwrap();
    let mut store = AuthorityStore::open(home.path()).unwrap();
    let enroll = AuthorityCommand::EnrollIssuer {
        operation_id: "admin-op-1".into(),
        issuer_id: "desktop".into(),
        subject_id: "subject-a".into(),
        principal_id: "agent-a".into(),
        key_id: "issuer-key-1".into(),
        public_key: [4; 32],
    };
    apply(&mut store, &root, "enroll_issuer", enroll, "admin-nonce-1").unwrap();
    let grant = AuthorityCommand::GrantProject {
        operation_id: "admin-op-2".into(),
        issuer_id: "desktop".into(),
        subject_id: "subject-a".into(),
        principal_id: "agent-a".into(),
        project_id: "project-a".into(),
    };
    apply(&mut store, &root, "grant_project", grant, "admin-nonce-2").unwrap();
    let replacement = SigningKey::from_bytes(&[5; 32]);
    let recover = AuthorityCommand::RecoverRoot {
        operation_id: "admin-op-3".into(),
        expected_epoch: 1,
        new_admin_id: "root-2".into(),
        new_public_key: replacement.verifying_key().to_bytes(),
    };
    apply(
        &mut store,
        &recovery,
        "recover_root",
        recover,
        "admin-nonce-3",
    )
    .unwrap();
    let revoke = AuthorityCommand::RevokeIssuer {
        operation_id: "admin-op-4".into(),
        issuer_id: "desktop".into(),
    };
    let raw = signed_request(&store, &root, "revoke_issuer", &revoke, "admin-nonce-4");
    assert!(matches!(
        apply_signed_admin(&mut store, &raw, revoke, "2026-08-30T00:00:00Z"),
        Err(AdminError::Activation(_))
    ));
}

fn apply(
    store: &mut AuthorityStore,
    key: &SigningKey,
    operation: &str,
    command: AuthorityCommand,
    nonce: &str,
) -> Result<(), AdminError> {
    let raw = signed_request(store, key, operation, &command, nonce);
    apply_signed_admin(store, &raw, command, "2026-08-30T00:00:00Z")
}

fn signed_request(
    store: &AuthorityStore,
    key: &SigningKey,
    operation: &str,
    command: &AuthorityCommand,
    nonce: &str,
) -> Vec<u8> {
    let snapshot = store.snapshot().unwrap();
    let after = snapshot
        .preview_admin_transaction(command, nonce, 4_102_444_799)
        .unwrap();
    let mut value = json!({
        "admin_epoch": snapshot.admin_epoch, "admin_id": snapshot.admin_id, "after_digest": after.digest().unwrap(),
        "alg": "Ed25519", "before_digest": snapshot.digest().unwrap(), "issued_at": "2026-08-30T00:00:00Z",
        "key_id": "admin-key-1", "nonce": nonce, "not_after": "2099-12-31T23:59:59Z",
        "operation": operation, "operation_id": command.operation_id(), "reason": "operator approved",
        "schema": "wayland.nano.admin-request/v1"
    });
    let payload = serde_jcs::to_vec(&value).unwrap();
    let mut message = b"WAYLAND-NANO-ADMIN\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    let signature = key.sign(&message);
    value.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(signature.to_bytes())),
    );
    serde_jcs::to_vec(&value).unwrap()
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
        Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", script])
            .env("NANO_TEST_SECURE_FILE", path)
            .status()
            .unwrap()
            .success()
    );
}

#[cfg(unix)]
fn make_alias(target: &std::path::Path, alias: &std::path::Path) -> bool {
    std::os::unix::fs::symlink(target, alias).is_ok()
}
#[cfg(windows)]
fn make_alias(target: &std::path::Path, alias: &std::path::Path) -> bool {
    std::os::windows::fs::symlink_file(target, alias).is_ok()
}
