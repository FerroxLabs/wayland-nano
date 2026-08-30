//! Phase-2-only, one-shot offline initial administrator bootstrap.
//!
//! This module is deliberately concrete. It is reachable only from the two
//! literal CLI commands named by the signed offline-bootstrap amendment; it
//! exports no proof constructor or generic authorization interface.

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
use crate::admin::sign_bootstrap_receipt;
use crate::admin::{BootstrapError, BootstrapKeyPaths};
use crate::authority::{AuthorityError, AuthoritySnapshot, KeyRole};
use crate::key_provider::{KeyProviderError, audit_owner_only_path, load_key_reference};
use crate::receipt::{ArtifactIdentity, ReceiptSigner};
use crate::signer_provider::{ExternalReceiptSigner, SignerProviderError, derive_public_key};
#[cfg(windows)]
use crate::store::AuthorityStore;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
#[cfg(windows)]
use chrono::Timelike as _;
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use nano_session::FileLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const ADMIN_ID: &str = "phase2-owner";
const AUTHORIZATION_ID: &str = "phase2-windows-exact-artifact-bootstrap-1";
const AUTHORIZATION_COUNTER: u64 = 1;
const AUTHORIZATION_KEY_ID: &str = "phase2-offline-bootstrap-2026-08-30";
const AUTHORIZATION_PUBLIC_KEY: [u8; 32] = [
    0x6f, 0x17, 0xbe, 0xf1, 0x4e, 0xe3, 0xa5, 0x8b, 0x0c, 0xf7, 0x38, 0x5e, 0x30, 0x1e, 0xae, 0xd5,
    0xa6, 0x0b, 0x88, 0x26, 0x34, 0xb8, 0x85, 0xf0, 0xaa, 0x87, 0xd0, 0x32, 0x0b, 0xc3, 0xcb, 0x28,
];
const AMENDMENT_SHA256: &str = "3babb5386a22ac7633ea44791066d600c092dbf208400b94c509faa8c0d853e3";
const CHALLENGE_SCHEMA: &str = "wayland.nano.offline-bootstrap-challenge/v1";
const AUTHORIZATION_SCHEMA: &str = "wayland.nano.offline-bootstrap-authorization/v1";
const RECEIPT_SCHEMA: &str = "wayland.nano.offline-bootstrap-consumption-receipt/v1";
const PURPOSE: &str = "initial_admin_bootstrap";
const REASON: &str =
    "owner-authorized Phase 2 artifact-bound bootstrap without physical-console presence";
const SIGNATURE_DOMAIN: &[u8] = b"WAYLAND-NANO-OFFLINE-BOOTSTRAP\0v1\0";
const RECEIPT_DOMAIN: &[u8] = b"WAYLAND-NANO-OFFLINE-BOOTSTRAP-RECEIPT\0v1\0";
const GENESIS_DOMAIN: &[u8] = b"WAYLAND-NANO-OFFLINE-BOOTSTRAP-LEDGER-GENESIS\0v1\0";
const MACHINE_DOMAIN: &[u8] = b"WAYLAND-NANO-OFFLINE-BOOTSTRAP-MACHINE\0v1\0";
const OWNER_DOMAIN: &[u8] = b"WAYLAND-NANO-OFFLINE-BOOTSTRAP-OWNER\0v1\0";
const HOME_DOMAIN: &[u8] = b"WAYLAND-NANO-OFFLINE-BOOTSTRAP-HOME\0v1\0";
const MAX_LIFETIME_SECONDS: i64 = 15 * 60;

