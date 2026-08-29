use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use nano_activation::RejectReason;
use nano_activation::admission::AdmissionGate;
use nano_activation::authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority};
use nano_activation::journal::AuthorityRecord;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, EffectiveControl, PolicyCeiling};
use nano_activation::receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner};
use nano_cli::activation::{SharedAdmission, TransportAdmission};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

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

#[test]
fn raw_admission_precedes_transport_serde_and_side_effects() {
    let home = tempfile::tempdir().unwrap();
    let issuer = SigningKey::from_bytes(&[1; 32]);
    let receipt = SigningKey::from_bytes(&[9; 32]);
    bootstrap(home.path(), &issuer, &receipt);
    let gate = SharedAdmission::from_gate(
        AdmissionGate::open(
            home.path(),
            Box::new(TestReceiptSigner(receipt)),
            ceiling(),
            artifact(),
        )
        .unwrap(),
    );

    let valid = signed_frame(&issuer, "nonce-good", "idem-good");
    let admitted = gate
        .admit_transport(&valid, "2026-08-30T10:00:00Z")
        .unwrap();
    let TransportAdmission::Activation(token) = admitted else {
        panic!("activation token")
    };
    assert_eq!(token.principal_id(), "main");
    assert!(serde_json::from_slice::<serde_json::Value>(token.receipt().as_bytes()).is_ok());
    let fingerprint = gate.bind_session(&token, "session-a").unwrap();
    let resumed = gate
        .admit_transport(
            &signed_resume_frame(&issuer, &fingerprint),
            "2026-08-30T10:00:00Z",
        )
        .unwrap();
    let TransportAdmission::Activation(resumed) = resumed else {
        panic!("resume token")
    };
    assert_eq!(resumed.session_id(), Some("session-a"));

    let duplicate = br#"{"id":2,"id":3,"jsonrpc":"2.0","method":"session/new","params":{"_meta":{"waylandNanoActivation":{}}}}"#;
    assert_eq!(
        gate.admit_transport(duplicate, "2026-08-30T10:00:00Z")
            .unwrap_err()
            .reason(),
        RejectReason::DuplicateKey,
    );
    let oversized = vec![b' '; 32 * 1024 + 1];
    assert_eq!(
        gate.admit_transport(&oversized, "2026-08-30T10:00:00Z")
            .unwrap_err()
            .reason(),
        RejectReason::CarrierOversized,
    );
    assert!(!home.path().join("sessions").exists());
    assert!(!home.path().join("hook-canary").exists());
    assert!(!home.path().join("tool-canary").exists());
    assert!(!home.path().join("fire-canary").exists());
}

fn ceiling() -> PolicyCeiling {
    PolicyCeiling {
        capabilities: [EffectiveCapability::FilesystemRead].into(),
        controls: [EffectiveControl::Cancel, EffectiveControl::Pause].into(),
        budgets: BudgetLimits {
            max_turns: 10,
            max_tool_calls: 10,
            max_input_tokens: 1000,
            max_output_tokens: 500,
            max_cost_microcents: 1000,
            wall_clock_ms: 5000,
        },
        deadline_utc: "2026-08-30T10:30:00Z".into(),
    }
}

fn artifact() -> ArtifactIdentity {
    ArtifactIdentity {
        source_commit_sha: "0".repeat(40),
        cargo_lock_sha256: "1".repeat(64),
        executable_sha256: "2".repeat(64),
    }
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

fn signed_frame(key: &SigningKey, nonce: &str, idem: &str) -> Vec<u8> {
    signed_frame_with(key, nonce, idem, "session/new", "fresh", None, None)
}

fn signed_resume_frame(key: &SigningKey, fingerprint: &str) -> Vec<u8> {
    signed_frame_with(
        key,
        "nonce-resume",
        "idem-resume",
        "session/load",
        "session_resume",
        Some("session-a"),
        Some(fingerprint),
    )
}

fn signed_frame_with(
    key: &SigningKey,
    nonce: &str,
    idem: &str,
    method: &str,
    strategy: &str,
    session_id: Option<&str>,
    fingerprint: Option<&str>,
) -> Vec<u8> {
    let mut carrier = json!({
        "activation_id":format!("act-{idem}"),"alg":"Ed25519",
        "budgets":{"max_cost_microcents":100,"max_input_tokens":100,"max_output_tokens":100,"max_tool_calls":2,"max_turns":1,"wall_clock_ms":1000},
        "capabilities":["filesystem.read"],"continuity":{"fallback":"none","resume_fingerprint":fingerprint,"strategy":strategy},
        "controls":["cancel","pause"],"deadline":"2026-08-30T10:20:00Z","idempotency_key":idem,
        "issued_at":"2026-08-30T09:59:59Z","issuer_id":"desktop","key_id":"desktop-key-1","nonce":nonce,
        "not_after":"2026-08-30T10:05:00Z","not_before":"2026-08-30T09:59:55Z","principal_id":"main",
        "product_subject_id":"subject-a","project_id":"project-a","schema":"wayland.nano.activation/v1","session_id":session_id
    });
    let payload = serde_jcs::to_vec(&carrier).unwrap();
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    carrier.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&json!({"id":1,"jsonrpc":"2.0","method":method,"params":{"sessionId":session_id,"_meta":{"waylandNanoActivation":carrier}}})).unwrap()
}
