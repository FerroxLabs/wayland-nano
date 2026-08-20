//! Standalone red-green receipt storage and read-only preflight primitives.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::VerifyError;
use crate::registry::{GateRegistry, closure_digest};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schema: u32,
    pub requirement: String,
    pub test: String,
    pub gate_id: String,
    pub gate_closure_digest: String,
    pub failing_run: FailingRun,
    pub fix_commit: String,
    pub minted_at: String,
    pub minted_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailingRun {
    pub exit_code: i64,
    pub log_digest: String,
    pub observed_at_commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyVerdict {
    Valid,
    NeverRed,
    FabricatedCommit,
    GateMismatch,
    AncestryUnproven,
    Unverifiable,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptPreflight {
    Ready,
    NeverRed,
    FabricatedCommit,
    GateMismatch,
    AncestryUnproven,
    Unverifiable,
}

pub fn canonical_receipt(receipt: &Receipt) -> Result<Vec<u8>, VerifyError> {
    validate_receipt(receipt)?;
    let value = serde_json::to_value(receipt).map_err(invalid_receipt)?;
    let normalized = normalize_value(value)?;
    serde_json::to_vec(&normalized).map_err(invalid_receipt)
}

pub fn mint_receipt(receipt: Receipt) -> Result<Receipt, VerifyError> {
    validate_receipt(&receipt)?;
    Ok(receipt)
}

pub fn preflight_receipt(
    repo_root: &Path,
    bytes: &[u8],
    registry: &GateRegistry,
) -> ReceiptPreflight {
    let Ok(receipt) = serde_json::from_slice::<Receipt>(bytes) else {
        return ReceiptPreflight::Unverifiable;
    };
    if receipt.schema != 1 {
        return ReceiptPreflight::Unverifiable;
    }
    if receipt.failing_run.exit_code == 0
        || !is_lower_hex(&receipt.failing_run.log_digest, 64)
        || !is_lower_hex(&receipt.failing_run.observed_at_commit, 40)
    {
        return ReceiptPreflight::NeverRed;
    }
    if !is_lower_hex(&receipt.fix_commit, 40) {
        return ReceiptPreflight::FabricatedCommit;
    }
    match git_probe(repo_root, &["rev-parse", "--is-inside-work-tree"]) {
        Probe::Present(output) if String::from_utf8_lossy(&output).trim() == "true" => {}
        _ => return ReceiptPreflight::Unverifiable,
    }
    for commit in [&receipt.failing_run.observed_at_commit, &receipt.fix_commit] {
        let object = format!("{commit}^{{commit}}");
        match git_existence_probe(repo_root, &["cat-file", "-e", &object]) {
            Probe::Present(_) => {}
            Probe::Absent => return ReceiptPreflight::FabricatedCommit,
            Probe::Unknown => return ReceiptPreflight::Unverifiable,
        }
    }
    match git_probe(
        repo_root,
        &[
            "merge-base",
            "--is-ancestor",
            &receipt.failing_run.observed_at_commit,
            &receipt.fix_commit,
        ],
    ) {
        Probe::Present(_) => {}
        Probe::Absent => return ReceiptPreflight::AncestryUnproven,
        Probe::Unknown => return ReceiptPreflight::Unverifiable,
    }
    if !valid_git_path(&receipt.test) {
        return ReceiptPreflight::AncestryUnproven;
    }
    let test_object = format!(
        "{}:{}",
        receipt.failing_run.observed_at_commit, receipt.test
    );
    match git_existence_probe(repo_root, &["cat-file", "-e", &test_object]) {
        Probe::Present(_) => {}
        Probe::Absent => return ReceiptPreflight::AncestryUnproven,
        Probe::Unknown => return ReceiptPreflight::Unverifiable,
    }
    let Some(mapped_gate) = registry.requirements.get(&receipt.requirement) else {
        return ReceiptPreflight::GateMismatch;
    };
    if mapped_gate != &receipt.gate_id {
        return ReceiptPreflight::GateMismatch;
    }
    let Some(entry) = registry.gates.get(&receipt.gate_id) else {
        return ReceiptPreflight::GateMismatch;
    };
    let Ok(actual_digest) = closure_digest(&entry.closure) else {
        return ReceiptPreflight::GateMismatch;
    };
    if actual_digest != entry.closure_digest || receipt.gate_closure_digest != entry.closure_digest
    {
        return ReceiptPreflight::GateMismatch;
    }
    ReceiptPreflight::Ready
}

enum Probe {
    Present(Vec<u8>),
    Absent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeFailure {
    Spawn,
    Stdout,
    Timeout,
    #[cfg(not(windows))]
    Wait,
    Nonzero(GitExitCode),
    NoExitCode,
    #[cfg(windows)]
    OpenProcess,
    #[cfg(windows)]
    WaitApi,
    #[cfg(windows)]
    ExitCode,
}

#[cfg(windows)]
type GitExitCode = u32;
#[cfg(not(windows))]
type GitExitCode = i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitExit {
    Code(GitExitCode),
    #[cfg_attr(windows, allow(dead_code))]
    NoExitCode,
}

#[cfg(windows)]
const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;

#[cfg(windows)]
fn classify_windows_process_exit(raw_exit: u32) -> Option<GitExit> {
    use windows_sys::Win32::Foundation::STILL_ACTIVE;

    (raw_exit != STILL_ACTIVE as u32).then_some(GitExit::Code(raw_exit))
}

fn unknown_probe(reason: ProbeFailure) -> Probe {
    // Fixed labels make CI failures actionable without exposing the repository
    // path, command arguments, inherited environment, or Git output.
    eprintln!("receipt Git probe unavailable: {reason:?}");
    Probe::Unknown
}

fn git_probe(repo_root: &Path, args: &[&str]) -> Probe {
    git_probe_with_absence(repo_root, args, false)
}

fn git_existence_probe(repo_root: &Path, args: &[&str]) -> Probe {
    git_probe_with_absence(repo_root, args, true)
}

fn git_probe_with_absence(repo_root: &Path, args: &[&str], any_nonzero_absent: bool) -> Probe {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for name in git_launch_environment() {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let Ok(mut child) = command.spawn() else {
        return unknown_probe(ProbeFailure::Spawn);
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = match wait_for_git(&mut child, deadline) {
        Ok(Some(status)) => status,
        Ok(None) => {
            terminate_git(&mut child);
            return unknown_probe(ProbeFailure::Timeout);
        }
        Err(reason) => {
            terminate_git(&mut child);
            return unknown_probe(reason);
        }
    };
    let mut output = Vec::new();
    if child
        .stdout
        .take()
        .is_none_or(|mut stdout| stdout.read_to_end(&mut output).is_err())
    {
        return unknown_probe(ProbeFailure::Stdout);
    }
    classify_git_exit(status, output, any_nonzero_absent)
}

fn classify_git_exit(status: GitExit, output: Vec<u8>, any_nonzero_absent: bool) -> Probe {
    match status {
        GitExit::Code(0) => Probe::Present(output),
        GitExit::Code(1) => Probe::Absent,
        GitExit::Code(_) if any_nonzero_absent => Probe::Absent,
        GitExit::Code(code) => unknown_probe(ProbeFailure::Nonzero(code)),
        GitExit::NoExitCode => unknown_probe(ProbeFailure::NoExitCode),
    }
}

#[cfg(not(windows))]
fn terminate_git(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn terminate_git(child: &mut std::process::Child) {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
        WaitForSingleObject,
    };

    // Open a native handle from the PID instead of using Rust's inherited child
    // handle, which can be unusable when ARM64 Windows supervises x64 Git.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
            0,
            child.id(),
        )
    };
    if handle == 0 {
        // The inherited child handle is the only cleanup route left. `kill`
        // and the native wait are both bounded; do not call `Child::wait`,
        // which can hang under ARM64/x64 emulation.
        let _ = child.kill();
        unsafe {
            let _ = WaitForSingleObject(child.as_raw_handle() as HANDLE, 1_000);
        }
        return;
    }
    // SAFETY: `handle` is owned here and closed below. Cleanup remains bounded.
    unsafe {
        let _ = TerminateProcess(handle, 1);
        let _ = WaitForSingleObject(handle, 1_000) == WAIT_OBJECT_0;
        let _ = CloseHandle(handle);
    }
}

#[cfg(not(windows))]
fn wait_for_git(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Result<Option<GitExit>, ProbeFailure> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Some(match status.code() {
                    Some(code) => GitExit::Code(code),
                    None => GitExit::NoExitCode,
                }));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => return Ok(None),
            Err(_) => return Err(ProbeFailure::Wait),
        }
    }
}

