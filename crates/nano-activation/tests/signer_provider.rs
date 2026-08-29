use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use nano_activation::authority::KeyRole;
use nano_activation::key_provider::load_key_reference;
use nano_activation::receipt::ReceiptSigner;
use nano_activation::signer_provider::{
    ExternalActivationSigner, ExternalReceiptSigner, SignerProviderError,
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
    let user = std::env::var("USERNAME").unwrap();
    assert!(
        std::process::Command::new("icacls")
            .arg(path)
            .arg("/inheritance:r")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("icacls")
            .arg(path)
            .arg("/grant:r")
            .arg(format!("{user}:(F)"))
            .status()
            .unwrap()
            .success()
    );
}
