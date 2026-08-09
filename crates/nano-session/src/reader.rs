//! Journal reader: tolerant tail, strict middle.
//!
//! Rules:
//! - valid lines parse to envelopes in order;
//! - a final line that fails to parse is treated as a crash-torn tail and
//!   dropped (reported, not fatal);
//! - a parse failure on any non-final line is an integrity error — the
//!   journal's middle must be authoritative or the whole restore fails loudly.

use crate::op::OpEnvelope;
use std::io;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct JournalReport {
    pub envelopes: Vec<OpEnvelope>,
    /// Byte offset of the torn tail, if one was dropped.
    pub torn_tail_at: Option<u64>,
    pub path: Option<PathBuf>,
}

pub fn read_journal(path: &Path) -> io::Result<JournalReport> {
    if !path.exists() {
        return Ok(JournalReport::default());
    }
    let bytes = std::fs::read(path)?;
    parse_journal_bytes(&bytes).map(|mut report| {
        report.path = Some(path.to_path_buf());
        report
    })
}

pub fn parse_journal_bytes(bytes: &[u8]) -> io::Result<JournalReport> {
    let mut report = JournalReport::default();
    let mut offset: u64 = 0;
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.split('\n').collect();

    for (index, line) in lines.iter().enumerate() {
        let line_len = line.len() as u64 + 1; // account for the '\n'
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let is_last_line = index == lines.len() - 1;

        if trimmed.is_empty() {
            offset += line_len;
            continue;
        }

        match serde_json::from_str::<OpEnvelope>(trimmed) {
            Ok(envelope) => report.envelopes.push(envelope),
            Err(err) if is_last_line => {
                // Crash-torn tail: drop it, keep everything before.
                report.torn_tail_at = Some(offset);
                let _ = err;
            }
            Err(err) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("journal integrity error at byte {offset}: {err}"),
                ));
            }
        }
        offset += line_len;
    }

    Ok(report)
}
