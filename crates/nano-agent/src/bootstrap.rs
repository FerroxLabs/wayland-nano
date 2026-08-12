//! Session bootstrap + the SessionGuard (C11 §2.1/§3.1).
//!
//! ONE honest bootstrap path for every entry point (acp-host, protocol-host,
//! exec, cron fire): resolve the journal path, open/create fail-closed,
//! journal the genesis/resume `SessionBegin`, read + fold. Adapters stay
//! thin; lifecycle orchestration lives here, never in a CLI or ACP adapter.
//!
//! The SessionGuard is the one engine-library exclusion for interactive
//! turns, forks, and cron fires, in two layers: an in-process async mutex
//! keyed by journal path (serializes within one host) and the OS advisory
//! lock on the journal file (serializes ACROSS hosts and covers unloaded
//! sessions). Contention is a typed busy error, never a silent queue that
//! reorders user intent.

use nano_session::FileLock;
use nano_session::JournalWriter;
use nano_session::Op;
use nano_session::OpEnvelope;
use nano_session::ReplayError;
use nano_session::SessionState;
use nano_session::read_journal;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::Weak;

/// In-process mutex registry keyed by canonical journal path. Entries are
/// weak so idle sessions never leak.
#[derive(Debug, Default)]
pub struct SessionGuardRegistry {
    inner: StdMutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
}

static REGISTRY: std::sync::OnceLock<SessionGuardRegistry> = std::sync::OnceLock::new();

/// The process-wide registry every host shares (acp-host, exec, cron).
pub fn session_guard_registry() -> &'static SessionGuardRegistry {
    REGISTRY.get_or_init(SessionGuardRegistry::default)
}

/// The held guard: in-process permit + OS file lock, released on drop.
#[derive(Debug)]
pub struct SessionGuard {
    _permit: tokio::sync::OwnedMutexGuard<()>,
    _file: FileLock,
}

/// Contention is typed busy — the caller defers (cron skip) or reports
/// (fork/prompt), never queues silently.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("session busy: another turn, fork, or cron fire holds the guard")]
    Busy,
    #[error("session guard io error: {0}")]
    Io(#[from] io::Error),
}

impl SessionGuardRegistry {
    /// Non-blocking acquire of both exclusion layers, in a fixed order
    /// (in-process mutex first, then the OS lock) so two contenders in one
    /// process cannot deadlock across layers.
    pub fn try_acquire(&self, journal_path: &Path) -> Result<SessionGuard, GuardError> {
        let key = journal_path.to_path_buf();
        let mutex = {
            let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            match map.get(&key).and_then(Weak::upgrade) {
                Some(existing) => existing,
                None => {
                    let fresh = Arc::new(tokio::sync::Mutex::new(()));
                    map.insert(key.clone(), Arc::downgrade(&fresh));
                    fresh
                }
            }
        };
        let permit = mutex.try_lock_owned().map_err(|_| GuardError::Busy)?;
        let file = match FileLock::try_acquire(journal_path) {
            Ok(file) => file,
            Err(nano_session::LockError::Busy) => return Err(GuardError::Busy),
            Err(nano_session::LockError::Io(err)) => return Err(GuardError::Io(err)),
        };
        Ok(SessionGuard {
            _permit: permit,
            _file: file,
        })
    }
}

/// What a bootstrap starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSeed {
    /// Fresh session: a new id, a genesis `SessionBegin` journaled
    /// fail-closed (a session we cannot journal is a session we could not
    /// honestly resume later).
    New,
    /// Resume an existing session by id: the SAME journal is appended (a
    /// fresh `SessionBegin` resume marker), never rewritten.
    Resume(String),
}

