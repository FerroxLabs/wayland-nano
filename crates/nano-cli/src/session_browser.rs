//! One bounded, derived session-listing path for ACP, TUI, and headless CLI.
//!
//! Privacy contract: listing opens only validated `*.jsonl` entries, uses a
//! no-follow final-component open, and reads at most the first journal line.
//! It never reads prompts or derives titles. `Live` is a point-in-time lock
//! probe, not lifetime ownership; picker-driven load of a live row is refused
//! until the separately reviewed session-ownership slice lands.

use nano_agent::bootstrap::is_fs_safe_session_id;
use nano_session::audit_private_session_dir;
use nano_session::lock::{FileLock, LockError};
use nano_session::op::{Op, OpEnvelope};
use serde::Serialize;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub const MAX_SESSION_SUMMARIES: usize = 200;
/// ACP extension method served by [`handle_list_request`].
pub const SESSION_LIST_METHOD: &str = "_wayland/session/list";
const MAX_FIRST_LINE_BYTES: u64 = 64 * 1024;

/// The lock classification is a point-in-time probe. Slice 0 closes the race.
pub const LIVE_STATUS_CAVEAT: &str =
    "live status is a point-in-time probe; selection never loads a live row";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Closed,
    Live,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub cwd: String,
    pub modified_ms: u64,
    pub size_bytes: u64,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionList {
    pub sessions: Vec<SessionSummary>,
    pub truncated: bool,
    pub live_status_caveat: String,
}

/// Report-only `/doctor` result for the per-user sessions directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDirectoryPermissionReport {
    pub private: bool,
    pub detail: String,
}

/// Derive at most 200 summaries. There is deliberately no cache.
pub fn list_sessions(sessions_dir: &Path) -> io::Result<SessionList> {
    let entries = match fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(SessionList {
                sessions: Vec::new(),
                truncated: false,
                live_status_caveat: LIVE_STATUS_CAVEAT.to_string(),
            });
        }
        Err(err) => return Err(err),
    };

    let mut summaries = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".jsonl")) else {
            continue;
        };
        // This check precedes use of entry.path(), and therefore every join.
        if !is_fs_safe_session_id(stem) {
            continue;
        }
        if let Some(summary) = summarize_session(sessions_dir, stem)? {
            summaries.push(summary);
        }
    }
    summaries.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    let truncated = summaries.len() > MAX_SESSION_SUMMARIES;
    summaries.truncate(MAX_SESSION_SUMMARIES);
    Ok(SessionList {
        sessions: summaries,
        truncated,
        live_status_caveat: LIVE_STATUS_CAVEAT.to_string(),
    })
}

/// ACP `_wayland/session/list` handler entry point; params are intentionally empty.
pub fn handle_list_request(sessions_dir: &Path) -> io::Result<serde_json::Value> {
    serde_json::to_value(list_sessions(sessions_dir)?)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

/// Integrator entry point for `/doctor`'s sessions-directory ACL line.
/// Failure is reported but never blocks normal session use.
pub fn sessions_dir_permission_report(sessions_dir: &Path) -> SessionDirectoryPermissionReport {
    match audit_private_session_dir(sessions_dir) {
        Ok(()) => SessionDirectoryPermissionReport {
            private: true,
            detail: "owner-only sessions directory".to_string(),
        },
        Err(err) => SessionDirectoryPermissionReport {
            private: false,
            detail: err.to_string(),
        },
    }
}

/// Headless rendering over the same list used by ACP and the TUI picker.
pub fn print_sessions(sessions_dir: &Path, out: &mut dyn Write) -> i32 {
    match list_sessions(sessions_dir) {
        Ok(list) => {
            for row in &list.sessions {
                if writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{:?}",
                    row.session_id, row.cwd, row.modified_ms, row.size_bytes, row.status
                )
                .is_err()
                {
                    return 2;
                }
            }
            if list.truncated && writeln!(out, "[truncated at {MAX_SESSION_SUMMARIES}]").is_err() {
                return 2;
            }
            0
        }
        Err(err) => {
            let _ = writeln!(out, "wayland-nano sessions: {err}");
            2
        }
    }
}

