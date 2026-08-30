use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use nano_activation::authority::KeyRole;
use nano_activation::key_provider::load_key_reference;
use nano_activation::receipt::ReceiptSigner;
use nano_activation::signer_provider::{
    ExternalActivationSigner, ExternalReceiptSigner, SignerProviderError, derive_public_key,
};
use std::path::Path;

#[test]
fn owner_only_file_provider_signs_with_public_identity() {
    let home = tempfile::tempdir().unwrap();
    let key_path = home.path().canonicalize().unwrap().join("receipt.seed");
    std::fs::write(&key_path, [7_u8; 32]).unwrap();
    secure(&key_path);
    let reference = write_reference(home.path(), "file", key_path.to_str().unwrap());

    let signer = ExternalReceiptSigner::from_key_reference(&reference).unwrap();
    signer.preflight().unwrap();
    let message = b"provider adapter test";
    let signature = Signature::from_bytes(&signer.sign(message).unwrap());
    VerifyingKey::from_bytes(&signer.public_key())
        .unwrap()
        .verify(message, &signature)
        .unwrap();
    assert!(signer.key_id().starts_with("receipt-ed25519-"));
}

#[test]
fn provider_derivation_binds_role_reference_to_actual_public_key() {
    let home = tempfile::tempdir().unwrap();
    let seed_path = home.path().canonicalize().unwrap().join("admin.seed");
    std::fs::write(&seed_path, [11_u8; 32]).unwrap();
    secure(&seed_path);
    let reference_path = home.path().join("admin.keyref");
    std::fs::write(
        &reference_path,
        serde_json::to_vec(&serde_json::json!({
            "provider": "file",
            "reference": seed_path,
            "role": "admin_root"
        }))
        .unwrap(),
    )
    .unwrap();
    secure(&reference_path);
    let reference = load_key_reference(&reference_path, KeyRole::AdminRoot).unwrap();
    let expected = ed25519_dalek::SigningKey::from_bytes(&[11_u8; 32])
        .verifying_key()
        .to_bytes();
    assert_eq!(
        derive_public_key(&reference, KeyRole::AdminRoot).unwrap(),
        expected
    );
    assert!(matches!(
        derive_public_key(&reference, KeyRole::RecoveryRoot),
        Err(SignerProviderError::RoleMismatch)
    ));
    std::fs::write(&seed_path, [12_u8; 32]).unwrap();
    secure(&seed_path);
    assert_ne!(
        derive_public_key(&reference, KeyRole::AdminRoot).unwrap(),
        expected
    );
}

#[test]
fn missing_changed_and_wrong_sized_keys_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let key_path = home.path().canonicalize().unwrap().join("receipt.seed");
    std::fs::write(&key_path, [8_u8; 32]).unwrap();
    secure(&key_path);
    let reference = write_reference(home.path(), "file", key_path.to_str().unwrap());
    let signer = ExternalReceiptSigner::from_key_reference(&reference).unwrap();

    std::fs::remove_file(&key_path).unwrap();
    assert!(signer.preflight().is_err());
    assert!(signer.sign(b"must not sign").is_err());

    std::fs::write(&key_path, [9_u8; 32]).unwrap();
    secure(&key_path);
    assert!(signer.preflight().is_err());
    assert!(signer.sign(b"must not sign").is_err());

    std::fs::write(&key_path, [1_u8; 31]).unwrap();
    secure(&key_path);
    assert!(matches!(
        ExternalReceiptSigner::from_key_reference(&reference),
        Err(SignerProviderError::InvalidKeyMaterial)
    ));
}

#[test]
fn os_provider_and_secrets_paths_are_typed_refusals() {
    let home = tempfile::tempdir().unwrap();
    let os_reference = write_reference(home.path(), "os", "receipt-key-1");
    assert!(matches!(
        ExternalReceiptSigner::from_key_reference(&os_reference),
        Err(SignerProviderError::OsProviderUnavailable)
    ));

    let secrets_path = home
        .path()
        .canonicalize()
        .unwrap()
        .join(".secrets")
        .join("receipt.seed");
    let file_reference = write_reference(home.path(), "file", secrets_path.to_str().unwrap());
    assert!(matches!(
        ExternalReceiptSigner::from_key_reference(&file_reference),
        Err(SignerProviderError::ForbiddenPath)
    ));
}

#[test]
fn local_activation_signer_requires_the_distinct_cli_role() {
    let home = tempfile::tempdir().unwrap();
    let key_path = home.path().canonicalize().unwrap().join("cli.seed");
    std::fs::write(&key_path, [6_u8; 32]).unwrap();
    secure(&key_path);
    let receipt = write_reference(home.path(), "file", key_path.to_str().unwrap());
    assert!(matches!(
        ExternalActivationSigner::from_key_reference(&receipt),
        Err(SignerProviderError::RoleMismatch)
    ));

    let path = home.path().join("cli.keyref");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "provider": "file",
            "reference": key_path,
            "role": "local_cli_issuer"
        }))
        .unwrap(),
    )
    .unwrap();
    secure(&path);
    let reference = load_key_reference(&path, KeyRole::LocalCliIssuer).unwrap();
    let signer = ExternalActivationSigner::from_key_reference(&reference).unwrap();
    let payload = b"{}";
    let signature = Signature::from_bytes(&signer.sign_activation(payload).unwrap());
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend_from_slice(payload);
    VerifyingKey::from_bytes(&signer.public_key())
        .unwrap()
        .verify(&message, &signature)
        .unwrap();
}

fn write_reference(
    home: &Path,
    provider: &str,
    locator: &str,
) -> nano_activation::key_provider::KeyReference {
    let path = home.join(format!("{provider}.keyref"));
    let bytes = serde_json::to_vec(&serde_json::json!({
        "provider": provider,
        "reference": locator,
        "role": "receipt_signer"
    }))
    .unwrap();
    std::fs::write(&path, bytes).unwrap();
    secure(&path);
    load_key_reference(&path, KeyRole::ReceiptSigner).unwrap()
}

#[cfg(unix)]
fn secure(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(windows)]
fn secure(path: &Path) {
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