#[derive(Debug, thiserror::Error)]
pub enum OfflineBootstrapError {
    #[error("offline bootstrap is supported only on Windows")]
    UnsupportedPlatform,
    #[error("offline bootstrap arguments are outside the signed exception")]
    Scope,
    #[error("offline bootstrap authorization is unavailable or insecure")]
    AuthorizationCustody,
    #[error("offline bootstrap document is malformed")]
    Malformed,
    #[error("offline bootstrap document is not canonical JCS")]
    Noncanonical,
    #[error("offline bootstrap authorization signature is invalid")]
    Signature,
    #[error("offline bootstrap authorization is outside its time window")]
    Expired,
    #[error("offline bootstrap artifact binding differs")]
    Artifact,
    #[error("offline bootstrap machine binding differs")]
    Machine,
    #[error("offline bootstrap owner binding differs")]
    Owner,
    #[error("offline bootstrap Nano-home binding differs")]
    Home,
    #[error("offline bootstrap proposed authority differs")]
    Snapshot,
    #[error("offline bootstrap key binding differs")]
    Key,
    #[error("offline bootstrap requires an RDP session for this exception")]
    RemoteSession,
    #[error("offline bootstrap ledger is corrupt or requires reconciliation")]
    LedgerCorrupt,
    #[error("offline bootstrap authorization was consumed by conflicting state")]
    ReplayConflict,
    #[error("offline bootstrap challenge is already live")]
    ChallengeLive,
    #[error("offline bootstrap authority already exists")]
    AlreadyBootstrapped,
    #[error("offline bootstrap I/O failed")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    #[error(transparent)]
    KeyProvider(#[from] KeyProviderError),
    #[error(transparent)]
    SignerProvider(#[from] SignerProviderError),
    #[error(transparent)]
    Bootstrap(#[from] BootstrapError),
}

#[derive(Debug, Clone)]
pub struct OfflineBootstrapResult {
    pub bootstrap_receipt: Vec<u8>,
    pub consumption_receipt: Vec<u8>,
    pub already_bootstrapped_same_authorization: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Challenge {
    schema: String,
    operation_id: String,
    authorization_id: String,
    purpose: String,
    authorization_key_id: String,
    authorization_counter: u64,
    challenge_nonce: String,
    admin_id: String,
    machine_binding_sha256: String,
    owner_sid_binding_sha256: String,
    nano_home_binding_sha256: String,
    proposed_snapshot_sha256: String,
    admin_root_key_fingerprint: String,
    recovery_root_key_fingerprint: String,
    receipt_signer_key_fingerprint: String,
    local_cli_issuer_key_fingerprint: String,
    nano_source_commit_sha: String,
    cargo_lock_sha256: String,
    executable_sha256: String,
    issued_at: String,
    not_before: String,
    not_after: String,
    amendment_sha256: String,
    challenge_intent_record_sha256: String,
    challenge_intent_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    schema: String,
    operation_id: String,
    authorization_id: String,
    purpose: String,
    authorization_key_id: String,
    authorization_counter: u64,
    challenge_nonce: String,
    admin_id: String,
    machine_binding_sha256: String,
    owner_sid_binding_sha256: String,
    nano_home_binding_sha256: String,
    proposed_snapshot_sha256: String,
    admin_root_key_fingerprint: String,
    recovery_root_key_fingerprint: String,
    receipt_signer_key_fingerprint: String,
    local_cli_issuer_key_fingerprint: String,
    nano_source_commit_sha: String,
    cargo_lock_sha256: String,
    executable_sha256: String,
    issued_at: String,
    not_before: String,
    not_after: String,
    amendment_sha256: String,
    challenge_intent_record_sha256: String,
    challenge_intent_sequence: u64,
    challenge_sha256: String,
    reason: String,
    owner_directed_agent_operated_bootstrap: bool,
    physical_console_presence_replaced: bool,
    remote_session_observed: bool,
    signature_alg: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OfflineConsumptionReceipt {
    schema: String,
    operation_id: String,
    authorization_id: String,
    authorization_key_id: String,
    authorization_counter: u64,
    challenge_sha256: String,
    authorization_sha256: String,
    issued_at: String,
    not_after: String,
    machine_binding_sha256: String,
    owner_sid_binding_sha256: String,
    nano_home_binding_sha256: String,
    proposed_snapshot_sha256: String,
    admin_root_key_fingerprint: String,
    recovery_root_key_fingerprint: String,
    receipt_signer_key_fingerprint: String,
    local_cli_issuer_key_fingerprint: String,
    nano_source_commit_sha: String,
    cargo_lock_sha256: String,
    executable_sha256: String,
    global_reservation_position: u64,
    target_accepted_position: u64,
    authority_bootstrap_position: u64,
    authority_receipt_position: u64,
    bootstrap_receipt_sha256: String,
    target_completion_position: u64,
    global_completion_position: u64,
    owner_directed_agent_operated_bootstrap: bool,
    physical_console_presence_replaced: bool,
    remote_session_observed: bool,
    result: String,
    receipt_signer_key_id: String,
    signature: String,
}

impl Authorization {
    fn challenge(&self) -> Challenge {
        Challenge {
            schema: CHALLENGE_SCHEMA.into(),
            operation_id: self.operation_id.clone(),
            authorization_id: self.authorization_id.clone(),
            purpose: self.purpose.clone(),
            authorization_key_id: self.authorization_key_id.clone(),
            authorization_counter: self.authorization_counter,
            challenge_nonce: self.challenge_nonce.clone(),
            admin_id: self.admin_id.clone(),
            machine_binding_sha256: self.machine_binding_sha256.clone(),
            owner_sid_binding_sha256: self.owner_sid_binding_sha256.clone(),
            nano_home_binding_sha256: self.nano_home_binding_sha256.clone(),
            proposed_snapshot_sha256: self.proposed_snapshot_sha256.clone(),
            admin_root_key_fingerprint: self.admin_root_key_fingerprint.clone(),
            recovery_root_key_fingerprint: self.recovery_root_key_fingerprint.clone(),
            receipt_signer_key_fingerprint: self.receipt_signer_key_fingerprint.clone(),
            local_cli_issuer_key_fingerprint: self.local_cli_issuer_key_fingerprint.clone(),
            nano_source_commit_sha: self.nano_source_commit_sha.clone(),
            cargo_lock_sha256: self.cargo_lock_sha256.clone(),
            executable_sha256: self.executable_sha256.clone(),
            issued_at: self.issued_at.clone(),
            not_before: self.not_before.clone(),
            not_after: self.not_after.clone(),
            amendment_sha256: self.amendment_sha256.clone(),
            challenge_intent_record_sha256: self.challenge_intent_record_sha256.clone(),
            challenge_intent_sequence: self.challenge_intent_sequence,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "transition", rename_all = "snake_case", deny_unknown_fields)]
enum Transition {
    ChallengeIntent {
        bindings: Box<ChallengeIntent>,
    },
    Reservation {
        authorization: ConsumptionBinding,
    },
    AuthorizationAccepted {
        authorization: ConsumptionBinding,
    },
    BootstrapCompleted {
        authorization_digest: String,
        bootstrap_record_sha256: String,
        bootstrap_receipt_sha256: String,
        authority_bootstrap_position: u64,
        authority_receipt_position: u64,
    },
    Completed {
        authorization_digest: String,
        target_completion_record_sha256: String,
        bootstrap_receipt_sha256: String,
        consumption_receipt_sha256: String,
        consumption_receipt: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ChallengeIntent {
    operation_id: String,
    authorization_id: String,
    authorization_key_id: String,
    authorization_counter: u64,
    challenge_nonce: String,
    admin_id: String,
    machine_binding_sha256: String,
    owner_sid_binding_sha256: String,
    nano_home_binding_sha256: String,
    proposed_snapshot_sha256: String,
    admin_root_key_fingerprint: String,
    recovery_root_key_fingerprint: String,
    receipt_signer_key_fingerprint: String,
    local_cli_issuer_key_fingerprint: String,
    nano_source_commit_sha: String,
    cargo_lock_sha256: String,
    executable_sha256: String,
    issued_at: String,
    not_before: String,
    not_after: String,
    amendment_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConsumptionBinding {
    operation_id: String,
    authorization_id: String,
    authorization_key_id: String,
    authorization_counter: u64,
    machine_binding_sha256: String,
    owner_sid_binding_sha256: String,
    nano_home_binding_sha256: String,
    challenge_sha256: String,
    authorization_sha256: String,
    proposed_snapshot_sha256: String,
    not_after: String,
    accepted_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerRecord {
    sequence: u64,
    previous_record_sha256: String,
    record_sha256: String,
    transition: Transition,
}

#[derive(Serialize)]
struct RecordPreimage<'a> {
    sequence: u64,
    previous_record_sha256: &'a str,
    transition: &'a Transition,
}

struct Ledger {
    file: File,
    records: Vec<LedgerRecord>,
    last_digest: String,
}

struct BootstrapContext {
    snapshot: AuthoritySnapshot,
    receipt_signer: Option<Box<dyn ReceiptSigner>>,
    artifact: ArtifactIdentity,
    machine: String,
    owner: String,
    home: String,
    fingerprints: [[u8; 32]; 4],
}

/// Generate or exactly replay the single durable Phase-2 challenge.
fn prepare_offline_challenge(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: &str,
    output_path: &Path,
) -> Result<Vec<u8>, OfflineBootstrapError> {
    let global_path = global_ledger_path()?;
    prepare_offline_challenge_at(nano_home, paths, admin_id, output_path, &global_path)
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn prepare_offline_challenge_at(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: &str,
    output_path: &Path,
    global_path: &Path,
) -> Result<Vec<u8>, OfflineBootstrapError> {
    ensure_scope(admin_id)?;
    #[cfg(not(windows))]
    return Err(OfflineBootstrapError::UnsupportedPlatform);
    #[cfg(windows)]
    {
        let context = build_context(nano_home, paths, admin_id, true)?;
        ensure_authority_absent(nano_home)?;
        let _global_lock = lock_for(global_path)?;
        let target_path = target_ledger_path(nano_home);
        let authority_lock = AuthorityStore::acquire_authority_lock(nano_home)?;
        let global = Ledger::open(global_path, "global")?;
        if !global.records.is_empty() {
            return Err(OfflineBootstrapError::ReplayConflict);
        }
        let mut target = Ledger::open(&target_path, "target")?;
        let now = Utc::now();
        let existing = target
            .records
            .iter()
            .find_map(|record| match &record.transition {
                Transition::ChallengeIntent { bindings } => Some((record, bindings)),
                _ => None,
            });
        let challenge = if let Some((record, intent)) = existing {
            let challenge = challenge_from_intent(intent, record);
            if parse_time(&challenge.not_after)? < now {
                return Err(OfflineBootstrapError::ChallengeLive);
            }
            if !intent_matches_context(intent, &context) {
                return Err(OfflineBootstrapError::ReplayConflict);
            }
            challenge
        } else {
            if !target.records.is_empty() {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            let issued = now
                .with_nanosecond(0)
                .ok_or(OfflineBootstrapError::Malformed)?;
            let intent = ChallengeIntent {
                operation_id: "initial-admin-bootstrap-phase2-1".into(),
                authorization_id: AUTHORIZATION_ID.into(),
                authorization_key_id: AUTHORIZATION_KEY_ID.into(),
                authorization_counter: AUTHORIZATION_COUNTER,
                challenge_nonce: URL_SAFE_NO_PAD.encode(random_nonce()?),
                admin_id: ADMIN_ID.into(),
                machine_binding_sha256: context.machine.clone(),
                owner_sid_binding_sha256: context.owner.clone(),
                nano_home_binding_sha256: context.home.clone(),
                proposed_snapshot_sha256: context.snapshot.digest()?,
                admin_root_key_fingerprint: hex_digest(context.fingerprints[0]),
                recovery_root_key_fingerprint: hex_digest(context.fingerprints[1]),
                receipt_signer_key_fingerprint: hex_digest(context.fingerprints[2]),
                local_cli_issuer_key_fingerprint: hex_digest(context.fingerprints[3]),
                nano_source_commit_sha: context.artifact.source_commit_sha.clone(),
                cargo_lock_sha256: context.artifact.cargo_lock_sha256.clone(),
                executable_sha256: context.artifact.executable_sha256.clone(),
                issued_at: timestamp(issued),
                not_before: timestamp(issued),
                not_after: timestamp(issued + chrono::Duration::seconds(MAX_LIFETIME_SECONDS)),
                amendment_sha256: AMENDMENT_SHA256.into(),
            };
            let record = target.append(Transition::ChallengeIntent {
                bindings: Box::new(intent.clone()),
            })?;
            challenge_from_intent(&intent, &record)
        };
        let bytes = serde_jcs::to_vec(&challenge).map_err(|_| OfflineBootstrapError::Malformed)?;
        secure_write_new(output_path, &bytes)?;
        drop(authority_lock);
        Ok(bytes)
    }
}

/// Consume the one signed authorization and install or exactly replay authority.
fn apply_offline_authorization(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: &str,
    authorization_path: &Path,
) -> Result<OfflineBootstrapResult, OfflineBootstrapError> {
    let global_path = global_ledger_path()?;
    apply_offline_authorization_at(nano_home, paths, admin_id, authorization_path, &global_path)
}

#[cfg_attr(not(windows), allow(unused_variables))]
fn apply_offline_authorization_at(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: &str,
    authorization_path: &Path,
    global_path: &Path,
) -> Result<OfflineBootstrapResult, OfflineBootstrapError> {
    ensure_scope(admin_id)?;
    #[cfg(not(windows))]
    return Err(OfflineBootstrapError::UnsupportedPlatform);
    #[cfg(windows)]
    {
        let raw = secure_read(authorization_path)?;
        let authorization = parse_authorization(&raw)?;
        verify_authorization_envelope(&authorization, &raw)?;
        verify_home(nano_home, false)?;
        if !remote_session_observed()? {
            return Err(OfflineBootstrapError::RemoteSession);
        }
        let challenge = authorization.challenge();
        let challenge_bytes =
            serde_jcs::to_vec(&challenge).map_err(|_| OfflineBootstrapError::Malformed)?;
        let challenge_digest = hex(&Sha256::digest(&challenge_bytes));
        if !ct_eq_hex(&challenge_digest, &authorization.challenge_sha256, 32) {
            return Err(OfflineBootstrapError::ReplayConflict);
        }
        let authorization_digest = hex(&Sha256::digest(&raw));
        let _global_lock = lock_for(global_path)?;
        let target_path = target_ledger_path(nano_home);
        let authority_lock = AuthorityStore::acquire_authority_lock(nano_home)?;
        let mut global = Ledger::open(global_path, "global")?;
        let mut target = Ledger::open(&target_path, "target")?;
        let authority_path = nano_home.join("activation/authority.jsonl");
        let (existing_bootstrap_snapshot, _, existing_receipt, _) =
            crate::journal::replay(&authority_path)?;
        if let Some(completed) = find_transition_record(
            &global,
            |transition| matches!(transition, Transition::Completed { authorization_digest: digest, .. } if ct_eq_hex(digest, &authorization_digest, 32)),
        ) {
            let snapshot = existing_bootstrap_snapshot
                .clone()
                .ok_or(OfflineBootstrapError::LedgerCorrupt)?;
            let bootstrap_receipt = existing_receipt
                .clone()
                .ok_or(OfflineBootstrapError::LedgerCorrupt)?;
            let context = replay_context(nano_home, snapshot)?;
            crate::admin::verify_bootstrap_receipt_snapshot(&bootstrap_receipt, &context.snapshot)?;
            verify_authorization_bindings(&authorization, &context)?;
            verify_challenge_record(&target, &challenge, &context)?;
            let binding = global
                .records
                .iter()
                .find_map(|record| match &record.transition {
                    Transition::Reservation { authorization } => Some(authorization.clone()),
                    _ => None,
                })
                .ok_or(OfflineBootstrapError::LedgerCorrupt)?;
            validate_durable_binding(&binding, &authorization, &authorization_digest)?;
            validate_consumption_state(&global, &target, &binding)?;
            let target_completion = find_transition_record(
                &target,
                |transition| matches!(transition, Transition::BootstrapCompleted { authorization_digest: digest, .. } if ct_eq_hex(digest, &authorization_digest, 32)),
            )
            .ok_or(OfflineBootstrapError::LedgerCorrupt)?;
            let Transition::BootstrapCompleted {
                authorization_digest: target_authorization_digest,
                bootstrap_record_sha256: target_bootstrap_record,
                bootstrap_receipt_sha256: target_bootstrap_receipt,
                authority_bootstrap_position,
                authority_receipt_position,
            } = &target_completion.transition
            else {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            };
            if !ct_eq_hex(target_authorization_digest, &authorization_digest, 32)
                || !ct_eq_hex(
                    target_bootstrap_record,
                    &authority_bootstrap_record_digest(nano_home)?,
                    32,
                )
                || !ct_eq_hex(
                    target_bootstrap_receipt,
                    &hex(&Sha256::digest(&bootstrap_receipt)),
                    32,
                )
                || *authority_bootstrap_position != 1
                || *authority_receipt_position != 2
            {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            let Transition::Completed {
                target_completion_record_sha256,
                bootstrap_receipt_sha256,
                consumption_receipt_sha256,
                consumption_receipt,
                ..
            } = &completed.transition
            else {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            };
            let stored = consumption_receipt.as_bytes().to_vec();
            if !ct_eq_hex(
                target_completion_record_sha256,
                &target_completion.record_sha256,
                32,
            ) || !ct_eq_hex(
                bootstrap_receipt_sha256,
                &hex(&Sha256::digest(&bootstrap_receipt)),
                32,
            ) || !ct_eq_hex(
                consumption_receipt_sha256,
                &hex(&Sha256::digest(&stored)),
                32,
            ) {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            verify_offline_consumption_receipt(
                &stored,
                &context
                    .snapshot
                    .receipt_signer_public_key
                    .ok_or(OfflineBootstrapError::LedgerCorrupt)?,
            )?;
            validate_consumption_receipt_fields(
                &stored,
                &authorization,
                &binding,
                &context,
                &hex(&Sha256::digest(&bootstrap_receipt)),
                completed.sequence,
                target_completion.sequence,
            )?;
            let receipt_value = crate::raw::parse_transport_frame(&stored)
                .map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
            let stored_receipt: OfflineConsumptionReceipt =
                serde_json::from_value(receipt_value)
                    .map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
            if !ct_eq_hex(
                &stored_receipt.bootstrap_receipt_sha256,
                &hex(&Sha256::digest(&bootstrap_receipt)),
                32,
            ) || stored_receipt.global_reservation_position
                != global
                    .records
                    .iter()
                    .find(|record| matches!(&record.transition, Transition::Reservation { .. }))
                    .map(|record| record.sequence)
                    .ok_or(OfflineBootstrapError::LedgerCorrupt)?
                || stored_receipt.target_accepted_position
                    != target
                        .records
                        .iter()
                        .find(|record| {
                            matches!(&record.transition, Transition::AuthorizationAccepted { .. })
                        })
                        .map(|record| record.sequence)
                        .ok_or(OfflineBootstrapError::LedgerCorrupt)?
                || stored_receipt.target_completion_position != target_completion.sequence
                || stored_receipt.global_completion_position != completed.sequence
            {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            drop(authority_lock);
            return Ok(OfflineBootstrapResult {
                bootstrap_receipt,
                consumption_receipt: stored,
                already_bootstrapped_same_authorization: true,
            });
        }
        let context = build_context(nano_home, paths, admin_id, false)?;
        verify_authorization_bindings(&authorization, &context)?;
        let durable_binding = global
            .records
            .iter()
            .find_map(|record| match &record.transition {
                Transition::Reservation { authorization } => Some(authorization.clone()),
                _ => None,
            })
            .or_else(|| {
                target
                    .records
                    .iter()
                    .find_map(|record| match &record.transition {
                        Transition::AuthorizationAccepted { authorization } => {
                            Some(authorization.clone())
                        }
                        _ => None,
                    })
            });
        let binding = if let Some(binding) = durable_binding {
            validate_durable_binding(&binding, &authorization, &authorization_digest)?;
            binding
        } else {
            verify_authorization_current_time(&authorization)?;
            consumption_binding(
                &authorization,
                &authorization_digest,
                timestamp(
                    Utc::now()
                        .with_nanosecond(0)
                        .ok_or(OfflineBootstrapError::Malformed)?,
                ),
            )
        };
        verify_challenge_record(&target, &challenge, &context)?;
        validate_consumption_state(&global, &target, &binding)?;

        if existing_bootstrap_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot != &context.snapshot)
        {
            return Err(OfflineBootstrapError::AlreadyBootstrapped);
        }
        let has_matching_reservation = has_transition(
            &global,
            |transition| matches!(transition, Transition::Reservation { authorization } if authorization == &binding),
        );
        let has_matching_intent = has_transition(
            &target,
            |transition| matches!(transition, Transition::AuthorizationAccepted { authorization } if authorization == &binding),
        );
        if existing_receipt.is_some() && !(has_matching_reservation && has_matching_intent) {
            return Err(OfflineBootstrapError::AlreadyBootstrapped);
        }

        if !has_matching_reservation {
            global.append(Transition::Reservation {
                authorization: binding.clone(),
            })?;
        }
        if !has_matching_intent {
            target.append(Transition::AuthorizationAccepted {
                authorization: binding.clone(),
            })?;
        }

        let already = existing_receipt.is_some();
        let receipt = sign_bootstrap_receipt(
            &context.snapshot,
            context
                .receipt_signer
                .as_deref()
                .ok_or(OfflineBootstrapError::Key)?,
        )?;
        let store = AuthorityStore::bootstrap_initial_with_held_lock(
            nano_home,
            context.snapshot.clone(),
            receipt,
            authority_lock,
        )?;
        let bootstrap_receipt = store
            .bootstrap_receipt()
            .ok_or(OfflineBootstrapError::LedgerCorrupt)?
            .to_vec();
        let bootstrap_receipt_digest = hex(&Sha256::digest(&bootstrap_receipt));
        let bootstrap_record_digest = authority_bootstrap_record_digest(nano_home)?;
        let target_completion = find_transition_record(
            &target,
            |t| matches!(t, Transition::BootstrapCompleted { authorization_digest: digest, .. } if digest == &authorization_digest),
        );
        let target_completion_record = if let Some(record) = target_completion {
            let Transition::BootstrapCompleted {
                bootstrap_record_sha256,
                bootstrap_receipt_sha256,
                authority_bootstrap_position,
                authority_receipt_position,
                ..
            } = &record.transition
            else {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            };
            if !ct_eq_hex(bootstrap_record_sha256, &bootstrap_record_digest, 32)
                || !ct_eq_hex(bootstrap_receipt_sha256, &bootstrap_receipt_digest, 32)
                || *authority_bootstrap_position != 1
                || *authority_receipt_position != 2
            {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            record.clone()
        } else {
            target.append(Transition::BootstrapCompleted {
                authorization_digest: authorization_digest.clone(),
                bootstrap_record_sha256: bootstrap_record_digest,
                bootstrap_receipt_sha256: bootstrap_receipt_digest.clone(),
                authority_bootstrap_position: 1,
                authority_receipt_position: 2,
            })?
        };
        let global_completion = find_transition_record(
            &global,
            |t| matches!(t, Transition::Completed { authorization_digest: digest, .. } if digest == &authorization_digest),
        );
        let consumption_receipt = if let Some(record) = global_completion {
            let Transition::Completed {
                target_completion_record_sha256,
                bootstrap_receipt_sha256: recorded_receipt,
                consumption_receipt_sha256,
                consumption_receipt,
                ..
            } = &record.transition
            else {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            };
            let stored = consumption_receipt.as_bytes().to_vec();
            if !ct_eq_hex(
                target_completion_record_sha256,
                &target_completion_record.record_sha256,
                32,
            ) || !ct_eq_hex(recorded_receipt, &bootstrap_receipt_digest, 32)
                || !ct_eq_hex(
                    consumption_receipt_sha256,
                    &hex(&Sha256::digest(&stored)),
                    32,
                )
            {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            verify_offline_consumption_receipt(
                &stored,
                &context
                    .snapshot
                    .receipt_signer_public_key
                    .ok_or(OfflineBootstrapError::LedgerCorrupt)?,
            )?;
            stored
        } else {
            let global_completion_position = global.records.len() as u64 + 1;
            let stored = sign_consumption_receipt(
                &authorization,
                &authorization_digest,
                &bootstrap_receipt_digest,
                &context,
                &global,
                &target,
                global_completion_position,
            )?;
            validate_consumption_receipt_fields(
                &stored,
                &authorization,
                &binding,
                &context,
                &bootstrap_receipt_digest,
                global_completion_position,
                target_completion_record.sequence,
            )?;
            global.append(Transition::Completed {
                authorization_digest: authorization_digest.clone(),
                target_completion_record_sha256: target_completion_record.record_sha256.clone(),
                bootstrap_receipt_sha256: bootstrap_receipt_digest.clone(),
                consumption_receipt_sha256: hex(&Sha256::digest(&stored)),
                consumption_receipt: String::from_utf8(stored.clone())
                    .map_err(|_| OfflineBootstrapError::Malformed)?,
            })?;
            stored
        };
        Ok(OfflineBootstrapResult {
            bootstrap_receipt,
            consumption_receipt,
            already_bootstrapped_same_authorization: already,
        })
    }
}

/// Strict, offline verification for the public consumption receipt.
///
/// This operation reads no Nano home, ledger, configuration, environment, or
/// private key. The caller supplies only the already-pinned receipt-signer
/// public key whose lifecycle evidence is verified separately.
pub fn verify_offline_consumption_receipt(
    raw: &[u8],
    public_key: &[u8; 32],
) -> Result<(), OfflineBootstrapError> {
    let value =
        crate::raw::parse_transport_frame(raw).map_err(|_| OfflineBootstrapError::Malformed)?;
    if serde_jcs::to_vec(&value).map_err(|_| OfflineBootstrapError::Malformed)? != raw {
        return Err(OfflineBootstrapError::Noncanonical);
    }
    let receipt: OfflineConsumptionReceipt =
        serde_json::from_value(value.clone()).map_err(|_| OfflineBootstrapError::Malformed)?;
    if receipt.schema != RECEIPT_SCHEMA
        || receipt.authorization_id != AUTHORIZATION_ID
        || receipt.authorization_key_id != AUTHORIZATION_KEY_ID
        || receipt.authorization_counter != AUTHORIZATION_COUNTER
        || !receipt.owner_directed_agent_operated_bootstrap
        || !receipt.physical_console_presence_replaced
        || !receipt.remote_session_observed
        || receipt.result != consumption_receipt_result()
        || receipt.authority_bootstrap_position != 1
        || receipt.authority_receipt_position != 2
        || receipt.global_reservation_position == 0
        || receipt.target_accepted_position == 0
        || receipt.target_completion_position == 0
        || receipt.global_completion_position == 0
        || receipt.global_completion_position <= receipt.global_reservation_position
        || receipt.target_completion_position <= receipt.target_accepted_position
        || receipt.operation_id.is_empty()
        || receipt.receipt_signer_key_id.is_empty()
    {
        return Err(OfflineBootstrapError::Malformed);
    }
    if parse_time(&receipt.issued_at)? > parse_time(&receipt.not_after)? {
        return Err(OfflineBootstrapError::Malformed);
    }
    for digest in [
        &receipt.challenge_sha256,
        &receipt.authorization_sha256,
        &receipt.machine_binding_sha256,
        &receipt.owner_sid_binding_sha256,
        &receipt.nano_home_binding_sha256,
        &receipt.proposed_snapshot_sha256,
        &receipt.admin_root_key_fingerprint,
        &receipt.recovery_root_key_fingerprint,
        &receipt.receipt_signer_key_fingerprint,
        &receipt.local_cli_issuer_key_fingerprint,
        &receipt.cargo_lock_sha256,
        &receipt.executable_sha256,
        &receipt.bootstrap_receipt_sha256,
    ] {
        decode_lower_hex(digest, 32).ok_or(OfflineBootstrapError::Malformed)?;
    }
    decode_lower_hex(&receipt.nano_source_commit_sha, 20)
        .ok_or(OfflineBootstrapError::Malformed)?;
    let signer_fingerprint = hex_digest(*public_key);
    let expected_key_id = format!("receipt-ed25519-{}", &signer_fingerprint[..32]);
    if !ct_eq_hex(
        &receipt.receipt_signer_key_fingerprint,
        &signer_fingerprint,
        32,
    ) || receipt.receipt_signer_key_id != expected_key_id
    {
        return Err(OfflineBootstrapError::Signature);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(&receipt.signature)
        .map_err(|_| OfflineBootstrapError::Signature)?;
    if signature.len() != 64 {
        return Err(OfflineBootstrapError::Signature);
    }
    let mut unsigned = value;
    unsigned
        .as_object_mut()
        .ok_or(OfflineBootstrapError::Malformed)?
        .remove("signature")
        .ok_or(OfflineBootstrapError::Malformed)?;
    let canonical = serde_jcs::to_vec(&unsigned).map_err(|_| OfflineBootstrapError::Malformed)?;
    let mut message = RECEIPT_DOMAIN.to_vec();
    message.extend_from_slice(&canonical);
    VerifyingKey::from_bytes(public_key)
        .map_err(|_| OfflineBootstrapError::Signature)?
        .verify(
            &message,
            &Signature::from_slice(&signature).map_err(|_| OfflineBootstrapError::Signature)?,
        )
        .map_err(|_| OfflineBootstrapError::Signature)
}

fn validate_consumption_receipt_fields(
    raw: &[u8],
    authorization: &Authorization,
    binding: &ConsumptionBinding,
    context: &BootstrapContext,
    bootstrap_receipt_sha256: &str,
    global_completion_position: u64,
    target_completion_position: u64,
) -> Result<(), OfflineBootstrapError> {
    let value =
        crate::raw::parse_transport_frame(raw).map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
    let receipt: OfflineConsumptionReceipt =
        serde_json::from_value(value).map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
    let expected = context.fingerprints.map(hex_digest);
    if receipt.operation_id != authorization.operation_id
        || receipt.authorization_id != AUTHORIZATION_ID
        || receipt.authorization_key_id != AUTHORIZATION_KEY_ID
        || receipt.authorization_counter != AUTHORIZATION_COUNTER
        || !ct_eq_hex(
            &receipt.challenge_sha256,
            &authorization.challenge_sha256,
            32,
        )
        || !ct_eq_hex(
            &receipt.authorization_sha256,
            &binding.authorization_sha256,
            32,
        )
        || receipt.issued_at != authorization.issued_at
        || receipt.not_after != authorization.not_after
        || !ct_eq_hex(&receipt.machine_binding_sha256, &context.machine, 32)
        || !ct_eq_hex(&receipt.owner_sid_binding_sha256, &context.owner, 32)
        || !ct_eq_hex(&receipt.nano_home_binding_sha256, &context.home, 32)
        || !ct_eq_hex(
            &receipt.proposed_snapshot_sha256,
            &context.snapshot.digest()?,
            32,
        )
        || !ct_eq_hex(&receipt.admin_root_key_fingerprint, &expected[0], 32)
        || !ct_eq_hex(&receipt.recovery_root_key_fingerprint, &expected[1], 32)
        || !ct_eq_hex(&receipt.receipt_signer_key_fingerprint, &expected[2], 32)
        || !ct_eq_hex(&receipt.local_cli_issuer_key_fingerprint, &expected[3], 32)
        || !ct_eq_hex(
            &receipt.nano_source_commit_sha,
            &context.artifact.source_commit_sha,
            20,
        )
        || !ct_eq_hex(
            &receipt.cargo_lock_sha256,
            &context.artifact.cargo_lock_sha256,
            32,
        )
        || !ct_eq_hex(
            &receipt.executable_sha256,
            &context.artifact.executable_sha256,
            32,
        )
        || !ct_eq_hex(
            &receipt.bootstrap_receipt_sha256,
            bootstrap_receipt_sha256,
            32,
        )
        || receipt.global_completion_position != global_completion_position
        || receipt.target_completion_position != target_completion_position
    {
        return Err(OfflineBootstrapError::LedgerCorrupt);
    }
    Ok(())
}

pub(crate) fn run_cli(home: &Path, args: &[String], out: &mut dyn Write) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("offline-bootstrap-challenge") => {
            parse_cli(args.get(1..).unwrap_or_default(), "--output").and_then(
                |(paths, document)| {
                    prepare_offline_challenge(home, &paths, ADMIN_ID, &document).and_then(|bytes| {
                        String::from_utf8(bytes).map_err(|_| OfflineBootstrapError::Malformed)
                    })
                },
            )
        }
        Some("offline-bootstrap-apply") => parse_cli(
            args.get(1..).unwrap_or_default(),
            "--authorization",
        )
        .and_then(|(paths, document)| {
            apply_offline_authorization(home, &paths, ADMIN_ID, &document).and_then(|result| {
                let status = if result.already_bootstrapped_same_authorization {
                    "already_bootstrapped_same_authorization"
                } else {
                    "bootstrapped"
                };
                let bootstrap = String::from_utf8(result.bootstrap_receipt)
                    .map_err(|_| OfflineBootstrapError::Malformed)?;
                let consumption = String::from_utf8(result.consumption_receipt)
                    .map_err(|_| OfflineBootstrapError::Malformed)?;
                Ok(format!(
                    "offline bootstrap result: {status}\n{bootstrap}\n{consumption}"
                ))
            })
        }),
        _ => Err(OfflineBootstrapError::Scope),
    };
    match result {
        Ok(message) => {
            let _ = writeln!(out, "{message}");
            0
        }
        Err(error) => {
            eprintln!("wayland-nano: {}", refusal(&error));
            2
        }
    }
}

fn parse_cli(
    args: &[String],
    document_flag: &'static str,
) -> Result<(BootstrapKeyPaths, PathBuf), OfflineBootstrapError> {
    let mut values = std::collections::BTreeMap::<&str, &str>::new();
    let mut chunks = args.chunks_exact(2);
    for pair in &mut chunks {
        let name = pair[0].as_str();
        if !matches!(
            name,
            "--admin-root-keyref"
                | "--recovery-root-keyref"
                | "--receipt-signer-keyref"
                | "--local-cli-keyref"
                | "--output"
                | "--authorization"
        ) || (name != document_flag && matches!(name, "--output" | "--authorization"))
            || values.insert(name, pair[1].as_str()).is_some()
        {
            return Err(OfflineBootstrapError::Scope);
        }
    }
    if !chunks.remainder().is_empty() {
        return Err(OfflineBootstrapError::Scope);
    }
    let absolute = |name: &str| -> Result<PathBuf, OfflineBootstrapError> {
        let value = values.get(name).ok_or(OfflineBootstrapError::Scope)?;
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(OfflineBootstrapError::Scope);
        }
        Ok(path)
    };
    Ok((
        BootstrapKeyPaths {
            admin_root: absolute("--admin-root-keyref")?,
            recovery_root: absolute("--recovery-root-keyref")?,
            receipt_signer: absolute("--receipt-signer-keyref")?,
            local_cli_issuer: absolute("--local-cli-keyref")?,
        },
        absolute(document_flag)?,
    ))
}

fn refusal(error: &OfflineBootstrapError) -> &'static str {
    match error {
        OfflineBootstrapError::UnsupportedPlatform => {
            "offline bootstrap is supported only on Windows"
        }
        OfflineBootstrapError::Scope => "offline bootstrap is outside the signed exception",
        OfflineBootstrapError::AuthorizationCustody => {
            "offline bootstrap authorization custody refused"
        }
        OfflineBootstrapError::Malformed => "offline bootstrap document malformed",
        OfflineBootstrapError::Noncanonical => "offline bootstrap document noncanonical",
        OfflineBootstrapError::Signature => "offline bootstrap signature refused",
        OfflineBootstrapError::Expired => "offline bootstrap authorization expired",
        OfflineBootstrapError::Artifact => "offline bootstrap artifact binding refused",
        OfflineBootstrapError::Machine => "offline bootstrap machine binding refused",
        OfflineBootstrapError::Owner => "offline bootstrap owner binding refused",
        OfflineBootstrapError::Home => "offline bootstrap Nano-home binding refused",
        OfflineBootstrapError::Snapshot => "offline bootstrap authority snapshot refused",
        OfflineBootstrapError::Key => "offline bootstrap key binding refused",
        OfflineBootstrapError::RemoteSession => {
            "offline bootstrap remote-session observation refused"
        }
        OfflineBootstrapError::LedgerCorrupt => "offline bootstrap ledger requires reconciliation",
        OfflineBootstrapError::ReplayConflict => "offline bootstrap replay conflict",
        OfflineBootstrapError::ChallengeLive => "offline bootstrap challenge conflict",
        OfflineBootstrapError::AlreadyBootstrapped => "offline bootstrap authority already exists",
        OfflineBootstrapError::Io(_) => "offline bootstrap I/O refused",
        OfflineBootstrapError::Authority(_) => "offline bootstrap authority commit refused",
        OfflineBootstrapError::KeyProvider(_) => "offline bootstrap key reference refused",
        OfflineBootstrapError::SignerProvider(_) => "offline bootstrap key custody refused",
        OfflineBootstrapError::Bootstrap(_) => "offline bootstrap authority refused",
    }
}

fn ensure_scope(admin_id: &str) -> Result<(), OfflineBootstrapError> {
    if admin_id == ADMIN_ID {
        Ok(())
    } else {
        Err(OfflineBootstrapError::Scope)
    }
}

fn build_context(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: &str,
    allow_home_create: bool,
) -> Result<BootstrapContext, OfflineBootstrapError> {
    ensure_scope(admin_id)?;
    verify_home(nano_home, allow_home_create)?;
    let references = [
        load_key_reference(&paths.admin_root, KeyRole::AdminRoot)?,
        load_key_reference(&paths.recovery_root, KeyRole::RecoveryRoot)?,
        load_key_reference(&paths.receipt_signer, KeyRole::ReceiptSigner)?,
        load_key_reference(&paths.local_cli_issuer, KeyRole::LocalCliIssuer)?,
    ];
    for left in 0..references.len() {
        for right in left + 1..references.len() {
            if (references[left].provider(), references[left].reference())
                == (references[right].provider(), references[right].reference())
            {
                return Err(OfflineBootstrapError::Key);
            }
        }
    }
    let public = [
        derive_public_key(&references[0], KeyRole::AdminRoot)?,
        derive_public_key(&references[1], KeyRole::RecoveryRoot)?,
        derive_public_key(&references[2], KeyRole::ReceiptSigner)?,
        derive_public_key(&references[3], KeyRole::LocalCliIssuer)?,
    ];
    for left in 0..public.len() {
        if public[left + 1..].contains(&public[left]) {
            return Err(OfflineBootstrapError::Key);
        }
    }
    if public.contains(&AUTHORIZATION_PUBLIC_KEY) {
        return Err(OfflineBootstrapError::Key);
    }
    let receipt_signer = ExternalReceiptSigner::from_key_reference(&references[2])?;
    receipt_signer
        .preflight()
        .map_err(|_| OfflineBootstrapError::Key)?;
    let snapshot = AuthoritySnapshot::empty(ADMIN_ID, public[0])
        .with_recovery_key(public[1])
        .with_service_keys(public[2], public[3]);
    Ok(BootstrapContext {
        snapshot,
        receipt_signer: Some(Box::new(receipt_signer)),
        artifact: running_artifact()?,
        machine: machine_binding()?,
        owner: owner_binding()?,
        home: home_binding(nano_home)?,
        fingerprints: public,
    })
}

fn replay_context(
    nano_home: &Path,
    snapshot: AuthoritySnapshot,
) -> Result<BootstrapContext, OfflineBootstrapError> {
    verify_home(nano_home, false)?;
    let public = [
        snapshot.admin_public_key,
        snapshot
            .recovery_public_key
            .ok_or(OfflineBootstrapError::LedgerCorrupt)?,
        snapshot
            .receipt_signer_public_key
            .ok_or(OfflineBootstrapError::LedgerCorrupt)?,
        snapshot
            .local_cli_public_key
            .ok_or(OfflineBootstrapError::LedgerCorrupt)?,
    ];
    Ok(BootstrapContext {
        snapshot,
        receipt_signer: None,
        artifact: running_artifact()?,
        machine: machine_binding()?,
        owner: owner_binding()?,
        home: home_binding(nano_home)?,
        fingerprints: public,
    })
}

fn challenge_from_intent(intent: &ChallengeIntent, record: &LedgerRecord) -> Challenge {
    Challenge {
        schema: CHALLENGE_SCHEMA.into(),
        operation_id: intent.operation_id.clone(),
        authorization_id: intent.authorization_id.clone(),
        purpose: PURPOSE.into(),
        authorization_key_id: intent.authorization_key_id.clone(),
        authorization_counter: intent.authorization_counter,
        challenge_nonce: intent.challenge_nonce.clone(),
        admin_id: intent.admin_id.clone(),
        machine_binding_sha256: intent.machine_binding_sha256.clone(),
        owner_sid_binding_sha256: intent.owner_sid_binding_sha256.clone(),
        nano_home_binding_sha256: intent.nano_home_binding_sha256.clone(),
        proposed_snapshot_sha256: intent.proposed_snapshot_sha256.clone(),
        admin_root_key_fingerprint: intent.admin_root_key_fingerprint.clone(),
        recovery_root_key_fingerprint: intent.recovery_root_key_fingerprint.clone(),
        receipt_signer_key_fingerprint: intent.receipt_signer_key_fingerprint.clone(),
        local_cli_issuer_key_fingerprint: intent.local_cli_issuer_key_fingerprint.clone(),
        nano_source_commit_sha: intent.nano_source_commit_sha.clone(),
        cargo_lock_sha256: intent.cargo_lock_sha256.clone(),
        executable_sha256: intent.executable_sha256.clone(),
        issued_at: intent.issued_at.clone(),
        not_before: intent.not_before.clone(),
        not_after: intent.not_after.clone(),
        amendment_sha256: intent.amendment_sha256.clone(),
        challenge_intent_record_sha256: record.record_sha256.clone(),
        challenge_intent_sequence: record.sequence,
    }
}

fn intent_matches_context(intent: &ChallengeIntent, context: &BootstrapContext) -> bool {
    intent.authorization_id == AUTHORIZATION_ID
        && intent.authorization_key_id == AUTHORIZATION_KEY_ID
        && intent.authorization_counter == AUTHORIZATION_COUNTER
        && intent.admin_id == ADMIN_ID
        && ct_eq_hex(&intent.machine_binding_sha256, &context.machine, 32)
        && ct_eq_hex(&intent.owner_sid_binding_sha256, &context.owner, 32)
        && ct_eq_hex(&intent.nano_home_binding_sha256, &context.home, 32)
        && ct_eq_hex(
            &intent.proposed_snapshot_sha256,
            &context.snapshot.digest().unwrap_or_default(),
            32,
        )
        && ct_eq_hex(
            &intent.nano_source_commit_sha,
            &context.artifact.source_commit_sha,
            20,
        )
        && ct_eq_hex(
            &intent.cargo_lock_sha256,
            &context.artifact.cargo_lock_sha256,
            32,
        )
        && ct_eq_hex(
            &intent.executable_sha256,
            &context.artifact.executable_sha256,
            32,
        )
        && ct_eq_hex(&intent.amendment_sha256, AMENDMENT_SHA256, 32)
}

fn parse_authorization(raw: &[u8]) -> Result<Authorization, OfflineBootstrapError> {
    let value =
        crate::raw::parse_transport_frame(raw).map_err(|_| OfflineBootstrapError::Malformed)?;
    if serde_jcs::to_vec(&value).map_err(|_| OfflineBootstrapError::Malformed)? != raw {
        return Err(OfflineBootstrapError::Noncanonical);
    }
    serde_json::from_value(value).map_err(|_| OfflineBootstrapError::Malformed)
}

fn verify_authorization_envelope(
    authorization: &Authorization,
    raw: &[u8],
) -> Result<(), OfflineBootstrapError> {
    if authorization.schema != AUTHORIZATION_SCHEMA
        || authorization.authorization_id != AUTHORIZATION_ID
        || authorization.authorization_key_id != AUTHORIZATION_KEY_ID
        || authorization.authorization_counter != AUTHORIZATION_COUNTER
        || authorization.purpose != PURPOSE
        || authorization.admin_id != ADMIN_ID
        || authorization.reason != REASON
        || authorization.signature_alg != "Ed25519"
        || !authorization.owner_directed_agent_operated_bootstrap
        || !authorization.physical_console_presence_replaced
        || !authorization.remote_session_observed
        || authorization.amendment_sha256 != AMENDMENT_SHA256
    {
        return Err(OfflineBootstrapError::Scope);
    }
    verify_authorization_signature(authorization, raw, &AUTHORIZATION_PUBLIC_KEY)?;

    let issued = parse_time(&authorization.issued_at)?;
    let before = parse_time(&authorization.not_before)?;
    let after = parse_time(&authorization.not_after)?;
    if before > issued
        || issued > after
        || after - issued > chrono::Duration::seconds(MAX_LIFETIME_SECONDS)
    {
        return Err(OfflineBootstrapError::Expired);
    }
    Ok(())
}

fn verify_authorization_bindings(
    authorization: &Authorization,
    context: &BootstrapContext,
) -> Result<(), OfflineBootstrapError> {
    if !ct_eq_hex(&authorization.machine_binding_sha256, &context.machine, 32) {
        return Err(OfflineBootstrapError::Machine);
    }
    if !ct_eq_hex(&authorization.owner_sid_binding_sha256, &context.owner, 32) {
        return Err(OfflineBootstrapError::Owner);
    }
    if !ct_eq_hex(&authorization.nano_home_binding_sha256, &context.home, 32) {
        return Err(OfflineBootstrapError::Home);
    }
    if !ct_eq_hex(
        &authorization.proposed_snapshot_sha256,
        &context.snapshot.digest()?,
        32,
    ) {
        return Err(OfflineBootstrapError::Snapshot);
    }
    if !ct_eq_hex(
        &authorization.nano_source_commit_sha,
        &context.artifact.source_commit_sha,
        20,
    ) || !ct_eq_hex(
        &authorization.cargo_lock_sha256,
        &context.artifact.cargo_lock_sha256,
        32,
    ) || !ct_eq_hex(
        &authorization.executable_sha256,
        &context.artifact.executable_sha256,
        32,
    ) {
        return Err(OfflineBootstrapError::Artifact);
    }
    let expected = context.fingerprints.map(hex_digest);
    if !ct_eq_hex(&authorization.admin_root_key_fingerprint, &expected[0], 32)
        || !ct_eq_hex(
            &authorization.recovery_root_key_fingerprint,
            &expected[1],
            32,
        )
        || !ct_eq_hex(
            &authorization.receipt_signer_key_fingerprint,
            &expected[2],
            32,
        )
        || !ct_eq_hex(
            &authorization.local_cli_issuer_key_fingerprint,
            &expected[3],
            32,
        )
    {
        return Err(OfflineBootstrapError::Key);
    }
    Ok(())
}

fn verify_authorization_signature(
    authorization: &Authorization,
    raw: &[u8],
    public_key: &[u8; 32],
) -> Result<(), OfflineBootstrapError> {
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(&authorization.signature)
        .map_err(|_| OfflineBootstrapError::Signature)?;
    if signature_bytes.len() != 64 {
        return Err(OfflineBootstrapError::Signature);
    }
    let mut unsigned: serde_json::Value =
        serde_json::from_slice(raw).map_err(|_| OfflineBootstrapError::Malformed)?;
    unsigned
        .as_object_mut()
        .ok_or(OfflineBootstrapError::Malformed)?
        .remove("signature");
    let canonical = serde_jcs::to_vec(&unsigned).map_err(|_| OfflineBootstrapError::Malformed)?;
    let mut message = SIGNATURE_DOMAIN.to_vec();
    message.extend_from_slice(&canonical);
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| OfflineBootstrapError::Signature)?;
    VerifyingKey::from_bytes(public_key)
        .map_err(|_| OfflineBootstrapError::Signature)?
        .verify(&message, &signature)
        .map_err(|_| OfflineBootstrapError::Signature)?;

    Ok(())
}

fn verify_authorization_current_time(
    authorization: &Authorization,
) -> Result<(), OfflineBootstrapError> {
    let now = Utc::now();
    if now < parse_time(&authorization.not_before)? || now > parse_time(&authorization.not_after)? {
        return Err(OfflineBootstrapError::Expired);
    }
    Ok(())
}

fn validate_durable_binding(
    binding: &ConsumptionBinding,
    authorization: &Authorization,
    authorization_digest: &str,
) -> Result<(), OfflineBootstrapError> {
    if binding.operation_id != authorization.operation_id
        || binding.authorization_id != AUTHORIZATION_ID
        || binding.authorization_key_id != AUTHORIZATION_KEY_ID
        || binding.authorization_counter != AUTHORIZATION_COUNTER
        || !ct_eq_hex(
            &binding.challenge_sha256,
            &authorization.challenge_sha256,
            32,
        )
        || !ct_eq_hex(&binding.authorization_sha256, authorization_digest, 32)
        || !ct_eq_hex(
            &binding.proposed_snapshot_sha256,
            &authorization.proposed_snapshot_sha256,
            32,
        )
        || binding.not_after != authorization.not_after
    {
        return Err(OfflineBootstrapError::ReplayConflict);
    }
    let accepted = parse_time(&binding.accepted_at)?;
    if accepted < parse_time(&authorization.not_before)?
        || accepted > parse_time(&authorization.not_after)?
    {
        return Err(OfflineBootstrapError::LedgerCorrupt);
    }
    Ok(())
}

fn verify_challenge_record(
    target: &Ledger,
    challenge: &Challenge,
    context: &BootstrapContext,
) -> Result<(), OfflineBootstrapError> {
    let record = target
        .records
        .iter()
        .find(|record| {
            record.sequence == challenge.challenge_intent_sequence
                && record.record_sha256 == challenge.challenge_intent_record_sha256
        })
        .ok_or(OfflineBootstrapError::ReplayConflict)?;
    let Transition::ChallengeIntent { bindings } = &record.transition else {
        return Err(OfflineBootstrapError::ReplayConflict);
    };
    if challenge_from_intent(bindings, record) != *challenge
        || !intent_matches_context(bindings, context)
    {
        return Err(OfflineBootstrapError::ReplayConflict);
    }
    Ok(())
}

fn consumption_binding(
    authorization: &Authorization,
    digest: &str,
    accepted_at: String,
) -> ConsumptionBinding {
    ConsumptionBinding {
        operation_id: authorization.operation_id.clone(),
        authorization_id: AUTHORIZATION_ID.into(),
        authorization_key_id: AUTHORIZATION_KEY_ID.into(),
        authorization_counter: AUTHORIZATION_COUNTER,
        machine_binding_sha256: authorization.machine_binding_sha256.clone(),
        owner_sid_binding_sha256: authorization.owner_sid_binding_sha256.clone(),
        nano_home_binding_sha256: authorization.nano_home_binding_sha256.clone(),
        challenge_sha256: authorization.challenge_sha256.clone(),
        authorization_sha256: digest.into(),
        proposed_snapshot_sha256: authorization.proposed_snapshot_sha256.clone(),
        not_after: authorization.not_after.clone(),
        accepted_at,
    }
}

fn validate_consumption_state(
    global: &Ledger,
    target: &Ledger,
    expected: &ConsumptionBinding,
) -> Result<(), OfflineBootstrapError> {
    let mut reservations = 0usize;
    let mut acceptances = 0usize;
    let mut target_completions = 0usize;
    let mut global_completions = 0usize;
    let mut challenge_sequence = None;
    let mut reservation_sequence = None;
    let mut acceptance_sequence = None;
    let mut target_completion_sequence = None;
    let mut global_completion_sequence = None;
    for record in global.records.iter().chain(target.records.iter()) {
        match &record.transition {
            Transition::Reservation { authorization }
            | Transition::AuthorizationAccepted { authorization }
                if authorization != expected =>
            {
                return Err(OfflineBootstrapError::ReplayConflict);
            }
            Transition::BootstrapCompleted {
                authorization_digest,
                ..
            }
            | Transition::Completed {
                authorization_digest,
                ..
            } if authorization_digest != &expected.authorization_sha256 => {
                return Err(OfflineBootstrapError::ReplayConflict);
            }
            _ => {}
        }
    }
    for record in &global.records {
        match &record.transition {
            Transition::Reservation { .. } => {
                reservations += 1;
                reservation_sequence = Some(record.sequence);
            }
            Transition::Completed { .. } => {
                global_completions += 1;
                global_completion_sequence = Some(record.sequence);
            }
            Transition::ChallengeIntent { .. }
            | Transition::AuthorizationAccepted { .. }
            | Transition::BootstrapCompleted { .. } => {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
        }
    }
    for record in &target.records {
        match &record.transition {
            Transition::ChallengeIntent { .. } => challenge_sequence = Some(record.sequence),
            Transition::AuthorizationAccepted { .. } => {
                acceptances += 1;
                acceptance_sequence = Some(record.sequence);
            }
            Transition::BootstrapCompleted { .. } => {
                target_completions += 1;
                target_completion_sequence = Some(record.sequence);
            }
            Transition::Reservation { .. } | Transition::Completed { .. } => {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
        }
    }
    if reservations > 1
        || acceptances > 1
        || target_completions > 1
        || global_completions > 1
        || challenge_sequence != Some(1)
        || (acceptances > 0 && reservations != 1)
        || (target_completions > 0 && (reservations != 1 || acceptances != 1))
        || (global_completions > 0
            && (reservations != 1 || acceptances != 1 || target_completions != 1))
        || acceptance_sequence.is_some_and(|accepted| accepted <= challenge_sequence.unwrap_or(0))
        || target_completion_sequence
            .is_some_and(|completed| completed <= acceptance_sequence.unwrap_or(0))
        || global_completion_sequence
            .is_some_and(|completed| completed <= reservation_sequence.unwrap_or(0))
    {
        return Err(OfflineBootstrapError::LedgerCorrupt);
    }
    Ok(())
}

fn sign_consumption_receipt(
    authorization: &Authorization,
    authorization_digest: &str,
    bootstrap_receipt_digest: &str,
    context: &BootstrapContext,
    global: &Ledger,
    target: &Ledger,
    global_completion_position: u64,
) -> Result<Vec<u8>, OfflineBootstrapError> {
    let position = |ledger: &Ledger,
                    predicate: fn(&Transition) -> bool|
     -> Result<u64, OfflineBootstrapError> {
        ledger
            .records
            .iter()
            .find(|r| predicate(&r.transition))
            .map(|r| r.sequence)
            .ok_or(OfflineBootstrapError::LedgerCorrupt)
    };
    let mut value = serde_json::json!({
        "schema": RECEIPT_SCHEMA,
        "operation_id": authorization.operation_id,
        "authorization_id": AUTHORIZATION_ID,
        "authorization_key_id": AUTHORIZATION_KEY_ID,
        "authorization_counter": AUTHORIZATION_COUNTER,
        "challenge_sha256": authorization.challenge_sha256,
        "authorization_sha256": authorization_digest,
        "issued_at": authorization.issued_at,
        "not_after": authorization.not_after,
        "machine_binding_sha256": context.machine,
        "owner_sid_binding_sha256": context.owner,
        "nano_home_binding_sha256": context.home,
        "proposed_snapshot_sha256": authorization.proposed_snapshot_sha256,
        "admin_root_key_fingerprint": authorization.admin_root_key_fingerprint,
        "recovery_root_key_fingerprint": authorization.recovery_root_key_fingerprint,
        "receipt_signer_key_fingerprint": authorization.receipt_signer_key_fingerprint,
        "local_cli_issuer_key_fingerprint": authorization.local_cli_issuer_key_fingerprint,
        "nano_source_commit_sha": context.artifact.source_commit_sha,
        "cargo_lock_sha256": context.artifact.cargo_lock_sha256,
        "executable_sha256": context.artifact.executable_sha256,
        "global_reservation_position": position(global, |t| matches!(t, Transition::Reservation { .. }))?,
        "target_accepted_position": position(target, |t| matches!(t, Transition::AuthorizationAccepted { .. }))?,
        "authority_bootstrap_position": 1,
        "authority_receipt_position": 2,
        "bootstrap_receipt_sha256": bootstrap_receipt_digest,
        "target_completion_position": position(target, |t| matches!(t, Transition::BootstrapCompleted { .. }))?,
        "global_completion_position": global_completion_position,
        "owner_directed_agent_operated_bootstrap": true,
        "physical_console_presence_replaced": true,
        "remote_session_observed": true,
        // Exact replay returns this byte-identical original decision receipt;
        // the out-of-band result flag reports that no second install occurred.
        "result": consumption_receipt_result(),
        "receipt_signer_key_id": context.receipt_signer.as_deref().ok_or(OfflineBootstrapError::Key)?.key_id()
    });
    let canonical = serde_jcs::to_vec(&value).map_err(|_| OfflineBootstrapError::Malformed)?;
    let mut message = RECEIPT_DOMAIN.to_vec();
    message.extend_from_slice(&canonical);
    let signature = context
        .receipt_signer
        .as_deref()
        .ok_or(OfflineBootstrapError::Key)?
        .sign(&message)
        .map_err(|_| OfflineBootstrapError::Key)?;
    value.as_object_mut().unwrap().insert(
        "signature".into(),
        serde_json::Value::String(URL_SAFE_NO_PAD.encode(signature)),
    );
    serde_jcs::to_vec(&value).map_err(|_| OfflineBootstrapError::Malformed)
}

impl Ledger {
    fn open(path: &Path, kind: &str) -> Result<Self, OfflineBootstrapError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        reject_reparse_chain(path)?;
        let mut file = open_ledger_file(path)?;
        audit_owner_only_path(path).map_err(|_| OfflineBootstrapError::AuthorizationCustody)?;
        let before = file_identity(&file)?;
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut bytes)?;
        if file_identity(&file)? != before {
            return Err(OfflineBootstrapError::LedgerCorrupt);
        }
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            let keep = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
            bytes.truncate(keep);
            file.set_len(keep as u64)?;
            file.sync_all()?;
        }
        let mut previous = genesis(kind);
        let mut records = Vec::new();
        for (index, line) in bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .enumerate()
        {
            let sequence = index as u64 + 1;
            let value = crate::raw::parse_transport_frame(line)
                .map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
            if serde_jcs::to_vec(&value).map_err(|_| OfflineBootstrapError::LedgerCorrupt)? != line
            {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            let record: LedgerRecord =
                serde_json::from_value(value).map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
            if record.sequence != sequence
                || record.previous_record_sha256 != previous
                || record_digest(
                    record.sequence,
                    &record.previous_record_sha256,
                    &record.transition,
                )? != record.record_sha256
            {
                return Err(OfflineBootstrapError::LedgerCorrupt);
            }
            previous = record.record_sha256.clone();
            records.push(record);
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            file,
            records,
            last_digest: previous,
        })
    }

    fn append(&mut self, transition: Transition) -> Result<LedgerRecord, OfflineBootstrapError> {
        let sequence = self.records.len() as u64 + 1;
        let digest = record_digest(sequence, &self.last_digest, &transition)?;
        let record = LedgerRecord {
            sequence,
            previous_record_sha256: self.last_digest.clone(),
            record_sha256: digest.clone(),
            transition,
        };
        let mut bytes =
            serde_jcs::to_vec(&record).map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
        bytes.push(b'\n');
        let before = file_identity(&self.file)?;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        let after = file_identity(&self.file)?;
        if after.0 != before.0 || after.1 != before.1 || after.2 != before.2 + bytes.len() as u64 {
            return Err(OfflineBootstrapError::LedgerCorrupt);
        }
        self.last_digest = digest;
        self.records.push(record.clone());
        Ok(record)
    }
}

#[cfg(windows)]
fn open_ledger_file(path: &Path) -> Result<File, OfflineBootstrapError> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & 0x400 != 0 {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    Ok(file)
}

#[cfg(unix)]
fn open_ledger_file(path: &Path) -> Result<File, OfflineBootstrapError> {
    use std::os::unix::fs::OpenOptionsExt as _;
    Ok(OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?)
}

fn record_digest(
    sequence: u64,
    previous: &str,
    transition: &Transition,
) -> Result<String, OfflineBootstrapError> {
    let preimage = RecordPreimage {
        sequence,
        previous_record_sha256: previous,
        transition,
    };
    let bytes = serde_jcs::to_vec(&preimage).map_err(|_| OfflineBootstrapError::LedgerCorrupt)?;
    Ok(hex(&Sha256::digest(bytes)))
}

fn genesis(kind: &str) -> String {
    let mut input = GENESIS_DOMAIN.to_vec();
    input.extend_from_slice(&AUTHORIZATION_PUBLIC_KEY);
    input.extend_from_slice(&decode_hex_32(AMENDMENT_SHA256));
    input.extend_from_slice(kind.as_bytes());
    hex(&Sha256::digest(input))
}

fn has_transition(ledger: &Ledger, predicate: impl Fn(&Transition) -> bool) -> bool {
    ledger
        .records
        .iter()
        .any(|record| predicate(&record.transition))
}

fn find_transition_record(
    ledger: &Ledger,
    predicate: impl Fn(&Transition) -> bool,
) -> Option<&LedgerRecord> {
    ledger
        .records
        .iter()
        .find(|record| predicate(&record.transition))
}

fn target_ledger_path(home: &Path) -> PathBuf {
    home.join("activation/offline-bootstrap-v1.jsonl")
}

fn consumption_receipt_result() -> &'static str {
    "bootstrapped"
}
fn lock_for(path: &Path) -> Result<FileLock, OfflineBootstrapError> {
    let lock = path.with_extension("lock");
    if let Some(parent) = lock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    FileLock::try_acquire(&lock).map_err(|_| OfflineBootstrapError::LedgerCorrupt)
}

fn ensure_authority_absent(home: &Path) -> Result<(), OfflineBootstrapError> {
    let activation = home.join("activation");
    for name in [
        "authority.jsonl",
        "authority.db",
        "authority.db-wal",
        "authority.db-shm",
    ] {
        if std::fs::metadata(activation.join(name))
            .map(|m| m.len() != 0)
            .unwrap_or(false)
        {
            return Err(OfflineBootstrapError::AlreadyBootstrapped);
        }
    }
    Ok(())
}

fn authority_bootstrap_record_digest(home: &Path) -> Result<String, OfflineBootstrapError> {
    let bytes = std::fs::read(home.join("activation/authority.jsonl"))?;
    let first = bytes
        .split(|b| *b == b'\n')
        .next()
        .ok_or(OfflineBootstrapError::LedgerCorrupt)?;
    if first.is_empty() {
        return Err(OfflineBootstrapError::LedgerCorrupt);
    }
    Ok(hex(&Sha256::digest(first)))
}

fn secure_read(path: &Path) -> Result<Vec<u8>, OfflineBootstrapError> {
    if !path.is_absolute() {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    reject_reparse_chain(path)?;
    #[cfg(windows)]
    if drive_type(path)? != 3 || !is_ntfs(path)? {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    audit_owner_only_path(path).map_err(|_| OfflineBootstrapError::AuthorizationCustody)?;
    let mut file = open_read_no_follow(path)?;
    #[cfg(windows)]
    if file_link_count(&file)? != 1 {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    let before = file_identity(&file)?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 || file_identity(&file)? != before {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    Ok(bytes)
}

fn secure_write_new(path: &Path, bytes: &[u8]) -> Result<(), OfflineBootstrapError> {
    if !path.is_absolute() {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    let parent = path
        .parent()
        .ok_or(OfflineBootstrapError::AuthorizationCustody)?;
    reject_reparse_chain(parent)?;
    #[cfg(windows)]
    if drive_type(parent)? != 3 || !is_ntfs(parent)? {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    audit_owner_only_path(parent).map_err(|_| OfflineBootstrapError::AuthorizationCustody)?;
    if path.exists() {
        return if secure_read(path)? == bytes {
            Ok(())
        } else {
            Err(OfflineBootstrapError::ReplayConflict)
        };
    }
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    #[cfg(windows)]
    if !file.metadata()?.is_file() || file_link_count(&file)? != 1 {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    audit_owner_only_path(path).map_err(|_| OfflineBootstrapError::AuthorizationCustody)
}

fn verify_home(path: &Path, allow_create: bool) -> Result<(), OfflineBootstrapError> {
    if !path.is_absolute() {
        return Err(OfflineBootstrapError::Home);
    }
    if allow_create {
        std::fs::create_dir_all(path)?;
    } else if !path.is_dir() {
        return Err(OfflineBootstrapError::Home);
    }
    reject_reparse_chain(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(OfflineBootstrapError::Home);
    }
    #[cfg(windows)]
    if drive_type(path).map_err(|_| OfflineBootstrapError::Home)? != 3
        || !is_ntfs(path).map_err(|_| OfflineBootstrapError::Home)?
    {
        return Err(OfflineBootstrapError::Home);
    }
    audit_owner_only_path(path).map_err(|_| OfflineBootstrapError::Home)
}

fn reject_reparse_chain(path: &Path) -> Result<(), OfflineBootstrapError> {
    let mut cursor = Some(path);
    while let Some(item) = cursor {
        if let Ok(metadata) = std::fs::symlink_metadata(item) {
            if metadata.file_type().is_symlink() {
                return Err(OfflineBootstrapError::AuthorizationCustody);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt as _;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(OfflineBootstrapError::AuthorizationCustody);
                }
            }
        }
        cursor = item.parent();
    }
    Ok(())
}

fn running_artifact() -> Result<ArtifactIdentity, OfflineBootstrapError> {
    let path = std::env::current_exe().map_err(|_| OfflineBootstrapError::Artifact)?;
    let mut file = open_running_executable(&path)?;
    let before = file_identity(&file)?;
    let before_len = file.metadata()?.len();
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    if file_identity(&file)? != before || file.metadata()?.len() != before_len {
        return Err(OfflineBootstrapError::Artifact);
    }
    crate::build_identity::compiled()
        .bind_executable(&hex(&hasher.finalize()))
        .map_err(|_| OfflineBootstrapError::Artifact)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, OfflineBootstrapError> {
    let parsed =
        DateTime::parse_from_rfc3339(value).map_err(|_| OfflineBootstrapError::Malformed)?;
    if parsed.offset().local_minus_utc() != 0 || value.len() != 20 || !value.ends_with('Z') {
        return Err(OfflineBootstrapError::Malformed);
    }
    Ok(parsed.with_timezone(&Utc))
}
fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}
fn hex_digest(value: [u8; 32]) -> String {
    hex(&Sha256::digest(value))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_lower_hex(value: &str, byte_len: usize) -> Option<Vec<u8>> {
    if value.len() != byte_len.checked_mul(2)? {
        return None;
    }
    let mut decoded = Vec::with_capacity(byte_len);
    for pair in value.as_bytes().chunks_exact(2) {
        if !pair
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return None;
        }
        let high = if pair[0].is_ascii_digit() {
            pair[0] - b'0'
        } else {
            pair[0] - b'a' + 10
        };
        let low = if pair[1].is_ascii_digit() {
            pair[1] - b'0'
        } else {
            pair[1] - b'a' + 10
        };
        decoded.push((high << 4) | low);
    }
    Some(decoded)
}

#[inline(never)]
fn ct_eq_hex(left: &str, right: &str, byte_len: usize) -> bool {
    let Some(left) = decode_lower_hex(left, byte_len) else {
        return false;
    };
    let Some(right) = decode_lower_hex(right, byte_len) else {
        return false;
    };
    let mut difference = 0u8;
    for index in 0..byte_len {
        difference |= left[index] ^ right[index];
    }
    std::hint::black_box(difference) == 0
}
fn decode_hex_32(value: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    out
}

#[cfg(not(windows))]
fn machine_binding() -> Result<String, OfflineBootstrapError> {
    Err(OfflineBootstrapError::UnsupportedPlatform)
}
#[cfg(not(windows))]
fn owner_binding() -> Result<String, OfflineBootstrapError> {
    Err(OfflineBootstrapError::UnsupportedPlatform)
}
#[cfg(not(windows))]
fn home_binding(_: &Path) -> Result<String, OfflineBootstrapError> {
    Err(OfflineBootstrapError::UnsupportedPlatform)
}
#[cfg(not(windows))]
fn global_ledger_path() -> Result<PathBuf, OfflineBootstrapError> {
    Err(OfflineBootstrapError::UnsupportedPlatform)
}
#[cfg(not(windows))]
fn random_nonce() -> Result<[u8; 32], OfflineBootstrapError> {
    Err(OfflineBootstrapError::UnsupportedPlatform)
}
#[cfg(not(windows))]
fn remote_session_observed() -> Result<bool, OfflineBootstrapError> {
    Err(OfflineBootstrapError::UnsupportedPlatform)
}
#[cfg(not(windows))]
fn open_read_no_follow(path: &Path) -> Result<File, OfflineBootstrapError> {
    Ok(File::open(path)?)
}
#[cfg(not(windows))]
fn open_running_executable(path: &Path) -> Result<File, OfflineBootstrapError> {
    Ok(File::open(path)?)
}
#[cfg(not(windows))]
fn file_identity(file: &File) -> Result<(u64, u64, u64), OfflineBootstrapError> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok((m.dev(), m.ino(), m.len()))
}

#[cfg(windows)]
fn file_link_count(file: &File) -> Result<u32, OfflineBootstrapError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(info.nNumberOfLinks)
}

#[cfg(windows)]
fn random_nonce() -> Result<[u8; 32], OfflineBootstrapError> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let mut out = [0u8; 32];
    if unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            out.as_mut_ptr(),
            out.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } != 0
    {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    Ok(out)
}

#[cfg(windows)]
fn machine_binding() -> Result<String, OfflineBootstrapError> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW,
    };
    let subkey: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "MachineGuid\0".encode_utf16().collect();
    let mut bytes = 0u32;
    if unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    } != 0
        || bytes < 4
    {
        return Err(OfflineBootstrapError::Machine);
    }
    let mut guid = vec![0u8; bytes as usize];
    if unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            guid.as_mut_ptr().cast(),
            &mut bytes,
        )
    } != 0
    {
        return Err(OfflineBootstrapError::Machine);
    }
    let _ = RegCloseKey; // RegGetValueW used a predefined handle; it must not be closed.
    let serial = system_volume_serial()?;
    let mut input = MACHINE_DOMAIN.to_vec();
    input.extend_from_slice(&guid[..bytes as usize]);
    input.extend_from_slice(&serial.to_le_bytes());
    Ok(hex(&Sha256::digest(input)))
}

#[cfg(windows)]
fn system_volume_serial() -> Result<u32, OfflineBootstrapError> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let required = unsafe { GetSystemDirectoryW(std::ptr::null_mut(), 0) };
    if required == 0 {
        return Err(OfflineBootstrapError::Machine);
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let written = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if written == 0 || written as usize >= buffer.len() {
        return Err(OfflineBootstrapError::Machine);
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..written as usize]));
    volume_serial(&path)
}

#[cfg(windows)]
fn owner_binding() -> Result<String, OfflineBootstrapError> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = 0isize;
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(OfflineBootstrapError::Owner);
    }
    let mut size = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
    let mut buffer = vec![0u8; size as usize];
    if size == 0
        || unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } == 0
    {
        unsafe { CloseHandle(token) };
        return Err(OfflineBootstrapError::Owner);
    }
    unsafe { CloseHandle(token) };
    let user = unsafe { std::ptr::read_unaligned(buffer.as_ptr() as *const TOKEN_USER) };
    let sid_len = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if sid_len == 0 {
        return Err(OfflineBootstrapError::Owner);
    }
    let sid = unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), sid_len) };
    let mut input = OWNER_DOMAIN.to_vec();
    input.extend_from_slice(sid);
    Ok(hex(&Sha256::digest(input)))
}

