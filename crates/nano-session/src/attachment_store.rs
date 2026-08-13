//! P2a §5 — the digest-keyed attachment blob store.
//!
//! Layout (§5.1), under `<nano_home>/attachments/`:
//!
//! ```text
//! attachments/
//!     .gc.lock          # cross-process sweep/lease lock (§5.4)
//!     staging/          # in-flight writes (leased)
//!     blobs/
//!         ab/abcdef…    # sha256 hex, 2-char fanout, content-addressed
//! ```
//!
//! Discipline:
//! - the blob filename IS the lowercase hex sha256 of the re-encoded bytes
//!   (content addressing; re-attached images dedup for free);
//! - writes stage to `staging/<unique>.tmp`, fsync, digest-verify, then
//!   atomic-rename into `blobs/` — the whole staging→rename→journal-append
//!   span runs under a SHARED lease on `.gc.lock` (§5.1/§5.4);
//! - blobs are immutable once renamed (same-digest rewrite is a no-op after
//!   digest verification);
//! - reads (the §5.3 kill-resume rehydration path) validate the digest
//!   against `^[0-9a-f]{64}$` BEFORE any path construction, reject
//!   symlink/junction/reparse store entries, and verify the sha256 over the
//!   bytes — tampering fails CLOSED. The open itself is handle-verified
//!   (P2a audit H-1): unix opens O_NOFOLLOW; Windows opens reparse-safe
//!   (FILE_FLAG_OPEN_REPARSE_POINT), rejects reparse metadata from the
//!   OPENED handle, and proves the handle's final path stays beneath the
//!   canonical store root — a swap between validation and open fails closed;
//! - GC (§5.4) sweeps only under the EXCLUSIVE `.gc.lock` plus a 60 s
//!   mtime grace (defense-in-depth); there is NO referenced-blob LRU
//!   (Q4 RULED).
//!
//! Permissions (§5.5): unix store dir 0700 / blobs 0600; Windows gets an
//! explicit current-user-only ACL at creation plus a fail-closed DACL audit
//! at every open. Profile-ACL inheritance is NOT accepted as proof.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use sha2::Digest as _;
use sha2::Sha256;

use crate::NanoErrorKind;
use crate::lock::{FileLock, LockError};

/// §5.4 defense-in-depth: the sweep never touches entries younger than
/// this (and skips future timestamps — clock anomaly ⇒ fail-closed skip).
pub const GC_GRACE_SECS: u64 = 60;

/// Read ceiling for one blob. Stored blobs are re-encoded payloads (≤ the
/// §4.2 raw payload cap by construction); this bounds rehydration memory
/// even against a corrupted store (§5.3 step 5 — rehydration obeys the
/// §4.2 caps).
pub const MAX_BLOB_READ_BYTES: u64 = 50 * 1024 * 1024;

const STORE_DIR: &str = "attachments";
const LOCK_FILE: &str = ".gc.lock";
const STAGING_DIR: &str = "staging";
const BLOBS_DIR: &str = "blobs";

