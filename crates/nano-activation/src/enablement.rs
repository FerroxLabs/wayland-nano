//! Authenticated, journaled and exact-artifact activation enablement.

use crate::{
    ActivationError, RejectReason, receipt::ArtifactIdentity, store::AuthorityStore,
    verify_admin_request,
};
use nano_session::FileLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnablementCommand {
    pub operation_id: String,
    pub enabled: bool,
    pub artifact: ArtifactIdentity,
    pub admin_epoch: u64,
    pub issuer_epoch: u64,
    pub grant_epoch: u64,
    pub revocation_epoch: u64,
    pub not_after: String,
}

impl EnablementCommand {
    pub fn digest(&self) -> String {
        hex(&Sha256::digest(
            serde_jcs::to_vec(self).expect("closed enablement"),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnablementFault {
    None,
    AfterPartialJournal,
    AfterJournal,
}

#[derive(Debug, thiserror::Error)]
pub enum EnablementError {
    #[error("activation enablement is missing")]
    Missing,
    #[error("activation enablement expired")]
    Expired,
    #[error("activation enablement was disabled or revoked")]
    Revoked,
    #[error("activation artifact does not match enablement")]
    ArtifactMismatch,
    #[error("activation authority epochs drifted")]
    EpochDrift,
    #[error("activation enablement journal recovery is ambiguous")]
    AmbiguousRecovery,
    #[error("signed admin enablement is invalid")]
    InvalidAdmin,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct EnablementStore {
    root: PathBuf,
    _lock: FileLock,
}

impl EnablementStore {
    pub fn open(home: &Path) -> Result<Self, EnablementError> {
        let root = home.join("activation");
        std::fs::create_dir_all(&root)?;
        let lock = FileLock::try_acquire(&root.join("enablement.lock"))
            .map_err(|_| EnablementError::AmbiguousRecovery)?;
        Ok(Self { root, _lock: lock })
    }

    pub fn state_digest(&self) -> Result<String, EnablementError> {
        Ok(self
            .replay()?
            .map_or_else(|| hex(&Sha256::digest(b"disabled")), |c| c.digest()))
    }

    pub fn apply_signed(
        &self,
        raw_admin: &[u8],
        command: &EnablementCommand,
        now: &str,
        fault: EnablementFault,
    ) -> Result<(), EnablementError> {
        let authority = AuthorityStore::open(self.root.parent().expect("home"))
            .map_err(|_| EnablementError::InvalidAdmin)?;
        let snapshot = authority
            .snapshot()
            .map_err(|_| EnablementError::InvalidAdmin)?;
        let verified = verify_admin_request(raw_admin, &snapshot.admin_public_key)
            .map_err(|_| EnablementError::InvalidAdmin)?;
        let operation = if command.enabled {
            "enable_artifact"
        } else {
            "disable_artifact"
        };
        if verified.operation() != operation
            || verified.admin_id() != snapshot.admin_id
            || verified.admin_epoch() != snapshot.admin_epoch
            || verified.operation_id() != command.operation_id
            || verified.before_digest() != self.state_digest()?
            || verified.after_digest() != command.digest()
            || verified.issued_at() > now
            || verified.not_after() < now
            || command.admin_epoch != snapshot.admin_epoch
        {
            return Err(EnablementError::InvalidAdmin);
        }
        drop(authority);
        let path = self.root.join("enablement.jsonl");
        let mut bytes = serde_jcs::to_vec(command).map_err(|_| EnablementError::InvalidAdmin)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        if fault == EnablementFault::AfterPartialJournal {
            file.write_all(&bytes[..bytes.len() / 2])?;
            file.sync_data()?;
            return Err(EnablementError::AmbiguousRecovery);
        }
        file.write_all(&bytes)?;
        file.sync_data()?;
        if fault == EnablementFault::AfterJournal {
            return Err(EnablementError::AmbiguousRecovery);
        }
        self.write_anchor(&command.digest())
    }

    pub fn require_enabled(
        &self,
        artifact: &ArtifactIdentity,
        epochs: [u64; 4],
        now: &str,
    ) -> Result<(), EnablementError> {
        let command = self.replay()?.ok_or(EnablementError::Missing)?;
        let anchor = std::fs::read_to_string(self.root.join("enablement.anchor"))
            .map_err(|_| EnablementError::AmbiguousRecovery)?;
        if anchor.trim() != command.digest() {
            return Err(EnablementError::AmbiguousRecovery);
        }
        if !command.enabled {
            return Err(EnablementError::Revoked);
        }
        if command.not_after.as_str() < now {
            return Err(EnablementError::Expired);
        }
        if &command.artifact != artifact {
            return Err(EnablementError::ArtifactMismatch);
        }
        if [
            command.admin_epoch,
            command.issuer_epoch,
            command.grant_epoch,
            command.revocation_epoch,
        ] != epochs
        {
            return Err(EnablementError::EpochDrift);
        }
        Ok(())
    }

    fn replay(&self) -> Result<Option<EnablementCommand>, EnablementError> {
        let path = self.root.join("enablement.jsonl");
        let mut data = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut data)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if !data.is_empty() && !data.ends_with(b"\n") {
            return Err(EnablementError::AmbiguousRecovery);
        }
        let mut last = None;
        for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            last =
                Some(serde_json::from_slice(line).map_err(|_| EnablementError::AmbiguousRecovery)?);
        }
        Ok(last)
    }
    fn write_anchor(&self, digest: &str) -> Result<(), EnablementError> {
        let tmp = self.root.join("enablement.anchor.tmp");
        let mut f = File::create(&tmp)?;
        f.write_all(digest.as_bytes())?;
        f.sync_data()?;
        std::fs::rename(tmp, self.root.join("enablement.anchor"))?;
        Ok(())
    }
}

impl From<EnablementError> for ActivationError {
    fn from(error: EnablementError) -> Self {
        let reason = match error {
            EnablementError::ArtifactMismatch => RejectReason::ArtifactMismatch,
            EnablementError::AmbiguousRecovery => RejectReason::AmbiguousRecovery,
            _ => RejectReason::ContinuityNotEnabled,
        };
        ActivationError::new(reason)
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