#[cfg(windows)]
fn home_binding(path: &Path) -> Result<String, OfflineBootstrapError> {
    let canonical = std::fs::canonicalize(path)?;
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&canonical)?;
    if file.metadata()?.file_attributes() & 0x400 != 0 {
        return Err(OfflineBootstrapError::Home);
    }
    let identity = file_identity(&file)?;
    let serial = volume_serial(&canonical)?;
    let mut input = HOME_DOMAIN.to_vec();
    use std::os::windows::ffi::OsStrExt as _;
    for unit in canonical.as_os_str().encode_wide() {
        input.extend_from_slice(&unit.to_le_bytes());
    }
    input.extend_from_slice(&serial.to_le_bytes());
    input.extend_from_slice(&identity.0.to_le_bytes());
    input.extend_from_slice(&identity.1.to_le_bytes());
    Ok(hex(&Sha256::digest(input)))
}

#[cfg(windows)]
fn global_ledger_path() -> Result<PathBuf, OfflineBootstrapError> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    let mut raw = std::ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT as u32, 0, &mut raw) }
        != S_OK
        || raw.is_null()
    {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    let len = unsafe { (0..).find(|i| *raw.add(*i) == 0).unwrap_or(0) };
    let path = PathBuf::from(std::ffi::OsString::from_wide(unsafe {
        std::slice::from_raw_parts(raw, len)
    }));
    unsafe { CoTaskMemFree(raw.cast()) };
    let path = path.join("WaylandNano/bootstrap-authorizations-v1.jsonl");
    if drive_type(&path)? != 3 || !is_ntfs(&path)? {
        return Err(OfflineBootstrapError::AuthorizationCustody);
    }
    Ok(path)
}

