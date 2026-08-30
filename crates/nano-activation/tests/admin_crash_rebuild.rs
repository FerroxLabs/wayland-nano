use ed25519_dalek::{Signer as _, SigningKey};
use nano_activation::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot, KeyRole};
use nano_activation::journal::AuthorityRecord;
use nano_activation::receipt::{ReceiptError, ReceiptSigner};
use nano_activation::store::AuthorityStore;

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

fn snapshot() -> AuthoritySnapshot {
    let signer = BootstrapSigner(SigningKey::from_bytes(&[9; 32]));
    AuthoritySnapshot::empty("root-1", [7; 32]).with_service_keys(signer.public_key(), [8; 32])
}

fn bootstrapped(home: &std::path::Path, snapshot: AuthoritySnapshot) -> AuthorityStore {
    let activation = home.join("activation");
    std::fs::create_dir_all(&activation).unwrap();
    let signer = BootstrapSigner(SigningKey::from_bytes(&[9; 32]));
    let receipt = nano_activation::admin::sign_bootstrap_receipt(&snapshot, &signer).unwrap();
    let mut bytes = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot,
    })
    .unwrap();
    bytes.push(b'\n');
    bytes.extend_from_slice(
        &serde_jcs::to_vec(&AuthorityRecord::BootstrapReceipt {
            sequence: 2,
            receipt: String::from_utf8(receipt).unwrap(),
        })
        .unwrap(),
    );
    bytes.push(b'\n');
    std::fs::write(activation.join("authority.jsonl"), bytes).unwrap();
    AuthorityStore::open(home).unwrap()
}

#[test]
fn bootstrap_receipt_signature_and_snapshot_binding_are_replay_authority() {
    for role in 0..4 {
        let home = tempfile::tempdir().unwrap();
        drop(bootstrapped(home.path(), snapshot()));
        let journal = home.path().join("activation/authority.jsonl");
        let original = std::fs::read_to_string(&journal).unwrap();
        let mut records: Vec<AuthorityRecord> = original
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let AuthorityRecord::Bootstrap {
            snapshot: mutated_snapshot,
            ..
        } = &mut records[0]
        else {
            panic!("bootstrap first");
        };
        match role {
            0 => mutated_snapshot.admin_public_key = [55; 32],
            1 => mutated_snapshot.recovery_public_key = Some([55; 32]),
            2 => mutated_snapshot.receipt_signer_public_key = Some([55; 32]),
            _ => mutated_snapshot.local_cli_public_key = Some([55; 32]),
        }
        write_records(&journal, &records);
        assert!(matches!(
            AuthorityStore::open(home.path()),
            Err(AuthorityError::InvalidRecord)
        ));
    }

    let forged_home = tempfile::tempdir().unwrap();
    drop(bootstrapped(forged_home.path(), snapshot()));
    let forged_journal = forged_home.path().join("activation/authority.jsonl");
    let mut records: Vec<AuthorityRecord> = std::fs::read_to_string(&forged_journal)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let AuthorityRecord::BootstrapReceipt { receipt, .. } = &mut records[1] else {
        panic!("receipt second");
    };
    let mut value: serde_json::Value = serde_json::from_str(receipt).unwrap();
    let signature = value["signature"].as_str().unwrap();
    let replacement = if signature.starts_with('A') { 'B' } else { 'A' };
    value["signature"] = format!("{replacement}{}", &signature[1..]).into();
    *receipt = String::from_utf8(serde_jcs::to_vec(&value).unwrap()).unwrap();
    write_records(&forged_journal, &records);
    assert!(matches!(
        AuthorityStore::open(forged_home.path()),
        Err(AuthorityError::InvalidRecord)
    ));
}