fn summarize_session(sessions_dir: &Path, session_id: &str) -> io::Result<Option<SessionSummary>> {
    debug_assert!(is_fs_safe_session_id(session_id));
    let path = sessions_dir.join(format!("{session_id}.jsonl"));
    let Some((file, metadata)) = open_regular_no_follow(&path)? else {
        return Ok(None);
    };
    let modified_ms = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?
        .as_millis()
        .try_into()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let size_bytes = metadata.len();
    let parsed = read_session_begin(&file, session_id);
    let (summary_id, cwd, status) = match parsed {
        Some((summary_id, cwd)) => {
            // Probe the exact no-follow handle already used for the first
            // line. There is no path reopen or swap window.
            let status = match FileLock::try_acquire_file(file) {
                Ok(probe) => {
                    drop(probe);
                    SessionStatus::Closed
                }
                Err(LockError::Busy) => SessionStatus::Live,
                Err(LockError::Io(err)) => return Err(err),
            };
            (summary_id, cwd, status)
        }
        None => (
            session_id.to_string(),
            String::new(),
            SessionStatus::Corrupt,
        ),
    };
    Ok(Some(SessionSummary {
        session_id: summary_id,
        cwd,
        modified_ms,
        size_bytes,
        status,
    }))
}

fn read_session_begin(file: &File, expected_id: &str) -> Option<(String, String)> {
    read_session_begin_from(file, expected_id)
}

fn read_session_begin_from(reader: impl Read, expected_id: &str) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    // Capacity one is intentional: the standard BufReader may otherwise
    // prefetch transcript bytes after the first newline into its buffer.
    let mut reader = BufReader::with_capacity(1, reader).take(MAX_FIRST_LINE_BYTES + 1);
    reader.read_until(b'\n', &mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FIRST_LINE_BYTES || !bytes.ends_with(b"\n") {
        return None;
    }
    let line = std::str::from_utf8(&bytes)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    let envelope: OpEnvelope = serde_json::from_str(line).ok()?;
    match envelope.op {
        Op::SessionBegin { session_id, cwd }
            if session_id == expected_id && is_fs_safe_session_id(&session_id) =>
        {
            Some((session_id, cwd))
        }
        _ => None,
    }
}

fn open_regular_no_follow(path: &Path) -> io::Result<Option<(File, Metadata)>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if !before.file_type().is_file() || is_reparse_point(&before) {
        return Ok(None);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options.open(path)?;
    let after = file.metadata()?;
    if !after.file_type().is_file() || is_reparse_point(&after) || !same_file(&before, &after) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session entry changed during open",
        ));
    }
    Ok(Some((file, after)))
}

#[cfg(unix)]
fn set_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_NOFOLLOW: i32 = 0x20_000;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const O_NOFOLLOW: i32 = 0x100;
    options.custom_flags(O_NOFOLLOW);
}

#[cfg(windows)]
fn set_no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    // Omit FILE_SHARE_DELETE: the validated name cannot be swapped between
    // the first-line read and FileLock's point-in-time liveness probe.
    options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &Metadata) -> bool {
    false
}

