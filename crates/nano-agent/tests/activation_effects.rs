use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use nano_activation::{
    admission::AdmissionGate,
    authority::{AuthorityKey, AuthoritySnapshot, IssuerAuthority},
    enablement::{EnablementCommand, EnablementFault, EnablementStore},
    journal::AuthorityRecord,
    policy::{BudgetLimits, EffectiveCapability, PolicyCeiling},
    receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner},
};
use nano_agent::{
    activation_effects::{ActivationEffectExecutor, EffectFault},
    loop_protection::ProgressSignals,
    turn::{ToolExecutor, ToolOutcome},
};
use nano_model::types::ToolCall;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone)]
struct TestReceiptSigner(SigningKey);
impl ReceiptSigner for TestReceiptSigner {
    fn key_id(&self) -> &str {
        "receipt"
    }
    fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }
    fn preflight(&self) -> Result<(), ReceiptError> {
        Ok(())
    }
    fn sign(&self, m: &[u8]) -> Result<[u8; 64], ReceiptError> {
        Ok(self.0.sign(m).to_bytes())
    }
}
#[derive(Debug)]
struct Counter(Arc<AtomicUsize>);
#[async_trait]
impl ToolExecutor for Counter {
    async fn execute(&self, _: &ToolCall) -> ToolOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        ToolOutcome {
            ok: true,
            output: "done".into(),
            progress: ProgressSignals::default(),
            error_kind: None,
        }
    }
}

#[tokio::test]
async fn authorized_effect_is_journaled_and_ambiguous_effect_never_redispatches() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, root, receipt) = bootstrap(home.path());
    let artifact = artifact();
    let mut gate = AdmissionGate::open(
        home.path(),
        Box::new(TestReceiptSigner(receipt)),
        ceiling(),
        artifact.clone(),
    )
    .unwrap();
    let token = gate
        .admit_raw(&frame(&issuer), "2026-08-30T10:00:00Z", None)
        .unwrap();
    let enable = EnablementStore::open(home.path()).unwrap();
    let command = EnablementCommand {
        operation_id: "enable-1".into(),
        enabled: true,
        artifact: artifact.clone(),
        admin_epoch: 1,
        issuer_epoch: 1,
        grant_epoch: 1,
        revocation_epoch: 1,
        not_after: "2026-08-30T10:05:00Z".into(),
    };
    enable
        .apply_signed(
            &admin(&root, &enable, &command),
            &command,
            "2026-08-30T10:00:00Z",
            EnablementFault::None,
        )
        .unwrap();
    drop(enable);
    let count = Arc::new(AtomicUsize::new(0));
    let call = ToolCall {
        id: "call-1".into(),
        name: "fs_read".into(),
        arguments: json!({"path":"README.md"}),
    };
    let executor = ActivationEffectExecutor::new(
        Counter(count.clone()),
        token,
        home.path(),
        artifact,
        [1; 4],
        "2026-08-30T10:00:00Z",
    )
    .with_fault(EffectFault::AfterDispatch);
    assert!(!executor.execute(&call).await.ok);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(!executor.execute(&call).await.ok);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let journal = std::fs::read_to_string(home.path().join("activation/effects.jsonl")).unwrap();
    assert!(journal.contains("\"state\":\"intent\""));
    assert!(journal.contains("\"state\":\"unknown_outcome\""));
}

#[tokio::test]
async fn missing_enablement_and_capability_refuse_before_dispatch() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, _root, receipt) = bootstrap(home.path());
    let artifact = artifact();
    let mut gate = AdmissionGate::open(
        home.path(),
        Box::new(TestReceiptSigner(receipt)),
        ceiling(),
        artifact.clone(),
    )
    .unwrap();
    let token = gate
        .admit_raw(&frame(&issuer), "2026-08-30T10:00:00Z", None)
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let executor = ActivationEffectExecutor::new(
        Counter(count.clone()),
        token,
        home.path(),
        artifact,
        [1; 4],
        "2026-08-30T10:00:00Z",
    );
    let call = ToolCall {
        id: "call-2".into(),
        name: "fs_write".into(),
        arguments: json!({}),
    };
    assert!(!executor.execute(&call).await.ok);
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

