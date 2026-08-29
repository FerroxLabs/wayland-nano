//! Canonical, separately signed activation receipts and offline verification.

use crate::{ActivationError, RejectReason, verify_receipt};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const RECEIPT_DOMAIN: &[u8] = b"WAYLAND-NANO-RECEIPT\0v1\0";

pub trait ReceiptSigner: Send + Sync {
    fn key_id(&self) -> &str;
    fn public_key(&self) -> [u8; 32];
    fn preflight(&self) -> Result<(), ReceiptError>;
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], ReceiptError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIdentity {
    pub source_commit_sha: String,
    pub cargo_lock_sha256: String,
    pub executable_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Admitted,
    Refused,
    Replayed,
    ControlAccepted,
    ControlRaceLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentState {
    Journaled,
    DispatchEligible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultState {
    Pending,
    Completed,
    UnknownOutcome,
    Cancelled,
    Paused,
}

#[derive(Debug, Clone)]
pub(crate) struct ReceiptFields<'a> {
    pub receipt_id: &'a str,
    pub issuer_id: &'a str,
    pub key_id: &'a str,
    pub product_subject_id: &'a str,
    pub principal_id: &'a str,
    pub project_id: &'a str,
    pub activation_id: &'a str,
    pub idempotency_key: &'a str,
    pub session_id: Option<&'a str>,
    pub raw_assertion_sha256: &'a str,
    pub canonical_payload_sha256: &'a str,
    pub decision: Decision,
    pub reason: RejectReason,
    pub effective_policy: &'a crate::policy::EffectivePolicy,
    pub intent_state: IntentState,
    pub result_state: ResultState,
    pub authority_journal_position: u64,
    pub activation_journal_position: u64,
    pub admin_epoch: u64,
    pub issuer_epoch: u64,
    pub grant_epoch: u64,
    pub revocation_epoch: u64,
    pub issued_at: &'a str,
    pub artifact: &'a ArtifactIdentity,
    pub control: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedReceipt {
    canonical: Vec<u8>,
}

impl SignedReceipt {
    pub fn as_bytes(&self) -> &[u8] {
        &self.canonical
    }

    pub fn verify_offline(&self, public_key: &[u8; 32]) -> Result<(), ActivationError> {
        verify_receipt(&self.canonical, public_key)
    }

    pub(crate) fn from_stored(canonical: Vec<u8>) -> Self {
        Self { canonical }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiptError {
    #[error("receipt signer is unavailable")]
    SignerUnavailable,
    #[error("receipt signer returned invalid output")]
    InvalidSignerOutput,
    #[error("receipt canonicalization failed")]
    Canonicalization,
}

pub(crate) fn mint(
    signer: &dyn ReceiptSigner,
    fields: ReceiptFields<'_>,
) -> Result<SignedReceipt, ReceiptError> {
    signer.preflight()?;
    let mut value = json!({
        "activation_id": fields.activation_id,
        "activation_journal_position": fields.activation_journal_position,
        "admin_epoch": fields.admin_epoch,
        "alg": "Ed25519",
        "authority_journal_position": fields.authority_journal_position,
        "canonical_payload_sha256": fields.canonical_payload_sha256,
        "cargo_lock_sha256": fields.artifact.cargo_lock_sha256,
        "decision": fields.decision,
        "control": fields.control,
        "effective_policy": fields.effective_policy,
        "executable_sha256": fields.artifact.executable_sha256,
        "grant_epoch": fields.grant_epoch,
        "idempotency_key": fields.idempotency_key,
        "intent_state": fields.intent_state,
        "issued_at": fields.issued_at,
        "issuer_epoch": fields.issuer_epoch,
        "issuer_id": fields.issuer_id,
        "key_id": fields.key_id,
        "principal_id": fields.principal_id,
        "product_subject_id": fields.product_subject_id,
        "project_id": fields.project_id,
        "raw_assertion_sha256": fields.raw_assertion_sha256,
        "reason": fields.reason,
        "receipt_id": fields.receipt_id,
        "receipt_key_id": signer.key_id(),
        "result_state": fields.result_state,
        "revocation_epoch": fields.revocation_epoch,
        "schema": "wayland.nano.activation-receipt/v1",
        "session_id": fields.session_id,
        "source_commit_sha": fields.artifact.source_commit_sha,
    });
    let payload = serde_jcs::to_vec(&value).map_err(|_| ReceiptError::Canonicalization)?;
    let mut message = Vec::with_capacity(RECEIPT_DOMAIN.len() + payload.len());
    message.extend_from_slice(RECEIPT_DOMAIN);
    message.extend_from_slice(&payload);
    let signature = signer.sign(&message)?;
    value
        .as_object_mut()
        .ok_or(ReceiptError::Canonicalization)?
        .insert(
            "signature".into(),
            Value::String(URL_SAFE_NO_PAD.encode(signature)),
        );
    let canonical = serde_jcs::to_vec(&value).map_err(|_| ReceiptError::Canonicalization)?;
    verify_receipt(&canonical, &signer.public_key())
        .map_err(|_| ReceiptError::InvalidSignerOutput)?;
    Ok(SignedReceipt { canonical })
}
