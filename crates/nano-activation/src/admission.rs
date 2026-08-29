//! Sole trusted admission constructor and crash-rebuildable activation ledger.

use crate::authority::{AuthorityError, AuthoritySnapshot};
use crate::control::{ControlKind, ControlOutcome};
use crate::policy::{EffectivePolicy, PolicyCeiling, intersect};
use crate::receipt::{
    ArtifactIdentity, Decision, IntentState, ReceiptFields, ReceiptSigner, ResultState,
    SignedReceipt, mint,
};
use crate::store::AuthorityStore;
use crate::{
    ActivationCarrier, ActivationError, ContinuityStrategy, Control, Fallback, RejectReason,
    activation_lookup, control_lookup, verify_activation_frame, verify_control,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use nano_session::FileLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeBinding {
    pub issuer_id: String,
    pub product_subject_id: String,
    pub principal_id: String,
    pub project_id: String,
    pub session_id: String,
    pub fingerprint: String,
    pub admin_epoch: u64,
    pub issuer_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBinding {
    pub issuer_id: String,
    pub product_subject_id: String,
    pub principal_id: String,
    pub project_id: String,
    pub session_id: String,
    pub fingerprint: String,
    pub admin_epoch: u64,
    pub issuer_epoch: u64,
    pub activation_id: String,
    pub receipt_id: String,
    pub canonical_payload_sha256: String,
    pub artifact: ArtifactIdentity,
}

#[derive(Debug, Clone)]
pub struct AdmittedToken {
    principal_id: String,
    project_id: String,
    activation_id: String,
    session_id: Option<String>,
    policy: EffectivePolicy,
    receipt: SignedReceipt,
}

#[derive(Debug, Clone)]
pub struct ControlDecision {
    outcome: ControlOutcome,
    receipt: SignedReceipt,
}

impl ControlDecision {
    pub fn outcome(&self) -> ControlOutcome {
        self.outcome
    }
    pub fn receipt(&self) -> &SignedReceipt {
        &self.receipt
    }
}

impl AdmittedToken {
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
    pub fn activation_id(&self) -> &str {
        &self.activation_id
    }
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
    pub fn policy(&self) -> &EffectivePolicy {
        &self.policy
    }
    pub fn receipt(&self) -> &SignedReceipt {
        &self.receipt
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionFault {
    None,
    AfterIntent,
    AfterDecision,
}

#[derive(Debug, thiserror::Error)]
pub enum GateOpenError {
    #[error("authority store unavailable: {0}")]
    Authority(#[from] AuthorityError),
    #[error("activation ledger unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("receipt signer unavailable")]
    ReceiptSignerUnavailable,
    #[error("receipt signer does not match the enrolled receipt key")]
    ReceiptSignerMismatch,
}

pub struct AdmissionGate {
    nano_home: std::path::PathBuf,
    signer: Box<dyn ReceiptSigner>,
    ceiling: PolicyCeiling,
    artifact: ArtifactIdentity,
}

impl AdmissionGate {
    pub fn open(
        nano_home: &Path,
        signer: Box<dyn ReceiptSigner>,
        ceiling: PolicyCeiling,
        artifact: ArtifactIdentity,
    ) -> Result<Self, GateOpenError> {
        signer
            .preflight()
            .map_err(|_| GateOpenError::ReceiptSignerUnavailable)?;
        let authority = AuthorityStore::open(nano_home)?;
        let snapshot = authority.snapshot()?;
        if snapshot.receipt_signer_public_key != Some(signer.public_key()) {
            return Err(GateOpenError::ReceiptSignerMismatch);
        }
        drop(authority);
        Ok(Self {
            nano_home: nano_home.to_owned(),
            signer,
            ceiling,
            artifact,
        })
    }

    pub fn admit_raw(
        &mut self,
        raw_frame: &[u8],
        now_utc: &str,
        resume: Option<&ResumeBinding>,
    ) -> Result<AdmittedToken, ActivationError> {
        self.admit_raw_with_fault(raw_frame, now_utc, resume, AdmissionFault::None)
    }

    pub fn admit_raw_with_fault(
        &mut self,
        raw_frame: &[u8],
        now_utc: &str,
        resume: Option<&ResumeBinding>,
        fault: AdmissionFault,
    ) -> Result<AdmittedToken, ActivationError> {
        self.signer
            .preflight()
            .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;
        let lookup = activation_lookup(raw_frame)?;
        let mut authority = AuthorityStore::open(&self.nano_home).map_err(map_authority_error)?;
        let snapshot = authority.snapshot().map_err(map_authority_error)?;
        let (public_key, issuer_epoch) = authorize(&snapshot, &lookup)?;
        let verified = verify_activation_frame(raw_frame, &public_key)?;
        let carrier = verified.carrier();
        validate_time(carrier, now_utc)?;
        let owned_resume = if matches!(
            carrier.continuity.strategy,
            ContinuityStrategy::SessionResume
        ) && resume.is_none()
        {
            let session_id = carrier
                .session_id
                .as_deref()
                .ok_or_else(|| ActivationError::new(RejectReason::ResumeDrift))?;
            Some(self.session_binding_with_snapshot(session_id, &snapshot)?)
        } else {
            None
        };
        let resolved_resume = owned_resume.as_ref().map(SessionBinding::as_resume);
        validate_continuity(
            carrier,
            resume.or(resolved_resume.as_ref()),
            &snapshot,
            issuer_epoch,
        )?;
        let policy = intersect(carrier, &self.ceiling)?;

        let raw_hash = hex(&Sha256::digest(raw_frame));
        let immutable_hash = verified.canonical_payload_sha256().to_owned();
        let tuple_key = tuple_key(carrier);

        // Global nonce uniqueness is committed before tuple idempotency is consulted.
        authority
            .consume_nonce(
                &carrier.nonce,
                &immutable_hash,
                parse_seconds(&carrier.not_after)?,
            )
            .map_err(map_authority_error)?;
        drop(authority);
        let mut ledger = ActivationLedger::open(&self.nano_home)
            .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;

        if let Some(existing) = ledger.state.tuples.get(&tuple_key) {
            if existing.immutable_hash != immutable_hash {
                return Err(ActivationError::new(RejectReason::IdempotencyConflict));
            }
            if let Some(bytes) = &existing.receipt {
                return Ok(token_from(
                    carrier,
                    policy,
                    SignedReceipt::from_stored(bytes.clone()),
                ));
            }
        } else {
            ledger.append(ActivationRecord::Intent {
                sequence: 0,
                tuple_key: tuple_key.clone(),
                activation_id: carrier.activation_id.clone(),
                immutable_hash: immutable_hash.clone(),
                raw_hash: raw_hash.clone(),
            })?;
        }
        if fault == AdmissionFault::AfterIntent {
            panic!("injected crash after durable activation intent");
        }

        let receipt_id = format!(
            "receipt-{}",
            &hex(&Sha256::digest(tuple_key.as_bytes()))[..24]
        );
        let receipt = mint(
            self.signer.as_ref(),
            ReceiptFields {
                receipt_id: &receipt_id,
                issuer_id: &carrier.issuer_id,
                key_id: &carrier.key_id,
                product_subject_id: &carrier.product_subject_id,
                principal_id: &carrier.principal_id,
                project_id: &carrier.project_id,
                activation_id: &carrier.activation_id,
                idempotency_key: &carrier.idempotency_key,
                session_id: carrier.session_id.as_deref(),
                raw_assertion_sha256: &raw_hash,
                canonical_payload_sha256: &immutable_hash,
                decision: Decision::Admitted,
                reason: RejectReason::None,
                effective_policy: &policy,
                intent_state: IntentState::Journaled,
                result_state: ResultState::Pending,
                authority_journal_position: snapshot.operations.len() as u64 + 1,
                activation_journal_position: ledger.next_sequence,
                admin_epoch: snapshot.admin_epoch,
                issuer_epoch,
                grant_epoch: issuer_epoch,
                revocation_epoch: issuer_epoch,
                issued_at: now_utc,
                artifact: &self.artifact,
                control: None,
            },
        )
        .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;
        ledger.append(ActivationRecord::Decision {
            sequence: 0,
            tuple_key,
            receipt: STANDARD.encode(receipt.as_bytes()),
        })?;
        if fault == AdmissionFault::AfterDecision {
            panic!("injected crash after durable activation decision");
        }
        Ok(token_from(carrier, policy, receipt))
    }

    pub fn mark_dispatch_eligible(&mut self, activation_id: &str) -> Result<(), ActivationError> {
        let mut ledger = self.open_ledger()?;
        Self::require_pending(&ledger, activation_id)?;
        ledger.append(ActivationRecord::DispatchEligible {
            sequence: 0,
            activation_id: activation_id.to_owned(),
        })?;
        Ok(())
    }

    pub fn bind_session(
        &mut self,
        activation_id: &str,
        session_id: &str,
        fingerprint: &str,
    ) -> Result<SessionBinding, ActivationError> {
        if fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ActivationError::new(RejectReason::ResumeDrift));
        }
        let authority = AuthorityStore::open(&self.nano_home).map_err(map_authority_error)?;
        let snapshot = authority.snapshot().map_err(map_authority_error)?;
        drop(authority);
        let mut ledger = self.open_ledger()?;
        let activation = ledger
            .state
            .activations
            .get(activation_id)
            .cloned()
            .ok_or_else(|| ActivationError::new(RejectReason::ResumeDrift))?;
        if session_id.is_empty()
            || session_id.len() > 128
            || !session_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || activation
                .session_id
                .as_deref()
                .is_some_and(|asserted| asserted != session_id)
        {
            return Err(ActivationError::new(RejectReason::ResumeDrift));
        }
        let session_id = session_id.to_owned();
        let issuer = snapshot
            .issuers
            .get(&activation.issuer_id)
            .filter(|issuer| !issuer.revoked)
            .ok_or_else(|| ActivationError::new(RejectReason::ResumeDrift))?;
        let binding = SessionBinding {
            issuer_id: activation.issuer_id,
            product_subject_id: activation.product_subject_id,
            principal_id: activation.principal_id,
            project_id: activation.project_id,
            session_id: session_id.clone(),
            fingerprint: fingerprint.to_owned(),
            admin_epoch: snapshot.admin_epoch,
            issuer_epoch: issuer.epoch,
            activation_id: activation_id.to_owned(),
            receipt_id: activation.receipt_id,
            canonical_payload_sha256: activation.canonical_payload_sha256,
            artifact: activation.artifact,
        };
        if let Some(existing) = ledger.state.sessions.get(&session_id) {
            return if existing == &binding {
                Ok(existing.clone())
            } else {
                Err(ActivationError::new(RejectReason::ResumeDrift))
            };
        }
        ledger.append(ActivationRecord::SessionBound {
            sequence: 0,
            binding: Box::new(binding.clone()),
        })?;
        Ok(binding)
    }

    pub fn session_binding(&self, session_id: &str) -> Result<SessionBinding, ActivationError> {
        let authority = AuthorityStore::open(&self.nano_home).map_err(map_authority_error)?;
        let snapshot = authority.snapshot().map_err(map_authority_error)?;
        drop(authority);
        self.session_binding_with_snapshot(session_id, &snapshot)
    }

    fn session_binding_with_snapshot(
        &self,
        session_id: &str,
        snapshot: &AuthoritySnapshot,
    ) -> Result<SessionBinding, ActivationError> {
        let binding = self
            .open_ledger()?
            .state
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| ActivationError::new(RejectReason::ResumeDrift))?;
        let issuer = snapshot
            .issuers
            .get(&binding.issuer_id)
            .filter(|issuer| !issuer.revoked)
            .ok_or_else(|| ActivationError::new(RejectReason::ResumeDrift))?;
        if binding.admin_epoch != snapshot.admin_epoch
            || binding.issuer_epoch != issuer.epoch
            || binding.product_subject_id != issuer.subject_id
            || binding.principal_id != issuer.principal_id
            || !issuer.projects.contains(&binding.project_id)
            || binding.artifact != self.artifact
        {
            return Err(ActivationError::new(RejectReason::ResumeDrift));
        }
        Ok(binding)
    }

    pub fn record_result(
        &mut self,
        activation_id: &str,
        result_digest: &str,
    ) -> Result<(), ActivationError> {
        let mut ledger = self.open_ledger()?;
        if ledger.state.controls.contains_key(activation_id) {
            return Err(ActivationError::new(RejectReason::ControlRaceLost));
        }
        Self::require_pending(&ledger, activation_id)?;
        if !ledger.state.dispatch_eligible.contains(activation_id) {
            return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
        }
        ledger.append(ActivationRecord::Result {
            sequence: 0,
            activation_id: activation_id.to_owned(),
            state: ResultState::Completed,
            result_digest: Some(result_digest.to_owned()),
        })?;
        Ok(())
    }

    pub fn record_unknown_outcome(&mut self, activation_id: &str) -> Result<(), ActivationError> {
        let mut ledger = self.open_ledger()?;
        Self::require_pending(&ledger, activation_id)?;
        if !ledger.state.dispatch_eligible.contains(activation_id) {
            return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
        }
        ledger.append(ActivationRecord::Result {
            sequence: 0,
            activation_id: activation_id.to_owned(),
            state: ResultState::UnknownOutcome,
            result_digest: None,
        })?;
        Ok(())
    }

    pub fn result_state(
        &self,
        activation_id: &str,
    ) -> Result<Option<ResultState>, ActivationError> {
        Ok(self
            .open_ledger()?
            .state
            .results
            .get(activation_id)
            .copied())
    }

    pub fn apply_control(
        &mut self,
        raw_control: &[u8],
        now_utc: &str,
    ) -> Result<ControlDecision, ActivationError> {
        self.signer
            .preflight()
            .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;
        let lookup = control_lookup(raw_control)?;
        let mut authority = AuthorityStore::open(&self.nano_home).map_err(map_authority_error)?;
        let snapshot = authority.snapshot().map_err(map_authority_error)?;
        let issuer = snapshot
            .issuers
            .get(&lookup.issuer_id)
            .ok_or_else(|| ActivationError::new(RejectReason::UnknownIssuer))?;
        if issuer.principal_id != lookup.principal_id
            || !issuer.projects.contains(&lookup.project_id)
        {
            return Err(ActivationError::new(RejectReason::ControlUnauthorized));
        }
        let key = issuer
            .keys
            .get(&lookup.key_id)
            .filter(|key| !key.revoked)
            .ok_or_else(|| ActivationError::new(RejectReason::ControlUnauthorized))?;
        let verified = verify_control(raw_control, &key.public_key)?;
        let control = verified.carrier();
        if now_utc < control.issued_at.as_str() || now_utc > control.not_after.as_str() {
            return Err(ActivationError::new(RejectReason::ClockOutOfBounds));
        }
        let mut ledger = self.open_ledger()?;
        let activation = ledger
            .state
            .activations
            .get(&control.activation_id)
            .cloned()
            .ok_or_else(|| ActivationError::new(RejectReason::ControlUnauthorized))?;
        if activation.principal_id != control.principal_id
            || activation.project_id != control.project_id
            || activation.session_id.as_deref() != Some(control.session_id.as_str())
        {
            return Err(ActivationError::new(RejectReason::ControlUnauthorized));
        }
        authority
            .consume_nonce(
                &control.nonce,
                &hex(&Sha256::digest(raw_control)),
                parse_seconds(&control.not_after)?,
            )
            .map_err(map_authority_error)?;
        drop(authority);
        let control_hash = hex(&Sha256::digest(raw_control));
        if let Some(existing) = ledger.state.control_nonces.get(&control.nonce) {
            if existing.immutable_hash != control_hash {
                return Err(ActivationError::new(RejectReason::NonceReplay));
            }
            return Ok(ControlDecision {
                outcome: existing.outcome,
                receipt: SignedReceipt::from_stored(existing.receipt.clone()),
            });
        }
        let terminal = ledger.state.results.get(&control.activation_id).copied();
        let (kind, outcome, state) = match (control.control, terminal) {
            (Control::Cancel, Some(state)) => {
                (ControlKind::Cancel, ControlOutcome::RaceLost, state)
            }
            (Control::Pause, Some(state)) => (ControlKind::Pause, ControlOutcome::RaceLost, state),
            (Control::Cancel, None) => (
                ControlKind::Cancel,
                ControlOutcome::Cancelled,
                ResultState::Cancelled,
            ),
            (Control::Pause, None) => (
                ControlKind::Pause,
                ControlOutcome::Paused,
                ResultState::Paused,
            ),
        };
        let receipt_id = format!("control-receipt-{}", &control_hash[..24]);
        let receipt = mint(
            self.signer.as_ref(),
            ReceiptFields {
                receipt_id: &receipt_id,
                issuer_id: &control.issuer_id,
                key_id: &control.key_id,
                product_subject_id: &activation.product_subject_id,
                principal_id: &control.principal_id,
                project_id: &control.project_id,
                activation_id: &control.activation_id,
                idempotency_key: &control.nonce,
                session_id: Some(&control.session_id),
                raw_assertion_sha256: &control_hash,
                canonical_payload_sha256: &control_hash,
                decision: if outcome == ControlOutcome::RaceLost {
                    Decision::ControlRaceLost
                } else {
                    Decision::ControlAccepted
                },
                reason: if outcome == ControlOutcome::RaceLost {
                    RejectReason::ControlRaceLost
                } else {
                    RejectReason::None
                },
                effective_policy: &activation.policy,
                intent_state: IntentState::Journaled,
                result_state: state,
                authority_journal_position: snapshot.operations.len() as u64 + 1,
                activation_journal_position: ledger.next_sequence,
                admin_epoch: snapshot.admin_epoch,
                issuer_epoch: issuer.epoch,
                grant_epoch: issuer.epoch,
                revocation_epoch: issuer.epoch,
                issued_at: now_utc,
                artifact: &self.artifact,
                control: Some(match kind {
                    ControlKind::Cancel => "cancel",
                    ControlKind::Pause => "pause",
                }),
            },
        )
        .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;
        ledger.append(ActivationRecord::Control {
            sequence: 0,
            activation_id: control.activation_id.clone(),
            nonce: control.nonce.clone(),
            immutable_hash: control_hash,
            kind,
            state,
            outcome,
            receipt: STANDARD.encode(receipt.as_bytes()),
        })?;
        Ok(ControlDecision { outcome, receipt })
    }

    fn require_pending(
        ledger: &ActivationLedger,
        activation_id: &str,
    ) -> Result<(), ActivationError> {
        if !ledger.state.activations.contains_key(activation_id)
            || ledger.state.results.contains_key(activation_id)
            || ledger.state.controls.contains_key(activation_id)
        {
            return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
        }
        Ok(())
    }

    fn open_ledger(&self) -> Result<ActivationLedger, ActivationError> {
        ActivationLedger::open(&self.nano_home)
            .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))
    }
}