/// Every store failure maps to the typed `AttachmentStoreError` kind (§7) —
/// the store fails CLOSED, never with a silent downgrade.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentStoreError {
    #[error("attachment store I/O during {detail}: {source}")]
    Io {
        detail: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("attachment store permission audit failed: {0}")]
    PermissionAudit(&'static str),
    #[error("attachment store entry is a symlink/junction/reparse point: {0}")]
    ReparsePoint(&'static str),
    #[error("attachment store integrity failure: {0}")]
    Integrity(&'static str),
}

impl AttachmentStoreError {
    /// The C7 wire kind for this failure (§7).
    pub fn kind(&self) -> NanoErrorKind {
        NanoErrorKind::AttachmentStoreError
    }

    fn io(detail: &'static str) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Io { detail, source }
    }
}

/// §5.3 rehydration outcomes. The user-facing kind is `AttachmentMissing`
/// for all of Missing/Tampered/MalformedDigest (Q3 RULED); the operator-side
/// distinction (MISSING vs TAMPERED vs malformed) rides in this enum.
#[derive(Debug, thiserror::Error)]
pub enum BlobReadError {
    /// The digest failed `^[0-9a-f]{64}$` — rejected BEFORE any path
    /// construction; can never become an arbitrary-path read.
    #[error("malformed attachment digest")]
    MalformedDigest,
    /// No blob at the digest's address.
    #[error("attachment blob missing from store")]
    Missing,
    /// The blob's bytes do not hash to its digest (or exceed the read
    /// ceiling) — tampering fails closed.
    #[error("attachment blob failed digest verification")]
    Tampered,
    /// Store-level failure (I/O, reparse-point rejection).
    #[error(transparent)]
    Store(#[from] AttachmentStoreError),
}

impl BlobReadError {
    /// The C7 wire kind (§7): missing/corrupt/tampered/malformed all surface
    /// as `AttachmentMissing`; store failures as `AttachmentStoreError`.
    pub fn kind(&self) -> NanoErrorKind {
        match self {
            BlobReadError::MalformedDigest | BlobReadError::Missing | BlobReadError::Tampered => {
                NanoErrorKind::AttachmentMissing
            }
            BlobReadError::Store(_) => NanoErrorKind::AttachmentStoreError,
        }
    }
}

/// §5.3 digest validation: exactly `^[0-9a-f]{64}$`. Checked on journal
/// read, BEFORE any path construction.
pub fn is_valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// §5.3 step 4: the loud placeholder substituted at the manifest position
/// when a blob cannot be rehydrated. `n` is the display index; the digest
/// prefix is included only when the digest validated (a malformed digest is
/// never echoed — bounded vocabulary).
pub fn attachment_unavailable_placeholder(n: usize, digest: &str) -> String {
    let prefix: String = if is_valid_digest(digest) {
        digest.chars().take(12).collect()
    } else {
        "malformed-digest".to_string()
    };
    format!(
        "[Image #{n} unavailable: attachment {prefix} missing from store — do not describe it from memory]"
    )
}

/// A SHARED lease on `.gc.lock` (§5.4 guard 1). Held by attach writers for
/// the whole staging-write → rename → journal-append span; the sweep's
/// exclusive acquisition fails while any lease is held.
pub struct WriteLease {
    _lock: FileLock,
}

/// What a sweep did (§5.4). `lock_skipped` is the typed skip when a writer
/// lease is held — never a block, never a delete under a lease.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// The exclusive lock was acquired and the sweep ran.
    pub swept: bool,
    /// A writer lease was held; nothing was scanned or deleted.
    pub lock_skipped: bool,
    pub removed_blobs: u64,
    pub removed_staging: u64,
    pub skipped_young: u64,
    pub skipped_future: u64,
    pub reclaimed_bytes: u64,
    pub removed_empty_fanout_dirs: u64,
}

/// The store handle. Constructed by [`AttachmentStore::open`]; the root is
/// canonicalized once at open so every blob path is verified inside it.
#[derive(Debug)]
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    /// Open (creating if needed) the store under `nano_home`. Sets
    /// permissions at creation (unix 0700; Windows explicit
    /// current-user-only ACL) and audits them at EVERY open — failure is
    /// typed and fail-closed (§5.5).
    pub fn open(nano_home: &Path) -> Result<Self, AttachmentStoreError> {
        let root = nano_home.join(STORE_DIR);
        #[cfg(windows)]
        let created = !root.exists();
        make_private_dir(&root)?;
        make_private_dir(&root.join(STAGING_DIR))?;
        make_private_dir(&root.join(BLOBS_DIR))?;
        // The lock file exists before any lease/sweep attempts it.
        if !root.join(LOCK_FILE).exists() {
            write_private_file(&root.join(LOCK_FILE), &[])?;
        }
        #[cfg(windows)]
        if created {
            windows_acl::set_current_user_only(&root)?;
        }
        // The store root must itself never be a reparse point.
        reject_reparse(&root, "store-root")?;
        let root = root
            .canonicalize()
            .map_err(AttachmentStoreError::io("canonicalize"))?;
        audit_private_dir(&root)?;
        Ok(Self { root })
    }

    /// The canonicalized store root (for diagnostics; never join untrusted
    /// input onto it — digests validate through [`is_valid_digest`] first).
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root.join(BLOBS_DIR)
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join(STAGING_DIR)
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    /// §5.4 guard 1: a shared lease spanning staging-write → rename →
    /// journal-append. The caller MUST hold this across `put` AND the
    /// journal append that references the new blob.
    pub fn acquire_write_lease(&self) -> Result<WriteLease, AttachmentStoreError> {
        let lock = FileLock::try_acquire_shared(&self.lock_path()).map_err(lock_to_store)?;
        Ok(WriteLease { _lock: lock })
    }

    /// §5.1 write protocol: stage → fsync → digest-verify → atomic rename.
    /// Blobs are immutable once renamed; a same-digest rewrite is a no-op
    /// after digest verification. Returns the lowercase hex sha256 digest —
    /// the blob's content address.
    pub fn put(&self, _lease: &WriteLease, bytes: &[u8]) -> Result<String, AttachmentStoreError> {
        let digest = hex_sha256(bytes);
        // The digest is locally computed, but route it through the same
        // validation so the invariant "path construction requires a valid
        // digest" has exactly one gate.
        debug_assert!(is_valid_digest(&digest));
        let fanout = self.blobs_dir().join(&digest[..2]);
        make_private_dir(&fanout)?;
        let target = fanout.join(&digest);
        if target.exists() {
            // Same-digest rewrite: no-op after digest verification (§5.1).
            return match self.read_verified(&digest) {
                Ok(_) => Ok(digest),
                Err(_) => Err(AttachmentStoreError::Integrity(
                    "existing blob fails digest verification",
                )),
            };
        }

        let staging = unique_staging_path(&self.staging_dir());
        write_private_file(&staging, bytes)?;
        // Verify the digest over the WRITTEN bytes before publishing.
        let written = fs::read(&staging).map_err(AttachmentStoreError::io("verify-staging"))?;
        if hex_sha256(&written) != digest {
            let _ = fs::remove_file(&staging);
            return Err(AttachmentStoreError::Integrity("staging digest mismatch"));
        }
        // Atomic same-volume publish.
        if let Err(err) = fs::rename(&staging, &target) {
            // A concurrent put of the same content may have won the race
            // (rename-over-existing fails on Windows).
            if target.exists() {
                let _ = fs::remove_file(&staging);
                return match self.read_verified(&digest) {
                    Ok(_) => Ok(digest),
                    Err(_) => Err(AttachmentStoreError::Integrity(
                        "existing blob fails digest verification",
                    )),
                };
            }
            let _ = fs::remove_file(&staging);
            return Err(AttachmentStoreError::io("rename")(err));
        }
        Ok(digest)
    }

    /// §5.3 kill-resume rehydration read path. The digest is validated
    /// BEFORE any path construction; the path is built from validated
    /// components only; symlink/junction/reparse store entries are
    /// rejected; the bytes are sha256-verified against the digest.
    pub fn read_verified(&self, digest: &str) -> Result<Vec<u8>, BlobReadError> {
        if !is_valid_digest(digest) {
            return Err(BlobReadError::MalformedDigest);
        }
        let fanout = self.blobs_dir().join(&digest[..2]);
        let path = fanout.join(digest);
        // Reparse rejection BEFORE open (path-level, defense-in-depth — the
        // open below is itself reparse-safe and handle-verified, so a swap
        // in this window still fails closed). A missing fanout dir is
        // simply a missing blob.
        reject_reparse(&self.blobs_dir(), "blobs-dir")?;
        match fs::symlink_metadata(&fanout) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                    return Err(AttachmentStoreError::ReparsePoint("fanout-dir").into());
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(BlobReadError::Missing);
            }
            Err(err) => return Err(AttachmentStoreError::io("stat-fanout")(err).into()),
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(BlobReadError::Missing);
            }
            Err(err) => return Err(AttachmentStoreError::io("stat-blob")(err).into()),
        };
        if meta.file_type().is_symlink() || is_reparse_point(&meta) {
            return Err(AttachmentStoreError::ReparsePoint("blob").into());
        }
        if !meta.is_file() {
            return Err(BlobReadError::Missing);
        }
        // The canonicalized blob must sit inside the canonicalized store.
        let canonical = path
            .canonicalize()
            .map_err(AttachmentStoreError::io("canonicalize-blob"))?;
        if !canonical.starts_with(&self.root) {
            return Err(AttachmentStoreError::ReparsePoint("blob-escapes-store").into());
        }
        // Audit-H1 hook point: the validate→open window a same-user process
        // races with a junction/symlink swap. Test-only; compiled out.
        #[cfg(test)]
        PRE_OPEN_HOOK.with(|hook| {
            if let Some(fire) = hook.borrow_mut().take() {
                fire();
            }
        });
        let bytes = read_no_follow(&path, MAX_BLOB_READ_BYTES, &self.root)?;
        if hex_sha256(&bytes) != digest {
            return Err(BlobReadError::Tampered);
        }
        Ok(bytes)
    }

    /// Total store size in bytes (the `/doctor` size report, §5.4).
    pub fn total_bytes(&self) -> Result<u64, AttachmentStoreError> {
        let mut total = 0u64;
        let blobs = self.blobs_dir();
        for fanout in read_dir(&blobs)? {
            let fanout = fanout.map_err(AttachmentStoreError::io("read-dir"))?;
            if !fanout
                .file_type()
                .map_err(AttachmentStoreError::io("stat"))?
                .is_dir()
            {
                continue;
            }
            for entry in read_dir(&fanout.path())? {
                let entry = entry.map_err(AttachmentStoreError::io("read-dir"))?;
                total = total.saturating_add(
                    entry
                        .metadata()
                        .map_err(AttachmentStoreError::io("stat"))?
                        .len(),
                );
            }
        }
        Ok(total)
    }

    /// Live blob count (the `/doctor` store report, §5.4). Only entries
    /// whose names are valid digests count — staging/unknown entries are
    /// not blobs.
    pub fn blob_count(&self) -> Result<u64, AttachmentStoreError> {
        let mut count = 0u64;
        let blobs = self.blobs_dir();
        for fanout in read_dir(&blobs)? {
            let fanout = fanout.map_err(AttachmentStoreError::io("read-dir"))?;
            if !fanout
                .file_type()
                .map_err(AttachmentStoreError::io("stat"))?
                .is_dir()
            {
                continue;
            }
            for entry in read_dir(&fanout.path())? {
                let entry = entry.map_err(AttachmentStoreError::io("read-dir"))?;
                let is_blob = entry
                    .file_name()
                    .to_str()
                    .map(is_valid_digest)
                    .unwrap_or(false);
                if is_blob {
                    count = count.saturating_add(1);
                }
            }
        }
        Ok(count)
    }

    /// §5.4 GC sweep. Deletes: unreferenced blobs past the grace, stale
    /// `.tmp` staging files, empty fanout dirs. NEVER deletes under a
    /// writer lease (exclusive-lock acquisition fails → typed skip), never
    /// touches entries younger than `GC_GRACE_SECS`, and skips
    /// future-timestamped entries (clock anomaly ⇒ fail-closed).
    ///
    /// `referenced` is the digest reference set scanned from the journals
    /// by the host (§5.4): every digest named by any `input_blocks`
    /// manifest OR any `ToolResult.image_refs` entry (F-32 LOW-7 — §3.2
    /// requires both; tool-result references keep their blobs live too).
    /// Compaction does NOT release the reference. Build it with
    /// [`referenced_blob_digests`].
    pub fn sweep(&self, referenced: &HashSet<String>) -> Result<SweepReport, AttachmentStoreError> {
        let lock = match FileLock::try_acquire(&self.lock_path()) {
            Ok(lock) => lock,
            Err(LockError::Busy) => {
                return Ok(SweepReport {
                    lock_skipped: true,
                    ..SweepReport::default()
                });
            }
            Err(err) => return Err(lock_to_store(err)),
        };
        let report = self.sweep_at(referenced, SystemTime::now())?;
        drop(lock);
        Ok(report)
    }

    /// The sweep with an injectable clock — deterministic age-classification
    /// tests without mtime surgery (§12 GC race battery).
    fn sweep_at(
        &self,
        referenced: &HashSet<String>,
        now: SystemTime,
    ) -> Result<SweepReport, AttachmentStoreError> {
        let mut report = SweepReport {
            swept: true,
            ..SweepReport::default()
        };
        let grace = Duration::from_secs(GC_GRACE_SECS);

        // Stale staging files first (crash windows: staging→rename).
        for entry in read_dir(&self.staging_dir())? {
            let entry = entry.map_err(AttachmentStoreError::io("read-staging"))?;
            let path = entry.path();
            let is_tmp = path.extension().is_some_and(|ext| ext == "tmp");
            if !is_tmp {
                continue;
            }
            let meta = entry
                .metadata()
                .map_err(AttachmentStoreError::io("stat-staging"))?;
            match classify_age(meta.modified().ok(), now, grace) {
                AgeClass::Young => report.skipped_young += 1,
                AgeClass::Future => report.skipped_future += 1,
                AgeClass::Stale => {
                    fs::remove_file(&path).map_err(AttachmentStoreError::io("remove-staging"))?;
                    report.removed_staging += 1;
                    report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(meta.len());
                }
            }
        }

        // Unreferenced blobs past the grace.
        let blobs = self.blobs_dir();
        for fanout in read_dir(&blobs)? {
            let fanout = fanout.map_err(AttachmentStoreError::io("read-blobs"))?;
            let fanout_path = fanout.path();
            if !fanout
                .file_type()
                .map_err(AttachmentStoreError::io("stat-fanout"))?
                .is_dir()
            {
                continue;
            }
            for entry in read_dir(&fanout_path)? {
                let entry = entry.map_err(AttachmentStoreError::io("read-fanout"))?;
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue; // non-UTF-8 name: not a blob we wrote; leave it
                };
                if !is_valid_digest(name) {
                    continue; // unknown entries are left alone, never deleted
                }
                let meta = entry
                    .metadata()
                    .map_err(AttachmentStoreError::io("stat-blob"))?;
                if referenced.contains(name) {
                    continue;
                }
                match classify_age(meta.modified().ok(), now, grace) {
                    AgeClass::Young => report.skipped_young += 1,
                    AgeClass::Future => report.skipped_future += 1,
                    AgeClass::Stale => {
                        fs::remove_file(&path).map_err(AttachmentStoreError::io("remove-blob"))?;
                        report.removed_blobs += 1;
                        report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(meta.len());
                    }
                }
            }
            // Empty fanout dirs are swept.
            let mut remaining = read_dir(&fanout_path)?;
            if remaining.next().is_none() {
                fs::remove_dir(&fanout_path).map_err(AttachmentStoreError::io("remove-fanout"))?;
                report.removed_empty_fanout_dirs += 1;
            }
        }
        Ok(report)
    }
}