#[cfg(windows)]
fn remote_session_observed() -> Result<bool, OfflineBootstrapError> {
    use windows_sys::Win32::System::RemoteDesktop::{
        ProcessIdToSessionId, WTS_CURRENT_SERVER_HANDLE, WTSClientProtocolType, WTSFreeMemory,
        WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    let mut session = 0u32;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(OfflineBootstrapError::RemoteSession);
    }
    let mut buffer = std::ptr::null_mut();
    let mut size = 0u32;
    if unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            session,
            WTSClientProtocolType,
            &mut buffer,
            &mut size,
        )
    } == 0
        || buffer.is_null()
        || size < 2
    {
        return Err(OfflineBootstrapError::RemoteSession);
    }
    let protocol = unsafe { *(buffer as *const u16) };
    unsafe { WTSFreeMemory(buffer.cast()) };
    Ok(protocol == 2 && session != unsafe { WTSGetActiveConsoleSessionId() })
}

#[cfg(windows)]
fn open_read_no_follow(path: &Path) -> Result<File, OfflineBootstrapError> {
    use std::os::windows::fs::OpenOptionsExt as _;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?)
}

#[cfg(windows)]
fn open_running_executable(path: &Path) -> Result<File, OfflineBootstrapError> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    Ok(OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)?)
}