impl SessionBinding {
    fn as_resume(&self) -> ResumeBinding {
        ResumeBinding {
            issuer_id: self.issuer_id.clone(),
            product_subject_id: self.product_subject_id.clone(),
            principal_id: self.principal_id.clone(),
            project_id: self.project_id.clone(),
            session_id: self.session_id.clone(),
            fingerprint: self.fingerprint.clone(),
            admin_epoch: self.admin_epoch,
            issuer_epoch: self.issuer_epoch,
        }
    }
}

fn token_from(
    carrier: &ActivationCarrier,
    policy: EffectivePolicy,
    receipt: SignedReceipt,
) -> AdmittedToken {
    AdmittedToken {
        principal_id: carrier.principal_id.clone(),
        project_id: carrier.project_id.clone(),
        activation_id: carrier.activation_id.clone(),
        session_id: carrier.session_id.clone(),
        policy,
        receipt,
    }
}

fn authorize(
    snapshot: &AuthoritySnapshot,
    carrier: &ActivationCarrier,
) -> Result<([u8; 32], u64), ActivationError> {
    let issuer = snapshot
        .issuers
        .get(&carrier.issuer_id)
        .ok_or_else(|| ActivationError::new(RejectReason::UnknownIssuer))?;
    if issuer.revoked {
        return Err(ActivationError::new(RejectReason::RevokedIssuer));
    }
    if issuer.subject_id != carrier.product_subject_id {
        return Err(ActivationError::new(RejectReason::UnknownProductSubject));
    }
    if issuer.principal_id != carrier.principal_id {
        return Err(ActivationError::new(RejectReason::PrincipalMismatch));
    }
    if !issuer.projects.contains(&carrier.project_id) {
        return Err(ActivationError::new(RejectReason::UnauthorizedProject));
    }
    let key = issuer
        .keys
        .get(&carrier.key_id)
        .ok_or_else(|| ActivationError::new(RejectReason::UnknownKey))?;
    if key.revoked {
        return Err(ActivationError::new(RejectReason::RevokedKey));
    }
    Ok((key.public_key, issuer.epoch))
}

