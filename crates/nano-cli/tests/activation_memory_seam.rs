use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use nano_activation::admission::AdmissionGate;
use nano_activation::authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority};
use nano_activation::journal::AuthorityRecord;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, PolicyCeiling};
use nano_activation::receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner};
use nano_memory::{
    AgentScope, ConfiguredAgents, FactWrite, MemoryPolicy, MemoryStore, RetrieveQuery, SourceTrust,
};
use nano_session::{Op, OpEnvelope, read_journal};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

struct TestReceiptSigner(SigningKey);

impl ReceiptSigner for TestReceiptSigner {
    fn key_id(&self) -> &str {
        "receipt-test-1"
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

fn configured() -> ConfiguredAgents {
    ConfiguredAgents::try_from_ids(std::iter::empty()).unwrap()
}

fn admitted_memory_token(
    home: &std::path::Path,
    idem: &str,
) -> nano_activation::admission::AdmittedToken {
    let issuer = SigningKey::from_bytes(&[1; 32]);
    let receipt = SigningKey::from_bytes(&[9; 32]);
    let mut keys = BTreeMap::new();
    keys.insert(
        "desktop-key-1".into(),
        AuthorityKey {
            public_key: issuer.verifying_key().to_bytes(),
            epoch: 1,
            revoked: false,
        },
    );
    let mut snapshot = AuthoritySnapshot::empty("root", [7; 32])
        .with_service_keys(receipt.verifying_key().to_bytes(), [8; 32]);
    snapshot.issuers.insert(
        "desktop".into(),
        IssuerAuthority {
            subject_id: "subject-a".into(),
            principal_id: "main".into(),
            epoch: 1,
            revoked: false,
            keys,
            projects: BTreeSet::from(["project-a".into()]),
        },
    );
    let activation_root = home.join("activation");
    std::fs::create_dir_all(&activation_root).unwrap();
    let bootstrap_receipt = nano_activation::admin::sign_bootstrap_receipt(
        &snapshot,
        &TestReceiptSigner(SigningKey::from_bytes(&receipt.to_bytes())),
    )
    .unwrap();
    let mut authority = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot,
    })
    .unwrap();
    authority.push(b'\n');
    authority.extend_from_slice(
        &serde_jcs::to_vec(&AuthorityRecord::BootstrapReceipt {
            sequence: 2,
            receipt: String::from_utf8(bootstrap_receipt).unwrap(),
        })
        .unwrap(),
    );
    authority.push(b'\n');
    std::fs::write(activation_root.join("authority.jsonl"), authority).unwrap();

    let mut carrier = serde_json::json!({
        "activation_id":format!("act-{idem}"), "alg":"Ed25519",
        "budgets":{"max_cost_microcents":100,"max_input_tokens":100,"max_output_tokens":100,"max_tool_calls":2,"max_turns":1,"wall_clock_ms":1000},
        "capabilities":["filesystem.read"], "continuity":{"fallback":"none","resume_fingerprint":null,"strategy":"memory_recall"},
        "controls":[], "deadline":"2026-08-30T10:30:00Z", "idempotency_key":idem,
        "issued_at":"2026-08-30T09:59:59Z", "issuer_id":"desktop", "key_id":"desktop-key-1", "nonce":format!("nonce-{idem}"),
        "not_after":"2026-08-30T10:05:00Z", "not_before":"2026-08-30T09:59:55Z", "principal_id":"main",
        "product_subject_id":"subject-a", "project_id":"project-a", "schema":"wayland.nano.activation/v1", "session_id":null
    });
    let payload = serde_jcs::to_vec(&carrier).unwrap();
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    carrier.as_object_mut().unwrap().insert(
        "signature".into(),
        serde_json::json!(URL_SAFE_NO_PAD.encode(issuer.sign(&message).to_bytes())),
    );
    let frame = serde_jcs::to_vec(&serde_json::json!({
        "id":1,"jsonrpc":"2.0","method":"session/new","params":{"_meta":{"waylandNanoActivation":carrier}}
    })).unwrap();
    let mut gate = AdmissionGate::open(
        home,
        Box::new(TestReceiptSigner(receipt)),
        PolicyCeiling {
            capabilities: [EffectiveCapability::FilesystemRead].into(),
            controls: BTreeSet::new(),
            budgets: BudgetLimits {
                max_turns: 10,
                max_tool_calls: 10,
                max_input_tokens: 1000,
                max_output_tokens: 500,
                max_cost_microcents: 1000,
                wall_clock_ms: 5000,
            },
            deadline_utc: "2026-08-30T10:30:00Z".into(),
        },
        ArtifactIdentity {
            source_commit_sha: "0".repeat(40),
            cargo_lock_sha256: "1".repeat(64),
            executable_sha256: "2".repeat(64),
        },
    )
    .unwrap();
    gate.admit_raw(&frame, "2026-08-30T10:00:00Z", None)
        .unwrap()
        .clone()
}

