use nano_activation::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot, KeyRole};
use nano_activation::store::AuthorityStore;

fn snapshot() -> AuthoritySnapshot {
    AuthoritySnapshot::empty("root-1", [7; 32])
}

#[test]
fn journal_rebuild_preserves_immutable_authority_and_revocation() {
    let home = tempfile::tempdir().unwrap();
    let mut store = AuthorityStore::bootstrap_for_test(home.path(), snapshot()).unwrap();
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
    assert!(rebuilt
        .authorize("desktop", "desktop-key-1", "subject-a", "agent-a", "project-a")
        .is_err());
    assert!(rebuilt
        .authorize("desktop", "desktop-key-2", "subject-a", "agent-a", "project-a")
        .is_ok());
}

#[test]
fn immutable_bindings_retirement_and_conflicting_duplicates_fail_closed() {
    let home = tempfile::tempdir().unwrap();
    let mut store = AuthorityStore::bootstrap_for_test(home.path(), snapshot()).unwrap();
    let enroll = AuthorityCommand::EnrollIssuer {
        operation_id: "op-1".into(), issuer_id: "desktop".into(), subject_id: "subject-a".into(),
        principal_id: "agent-a".into(), key_id: "key-1".into(), public_key: [8; 32],
    };
    store.commit(enroll.clone()).unwrap();
    store.commit(enroll).unwrap();
    assert!(matches!(store.commit(AuthorityCommand::EnrollIssuer {
        operation_id: "op-1".into(), issuer_id: "desktop".into(), subject_id: "subject-a".into(),
        principal_id: "agent-b".into(), key_id: "key-1".into(), public_key: [8; 32],
    }), Err(AuthorityError::OperationConflict)));
    assert!(matches!(store.commit(AuthorityCommand::EnrollIssuer {
        operation_id: "op-2".into(), issuer_id: "desktop".into(), subject_id: "subject-a".into(),
        principal_id: "agent-b".into(), key_id: "key-2".into(), public_key: [8; 32],
    }), Err(AuthorityError::ImmutableBinding)));
    store.commit(AuthorityCommand::RetireSubject { operation_id: "op-3".into(), subject_id: "subject-a".into() }).unwrap();
    assert!(matches!(store.commit(AuthorityCommand::EnrollIssuer {
        operation_id: "op-4".into(), issuer_id: "other".into(), subject_id: "subject-a".into(),
        principal_id: "agent-a".into(), key_id: "key-3".into(), public_key: [8; 32],
    }), Err(AuthorityError::RetiredIdentifier)));
}

#[test]
fn nonce_uniqueness_and_projection_failure_are_journal_authoritative() {
    let home = tempfile::tempdir().unwrap();
    let mut store = AuthorityStore::bootstrap_for_test(home.path(), snapshot()).unwrap();
    store.consume_nonce("nonce-1", "tuple-a", 4_102_444_800).unwrap();
    store.consume_nonce("nonce-1", "tuple-a", 4_102_444_800).unwrap();
    assert!(matches!(store.consume_nonce("nonce-1", "tuple-b", 4_102_444_800), Err(AuthorityError::NonceReplay)));
    let expected = store.snapshot().unwrap();
    drop(store);
    std::fs::remove_file(home.path().join("activation/authority.db")).unwrap();
    let rebuilt = AuthorityStore::open(home.path()).unwrap();
    assert_eq!(rebuilt.snapshot().unwrap(), expected);
    assert!(matches!(rebuilt.authorize_unknown_record("future"), Err(AuthorityError::UnknownRecord)));
}

#[test]
fn lock_excludes_concurrent_writer() {
    let home = tempfile::tempdir().unwrap();
    let store = AuthorityStore::bootstrap_for_test(home.path(), snapshot()).unwrap();
    assert!(matches!(AuthorityStore::open(home.path()), Err(AuthorityError::Contention)));
    drop(store);
    AuthorityStore::open(home.path()).unwrap();
}

#[test]
fn key_roles_are_distinct() {
    assert_ne!(KeyRole::AdminRoot, KeyRole::ReceiptSigner);
    assert_ne!(KeyRole::AdminRoot, KeyRole::LocalCliIssuer);
}