fn validate_time(carrier: &ActivationCarrier, now: &str) -> Result<(), ActivationError> {
    if now < carrier.not_before.as_str() {
        return Err(ActivationError::new(RejectReason::AssertionNotYetValid));
    }
    if now > carrier.not_after.as_str() || now > carrier.deadline.as_str() {
        return Err(ActivationError::new(RejectReason::AssertionExpired));
    }
    if carrier.issued_at > carrier.not_after || carrier.not_before > carrier.not_after {
        return Err(ActivationError::new(RejectReason::ClockOutOfBounds));
    }
    Ok(())
}

fn validate_continuity(
    carrier: &ActivationCarrier,
    resume: Option<&ResumeBinding>,
    snapshot: &AuthoritySnapshot,
    issuer_epoch: u64,
) -> Result<(), ActivationError> {
    match carrier.continuity.strategy {
        ContinuityStrategy::Fresh => {
            if !matches!(carrier.continuity.fallback, Fallback::None) {
                return Err(ActivationError::new(RejectReason::FallbackUnauthorized));
            }
            Ok(())
        }
        ContinuityStrategy::MemoryRecall => {
            Err(ActivationError::new(RejectReason::ContinuityNotEnabled))
        }
        ContinuityStrategy::SessionResume => {
            if !matches!(carrier.continuity.fallback, Fallback::None) {
                return Err(ActivationError::new(RejectReason::FallbackUnauthorized));
            }
            let expected = resume.ok_or_else(|| ActivationError::new(RejectReason::ResumeDrift))?;
            if expected.issuer_id != carrier.issuer_id
                || expected.product_subject_id != carrier.product_subject_id
                || expected.principal_id != carrier.principal_id
                || expected.project_id != carrier.project_id
                || Some(expected.session_id.as_str()) != carrier.session_id.as_deref()
                || Some(expected.fingerprint.as_str())
                    != carrier.continuity.resume_fingerprint.as_deref()
                || expected.admin_epoch != snapshot.admin_epoch
                || expected.issuer_epoch != issuer_epoch
            {
                return Err(ActivationError::new(RejectReason::ResumeDrift));
            }
            Ok(())
        }
    }
}

