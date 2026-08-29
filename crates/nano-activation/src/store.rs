//! Locked journal-first authority store with a disposable SQLite projection.

use crate::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot, apply};
use crate::journal::{AuthorityJournal, replay};
use nano_session::FileLock;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub struct AuthorityStore {
    home: PathBuf,
    journal: AuthorityJournal,
    db: Connection,
    snapshot: AuthoritySnapshot,
    _lock: FileLock,
}

impl std::fmt::Debug for AuthorityStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("AuthorityStore").field("home", &self.home).finish_non_exhaustive() }
}

impl AuthorityStore {
    pub fn open(nano_home: &Path) -> Result<Self, AuthorityError> {
        let activation = nano_home.join("activation");
        std::fs::create_dir_all(&activation)?;
        let journal_path = activation.join("authority.jsonl");
        let lock = FileLock::try_acquire(&activation.join("authority.lock")).map_err(|error| match error { nano_session::LockError::Busy => AuthorityError::Contention, nano_session::LockError::Io(error) => AuthorityError::Io(error) })?;
        let (snapshot, _) = replay(&journal_path)?;
        let snapshot = snapshot.ok_or(AuthorityError::InvalidRecord)?;
        let db = open_projection(&activation.join("authority.db"), &snapshot)?;
        Ok(Self { home: nano_home.to_owned(), journal: AuthorityJournal::open(&journal_path)?, db, snapshot, _lock: lock })
    }

    pub fn bootstrap_for_test(nano_home: &Path, snapshot: AuthoritySnapshot) -> Result<Self, AuthorityError> {
        let activation = nano_home.join("activation"); std::fs::create_dir_all(&activation)?;
        let journal_path = activation.join("authority.jsonl");
        let lock = FileLock::try_acquire(&activation.join("authority.lock")).map_err(|error| match error { nano_session::LockError::Busy => AuthorityError::Contention, nano_session::LockError::Io(error) => AuthorityError::Io(error) })?;
        if journal_path.exists() && std::fs::metadata(&journal_path)?.len() != 0 { return Err(AuthorityError::OperationConflict); }
        let mut journal = AuthorityJournal::open(&journal_path)?; journal.append_bootstrap(snapshot.clone())?;
        let db = open_projection(&activation.join("authority.db"), &snapshot)?;
        Ok(Self { home: nano_home.to_owned(), journal, db, snapshot, _lock: lock })
    }

    pub fn commit(&mut self, command: AuthorityCommand) -> Result<(), AuthorityError> { self.commit_with_fault(command, || {}) }

    pub fn commit_with_fault<F: FnOnce()>(&mut self, command: AuthorityCommand, after_journal: F) -> Result<(), AuthorityError> {
        let mut next = self.snapshot.clone();
        if !apply(&mut next, &command)? { return Ok(()); }
        self.journal.append_command(command)?;
        after_journal();
        write_projection(&mut self.db, &next)?;
        self.snapshot = next;
        Ok(())
    }

    pub fn consume_nonce(&mut self, nonce: &str, tuple: &str, expires_at_unix: i64) -> Result<(), AuthorityError> {
        let tuple_digest = crate::authority::hex(&Sha256::digest(tuple.as_bytes()));
        self.commit(AuthorityCommand::ConsumeNonce {
            operation_id: format!("nonce-{nonce}-{tuple_digest}"),
            nonce: nonce.to_owned(),
            tuple_digest,
            expires_at_unix,
        })
    }

    pub fn snapshot(&self) -> Result<AuthoritySnapshot, AuthorityError> {
        let encoded: String = self.db.query_row("SELECT snapshot FROM authority_projection WHERE singleton = 1", [], |row| row.get(0))?;
        serde_json::from_str(&encoded).map_err(|_| AuthorityError::InvalidRecord)
    }

    pub fn authorize(&self, issuer_id: &str, key_id: &str, subject_id: &str, principal_id: &str, project_id: &str) -> Result<[u8; 32], AuthorityError> {
        let issuer = self.snapshot.issuers.get(issuer_id).ok_or(AuthorityError::Unauthorized)?;
        if issuer.revoked || issuer.subject_id != subject_id || issuer.principal_id != principal_id || !issuer.projects.contains(project_id) { return Err(AuthorityError::Unauthorized); }
        let key = issuer.keys.get(key_id).ok_or(AuthorityError::Unauthorized)?;
        if key.revoked { return Err(AuthorityError::Unauthorized); }
        Ok(key.public_key)
    }

    pub fn authorize_unknown_record(&self, _kind: &str) -> Result<(), AuthorityError> { Err(AuthorityError::UnknownRecord) }
}

fn open_projection(path: &Path, snapshot: &AuthoritySnapshot) -> Result<Connection, AuthorityError> {
    let existing = Connection::open(path)?;
    existing.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS authority_projection (singleton INTEGER PRIMARY KEY CHECK(singleton=1), snapshot TEXT NOT NULL);")?;
    let encoded: Option<String> = existing.query_row("SELECT snapshot FROM authority_projection WHERE singleton=1", [], |row| row.get(0)).optional()?;
    if encoded.as_deref().and_then(|text| serde_json::from_str::<AuthoritySnapshot>(text).ok().as_ref().map(|_| ())).is_none() || encoded.as_deref().and_then(|text| serde_json::from_str::<AuthoritySnapshot>(text).ok()).as_ref() != Some(snapshot) {
        let mut db = existing; write_projection(&mut db, snapshot)?; Ok(db)
    } else { Ok(existing) }
}

fn write_projection(db: &mut Connection, snapshot: &AuthoritySnapshot) -> Result<(), AuthorityError> {
    let encoded = serde_json::to_string(snapshot).map_err(|_| AuthorityError::InvalidRecord)?;
    let tx = db.transaction()?;
    tx.execute("INSERT INTO authority_projection(singleton,snapshot) VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET snapshot=excluded.snapshot", params![encoded])?;
    tx.commit()?; Ok(())
}