#[test]
fn attributed_policy_record_round_trips_and_legacy_shape_stays_readable() {
    let op = nano_cli::memory_seam::policy_audit_op(
        &MemoryPolicy::default(),
        "project-a",
        "main",
        "session-real",
    );
    let encoded = serde_json::to_string(&OpEnvelope::new("policy-1", "now", op)).unwrap();
    let decoded: OpEnvelope = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(decoded.op, Op::MemoryPolicyResolved {
        project: Some(ref project), agent_id: Some(ref agent), session_id: Some(ref session), ..
    } if project == "project-a" && agent == "main" && session == "session-real"));
    let legacy = r#"{"v":1,"id":"legacy","ts":"now","op":{"type":"memory_policy_resolved","enabled":true,"write":"SessionAndProject","read_scope":"SessionAndProject","episode_cap":1,"fact_cap":1,"byte_cap":1,"deletion":"Never","min_tier":"ModelInference"}}"#;
    let decoded: OpEnvelope = serde_json::from_str(legacy).unwrap();
    assert!(matches!(
        decoded.op,
        Op::MemoryPolicyResolved {
            project: None,
            agent_id: None,
            session_id: None,
            ..
        }
    ));
}

#[test]
fn store_open_validates_but_does_not_emit_a_duplicate_policy_audit() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("memory.jsonl");
    let _store = MemoryStore::open_at(
        &temp.path().join("memory.db"),
        &journal,
        MemoryPolicy::default(),
        "main",
        configured(),
    )
    .unwrap();
    assert!(
        read_journal(&journal)
            .unwrap()
            .envelopes
            .iter()
            .all(|row| !matches!(row.op, Op::MemoryPolicyResolved { .. }))
    );
}

#[test]
fn memory_seam_definitions_expose_only_recall_and_mediated_propose() {
    let names = nano_cli::memory_seam::tool_definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["memory_recall", "memory_propose"]);
    for legacy in ["memory_list", "memory_read", "memory_save", "memory_delete"] {
        assert!(!names.iter().any(|name| name == legacy));
    }
}

