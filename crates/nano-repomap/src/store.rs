//! In-memory per-session store, keyed by canonical path (P4 design §5.2).
//!
//! `BTreeMap<PathKey, FileEntry>` where `PathKey` =
//! `nano_sandbox::canonical_path_key` (dunce-canonicalize → `/`
//! separators → lowercase) — case-insensitivity, separator, 8.3
//! short-name and drive-letter normalization collapse to ONE key form
//! (§14 leg 4's Windows proof rests on this).
//!
//! Freshness (§5.2, r2 codex-F9 — the mtime+len shortcut is unsound):
//! every [`FileEntry`] carries `mtime` + `size_bytes` + `content_hash`
//! (sha2). Each EXPLICIT refresh re-stats every tracked file; candidates
//! (mtime OR len changed) are re-hashed, and once per
//! `full_rehash_interval` (default 30s) ALL tracked files are re-hashed,
//! so a preserved-mtime/same-length edit is caught within one
//! full-rehash cadence. Re-extraction happens ONLY on hash change.
//! Entries without a content hash (oversize/overline/unreadable) are
//! exempt from the full pass: they carry no symbols to go stale and the
//! per-candidate read bound (≤ `max_file_bytes`) stays honest.
//!
//! Rename vs duplicate (§5.2): a new path whose hash matches an existing
//! entry is a RENAME only when the old canonical path no longer exists
//! (disappearance proves the move — the entry is re-keyed, symbols kept);
//! equal-hash with the old path still present ⇒ DUPLICATE — the new path
//! is extracted and indexed independently, no symbols are "moved" (hash
//! equality cannot distinguish copy from rename).
//!
//! No persistence, no filesystem writes (§5.4 — v1 discharges the
//! write-classification requirement structurally); wrong-shape/corrupt
//! state is impossible by construction.

use std::collections::{BTreeMap, HashSet};
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use nano_sandbox::canonical_path_key;
use sha2::Digest as _;

use crate::extractor::{extract, first_meaningful};
use crate::policy::ReadPolicy;
use crate::types::{IndexOptions, Language, RepoMapError, Symbol};
use crate::walk::walk;

/// Why an entry was recorded with empty symbols (design §5.1: oversize/
/// overline/per-entry-IO are RECORDED, never fatal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Larger than `max_file_bytes` — never read (metadata carries size).
    Oversize,
    /// More than `max_lines` lines — the bounded streaming read STOPPED
    /// at line `max_lines + 1`.
    Overline,
    /// Open/read/UTF-8 failure (binary files land here too).
    Unreadable,
}

/// One indexed (or recorded-but-skipped) file.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Canonical absolute path (display form).
    pub path: PathBuf,
    pub language: Language,
    /// Lines seen: the true count when fully read, or `max_lines + 1`
    /// when the streaming read stopped early.
    pub lines: usize,
    pub size_bytes: u64,
    pub mtime: Option<SystemTime>,
    /// sha2-256 of the full content. `None` for skipped entries (their
    /// content was never fully read).
    pub content_hash: Option<[u8; 32]>,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<String>,
    /// `Language::Other` only: first non-blank, non-comment line
    /// (200-byte cap).
    pub first_meaningful_line: Option<String>,
    /// `Some` when the file was recorded with empty symbols.
    pub skip: Option<SkipReason>,
}

/// Honest staleness reporting carried by every query result (§5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexStats {
    pub files: usize,
    pub symbols: usize,
    /// Age of the last refresh pass; `None` = never refreshed (a lazy
    /// store before its first query says so).
    pub last_refresh_age_ms: Option<u64>,
    /// Files re-extracted (or newly indexed / re-keyed) by the last pass.
    pub refreshed_files: usize,
    /// Cumulative count of policy-denied paths skipped by the walker.
    /// Counted, never enumerated (§5.3).
    pub skipped_denied: u64,
}

/// The per-session store. NOT `Sync`-internally — hosts wrap it (the
/// nano-tools wrapper uses a `Mutex`) so query-triggered refresh can
/// mutate through a shared reference.
pub struct RepoMap {
    root: PathBuf,
    options: IndexOptions,
    policy: ReadPolicy,
    entries: BTreeMap<String, FileEntry>,
    last_refresh: Option<Instant>,
    last_full_hash: Option<Instant>,
    skipped_denied: u64,
    refreshed_files: usize,
}