#[derive(Debug)]
pub struct BootstrappedSession {
    pub session_id: String,
    pub journal_path: PathBuf,
    /// Every envelope through bootstrap (post genesis/resume marker).
    pub envelopes: Vec<OpEnvelope>,
    /// The folded live state (goal normalization, suppression, cron
    /// occurrence sets all applied).
    pub state: SessionState,
    /// Turns journaled so far — turn ids never collide across resumes.
    pub turn_counter: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("invalid session id: {0}")]
    InvalidSessionId(String),
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("session journal io error: {0}")]
    Io(#[from] io::Error),
    #[error("session journal unreadable: {0}")]
    Corrupt(String),
    #[error("session journal replay error: {0}")]
    Replay(#[from] ReplayError),
}

/// Session ids are filesystem-safe (they name the journal file) and unique
/// per session without embedding the pid: nanosecond clock plus a
/// process-local counter.
pub fn new_session_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("wayland-nano-session-{nanos}-{n}")
}

/// Session ids name a journal file directly, so anything that could escape
/// the sessions directory (or not round-trip as a filename) is rejected.
pub fn is_fs_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The ONE session bootstrap (C11 §2.1). New: create + genesis. Resume:
/// replay + resume marker. Both journal-first and fail-closed; the returned
/// state is the strict fold (a fork lineage overrun is a typed error, never
/// a silent short fold).
pub fn bootstrap_session(
    sessions_dir: &Path,
    cwd: &Path,
    seed: SessionSeed,
) -> Result<BootstrappedSession, BootstrapError> {
    match seed {
        SessionSeed::New => {
            let session_id = new_session_id();
            let journal_path = sessions_dir.join(format!("{session_id}.jsonl"));
            let mut writer = JournalWriter::open(&journal_path)?;
            writer.append(&OpEnvelope::new(
                format!("{session_id}-begin-1"),
                "now",
                Op::SessionBegin {
                    session_id: session_id.clone(),
                    cwd: cwd.display().to_string(),
                },
            ))?;
            let report = read_journal(&journal_path)
                .map_err(|err| BootstrapError::Corrupt(err.to_string()))?;
            let state = SessionState::fold_strict(&report.envelopes)?;
            Ok(BootstrappedSession {
                session_id,
                journal_path,
                envelopes: report.envelopes,
                state,
                turn_counter: 0,
            })
        }
        SessionSeed::Resume(id) => {
            if !is_fs_safe_session_id(&id) {
                return Err(BootstrapError::InvalidSessionId(id));
            }
            let journal_path = sessions_dir.join(format!("{id}.jsonl"));
            if !journal_path.exists() {
                return Err(BootstrapError::NotFound(id));
            }
            let report = read_journal(&journal_path)
                .map_err(|err| BootstrapError::Corrupt(err.to_string()))?;
            let turn_counter = report
                .envelopes
                .iter()
                .filter(|e| matches!(e.op, Op::TurnBegin { .. }))
                .count() as u64;
            let begin_count = report
                .envelopes
                .iter()
                .filter(|e| matches!(e.op, Op::SessionBegin { .. }))
                .count();
            // A fresh SessionBegin marks the resume in the journal itself
            // (audit trail; replay treats non-genesis begins as inert).
            let mut writer = JournalWriter::open(&journal_path)?;
            writer.append(&OpEnvelope::new(
                format!("{id}-begin-{}", begin_count + 1),
                "now",
                Op::SessionBegin {
                    session_id: id.clone(),
                    cwd: cwd.display().to_string(),
                },
            ))?;
            let report = read_journal(&journal_path)
                .map_err(|err| BootstrapError::Corrupt(err.to_string()))?;
            let state = SessionState::fold_strict(&report.envelopes)?;
            Ok(BootstrappedSession {
                session_id: id,
                journal_path,
                envelopes: report.envelopes,
                state,
                turn_counter,
            })
        }
    }
}

/// The newest journal in a sessions directory (for `--resume-last`):
/// lexicographically-last by modified time; an empty/unreadable directory
/// is a typed not-found.
pub fn latest_session_id(sessions_dir: &Path) -> Result<Option<String>, io::Error> {
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    let entries = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".jsonl")) else {
            continue;
        };
        if !is_fs_safe_session_id(stem) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(time, _)| modified >= *time) {
            newest = Some((modified, stem.to_string()));
        }
    }
    Ok(newest.map(|(_, id)| id))
}