fn tuple_key(carrier: &ActivationCarrier) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        carrier.issuer_id, carrier.principal_id, carrier.project_id, carrier.idempotency_key
    )
}

fn map_authority_error(error: AuthorityError) -> ActivationError {
    match error {
        AuthorityError::NonceReplay => ActivationError::new(RejectReason::NonceReplay),
        AuthorityError::Unauthorized => ActivationError::new(RejectReason::UnauthorizedProject),
        AuthorityError::UnknownRecord => ActivationError::new(RejectReason::AmbiguousRecovery),
        _ => ActivationError::new(RejectReason::AuthorityStoreUnavailable),
    }
}

fn parse_seconds(value: &str) -> Result<i64, ActivationError> {
    // The contract parser already proves YYYY-MM-DDTHH:MM:SSZ. This stable scalar
    // preserves tombstone ordering without introducing a second time parser.
    value
        .bytes()
        .filter(u8::is_ascii_digit)
        .try_fold(0i64, |value, digit| {
            value.checked_mul(10)?.checked_add(i64::from(digit - b'0'))
        })
        .ok_or_else(|| ActivationError::new(RejectReason::ClockOutOfBounds))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[(byte >> 4) as usize] as char);
        value.push(HEX[(byte & 15) as usize] as char);
    }
    value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case", deny_unknown_fields)]