#[test]
fn every_runtime_entrypoint_uses_the_ordered_fail_closed_bootstrap() {
    for entrypoint in [
        "acp-new",
        "acp-load",
        "exec-fresh",
        "exec-resume",
        "protocol-host",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let journal_path = temp.path().join(format!("{entrypoint}.jsonl"));
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal_path).unwrap());
        std::fs::write(temp.path().join("memory-policy.toml"), "enabled = true\nwrite = \"SessionAndProject\"\nread_scope = \"SessionAndProject\"\nembedding_backend = \"HashedLocal\"\ndeletion = \"Never\"\nmin_tier = \"User\"\n[retention]\nepisodes = 10\nfacts = 10\nbytes = 4096\n").unwrap();
        let resolved = nano_cli::memory_policy::resolve(temp.path()).unwrap();
        let token = admitted_memory_token(temp.path(), entrypoint);
        let mut seed = MemoryStore::open_at(
            &temp.path().join("memory").join("memory.db"),
            &temp.path().join("memory.jsonl"),
            resolved.policy().clone(),
            "main",
            resolved.configured_agents().clone(),
        )
        .unwrap();
        seed.write_fact(FactWrite {
            id: format!("{entrypoint}-fact"),
            subject: "runtime".into(),
            predicate: "marker".into(),
            object: format!("{entrypoint} continuity needle"),
            confidence: 1.0,
            source_episode: None,
            valid_from: "1".into(),
            valid_to: None,
            source_trust: SourceTrust::User,
            project: "project-a".into(),
            agent_id: "main".into(),
        })
        .unwrap();
        let direct = seed
            .retrieve(&RetrieveQuery {
                text: "continuity needle".into(),
                project: "project-a".into(),
                agent_id: "main".into(),
                agent_scope: AgentScope::Own,
                limit: 10,
                token_budget: 5_000,
                min_tier: SourceTrust::User,
            })
            .unwrap();
        assert_eq!(direct.len(), 1, "seed must be directly retrievable");
        drop(seed);
        coordinator
            .append(&OpEnvelope::new(
                format!("{entrypoint}-begin"),
                "now",
                Op::SessionBegin {
                    session_id: entrypoint.into(),
                    cwd: "workspace".into(),
                },
            ))
            .unwrap();
        let result = nano_cli::memory_seam::start_entrypoint_after_begin(
            temp.path(),
            entrypoint,
            &token,
            &resolved,
            coordinator.clone(),
            || Ok(()),
            |_| {},
        )
        .unwrap();
        let seam = result.expect("memory_recall opens the real store");
        let recall = seam.recall_block("continuity needle").unwrap().unwrap();
        assert!(recall.contains(entrypoint));
        assert_eq!(
            nano_cli::memory_seam::tool_definitions()
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            ["memory_recall", "memory_propose"]
        );
        coordinator
            .append(&OpEnvelope::new(
                format!("{entrypoint}-effect"),
                "now",
                Op::MemoryWriteReceipt {
                    write_id: format!("{entrypoint}-effect"),
                    agent_id: "main".into(),
                    message: "entrypoint effect".into(),
                },
            ))
            .unwrap();
        let rows = read_journal(&journal_path).unwrap().envelopes;
        assert!(
            matches!(rows[0].op, Op::SessionBegin { ref session_id, .. } if session_id == entrypoint)
        );
        assert!(matches!(rows[1].op, Op::MemoryPolicyResolved {
            project: Some(ref project), agent_id: Some(ref agent), session_id: Some(ref session), ..
        } if project == "project-a" && agent == "main" && session == entrypoint));
        assert!(matches!(rows[2].op, Op::MemoryWriteReceipt { .. }));
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row.op, Op::MemoryPolicyResolved { .. }))
                .count(),
            1
        );
    }
}

#[test]
fn every_runtime_entrypoint_refuses_policy_append_failure_before_effect() {
    for entrypoint in [
        "acp-new",
        "acp-load",
        "exec-fresh",
        "exec-resume",
        "protocol-host",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let journal_path = temp.path().join(format!("{entrypoint}.jsonl"));
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal_path).unwrap());
        let resolved = nano_cli::memory_policy::ResolvedMemoryPolicy::disabled();
        let token = admitted_memory_token(temp.path(), &format!("fail-{entrypoint}"));
        coordinator
            .append(&OpEnvelope::new(
                format!("{entrypoint}-begin"),
                "now",
                Op::SessionBegin {
                    session_id: entrypoint.into(),
                    cwd: "workspace".into(),
                },
            ))
            .unwrap();
        let driver_calls = std::cell::Cell::new(0_u32);
        let error = nano_cli::memory_seam::start_entrypoint_after_begin(
            temp.path(),
            entrypoint,
            &token,
            &resolved,
            coordinator,
            || {
                std::fs::remove_file(&journal_path)?;
                Ok(())
            },
            |_| driver_calls.set(driver_calls.get() + 1),
        )
        .unwrap_err();
        assert_eq!(error.kind, nano_session::NanoErrorKind::JournalUnavailable);
        assert_eq!(driver_calls.get(), 0);
        assert!(!temp.path().join("memory").join("memory.db").exists());
        assert!(!journal_path.exists());
    }
}