#[cfg(windows)]
fn wait_for_git(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Result<Option<GitExit>, ProbeFailure> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
    };

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Ok(None);
    }
    let timeout_ms = remaining.as_millis().min(u128::from(u32::MAX)) as u32;
    // A fresh native handle avoids the emulation-sensitive handle Rust inherited
    // while spawning x64 Git from an ARM64 process.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            child.id(),
        )
    };
    if handle == 0 {
        return Err(ProbeFailure::OpenProcess);
    }
    // SAFETY: `handle` is live and owned until the unconditional close below.
    let wait_result = unsafe { WaitForSingleObject(handle, timeout_ms) };
    let result = match wait_result {
        WAIT_OBJECT_0 => loop {
            let mut exit_code = 0;
            if unsafe { GetExitCodeProcess(handle, &mut exit_code) } == 0 {
                break Err(ProbeFailure::ExitCode);
            }
            if let Some(status) = classify_windows_process_exit(exit_code) {
                break Ok(Some(status));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break Ok(None);
            }
            std::thread::sleep(remaining.min(Duration::from_millis(25)));
        },
        WAIT_TIMEOUT => Ok(None),
        _ => Err(ProbeFailure::WaitApi),
    };
    // SAFETY: this function owns `handle`, and it is no longer used.
    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

