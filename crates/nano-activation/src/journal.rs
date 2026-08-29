//! Append-only authority journal. Unknown records are retained but never reduced.

use crate::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot, apply};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorityRecord {
    Bootstrap { sequence: u64, snapshot: AuthoritySnapshot },
    Command { sequence: u64, command: AuthorityCommand },
}

pub struct AuthorityJournal { file: File, path: PathBuf, next_sequence: u64 }

impl AuthorityJournal {
    pub fn open(path: &Path) -> Result<Self, AuthorityError> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let (_, sequence) = replay(path)?;
        let file = OpenOptions::new().create(true).append(true).read(true).open(path)?;
        Ok(Self { file, path: path.to_owned(), next_sequence: sequence + 1 })
    }

    pub fn path(&self) -> &Path { &self.path }

    pub fn append_bootstrap(&mut self, snapshot: AuthoritySnapshot) -> Result<(), AuthorityError> {
        self.append(AuthorityRecord::Bootstrap { sequence: self.next_sequence, snapshot })
    }

    pub fn append_command(&mut self, command: AuthorityCommand) -> Result<(), AuthorityError> {
        self.append(AuthorityRecord::Command { sequence: self.next_sequence, command })
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

pub fn replay(path: &Path) -> Result<(Option<AuthoritySnapshot>, u64), AuthorityError> {
    let bytes = match std::fs::read(path) { Ok(bytes) => bytes, Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((None, 0)), Err(error) => return Err(error.into()) };
    let mut snapshot: Option<AuthoritySnapshot> = None;
    let mut expected = 1u64;
    for (index, raw) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if raw.is_empty() { continue; }
        let value: Value = match serde_json::from_slice(raw) {
            Ok(value) => value,
            Err(_) if index == bytes.split(|byte| *byte == b'\n').count() - 1 && !bytes.ends_with(b"\n") => break,
            Err(_) => return Err(AuthorityError::InvalidRecord),
        };
        let record: AuthorityRecord = match serde_json::from_value(value.clone()) {
            Ok(record) => record,
            Err(_) => {
                if let Some(state) = snapshot.as_mut() { state.unknown_records.push(String::from_utf8_lossy(raw).into_owned()); }
                expected += 1;
                continue;
            }
        };
        let sequence = match &record { AuthorityRecord::Bootstrap { sequence, .. } | AuthorityRecord::Command { sequence, .. } => *sequence };
        if sequence != expected { return Err(AuthorityError::InvalidRecord); }
        match record {
            AuthorityRecord::Bootstrap { snapshot: initial, .. } if snapshot.is_none() => snapshot = Some(initial),
            AuthorityRecord::Bootstrap { .. } => return Err(AuthorityError::InvalidRecord),
            AuthorityRecord::Command { command, .. } => { apply(snapshot.as_mut().ok_or(AuthorityError::InvalidRecord)?, &command)?; }
        }
        expected += 1;
    }
    Ok((snapshot, expected.saturating_sub(1)))
}
