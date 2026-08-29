//! Byte-preserving contract boundary for Wayland Nano activation messages.
//!
//! It validates raw transport ambiguity, the frozen carrier, JCS and Ed25519 proof,
//! then narrows admitted authority through one durable, fail-closed gate.

pub mod admin;
pub mod admission;
pub mod authority;
pub mod control;
pub mod journal;
pub mod key_provider;
pub mod policy;
mod raw;
pub mod receipt;
pub mod store;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fmt};

const ACTIVATION_DOMAIN: &[u8] = b"WAYLAND-NANO-ACTIVATION\0v1\0";
const CONTROL_DOMAIN: &[u8] = b"WAYLAND-NANO-CONTROL\0v1\0";
const ADMIN_DOMAIN: &[u8] = b"WAYLAND-NANO-ADMIN\0v1\0";
const RECEIPT_DOMAIN: &[u8] = b"WAYLAND-NANO-RECEIPT\0v1\0";
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    None,
    CarrierMissing,
    CarrierOversized,
    MalformedJson,
    DuplicateKey,
    NoncanonicalPayload,
    UnknownField,
    UnsupportedSchema,
    UnsupportedAlgorithm,
    InvalidKeyEncoding,
    InvalidSignatureEncoding,
    InvalidSignature,
    UnknownIssuer,
    RevokedIssuer,
    UnknownKey,
    RevokedKey,
    KeyNotYetValid,
    KeyExpired,
    AssertionNotYetValid,
    AssertionExpired,
    ClockOutOfBounds,
    NonceReplay,
    IdempotencyConflict,
    UnknownProductSubject,
    RetiredProductSubject,
    PrincipalMismatch,
    PrincipalRemap,
    RetiredIdentifierReuse,
    UnauthorizedProject,
    AuthorityWidening,
    ArtifactMismatch,
    ResumeFingerprintMissing,
    ResumeDrift,
    FallbackUnauthorized,
    ContinuityNotEnabled,
    ControlUnauthorized,
    ControlRaceLost,
    AuthorityStoreUnavailable,
    AmbiguousRecovery,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let encoded = serde_json::to_value(self).map_err(|_| fmt::Error)?;
        formatter.write_str(encoded.as_str().ok_or(fmt::Error)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationError {
    reason: RejectReason,
}

impl ActivationError {
    pub(crate) fn new(reason: RejectReason) -> Self {
        Self { reason }
    }

    pub fn reason(&self) -> RejectReason {
        self.reason
    }
}

impl fmt::Display for ActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "activation refused: {}", self.reason)
    }
}

impl std::error::Error for ActivationError {}

#[derive(Debug, Clone)]
pub struct AdmittedActivation {
    carrier: ActivationCarrier,
    canonical_payload_sha256: String,
}

impl AdmittedActivation {
    pub fn principal_id(&self) -> &str {
        &self.carrier.principal_id
    }

    pub fn project_id(&self) -> &str {
        &self.carrier.project_id
    }

    pub fn canonical_payload_sha256(&self) -> &str {
        &self.canonical_payload_sha256
    }