fn artifact() -> ArtifactIdentity {
    ArtifactIdentity {
        source_commit_sha: "0".repeat(40),
        cargo_lock_sha256: "1".repeat(64),
        executable_sha256: "2".repeat(64),
    }
}
fn ceiling() -> PolicyCeiling {
    PolicyCeiling {
        capabilities: [EffectiveCapability::FilesystemRead].into(),
        controls: BTreeSet::new(),
        budgets: BudgetLimits {
            max_turns: 2,
            max_tool_calls: 2,
            max_input_tokens: 100,
            max_output_tokens: 100,
            max_cost_microcents: 100,
            wall_clock_ms: 100,
        },
        deadline_utc: "2026-08-30T10:10:00Z".into(),
    }
}
fn bootstrap(home: &std::path::Path) -> (SigningKey, SigningKey, SigningKey) {
    let issuer = SigningKey::from_bytes(&[1; 32]);
    let root = SigningKey::from_bytes(&[7; 32]);
    let receipt = SigningKey::from_bytes(&[9; 32]);
    let mut keys = BTreeMap::new();
    keys.insert(
        "key".into(),
        AuthorityKey {
            public_key: issuer.verifying_key().to_bytes(),
            epoch: 1,
            revoked: false,
        },
    );
    let mut projects = BTreeSet::new();
    projects.insert("project".into());
    let mut snapshot = AuthoritySnapshot::empty("root", root.verifying_key().to_bytes())
        .with_service_keys(receipt.verifying_key().to_bytes(), [8; 32]);
    snapshot.issuers.insert(
        "desktop".into(),
        IssuerAuthority {
            subject_id: "subject".into(),
            principal_id: "main".into(),
            epoch: 1,
            revoked: false,
            keys,
            projects,
        },
    );
    let dir = home.join("activation");
    std::fs::create_dir_all(&dir).unwrap();
    let mut bytes = serde_jcs::to_vec(&AuthorityRecord::Bootstrap {
        sequence: 1,
        snapshot,
    })
    .unwrap();
    bytes.push(b'\n');
    std::fs::write(dir.join("authority.jsonl"), bytes).unwrap();
    (issuer, root, receipt)
}
fn frame(key: &SigningKey) -> Vec<u8> {
    let mut c = json!({"activation_id":"activation-1","alg":"Ed25519","budgets":{"max_cost_microcents":100,"max_input_tokens":100,"max_output_tokens":100,"max_tool_calls":2,"max_turns":2,"wall_clock_ms":100},"capabilities":["filesystem.read"],"continuity":{"fallback":"none","resume_fingerprint":null,"strategy":"fresh"},"controls":[],"deadline":"2026-08-30T10:05:00Z","idempotency_key":"idem-1","issued_at":"2026-08-30T09:59:59Z","issuer_id":"desktop","key_id":"key","nonce":"nonce-1","not_after":"2026-08-30T10:05:00Z","not_before":"2026-08-30T09:59:00Z","principal_id":"main","product_subject_id":"subject","project_id":"project","schema":"wayland.nano.activation/v1","session_id":null});
    let payload = serde_jcs::to_vec(&c).unwrap();
    let mut m = b"WAYLAND-NANO-ACTIVATION\0v1\0".to_vec();
    m.extend(payload);
    c.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&m).to_bytes())),
    );
    serde_jcs::to_vec(&json!({"id":1,"jsonrpc":"2.0","method":"session/new","params":{"_meta":{"waylandNanoActivation":c}}})).unwrap()
}
fn admin(key: &SigningKey, store: &EnablementStore, c: &EnablementCommand) -> Vec<u8> {
    let mut v = json!({"admin_epoch":1,"admin_id":"root","after_digest":c.digest(),"alg":"Ed25519","before_digest":store.state_digest().unwrap(),"issued_at":"2026-08-30T09:59:59Z","key_id":"root-key","nonce":"enable-nonce","not_after":"2026-08-30T10:05:00Z","operation":"enable_artifact","operation_id":c.operation_id,"reason":"test","schema":"wayland.nano.admin-request/v1"});
    let p = serde_jcs::to_vec(&v).unwrap();
    let mut m = b"WAYLAND-NANO-ADMIN\0v1\0".to_vec();
    m.extend(p);
    v.as_object_mut().unwrap().insert(
        "signature".into(),
        json!(URL_SAFE_NO_PAD.encode(key.sign(&m).to_bytes())),
    );
    serde_jcs::to_vec(&v).unwrap()
}