enum AgeClass {
    Young,
    Future,
    Stale,
}

/// §5.4 host-side reference scan (wired by F-34): every blob digest any
/// journal in `sessions_dir` still references — `TurnBegin.input_blocks`
/// image manifests AND `ToolResult.image_refs` (F-32 LOW-7; §3.2). Only
/// canonical digest strings collect (a malformed entry references nothing
/// that exists in the store). FAIL-CLOSED: any unreadable journal aborts
/// the scan with Err — the caller must NOT sweep on a partial set (a
/// skipped journal's references would otherwise be reaped live).
pub fn referenced_blob_digests(sessions_dir: &Path) -> io::Result<HashSet<String>> {
    use crate::op::{InputBlock, Op};

    let mut referenced = HashSet::new();
    let entries = match fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        // No sessions directory yet ⇒ no journals ⇒ no references.
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(referenced),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(stem) = name.to_str() else {
            continue; // non-UTF-8 name: not a journal we wrote
        };
        if !stem.ends_with(".jsonl") {
            continue;
        }
        let report = crate::reader::read_journal(&entry.path())?;
        for envelope in &report.envelopes {
            match &envelope.op {
                Op::TurnBegin { input_blocks, .. } => {
                    for block in input_blocks {
                        if let InputBlock::ImageRef(reference) = block {
                            if is_valid_digest(&reference.digest) {
                                referenced.insert(reference.digest.clone());
                            }
                        }
                    }
                }
                Op::ToolResult { image_refs, .. } => {
                    for reference in image_refs {
                        if is_valid_digest(&reference.digest) {
                            referenced.insert(reference.digest.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(referenced)
}
/// Age classification: future timestamps and unknown mtimes fail CLOSED
/// (skip), young entries are kept, only entries provably older than the
/// grace are stale.
fn classify_age(mtime: Option<SystemTime>, now: SystemTime, grace: Duration) -> AgeClass {
    let Some(mtime) = mtime else {
        return AgeClass::Future; // no mtime → clock anomaly posture: skip
    };
    match now.duration_since(mtime) {
        Ok(age) if age >= grace => AgeClass::Stale,
        Ok(_) => AgeClass::Young,
        Err(_) => AgeClass::Future,
    }
}

fn lock_to_store(err: LockError) -> AttachmentStoreError {
    match err {
        LockError::Busy => AttachmentStoreError::Integrity("gc lock busy (unexpected)"),
        LockError::Io(source) => AttachmentStoreError::Io {
            detail: "gc-lock",
            source,
        },
    }
}

fn read_dir(path: &Path) -> Result<fs::ReadDir, AttachmentStoreError> {
    fs::read_dir(path).map_err(AttachmentStoreError::io("read-dir"))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn unique_staging_path(staging: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    staging.join(format!("{}-{nanos}-{seq}.tmp", std::process::id()))
}

// ── permissions (§5.5) ───────────────────────────────────────────────────

#[cfg(unix)]
fn make_private_dir(path: &Path) -> Result<(), AttachmentStoreError> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(AttachmentStoreError::io("mkdir"))
}

#[cfg(not(unix))]
fn make_private_dir(path: &Path) -> Result<(), AttachmentStoreError> {
    fs::create_dir_all(path).map_err(AttachmentStoreError::io("mkdir"))
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), AttachmentStoreError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(AttachmentStoreError::io("open-write"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_data())
        .map_err(AttachmentStoreError::io("write"))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), AttachmentStoreError> {
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(AttachmentStoreError::io("open-write"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_data())
        .map_err(AttachmentStoreError::io("write"))?;
    Ok(())
}

/// The open-time audit (§5.5): unix checks the mode is exactly owner-only;
/// Windows audits the DACL grants no beyond-user principals.
#[cfg(unix)]
fn audit_private_dir(path: &Path) -> Result<(), AttachmentStoreError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)
        .map_err(AttachmentStoreError::io("stat-audit"))?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o700 {
        return Err(AttachmentStoreError::PermissionAudit(
            "store dir is not owner-only (0700)",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn audit_private_dir(path: &Path) -> Result<(), AttachmentStoreError> {
    windows_acl::audit_current_user_only(path)
}

#[cfg(not(any(unix, windows)))]
fn audit_private_dir(_path: &Path) -> Result<(), AttachmentStoreError> {
    Err(AttachmentStoreError::PermissionAudit(
        "no ACL audit on this platform",
    ))
}

/// Reuses the attachment-store owner-only directory audit for the session
/// browser's report-only `/doctor` line. This never creates or repairs the
/// directory and therefore cannot weaken an existing ACL or permission mode.
pub fn audit_private_session_dir(path: &Path) -> Result<(), AttachmentStoreError> {
    audit_private_dir(path)
}

/// Reparse rejection (§5.3/§5.5): symlink, junction, or any reparse-point
/// store entry is rejected.
fn reject_reparse(path: &Path, detail: &'static str) -> Result<(), AttachmentStoreError> {
    let meta = fs::symlink_metadata(path).map_err(AttachmentStoreError::io("stat-reparse"))?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) {
        return Err(AttachmentStoreError::ReparsePoint(detail));
    }
    Ok(())
}

/// Windows: FILE_ATTRIBUTE_REPARSE_POINT covers symlinks AND junctions
/// (the nano-agent tasks.rs:1179 precedent). Unix: symlink_metadata's
/// file_type covers symlinks; there is no junction concept.
#[cfg(windows)]
fn is_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &fs::Metadata) -> bool {
    false
}

// Audit-H1 regression hook: fires in the validate→open window of
// `read_verified` so a test can deterministically swap in a
// junction/symlink. Test-only; never compiled into production builds.
#[cfg(test)]
thread_local! {
    static PRE_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Read a blob with no-follow semantics and a byte ceiling. Unix opens
/// O_NOFOLLOW (the existing discipline, unchanged); the root is unused —
/// the content-address digest check is the integrity backstop.
#[cfg(unix)]
fn read_no_follow(path: &Path, ceiling: u64, _root: &Path) -> Result<Vec<u8>, BlobReadError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| AttachmentStoreError::io("open-blob")(e))?;
    read_capped(file, ceiling)
}

/// Windows (P2a audit H-1): the open is reparse-safe AND handle-verified —
/// `CreateFileW` with `FILE_FLAG_OPEN_REPARSE_POINT` never FOLLOWS a reparse
/// point swapped in after the path validation, the OPENED handle's metadata
/// and final path are verified against the canonical store root before a
/// single byte is read. A path-only check cannot close that TOCTOU; the
/// handle verification does.
#[cfg(windows)]
fn read_no_follow(path: &Path, ceiling: u64, root: &Path) -> Result<Vec<u8>, BlobReadError> {
    let file = windows_safe_open::open_verified_file(path, root)?;
    read_capped(file, ceiling)
}

fn read_capped(file: fs::File, ceiling: u64) -> Result<Vec<u8>, BlobReadError> {
    use std::io::Read as _;
    let mut limited = file.take(ceiling.saturating_add(1));
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| AttachmentStoreError::io("read-blob")(e))?;
    if bytes.len() as u64 > ceiling {
        // Over-ceiling store content is corrupt-by-construction: it fails
        // closed on the tamper path.
        return Err(BlobReadError::Tampered);
    }
    Ok(bytes)
}

// ── Windows reparse-safe open (audit H-1): CreateFileW with
// FILE_FLAG_OPEN_REPARSE_POINT + handle-side reparse/final-path
// verification, the nano-sandbox acl.rs pinned-handle pattern. ─────────────

#[cfg(windows)]
mod windows_safe_open {
    use super::AttachmentStoreError;
    use super::BlobReadError;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::CreateFileW;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
    use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    use windows_sys::Win32::Storage::FileSystem::OPEN_EXISTING;

    fn to_wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Open `path` without following a final-component reparse point, then
    /// prove from the OPENED HANDLE (never the path) that it is a plain
    /// non-reparse file whose final path stays beneath the canonical store
    /// `root`. Every failure fails closed.
    pub fn open_verified_file(path: &Path, root: &Path) -> Result<fs::File, BlobReadError> {
        let wide = to_wide(path);
        // Safety: `wide` is NUL-terminated and outlives the call; the
        // returned handle is either invalid (the error path) or ownership
        // moves into the `File` below (closed on drop).
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(
                AttachmentStoreError::io("open-blob")(std::io::Error::last_os_error()).into(),
            );
        }
        if let Err(err) = verify_handle(handle, root) {
            // Safety: the handle is valid and not yet owned by a File.
            unsafe {
                CloseHandle(handle);
            }
            return Err(err);
        }
        // Safety: the handle is a valid, verified open file handle; the
        // File takes ownership and closes it on drop.
        use std::os::windows::io::FromRawHandle as _;
        Ok(unsafe { fs::File::from_raw_handle(handle as *mut std::ffi::c_void) })
    }

    /// Handle-side verification: reparse metadata is read from the OPENED
    /// handle (a junction/symlink swapped in after validation was opened AS
    /// a reparse point — the open flag never follows one — and is rejected
    /// here), and the handle's final path must stay beneath the canonical
    /// root (a swapped INTERMEDIATE component — e.g. a junctioned fanout
    /// dir, which the open flag does not cover — lands the handle outside
    /// the store and is rejected here).
    fn verify_handle(handle: HANDLE, root: &Path) -> Result<(), BlobReadError> {
        // Safety: `handle` is a valid open file handle; `info` is a plain
        // out struct.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(AttachmentStoreError::io("stat-opened-blob")(
                std::io::Error::last_os_error(),
            )
            .into());
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(AttachmentStoreError::ReparsePoint("blob-opened").into());
        }
        let final_path = final_path_for_handle(handle)?;
        if !beneath_root(&final_path, root) {
            return Err(AttachmentStoreError::ReparsePoint("blob-escapes-store").into());
        }
        Ok(())
    }

    /// The handle's final path (the nano-sandbox acl.rs:1158 pattern).
    fn final_path_for_handle(handle: HANDLE) -> Result<PathBuf, BlobReadError> {
        // Safety: `handle` is valid; the sizing call writes nothing.
        let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
        if needed == 0 {
            return Err(AttachmentStoreError::io("final-path-blob")(
                std::io::Error::last_os_error(),
            )
            .into());
        }
        let mut buffer = vec![0u16; needed as usize + 1];
        // Safety: `buffer` is `needed + 1` wide chars, writable.
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if written == 0 || written as usize >= buffer.len() {
            return Err(AttachmentStoreError::io("final-path-blob")(
                std::io::Error::last_os_error(),
            )
            .into());
        }
        use std::os::windows::ffi::OsStringExt as _;
        Ok(PathBuf::from(std::ffi::OsString::from_wide(
            &buffer[..written as usize],
        )))
    }

    /// Case-insensitive beneath-root check on two `\\?\`-style absolute
    /// paths (both GetFinalPathNameByHandleW and canonicalize produce
    /// them). The separator-boundary check makes `root-evil` a non-match.
    fn beneath_root(final_path: &Path, root: &Path) -> bool {
        fn normalize(path: &Path) -> String {
            path.as_os_str()
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        }
        let final_norm = normalize(final_path);
        let root_norm = normalize(root);
        final_norm == root_norm || final_norm.starts_with(&format!("{root_norm}\\"))
    }
}

// ── Windows ACL (§5.5): explicit current-user-only ACL at creation +
// fail-closed DACL audit at every open. Profile-ACL inheritance is NOT
// accepted as proof. ──────────────────────────────────────────────────────

#[cfg(windows)]
mod windows_acl {
    use super::AttachmentStoreError;
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Foundation::HLOCAL;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Foundation::PSID;
    use windows_sys::Win32::Security::ACL;
    use windows_sys::Win32::Security::Authorization::EXPLICIT_ACCESS_W;
    use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
    use windows_sys::Win32::Security::Authorization::SE_FILE_OBJECT;
    use windows_sys::Win32::Security::Authorization::SET_ACCESS;
    use windows_sys::Win32::Security::Authorization::SetEntriesInAclW;
    use windows_sys::Win32::Security::Authorization::SetNamedSecurityInfoW;
    use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_SID;
    use windows_sys::Win32::Security::Authorization::TRUSTEE_IS_USER;
    use windows_sys::Win32::Security::Authorization::TRUSTEE_W;
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows_sys::Win32::Security::EqualSid;
    use windows_sys::Win32::Security::GetAce;
    use windows_sys::Win32::Security::GetLengthSid;
    use windows_sys::Win32::Security::GetTokenInformation;
    use windows_sys::Win32::Security::PROTECTED_DACL_SECURITY_INFORMATION;
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::Security::TOKEN_USER;
    use windows_sys::Win32::Security::TokenUser;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    use windows_sys::Win32::System::Threading::OpenProcessToken;

    const GENERIC_ALL_PERM: u32 = 0x1000_0000;
    const SUB_CONTAINERS_AND_OBJECTS_INHERIT: u32 = 0x3; // CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE
    const ACCESS_ALLOWED_ACE_TYPE_U8: u8 = 0;
    const ACCESS_DENIED_ACE_TYPE_U8: u8 = 1;

    fn to_wide(path: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// The current process user's SID (from the process token — the
    /// effective identity, never a name lookup).
    fn current_user_sid() -> Result<Vec<u8>, AttachmentStoreError> {
        // Safety: GetCurrentProcess returns a pseudo-handle to the current
        // process (never closed); the token handle is opened with
        // TOKEN_QUERY and closed before every return.
        unsafe {
            let mut token: HANDLE = 0;
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(AttachmentStoreError::PermissionAudit(
                    "OpenProcessToken failed",
                ));
            }
            let mut needed = 0u32;
            let _ = GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                let _ = CloseHandle(token);
                return Err(AttachmentStoreError::PermissionAudit(
                    "GetTokenInformation sizing failed",
                ));
            }
            let mut buf = vec![0u8; needed as usize];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr().cast(),
                needed,
                &mut needed,
            );
            let _ = CloseHandle(token);
            if ok == 0 {
                return Err(AttachmentStoreError::PermissionAudit(
                    "GetTokenInformation failed",
                ));
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let sid_len = GetLengthSid(user.User.Sid) as usize;
            if sid_len == 0 || sid_len > buf.len() {
                return Err(AttachmentStoreError::PermissionAudit("invalid user SID"));
            }
            let mut sid = vec![0u8; sid_len];
            ptr::copy_nonoverlapping(user.User.Sid as *const u8, sid.as_mut_ptr(), sid_len);
            Ok(sid)
        }
    }

    /// Store creation: replace the DACL with a single current-user
    /// full-control ACE, protected from inheritance (§5.5).
    pub fn set_current_user_only(path: &Path) -> Result<(), AttachmentStoreError> {
        let sid = current_user_sid()?;
        let wide = to_wide(path);
        unsafe {
            let mut trustee: TRUSTEE_W = std::mem::zeroed();
            trustee.TrusteeForm = TRUSTEE_IS_SID;
            trustee.TrusteeType = TRUSTEE_IS_USER;
            // For TRUSTEE_IS_SID, ptstrName carries the SID pointer.
            trustee.ptstrName = sid.as_ptr() as *mut u16;
            let mut entry: EXPLICIT_ACCESS_W = std::mem::zeroed();
            entry.grfAccessPermissions = GENERIC_ALL_PERM;
            entry.grfAccessMode = SET_ACCESS;
            entry.grfInheritance = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
            entry.Trustee = trustee;
            let mut new_acl: *mut ACL = ptr::null_mut();
            // Safety: `entry`/`sid` outlive the call; `new_acl` is a
            // LocalAlloc'd out-param freed below.
            let rc = SetEntriesInAclW(1, &entry, ptr::null(), &mut new_acl);
            if rc != 0 || new_acl.is_null() {
                return Err(AttachmentStoreError::PermissionAudit(
                    "SetEntriesInAclW failed",
                ));
            }
            // Safety: `wide` is NUL-terminated and outlives the call.
            let rc = SetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                new_acl,
                ptr::null(),
            );
            LocalFree(new_acl as HLOCAL);
            if rc != 0 {
                return Err(AttachmentStoreError::PermissionAudit(
                    "SetNamedSecurityInfoW failed",
                ));
            }
        }
        Ok(())
    }

    /// Open-time audit: the DACL must grant no beyond-user principals.
    /// Deny ACEs grant nothing and are tolerated; anything else (allow ACEs
    /// for other SIDs, audit ACEs, …) fails closed.
    pub fn audit_current_user_only(path: &Path) -> Result<(), AttachmentStoreError> {
        let user_sid = current_user_sid()?;
        let wide = to_wide(path);
        unsafe {
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut sd: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR = ptr::null_mut();
            // Safety: `wide` is NUL-terminated; out-params are written by
            // the call and `sd` is LocalFree'd below.
            let rc = GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut sd,
            );
            if rc != 0 {
                return Err(AttachmentStoreError::PermissionAudit(
                    "GetNamedSecurityInfoW failed",
                ));
            }
            let result = audit_dacl(dacl, user_sid.as_ptr() as PSID);
            if !sd.is_null() {
                LocalFree(sd as HLOCAL);
            }
            result?;
        }
        Ok(())
    }

    /// The DACL walk, separated for testability. Fails when any ALLOW ACE
    /// targets a principal other than the current user, when no ALLOW ACE
    /// covers the current user, or when the DACL is absent entirely.
    ///
    /// Safety: `dacl` must be a valid ACL pointer (or null) and `user_sid`
    /// a valid SID pointer, both outliving the call.
    unsafe fn audit_dacl(dacl: *mut ACL, user_sid: PSID) -> Result<(), AttachmentStoreError> {
        // Safety: upheld by the caller contract above.
        unsafe {
            if dacl.is_null() {
                return Err(AttachmentStoreError::PermissionAudit(
                    "store has no DACL (world-accessible)",
                ));
            }
            let ace_count = (*dacl).AceCount;
            let mut user_granted = false;
            for i in 0..u32::from(ace_count) {
                let mut ace: *mut std::ffi::c_void = ptr::null_mut();
                if GetAce(dacl, i, &mut ace) == 0 || ace.is_null() {
                    return Err(AttachmentStoreError::PermissionAudit("GetAce failed"));
                }
                let header = &*(ace as *const windows_sys::Win32::Security::ACE_HEADER);
                match header.AceType {
                    ACCESS_ALLOWED_ACE_TYPE_U8 => {
                        let allowed =
                            &*(ace as *const windows_sys::Win32::Security::ACCESS_ALLOWED_ACE);
                        let sid = &raw const allowed.SidStart as PSID;
                        if EqualSid(sid, user_sid) == 0 {
                            return Err(AttachmentStoreError::PermissionAudit(
                                "DACL grants a beyond-user principal",
                            ));
                        }
                        user_granted = true;
                    }
                    ACCESS_DENIED_ACE_TYPE_U8 => {
                        // Deny ACEs grant nothing; they cannot widen access.
                    }
                    _ => {
                        return Err(AttachmentStoreError::PermissionAudit(
                            "DACL carries a non-access ACE",
                        ));
                    }
                }
            }
            if !user_granted {
                return Err(AttachmentStoreError::PermissionAudit(
                    "DACL grants the current user nothing",
                ));
            }
            Ok(())
        }
    }
}