fn git_launch_environment() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &[
            "PATH",
            "SYSTEMROOT",
            "PATHEXT",
            "COMSPEC",
            "TEMP",
            "TMP",
            "PROCESSOR_ARCHITEW6432",
        ]
    }
    #[cfg(not(windows))]
    {
        &["PATH", "TMPDIR", "TEMP", "TMP"]
    }
}

fn valid_git_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains([':', '\\'])
        && !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

pub fn read_receipt(path: &Path) -> Result<Receipt, ReceiptPreflight> {
    for attempt in 0..2 {
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(receipt) = serde_json::from_slice::<Receipt>(&bytes)
            && validate_receipt(&receipt).is_ok()
        {
            return Ok(receipt);
        }
        if attempt == 0 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err(ReceiptPreflight::Unverifiable)
}

pub fn write_receipt(directory: &Path, receipt: &Receipt) -> Result<PathBuf, VerifyError> {
    write_receipt_with_policy(
        directory,
        receipt,
        Duration::from_millis(50),
        Duration::from_secs(10),
        Duration::from_secs(60),
    )
}

fn write_receipt_with_policy(
    directory: &Path,
    receipt: &Receipt,
    retry: Duration,
    deadline: Duration,
    stale_after: Duration,
) -> Result<PathBuf, VerifyError> {
    validate_slug(&receipt.requirement)?;
    let target = directory.join(format!("{}.receipt.json", receipt.requirement));
    let lock_path = target.with_extension("lock");
    let _lock = acquire_lock(&lock_path, retry, deadline, stale_after)?;
    let bytes = canonical_receipt(receipt)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(VerifyError::StoreIo)?;
    temporary.write_all(&bytes).map_err(VerifyError::StoreIo)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(VerifyError::StoreIo)?;
    platform_replace(temporary.path(), &target)?;
    sync_directory(directory)?;
    Ok(target)
}

struct ReceiptLock {
    path: PathBuf,
    _file: File,
}

impl Drop for ReceiptLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire_lock(
    path: &Path,
    retry: Duration,
    deadline: Duration,
    stale_after: Duration,
) -> Result<ReceiptLock, VerifyError> {
    let started = Instant::now();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => {
                return Ok(ReceiptLock {
                    path: path.to_owned(),
                    _file: file,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_is_stale(path, stale_after) {
                    match std::fs::remove_file(path) {
                        Ok(()) => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(VerifyError::StoreIo(error)),
                    }
                }
                if started.elapsed() >= deadline {
                    return Err(VerifyError::LockHeld(path.display().to_string()));
                }
                std::thread::sleep(retry.min(deadline.saturating_sub(started.elapsed())));
            }
            Err(error) => return Err(VerifyError::StoreIo(error)),
        }
    }
}

fn lock_is_stale(path: &Path, stale_after: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > stale_after)
}