enum ActivationRecord {
    Intent {
        sequence: u64,
        tuple_key: String,
        activation_id: String,
        immutable_hash: String,
        raw_hash: String,
    },
    Decision {
        sequence: u64,
        tuple_key: String,
        receipt: String,
    },
    DispatchEligible {
        sequence: u64,
        activation_id: String,
    },
    Result {
        sequence: u64,
        activation_id: String,
        state: ResultState,
        result_digest: Option<String>,
    },
    Control {
        sequence: u64,
        activation_id: String,
        nonce: String,
        immutable_hash: String,
        kind: ControlKind,
        state: ResultState,
        outcome: ControlOutcome,
        receipt: String,
    },
    SessionBound {
        sequence: u64,
        binding: Box<SessionBinding>,
    },
}

impl ActivationRecord {
    fn sequence(&self) -> u64 {
        match self {
            Self::Intent { sequence, .. }
            | Self::Decision { sequence, .. }
            | Self::DispatchEligible { sequence, .. }
            | Self::Result { sequence, .. }
            | Self::Control { sequence, .. }
            | Self::SessionBound { sequence, .. } => *sequence,
        }
    }
    fn set_sequence(&mut self, value: u64) {
        match self {
            Self::Intent { sequence, .. }
            | Self::Decision { sequence, .. }
            | Self::DispatchEligible { sequence, .. }
            | Self::Result { sequence, .. }
            | Self::Control { sequence, .. }
            | Self::SessionBound { sequence, .. } => *sequence = value,
        }
    }
}

