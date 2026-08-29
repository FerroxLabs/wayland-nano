use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::RejectReason;
use nano_activation::admission::{AdmissionGate, ResumeBinding};
use nano_activation::authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority};
use nano_activation::journal::AuthorityRecord;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, EffectiveControl, PolicyCeiling};
use nano_activation::receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

struct TestSigner(SigningKey);

impl ReceiptSigner for TestSigner {
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

#[test]
fn admission_is_closed_narrowed_scoped_and_replay_deterministic() {
    let home = tempfile::tempdir().unwrap();
    let issuer = SigningKey::from_bytes(&[1; 32]);
    let receipt = SigningKey::from_bytes(&[9; 32]);
    bootstrap(home.path(), &issuer, &receipt);
    let mut gate = gate(home.path(), receipt);
    let raw = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-a",
            nonce: "nonce-a",
            capabilities: &["filesystem.read", "shell.execute"],
            max_turns: 64,
            ..FrameSpec::default()
        },
    );
    let token = gate.admit_raw(&raw, "2026-08-30T10:00:00Z", None).unwrap();
    assert_eq!(token.principal_id(), "main");
    assert_eq!(token.project_id(), "project-a");
    assert_eq!(token.policy().agent_scope(), "own");
    assert_eq!(token.policy().read_scope(), "session_and_project");
    assert_eq!(token.policy().budgets().max_turns, 10);
    token.receipt().verify_offline(&[0; 32]).unwrap_err();
    let first = token.receipt().as_bytes().to_vec();
    let receipt_json: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(
        receipt_json["effective_policy"]["capabilities"][0],
        "filesystem.read"
    );
    let replay = gate.admit_raw(&raw, "2026-08-30T10:00:00Z", None).unwrap();
    assert_eq!(replay.receipt().as_bytes(), first);

    let widening = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-b",
            nonce: "nonce-b",
            capabilities: &["network.egress"],
            ..FrameSpec::default()
        },
    );
    assert_eq!(
        gate.admit_raw(&widening, "2026-08-30T10:00:00Z", None)
            .unwrap_err()
            .reason(),
        RejectReason::AuthorityWidening
    );
    let recall = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-c",
            nonce: "nonce-c",
            strategy: "memory_recall",
            ..FrameSpec::default()
        },
    );
    assert_eq!(
        gate.admit_raw(&recall, "2026-08-30T10:00:00Z", None)
            .unwrap_err()
            .reason(),
        RejectReason::ContinuityNotEnabled
    );
    let wrong_project = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-d",
            nonce: "nonce-d",
            project: "project-b",
            ..FrameSpec::default()
        },
    );
    assert_eq!(
        gate.admit_raw(&wrong_project, "2026-08-30T10:00:00Z", None)
            .unwrap_err()
            .reason(),
        RejectReason::UnauthorizedProject
    );
}

#[test]
fn nonce_precedes_tuple_and_resume_requires_exact_binding() {
    let home = tempfile::tempdir().unwrap();
    let issuer = SigningKey::from_bytes(&[1; 32]);
    let receipt = SigningKey::from_bytes(&[9; 32]);
    bootstrap(home.path(), &issuer, &receipt);
    let mut gate = gate(home.path(), receipt);
    let first = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-a",
            nonce: "shared-nonce",
            ..FrameSpec::default()
        },
    );
    gate.admit_raw(&first, "2026-08-30T10:00:00Z", None)
        .unwrap();
    let changed = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-b",
            nonce: "shared-nonce",
            ..FrameSpec::default()
        },
    );
    assert_eq!(
        gate.admit_raw(&changed, "2026-08-30T10:00:00Z", None)
            .unwrap_err()
            .reason(),
        RejectReason::NonceReplay
    );

    let fingerprint = "a".repeat(64);
    let resume_raw = signed_frame(
        &issuer,
        FrameSpec {
            idem: "idem-resume",
            nonce: "nonce-resume",
            strategy: "session_resume",
            session: Some("session-a"),
            fingerprint: Some(&fingerprint),
            ..FrameSpec::default()
        },
    );
    let binding = ResumeBinding {
        issuer_id: "desktop".into(),
        product_subject_id: "subject-a".into(),
        principal_id: "main".into(),
        project_id: "project-a".into(),
        session_id: "session-a".into(),
        fingerprint,
        admin_epoch: 1,
        issuer_epoch: 1,
    };
    gate.admit_raw(&resume_raw, "2026-08-30T10:00:00Z", Some(&binding))
        .unwrap();
}