fn validate_slug(requirement: &str) -> Result<(), VerifyError> {
    if requirement.is_empty()
        || !requirement
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || requirement == "."
        || requirement == ".."
    {
        return Err(invalid_receipt(
            "requirement is not a confined filename slug",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn platform_replace(source: &Path, target: &Path) -> Result<(), VerifyError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both pointers reference NUL-terminated UTF-16 paths for this call.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
        return Err(VerifyError::StoreIo(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(unix)]
fn platform_replace(source: &Path, target: &Path) -> Result<(), VerifyError> {
    std::fs::rename(source, target).map_err(VerifyError::StoreIo)
}

#[cfg(not(any(windows, unix)))]
fn platform_replace(_source: &Path, _target: &Path) -> Result<(), VerifyError> {
    Err(VerifyError::StoreIo(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no authoritative atomic replacement primitive",
    )))
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), VerifyError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(VerifyError::StoreIo)
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<(), VerifyError> {
    Ok(())
}

fn validate_receipt(receipt: &Receipt) -> Result<(), VerifyError> {
    if receipt.schema != 1
        || receipt.failing_run.exit_code == 0
        || !is_lower_hex(&receipt.failing_run.log_digest, 64)
        || !is_lower_hex(&receipt.gate_closure_digest, 64)
        || !is_lower_hex(&receipt.failing_run.observed_at_commit, 40)
        || !is_lower_hex(&receipt.fix_commit, 40)
        || receipt.requirement.is_empty()
        || receipt.test.is_empty()
        || receipt.gate_id.is_empty()
        || receipt.minted_by.is_empty()
        || !is_rfc3339_utc(&receipt.minted_at)
    {
        return Err(invalid_receipt("invalid schema-1 red receipt"));
    }
    Ok(())
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_rfc3339_utc(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || *bytes.last().unwrap_or(&0) != b'Z'
    {
        return false;
    }
    let main = &value[..value.len() - 1];
    let (seconds, fraction) = main[17..].split_once('.').unwrap_or((&main[17..], ""));
    let number = |part: &str| part.parse::<u32>().ok();
    let year = number(&value[..4]);
    let month = number(&value[5..7]);
    let day = number(&value[8..10]);
    let hour = number(&value[11..13]);
    let minute = number(&value[14..16]);
    let second = number(seconds);
    let leap = year.is_some_and(|year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0));
    let max_day = match month {
        Some(2) if leap => 29,
        Some(2) => 28,
        Some(4 | 6 | 9 | 11) => 30,
        Some(1 | 3 | 5 | 7 | 8 | 10 | 12) => 31,
        _ => return false,
    };
    value[..4].bytes().all(|b| b.is_ascii_digit())
        && value[5..7].bytes().all(|b| b.is_ascii_digit())
        && value[8..10].bytes().all(|b| b.is_ascii_digit())
        && value[11..13].bytes().all(|b| b.is_ascii_digit())
        && value[14..16].bytes().all(|b| b.is_ascii_digit())
        && day.is_some_and(|day| (1..=max_day).contains(&day))
        && hour.is_some_and(|hour| hour <= 23)
        && minute.is_some_and(|minute| minute <= 59)
        && second.is_some_and(|second| second <= 59)
        && seconds.len() == 2
        && (!main[17..].contains('.')
            || (!fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit())))
}

fn normalize_value(value: serde_json::Value) -> Result<serde_json::Value, VerifyError> {
    match value {
        serde_json::Value::String(value) => Ok(serde_json::Value::String(value.nfc().collect())),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(normalize_value)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key.nfc().collect(), normalize_value(value)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        serde_json::Value::Number(ref number) if !number.is_i64() && !number.is_u64() => {
            Err(invalid_receipt("canonical JSON permits integers only"))
        }
        other => Ok(other),
    }
}

fn invalid_receipt(error: impl std::fmt::Display) -> VerifyError {
    VerifyError::StoreIo(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::registry::{CwdPolicy, GateClosure, GateRegistryEntry};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn unknown_probe_uses_a_bounded_secret_safe_reason() {
        assert_eq!(format!("{:?}", ProbeFailure::Spawn), "Spawn");
        assert!(matches!(unknown_probe(ProbeFailure::Spawn), Probe::Unknown));
    }

    #[test]
    fn nonzero_probe_reason_reports_only_a_bounded_raw_exit_code() {
        #[cfg(windows)]
        let (code, expected) = (u32::MAX, "Nonzero(4294967295)");
        #[cfg(not(windows))]
        let (code, expected) = (i32::MIN, "Nonzero(-2147483648)");

        let reason = format!("{:?}", ProbeFailure::Nonzero(code));
        assert_eq!(reason, expected);
        assert!(reason.len() <= 20);
    }

    #[test]
    fn raw_git_exit_codes_have_fixed_probe_semantics() {
        assert!(matches!(
            classify_git_exit(GitExit::Code(0), b"object".to_vec(), false),
            Probe::Present(output) if output == b"object"
        ));
        assert!(matches!(
            classify_git_exit(GitExit::Code(1), Vec::new(), false),
            Probe::Absent
        ));
        assert!(matches!(
            classify_git_exit(GitExit::Code(259), Vec::new(), false),
            Probe::Unknown
        ));
        assert!(matches!(
            classify_git_exit(GitExit::Code(259), Vec::new(), true),
            Probe::Absent
        ));
        assert!(matches!(
            classify_git_exit(GitExit::NoExitCode, Vec::new(), true),
            Probe::Unknown
        ));
    }

    #[cfg(windows)]
    #[test]
    fn git_probe_forwards_windows_emulation_architecture_marker() {
        assert!(git_launch_environment().contains(&"PROCESSOR_ARCHITEW6432"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_still_active_is_polled_until_a_final_raw_exit_code() {
        assert_eq!(classify_windows_process_exit(259), None);
        assert_eq!(classify_windows_process_exit(0), Some(GitExit::Code(0)));
        assert_eq!(classify_windows_process_exit(7), Some(GitExit::Code(7)));
    }

    fn receipt(requirement: &str) -> Receipt {
        Receipt {
            schema: 1,
            requirement: requirement.into(),
            test: "tests/red.rs".into(),
            gate_id: "gate-a".into(),
            gate_closure_digest: "a".repeat(64),
            failing_run: FailingRun {
                exit_code: 1,
                log_digest: "b".repeat(64),
                observed_at_commit: "c".repeat(40),
            },
            fix_commit: "d".repeat(40),
            minted_at: "2026-08-17T00:00:00Z".into(),
            minted_by: "wayland-nano 0.1.1".into(),
        }
    }

    #[test]
    fn canonical_schema_and_mint_validation() {
        let valid = receipt("RCPT-01");
        let bytes = canonical_receipt(&valid).expect("valid receipt canonicalizes");
        assert!(!bytes.ends_with(b"\n"));
        assert_eq!(serde_json::from_slice::<Receipt>(&bytes).unwrap(), valid);

        let mut green = receipt("RCPT-01");
        green.failing_run.exit_code = 0;
        assert!(mint_receipt(green).is_err());
    }

    #[test]
    fn store_reader_retry_then_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.receipt.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let started = Instant::now();
        assert_eq!(read_receipt(&path), Err(ReceiptPreflight::Unverifiable));
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "reader returned before the required retry delay"
        );
    }

    #[test]
    fn store_lock_contention_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("RCPT-01.receipt.json");
        std::fs::write(target.with_extension("lock"), b"held").unwrap();
        let started = Instant::now();
        let result = write_receipt_with_policy(
            dir.path(),
            &receipt("RCPT-01"),
            Duration::from_millis(10),
            Duration::from_millis(60),
            Duration::from_secs(60),
        );
        assert!(matches!(result, Err(VerifyError::LockHeld(_))));
        assert!(started.elapsed() >= Duration::from_millis(60));
        assert!(started.elapsed() < Duration::from_secs(1));

        std::thread::sleep(Duration::from_millis(2));
        assert!(
            write_receipt_with_policy(
                dir.path(),
                &receipt("RCPT-01"),
                Duration::from_millis(1),
                Duration::from_millis(50),
                Duration::ZERO,
            )
            .is_ok()
        );
    }

    #[test]
    fn store_replace_overwrites_existing_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let old = receipt("RCPT-01");
        let target = dir.path().join("RCPT-01.receipt.json");
        std::fs::write(&target, canonical_receipt(&old).unwrap()).unwrap();

        let mut new = old.clone();
        new.fix_commit = "e".repeat(40);
        let old_bytes = canonical_receipt(&old).unwrap();
        let new_bytes = canonical_receipt(&new).unwrap();
        assert_eq!(write_receipt(dir.path(), &new).unwrap(), target);
        let observed = std::fs::read(&target).unwrap();
        assert!(observed == old_bytes || observed == new_bytes);
        assert_eq!(read_receipt(&target).unwrap(), new);
    }

    #[test]
    fn hostile_object_database_cannot_supply_foreign_commits() {
        let _guard = ENV_LOCK.lock().unwrap();
        let local = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        init_repo(local.path());
        init_repo(foreign.path());

        std::fs::create_dir_all(foreign.path().join("tests")).unwrap();
        std::fs::write(foreign.path().join("tests/red.rs"), "#[test] fn red() {}\n").unwrap();
        git(foreign.path(), &["add", "tests/red.rs"]);
        git(foreign.path(), &["commit", "-m", "observed red"]);
        let observed = git_output(foreign.path(), &["rev-parse", "HEAD"]);
        std::fs::write(foreign.path().join("fix.txt"), "fixed\n").unwrap();
        git(foreign.path(), &["add", "fix.txt"]);
        git(foreign.path(), &["commit", "-m", "fix"]);
        let fixed = git_output(foreign.path(), &["rev-parse", "HEAD"]);

        let closure = GateClosure {
            argv: vec!["gate".into()],
            env: BTreeMap::new(),
            cwd_policy: CwdPolicy::RepoRoot,
            wrapped_tools: Vec::new(),
        };
        let digest = closure_digest(&closure).unwrap();
        let registry = GateRegistry {
            schema: 1,
            gates: BTreeMap::from([(
                "gate-a".into(),
                GateRegistryEntry {
                    card: "unused".into(),
                    script: "unused".into(),
                    closure,
                    closure_digest: digest.clone(),
                    run_artifact: "unused".into(),
                },
            )]),
            requirements: BTreeMap::from([("RCPT-01".into(), "gate-a".into())]),
        };
        let mut hostile_receipt = receipt("RCPT-01");
        hostile_receipt.failing_run.observed_at_commit = observed;
        hostile_receipt.fix_commit = fixed;
        hostile_receipt.gate_closure_digest = digest;
        let bytes = serde_json::to_vec(&hostile_receipt).unwrap();

        let variable = "GIT_OBJECT_DIRECTORY";
        let previous = std::env::var_os(variable);
        unsafe {
            std::env::set_var(variable, foreign.path().join(".git/objects"));
        }
        let result = preflight_receipt(local.path(), &bytes, &registry);
        unsafe {
            if let Some(value) = previous {
                std::env::set_var(variable, value);
            } else {
                std::env::remove_var(variable);
            }
        }
        assert_ne!(result, ReceiptPreflight::Ready);
        assert!(matches!(
            result,
            ReceiptPreflight::FabricatedCommit | ReceiptPreflight::Unverifiable
        ));
    }

    fn init_repo(path: &Path) {
        git(path, &["init"]);
        git(path, &["config", "user.name", "Nano Verify Test"]);
        git(
            path,
            &["config", "user.email", "nano-verify@example.invalid"],
        );
    }

    fn git(path: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }

    fn git_output(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}
