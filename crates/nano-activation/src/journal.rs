//! Append-only authority journal. Unknown records are retained but never reduced.

use crate::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot, apply};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorityRecord {
    Bootstrap {
        sequence: u64,
        snapshot: AuthoritySnapshot,
    },
    BootstrapReceipt {
        sequence: u64,
        receipt: String,
    },
    Command {
        sequence: u64,
        command: AuthorityCommand,
    },
    Transaction {
        sequence: u64,
        command: AuthorityCommand,
        nonce_command: AuthorityCommand,
    },
}

pub(crate) struct AuthorityJournal {
    file: File,
    next_sequence: u64,
}

type ReplayState = (
    Option<AuthoritySnapshot>,
    Option<AuthoritySnapshot>,
    Option<Vec<u8>>,
    u64,
);

impl AuthorityJournal {
    pub(crate) fn open(path: &Path) -> Result<Self, AuthorityError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (_, _, _, sequence) = replay(path)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        Ok(Self {
            file,
            next_sequence: sequence + 1,
        })
    }

    pub(crate) fn append_bootstrap(
        &mut self,
        snapshot: AuthoritySnapshot,
    ) -> Result<(), AuthorityError> {
        self.append(AuthorityRecord::Bootstrap {
            sequence: self.next_sequence,
            snapshot,
        })
    }

    pub(crate) fn append_bootstrap_receipt(
        &mut self,
        receipt: String,
    ) -> Result<(), AuthorityError> {
        self.append(AuthorityRecord::BootstrapReceipt {
            sequence: self.next_sequence,
            receipt,
        })
    }

    pub(crate) fn append_command(
        &mut self,
        command: AuthorityCommand,
    ) -> Result<(), AuthorityError> {
        self.append(AuthorityRecord::Command {
            sequence: self.next_sequence,
            command,
        })
    }

    pub(crate) fn append_transaction(
        &mut self,
        command: AuthorityCommand,
        nonce_command: AuthorityCommand,
    ) -> Result<(), AuthorityError> {
        self.append(AuthorityRecord::Transaction {
            sequence: self.next_sequence,
            command,
            nonce_command,
        })
    }

    fn append(&mut self, record: AuthorityRecord) -> Result<(), AuthorityError> {
        let mut bytes = serde_jcs::to_vec(&record).map_err(|_| AuthorityError::InvalidRecord)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.next_sequence += 1;
        Ok(())
    }
}

pub(crate) fn replay(path: &Path) -> Result<ReplayState, AuthorityError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((None, None, None, 0));
        }
        Err(error) => return Err(error.into()),
    };
    let mut snapshot: Option<AuthoritySnapshot> = None;
    let mut bootstrap_snapshot = None;
    let mut bootstrap_receipt = None;
    let mut expected = 1u64;
    for (index, raw) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if raw.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_slice(raw) {
            Ok(value) => value,
            Err(_)
                if index == bytes.split(|byte| *byte == b'\n').count() - 1
                    && !bytes.ends_with(b"\n") =>
            {
                break;
            }
            Err(_) => return Err(AuthorityError::InvalidRecord),
        };
        let record: AuthorityRecord = match serde_json::from_value(value.clone()) {
            Ok(record) => record,
            Err(_) => {
                let sequence = value
                    .get("sequence")
                    .and_then(Value::as_u64)
                    .ok_or(AuthorityError::InvalidRecord)?;
                if sequence != expected {
                    return Err(AuthorityError::InvalidRecord);
                }
                let state = snapshot.as_mut().ok_or(AuthorityError::InvalidRecord)?;
                state
                    .unknown_records
                    .push(String::from_utf8_lossy(raw).into_owned());
                expected += 1;
                continue;
            }
        };
        let sequence = match &record {
            AuthorityRecord::Bootstrap { sequence, .. }
            | AuthorityRecord::BootstrapReceipt { sequence, .. }
            | AuthorityRecord::Command { sequence, .. }
            | AuthorityRecord::Transaction { sequence, .. } => *sequence,
        };
        if sequence != expected {
            return Err(AuthorityError::InvalidRecord);
        }
        match record {
            AuthorityRecord::Bootstrap {
                snapshot: initial, ..
            } if snapshot.is_none() => {
                bootstrap_snapshot = Some(initial.clone());
                snapshot = Some(initial);
            }
            AuthorityRecord::Bootstrap { .. } => return Err(AuthorityError::InvalidRecord),
            AuthorityRecord::BootstrapReceipt { receipt, .. } => {
                if snapshot.is_none() || bootstrap_receipt.is_some() {
                    return Err(AuthorityError::InvalidRecord);
                }
                bootstrap_receipt = Some(receipt.into_bytes());
            }
            AuthorityRecord::Command { command, .. } => {
                apply(
                    snapshot.as_mut().ok_or(AuthorityError::InvalidRecord)?,
                    &command,
                )?;
            }
            AuthorityRecord::Transaction {
                command,
                nonce_command,
                ..
            } => {
                let state = snapshot.as_mut().ok_or(AuthorityError::InvalidRecord)?;
                apply(state, &command)?;
                apply(state, &nonce_command)?;
            }
        }
        expected += 1;
    }
    Ok((
        bootstrap_snapshot,
        snapshot,
        bootstrap_receipt,
        expected.saturating_sub(1),
    ))
}

/// Remove only an incomplete final record while the authority writer lock is held.
pub(crate) fn repair_torn_tail(path: &Path) -> Result<(), AuthorityError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return Ok(());
    }
    let keep = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(keep as u64)?;
    file.sync_data()?;
    Ok(())
}
