mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::receipt::{ReceiptError, ReceiptSigner};
use nano_activation::{
    build_identity,
    enablement::{EnablementCommand, EnablementError, EnablementFault, EnablementStore},
};
use serde_json::json;
use sha2::{Digest, Sha256};

const NOW: &str = "2026-08-30T10:00:00Z";

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
fn compiled_identity_matches_repository_and_workspace_lock() {
    let compiled = build_identity::compiled();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let commit = command(root, &["rev-parse", "HEAD"]);
    let lock = std::fs::read(root.join("Cargo.lock")).unwrap();
    assert_eq!(compiled.source_commit_sha, commit);
    assert_eq!(compiled.cargo_lock_sha256, hex(&Sha256::digest(lock)));
    assert_eq!(
        compiled
            .bind_executable(&"a".repeat(64))
            .unwrap()
            .executable_sha256,
        "a".repeat(64)
    );
}

#[test]
fn default_off_and_exact_current_window_are_authoritative() {
    let home = tempfile::tempdir().unwrap();
    let root = bootstrap(home.path());
    let store = EnablementStore::open(home.path()).unwrap();
    let command = enabled("op-1", support::artifact(), 1, "2026-08-30T10:05:00Z");
    assert!(matches!(
        store.require_enabled(&command.artifact, [1; 4], NOW),
        Err(EnablementError::Missing)
    ));
    store
        .apply_signed(
            &signed(&root, &store, &command, "enable_artifact"),
            &command,
            NOW,
            EnablementFault::None,
        )
        .unwrap();
    store
        .require_enabled(&command.artifact, [1; 4], NOW)
        .unwrap();
    assert!(matches!(
        store.require_enabled(&command.artifact, [2, 1, 1, 1], NOW),
        Err(EnablementError::EpochDrift)
    ));
    assert!(matches!(
        store.require_enabled(&support::artifact_with_exe('b'), [1; 4], NOW),
        Err(EnablementError::ArtifactMismatch)
    ));
    assert!(matches!(
        store.require_enabled(&command.artifact, [1; 4], "2026-08-30T10:06:00Z"),
        Err(EnablementError::Expired)
    ));
}

#[test]
fn disable_revokes_and_environment_cannot_enable() {
    let home = tempfile::tempdir().unwrap();
    let root = bootstrap(home.path());
    let store = EnablementStore::open(home.path()).unwrap();
    unsafe { std::env::set_var("NANO_ACTIVATION_ENABLED", "1") };
    assert!(matches!(
        store.require_enabled(&support::artifact(), [1; 4], NOW),
        Err(EnablementError::Missing)
    ));
    unsafe { std::env::remove_var("NANO_ACTIVATION_ENABLED") };
    let enable = enabled("op-1", support::artifact(), 1, "2026-08-30T10:05:00Z");
    store
        .apply_signed(
            &signed(&root, &store, &enable, "enable_artifact"),
            &enable,
            NOW,
            EnablementFault::None,
        )
        .unwrap();
    let mut disable = enabled("op-2", support::artifact(), 1, "2026-08-30T10:05:00Z");
    disable.enabled = false;
    store
        .apply_signed(
            &signed(&root, &store, &disable, "disable_artifact"),
            &disable,
            NOW,
            EnablementFault::None,
        )
        .unwrap();
    assert!(matches!(
        store.require_enabled(&disable.artifact, [1; 4], NOW),
        Err(EnablementError::Revoked)
    ));
}

#[test]
fn partial_crash_and_journal_anchor_ambiguity_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let root = bootstrap(home.path());
    let store = EnablementStore::open(home.path()).unwrap();
    let command = enabled("op-1", support::artifact(), 1, "2026-08-30T10:05:00Z");
    assert!(matches!(
        store.apply_signed(
            &signed(&root, &store, &command, "enable_artifact"),
            &command,
            NOW,
            EnablementFault::AfterPartialJournal
        ),
        Err(EnablementError::AmbiguousRecovery)
    ));
    assert!(matches!(
        store.require_enabled(&command.artifact, [1; 4], NOW),
        Err(EnablementError::AmbiguousRecovery)
    ));
    drop(store);
    let home2 = tempfile::tempdir().unwrap();
    let root2 = bootstrap(home2.path());
    let store2 = EnablementStore::open(home2.path()).unwrap();
    assert!(matches!(
        store2.apply_signed(
            &signed(&root2, &store2, &command, "enable_artifact"),
            &command,
            NOW,
            EnablementFault::AfterJournal
        ),
        Err(EnablementError::AmbiguousRecovery)
    ));
    assert!(matches!(
        store2.require_enabled(&command.artifact, [1; 4], NOW),
        Err(EnablementError::AmbiguousRecovery)
    ));
}

fn enabled(
    id: &str,
    artifact: nano_activation::receipt::ArtifactIdentity,
    epoch: u64,
    not_after: &str,
) -> EnablementCommand {
    EnablementCommand {
        operation_id: id.into(),
        enabled: true,
        artifact,
        admin_epoch: epoch,
        issuer_epoch: 1,
        grant_epoch: 1,
        revocation_epoch: 1,
        not_after: not_after.into(),
    }
}
fn bootstrap(home: &std::path::Path) -> SigningKey {
    let root = SigningKey::from_bytes(&[7; 32]);
    let receipt_signer = BootstrapSigner(SigningKey::from_bytes(&[9; 32]));
    let mut snapshot = nano_activation::authority::AuthoritySnapshot::empty(
        "root",
        root.verifying_key().to_bytes(),
    )
    .with_service_keys(receipt_signer.public_key(), [8; 32]);
    snapshot.issuers = Default::default();
    let dir = home.join("activation");
    std::fs::create_dir_all(&dir).unwrap();
    let receipt =
        nano_activation::admin::sign_bootstrap_receipt(&snapshot, &receipt_signer).unwrap();
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
                receipt: String::from_utf8(receipt).unwrap(),
            },
        )
        .unwrap(),
    );
    bytes.push(b'\n');
    std::fs::write(dir.join("authority.jsonl"), bytes).unwrap();
    root
}
fn signed(
    key: &SigningKey,
    store: &EnablementStore,
    command: &EnablementCommand,
    operation: &str,
) -> Vec<u8> {
    let mut v = json!({"admin_epoch":1,"admin_id":"root","after_digest":command.digest(),"alg":"Ed25519","before_digest":store.state_digest().unwrap(),"issued_at":"2026-08-30T09:59:59Z","key_id":"root-key","nonce":format!("nonce-{}",command.operation_id),"not_after":"2026-08-30T10:05:00Z","operation":operation,"operation_id":command.operation_id,"reason":"operator decision","schema":"wayland.nano.admin-request/v1"});
    let payload = serde_jcs::to_vec(&v).unwrap();
    let mut msg = b"WAYLAND-NANO-ADMIN\0v1\0".to_vec();
    msg.extend(payload);
    v.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&msg).to_bytes())),
    );
    serde_jcs::to_vec(&v).unwrap()
}
fn command(root: &std::path::Path, args: &[&str]) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .into()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