/// Outcome of the bounded streaming read of one file.
enum ScannedFile {
    /// Within caps: full content, its hash, and the true line count.
    Content {
        hash: [u8; 32],
        lines: usize,
        text: String,
    },
    Skipped {
        reason: SkipReason,
        lines_seen: usize,
    },
}

impl RepoMap {
    /// Construct a LAZY store (no IO beyond canonicalizing the root).
    /// The first refresh — explicit or query-triggered — performs the
    /// initial index; until then `stats().last_refresh_age_ms` is `None`.
    pub fn new(
        root: &Path,
        options: IndexOptions,
        policy: ReadPolicy,
    ) -> Result<Self, RepoMapError> {
        let root = dunce::canonicalize(root).map_err(|source| RepoMapError::Root {
            path: root.to_path_buf(),
            source,
        })?;
        Ok(Self {
            root,
            options,
            policy,
            entries: BTreeMap::new(),
            last_refresh: None,
            last_full_hash: None,
            skipped_denied: 0,
            refreshed_files: 0,
        })
    }

    /// Construct and index immediately.
    pub fn build(
        root: &Path,
        options: IndexOptions,
        policy: ReadPolicy,
    ) -> Result<Self, RepoMapError> {
        let mut map = Self::new(root, options, policy)?;
        map.refresh();
        Ok(map)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read-only view of one entry by any spelling of its path
    /// (canonical-key collapse makes 8.3/case/separator variants one
    /// lookup — §14 leg 4).
    pub fn entry(&self, path: &Path) -> Option<&FileEntry> {
        self.entries.get(&canonical_path_key(path))
    }

    /// Crate-internal iteration for the query surface (§5.5): key +
    /// entry, in canonical-key order.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&String, &FileEntry)> {
        self.entries.iter()
    }