#[derive(Debug, Clone)]
struct TupleState {
    immutable_hash: String,
    receipt: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct ActivationIdentity {
    issuer_id: String,
    product_subject_id: String,
    principal_id: String,
    project_id: String,
    session_id: Option<String>,
    policy: EffectivePolicy,
    receipt_id: String,
    canonical_payload_sha256: String,
    artifact: ArtifactIdentity,
}

#[derive(Debug, Clone)]
struct ControlNonceState {
    immutable_hash: String,
    outcome: ControlOutcome,
    receipt: Vec<u8>,
}

#[derive(Default)]
struct LedgerState {
    tuples: BTreeMap<String, TupleState>,
    activations: BTreeMap<String, ActivationIdentity>,
    results: BTreeMap<String, ResultState>,
    controls: BTreeMap<String, ControlKind>,
    dispatch_eligible: std::collections::BTreeSet<String>,
    control_nonces: BTreeMap<String, ControlNonceState>,
    sessions: BTreeMap<String, SessionBinding>,
}

struct ActivationLedger {
    file: File,
    next_sequence: u64,
    state: LedgerState,
    _lock: FileLock,
}

impl ActivationLedger {
    fn open(home: &Path) -> Result<Self, GateOpenError> {
        let root = home.join("activation");
        std::fs::create_dir_all(&root)?;
        let lock =
            FileLock::try_acquire(&root.join("admission.lock")).map_err(|error| match error {
                nano_session::LockError::Busy => {
                    std::io::Error::new(std::io::ErrorKind::WouldBlock, error)
                }
                nano_session::LockError::Io(error) => error,
            })?;
        let path = root.join("admission.jsonl");
        let (state, last) = replay_ledger(&path)?;
        truncate_torn_tail(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        Ok(Self {
            file,
            next_sequence: last + 1,
            state,
            _lock: lock,
        })
    }

    fn append(&mut self, mut record: ActivationRecord) -> Result<(), ActivationError> {
        record.set_sequence(self.next_sequence);
        let mut bytes = serde_jcs::to_vec(&record)
            .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;
        bytes.push(b'\n');
        self.file
            .write_all(&bytes)
            .and_then(|_| self.file.sync_data())
            .map_err(|_| ActivationError::new(RejectReason::AuthorityStoreUnavailable))?;
        reduce(&mut self.state, &record)?;
        self.next_sequence += 1;
        Ok(())
    }
}

fn truncate_torn_tail(path: &Path) -> Result<(), std::io::Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return Ok(());
    }
    let durable_len = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    OpenOptions::new()
        .write(true)
        .open(path)?
        .set_len(durable_len as u64)
}

