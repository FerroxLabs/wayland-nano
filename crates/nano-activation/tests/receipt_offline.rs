mod support;

use nano_activation::admission::AdmissionGate;
use nano_activation::receipt::{ReceiptError, ReceiptSigner};
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