    pub(crate) fn carrier(&self) -> &ActivationCarrier {
        &self.carrier
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivationCarrier {
    pub(crate) schema: String,
    pub(crate) issuer_id: String,
    pub(crate) key_id: String,
    pub(crate) alg: String,
    pub(crate) issued_at: String,
    pub(crate) not_before: String,
    pub(crate) not_after: String,
    pub(crate) nonce: String,
    pub(crate) product_subject_id: String,
    pub(crate) principal_id: String,
    pub(crate) project_id: String,
    pub(crate) activation_id: String,
    pub(crate) idempotency_key: String,
    pub(crate) session_id: Option<String>,
    pub(crate) continuity: Continuity,
    pub(crate) capabilities: Vec<Capability>,
    pub(crate) budgets: Budgets,
    pub(crate) deadline: String,
    pub(crate) controls: Vec<Control>,
    pub(crate) signature: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Continuity {
    pub(crate) strategy: ContinuityStrategy,
    pub(crate) fallback: Fallback,
    pub(crate) resume_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContinuityStrategy {
    Fresh,
    SessionResume,
    MemoryRecall,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Fallback {
    None,
    Fresh,
    MemoryRecall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub(crate) enum Capability {
    #[serde(rename = "filesystem.read")]
    FilesystemRead,
    #[serde(rename = "filesystem.write")]
    FilesystemWrite,
    #[serde(rename = "shell.execute")]
    ShellExecute,
    #[serde(rename = "network.egress")]
    NetworkEgress,
    #[serde(rename = "mcp.invoke")]
    McpInvoke,
    #[serde(rename = "task.spawn")]
    TaskSpawn,
    #[serde(rename = "checkpoint.mutate")]
    CheckpointMutate,
    #[serde(rename = "computer.use")]
    ComputerUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Control {
    Cancel,
    Pause,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Budgets {
    pub(crate) max_turns: u64,
    pub(crate) max_tool_calls: u64,
    pub(crate) max_input_tokens: u64,
    pub(crate) max_output_tokens: u64,
    pub(crate) max_cost_microcents: u64,
    pub(crate) wall_clock_ms: u64,
}

#[derive(Debug, Clone)]
pub struct VerifiedControl {
    control: Control,
    carrier: ControlCarrier,
}

impl VerifiedControl {
    pub fn control(&self) -> &'static str {
        match self.control {
            Control::Cancel => "cancel",
            Control::Pause => "pause",
        }
    }

    pub(crate) fn carrier(&self) -> &ControlCarrier {
        &self.carrier
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlCarrier {
    pub(crate) schema: String,
    pub(crate) issuer_id: String,
    pub(crate) key_id: String,
    pub(crate) alg: String,
    pub(crate) activation_id: String,
    pub(crate) session_id: String,
    pub(crate) principal_id: String,
    pub(crate) project_id: String,
    pub(crate) control: Control,
    pub(crate) nonce: String,
    pub(crate) issued_at: String,
    pub(crate) not_after: String,
    pub(crate) signature: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedAdminRequest {
    request: AdminRequest,
}

impl VerifiedAdminRequest {
    pub fn operation(&self) -> &'static str {
        self.request.operation.as_str()
    }

    pub fn admin_id(&self) -> &str {
        &self.request.admin_id
    }
    pub fn admin_epoch(&self) -> u64 {
        self.request.admin_epoch
    }
    pub fn operation_id(&self) -> &str {
        &self.request.operation_id
    }
    pub fn nonce(&self) -> &str {
        &self.request.nonce
    }
    pub fn issued_at(&self) -> &str {
        &self.request.issued_at
    }
    pub fn not_after(&self) -> &str {
        &self.request.not_after
    }
    pub fn before_digest(&self) -> &str {
        &self.request.before_digest
    }
    pub fn after_digest(&self) -> &str {
        &self.request.after_digest
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum AdminOperation {
    EnrollIssuer,
    GrantProject,
    RotateKey,
    RevokeKey,
    RevokeIssuer,
    RecoverRoot,
    ReplaceRoot,
    Rollback,
    EnableArtifact,
    DisableArtifact,
}

impl AdminOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::EnrollIssuer => "enroll_issuer",
            Self::GrantProject => "grant_project",
            Self::RotateKey => "rotate_key",
            Self::RevokeKey => "revoke_key",
            Self::RevokeIssuer => "revoke_issuer",
            Self::RecoverRoot => "recover_root",
            Self::ReplaceRoot => "replace_root",
            Self::Rollback => "rollback",
            Self::EnableArtifact => "enable_artifact",
            Self::DisableArtifact => "disable_artifact",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdminRequest {
    schema: String,
    admin_id: String,
    admin_epoch: u64,
    operation: AdminOperation,
    operation_id: String,
    nonce: String,
    issued_at: String,
    not_after: String,
    before_digest: String,
    after_digest: String,
    reason: String,
    key_id: String,
    alg: String,
    signature: String,
}

pub fn verify_activation_frame(
    raw_frame: &[u8],
    public_key: &[u8; 32],
) -> Result<AdmittedActivation, ActivationError> {
    let frame = raw::parse_frame(raw_frame)?;
    let carrier_value = raw::locate_activation(raw_frame, &frame)?;
    classify_header(carrier_value)?;
    let carrier: ActivationCarrier = serde_json::from_value(carrier_value.clone())
        .map_err(|_| ActivationError::new(RejectReason::UnknownField))?;
    validate_carrier(&carrier)?;

    let canonical_payload = canonical_without_signature(carrier_value)?;
    verify_signature(
        ACTIVATION_DOMAIN,
        &canonical_payload,
        &carrier.signature,
        public_key,
    )?;
    Ok(AdmittedActivation {
        carrier,
        canonical_payload_sha256: hex_lower(&Sha256::digest(canonical_payload)),
    })
}

pub fn verify_receipt(raw_receipt: &[u8], public_key: &[u8; 32]) -> Result<(), ActivationError> {
    let receipt = raw::parse_frame(raw_receipt)?;
    let object = receipt
        .as_object()
        .ok_or_else(|| ActivationError::new(RejectReason::MalformedJson))?;
    if object.get("schema").and_then(Value::as_str) != Some("wayland.nano.activation-receipt/v1") {
        return Err(ActivationError::new(RejectReason::UnsupportedSchema));
    }
    if object.get("alg").and_then(Value::as_str) != Some("Ed25519") {
        return Err(ActivationError::new(RejectReason::UnsupportedAlgorithm));
    }
    let canonical = serde_jcs::to_vec(&receipt)
        .map_err(|_| ActivationError::new(RejectReason::NoncanonicalPayload))?;
    if canonical != raw_receipt {
        return Err(ActivationError::new(RejectReason::NoncanonicalPayload));
    }
    let signature = object
        .get("signature")
        .and_then(Value::as_str)
        .ok_or_else(|| ActivationError::new(RejectReason::InvalidSignatureEncoding))?;
    verify_signature(
        RECEIPT_DOMAIN,
        &canonical_without_signature(&receipt)?,
        signature,
        public_key,
    )
}

pub fn verify_control(
    raw_control: &[u8],
    public_key: &[u8; 32],
) -> Result<VerifiedControl, ActivationError> {
    let value = strict_canonical_document(raw_control)?;
    classify_document_header(&value, "wayland.nano.control/v1")?;
    let control: ControlCarrier = serde_json::from_value(value.clone())
        .map_err(|_| ActivationError::new(RejectReason::UnknownField))?;
    for identifier in [&control.activation_id, &control.session_id, &control.nonce] {
        if !correlation_id(identifier) {
            return Err(ActivationError::new(RejectReason::UnknownField));
        }
    }
    if !issuer_id(&control.issuer_id)
        || !issuer_id(&control.key_id)
        || !opaque_id(&control.principal_id)
        || !opaque_id(&control.project_id)
        || !utc_seconds(&control.issued_at)
        || !utc_seconds(&control.not_after)
    {
        return Err(ActivationError::new(RejectReason::UnknownField));
    }
    verify_signature(
        CONTROL_DOMAIN,
        &canonical_without_signature(&value)?,
        &control.signature,
        public_key,
    )?;
    Ok(VerifiedControl {
        control: control.control,
        carrier: control,
    })
}

pub(crate) fn activation_lookup(raw_frame: &[u8]) -> Result<ActivationCarrier, ActivationError> {
    let frame = raw::parse_frame(raw_frame)?;
    let carrier_value = raw::locate_activation(raw_frame, &frame)?;
    classify_header(carrier_value)?;
    let carrier: ActivationCarrier = serde_json::from_value(carrier_value.clone())
        .map_err(|_| ActivationError::new(RejectReason::UnknownField))?;
    validate_carrier(&carrier)?;
    canonical_without_signature(carrier_value)?;
    Ok(carrier)
}

pub(crate) fn control_lookup(raw_control: &[u8]) -> Result<ControlCarrier, ActivationError> {
    let value = strict_canonical_document(raw_control)?;
    classify_document_header(&value, "wayland.nano.control/v1")?;
    serde_json::from_value(value).map_err(|_| ActivationError::new(RejectReason::UnknownField))
}

pub fn verify_admin_request(
    raw_request: &[u8],
    public_key: &[u8; 32],
) -> Result<VerifiedAdminRequest, ActivationError> {
    let value = strict_canonical_document(raw_request)?;
    classify_document_header(&value, "wayland.nano.admin-request/v1")?;
    let request: AdminRequest = serde_json::from_value(value.clone())
        .map_err(|_| ActivationError::new(RejectReason::UnknownField))?;
    if !issuer_id(&request.admin_id)
        || !issuer_id(&request.key_id)
        || request.admin_epoch == 0
        || request.admin_epoch > MAX_SAFE_INTEGER
        || !correlation_id(&request.operation_id)
        || !correlation_id(&request.nonce)
        || !utc_seconds(&request.issued_at)
        || !utc_seconds(&request.not_after)
        || !sha256_hex(&request.before_digest)
        || !sha256_hex(&request.after_digest)
        || request.reason.is_empty()
        || request.reason.len() > 256
    {
        return Err(ActivationError::new(RejectReason::UnknownField));
    }
    verify_signature(
        ADMIN_DOMAIN,
        &canonical_without_signature(&value)?,
        &request.signature,
        public_key,
    )?;
    Ok(VerifiedAdminRequest { request })
}

fn strict_canonical_document(raw: &[u8]) -> Result<Value, ActivationError> {
    let value = raw::parse_frame(raw)?;
    let canonical = serde_jcs::to_vec(&value)
        .map_err(|_| ActivationError::new(RejectReason::NoncanonicalPayload))?;
    if canonical != raw {
        return Err(ActivationError::new(RejectReason::NoncanonicalPayload));
    }
    Ok(value)
}

fn classify_document_header(value: &Value, schema: &str) -> Result<(), ActivationError> {
    let object = value
        .as_object()
        .ok_or_else(|| ActivationError::new(RejectReason::MalformedJson))?;
    if object.get("schema").and_then(Value::as_str) != Some(schema) {
        return Err(ActivationError::new(RejectReason::UnsupportedSchema));
    }
    if object.get("alg").and_then(Value::as_str) != Some("Ed25519") {
        return Err(ActivationError::new(RejectReason::UnsupportedAlgorithm));
    }
    Ok(())
}

fn classify_header(value: &Value) -> Result<(), ActivationError> {
    let object = value
        .as_object()
        .ok_or_else(|| ActivationError::new(RejectReason::MalformedJson))?;
    if object.get("schema").and_then(Value::as_str) != Some("wayland.nano.activation/v1") {
        return Err(ActivationError::new(if object.contains_key("schema") {
            RejectReason::UnsupportedSchema
        } else {
            RejectReason::UnknownField
        }));
    }
    if object.get("alg").and_then(Value::as_str) != Some("Ed25519") {
        return Err(ActivationError::new(if object.contains_key("alg") {
            RejectReason::UnsupportedAlgorithm
        } else {
            RejectReason::UnknownField
        }));
    }
    if let Some(signature) = object.get("signature").and_then(Value::as_str) {
        if signature.contains('=') || signature.len() != 86 {
            return Err(ActivationError::new(RejectReason::InvalidSignatureEncoding));
        }
    }
    Ok(())
}

fn validate_carrier(carrier: &ActivationCarrier) -> Result<(), ActivationError> {
    if !issuer_id(&carrier.issuer_id) || !issuer_id(&carrier.key_id) {
        return Err(ActivationError::new(RejectReason::UnknownField));
    }
    for value in [
        &carrier.product_subject_id,
        &carrier.principal_id,
        &carrier.project_id,
    ] {
        if !opaque_id(value) {
            return Err(ActivationError::new(RejectReason::UnknownField));
        }
    }
    for value in [
        Some(&carrier.activation_id),
        Some(&carrier.idempotency_key),
        Some(&carrier.nonce),
        carrier.session_id.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if !correlation_id(value) {
            return Err(ActivationError::new(RejectReason::UnknownField));
        }
    }
    for value in [
        &carrier.issued_at,
        &carrier.not_before,
        &carrier.not_after,
        &carrier.deadline,
    ] {
        if !utc_seconds(value) {
            return Err(ActivationError::new(RejectReason::ClockOutOfBounds));
        }
    }
    let budgets = &carrier.budgets;
    for value in [
        budgets.max_turns,
        budgets.max_tool_calls,
        budgets.max_input_tokens,
        budgets.max_output_tokens,
        budgets.max_cost_microcents,
        budgets.wall_clock_ms,
    ] {
        if value == 0 || value > MAX_SAFE_INTEGER {
            return Err(ActivationError::new(RejectReason::AuthorityWidening));
        }
    }
    if carrier.capabilities.len() > 8
        || carrier
            .capabilities
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != carrier.capabilities.len()
        || carrier.controls.len() > 2
        || carrier
            .controls
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != carrier.controls.len()
    {
        return Err(ActivationError::new(RejectReason::UnknownField));
    }
    if matches!(
        carrier.continuity.strategy,
        ContinuityStrategy::SessionResume
    ) && (carrier.session_id.is_none() || carrier.continuity.resume_fingerprint.is_none())
    {
        return Err(ActivationError::new(RejectReason::ResumeFingerprintMissing));
    }
    if let Some(fingerprint) = &carrier.continuity.resume_fingerprint
        && !sha256_hex(fingerprint)
    {
        return Err(ActivationError::new(RejectReason::ResumeDrift));
    }
    Ok(())
}

fn canonical_without_signature(value: &Value) -> Result<Vec<u8>, ActivationError> {
    let mut payload = value.clone();
    payload
        .as_object_mut()
        .and_then(|object| object.remove("signature"))
        .ok_or_else(|| ActivationError::new(RejectReason::InvalidSignatureEncoding))?;
    serde_jcs::to_vec(&payload).map_err(|_| ActivationError::new(RejectReason::NoncanonicalPayload))
}

fn verify_signature(
    domain: &[u8],
    canonical_payload: &[u8],
    encoded_signature: &str,
    public_key: &[u8; 32],
) -> Result<(), ActivationError> {
    if encoded_signature.contains('=') || encoded_signature.len() != 86 {
        return Err(ActivationError::new(RejectReason::InvalidSignatureEncoding));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| ActivationError::new(RejectReason::InvalidSignatureEncoding))?;
    let signature_bytes: [u8; 64] = bytes
        .try_into()
        .map_err(|_| ActivationError::new(RejectReason::InvalidSignatureEncoding))?;
    let key = VerifyingKey::from_bytes(public_key)
        .map_err(|_| ActivationError::new(RejectReason::InvalidKeyEncoding))?;
    let signature = Signature::from_bytes(&signature_bytes);
    let mut message = Vec::with_capacity(domain.len() + canonical_payload.len());
    message.extend_from_slice(domain);
    message.extend_from_slice(canonical_payload);
    key.verify(&message, &signature)
        .map_err(|_| ActivationError::new(RejectReason::InvalidSignature))
}

fn issuer_id(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn opaque_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn correlation_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn utc_seconds(value: &str) -> bool {
    value.len() == 20
        && value.ends_with('Z')
        && value.as_bytes().iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7) && *byte == b'-'
                || index == 10 && *byte == b'T'
                || matches!(index, 13 | 16) && *byte == b':'
                || !matches!(index, 4 | 7 | 10 | 13 | 16 | 19) && byte.is_ascii_digit()
                || index == 19 && *byte == b'Z'
        })
}

fn sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}
