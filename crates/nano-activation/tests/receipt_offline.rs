mod support;

use nano_activation::admission::AdmissionGate;
use nano_activation::receipt::{ReceiptError, ReceiptSigner};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use support::{artifact, bootstrap, ceiling, signed_frame};

struct Unavailable;
impl ReceiptSigner for Unavailable {
    fn key_id(&self) -> &str {
        "receipt-test-1"
    }
    fn public_key(&self) -> [u8; 32] {
        support::receipt_key().verifying_key().to_bytes()
    }
    fn preflight(&self) -> Result<(), ReceiptError> {
        Err(ReceiptError::SignerUnavailable)
    }
    fn sign(&self, _: &[u8]) -> Result<[u8; 64], ReceiptError> {
        Err(ReceiptError::SignerUnavailable)
    }
}

#[test]
fn signer_unavailable_refuses_before_activation_ledger_exists() {
    let home = tempfile::tempdir().unwrap();
    bootstrap(home.path());
    assert!(
        AdmissionGate::open(home.path(), Box::new(Unavailable), ceiling(), artifact()).is_err()
    );
    assert!(!home.path().join("activation/admission.jsonl").exists());
}

#[test]
fn receipt_is_canonical_offline_verifiable_and_mutation_fails() {
    let home = tempfile::tempdir().unwrap();
    let (issuer, receipt) = bootstrap(home.path());
    let raw = signed_frame(&issuer, "idem-r", "nonce-r", None);
    let mut gate = support::gate(home.path(), receipt.clone());
    let admitted = gate.admit_raw(&raw, "2026-08-30T10:00:00Z", None).unwrap();
    admitted
        .receipt()
        .verify_offline(&receipt.public_key())
        .unwrap();
    let mut mutated = admitted.receipt().as_bytes().to_vec();
    let index = mutated.iter().position(|byte| *byte == b'a').unwrap();
    mutated[index] = b'b';
    assert!(nano_activation::verify_receipt(&mutated, &receipt.public_key()).is_err());
}

#[test]
fn refusal_receipt_is_canonical_typed_safe_and_offline_verifiable() {
    let home = tempfile::tempdir().unwrap();
    let (_issuer, signer) = bootstrap(home.path());
    let raw = br#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}"#;
    let mut gate = support::gate(home.path(), signer.clone());
    let refusal = gate
        .admit_raw_with_receipt(raw, "2026-08-30T10:00:00Z", None)
        .unwrap_err();

    assert_eq!(
        refusal.reason(),
        nano_activation::RejectReason::CarrierMissing
    );
    assert_eq!(
        refusal.kind(),
        nano_session::NanoErrorKind::ActivationCarrierMissing
    );
    let receipt = refusal
        .receipt()
        .expect("available signer must sign refusal");
    receipt.verify_offline(&signer.public_key()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(receipt.as_bytes()).unwrap();
    assert_eq!(value["decision"], "refused");
    assert_eq!(value["reason"], "carrier_missing");
    assert_eq!(serde_jcs::to_vec(&value).unwrap(), receipt.as_bytes());
    let rendered = String::from_utf8_lossy(receipt.as_bytes());
    assert!(!rendered.contains("waylandNanoActivation"));
    assert!(!rendered.contains("receipt.seed"));
    assert!(!rendered.contains("\\") && !rendered.contains(":/"));
}

#[derive(Clone)]
struct SwitchableSigner {
    inner: support::TestSigner,
    available: Arc<AtomicBool>,
}

impl ReceiptSigner for SwitchableSigner {
    fn key_id(&self) -> &str {
        self.inner.key_id()
    }
    fn public_key(&self) -> [u8; 32] {
        self.inner.public_key()
    }
    fn preflight(&self) -> Result<(), ReceiptError> {
        if self.available.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ReceiptError::SignerUnavailable)
        }
    }
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], ReceiptError> {
        self.preflight()?;
        self.inner.sign(message)
    }
}

#[test]
fn signer_loss_fails_closed_without_unsigned_refusal() {
    let home = tempfile::tempdir().unwrap();
    bootstrap(home.path());
    let available = Arc::new(AtomicBool::new(true));
    let signer = SwitchableSigner {
        inner: support::receipt_signer(),
        available: Arc::clone(&available),
    };
    let mut gate =
        AdmissionGate::open(home.path(), Box::new(signer), ceiling(), artifact()).unwrap();
    available.store(false, Ordering::SeqCst);
    let refusal = gate
        .admit_raw_with_receipt(b"{}", "2026-08-30T10:00:00Z", None)
        .unwrap_err();
    assert_eq!(
        refusal.reason(),
        nano_activation::RejectReason::AuthorityStoreUnavailable
    );
    assert_eq!(
        refusal.kind(),
        nano_session::NanoErrorKind::ActivationAuthorityStoreUnavailable
    );
    assert!(refusal.receipt().is_none());
}
