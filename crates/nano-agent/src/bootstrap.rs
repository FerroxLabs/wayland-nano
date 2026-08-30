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
//!
//! F-P4-3 layers single-writer session OWNERSHIP on top: a host that opens
//! a session for writing (session/new, session/load, exec) holds the OS
//! lock for the session's whole lifetime via [`SessionOwnership`], so a
//! second host's open of the same session is a typed busy instead of a
//! silent double-load. Per-turn guards on an owned journal take the
//! in-process layer only — the ownership lock is the OS layer.

use nano_session::FileLock;
use nano_session::JournalCoordinator;
use nano_session::Op;
use nano_session::OpEnvelope;
use nano_session::ReplayError;
use nano_session::SessionState;
use nano_session::read_journal;
use std::collections::HashMap;
use std::collections::HashSet;
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
    /// Journal paths this process OWNS for writing (F-P4-3 single-writer
    /// ownership): a [`SessionOwnership`] holds the OS lock for the whole
    /// session lifetime, so per-turn/per-fire guards on these paths take the
    /// in-process layer only (the OS layer is already held — re-acquiring it
    /// through a second handle would self-conflict on both platforms).
    owned: Arc<StdMutex<HashSet<PathBuf>>>,
}

static REGISTRY: std::sync::OnceLock<SessionGuardRegistry> = std::sync::OnceLock::new();

/// The process-wide registry every host shares (acp-host, exec, cron).
pub fn session_guard_registry() -> &'static SessionGuardRegistry {
    REGISTRY.get_or_init(SessionGuardRegistry::default)
}

/// The held guard: in-process permit + OS file lock, released on drop. The
/// OS lock is `None` when this process already owns the journal for the
/// session lifetime (see [`SessionGuardRegistry::try_own`]) — the ownership
/// lock IS the OS layer then.
#[derive(Debug)]
pub struct SessionGuard {
    _permit: tokio::sync::OwnedMutexGuard<()>,
    _file: Option<FileLock>,
}

/// Lifetime write ownership of one session journal (F-P4-3): acquired when a
/// host opens a session for writing (session/new, session/load, exec) and
/// held until the session closes or the process dies — the OS releases the
/// handle lock on death, so a killed host never wedges the session (no lock
/// FILE, no stale-break logic). A second host/process opening the same
/// session for writing gets a typed busy, never a silent double-load; the
/// read-only session browser never takes this lock.
///
/// Dropping unregisters BEFORE the OS lock releases (Drop impls run before
/// field drops), so a racing guard can only see the fail-closed direction
/// (still-registered + still-locked, or unregistered + acquirable).
#[derive(Debug)]
pub struct SessionOwnership {
    path: PathBuf,
    owned: Arc<StdMutex<HashSet<PathBuf>>>,
    _file: FileLock,
}

impl Drop for SessionOwnership {
    fn drop(&mut self) {
        self.owned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.path);
    }
}

/// Contention is typed busy — the caller defers (cron skip) or reports
/// (fork/prompt), never queues silently.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("session busy: another turn, fork, cron fire, or owning host holds the session")]
    Busy,
    #[error("session guard io error: {0}")]
    Io(#[from] io::Error),
}