fn gate(home: &std::path::Path, receipt: SigningKey) -> AdmissionGate {
    AdmissionGate::open(
        home,
        Box::new(TestSigner(receipt)),
        PolicyCeiling {
            capabilities: [
                EffectiveCapability::FilesystemRead,
                EffectiveCapability::ShellExecute,
            ]
            .into(),
            controls: [EffectiveControl::Cancel, EffectiveControl::Pause].into(),
            budgets: BudgetLimits {
                max_turns: 10,
                max_tool_calls: 20,
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
    .unwrap()
}

fn bootstrap(home: &std::path::Path, issuer_key: &SigningKey, receipt: &SigningKey) {
    let mut keys = BTreeMap::new();
    keys.insert(
        "desktop-key-1".into(),
        AuthorityKey {
            public_key: issuer_key.verifying_key().to_bytes(),
            epoch: 1,
            revoked: false,
        },
    );
    let mut projects = BTreeSet::new();
    projects.insert("project-a".into());
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
            projects,
        },
    );
    let root = home.join("activation");
    std::fs::create_dir_all(&root).unwrap();
    let mut bytes = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot,
    })
    .unwrap();
    bytes.push(b'\n');
    std::fs::write(root.join("authority.jsonl"), bytes).unwrap();
}

struct FrameSpec<'a> {
    idem: &'a str,
    nonce: &'a str,
    strategy: &'a str,
    session: Option<&'a str>,
    fingerprint: Option<&'a str>,
    capabilities: &'a [&'a str],
    max_turns: u64,
    project: &'a str,
}

impl Default for FrameSpec<'static> {
    fn default() -> Self {
        Self {
            idem: "idem",
            nonce: "nonce",
            strategy: "fresh",
            session: None,
            fingerprint: None,
            capabilities: &[],
            max_turns: 1,
            project: "project-a",
        }
    }
}

fn signed_frame(key: &SigningKey, spec: FrameSpec<'_>) -> Vec<u8> {
    let mut carrier = json!({
        "activation_id": format!("act-{}", spec.idem), "alg":"Ed25519",
        "budgets":{"max_cost_microcents":2000,"max_input_tokens":2000,"max_output_tokens":1000,"max_tool_calls":30,"max_turns":spec.max_turns,"wall_clock_ms":10000},
        "capabilities":spec.capabilities, "continuity":{"fallback":"none","resume_fingerprint":spec.fingerprint,"strategy":spec.strategy},
        "controls":["cancel","pause"], "deadline":"2026-08-30T10:20:00Z", "idempotency_key":spec.idem,
        "issued_at":"2026-08-30T09:59:59Z", "issuer_id":"desktop", "key_id":"desktop-key-1", "nonce":spec.nonce,
        "not_after":"2026-08-30T10:05:00Z", "not_before":"2026-08-30T09:59:55Z", "principal_id":"main",
        "product_subject_id":"subject-a", "project_id":spec.project, "schema":"wayland.nano.activation/v1", "session_id":spec.session
    });
    let payload = serde_jcs::to_vec(&carrier).unwrap();
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    carrier.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&json!({"id":1,"jsonrpc":"2.0","method":"session/new","params":{"_meta":{"waylandNanoActivation":carrier}}})).unwrap()
}
