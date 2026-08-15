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
use std::io::Read;
use std::io::Seek;
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

/// The result of an incremental tail read: the envelopes appended since
/// `offset`, plus the absolute byte offset of the first unconsumed byte.
#[derive(Debug, Default)]
pub struct JournalTail {
    pub report: JournalReport,
    /// Where the NEXT tail read must start: end of the last complete,
    /// newline-terminated line consumed (or the start of an incomplete
    /// final line, which is left for a later read).
    pub next_offset: u64,
}

/// Incremental tail read (S10 soak fix): parse only the bytes appended since
/// `offset`, so a live session folds each journal byte once instead of
/// re-reading the whole journal after every turn. Line-boundary rules:
///
/// - a newline-terminated line parses exactly as [`read_journal`] parses it;
/// - a final line WITHOUT a trailing newline is left UNCONSUMED
///   (`next_offset` points at its start): the single writer appends whole
///   lines, so an unterminated tail is either an in-flight append (a later
///   read completes it) or a crash-torn tail (the next open truncates it —
///   the same bytes the full reader would drop, never folded twice);
/// - a parse failure on any TERMINATED line is the same integrity error the
///   full reader reports — the middle must stay authoritative;
/// - a file shorter than `offset` (truncated or replaced out from under the
///   session) is an error, never a silent resync — the caller falls back to
///   a full [`read_journal`], which fails loudly on real corruption.
///
/// Envelopes returned by consecutive tail reads from a monotonically
/// advancing offset are exactly the suffixes of one full read, in order.
pub fn read_journal_from(path: &Path, offset: u64) -> io::Result<JournalTail> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < offset {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("journal shrank below fold offset {offset} (len {file_len})"),
        ));
    }
    file.seek(io::SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut tail = JournalTail {
        report: JournalReport {
            path: Some(path.to_path_buf()),
            ..JournalReport::default()
        },
        next_offset: offset,
    };
    let text = String::from_utf8_lossy(&bytes);
    // Same split-and-trim discipline as parse_journal_bytes, but offsets are
    // absolute (base + line start) and an unterminated final line is left
    // unconsumed rather than classified.
    let lines: Vec<&str> = text.split('\n').collect();
    let mut cursor: u64 = offset;
    for (index, line) in lines.iter().enumerate() {
        let is_last_line = index == lines.len() - 1;
        if is_last_line && line.is_empty() && bytes.ends_with(b"\n") {
            // split('\n') artifact: no bytes follow the final newline.
            break;
        }
        let terminated = !is_last_line || bytes.ends_with(b"\n");
        if !terminated {
            // Unterminated tail: consumed by a later read (in-flight append)
            // or truncated at the next open (crash-torn). Never folded now.
            break;
        }
        let line_len = line.len() as u64 + 1; // account for the '\n'
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            cursor += line_len;
            continue;
        }
        match serde_json::from_str::<OpEnvelope>(trimmed) {
            Ok(envelope) => tail.report.envelopes.push(envelope),
            Err(err) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("journal integrity error at byte {cursor}: {err}"),
                ));
            }
        }
        cursor += line_len;
    }
    tail.next_offset = cursor;
    Ok(tail)
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

#[cfg(test)]
mod tail_tests {
    use super::*;
    use crate::op::Op;

    fn envelope(id: &str) -> OpEnvelope {
        OpEnvelope::new(
            id,
            "now",
            Op::AssistantText {
                turn_id: format!("t-{id}"),
                text: format!("text {id}"),
            },
        )
    }

    fn append_line(path: &Path, line: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(line.as_bytes()).unwrap();
        file.sync_data().unwrap();
    }

    fn temp_journal(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "wayland-nano-tail-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (dir.clone(), dir.join("journal.jsonl"))
    }

    #[test]
    fn tail_read_consumes_each_byte_once_and_matches_full_read() {
        let (dir, path) = temp_journal("delta");
        let mut offset = 0u64;
        let mut incremental: Vec<OpEnvelope> = Vec::new();
        for id in ["1", "2", "3", "4"] {
            let line = format!("{}\n", serde_json::to_string(&envelope(id)).unwrap());
            append_line(&path, &line);
            let tail = read_journal_from(&path, offset).expect("tail read");
            assert_eq!(
                tail.report.envelopes.len(),
                1,
                "one new envelope per append"
            );
            assert_eq!(
                tail.next_offset as usize,
                path.metadata().unwrap().len() as usize
            );
            offset = tail.next_offset;
            incremental.extend(tail.report.envelopes);
        }
        // Idle read at EOF: no envelopes, offset unmoved.
        let tail = read_journal_from(&path, offset).expect("idle tail read");
        assert!(tail.report.envelopes.is_empty());
        assert_eq!(tail.next_offset, offset);
        // The incremental stream equals the full read byte-for-byte.
        let full = read_journal(&path).expect("full read");
        assert_eq!(
            incremental.iter().map(|e| &e.id).collect::<Vec<_>>(),
            full.envelopes.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_read_leaves_unterminated_final_line_unconsumed() {
        let (dir, path) = temp_journal("unterminated");
        let line_one = format!("{}\n", serde_json::to_string(&envelope("1")).unwrap());
        append_line(&path, &line_one);
        let tail = read_journal_from(&path, 0).expect("first read");
        assert_eq!(tail.report.envelopes.len(), 1);
        let offset = tail.next_offset;
        // An in-flight append visible without its terminating newline yet.
        let partial = serde_json::to_string(&envelope("2")).unwrap();
        append_line(&path, &partial);
        let tail = read_journal_from(&path, offset).expect("partial read");
        assert!(
            tail.report.envelopes.is_empty(),
            "an unterminated tail is never folded"
        );
        assert_eq!(
            tail.next_offset, offset,
            "the partial line stays unconsumed"
        );
        // The newline lands: the same read now completes the line.
        append_line(&path, "\n");
        let tail = read_journal_from(&path, offset).expect("completed read");
        assert_eq!(tail.report.envelopes.len(), 1);
        assert_eq!(tail.report.envelopes[0].id, "2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_read_reports_integrity_error_on_corrupt_terminated_line() {
        let (dir, path) = temp_journal("corrupt");
        append_line(&path, "not-json-at-all\n");
        let err = read_journal_from(&path, 0).expect_err("corrupt line must fail loudly");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_read_fails_closed_when_the_journal_shrinks() {
        let (dir, path) = temp_journal("shrunk");
        let line = format!("{}\n", serde_json::to_string(&envelope("1")).unwrap());
        append_line(&path, &line);
        let len = path.metadata().unwrap().len();
        let err = read_journal_from(&path, len + 4096)
            .expect_err("a read past EOF means the journal was replaced");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