fn write_records(path: &std::path::Path, records: &[AuthorityRecord]) {
    let mut bytes = Vec::new();
    for record in records {
        bytes.extend_from_slice(&serde_jcs::to_vec(record).unwrap());
        bytes.push(b'\n');
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn journal_rebuild_preserves_immutable_authority_and_revocation() {
    let home = tempfile::tempdir().unwrap();
    let mut store = bootstrapped(home.path(), snapshot());
    store
        .commit(AuthorityCommand::EnrollIssuer {
            operation_id: "op-enroll".into(),
            issuer_id: "desktop".into(),
            subject_id: "subject-a".into(),
            principal_id: "agent-a".into(),
            key_id: "desktop-key-1".into(),
            public_key: [9; 32],
        })
        .unwrap();
    store
        .commit(AuthorityCommand::GrantProject {
            operation_id: "op-grant".into(),
            issuer_id: "desktop".into(),
            subject_id: "subject-a".into(),
            principal_id: "agent-a".into(),
            project_id: "project-a".into(),
        })
        .unwrap();
    store
        .commit(AuthorityCommand::RotateKey {
            operation_id: "op-rotate".into(),
            issuer_id: "desktop".into(),
            old_key_id: "desktop-key-1".into(),
            new_key_id: "desktop-key-2".into(),
            public_key: [10; 32],
        })
        .unwrap();
    store
        .commit(AuthorityCommand::RevokeKey {
            operation_id: "op-revoke".into(),
            issuer_id: "desktop".into(),
            key_id: "desktop-key-1".into(),
        })
        .unwrap();
    let expected = store.snapshot().unwrap();
    drop(store);

    std::fs::remove_file(home.path().join("activation/authority.db")).unwrap();
    let rebuilt = AuthorityStore::open(home.path()).unwrap();
    assert_eq!(rebuilt.snapshot().unwrap(), expected);
    assert!(
        rebuilt
            .authorize(
                "desktop",
                "desktop-key-1",
                "subject-a",
                "agent-a",
                "project-a"
            )
            .is_err()
    );
    assert!(
        rebuilt
            .authorize(
                "desktop",
                "desktop-key-2",
                "subject-a",
                "agent-a",
                "project-a"
            )
            .is_ok()
    );
}

#[test]
fn immutable_bindings_retirement_and_conflicting_duplicates_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let mut store = bootstrapped(home.path(), snapshot());
    let enroll = AuthorityCommand::EnrollIssuer {
        operation_id: "op-1".into(),
        issuer_id: "desktop".into(),
        subject_id: "subject-a".into(),
        principal_id: "agent-a".into(),
        key_id: "key-1".into(),
        public_key: [8; 32],
    };
    store.commit(enroll.clone()).unwrap();
    store.commit(enroll).unwrap();
    assert!(matches!(
        store.commit(AuthorityCommand::EnrollIssuer {
            operation_id: "op-1".into(),
            issuer_id: "desktop".into(),
            subject_id: "subject-a".into(),
            principal_id: "agent-b".into(),
            key_id: "key-1".into(),
            public_key: [8; 32],
        }),
        Err(AuthorityError::OperationConflict)
    ));
    assert!(matches!(
        store.commit(AuthorityCommand::EnrollIssuer {
            operation_id: "op-2".into(),
            issuer_id: "desktop".into(),
            subject_id: "subject-a".into(),
            principal_id: "agent-b".into(),
            key_id: "key-2".into(),
            public_key: [8; 32],
        }),
        Err(AuthorityError::ImmutableBinding)
    ));
    store
        .commit(AuthorityCommand::RetireSubject {
            operation_id: "op-3".into(),
            subject_id: "subject-a".into(),
        })
        .unwrap();
    assert!(matches!(
        store.commit(AuthorityCommand::EnrollIssuer {
            operation_id: "op-4".into(),
            issuer_id: "other".into(),
            subject_id: "subject-a".into(),
            principal_id: "agent-a".into(),
            key_id: "key-3".into(),
            public_key: [8; 32],
        }),
        Err(AuthorityError::RetiredIdentifier)
    ));
}

#[test]
fn nonce_uniqueness_and_projection_failure_are_journal_authoritative() {
    let home = tempfile::tempdir().unwrap();
    let mut store = bootstrapped(home.path(), snapshot());
    store
        .consume_nonce("nonce-1", "tuple-a", 4_102_444_800)
        .unwrap();
    store
        .consume_nonce("nonce-1", "tuple-a", 4_102_444_800)
        .unwrap();
    assert!(matches!(
        store.consume_nonce("nonce-1", "tuple-b", 4_102_444_800),
        Err(AuthorityError::NonceReplay)
    ));
    let expected = store.snapshot().unwrap();
    drop(store);
    std::fs::remove_file(home.path().join("activation/authority.db")).unwrap();
    let rebuilt = AuthorityStore::open(home.path()).unwrap();
    assert_eq!(rebuilt.snapshot().unwrap(), expected);
    assert!(matches!(
        rebuilt.authorize_unknown_record("future"),
        Err(AuthorityError::UnknownRecord)
    ));
}

#[test]
fn lock_excludes_concurrent_writer() {
    let home = tempfile::tempdir().unwrap();
    let store = bootstrapped(home.path(), snapshot());
    assert!(matches!(
        AuthorityStore::open(home.path()),
        Err(AuthorityError::Contention)
    ));
    drop(store);
    AuthorityStore::open(home.path()).unwrap();
}

#[test]
fn journal_flush_before_projection_rebuilds_after_injected_abort() {
    let home = tempfile::tempdir().unwrap();
    let mut store = bootstrapped(home.path(), snapshot());
    let command = AuthorityCommand::EnrollIssuer {
        operation_id: "crash-op".into(),
        issuer_id: "desktop".into(),
        subject_id: "subject-a".into(),
        principal_id: "agent-a".into(),
        key_id: "key-1".into(),
        public_key: [8; 32],
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.commit_with_fault(command, || panic!("injected after durable journal"))
    }));
    assert!(result.is_err());
    drop(store);
    std::fs::remove_file(home.path().join("activation/authority.db")).unwrap();
    let rebuilt = AuthorityStore::open(home.path()).unwrap();
    assert_eq!(rebuilt.snapshot().unwrap().issuers.len(), 1);
}

#[test]
fn key_roles_are_distinct() {
    assert_ne!(KeyRole::AdminRoot, KeyRole::ReceiptSigner);
    assert_ne!(KeyRole::AdminRoot, KeyRole::LocalCliIssuer);
}