fn replay_ledger(path: &Path) -> Result<(LedgerState, u64), std::io::Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((LedgerState::default(), 0));
        }
        Err(error) => return Err(error),
    };
    let mut state = LedgerState::default();
    let mut expected = 1u64;
    let records: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
    for (index, raw) in records.iter().copied().enumerate() {
        if raw.is_empty() {
            continue;
        }
        let record: ActivationRecord = match serde_json::from_slice(raw) {
            Ok(record) => record,
            Err(_) if index == records.len() - 1 && !bytes.ends_with(b"\n") => break,
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid activation journal",
                ));
            }
        };
        if record.sequence() != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "activation journal sequence gap",
            ));
        }
        reduce(&mut state, &record).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid activation transition",
            )
        })?;
        expected += 1;
    }
    Ok((state, expected.saturating_sub(1)))
}

fn reduce(state: &mut LedgerState, record: &ActivationRecord) -> Result<(), ActivationError> {
    match record {
        ActivationRecord::Intent {
            tuple_key,
            activation_id,
            immutable_hash,
            ..
        } => {
            if let Some(existing) = state.tuples.get(tuple_key) {
                if existing.immutable_hash != *immutable_hash {
                    return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
                }
                return Ok(());
            }
            state.tuples.insert(
                tuple_key.clone(),
                TupleState {
                    immutable_hash: immutable_hash.clone(),
                    receipt: None,
                },
            );
            let _ = activation_id;
        }
        ActivationRecord::Decision {
            tuple_key, receipt, ..
        } => {
            let bytes = STANDARD
                .decode(receipt)
                .map_err(|_| ActivationError::new(RejectReason::AmbiguousRecovery))?;
            let tuple = state
                .tuples
                .get_mut(tuple_key)
                .ok_or_else(|| ActivationError::new(RejectReason::AmbiguousRecovery))?;
            tuple.receipt = Some(bytes.clone());
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| ActivationError::new(RejectReason::AmbiguousRecovery))?;
            let activation_id = value["activation_id"]
                .as_str()
                .ok_or_else(|| ActivationError::new(RejectReason::AmbiguousRecovery))?;
            state.activations.insert(
                activation_id.into(),
                ActivationIdentity {
                    issuer_id: value["issuer_id"].as_str().unwrap_or_default().into(),
                    product_subject_id: value["product_subject_id"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                    principal_id: value["principal_id"].as_str().unwrap_or_default().into(),
                    project_id: value["project_id"].as_str().unwrap_or_default().into(),
                    session_id: value["session_id"].as_str().map(str::to_owned),
                    policy: serde_json::from_value(value["effective_policy"].clone())
                        .map_err(|_| ActivationError::new(RejectReason::AmbiguousRecovery))?,
                    receipt_id: value["receipt_id"].as_str().unwrap_or_default().into(),
                    canonical_payload_sha256: value["canonical_payload_sha256"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                    artifact: ArtifactIdentity {
                        source_commit_sha: value["source_commit_sha"]
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                        cargo_lock_sha256: value["cargo_lock_sha256"]
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                        executable_sha256: value["executable_sha256"]
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                    },
                },
            );
        }
        ActivationRecord::DispatchEligible { activation_id, .. } => {
            if !state.activations.contains_key(activation_id) {
                return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
            }
            state.dispatch_eligible.insert(activation_id.clone());
        }
        ActivationRecord::Result {
            activation_id,
            state: result,
            ..
        } => {
            state.results.insert(activation_id.clone(), *result);
        }
        ActivationRecord::Control {
            activation_id,
            nonce,
            immutable_hash,
            kind,
            state: result,
            outcome,
            receipt,
            ..
        } => {
            let receipt = STANDARD
                .decode(receipt)
                .map_err(|_| ActivationError::new(RejectReason::AmbiguousRecovery))?;
            if state
                .control_nonces
                .insert(
                    nonce.clone(),
                    ControlNonceState {
                        immutable_hash: immutable_hash.clone(),
                        outcome: *outcome,
                        receipt,
                    },
                )
                .is_some()
            {
                return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
            }
            state.controls.insert(activation_id.clone(), *kind);
            if *outcome != ControlOutcome::RaceLost {
                state.results.insert(activation_id.clone(), *result);
            }
        }
        ActivationRecord::SessionBound { binding, .. } => {
            let activation = state
                .activations
                .get(&binding.activation_id)
                .ok_or_else(|| ActivationError::new(RejectReason::AmbiguousRecovery))?;
            if activation.issuer_id != binding.issuer_id
                || activation.product_subject_id != binding.product_subject_id
                || activation.principal_id != binding.principal_id
                || activation.project_id != binding.project_id
                || activation
                    .session_id
                    .as_deref()
                    .is_some_and(|asserted| asserted != binding.session_id)
                || activation.receipt_id != binding.receipt_id
                || activation.canonical_payload_sha256 != binding.canonical_payload_sha256
                || activation.artifact != binding.artifact
            {
                return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
            }
            if let Some(existing) = state.sessions.get(&binding.session_id) {
                if existing != binding.as_ref() {
                    return Err(ActivationError::new(RejectReason::AmbiguousRecovery));
                }
            } else {
                state
                    .sessions
                    .insert(binding.session_id.clone(), binding.as_ref().clone());
            }
        }
    }
    Ok(())
}