#[cfg(windows)]
fn file_identity(file: &File) -> Result<(u64, u64, u64), OfflineBootstrapError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        file.metadata()?.len(),
    ))
}

#[cfg(windows)]
fn volume_serial(path: &Path) -> Result<u32, OfflineBootstrapError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let canonical = std::fs::canonicalize(path)?;
    let text = canonical.as_os_str().to_string_lossy();
    let root = if text.starts_with(r"\\?\") {
        text.get(4..7)
    } else {
        text.get(0..3)
    }
    .ok_or(OfflineBootstrapError::Machine)?;
    let wide: Vec<u16> = std::ffi::OsStr::new(root)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut serial = 0u32;
    if unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(serial)
}

#[cfg(windows)]
fn drive_type(path: &Path) -> Result<u32, OfflineBootstrapError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    let text = path.as_os_str().to_string_lossy();
    let root = text
        .get(0..3)
        .ok_or(OfflineBootstrapError::AuthorizationCustody)?;
    let wide: Vec<u16> = std::ffi::OsStr::new(root)
        .encode_wide()
        .chain(Some(0))
        .collect();
    Ok(unsafe { GetDriveTypeW(wide.as_ptr()) })
}

#[cfg(windows)]
fn is_ntfs(path: &Path) -> Result<bool, OfflineBootstrapError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;
    let text = path.as_os_str().to_string_lossy();
    let root = text
        .get(0..3)
        .ok_or(OfflineBootstrapError::AuthorizationCustody)?;
    let wide: Vec<u16> = std::ffi::OsStr::new(root)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut fs = [0u16; 32];
    if unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let len = fs.iter().position(|u| *u == 0).unwrap_or(fs.len());
    Ok(String::from_utf16_lossy(&fs[..len]).eq_ignore_ascii_case("NTFS"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::ReceiptError;
    use ed25519_dalek::{Signer as _, SigningKey};
    use std::process::Command;

    struct CrashSigner(SigningKey, String);

    #[cfg(windows)]
    struct IsolatedFixture {
        root: tempfile::TempDir,
        home: PathBuf,
        paths: BootstrapKeyPaths,
        challenge: PathBuf,
        global: PathBuf,
    }

    #[cfg(windows)]
    impl IsolatedFixture {
        fn new() -> Self {
            let root = tempfile::Builder::new()
                .prefix("nano-offline-isolated-")
                .tempdir_in(std::env::var_os("LOCALAPPDATA").unwrap())
                .unwrap();
            secure_test_directory(root.path());
            let home = root.path().join("home");
            let custody = root.path().join("custody");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&custody).unwrap();
            secure_test_directory(&home);
            secure_test_directory(&custody);
            let mut refs = Vec::new();
            for (name, role, seed) in [
                ("admin", "admin_root", 11u8),
                ("recovery", "recovery_root", 22u8),
                ("receipt", "receipt_signer", 33u8),
                ("cli", "local_cli_issuer", 44u8),
            ] {
                let key = custody.join(format!("{name}.seed"));
                std::fs::write(&key, [seed; 32]).unwrap();
                secure_test_file(&key);
                let reference = custody.join(format!("{name}.keyref"));
                std::fs::write(
                    &reference,
                    serde_jcs::to_vec(&serde_json::json!({
                        "provider":"file", "reference":key.to_string_lossy(), "role":role
                    }))
                    .unwrap(),
                )
                .unwrap();
                secure_test_file(&reference);
                refs.push(reference);
            }
            Self {
                home,
                paths: BootstrapKeyPaths {
                    admin_root: refs[0].clone(),
                    recovery_root: refs[1].clone(),
                    receipt_signer: refs[2].clone(),
                    local_cli_issuer: refs[3].clone(),
                },
                challenge: root.path().join("challenge.json"),
                global: root.path().join("global.jsonl"),
                root,
            }
        }

        fn prepare(&self, output: &Path) -> Result<Vec<u8>, OfflineBootstrapError> {
            prepare_offline_challenge_at(&self.home, &self.paths, ADMIN_ID, output, &self.global)
        }
    }

    impl CrashSigner {
        fn new() -> Self {
            let key = SigningKey::from_bytes(&[73u8; 32]);
            let fingerprint = hex_digest(key.verifying_key().to_bytes());
            Self(key, format!("receipt-ed25519-{}", &fingerprint[..32]))
        }
    }

    impl ReceiptSigner for CrashSigner {
        fn key_id(&self) -> &str {
            &self.1
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

    fn test_consumption() -> ConsumptionBinding {
        ConsumptionBinding {
            operation_id: "initial-admin-bootstrap-phase2-1".into(),
            authorization_id: AUTHORIZATION_ID.into(),
            authorization_key_id: AUTHORIZATION_KEY_ID.into(),
            authorization_counter: AUTHORIZATION_COUNTER,
            machine_binding_sha256: "11".repeat(32),
            owner_sid_binding_sha256: "22".repeat(32),
            nano_home_binding_sha256: "33".repeat(32),
            challenge_sha256: "44".repeat(32),
            authorization_sha256: "55".repeat(32),
            proposed_snapshot_sha256: "66".repeat(32),
            not_after: "2026-08-30T12:15:00Z".into(),
            accepted_at: "2026-08-30T12:01:00Z".into(),
        }
    }

    fn test_challenge_intent() -> ChallengeIntent {
        ChallengeIntent {
            operation_id: "initial-admin-bootstrap-phase2-1".into(),
            authorization_id: AUTHORIZATION_ID.into(),
            authorization_key_id: AUTHORIZATION_KEY_ID.into(),
            authorization_counter: AUTHORIZATION_COUNTER,
            challenge_nonce: URL_SAFE_NO_PAD.encode([7u8; 32]),
            admin_id: ADMIN_ID.into(),
            machine_binding_sha256: "11".repeat(32),
            owner_sid_binding_sha256: "22".repeat(32),
            nano_home_binding_sha256: "33".repeat(32),
            proposed_snapshot_sha256: "66".repeat(32),
            admin_root_key_fingerprint: "77".repeat(32),
            recovery_root_key_fingerprint: "88".repeat(32),
            receipt_signer_key_fingerprint: "99".repeat(32),
            local_cli_issuer_key_fingerprint: "aa".repeat(32),
            nano_source_commit_sha: "b".repeat(40),
            cargo_lock_sha256: "cc".repeat(32),
            executable_sha256: "dd".repeat(32),
            issued_at: "2026-08-30T12:00:00Z".into(),
            not_before: "2026-08-30T12:00:00Z".into(),
            not_after: "2026-08-30T12:15:00Z".into(),
            amendment_sha256: AMENDMENT_SHA256.into(),
        }
    }

    fn test_authorization() -> Authorization {
        let intent = test_challenge_intent();
        Authorization {
            schema: AUTHORIZATION_SCHEMA.into(),
            operation_id: intent.operation_id,
            authorization_id: AUTHORIZATION_ID.into(),
            purpose: PURPOSE.into(),
            authorization_key_id: AUTHORIZATION_KEY_ID.into(),
            authorization_counter: AUTHORIZATION_COUNTER,
            challenge_nonce: intent.challenge_nonce,
            admin_id: ADMIN_ID.into(),
            machine_binding_sha256: intent.machine_binding_sha256,
            owner_sid_binding_sha256: intent.owner_sid_binding_sha256,
            nano_home_binding_sha256: intent.nano_home_binding_sha256,
            proposed_snapshot_sha256: intent.proposed_snapshot_sha256,
            admin_root_key_fingerprint: intent.admin_root_key_fingerprint,
            recovery_root_key_fingerprint: intent.recovery_root_key_fingerprint,
            receipt_signer_key_fingerprint: intent.receipt_signer_key_fingerprint,
            local_cli_issuer_key_fingerprint: intent.local_cli_issuer_key_fingerprint,
            nano_source_commit_sha: intent.nano_source_commit_sha,
            cargo_lock_sha256: intent.cargo_lock_sha256,
            executable_sha256: intent.executable_sha256,
            issued_at: intent.issued_at,
            not_before: intent.not_before,
            not_after: intent.not_after,
            amendment_sha256: AMENDMENT_SHA256.into(),
            challenge_intent_record_sha256: "ab".repeat(32),
            challenge_intent_sequence: 1,
            challenge_sha256: "cd".repeat(32),
            reason: REASON.into(),
            owner_directed_agent_operated_bootstrap: true,
            physical_console_presence_replaced: true,
            remote_session_observed: true,
            signature_alg: "Ed25519".into(),
            signature: String::new(),
        }
    }

    fn test_ledger(kind: &str, transitions: Vec<Transition>) -> Ledger {
        let file = tempfile::tempfile().unwrap();
        let mut previous = genesis(kind);
        let mut records = Vec::new();
        for (index, transition) in transitions.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            let digest = record_digest(sequence, &previous, &transition).unwrap();
            records.push(LedgerRecord {
                sequence,
                previous_record_sha256: previous,
                record_sha256: digest.clone(),
                transition,
            });
            previous = digest;
        }
        Ledger {
            file,
            records,
            last_digest: previous,
        }
    }

    fn test_consumption_receipt(key: &SigningKey) -> Vec<u8> {
        let fingerprint = hex_digest(key.verifying_key().to_bytes());
        let mut value = serde_json::json!({
            "schema": RECEIPT_SCHEMA,
            "operation_id": "initial-admin-bootstrap-phase2-1",
            "authorization_id": AUTHORIZATION_ID,
            "authorization_key_id": AUTHORIZATION_KEY_ID,
            "authorization_counter": AUTHORIZATION_COUNTER,
            "challenge_sha256": "11".repeat(32),
            "authorization_sha256": "22".repeat(32),
            "issued_at": "2026-08-30T12:00:00Z",
            "not_after": "2026-08-30T12:15:00Z",
            "machine_binding_sha256": "33".repeat(32),
            "owner_sid_binding_sha256": "44".repeat(32),
            "nano_home_binding_sha256": "55".repeat(32),
            "proposed_snapshot_sha256": "66".repeat(32),
            "admin_root_key_fingerprint": "77".repeat(32),
            "recovery_root_key_fingerprint": "88".repeat(32),
            "receipt_signer_key_fingerprint": fingerprint.clone(),
            "local_cli_issuer_key_fingerprint": "aa".repeat(32),
            "nano_source_commit_sha": "b".repeat(40),
            "cargo_lock_sha256": "cc".repeat(32),
            "executable_sha256": "dd".repeat(32),
            "global_reservation_position": 1,
            "target_accepted_position": 2,
            "authority_bootstrap_position": 1,
            "authority_receipt_position": 2,
            "bootstrap_receipt_sha256": "ee".repeat(32),
            "target_completion_position": 3,
            "global_completion_position": 2,
            "owner_directed_agent_operated_bootstrap": true,
            "physical_console_presence_replaced": true,
            "remote_session_observed": true,
            "result": "bootstrapped",
            "receipt_signer_key_id": format!("receipt-ed25519-{}", &fingerprint[..32])
        });
        let canonical = serde_jcs::to_vec(&value).unwrap();
        let mut message = RECEIPT_DOMAIN.to_vec();
        message.extend_from_slice(&canonical);
        value.as_object_mut().unwrap().insert(
            "signature".into(),
            serde_json::Value::String(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
        );
        serde_jcs::to_vec(&value).unwrap()
    }

    #[test]
    fn pinned_authorization_key_matches_ratified_fingerprint() {
        assert_eq!(
            hex(&Sha256::digest(AUTHORIZATION_PUBLIC_KEY)),
            "c55374f03f95d7ef7c6493bb73c8ff46ff8dc4abf6aba46c4f981b77eabddc12"
        );
    }

    #[test]
    fn genesis_is_kind_bound_and_stable() {
        assert_ne!(genesis("global"), genesis("target"));
        assert_eq!(genesis("global").len(), 64);
    }

    #[test]
    fn strict_scope_is_not_generalized() {
        assert!(ensure_scope(ADMIN_ID).is_ok());
        assert!(matches!(
            ensure_scope("other"),
            Err(OfflineBootstrapError::Scope)
        ));
    }

    #[test]
    fn exact_replay_keeps_original_signed_result() {
        assert_eq!(consumption_receipt_result(), "bootstrapped");
    }

    #[test]
    fn target_acceptance_without_global_reservation_is_detected_rollback() {
        let binding = test_consumption();
        let global = test_ledger("global", vec![]);
        let target = test_ledger(
            "target",
            vec![
                Transition::ChallengeIntent {
                    bindings: Box::new(test_challenge_intent()),
                },
                Transition::AuthorizationAccepted {
                    authorization: binding.clone(),
                },
            ],
        );
        assert!(matches!(
            validate_consumption_state(&global, &target, &binding),
            Err(OfflineBootstrapError::LedgerCorrupt)
        ));
    }

    #[test]
    fn reservation_before_target_intent_is_the_only_missing_intent_recovery_shape() {
        let binding = test_consumption();
        let global = test_ledger(
            "global",
            vec![Transition::Reservation {
                authorization: binding.clone(),
            }],
        );
        let target = test_ledger(
            "target",
            vec![Transition::ChallengeIntent {
                bindings: Box::new(test_challenge_intent()),
            }],
        );
        validate_consumption_state(&global, &target, &binding).unwrap();
    }

    #[test]
    fn global_completion_without_target_completion_is_corrupt() {
        let binding = test_consumption();
        let global = test_ledger(
            "global",
            vec![
                Transition::Reservation {
                    authorization: binding.clone(),
                },
                Transition::Completed {
                    authorization_digest: binding.authorization_sha256.clone(),
                    target_completion_record_sha256: "ee".repeat(32),
                    bootstrap_receipt_sha256: "ff".repeat(32),
                    consumption_receipt_sha256: "aa".repeat(32),
                    consumption_receipt: "{}".into(),
                },
            ],
        );
        let target = test_ledger(
            "target",
            vec![
                Transition::ChallengeIntent {
                    bindings: Box::new(test_challenge_intent()),
                },
                Transition::AuthorizationAccepted {
                    authorization: binding.clone(),
                },
            ],
        );
        assert!(matches!(
            validate_consumption_state(&global, &target, &binding),
            Err(OfflineBootstrapError::LedgerCorrupt)
        ));
    }

    #[test]
    fn private_test_authorizer_exercises_valid_signature_and_mutation() {
        let key = SigningKey::from_bytes(&[42u8; 32]);
        let mut authorization = test_authorization();
        let mut unsigned = serde_json::to_value(&authorization).unwrap();
        unsigned.as_object_mut().unwrap().remove("signature");
        let canonical = serde_jcs::to_vec(&unsigned).unwrap();
        let mut message = SIGNATURE_DOMAIN.to_vec();
        message.extend_from_slice(&canonical);
        authorization.signature = URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes());
        let raw = serde_jcs::to_vec(&authorization).unwrap();
        verify_authorization_signature(&authorization, &raw, &key.verifying_key().to_bytes())
            .unwrap();

        let mut changed = raw.clone();
        let index = changed.iter().position(|byte| *byte == b'4').unwrap();
        changed[index] = b'5';
        assert!(matches!(
            verify_authorization_signature(
                &authorization,
                &changed,
                &key.verifying_key().to_bytes()
            ),
            Err(OfflineBootstrapError::Signature)
        ));
    }

    #[test]
    fn consumption_receipt_is_strict_offline_verifiable() {
        let key = SigningKey::from_bytes(&[91u8; 32]);
        let raw = test_consumption_receipt(&key);
        verify_offline_consumption_receipt(&raw, &key.verifying_key().to_bytes()).unwrap();

        let mut tampered: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        tampered["result"] = serde_json::Value::String("already_bootstrapped".into());
        assert!(
            verify_offline_consumption_receipt(
                &serde_jcs::to_vec(&tampered).unwrap(),
                &key.verifying_key().to_bytes()
            )
            .is_err()
        );

        let mut unknown: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        unknown["extra"] = serde_json::Value::Bool(true);
        assert!(
            verify_offline_consumption_receipt(
                &serde_jcs::to_vec(&unknown).unwrap(),
                &key.verifying_key().to_bytes()
            )
            .is_err()
        );

        let pretty =
            serde_json::to_vec_pretty(&serde_json::from_slice::<serde_json::Value>(&raw).unwrap())
                .unwrap();
        assert!(
            verify_offline_consumption_receipt(&pretty, &key.verifying_key().to_bytes()).is_err()
        );
    }

    #[test]
    fn sensitive_hash_comparison_is_fixed_length_and_lowercase() {
        let value = "ab".repeat(32);
        assert!(ct_eq_hex(&value, &value, 32));
        assert!(!ct_eq_hex(&value, &"ac".repeat(32), 32));
        assert!(!ct_eq_hex(&value, &value.to_uppercase(), 32));
        assert!(!ct_eq_hex(&value, &value[..62], 32));
    }

    #[test]
    #[cfg(windows)]
    fn isolated_challenge_is_canonical_and_exactly_regenerated() {
        let fixture = IsolatedFixture::new();
        let first = fixture.prepare(&fixture.challenge).unwrap();
        let replay_path = fixture.root.path().join("challenge-replay.json");
        let replay = fixture.prepare(&replay_path).unwrap();
        assert_eq!(first, replay);
        let value = crate::raw::parse_transport_frame(&first).unwrap();
        assert_eq!(serde_jcs::to_vec(&value).unwrap(), first);
        assert!(!value.as_object().unwrap().contains_key("challenge_sha256"));
        assert!(!fixture.global.exists() || std::fs::read(&fixture.global).unwrap().is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn isolated_copied_home_changed_key_and_ledger_corruption_fail_closed() {
        let source = IsolatedFixture::new();
        source.prepare(&source.challenge).unwrap();
        let changed_ref: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&source.paths.admin_root).unwrap()).unwrap();
        let changed_seed = PathBuf::from(changed_ref["reference"].as_str().unwrap());
        std::fs::write(&changed_seed, [99u8; 32]).unwrap();
        secure_test_file(&changed_seed);
        assert!(
            source
                .prepare(&source.root.path().join("changed-key.json"))
                .is_err()
        );

        let copied = IsolatedFixture::new();
        let copied_target = target_ledger_path(&copied.home);
        std::fs::create_dir_all(copied_target.parent().unwrap()).unwrap();
        std::fs::copy(target_ledger_path(&source.home), &copied_target).unwrap();
        secure_test_file(&copied_target);
        assert!(
            copied
                .prepare(&copied.root.path().join("copied-home.json"))
                .is_err()
        );

        let corrupt = IsolatedFixture::new();
        corrupt.prepare(&corrupt.challenge).unwrap();
        let ledger = target_ledger_path(&corrupt.home);
        let original = std::fs::read(&ledger).unwrap();
        let mut torn = original.clone();
        torn.extend_from_slice(b"{\"transition\":");
        std::fs::write(&ledger, torn).unwrap();
        let replay = corrupt
            .prepare(&corrupt.root.path().join("after-torn.json"))
            .unwrap();
        assert_eq!(replay, std::fs::read(&corrupt.challenge).unwrap());
        assert_eq!(std::fs::read(&ledger).unwrap(), original);
        let mut newline_corrupt = original;
        newline_corrupt.extend_from_slice(b"{}\n");
        std::fs::write(&ledger, &newline_corrupt).unwrap();
        assert!(
            corrupt
                .prepare(&corrupt.root.path().join("newline-corrupt.json"))
                .is_err()
        );
        assert_eq!(std::fs::read(&ledger).unwrap(), newline_corrupt);
    }

    #[test]
    fn signed_but_wrong_binding_is_rejected_after_signature() {
        let key = SigningKey::from_bytes(&[42u8; 32]);
        let mut authorization = test_authorization();
        let signer = CrashSigner::new();
        let snapshot = AuthoritySnapshot::empty(ADMIN_ID, [1u8; 32])
            .with_recovery_key([2u8; 32])
            .with_service_keys(signer.public_key(), [4u8; 32]);
        let context = BootstrapContext {
            snapshot,
            receipt_signer: Some(Box::new(signer)),
            artifact: ArtifactIdentity {
                source_commit_sha: authorization.nano_source_commit_sha.clone(),
                cargo_lock_sha256: authorization.cargo_lock_sha256.clone(),
                executable_sha256: authorization.executable_sha256.clone(),
            },
            machine: authorization.machine_binding_sha256.clone(),
            owner: authorization.owner_sid_binding_sha256.clone(),
            home: authorization.nano_home_binding_sha256.clone(),
            fingerprints: [
                [1u8; 32],
                [2u8; 32],
                CrashSigner::new().public_key(),
                [4u8; 32],
            ],
        };
        authorization.machine_binding_sha256 = "fe".repeat(32);
        let mut unsigned = serde_json::to_value(&authorization).unwrap();
        unsigned.as_object_mut().unwrap().remove("signature");
        let canonical = serde_jcs::to_vec(&unsigned).unwrap();
        let mut message = SIGNATURE_DOMAIN.to_vec();
        message.extend_from_slice(&canonical);
        authorization.signature = URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes());
        let raw = serde_jcs::to_vec(&authorization).unwrap();
        verify_authorization_signature(&authorization, &raw, &key.verifying_key().to_bytes())
            .unwrap();
        assert!(matches!(
            verify_authorization_bindings(&authorization, &context),
            Err(OfflineBootstrapError::Machine)
        ));
    }

    #[test]
    fn every_durable_transition_prefix_has_one_deterministic_recovery_shape() {
        let binding = test_consumption();
        let challenge = Transition::ChallengeIntent {
            bindings: Box::new(test_challenge_intent()),
        };
        let reservation = Transition::Reservation {
            authorization: binding.clone(),
        };
        let accepted = Transition::AuthorizationAccepted {
            authorization: binding.clone(),
        };
        let target_completed = Transition::BootstrapCompleted {
            authorization_digest: binding.authorization_sha256.clone(),
            bootstrap_record_sha256: "77".repeat(32),
            bootstrap_receipt_sha256: "88".repeat(32),
            authority_bootstrap_position: 1,
            authority_receipt_position: 2,
        };
        let target_with_completion = test_ledger(
            "target",
            vec![challenge.clone(), accepted.clone(), target_completed],
        );
        let target_with_acceptance =
            test_ledger("target", vec![challenge.clone(), accepted.clone()]);
        let target_challenge_only = test_ledger("target", vec![challenge]);
        let global_empty = test_ledger("global", vec![]);
        let global_reserved = test_ledger("global", vec![reservation.clone()]);
        let target_completion_digest = target_with_completion.records[2].record_sha256.clone();
        let global_complete = test_ledger(
            "global",
            vec![
                reservation,
                Transition::Completed {
                    authorization_digest: binding.authorization_sha256.clone(),
                    target_completion_record_sha256: target_completion_digest,
                    bootstrap_receipt_sha256: "88".repeat(32),
                    consumption_receipt_sha256: "aa".repeat(32),
                    consumption_receipt: "{}".into(),
                },
            ],
        );

        for (global, target) in [
            (&global_empty, &target_challenge_only),
            (&global_reserved, &target_challenge_only),
            (&global_reserved, &target_with_acceptance),
            (&global_reserved, &target_with_completion),
            (&global_complete, &target_with_completion),
        ] {
            validate_consumption_state(global, target, &binding).unwrap();
        }
    }

    #[test]
    #[cfg(windows)]
    fn crash_boundary_child_driver() {
        let Some(root) = std::env::var_os("NANO_OFFLINE_CRASH_ROOT") else {
            return;
        };
        let stage = std::env::var("NANO_OFFLINE_CRASH_STAGE").unwrap();
        run_crash_engine(Path::new(&root), &stage).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn terminate_process_at_each_durable_boundary_recovers_exactly_once() {
        for stage in [
            "reservation",
            "target_accepted",
            "authority_bootstrap",
            "bootstrap_receipt",
            "target_completion",
            "global_completion",
            "projection",
        ] {
            let root = tempfile::Builder::new()
                .prefix("nano-offline-crash-")
                .tempdir_in(std::env::var_os("LOCALAPPDATA").unwrap())
                .unwrap();
            secure_test_directory(root.path());
            let home = root.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            secure_test_directory(&home);
            let run = |mode: &str| {
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "offline_bootstrap::tests::crash_boundary_child_driver",
                        "--nocapture",
                    ])
                    .env("NANO_OFFLINE_CRASH_ROOT", root.path())
                    .env("NANO_OFFLINE_CRASH_STAGE", mode)
                    .status()
                    .unwrap()
            };
            assert!(!run(stage).success(), "{stage} must terminate abruptly");
            assert!(run("recover-1").success(), "{stage} first recovery");
            assert!(run("recover-2").success(), "{stage} exact replay");
            let first = std::fs::read(root.path().join("result-1.json")).unwrap();
            let second = std::fs::read(root.path().join("result-2.json")).unwrap();
            assert_eq!(first, second, "{stage} receipt identity");
            let authority = std::fs::read(home.join("activation/authority.jsonl")).unwrap();
            assert_eq!(
                authority
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .count(),
                2,
                "{stage} installs one authority"
            );
        }
    }

    #[cfg(windows)]
    fn run_crash_engine(root: &Path, stage: &str) -> Result<(), OfflineBootstrapError> {
        let home = root.join("home");
        let global_path = root.join("global.jsonl");
        let target_path = target_ledger_path(&home);
        let _global_lock = lock_for(&global_path)?;
        let authority_lock = AuthorityStore::acquire_authority_lock(&home)?;
        let mut global = Ledger::open(&global_path, "global")?;
        let mut target = Ledger::open(&target_path, "target")?;
        let binding = test_consumption();
        if target.records.is_empty() {
            target.append(Transition::ChallengeIntent {
                bindings: Box::new(test_challenge_intent()),
            })?;
        }
        if !has_transition(&global, |item| {
            matches!(item, Transition::Reservation { .. })
        }) {
            global.append(Transition::Reservation {
                authorization: binding.clone(),
            })?;
            crash_if(stage, "reservation");
        }
        if !has_transition(&target, |item| {
            matches!(item, Transition::AuthorizationAccepted { .. })
        }) {
            target.append(Transition::AuthorizationAccepted {
                authorization: binding.clone(),
            })?;
            crash_if(stage, "target_accepted");
        }

        let signer = CrashSigner::new();
        let snapshot = AuthoritySnapshot::empty(ADMIN_ID, [1u8; 32])
            .with_recovery_key([2u8; 32])
            .with_service_keys(signer.public_key(), [4u8; 32]);
        let bootstrap_receipt = sign_bootstrap_receipt(&snapshot, &signer)?;
        let authority_path = home.join("activation/authority.jsonl");
        let (_, _, existing_receipt, sequence) = crate::journal::replay(&authority_path)?;
        if sequence == 0 && stage == "authority_bootstrap" {
            append_test_authority_record(
                &authority_path,
                &crate::journal::AuthorityRecord::Bootstrap {
                    sequence: 1,
                    snapshot: snapshot.clone(),
                },
            )?;
            crash_now();
        }
        if existing_receipt.is_none() && stage == "bootstrap_receipt" {
            if sequence == 0 {
                append_test_authority_record(
                    &authority_path,
                    &crate::journal::AuthorityRecord::Bootstrap {
                        sequence: 1,
                        snapshot: snapshot.clone(),
                    },
                )?;
            }
            append_test_authority_record(
                &authority_path,
                &crate::journal::AuthorityRecord::BootstrapReceipt {
                    sequence: 2,
                    receipt: String::from_utf8(bootstrap_receipt.clone()).unwrap(),
                },
            )?;
            crash_now();
        }
        let store = AuthorityStore::bootstrap_initial_with_held_lock(
            &home,
            snapshot.clone(),
            bootstrap_receipt.clone(),
            authority_lock,
        )?;
        let bootstrap_digest = hex(&Sha256::digest(&bootstrap_receipt));
        let target_completion = if let Some(record) = find_transition_record(&target, |item| {
            matches!(item, Transition::BootstrapCompleted { .. })
        }) {
            record.clone()
        } else {
            let record = target.append(Transition::BootstrapCompleted {
                authorization_digest: binding.authorization_sha256.clone(),
                bootstrap_record_sha256: authority_bootstrap_record_digest(&home)?,
                bootstrap_receipt_sha256: bootstrap_digest.clone(),
                authority_bootstrap_position: 1,
                authority_receipt_position: 2,
            })?;
            crash_if(stage, "target_completion");
            record
        };
        let context = BootstrapContext {
            snapshot,
            receipt_signer: Some(Box::new(CrashSigner::new())),
            artifact: ArtifactIdentity {
                source_commit_sha: "b".repeat(40),
                cargo_lock_sha256: "cc".repeat(32),
                executable_sha256: "dd".repeat(32),
            },
            machine: "11".repeat(32),
            owner: "22".repeat(32),
            home: "33".repeat(32),
            fingerprints: [
                [1u8; 32],
                [2u8; 32],
                CrashSigner::new().public_key(),
                [4u8; 32],
            ],
        };
        let authorization = test_authorization();
        let receipt = if let Some(record) =
            find_transition_record(&global, |item| matches!(item, Transition::Completed { .. }))
        {
            let Transition::Completed {
                consumption_receipt,
                ..
            } = &record.transition
            else {
                unreachable!()
            };
            consumption_receipt.as_bytes().to_vec()
        } else {
            let position = global.records.len() as u64 + 1;
            let receipt = sign_consumption_receipt(
                &authorization,
                &binding.authorization_sha256,
                &bootstrap_digest,
                &context,
                &global,
                &target,
                position,
            )?;
            global.append(Transition::Completed {
                authorization_digest: binding.authorization_sha256.clone(),
                target_completion_record_sha256: target_completion.record_sha256,
                bootstrap_receipt_sha256: bootstrap_digest,
                consumption_receipt_sha256: hex(&Sha256::digest(&receipt)),
                consumption_receipt: String::from_utf8(receipt.clone()).unwrap(),
            })?;
            crash_if(stage, "global_completion");
            receipt
        };
        if stage == "projection" {
            drop(store);
            for name in ["authority.db", "authority.db-wal", "authority.db-shm"] {
                let _ = std::fs::remove_file(home.join("activation").join(name));
            }
            crash_now();
        }
        let result = if stage == "recover-1" {
            "result-1.json"
        } else {
            "result-2.json"
        };
        std::fs::write(root.join(result), receipt)?;
        Ok(())
    }

    #[cfg(windows)]
    fn append_test_authority_record(
        path: &Path,
        record: &crate::journal::AuthorityRecord,
    ) -> Result<(), OfflineBootstrapError> {
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut bytes = serde_jcs::to_vec(record).map_err(|_| OfflineBootstrapError::Malformed)?;
        bytes.push(b'\n');
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    }

    #[cfg(windows)]
    fn crash_if(actual: &str, expected: &str) {
        if actual == expected {
            crash_now();
        }
    }

    #[cfg(windows)]
    fn crash_now() -> ! {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        unsafe { TerminateProcess(GetCurrentProcess(), 91) };
        std::process::abort()
    }

    #[cfg(windows)]
    fn secure_test_directory(path: &Path) {
        secure_test_path(path, true);
    }

    #[cfg(windows)]
    fn secure_test_file(path: &Path) {
        secure_test_path(path, false);
    }

    #[cfg(windows)]
    fn secure_test_path(path: &Path, directory: bool) {
        let script = r#"
$item = if ($env:NANO_TEST_SECURE_DIRECTORY -eq '1') { [System.IO.DirectoryInfo]::new($env:NANO_TEST_SECURE_PATH) } else { [System.IO.FileInfo]::new($env:NANO_TEST_SECURE_PATH) }
$acl = $item.GetAccessControl()
$acl.SetAccessRuleProtection($true, $false)
foreach ($rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleSpecific($rule) }
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl.SetOwner($sid)
$inheritance = if ($env:NANO_TEST_SECURE_DIRECTORY -eq '1') { 'ContainerInherit, ObjectInherit' } else { 'None' }
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new($sid,'FullControl',$inheritance,'None','Allow')
[void]$acl.AddAccessRule($rule)
$item.SetAccessControl($acl)
"#;
        assert!(
            Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", script])
                .env("NANO_TEST_SECURE_PATH", path)
                .env(
                    "NANO_TEST_SECURE_DIRECTORY",
                    if directory { "1" } else { "0" }
                )
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn strict_nested_ledger_record_round_trips() {
        let transition = Transition::Completed {
            authorization_digest: "11".repeat(32),
            target_completion_record_sha256: "22".repeat(32),
            bootstrap_receipt_sha256: "33".repeat(32),
            consumption_receipt_sha256: "44".repeat(32),
            consumption_receipt: "{}".into(),
        };
        let previous = genesis("global");
        let record = LedgerRecord {
            sequence: 1,
            previous_record_sha256: previous.clone(),
            record_sha256: record_digest(1, &previous, &transition).unwrap(),
            transition,
        };
        let bytes = serde_jcs::to_vec(&record).unwrap();
        let value = crate::raw::parse_transport_frame(&bytes).unwrap();
        let replayed: LedgerRecord = serde_json::from_value(value).unwrap();
        assert_eq!(replayed.sequence, record.sequence);
        assert_eq!(replayed.previous_record_sha256, previous);
        assert_eq!(
            record_digest(
                replayed.sequence,
                &replayed.previous_record_sha256,
                &replayed.transition,
            )
            .unwrap(),
            replayed.record_sha256,
        );
    }
}