#[cfg(unix)]
fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(windows)]
fn same_file(_: &Metadata, _: &Metadata) -> bool {
    // `FILE_FLAG_OPEN_REPARSE_POINT` makes the opened handle, rather than a
    // racy path precheck, authoritative. Handle metadata above then rejects
    // any reparse point. Stable std does not expose the Windows file index.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_session::op::OpEnvelope;
    use std::cell::Cell;
    use std::fs;
    use std::io::Cursor;
    use std::rc::Rc;

    fn journal(path: &Path, id: &str, cwd: &str, tail: &str) {
        let first = serde_json::to_string(&OpEnvelope::new(
            id,
            "2026-08-13T00:00:00Z",
            Op::SessionBegin {
                session_id: id.into(),
                cwd: cwd.into(),
            },
        ))
        .unwrap();
        fs::write(path, format!("{first}\n{tail}")).unwrap();
    }

    #[test]
    fn classifies_bounds_sorts_and_never_uses_transcript_text() {
        let dir = tempfile::tempdir().unwrap();
        journal(
            &dir.path().join("closed.jsonl"),
            "closed",
            "C:/closed",
            "SECRET PROMPT",
        );
        fs::write(dir.path().join("corrupt.jsonl"), "not-json\nSECRET").unwrap();
        let held_path = dir.path().join("live.jsonl");
        journal(&held_path, "live", "C:/live", "SECRET");
        let _held = FileLock::try_acquire(&held_path).unwrap();
        let list = list_sessions(dir.path()).unwrap();
        assert_eq!(list.sessions.len(), 3);
        assert!(
            list.sessions
                .iter()
                .any(|s| s.session_id == "live" && s.status == SessionStatus::Live)
        );
        assert!(
            list.sessions
                .iter()
                .any(|s| s.session_id == "corrupt" && s.status == SessionStatus::Corrupt)
        );
        assert!(!serde_json::to_string(&list).unwrap().contains("SECRET"));
    }

    #[test]
    fn caps_with_typed_truncation_and_rejects_unsafe_names() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..=MAX_SESSION_SUMMARIES {
            let id = format!("s{index:03}");
            journal(&dir.path().join(format!("{id}.jsonl")), &id, "C:/w", "");
        }
        fs::write(dir.path().join("...jsonl"), "{}\n").unwrap();
        let list = list_sessions(dir.path()).unwrap();
        assert_eq!(list.sessions.len(), MAX_SESSION_SUMMARIES);
        assert!(list.truncated);
    }

    #[test]
    fn summaries_are_sorted_by_mtime_descending_with_metadata_fields() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("older.jsonl");
        let newer = dir.path().join("newer.jsonl");
        journal(&older, "older", "C:/older", "");
        journal(&newer, "newer", "C:/newer", "tail");
        File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(10))
            .unwrap();
        File::options()
            .write(true)
            .open(&newer)
            .unwrap()
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(20))
            .unwrap();
        let list = list_sessions(dir.path()).unwrap();
        assert_eq!(list.sessions[0].session_id, "newer");
        assert_eq!(list.sessions[0].cwd, "C:/newer");
        assert_eq!(list.sessions[0].modified_ms, 20_000);
        assert_eq!(
            list.sessions[0].size_bytes,
            fs::metadata(newer).unwrap().len()
        );
        assert_eq!(list.sessions[1].session_id, "older");
    }

    #[test]
    fn wire_result_labels_point_in_time_status_and_typed_truncation() {
        let dir = tempfile::tempdir().unwrap();
        journal(&dir.path().join("s1.jsonl"), "s1", "C:/workspace", "");
        let result = handle_list_request(dir.path()).unwrap();
        assert_eq!(result["truncated"], false);
        assert_eq!(result["liveStatusCaveat"], LIVE_STATUS_CAVEAT);
        assert_eq!(result["sessions"][0]["status"], "closed");
        assert!(result["sessions"][0].get("title").is_none());
    }

    #[test]
    fn listing_one_users_directory_never_enumerates_another() {
        let root = tempfile::tempdir().unwrap();
        let user_a = root.path().join("user-a");
        let user_b = root.path().join("user-b");
        fs::create_dir_all(&user_a).unwrap();
        fs::create_dir_all(&user_b).unwrap();
        journal(&user_a.join("a.jsonl"), "a", "C:/a", "");
        journal(&user_b.join("b.jsonl"), "b", "C:/b", "");
        let list = list_sessions(&user_a).unwrap();
        assert_eq!(list.sessions.len(), 1);
        assert_eq!(list.sessions[0].session_id, "a");
    }

    #[cfg(unix)]
    #[test]
    fn doctor_permission_report_reuses_owner_only_audit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(sessions_dir_permission_report(dir.path()).private);
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!sessions_dir_permission_report(dir.path()).private);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entry_is_skipped_unopened() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "not a journal").unwrap();
        symlink(&target, dir.path().join("planted.jsonl")).unwrap();
        assert!(list_sessions(dir.path()).unwrap().sessions.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn symlink_entry_is_skipped_unopened() {
        use std::os::windows::fs::symlink_file;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "not a journal").unwrap();
        symlink_file(&target, dir.path().join("planted.jsonl")).unwrap();
        assert!(list_sessions(dir.path()).unwrap().sessions.is_empty());
    }

    struct CountingReader {
        inner: Cursor<Vec<u8>>,
        bytes_read: Rc<Cell<usize>>,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buffer)?;
            self.bytes_read.set(self.bytes_read.get() + read);
            Ok(read)
        }
    }

    #[test]
    fn first_line_reader_does_not_prefetch_transcript_bytes() {
        let first = serde_json::to_string(&OpEnvelope::new(
            "op-1",
            "2026-08-13T00:00:00Z",
            Op::SessionBegin {
                session_id: "s1".into(),
                cwd: "C:/workspace".into(),
            },
        ))
        .unwrap();
        let bytes = format!("{first}\nTRANSCRIPT SECRET THAT MUST NOT BE READ").into_bytes();
        let count = Rc::new(Cell::new(0));
        let reader = CountingReader {
            inner: Cursor::new(bytes),
            bytes_read: Rc::clone(&count),
        };
        assert_eq!(
            read_session_begin_from(reader, "s1"),
            Some(("s1".into(), "C:/workspace".into()))
        );
        assert_eq!(count.get(), first.len() + 1);
    }

    #[test]
    fn list_during_concurrent_append_stays_bounded_and_typed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("active.jsonl");
        journal(&path, "active", "C:/workspace", "");
        let append_path = path.clone();
        let appender = std::thread::spawn(move || {
            let mut file = OpenOptions::new().append(true).open(append_path).unwrap();
            for index in 0..500 {
                writeln!(file, "{{\"tail\":{index}}}").unwrap();
            }
        });
        for _ in 0..25 {
            let list = list_sessions(dir.path()).unwrap();
            assert_eq!(list.sessions.len(), 1);
            assert!(list.sessions.len() <= MAX_SESSION_SUMMARIES);
            assert!(matches!(
                list.sessions[0].status,
                SessionStatus::Closed | SessionStatus::Live
            ));
        }
        appender.join().unwrap();
    }

    #[test]
    fn lock_holder_fixture() {
        let Some(path) = std::env::var_os("NANO_SESSION_BROWSER_HOLD_PATH") else {
            return;
        };
        let ready =
            std::path::PathBuf::from(std::env::var_os("NANO_SESSION_BROWSER_READY_PATH").unwrap());
        let release = std::path::PathBuf::from(
            std::env::var_os("NANO_SESSION_BROWSER_RELEASE_PATH").unwrap(),
        );
        let _lock = FileLock::try_acquire(Path::new(&path)).unwrap();
        fs::write(&ready, b"ready").unwrap();
        for _ in 0..200 {
            if release.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("parent did not release fixture lock");
    }

    #[test]
    fn second_process_lock_is_live_and_probe_does_not_disturb_holder() {
        let dir = tempfile::tempdir().unwrap();
        let journal_path = dir.path().join("live-process.jsonl");
        let ready = dir.path().join("ready");
        let release = dir.path().join("release");
        journal(&journal_path, "live-process", "C:/workspace", "");
        journal(
            &dir.path().join("closed-process.jsonl"),
            "closed-process",
            "C:/closed",
            "",
        );
        fs::write(dir.path().join("corrupt-process.jsonl"), "bad\nsecret").unwrap();
        let link_target = dir.path().join("link-target");
        fs::write(&link_target, "must never be opened").unwrap();
        plant_session_symlink(&link_target, &dir.path().join("planted.jsonl"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "session_browser::tests::lock_holder_fixture",
                "--nocapture",
            ])
            .env("NANO_SESSION_BROWSER_HOLD_PATH", &journal_path)
            .env("NANO_SESSION_BROWSER_READY_PATH", &ready)
            .env("NANO_SESSION_BROWSER_RELEASE_PATH", &release)
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            assert!(child.try_wait().unwrap().is_none(), "fixture exited early");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(ready.exists(), "fixture did not acquire lock");
        let list = list_sessions(dir.path()).unwrap();
        assert_eq!(list.sessions.len(), 3, "planted link is skipped: {list:?}");
        assert!(list.sessions.iter().any(|summary| {
            summary.session_id == "live-process" && summary.status == SessionStatus::Live
        }));
        assert!(list.sessions.iter().any(|summary| {
            summary.session_id == "closed-process" && summary.status == SessionStatus::Closed
        }));
        assert!(list.sessions.iter().any(|summary| {
            summary.session_id == "corrupt-process" && summary.status == SessionStatus::Corrupt
        }));
        assert!(
            !list
                .sessions
                .iter()
                .any(|summary| summary.session_id == "planted")
        );
        // A second list probes and releases its own handle; the holder stays
        // live until the external release oracle is written.
        assert_eq!(
            list_sessions(dir.path())
                .unwrap()
                .sessions
                .iter()
                .find(|summary| summary.session_id == "live-process")
                .unwrap()
                .status,
            SessionStatus::Live,
        );
        fs::write(&release, b"release").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            list_sessions(dir.path())
                .unwrap()
                .sessions
                .iter()
                .find(|summary| summary.session_id == "live-process")
                .unwrap()
                .status,
            SessionStatus::Closed,
        );
    }

    #[cfg(unix)]
    fn plant_session_symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn plant_session_symlink(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_file(target, link).unwrap();
    }
}