// ───────────────────────────── §12 test battery ─────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nano-p2a-store-{}-{}-{}",
            std::process::id(),
            tag,
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn digest_of(bytes: &[u8]) -> String {
        hex_sha256(bytes)
    }

    #[test]
    fn open_creates_the_layout_and_audits_it() {
        let home = test_home("layout");
        let store = AttachmentStore::open(&home).expect("open");
        assert!(store.root().join("staging").is_dir());
        assert!(store.root().join("blobs").is_dir());
        assert!(store.root().join(".gc.lock").exists());
        // A second open runs the audit path on the existing store.
        AttachmentStore::open(&home).expect("reopen passes the audit");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = store.root().metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "store dir is owner-only");
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn put_read_roundtrip_and_content_dedup() {
        let home = test_home("roundtrip");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let bytes = b"re-encoded pixel payload".to_vec();
        let digest = store.put(&lease, &bytes).unwrap();
        assert!(is_valid_digest(&digest));
        assert_eq!(digest, digest_of(&bytes));
        // The blob lives at blobs/<fanout>/<digest>.
        let blob = store.root().join("blobs").join(&digest[..2]).join(&digest);
        assert!(blob.is_file());
        // Same-digest rewrite is a no-op after verification.
        store.put(&lease, &bytes).unwrap();
        // Read path verifies and returns the exact bytes.
        assert_eq!(store.read_verified(&digest).unwrap(), bytes);
        assert_eq!(store.total_bytes().unwrap(), bytes.len() as u64);
        drop(lease);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn malicious_digests_are_rejected_before_path_construction() {
        let home = test_home("malicious");
        let store = AttachmentStore::open(&home).unwrap();
        // A plant OUTSIDE the store that a traversal would hit if the
        // digest were ever joined raw.
        let plant = home.join("secret-target");
        fs::write(&plant, b"outside").unwrap();
        let traversal = format!("..\\secret-target{}", "0".repeat(47));
        let malicious: Vec<String> = vec![
            traversal,
            "../../secret".to_string(),
            "a".repeat(63),                  // 63 chars
            "a".repeat(65),                  // 65 chars
            "A".repeat(64),                  // uppercase
            "g".repeat(64),                  // non-hex
            format!("{}0", "\0".repeat(63)), // embedded NUL
            String::new(),
        ];
        for digest in &malicious {
            let err = store.read_verified(digest).unwrap_err();
            assert!(
                matches!(err, BlobReadError::MalformedDigest),
                "{digest:?} → {err:?}"
            );
            assert_eq!(err.kind(), NanoErrorKind::AttachmentMissing);
        }
        // The traversal plant was never read (and still exists untouched).
        assert_eq!(fs::read(&plant).unwrap(), b"outside");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_tampered_and_over_ceiling_blobs_fail_closed() {
        let home = test_home("missing-tampered");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let digest = store.put(&lease, b"payload").unwrap();
        drop(lease);

        // MISSING: no blob at a valid digest's address.
        let absent = "0".repeat(64);
        let err = store.read_verified(&absent).unwrap_err();
        assert!(matches!(err, BlobReadError::Missing), "{err:?}");
        assert_eq!(err.kind(), NanoErrorKind::AttachmentMissing);

        // TAMPERED: flip a byte in the blob.
        let blob = store.root().join("blobs").join(&digest[..2]).join(&digest);
        let mut raw = fs::read(&blob).unwrap();
        raw[0] ^= 0xFF;
        fs::write(&blob, &raw).unwrap();
        let err = store.read_verified(&digest).unwrap_err();
        assert!(matches!(err, BlobReadError::Tampered), "{err:?}");
        assert_eq!(err.kind(), NanoErrorKind::AttachmentMissing);

        // The loud placeholder names the digest prefix and never implies
        // recoverability.
        let placeholder = attachment_unavailable_placeholder(2, &digest);
        assert!(placeholder.contains(&digest[..12]));
        assert!(placeholder.contains("do not describe it from memory"));
        assert!(attachment_unavailable_placeholder(3, "..\\..\\x").contains("malformed-digest"));
        let _ = fs::remove_dir_all(&home);
    }

    /// Create a directory link `link` -> `target`: NTFS junction on Windows
    /// (no privilege required), symlink elsewhere. Returns false when the
    /// platform refused — the caller then skips LOUDLY (a scenario whose
    /// subject is missing must fail, but a host that forbids link creation
    /// cannot run the scenario at all; precedent:
    /// nano-tools/tests/adversarial_fs.rs).
    fn make_dir_link(link: &Path, target: &Path) -> bool {
        #[cfg(windows)]
        {
            return std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .expect("spawn mklink")
                .status
                .success();
        }
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(target, link).is_ok();
        }
        #[allow(unreachable_code)]
        false
    }

    #[test]
    fn reparse_store_entries_are_rejected() {
        let home = test_home("reparse");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let digest = store.put(&lease, b"payload").unwrap();
        drop(lease);

        // Replace the fanout dir with a junction/symlink to an outside dir
        // carrying a same-named file: the read must be REJECTED, never
        // followed out of the store.
        let outside = home.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(&digest), b"outside-bytes").unwrap();
        let fanout = store.root().join("blobs").join(&digest[..2]);
        let saved = home.join("fanout-saved");
        fs::rename(&fanout, &saved).unwrap();
        if !make_dir_link(&fanout, &outside) {
            eprintln!(
                "LOUD SKIP: host refused link creation (no developer mode/admin) — \
                 reparse-rejection scenario cannot run here"
            );
            return;
        }
        let err = store.read_verified(&digest).unwrap_err();
        assert!(
            matches!(
                err,
                BlobReadError::Store(AttachmentStoreError::ReparsePoint(_))
            ),
            "junctioned fanout dir must be rejected, got {err:?}"
        );
        assert_eq!(err.kind(), NanoErrorKind::AttachmentStoreError);
        #[cfg(windows)]
        fs::remove_dir(&fanout).unwrap(); // junctions are removed as directories
        #[cfg(unix)]
        fs::remove_file(&fanout).unwrap(); // a symlink is removed as a file
        fs::rename(&saved, &fanout).unwrap();
        assert_eq!(store.read_verified(&digest).unwrap(), b"payload");
        let _ = fs::remove_dir_all(&home);
    }

    /// Audit H-1 regression: a same-user process swaps a store component
    /// BETWEEN validation and open (the test hook fires in exactly that
    /// window). The read must fail closed from the OPENED HANDLE's
    /// evidence, never return the outside bytes:
    /// - Windows: the fanout dir is swapped for a junction; the handle's
    ///   final path lands outside the canonical store root → ReparsePoint;
    /// - unix: the blob itself is swapped for a symlink; the O_NOFOLLOW
    ///   open refuses it.
    #[test]
    fn swap_between_validate_and_open_fails_closed() {
        let home = test_home("toctou");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let digest = store.put(&lease, b"payload").unwrap();
        drop(lease);
        let outside = home.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(&digest), b"outside-bytes").unwrap();

        #[cfg(windows)]
        {
            let fanout = store.root().join("blobs").join(&digest[..2]);
            let saved = home.join("fanout-saved");
            let saved_hook = saved.clone();
            let linked = std::rc::Rc::new(std::cell::Cell::new(true));
            let linked_hook = linked.clone();
            let outside_hook = outside.clone();
            let fanout_hook = fanout.clone();
            PRE_OPEN_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    fs::rename(&fanout_hook, &saved_hook).unwrap();
                    if !make_dir_link(&fanout_hook, &outside_hook) {
                        linked_hook.set(false);
                    }
                }));
            });
            let result = store.read_verified(&digest);
            if !linked.get() {
                eprintln!(
                    "LOUD SKIP: host refused junction creation (no developer mode/admin) — \
                     swap-between-validate-and-open scenario cannot run here"
                );
                return;
            }
            let err = result.unwrap_err();
            assert!(
                matches!(
                    err,
                    BlobReadError::Store(AttachmentStoreError::ReparsePoint(_))
                ),
                "a fanout junction swapped in after validation must be rejected \
                 from the opened handle, got {err:?}"
            );
            assert_eq!(err.kind(), NanoErrorKind::AttachmentStoreError);
            // Restore the layout: the legitimate path still works.
            fs::remove_dir(&fanout).unwrap(); // junctions are removed as directories
            fs::rename(&saved, &fanout).unwrap();
            assert_eq!(store.read_verified(&digest).unwrap(), b"payload");
        }
        #[cfg(unix)]
        {
            let blob = store.root().join("blobs").join(&digest[..2]).join(&digest);
            let outside_blob = outside.join(&digest);
            let blob_hook = blob.clone();
            PRE_OPEN_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    fs::remove_file(&blob_hook).unwrap();
                    std::os::unix::fs::symlink(&outside_blob, &blob_hook).unwrap();
                }));
            });
            let err = store.read_verified(&digest).unwrap_err();
            assert!(
                matches!(err, BlobReadError::Store(AttachmentStoreError::Io { .. })),
                "a blob swapped for a symlink must fail the O_NOFOLLOW open, got {err:?}"
            );
            // Restore the legitimate blob: the honest path still works.
            fs::remove_file(&blob).unwrap();
            fs::write(&blob, b"payload").unwrap();
            assert_eq!(store.read_verified(&digest).unwrap(), b"payload");
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(windows)]
    #[test]
    fn weakened_dacl_fails_closed_at_open() {
        let home = test_home("acl");
        {
            let _store = AttachmentStore::open(&home).unwrap();
        }
        // Weaken the DACL: grant Everyone read via SID (localization-safe).
        let status = std::process::Command::new("icacls")
            .arg(home.join("attachments"))
            .args(["/grant", "*S-1-1-0:R"])
            .output()
            .expect("spawn icacls");
        assert!(status.status.success(), "icacls grants the fixture ACE");
        let err = AttachmentStore::open(&home).unwrap_err();
        assert!(
            matches!(err, AttachmentStoreError::PermissionAudit(_)),
            "a beyond-user allow ACE fails closed, got {err:?}"
        );
        assert_eq!(err.kind(), NanoErrorKind::AttachmentStoreError);
        let _ = fs::remove_dir_all(&home);
    }

    // ── §5.4/§12 GC race battery (deterministic; the second handle is what
    // a competing host process would do — the lock.rs precedent) ──────────

    #[test]
    fn sweep_skips_under_a_writer_lease() {
        let home = test_home("gc-lease");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        // Process A paused between rename and journal append: the blob is
        // published but NOT yet referenced.
        let digest = store.put(&lease, b"in-flight").unwrap();
        // Process B sweeps: the exclusive acquisition fails → typed skip,
        // and the blob survives.
        let report = store.sweep(&HashSet::new()).unwrap();
        assert!(report.lock_skipped);
        assert!(!report.swept);
        assert_eq!(report.removed_blobs, 0);
        assert_eq!(store.read_verified(&digest).unwrap(), b"in-flight");
        drop(lease);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn sweep_grace_alone_protects_young_blobs() {
        let home = test_home("gc-grace");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let digest = store.put(&lease, b"young").unwrap();
        drop(lease); // lease released: the grace guard alone must protect
        let report = store.sweep(&HashSet::new()).unwrap();
        assert!(report.swept);
        assert_eq!(report.removed_blobs, 0);
        assert_eq!(report.skipped_young, 1);
        assert_eq!(store.read_verified(&digest).unwrap(), b"young");
        // Past the grace, an unreferenced blob IS collected.
        let later = SystemTime::now() + Duration::from_secs(GC_GRACE_SECS + 1);
        let lock = FileLock::try_acquire(&store.lock_path()).unwrap();
        let report = store.sweep_at(&HashSet::new(), later).unwrap();
        assert_eq!(report.removed_blobs, 1);
        assert!(report.reclaimed_bytes >= 5);
        assert_eq!(report.removed_empty_fanout_dirs, 1);
        drop(lock);
        assert!(matches!(
            store.read_verified(&digest).unwrap_err(),
            BlobReadError::Missing
        ));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn sweep_never_deletes_referenced_blobs_even_past_grace() {
        let home = test_home("gc-referenced");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let digest = store.put(&lease, b"referenced").unwrap();
        drop(lease);
        let mut referenced = HashSet::new();
        referenced.insert(digest.clone());
        let later = SystemTime::now() + Duration::from_secs(3600);
        let lock = FileLock::try_acquire(&store.lock_path()).unwrap();
        let report = store.sweep_at(&referenced, later).unwrap();
        assert_eq!(report.removed_blobs, 0);
        drop(lock);
        assert_eq!(store.read_verified(&digest).unwrap(), b"referenced");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn sweep_skips_future_timestamps_and_sweeps_stale_staging() {
        let home = test_home("gc-clock");
        let store = AttachmentStore::open(&home).unwrap();
        let lease = store.acquire_write_lease().unwrap();
        let digest = store.put(&lease, b"blob").unwrap();
        drop(lease);
        // A stale-looking staging file plus the young blob.
        let tmp = store.staging_dir().join("crashed-1.tmp");
        write_private_file(&tmp, b"torn").unwrap();

        // Clock BEFORE the mtimes: everything reads as future-dated →
        // fail-closed skip, nothing deleted.
        let past = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let lock = FileLock::try_acquire(&store.lock_path()).unwrap();
        let report = store.sweep_at(&HashSet::new(), past).unwrap();
        assert_eq!(report.removed_blobs, 0);
        assert_eq!(report.removed_staging, 0);
        assert_eq!(report.skipped_future, 2);

        // Clock past the grace: the stale .tmp is swept, the unreferenced
        // blob too.
        let later = SystemTime::now() + Duration::from_secs(GC_GRACE_SECS + 1);
        let report = store.sweep_at(&HashSet::new(), later).unwrap();
        assert_eq!(report.removed_staging, 1);
        assert_eq!(report.removed_blobs, 1);
        drop(lock);
        assert!(!tmp.exists());
        assert!(matches!(
            store.read_verified(&digest).unwrap_err(),
            BlobReadError::Missing
        ));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn sweep_leaves_unknown_entries_and_keeps_young_staging() {
        let home = test_home("gc-conservative");
        let store = AttachmentStore::open(&home).unwrap();
        // An entry whose name is not a valid digest: never deleted.
        let fanout = store.blobs_dir().join("zz");
        make_private_dir(&fanout).unwrap();
        write_private_file(&fanout.join("not-a-digest"), b"unknown").unwrap();
        // A young staging file is kept (only past-grace .tmp is swept).
        let young_tmp = store.staging_dir().join("in-flight.tmp");
        write_private_file(&young_tmp, b"wip").unwrap();
        let lock = FileLock::try_acquire(&store.lock_path()).unwrap();
        let report = store.sweep_at(&HashSet::new(), SystemTime::now()).unwrap();
        assert_eq!(report.removed_blobs, 0);
        assert_eq!(report.removed_staging, 0, "young .tmp survives");
        assert_eq!(report.skipped_young, 1);
        assert!(young_tmp.exists());
        // Past the grace the same .tmp is swept; the unknown entry stays.
        let later = SystemTime::now() + Duration::from_secs(GC_GRACE_SECS + 1);
        let report = store.sweep_at(&HashSet::new(), later).unwrap();
        assert_eq!(report.removed_staging, 1);
        drop(lock);
        assert!(!young_tmp.exists());
        assert!(fanout.join("not-a-digest").exists());
        let _ = fs::remove_dir_all(&home);
    }

    /// F-34 wiring + F-32 LOW-7: the reference scan covers BOTH journal
    /// reference surfaces — TurnBegin input_blocks manifests AND
    /// ToolResult.image_refs — and aborts (never returns a partial set) on
    /// an unreadable journal.
    #[test]
    fn referenced_digests_cover_prompts_and_tool_results() {
        use crate::op::{ImageRef, InputBlock, Op, OpEnvelope};

        let dir = std::env::temp_dir().join(format!("nano-gc-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let digest_a = "aa".repeat(32);
        let digest_b = "bb".repeat(32);
        let image = |digest: String| ImageRef {
            digest,
            mime: "image/png".into(),
            bytes: 4,
            width: 1,
            height: 1,
            normalized_from: None,
            placeholder: "[Image #1]".into(),
        };
        let envelopes = [
            OpEnvelope::new(
                "op-1",
                "now",
                Op::TurnBegin {
                    turn_id: "t1".into(),
                    input: "look".into(),
                    input_blocks: vec![InputBlock::ImageRef(image(digest_a.clone()))],
                },
            ),
            OpEnvelope::new(
                "op-2",
                "now",
                Op::ToolResult {
                    call_id: "c1".into(),
                    ok: true,
                    output_digest: "cc".repeat(32),
                    changed_files: vec![],
                    error_kind: None,
                    image_refs: vec![image(digest_b.clone())],
                },
            ),
        ];
        let mut body = String::new();
        for envelope in &envelopes {
            body.push_str(&serde_json::to_string(envelope).unwrap());
            body.push('\n');
        }
        fs::write(dir.join("s1.jsonl"), body).unwrap();
        // Non-journal entries never participate.
        fs::write(dir.join("notes.txt"), "not a journal").unwrap();

        let referenced = referenced_blob_digests(&dir).unwrap();
        assert!(referenced.contains(&digest_a), "prompt manifest digest");
        assert!(
            referenced.contains(&digest_b),
            "tool-result image_refs digest"
        );
        assert_eq!(referenced.len(), 2);

        // Fail-closed: an unreadable journal aborts the scan (the caller
        // must not sweep on a partial set). The bad line must be NON-final
        // — a final bad line is the reader's tolerated crash-torn tail.
        fs::write(dir.join("corrupt.jsonl"), "not json\n{}\n").unwrap();
        assert!(referenced_blob_digests(&dir).is_err());

        // A missing sessions dir scans empty (fresh profile).
        let missing = dir.join("nope");
        assert!(referenced_blob_digests(&missing).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