    /// Current stats. Cheap; never triggers IO.
    pub fn stats(&self) -> IndexStats {
        IndexStats {
            files: self.entries.len(),
            symbols: self.entries.values().map(|e| e.symbols.len()).sum(),
            last_refresh_age_ms: self
                .last_refresh
                .map(|t| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)),
            refreshed_files: self.refreshed_files,
            skipped_denied: self.skipped_denied,
        }
    }

    /// Query-triggered refresh (§5.2): runs a pass when the store has
    /// never been refreshed or the last pass is older than
    /// `refresh_throttle`. Returns whether a pass ran.
    pub fn maybe_refresh(&mut self) -> bool {
        let due = self
            .last_refresh
            .is_none_or(|t| t.elapsed() >= self.options.refresh_throttle);
        if due {
            self.refresh();
        }
        due
    }

    /// One explicit refresh pass: re-walk, re-stat everything, re-hash
    /// candidates (plus ALL hashed entries once per
    /// `full_rehash_interval`), re-extract only on hash change.
    pub fn refresh(&mut self) {
        let now = Instant::now();
        let walked = walk(&self.root, &self.policy, self.options.respect_gitignore);
        self.skipped_denied = self.skipped_denied.saturating_add(walked.skipped_denied);
        let full_hash = self
            .last_full_hash
            .is_none_or(|t| t.elapsed() >= self.options.full_rehash_interval);

        let mut refreshed = 0usize;
        let mut seen_keys: HashSet<String> = HashSet::with_capacity(walked.files.len());

        for path in &walked.files {
            let key = canonical_path_key(path);
            seen_keys.insert(key.clone());
            let meta = std::fs::metadata(path).ok();
            let mtime = meta.as_ref().and_then(|m| m.modified().ok());
            let len = meta.as_ref().map_or(0, |m| m.len());

            if let Some(existing) = self.entries.get(&key) {
                let stat_changed = existing.mtime != mtime || existing.size_bytes != len;
                let hash_due = full_hash && existing.content_hash.is_some();
                if !stat_changed && !hash_due {
                    continue; // untouched and not due a hash pass
                }
                let scanned = scan_file(path, len, &self.options);
                let changed = match (&scanned, &existing.content_hash) {
                    (ScannedFile::Content { hash, .. }, Some(old)) => *hash != *old,
                    // Stat-triggered re-scan of a skipped (hash-less)
                    // entry: count it refreshed only if its recorded
                    // state actually changed (handled below by compare).
                    _ => true,
                };
                if !changed {
                    // Content identical despite stat change (touch /
                    // checkout with preserved content): adopt new stat.
                    let entry = self.entries.get_mut(&key).expect("key checked above");
                    entry.mtime = mtime;
                    entry.size_bytes = len;
                    continue;
                }
                let language = Language::from_path(path);
                let new_entry = entry_from_scan(path.clone(), language, scanned, mtime, len, None);
                let entry = self.entries.get_mut(&key).expect("key checked above");
                if *entry != new_entry {
                    *entry = new_entry;
                    refreshed += 1;
                } else {
                    entry.mtime = mtime;
                    entry.size_bytes = len;
                }
                continue;
            }

            // New path. §5.2 rename vs duplicate: an equal-hash entry
            // whose canonical path has DISAPPEARED is a rename — re-key
            // it (symbols kept; extraction is content-pure). Equal-hash
            // with the old path still on disk is a duplicate — extract
            // and index independently, nothing is moved.
            let scanned = scan_file(path, len, &self.options);
            let language = Language::from_path(path);
            let rename_from = match &scanned {
                ScannedFile::Content { hash, .. } => self
                    .find_rename_source(hash, &seen_keys)
                    // A rename that also changed the EXTENSION changes the
                    // language, and extraction is language-dependent — only
                    // reuse symbols when the language is unchanged,
                    // otherwise extract fresh.
                    .filter(|old_key| {
                        self.entries
                            .get(old_key)
                            .is_some_and(|old| old.language == language)
                    }),
                ScannedFile::Skipped { .. } => None,
            };
            let entry = match rename_from {
                Some(old_key) => {
                    let old = self
                        .entries
                        .remove(&old_key)
                        .expect("rename source present");
                    entry_from_scan(path.clone(), language, scanned, mtime, len, Some(&old))
                }
                None => entry_from_scan(path.clone(), language, scanned, mtime, len, None),
            };
            self.entries.insert(key, entry);
            refreshed += 1;
        }

        // Deletions (and rename sources already removed above).
        self.entries.retain(|k, _| seen_keys.contains(k));

        self.last_refresh = Some(now);
        if full_hash {
            self.last_full_hash = Some(now);
        }
        self.refreshed_files = refreshed;
    }

    /// Find an entry that proves a rename: same content hash, its path
    /// NOT in the current walk, and its path gone from disk
    /// (disappearance proves the move, §5.2). Deterministic (BTreeMap
    /// order).
    fn find_rename_source(&self, hash: &[u8; 32], seen_keys: &HashSet<String>) -> Option<String> {
        self.entries
            .iter()
            .find(|(k, e)| {
                !seen_keys.contains(*k) && e.content_hash.as_ref() == Some(hash) && !e.path.exists()
            })
            .map(|(k, _)| k.clone())
    }
}

/// The bounded streaming read (§5.1 r2 codex-F9): lines are counted
/// WHILE reading and the read STOPS at line `max_lines + 1` — metadata
/// carries size, not line count, so the line cap cannot be a metadata
/// check. Files over `max_file_bytes` are never read at all.
fn scan_file(path: &Path, len: u64, options: &IndexOptions) -> ScannedFile {
    if len > options.max_file_bytes {
        return ScannedFile::Skipped {
            reason: SkipReason::Oversize,
            lines_seen: 0,
        };
    }
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            return ScannedFile::Skipped {
                reason: SkipReason::Unreadable,
                lines_seen: 0,
            };
        }
    };
    let mut reader = std::io::BufReader::new(file);
    let mut text = String::new();
    let mut lines = 0usize;
    loop {
        let mut buf = String::new();
        match reader.read_line(&mut buf) {
            Ok(0) => break, // EOF
            Ok(_) => {
                lines += 1;
                if lines > options.max_lines {
                    // Stops AT line max_lines + 1: the rest is never read.
                    return ScannedFile::Skipped {
                        reason: SkipReason::Overline,
                        lines_seen: lines,
                    };
                }
                text.push_str(&buf);
            }
            Err(_) => {
                // IO error or invalid UTF-8 mid-file (binary files land
                // here): recorded, never fatal.
                return ScannedFile::Skipped {
                    reason: SkipReason::Unreadable,
                    lines_seen: lines,
                };
            }
        }
    }
    let hash: [u8; 32] = sha2::Sha256::digest(text.as_bytes()).into();
    ScannedFile::Content { hash, lines, text }
}

