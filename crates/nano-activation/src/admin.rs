//! Human-rooted local bootstrap and signed admin operation application.

use crate::authority::KeyRole;
use crate::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot};
use crate::key_provider::{KeyProviderError, load_key_reference};
use crate::receipt::ReceiptSigner;
use crate::signer_provider::{ExternalReceiptSigner, SignerProviderError, derive_public_key};
use crate::store::AuthorityStore;
use crate::{ActivationError, VerifiedAdminRequest, verify_admin_request};
use base64::Engine as _;
use sha2::Digest as _;
use std::io::IsTerminal;
use std::path::Path;

#[derive(Debug, Clone)]
struct BootstrapRequest {
    admin_public_key: [u8; 32],
    recovery_public_key: [u8; 32],
    receipt_signer_public_key: [u8; 32],
    local_cli_public_key: [u8; 32],
    admin_id: String,
}

impl BootstrapRequest {
    fn new(
        admin_public_key: [u8; 32],
        recovery_public_key: [u8; 32],
        receipt_signer_public_key: [u8; 32],
        local_cli_public_key: [u8; 32],
        admin_id: impl Into<String>,
    ) -> Self {
        Self {
            admin_public_key,
            recovery_public_key,
            receipt_signer_public_key,
            local_cli_public_key,
            admin_id: admin_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootstrapKeyPaths {
    pub admin_root: std::path::PathBuf,
    pub recovery_root: std::path::PathBuf,
    pub receipt_signer: std::path::PathBuf,
    pub local_cli_issuer: std::path::PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("bootstrap requires explicit confirmation")]
    ConfirmationRequired,
    #[error("bootstrap requires an attached controlling TTY")]
    NoControllingTty,
    #[error("authority is already bootstrapped")]
    AlreadyBootstrapped,
    #[error("Nano home is not secure and owner controlled")]
    InsecureHome,
    #[error("bootstrap keys and key references must be distinct across roles")]
    RoleKeyReuse,
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    #[error(transparent)]
    KeyProvider(#[from] KeyProviderError),
    #[error(transparent)]
    SignerProvider(#[from] SignerProviderError),
    #[error("bootstrap receipt signing failed")]
    Receipt,
}

pub fn bootstrap(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: impl Into<String>,
    confirmed: bool,
) -> Result<AuthorityStore, BootstrapError> {
    if !confirmed {
        return Err(BootstrapError::ConfirmationRequired);
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(BootstrapError::NoControllingTty);
    }
    let references = [
        load_key_reference(&paths.admin_root, KeyRole::AdminRoot)?,
        load_key_reference(&paths.recovery_root, KeyRole::RecoveryRoot)?,
        load_key_reference(&paths.receipt_signer, KeyRole::ReceiptSigner)?,
        load_key_reference(&paths.local_cli_issuer, KeyRole::LocalCliIssuer)?,
    ];
    for left in 0..references.len() {
        for right in left + 1..references.len() {
            if (references[left].provider(), references[left].reference())
                == (references[right].provider(), references[right].reference())
            {
                return Err(BootstrapError::RoleKeyReuse);
            }
        }
    }
    let receipt_signer = ExternalReceiptSigner::from_key_reference(&references[2])?;
    let request = BootstrapRequest::new(
        derive_public_key(&references[0], KeyRole::AdminRoot)?,
        derive_public_key(&references[1], KeyRole::RecoveryRoot)?,
        receipt_signer.public_key(),
        derive_public_key(&references[3], KeyRole::LocalCliIssuer)?,
        admin_id,
    );
    bootstrap_attested(nano_home, request, &receipt_signer)
}

fn bootstrap_attested(
    nano_home: &Path,
    request: BootstrapRequest,
    receipt_signer: &dyn ReceiptSigner,
) -> Result<AuthorityStore, BootstrapError> {
    let public_keys = [
        request.admin_public_key,
        request.recovery_public_key,
        request.receipt_signer_public_key,
        request.local_cli_public_key,
    ];
    for left in 0..public_keys.len() {
        if public_keys[left + 1..].contains(&public_keys[left]) {
            return Err(BootstrapError::RoleKeyReuse);
        }
    }
    verify_secure_home(nano_home)?;
    let snapshot = AuthoritySnapshot::empty(request.admin_id, request.admin_public_key)
        .with_recovery_key(request.recovery_public_key)
        .with_service_keys(
            request.receipt_signer_public_key,
            request.local_cli_public_key,
        );
    if receipt_signer.public_key() != request.receipt_signer_public_key {
        return Err(BootstrapError::RoleKeyReuse);
    }
    let receipt = sign_bootstrap_receipt(&snapshot, receipt_signer)?;
    match AuthorityStore::bootstrap_initial(nano_home, snapshot, receipt) {
        Ok(store) => Ok(store),
        Err(AuthorityError::OperationConflict) => Err(BootstrapError::AlreadyBootstrapped),
        Err(error) => Err(error.into()),
    }
}

fn sign_bootstrap_receipt(
    snapshot: &AuthoritySnapshot,
    signer: &dyn ReceiptSigner,
) -> Result<Vec<u8>, BootstrapError> {
    signer.preflight().map_err(|_| BootstrapError::Receipt)?;
    let mut receipt = serde_json::json!({
        "admin_epoch": snapshot.admin_epoch,
        "admin_id": snapshot.admin_id,
        "authority_journal_position": 1,
        "receipt_signer_key_id": signer.key_id(),
        "root_public_key_fingerprint": crate::authority::hex(&sha2::Sha256::digest(snapshot.admin_public_key)),
        "schema": "wayland.nano.admin-bootstrap-receipt/v1"
    });
    let canonical = serde_jcs::to_vec(&receipt).map_err(|_| BootstrapError::Receipt)?;
    let mut message = b"WAYLAND-NANO-ADMIN-BOOTSTRAP\0v1\0".to_vec();
    message.extend_from_slice(&canonical);
    let signature = signer.sign(&message).map_err(|_| BootstrapError::Receipt)?;
    receipt
        .as_object_mut()
        .ok_or(BootstrapError::Receipt)?
        .insert(
            "signature".into(),
            serde_json::Value::String(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature),
            ),
        );
    serde_jcs::to_vec(&receipt).map_err(|_| BootstrapError::Receipt)
}

pub fn verify_bootstrap_receipt(raw: &[u8], public_key: &[u8; 32]) -> Result<(), BootstrapError> {
    use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
    let mut value = crate::raw::parse_transport_frame(raw).map_err(|_| BootstrapError::Receipt)?;
    if serde_jcs::to_vec(&value).map_err(|_| BootstrapError::Receipt)? != raw {
        return Err(BootstrapError::Receipt);
    }
    let object = value.as_object_mut().ok_or(BootstrapError::Receipt)?;
    let expected = [
        "admin_epoch",
        "admin_id",
        "authority_journal_position",
        "receipt_signer_key_id",
        "root_public_key_fingerprint",
        "schema",
        "signature",
    ];
    if object.len() != expected.len() || expected.iter().any(|field| !object.contains_key(*field)) {
        return Err(BootstrapError::Receipt);
    }
    if object.get("schema").and_then(serde_json::Value::as_str)
        != Some("wayland.nano.admin-bootstrap-receipt/v1")
        || object
            .get("authority_journal_position")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        return Err(BootstrapError::Receipt);
    }
    let encoded = object
        .remove("signature")
        .and_then(|signature| signature.as_str().map(str::to_owned))
        .ok_or(BootstrapError::Receipt)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| BootstrapError::Receipt)?;
    let signature = Signature::from_slice(&bytes).map_err(|_| BootstrapError::Receipt)?;
    let canonical = serde_jcs::to_vec(&value).map_err(|_| BootstrapError::Receipt)?;
    let mut message = b"WAYLAND-NANO-ADMIN-BOOTSTRAP\0v1\0".to_vec();
    message.extend_from_slice(&canonical);
    VerifyingKey::from_bytes(public_key)
        .map_err(|_| BootstrapError::Receipt)?
        .verify(&message, &signature)
        .map_err(|_| BootstrapError::Receipt)
}

pub fn apply_signed_admin(
    store: &mut AuthorityStore,
    raw_request: &[u8],
    command: AuthorityCommand,
    now_utc: &str,
) -> Result<(), AdminError> {
    let before = store.snapshot()?.digest()?;
    let snapshot = store.snapshot()?;
    let verification_key = if matches!(command, AuthorityCommand::RecoverRoot { .. }) {
        snapshot
            .recovery_public_key
            .ok_or(AdminError::RecoveryUnavailable)?
    } else {
        snapshot.admin_public_key
    };
    let verified = verify_admin_request(raw_request, &verification_key)?;
    validate_admin_envelope(
        &verified,
        &snapshot.admin_id,
        snapshot.admin_epoch,
        command.operation_id(),
        &before,
        now_utc,
    )?;
    if verified.operation() != command_operation(&command) {
        return Err(AdminError::OperationMismatch);
    }
    let expires = parse_utc_seconds(verified.not_after()).ok_or(AdminError::Expired)?;
    let projected = snapshot.preview_admin_transaction(&command, verified.nonce(), expires)?;
    if projected.digest()? != verified.after_digest() {
        return Err(AdminError::Authority(AuthorityError::DigestMismatch));
    }
    store.commit_admin_transaction(command, verified.nonce(), expires)?;
    Ok(())
}

fn validate_admin_envelope(
    request: &VerifiedAdminRequest,
    admin_id: &str,
    epoch: u64,
    operation_id: &str,
    before: &str,
    now: &str,
) -> Result<(), AdminError> {
    if request.admin_id() != admin_id || request.admin_epoch() != epoch {
        return Err(AdminError::Authority(AuthorityError::StaleEpoch));
    }
    if request.operation_id() != operation_id || request.before_digest() != before {
        return Err(AdminError::Authority(AuthorityError::DigestMismatch));
    }
    if request.issued_at() > now || request.not_after() < now {
        return Err(AdminError::Expired);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error(transparent)]
    Activation(#[from] ActivationError),
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    #[error("admin request is outside its signed time window")]
    Expired,
    #[error("signed admin operation does not match the requested command")]
    OperationMismatch,
    #[error("root recovery is not provisioned")]
    RecoveryUnavailable,
}

fn command_operation(command: &AuthorityCommand) -> &'static str {
    match command {
        AuthorityCommand::EnrollIssuer { .. } => "enroll_issuer",
        AuthorityCommand::GrantProject { .. } => "grant_project",
        AuthorityCommand::RotateKey { .. } => "rotate_key",
        AuthorityCommand::RevokeKey { .. } => "revoke_key",
        AuthorityCommand::RevokeIssuer { .. } | AuthorityCommand::RetireSubject { .. } => {
            "revoke_issuer"
        }
        AuthorityCommand::RecoverRoot { .. } => "recover_root",
        AuthorityCommand::ConsumeNonce { .. } => "rollback",
    }
}

fn parse_utc_seconds(value: &str) -> Option<i64> {
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hour * 3600 + minute * 60 + second)
}

