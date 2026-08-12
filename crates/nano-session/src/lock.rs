//! Cross-process advisory lock on a journal file (C11 SessionGuard, OS
//! layer). One half of the engine-library exclusion: the in-process async
//! mutex (nano-agent) serializes cooperating writers within one host; this
//! OS lock serializes ACROSS host processes and covers unloaded sessions.
//!
//! Semantics: non-blocking exclusive acquire (`flock(LOCK_EX|LOCK_NB)` on
//! unix, `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK|LOCKFILE_FAIL_IMMEDIATELY)` on
//! Windows). Held for a short critical section only; released on drop (and
//! by the OS if the holder process dies — no stale-lock wedge, which is why
//! this is an OS lock and not a create-if-absent lock file). Contention is a
//! typed [`LockBusy`], never a silent queue.
//!
//! Windows note: `LockFileEx` byte ranges are ENFORCED against other handles
//! in the same process (unlike advisory `flock`), so the lock region is a
//! fixed offset far past any real journal size (0xFFFFFFFF_FFFFF000) — the
//! lock is a rendezvous, never a data guard, and ordinary reads/writes of
//! journal content never intersect it.

use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another handle (this process or another) holds the lock.
    #[error("session journal is locked by another writer")]
    Busy,
    #[error("cannot acquire session journal lock: {0}")]
    Io(#[from] io::Error),
}

/// RAII handle: the lock is held until this value drops.
pub struct FileLock {
    file: File,
}

impl std::fmt::Debug for FileLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileLock").finish_non_exhaustive()
    }
}

impl FileLock {
    /// Non-blocking exclusive acquire on `path` (the journal file itself —
    /// locking the data file, not a sidecar, so the lock lifecycle tracks
    /// the journal's).
    pub fn try_acquire(path: &Path) -> Result<Self, LockError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        lock_file(&file)?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn lock_file(file: &File) -> Result<(), LockError> {
    use std::os::unix::io::AsRawFd;
    // Safety: fd is a valid open file descriptor owned by `file`; flock with
    // LOCK_NB either takes the exclusive lock or fails with EWOULDBLOCK.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            Err(LockError::Busy)
        } else {
            Err(LockError::Io(err))
        }
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) {
    use std::os::unix::io::AsRawFd;
    // Safety: fd is valid and owned by `file`; releasing a lock we hold (or
    // the OS already released) is harmless.
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

#[cfg(windows)]
/// The lock rendezvous region: a fixed 1-byte range at a far-past-EOF
/// offset, so enforced byte-range locking never conflicts with real journal
/// I/O through other handles (see module docs).
const LOCK_OFFSET: u64 = 0xFFFF_FFFF_FFFF_F000;

#[cfg(windows)]
fn lock_overlapped() -> windows_sys::Win32::System::IO::OVERLAPPED {
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe {
        // Safety: OVERLAPPED is a plain C struct; all-zero is a valid base
        // (no event), with the offset fields set below.
        std::mem::zeroed()
    };
    overlapped.Anonymous.Anonymous.Offset = (LOCK_OFFSET & 0xFFFF_FFFF) as u32;
    overlapped.Anonymous.Anonymous.OffsetHigh = (LOCK_OFFSET >> 32) as u32;
    overlapped
}

#[cfg(windows)]
fn lock_file(file: &File) -> Result<(), LockError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::LOCKFILE_EXCLUSIVE_LOCK;
    use windows_sys::Win32::Storage::FileSystem::LOCKFILE_FAIL_IMMEDIATELY;
    use windows_sys::Win32::Storage::FileSystem::LockFileEx;
    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let mut overlapped = lock_overlapped();
    // Safety: `handle` is a valid open file handle with write access;
    // `overlapped` points at a valid, live OVERLAPPED for the call.
    let ok = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok == 0 {
        let err = io::Error::last_os_error();
        return match err.raw_os_error().map(|code| code as u32) {
            Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING)
            | Some(windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION) => Err(LockError::Busy),
            _ => Err(LockError::Io(err)),
        };
    }
    Ok(())
}

#[cfg(windows)]
fn unlock_file(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let mut overlapped = lock_overlapped();
    // Safety: `handle` is valid and holds the lock being released.
    unsafe {
        UnlockFileEx(handle, 0, 1, 0, &mut overlapped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_typed_busy_until_release() {
        let dir = std::env::temp_dir().join(format!(
            "nano-c11-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(&path, "{}\n").unwrap();

        let first = FileLock::try_acquire(&path).expect("first acquire");
        // The lock is a rendezvous, never a data guard: content I/O through
        // other handles keeps working while the lock is held (the pinned
        // far-past-EOF region on Windows; advisory flock on unix).
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
        // A SECOND HANDLE (what a competing host process would do) must see
        // typed busy on both platforms: flock is per open-file-description,
        // LockFileEx byte-range conflicts across handles.
        match FileLock::try_acquire(&path) {
            Err(LockError::Busy) => {}
            other => panic!("expected typed busy, got {other:?}"),
        }
        drop(first);
        let second = FileLock::try_acquire(&path).expect("reacquire after release");
        drop(second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
