//! Closed, product-agnostic authority vocabulary and deterministic reducer.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyRole {
    AdminRoot,
    ReceiptSigner,
    LocalCliIssuer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityKey {
    pub public_key: [u8; 32],
    pub epoch: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuerAuthority {
    pub subject_id: String,
    pub principal_id: String,
    pub epoch: u64,
    pub revoked: bool,
    pub keys: BTreeMap<String, AuthorityKey>,
    pub projects: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonceTombstone {
    pub tuple_digest: String,
    pub expires_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoritySnapshot {
    pub admin_id: String,
    pub admin_epoch: u64,
    pub admin_public_key: [u8; 32],
    pub recovery_public_key: Option<[u8; 32]>,
    pub issuers: BTreeMap<String, IssuerAuthority>,
    pub retired_subjects: BTreeSet<String>,
    pub retired_principals: BTreeSet<String>,
    pub operations: BTreeMap<String, String>,
    pub nonces: BTreeMap<String, NonceTombstone>,
    pub unknown_records: Vec<String>,
}

impl AuthoritySnapshot {
    pub fn empty(admin_id: impl Into<String>, admin_public_key: [u8; 32]) -> Self {
        Self {
            admin_id: admin_id.into(),
            admin_epoch: 1,
            admin_public_key,
            recovery_public_key: None,
            issuers: BTreeMap::new(),
            retired_subjects: BTreeSet::new(),
            retired_principals: BTreeSet::new(),
            operations: BTreeMap::new(),
            nonces: BTreeMap::new(),
            unknown_records: Vec::new(),
        }
    }

    pub fn with_recovery_key(mut self, recovery_public_key: [u8; 32]) -> Self {
        self.recovery_public_key = Some(recovery_public_key);
        self
    }

    pub fn digest(&self) -> Result<String, AuthorityError> {
        let bytes = serde_jcs::to_vec(self).map_err(|_| AuthorityError::InvalidRecord)?;
        Ok(hex(&Sha256::digest(bytes)))
    }

    pub fn preview_admin_transaction(
        &self,
        command: &AuthorityCommand,
        nonce: &str,
        expires_at_unix: i64,
    ) -> Result<Self, AuthorityError> {
        let mut next = self.clone();
        apply(&mut next, command)?;
        apply(&mut next, &nonce_command(nonce, command, expires_at_unix)?)?;
        Ok(next)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorityCommand {
    EnrollIssuer {
        operation_id: String,
        issuer_id: String,
        subject_id: String,
        principal_id: String,
        key_id: String,
        public_key: [u8; 32],
    },
    GrantProject {
        operation_id: String,
        issuer_id: String,
        subject_id: String,
        principal_id: String,
        project_id: String,
    },
    RotateKey {
        operation_id: String,
        issuer_id: String,
        old_key_id: String,
        new_key_id: String,
        public_key: [u8; 32],
    },
    RevokeKey {
        operation_id: String,
        issuer_id: String,
        key_id: String,
    },
    RevokeIssuer {
        operation_id: String,
        issuer_id: String,
    },
    RetireSubject {
        operation_id: String,
        subject_id: String,
    },
    RecoverRoot {
        operation_id: String,
        expected_epoch: u64,
        new_admin_id: String,
        new_public_key: [u8; 32],
    },
    ConsumeNonce {
        operation_id: String,
        nonce: String,
        tuple_digest: String,
        expires_at_unix: i64,
    },
}

impl AuthorityCommand {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::EnrollIssuer { operation_id, .. }
            | Self::GrantProject { operation_id, .. }
            | Self::RotateKey { operation_id, .. }
            | Self::RevokeKey { operation_id, .. }
            | Self::RevokeIssuer { operation_id, .. }
            | Self::RetireSubject { operation_id, .. }
            | Self::RecoverRoot { operation_id, .. }
            | Self::ConsumeNonce { operation_id, .. } => operation_id,
        }
    }

    pub fn digest(&self) -> Result<String, AuthorityError> {
        let bytes = serde_jcs::to_vec(self).map_err(|_| AuthorityError::InvalidRecord)?;
        Ok(hex(&Sha256::digest(bytes)))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("authority writer is already active")]
    Contention,
    #[error("authority journal or projection I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("authority projection failed: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("authority record is malformed")]
    InvalidRecord,
    #[error("operation id was reused with different immutable content")]
    OperationConflict,
    #[error("subject or principal binding is immutable")]
    ImmutableBinding,
    #[error("retired identifier cannot be reused")]
    RetiredIdentifier,
    #[error("issuer, key, subject, principal, or project is not authorized")]
    Unauthorized,
    #[error("nonce was reused with different immutable content")]
    NonceReplay,
    #[error("unknown authority record never authorizes")]
    UnknownRecord,
    #[error("admin epoch is stale")]
    StaleEpoch,
    #[error("authority state digest did not match")]
    DigestMismatch,
}

pub(crate) fn apply(
    snapshot: &mut AuthoritySnapshot,
    command: &AuthorityCommand,
) -> Result<bool, AuthorityError> {
    validate_id(command.operation_id())?;
    let digest = command.digest()?;
    if let Some(existing) = snapshot.operations.get(command.operation_id()) {
        return if existing == &digest {
            Ok(false)
        } else {
            Err(AuthorityError::OperationConflict)
        };
    }

    match command {
        AuthorityCommand::EnrollIssuer {
            issuer_id,
            subject_id,
            principal_id,
            key_id,
            public_key,
            ..
        } => {
            validate_id(issuer_id)?;
            validate_id(subject_id)?;
            validate_id(principal_id)?;
            validate_id(key_id)?;
            if snapshot.retired_subjects.contains(subject_id)
                || snapshot.retired_principals.contains(principal_id)
            {
                return Err(AuthorityError::RetiredIdentifier);
            }
            if snapshot.issuers.values().any(|issuer| {
                issuer.subject_id == *subject_id && issuer.principal_id != *principal_id
            }) {
                return Err(AuthorityError::ImmutableBinding);
            }
            if let Some(existing) = snapshot.issuers.get(issuer_id) {
                if existing.subject_id != *subject_id || existing.principal_id != *principal_id {
                    return Err(AuthorityError::ImmutableBinding);
                }
                return Err(AuthorityError::OperationConflict);
            }
            let mut keys = BTreeMap::new();
            keys.insert(
                key_id.clone(),
                AuthorityKey {
                    public_key: *public_key,
                    epoch: 1,
                    revoked: false,
                },
            );
            snapshot.issuers.insert(
                issuer_id.clone(),
                IssuerAuthority {
                    subject_id: subject_id.clone(),
                    principal_id: principal_id.clone(),
                    epoch: 1,
                    revoked: false,
                    keys,
                    projects: BTreeSet::new(),
                },
            );
        }
        AuthorityCommand::GrantProject {
            issuer_id,
            subject_id,
            principal_id,
            project_id,
            ..
        } => {
            validate_id(project_id)?;
            let issuer = snapshot
                .issuers
                .get_mut(issuer_id)
                .ok_or(AuthorityError::Unauthorized)?;
            if issuer.revoked
                || issuer.subject_id != *subject_id
                || issuer.principal_id != *principal_id
            {
                return Err(AuthorityError::ImmutableBinding);
            }
            issuer.projects.insert(project_id.clone());
        }
        AuthorityCommand::RotateKey {
            issuer_id,
            old_key_id,
            new_key_id,
            public_key,
            ..
        } => {
            let issuer = snapshot
                .issuers
                .get_mut(issuer_id)
                .ok_or(AuthorityError::Unauthorized)?;
            let old = issuer
                .keys
                .get(old_key_id)
                .ok_or(AuthorityError::Unauthorized)?;
            if old.revoked || issuer.keys.contains_key(new_key_id) {
                return Err(AuthorityError::OperationConflict);
            }
            issuer.epoch += 1;
            issuer.keys.insert(
                new_key_id.clone(),
                AuthorityKey {
                    public_key: *public_key,
                    epoch: issuer.epoch,
                    revoked: false,
                },
            );
        }
        AuthorityCommand::RevokeKey {
            issuer_id, key_id, ..
        } => {
            let issuer = snapshot
                .issuers
                .get_mut(issuer_id)
                .ok_or(AuthorityError::Unauthorized)?;
            let key = issuer
                .keys
                .get_mut(key_id)
                .ok_or(AuthorityError::Unauthorized)?;
            key.revoked = true;
            issuer.epoch += 1;
        }
        AuthorityCommand::RevokeIssuer { issuer_id, .. } => {
            let issuer = snapshot
                .issuers
                .get_mut(issuer_id)
                .ok_or(AuthorityError::Unauthorized)?;
            issuer.revoked = true;
            issuer.epoch += 1;
        }
        AuthorityCommand::RetireSubject { subject_id, .. } => {
            let issuer = snapshot
                .issuers
                .values_mut()
                .find(|issuer| issuer.subject_id == *subject_id)
                .ok_or(AuthorityError::Unauthorized)?;
            issuer.revoked = true;
            snapshot.retired_subjects.insert(subject_id.clone());
            snapshot
                .retired_principals
                .insert(issuer.principal_id.clone());
        }
        AuthorityCommand::RecoverRoot {
            expected_epoch,
            new_admin_id,
            new_public_key,
            ..
        } => {
            if *expected_epoch != snapshot.admin_epoch {
                return Err(AuthorityError::StaleEpoch);
            }
            snapshot.admin_id = new_admin_id.clone();
            snapshot.admin_public_key = *new_public_key;
            snapshot.admin_epoch += 1;
        }
        AuthorityCommand::ConsumeNonce {
            nonce,
            tuple_digest,
            expires_at_unix,
            ..
        } => {
            if let Some(existing) = snapshot.nonces.get(nonce) {
                if existing.tuple_digest != *tuple_digest {
                    return Err(AuthorityError::NonceReplay);
                }
            } else {
                snapshot.nonces.insert(
                    nonce.clone(),
                    NonceTombstone {
                        tuple_digest: tuple_digest.clone(),
                        expires_at_unix: *expires_at_unix,
                    },
                );
            }
        }
    }
    snapshot
        .operations
        .insert(command.operation_id().to_owned(), digest);
    Ok(true)
}

pub(crate) fn nonce_command(
    nonce: &str,
    command: &AuthorityCommand,
    expires_at_unix: i64,
) -> Result<AuthorityCommand, AuthorityError> {
    let tuple_digest = command.digest()?;
    Ok(AuthorityCommand::ConsumeNonce {
        operation_id: format!("admin-nonce-{nonce}-{tuple_digest}"),
        nonce: nonce.to_owned(),
        tuple_digest,
        expires_at_unix,
    })
}

fn validate_id(value: &str) -> Result<(), AuthorityError> {
    if (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        Ok(())
    } else {
        Err(AuthorityError::InvalidRecord)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}