fn verify_secure_home(path: &Path) -> Result<(), BootstrapError> {
    if !path.is_absolute() {
        return Err(BootstrapError::InsecureHome);
    }
    std::fs::create_dir_all(path).map_err(AuthorityError::from)?;
    let metadata = std::fs::symlink_metadata(path).map_err(AuthorityError::from)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(BootstrapError::InsecureHome);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(BootstrapError::InsecureHome);
        }
    }
    #[cfg(windows)]
    crate::key_provider::audit_owner_only_path(path).map_err(|_| BootstrapError::InsecureHome)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    struct TestReceiptSigner(SigningKey);
    impl ReceiptSigner for TestReceiptSigner {
        fn key_id(&self) -> &str {
            "test-bootstrap-receipt"
        }
        fn public_key(&self) -> [u8; 32] {
            self.0.verifying_key().to_bytes()
        }
        fn preflight(&self) -> Result<(), crate::receipt::ReceiptError> {
            Ok(())
        }
        fn sign(&self, message: &[u8]) -> Result<[u8; 64], crate::receipt::ReceiptError> {
            Ok(self.0.sign(message).to_bytes())
        }
    }

    #[test]
    fn attested_bootstrap_is_exactly_once() {
        // GitHub's RUNNER_TEMP and checkout ancestry can be writable by other
        // principals, which production correctly rejects as an insecure home.
        #[cfg(unix)]
        let home = {
            use std::os::unix::fs::PermissionsExt;
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in(std::env::var_os("HOME").unwrap())
                .unwrap()
        };
        #[cfg(windows)]
        let home = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        {
            let script = r#"
$directory = [System.IO.DirectoryInfo]::new($env:NANO_TEST_SECURE_HOME)
$acl = $directory.GetAccessControl()
$acl.SetAccessRuleProtection($true, $false)
foreach ($rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleSpecific($rule) }
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$owner = $acl.GetOwner([System.Security.Principal.SecurityIdentifier])
if ($owner -ne $sid) { $acl.SetOwner($sid) }
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
  $sid,
  [System.Security.AccessControl.FileSystemRights]::FullControl,
  [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',
  [System.Security.AccessControl.PropagationFlags]::None,
[System.Security.AccessControl.AccessControlType]::Allow)
[void]$acl.AddAccessRule($rule)
$directory.SetAccessControl($acl)
"#;
            assert!(
                std::process::Command::new("powershell.exe")
                    .args(["-NoProfile", "-Command", script])
                    .env("NANO_TEST_SECURE_HOME", home.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        let first = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        let first_receipt = first.bootstrap_receipt().unwrap().to_vec();
        verify_bootstrap_receipt(&first_receipt, &signer.public_key()).unwrap();
        let mut tampered = first_receipt.clone();
        let index = tampered.iter().position(|byte| *byte == b'r').unwrap();
        tampered[index] = b's';
        assert!(verify_bootstrap_receipt(&tampered, &signer.public_key()).is_err());
        let receipt_text = String::from_utf8(first_receipt.clone()).unwrap();
        assert!(!receipt_text.contains("reference"));
        assert!(!receipt_text.contains("private"));
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
                &signer,
            ),
            Err(BootstrapError::Authority(AuthorityError::Contention))
        ));
        drop(first);
        let replayed = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        assert_eq!(replayed.bootstrap_receipt(), Some(first_receipt.as_slice()));
        assert_eq!(
            std::fs::read_to_string(home.path().join("activation/authority.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn attested_bootstrap_rejects_role_key_reuse() {
        let home = tempfile::tempdir().unwrap();
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [1; 32], signer.public_key(), [4; 32], "root-1"),
                &signer,
            ),
            Err(BootstrapError::RoleKeyReuse)
        ));
        assert!(!home.path().join("activation").exists());
    }

    #[cfg(unix)]
    #[test]
    fn attested_bootstrap_rejects_insecure_home() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
                &signer,
            ),
            Err(BootstrapError::InsecureHome)
        ));
        assert!(!home.path().join("activation").exists());
    }

    #[cfg(unix)]
    #[test]
    fn attested_bootstrap_repairs_torn_initial_record() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::var_os("HOME").unwrap())
            .unwrap();
        let activation = home.path().join("activation");
        std::fs::create_dir_all(&activation).unwrap();
        std::fs::write(activation.join("authority.jsonl"), b"{\"record_type\":").unwrap();
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        let store = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        assert!(store.bootstrap_receipt().is_some());
        assert_eq!(
            std::fs::read_to_string(activation.join("authority.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn killed_between_bootstrap_and_receipt_restarts_idempotently() {
        use std::os::unix::fs::PermissionsExt;
        if let Some(home) = std::env::var_os("NANO_BOOTSTRAP_KILL_HOME") {
            let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
            let snapshot = AuthoritySnapshot::empty("root-1", [1; 32])
                .with_recovery_key([2; 32])
                .with_service_keys(signer.public_key(), [4; 32]);
            let receipt = sign_bootstrap_receipt(&snapshot, &signer).unwrap();
            let _ = AuthorityStore::bootstrap_initial_with_fault(
                Path::new(&home),
                snapshot,
                receipt,
                || std::process::abort(),
            );
            unreachable!();
        }
        let home = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::var_os("HOME").unwrap())
            .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("admin::tests::killed_between_bootstrap_and_receipt_restarts_idempotently")
            .arg("--nocapture")
            .env("NANO_BOOTSTRAP_KILL_HOME", home.path())
            .status()
            .unwrap();
        assert!(!status.success());
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        let store = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        verify_bootstrap_receipt(store.bootstrap_receipt().unwrap(), &signer.public_key()).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join("activation/authority.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}
