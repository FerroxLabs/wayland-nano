//! Journal writer: append-only, fsync-bounded, id-unique.

use crate::op::OpEnvelope;
use std::collections::HashSet;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub struct JournalWriter {
    file: File,
    path: PathBuf,
    /// Ids already present (loaded at open + appended since), so a retried
    /// write after an uncertain crash cannot double-append.
    seen_ids: HashSet<String>,
    /// Whether each append is followed by fsync. The safe default is true;
    /// tests may relax it to simulate crash windows.
    sync_each_append: bool,
}

impl JournalWriter {
    /// Opens (creating if needed) the journal at `path`, scanning existing ids
    /// for idempotence. Existing content is never rewritten; the single
    /// exception is a crash-torn tail, which is truncated at open so a
    /// retried append starts on a fresh line instead of gluing onto torn
    /// bytes (the retry's trailing newline would demote the glued garbage to
    /// a malformed MIDDLE line and brick every later restore). An
    /// integrity-broken middle is never truncated — restore stays fail-loud.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = std::fs::read(path).unwrap_or_default();
        let report = crate::reader::parse_journal_bytes(&bytes);
        if let Ok(report) = &report
            && report.torn_tail_at.is_some()
        {
            truncate_torn_tail(path, &bytes)?;
        }
        let seen_ids = report
            .map(|report| report.envelopes.into_iter().map(|e| e.id).collect())
            .unwrap_or_default();
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            seen_ids,
            sync_each_append: true,
        })
    }

    pub fn with_sync_each_append(mut self, sync: bool) -> Self {
        self.sync_each_append = sync;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one envelope. Returns `Ok(false)` when the id is already
    /// present (idempotent no-op) so callers can distinguish a retry from a
    /// new write.
    pub fn append(&mut self, envelope: &OpEnvelope) -> io::Result<bool> {
        if !self.seen_ids.insert(envelope.id.clone()) {
            return Ok(false);
        }
        let mut line = serde_json::to_vec(envelope)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        if self.sync_each_append {
            self.file.sync_data()?;
        }
        Ok(true)
    }

    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }
}

/// Removes a crash-torn final line. `bytes` is the content the reader just
/// classified as carrying a torn tail, i.e. the newline-free final line is
/// unrecoverable garbage and every line before it parsed. Truncation keeps
/// every complete line byte-for-byte and removes exactly that final line —
/// never a valid record.
fn truncate_torn_tail(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // Line boundaries are byte-identical between the raw file and the
    // reader's lossy view ('\n' is a single byte and cannot occur inside a
    // multi-byte UTF-8 sequence), so the raw-space torn tail starts right
    // after the last '\n' (or at 0 when the file holds only the torn line).
    let keep = bytes
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |index| index + 1);
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(keep as u64)
}