/// Build the recorded entry for one scanned file. `rename_source` is the
/// disappeared equal-hash entry whose symbols/imports are reused
/// verbatim (same hash ⇒ identical extraction, §5.2).
fn entry_from_scan(
    path: PathBuf,
    language: Language,
    scanned: ScannedFile,
    mtime: Option<SystemTime>,
    len: u64,
    rename_source: Option<&FileEntry>,
) -> FileEntry {
    match scanned {
        ScannedFile::Content { hash, lines, text } => {
            let (symbols, imports, meaningful) = match rename_source {
                Some(old) => (
                    old.symbols.clone(),
                    old.imports.clone(),
                    old.first_meaningful_line.clone(),
                ),
                None => {
                    let (symbols, imports) = extract(language, &text);
                    let meaningful = match language {
                        Language::Other => first_meaningful(&text),
                        _ => None,
                    };
                    (symbols, imports, meaningful)
                }
            };
            FileEntry {
                path,
                language,
                lines,
                size_bytes: len,
                mtime,
                content_hash: Some(hash),
                symbols,
                imports,
                first_meaningful_line: meaningful,
                skip: None,
            }
        }
        ScannedFile::Skipped { reason, lines_seen } => FileEntry {
            path,
            language,
            lines: lines_seen,
            size_bytes: len,
            mtime,
            content_hash: None,
            symbols: Vec::new(),
            imports: Vec::new(),
            first_meaningful_line: None,
            skip: Some(reason),
        },
    }
}

impl PartialEq for FileEntry {
    /// Material equality for the refresh diff: everything except the
    /// stat pair (mtime/len are adopted separately).
    fn eq(&self, other: &Self) -> bool {
        self.language == other.language
            && self.lines == other.lines
            && self.content_hash == other.content_hash
            && self.symbols == other.symbols
            && self.imports == other.imports
            && self.first_meaningful_line == other.first_meaningful_line
            && self.skip == other.skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overline_read_stops_at_cap_plus_one() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("big.rs");
        let body = "fn f() {}\n".repeat(60_000);
        std::fs::write(&file, body).unwrap();
        let len = std::fs::metadata(&file).unwrap().len();
        let options = IndexOptions {
            max_lines: 50_000,
            ..Default::default()
        };
        let scanned = scan_file(&file, len, &options);
        match scanned {
            ScannedFile::Skipped { reason, lines_seen } => {
                assert_eq!(reason, SkipReason::Overline);
                assert_eq!(lines_seen, 50_001);
            }
            ScannedFile::Content { .. } => panic!("51k-line file must be recorded as overline"),
        }
    }

    #[test]
    fn oversize_never_read() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("huge.rs");
        std::fs::write(&file, "fn f() {}\n").unwrap();
        let len = std::fs::metadata(&file).unwrap().len();
        let options = IndexOptions {
            max_file_bytes: 4, // smaller than the file
            ..Default::default()
        };
        match scan_file(&file, len, &options) {
            ScannedFile::Skipped { reason, lines_seen } => {
                assert_eq!(reason, SkipReason::Oversize);
                assert_eq!(lines_seen, 0);
            }
            ScannedFile::Content { .. } => panic!("oversize file must never be read"),
        }
    }

    #[test]
    fn binary_file_recorded_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("blob.rs");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        let len = std::fs::metadata(&file).unwrap().len();
        match scan_file(&file, len, &IndexOptions::default()) {
            ScannedFile::Skipped { reason, .. } => assert_eq!(reason, SkipReason::Unreadable),
            ScannedFile::Content { .. } => panic!("non-UTF8 must be recorded, not indexed"),
        }
    }
}
