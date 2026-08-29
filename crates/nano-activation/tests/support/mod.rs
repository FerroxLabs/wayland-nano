#![allow(dead_code)]

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::admission::AdmissionGate;
use nano_activation::authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority};
use nano_activation::journal::AuthorityRecord;
use nano_activation::policy::{BudgetLimits, EffectiveCapability, EffectiveControl, PolicyCeiling};
use nano_activation::receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct TestSigner(SigningKey);
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

pub fn receipt_key() -> SigningKey {
    SigningKey::from_bytes(&[9; 32])
}
pub fn receipt_signer() -> TestSigner {
    TestSigner(receipt_key())
}

pub fn bootstrap(home: &std::path::Path) -> (SigningKey, TestSigner) {
    let issuer = SigningKey::from_bytes(&[1; 32]);
    let receipt = receipt_key();
    let mut keys = BTreeMap::new();
    keys.insert(
        "desktop-key-1".into(),
        AuthorityKey {
            public_key: issuer.verifying_key().to_bytes(),
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
    (issuer, TestSigner(receipt))
}

pub fn ceiling() -> PolicyCeiling {
    PolicyCeiling {
        capabilities: [EffectiveCapability::FilesystemRead].into(),
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
    }
}
pub fn artifact() -> ArtifactIdentity {
    ArtifactIdentity {
        source_commit_sha: "0".repeat(40),
        cargo_lock_sha256: "1".repeat(64),
        executable_sha256: "2".repeat(64),
    }
}
pub fn artifact_with_exe(value: char) -> ArtifactIdentity {
    let mut artifact = artifact();
    artifact.executable_sha256 = value.to_string().repeat(64);
    artifact
}
pub fn gate(home: &std::path::Path, signer: TestSigner) -> AdmissionGate {
    AdmissionGate::open(home, Box::new(signer), ceiling(), artifact()).unwrap()
}

pub fn signed_frame(key: &SigningKey, idem: &str, nonce: &str, session: Option<&str>) -> Vec<u8> {
    let mut carrier = json!({"activation_id":format!("act-{idem}"),"alg":"Ed25519","budgets":{"max_cost_microcents":100,"max_input_tokens":100,"max_output_tokens":100,"max_tool_calls":2,"max_turns":2,"wall_clock_ms":100},"capabilities":[],"continuity":{"fallback":"none","resume_fingerprint":null,"strategy":"fresh"},"controls":["cancel","pause"],"deadline":"2026-08-30T10:20:00Z","idempotency_key":idem,"issued_at":"2026-08-30T09:59:59Z","issuer_id":"desktop","key_id":"desktop-key-1","nonce":nonce,"not_after":"2026-08-30T10:05:00Z","not_before":"2026-08-30T09:59:55Z","principal_id":"main","product_subject_id":"subject-a","project_id":"project-a","schema":"wayland.nano.activation/v1","session_id":session});
    let payload = serde_jcs::to_vec(&carrier).unwrap();
    let mut message = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    carrier.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&json!({"id":1,"jsonrpc":"2.0","method":"session/new","params":{"_meta":{"waylandNanoActivation":carrier}}})).unwrap()
}

pub fn signed_control(
    key: &SigningKey,
    activation: &str,
    session: &str,
    control: &str,
    nonce: &str,
) -> Vec<u8> {
    let mut value = json!({"activation_id":activation,"alg":"Ed25519","control":control,"issued_at":"2026-08-30T10:00:00Z","issuer_id":"desktop","key_id":"desktop-key-1","nonce":nonce,"not_after":"2026-08-30T10:05:00Z","principal_id":"main","project_id":"project-a","schema":"wayland.nano.control/v1","session_id":session});
    let payload = serde_jcs::to_vec(&value).unwrap();
    let mut message = b"WAYLAND-NANO-CONTROL\0v1\0".to_vec();
    message.extend_from_slice(&payload);
    value.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
    );
    serde_jcs::to_vec(&value).unwrap()
}