impl SessionGuardRegistry {
    /// Non-blocking acquire of both exclusion layers, in a fixed order
    /// (in-process mutex first, then the OS lock) so two contenders in one
    /// process cannot deadlock across layers. On a journal this process
    /// OWNS (try_own), the OS layer is skipped — the lifetime ownership lock
    /// already excludes every other process, and a second handle would
    /// self-conflict.
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
        let owned_here = self
            .owned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&key);
        let file = if owned_here {
            None
        } else {
            match FileLock::try_acquire(journal_path) {
                Ok(file) => Some(file),
                Err(nano_session::LockError::Busy) => return Err(GuardError::Busy),
                Err(nano_session::LockError::Io(err)) => return Err(GuardError::Io(err)),
            }
        };
        Ok(SessionGuard {
            _permit: permit,
            _file: file,
        })
    }

    /// Take lifetime write ownership of a session journal (F-P4-3). The OS
    /// lock is acquired first (the cross-process arbiter), then the path is
    /// registered so this process's per-turn guards take the in-process
    /// layer only. A journal already owned — by this process or any other —
    /// is a typed [`GuardError::Busy`], never a silent second writer.
    pub fn try_own(&self, journal_path: &Path) -> Result<SessionOwnership, GuardError> {
        let key = journal_path.to_path_buf();
        if self
            .owned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&key)
        {
            return Err(GuardError::Busy);
        }
        // Ownership precedes the first journal open, so it inherits the
        // writer's create-the-parents discipline (the JournalWriter open).
        if let Some(parent) = journal_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = match FileLock::try_acquire(journal_path) {
            Ok(file) => file,
            Err(nano_session::LockError::Busy) => return Err(GuardError::Busy),
            Err(nano_session::LockError::Io(err)) => return Err(GuardError::Io(err)),
        };
        // Registration after the OS acquire can only fail-closed for a
        // racing same-process guard (it attempts the OS lock and sees busy).
        self.owned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key.clone());
        Ok(SessionOwnership {
            path: key,
            owned: Arc::clone(&self.owned),
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

#[derive(Debug)]
pub struct HookedBootstrappedSession {
    session: BootstrappedSession,
    hooks: Arc<nano_hooks::HookEngine>,
}

impl std::ops::Deref for HookedBootstrappedSession {
    type Target = BootstrappedSession;
    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl Drop for HookedBootstrappedSession {
    fn drop(&mut self) {
        let run = run_lifecycle_hook(
            self.hooks.clone(),
            nano_hooks::HookEvent::SessionEnd,
            Some("drop"),
            serde_json::json!({"hook_event_name":"SessionEnd", "session_id":self.session_id, "reason":"drop"}),
        );
        append_lifecycle_decisions(
            &self.journal_path,
            &self.session_id,
            nano_session::op::HookEvent::SessionEnd,
            &run,
        );
    }
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
    bootstrap_base(sessions_dir, cwd, seed)
}

/// Fresh bootstrap with an already reserved session id. Authenticated hosts
/// use this only after their trusted activation ledger has durably bound the
/// id, preserving the same journal format and fold semantics as `New`.
pub fn bootstrap_bound_session(
    sessions_dir: &Path,
    cwd: &Path,
    session_id: String,
) -> Result<BootstrappedSession, BootstrapError> {
    if !is_fs_safe_session_id(&session_id) {
        return Err(BootstrapError::InvalidSessionId(session_id));
    }
    bootstrap_new(sessions_dir, cwd, session_id)
}

pub fn bootstrap_session_with_hooks(
    sessions_dir: &Path,
    cwd: &Path,
    seed: SessionSeed,
    hooks: Arc<nano_hooks::HookEngine>,
) -> Result<HookedBootstrappedSession, BootstrapError> {
    let source = if matches!(seed, SessionSeed::New) {
        "startup"
    } else {
        "resume"
    };
    let mut session = bootstrap_base(sessions_dir, cwd, seed)?;
    {
        let run = run_lifecycle_hook(
            hooks.clone(),
            nano_hooks::HookEvent::SessionStart,
            Some(source),
            serde_json::json!({"hook_event_name":"SessionStart", "session_id":session.session_id, "source":source}),
        );
        append_lifecycle_decisions(
            &session.journal_path,
            &session.session_id,
            nano_session::op::HookEvent::SessionStart,
            &run,
        );
        if let Ok(report) = read_journal(&session.journal_path) {
            session.envelopes = report.envelopes;
        }
    }
    Ok(HookedBootstrappedSession { session, hooks })
}

fn bootstrap_base(
    sessions_dir: &Path,
    cwd: &Path,
    seed: SessionSeed,
) -> Result<BootstrappedSession, BootstrapError> {
    match seed {
        SessionSeed::New => {
            let session_id = new_session_id();
            bootstrap_new(sessions_dir, cwd, session_id)
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
            let writer = JournalCoordinator::open(&journal_path)?;
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

fn bootstrap_new(
    sessions_dir: &Path,
    cwd: &Path,
    session_id: String,
) -> Result<BootstrappedSession, BootstrapError> {
    let journal_path = sessions_dir.join(format!("{session_id}.jsonl"));
    let writer = JournalCoordinator::open(&journal_path)?;
    writer.append(&OpEnvelope::new(
        format!("{session_id}-begin-1"),
        "now",
        Op::SessionBegin {
            session_id: session_id.clone(),
            cwd: cwd.display().to_string(),
        },
    ))?;
    let report =
        read_journal(&journal_path).map_err(|err| BootstrapError::Corrupt(err.to_string()))?;
    let state = SessionState::fold_strict(&report.envelopes)?;
    Ok(BootstrappedSession {
        session_id,
        journal_path,
        envelopes: report.envelopes,
        state,
        turn_counter: 0,
    })
}

fn run_lifecycle_hook(
    hooks: Arc<nano_hooks::HookEngine>,
    event: nano_hooks::HookEvent,
    matcher: Option<&str>,
    payload: serde_json::Value,
) -> nano_hooks::HookRun {
    let matcher = matcher.map(str::to_string);
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(hooks.run(event, matcher.as_deref(), &payload)))
            .unwrap_or_default()
    })
    .join()
    .unwrap_or_default()
}

fn append_lifecycle_decisions(
    journal_path: &Path,
    session_id: &str,
    event: nano_session::op::HookEvent,
    run: &nano_hooks::HookRun,
) {
    let Ok(writer) = JournalCoordinator::open(journal_path) else {
        eprintln!("wayland-nano: lifecycle hook decision journal unavailable");
        return;
    };
    for (index, decision) in run.decisions.iter().enumerate() {
        let envelope = OpEnvelope::new(
            format!("{session_id}-hook-{}-{}", std::process::id(), index),
            "now",
            Op::HookDecision {
                turn_id: session_id.to_string(),
                event,
                handler_id: decision.handler_id.clone(),
                matcher_input: decision.matcher_input.clone(),
                outcome: match decision.outcome {
                    nano_hooks::HookOutcome::Pass => nano_session::op::HookOutcome::Pass,
                    nano_hooks::HookOutcome::Blocked => nano_session::op::HookOutcome::Blocked,
                    nano_hooks::HookOutcome::Failed => nano_session::op::HookOutcome::Failed,
                    nano_hooks::HookOutcome::Timeout => nano_session::op::HookOutcome::Timeout,
                    nano_hooks::HookOutcome::BoundedOutput => {
                        nano_session::op::HookOutcome::BoundedOutput
                    }
                },
                duration_ms: decision.duration_ms,
            },
        );
        if writer.append(&envelope).is_err() {
            eprintln!("wayland-nano: lifecycle hook decision journal unavailable");
            break;
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

#[cfg(test)]
mod ownership_tests {
    //! F-P4-3: single-writer session ownership. The cross-process legs spawn
    //! the current test binary as a fixture child (the session_browser
    //! `lock_holder_fixture` pattern).

    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nano-s3-ownership-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn ownership_excludes_second_writer_while_turn_guards_serialize() {
        let dir = unique_dir("inproc");
        let path = dir.join("s.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        let registry = SessionGuardRegistry::default();

        let ownership = registry.try_own(&path).expect("first owner");
        // A second owner in the same process is typed busy (owned-set), as
        // is a raw second-handle OS acquisition (what another process sees).
        match registry.try_own(&path) {
            Err(GuardError::Busy) => {}
            other => panic!("second owner must be typed busy, got {other:?}"),
        }
        match FileLock::try_acquire(&path) {
            Err(nano_session::LockError::Busy) => {}
            other => panic!("second OS handle must be busy, got {other:?}"),
        }
        // The owning host's own turn/cron guards take the in-process layer
        // only: first acquires, a concurrent second is typed busy.
        let guard = registry.try_acquire(&path).expect("owner's turn guard");
        match registry.try_acquire(&path) {
            Err(GuardError::Busy) => {}
            other => panic!("concurrent guard must be busy, got {other:?}"),
        }
        drop(guard);
        registry.try_acquire(&path).expect("guard after release");
        // Close then reopen in the same process (the session/new →
        // session/load-same-id flow) must reacquire cleanly.
        drop(ownership);
        let reopened = registry.try_own(&path).expect("re-own after close");
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fixture child: own the journal named by NANO_OWNERSHIP_HOLD_PATH,
    /// signal readiness, then hold until killed (no clean exit path — the
    /// parent kills it to prove OS-level release on process death).
    #[test]
    fn ownership_holder_fixture() {
        let Some(path) = std::env::var_os("NANO_OWNERSHIP_HOLD_PATH") else {
            return;
        };
        let ready =
            PathBuf::from(std::env::var_os("NANO_OWNERSHIP_READY_PATH").expect("ready path"));
        let _ownership = session_guard_registry()
            .try_own(Path::new(&path))
            .expect("fixture owns the journal");
        std::fs::write(&ready, b"ready").unwrap();
        // Bounded lifetime: if the parent dies before killing us, exit
        // rather than holding a temp-file lock forever.
        for _ in 0..1200 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn killed_holder_releases_and_a_new_owner_reacquires() {
        let dir = unique_dir("kill");
        let path = dir.join("s.jsonl");
        let ready = dir.join("ready");
        std::fs::write(&path, "{}\n").unwrap();

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bootstrap::ownership_tests::ownership_holder_fixture",
                "--nocapture",
            ])
            .env("NANO_OWNERSHIP_HOLD_PATH", &path)
            .env("NANO_OWNERSHIP_READY_PATH", &ready)
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            assert!(child.try_wait().unwrap().is_none(), "fixture exited early");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(ready.exists(), "fixture did not acquire ownership");

        // While the child lives, a second host's open is typed busy.
        let registry = SessionGuardRegistry::default();
        match registry.try_own(&path) {
            Err(GuardError::Busy) => {}
            other => panic!("cross-process second owner must be busy, got {other:?}"),
        }
        match registry.try_acquire(&path) {
            Err(GuardError::Busy) => {}
            other => panic!("cross-process guard must be busy, got {other:?}"),
        }

        // kill -9 equivalent: no Drop, no clean close — the OS must release
        // the handle lock, and the next owner (crash-resume in a fresh host)
        // reacquires cleanly.
        child.kill().unwrap();
        child.wait().unwrap();
        let ownership = registry
            .try_own(&path)
            .expect("ownership after holder death");
        drop(ownership);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
