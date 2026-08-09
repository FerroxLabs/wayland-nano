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
    /// for idempotence. Existing content is never truncated or rewritten.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let seen_ids = crate::reader::read_journal(path)
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
