//! Human-rooted local bootstrap and signed admin operation application.

use crate::authority::KeyRole;
use crate::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot};
use crate::key_provider::{KeyProviderError, load_key_reference};
use crate::store::AuthorityStore;
use crate::{ActivationError, VerifiedAdminRequest, verify_admin_request};
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

#[derive(Debug, Clone)]
pub struct BootstrapPublicKeys {
    pub admin_root: [u8; 32],
    pub recovery_root: [u8; 32],
    pub receipt_signer: [u8; 32],
    pub local_cli_issuer: [u8; 32],
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
}

pub fn bootstrap(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    keys: BootstrapPublicKeys,
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
    let request = BootstrapRequest::new(
        keys.admin_root,
        keys.recovery_root,
        keys.receipt_signer,
        keys.local_cli_issuer,
        admin_id,
    );
    bootstrap_attested(nano_home, request)
}

fn bootstrap_attested(
    nano_home: &Path,
    request: BootstrapRequest,
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
    if nano_home.join("activation/authority.jsonl").exists() {
        return Err(BootstrapError::AlreadyBootstrapped);
    }
    verify_secure_home(nano_home)?;
    AuthorityStore::bootstrap_initial(
        nano_home,
        AuthoritySnapshot::empty(request.admin_id, request.admin_public_key)
            .with_recovery_key(request.recovery_public_key)
            .with_service_keys(
                request.receipt_signer_public_key,
                request.local_cli_public_key,
            ),
    )
    .map_err(Into::into)
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
        bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], [3; 32], [4; 32], "root-1"),
        )
        .unwrap();
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [2; 32], [3; 32], [4; 32], "root-1")
            ),
            Err(BootstrapError::AlreadyBootstrapped)
        ));
    }

    #[test]
    fn attested_bootstrap_rejects_role_key_reuse() {
        let home = tempfile::tempdir().unwrap();
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [1; 32], [3; 32], [4; 32], "root-1")
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
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [2; 32], [3; 32], [4; 32], "root-1")
            ),
            Err(BootstrapError::InsecureHome)
        ));
        assert!(!home.path().join("activation").exists());
    }
}
