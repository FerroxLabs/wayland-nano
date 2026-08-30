//! Human-rooted local bootstrap and signed admin operation application.

use crate::authority::KeyRole;
use crate::authority::{AuthorityCommand, AuthorityError, AuthoritySnapshot};
use crate::key_provider::{KeyProviderError, load_key_reference};
use crate::receipt::ReceiptSigner;
use crate::signer_provider::{ExternalReceiptSigner, SignerProviderError, derive_public_key};
use crate::store::AuthorityStore;
use crate::{ActivationError, VerifiedAdminRequest, verify_admin_request};
use base64::Engine as _;
use sha2::Digest as _;
use std::io::{BufRead, Write};
use std::path::Path;

#[derive(Debug, Clone)]
struct BootstrapRequest {
    admin_public_key: [u8; 32],
    recovery_public_key: [u8; 32],
    receipt_signer_public_key: [u8; 32],
    local_cli_public_key: [u8; 32],
    admin_id: String,
}

impl BootstrapRequest {
    fn new(
        admin_public_key: [u8; 32],
        recovery_public_key: [u8; 32],
        receipt_signer_public_key: [u8; 32],
        local_cli_public_key: [u8; 32],
        admin_id: impl Into<String>,
    ) -> Self {
        Self {
            admin_public_key,
            recovery_public_key,
            receipt_signer_public_key,
            local_cli_public_key,
            admin_id: admin_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootstrapKeyPaths {
    pub admin_root: std::path::PathBuf,
    pub recovery_root: std::path::PathBuf,
    pub receipt_signer: std::path::PathBuf,
    pub local_cli_issuer: std::path::PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("bootstrap requires explicit confirmation")]
    ConfirmationRequired,
    #[error("bootstrap requires an attached controlling TTY")]
    NoControllingTty,
    #[error("bootstrap refuses remote or non-foreground login sessions")]
    RemoteSession,
    #[error("authority is already bootstrapped")]
    AlreadyBootstrapped,
    #[error("Nano home is not secure and owner controlled")]
    InsecureHome,
    #[error("bootstrap keys and key references must be distinct across roles")]
    RoleKeyReuse,
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    #[error(transparent)]
    KeyProvider(#[from] KeyProviderError),
    #[error(transparent)]
    SignerProvider(#[from] SignerProviderError),
    #[error("bootstrap receipt signing failed")]
    Receipt,
}

pub struct InteractiveOwnerProof {
    _private: (),
}

pub fn attest_interactive_owner(admin_id: &str) -> Result<InteractiveOwnerProof, BootstrapError> {
    let (mut input, mut output) = platform_owner_terminal()?;
    attest_interactive_owner_with(
        admin_id,
        || Ok(()),
        &mut std::io::BufReader::new(&mut input),
        &mut output,
    )
}

fn attest_interactive_owner_with(
    admin_id: &str,
    verify_platform: impl FnOnce() -> Result<(), BootstrapError>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<InteractiveOwnerProof, BootstrapError> {
    verify_platform()?;
    let phrase = format!("BOOTSTRAP {admin_id}");
    writeln!(output, "Type `{phrase}` to initialize Nano authority:")
        .map_err(|_| BootstrapError::NoControllingTty)?;
    output
        .flush()
        .map_err(|_| BootstrapError::NoControllingTty)?;
    let mut answer = String::new();
    input
        .read_line(&mut answer)
        .map_err(|_| BootstrapError::NoControllingTty)?;
    if answer.trim_end_matches(['\r', '\n']) != phrase {
        return Err(BootstrapError::ConfirmationRequired);
    }
    Ok(InteractiveOwnerProof { _private: () })
}

pub fn bootstrap(
    nano_home: &Path,
    paths: &BootstrapKeyPaths,
    admin_id: impl Into<String>,
    _owner_proof: InteractiveOwnerProof,
) -> Result<AuthorityStore, BootstrapError> {
    let references = [
        load_key_reference(&paths.admin_root, KeyRole::AdminRoot)?,
        load_key_reference(&paths.recovery_root, KeyRole::RecoveryRoot)?,
        load_key_reference(&paths.receipt_signer, KeyRole::ReceiptSigner)?,
        load_key_reference(&paths.local_cli_issuer, KeyRole::LocalCliIssuer)?,
    ];
    for left in 0..references.len() {
        for right in left + 1..references.len() {
            if (references[left].provider(), references[left].reference())
                == (references[right].provider(), references[right].reference())
            {
                return Err(BootstrapError::RoleKeyReuse);
            }
        }
    }
    let receipt_signer = ExternalReceiptSigner::from_key_reference(&references[2])?;
    let request = BootstrapRequest::new(
        derive_public_key(&references[0], KeyRole::AdminRoot)?,
        derive_public_key(&references[1], KeyRole::RecoveryRoot)?,
        receipt_signer.public_key(),
        derive_public_key(&references[3], KeyRole::LocalCliIssuer)?,
        admin_id,
    );
    bootstrap_attested(nano_home, request, &receipt_signer)
}

fn bootstrap_attested(
    nano_home: &Path,
    request: BootstrapRequest,
    receipt_signer: &dyn ReceiptSigner,
) -> Result<AuthorityStore, BootstrapError> {
    let public_keys = [
        request.admin_public_key,
        request.recovery_public_key,
        request.receipt_signer_public_key,
        request.local_cli_public_key,
    ];
    for left in 0..public_keys.len() {
        if public_keys[left + 1..].contains(&public_keys[left]) {
            return Err(BootstrapError::RoleKeyReuse);
        }
    }
    verify_secure_home(nano_home)?;
    let snapshot = AuthoritySnapshot::empty(request.admin_id, request.admin_public_key)
        .with_recovery_key(request.recovery_public_key)
        .with_service_keys(
            request.receipt_signer_public_key,
            request.local_cli_public_key,
        );
    if receipt_signer.public_key() != request.receipt_signer_public_key {
        return Err(BootstrapError::RoleKeyReuse);
    }
    let receipt = sign_bootstrap_receipt(&snapshot, receipt_signer)?;
    match AuthorityStore::bootstrap_initial(nano_home, snapshot, receipt) {
        Ok(store) => Ok(store),
        Err(AuthorityError::OperationConflict) => Err(BootstrapError::AlreadyBootstrapped),
        Err(error) => Err(error.into()),
    }
}

pub fn sign_bootstrap_receipt(
    snapshot: &AuthoritySnapshot,
    signer: &dyn ReceiptSigner,
) -> Result<Vec<u8>, BootstrapError> {
    signer.preflight().map_err(|_| BootstrapError::Receipt)?;
    let mut receipt = serde_json::json!({
        "admin_epoch": snapshot.admin_epoch,
        "admin_id": snapshot.admin_id,
        "authority_journal_position": 1,
        "authority_snapshot_sha256": snapshot.digest().map_err(|_| BootstrapError::Receipt)?,
        "receipt_signer_key_id": signer.key_id(),
        "root_public_key_fingerprint": crate::authority::hex(&sha2::Sha256::digest(snapshot.admin_public_key)),
        "schema": "wayland.nano.admin-bootstrap-receipt/v1"
    });
    let canonical = serde_jcs::to_vec(&receipt).map_err(|_| BootstrapError::Receipt)?;
    let mut message = b"WAYLAND-NANO-ADMIN-BOOTSTRAP\0v1\0".to_vec();
    message.extend_from_slice(&canonical);
    let signature = signer.sign(&message).map_err(|_| BootstrapError::Receipt)?;
    receipt
        .as_object_mut()
        .ok_or(BootstrapError::Receipt)?
        .insert(
            "signature".into(),
            serde_json::Value::String(
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature),
            ),
        );
    serde_jcs::to_vec(&receipt).map_err(|_| BootstrapError::Receipt)
}

pub fn verify_bootstrap_receipt(raw: &[u8], public_key: &[u8; 32]) -> Result<(), BootstrapError> {
    verify_bootstrap_receipt_fields(raw, public_key).map(|_| ())
}

fn verify_bootstrap_receipt_fields(
    raw: &[u8],
    public_key: &[u8; 32],
) -> Result<serde_json::Value, BootstrapError> {
    use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
    let mut value = crate::raw::parse_transport_frame(raw).map_err(|_| BootstrapError::Receipt)?;
    if serde_jcs::to_vec(&value).map_err(|_| BootstrapError::Receipt)? != raw {
        return Err(BootstrapError::Receipt);
    }
    let object = value.as_object_mut().ok_or(BootstrapError::Receipt)?;
    let expected = [
        "admin_epoch",
        "admin_id",
        "authority_journal_position",
        "authority_snapshot_sha256",
        "receipt_signer_key_id",
        "root_public_key_fingerprint",
        "schema",
        "signature",
    ];
    if object.len() != expected.len() || expected.iter().any(|field| !object.contains_key(*field)) {
        return Err(BootstrapError::Receipt);
    }
    if object.get("schema").and_then(serde_json::Value::as_str)
        != Some("wayland.nano.admin-bootstrap-receipt/v1")
        || object
            .get("authority_journal_position")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        return Err(BootstrapError::Receipt);
    }
    let encoded = object
        .remove("signature")
        .and_then(|signature| signature.as_str().map(str::to_owned))
        .ok_or(BootstrapError::Receipt)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| BootstrapError::Receipt)?;
    let signature = Signature::from_slice(&bytes).map_err(|_| BootstrapError::Receipt)?;
    let canonical = serde_jcs::to_vec(&value).map_err(|_| BootstrapError::Receipt)?;
    let mut message = b"WAYLAND-NANO-ADMIN-BOOTSTRAP\0v1\0".to_vec();
    message.extend_from_slice(&canonical);
    VerifyingKey::from_bytes(public_key)
        .map_err(|_| BootstrapError::Receipt)?
        .verify(&message, &signature)
        .map_err(|_| BootstrapError::Receipt)?;
    Ok(value)
}

pub(crate) fn verify_bootstrap_receipt_snapshot(
    raw: &[u8],
    snapshot: &AuthoritySnapshot,
) -> Result<(), BootstrapError> {
    let public_key = snapshot
        .receipt_signer_public_key
        .ok_or(BootstrapError::Receipt)?;
    let value = verify_bootstrap_receipt_fields(raw, &public_key)?;
    let object = value.as_object().ok_or(BootstrapError::Receipt)?;
    let digest = snapshot.digest().map_err(|_| BootstrapError::Receipt)?;
    let root_fingerprint = crate::authority::hex(&sha2::Sha256::digest(snapshot.admin_public_key));
    if object.get("admin_id").and_then(serde_json::Value::as_str)
        != Some(snapshot.admin_id.as_str())
        || object
            .get("admin_epoch")
            .and_then(serde_json::Value::as_u64)
            != Some(snapshot.admin_epoch)
        || object
            .get("authority_snapshot_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(digest.as_str())
        || object
            .get("root_public_key_fingerprint")
            .and_then(serde_json::Value::as_str)
            != Some(root_fingerprint.as_str())
    {
        return Err(BootstrapError::Receipt);
    }
    Ok(())
}

pub fn apply_signed_admin(
    store: &mut AuthorityStore,
    raw_request: &[u8],
    command: AuthorityCommand,
    now_utc: &str,
) -> Result<(), AdminError> {
    let before = store.snapshot()?.digest()?;
    let snapshot = store.snapshot()?;
    let verification_key = if matches!(command, AuthorityCommand::RecoverRoot { .. }) {
        snapshot
            .recovery_public_key
            .ok_or(AdminError::RecoveryUnavailable)?
    } else {
        snapshot.admin_public_key
    };
    let verified = verify_admin_request(raw_request, &verification_key)?;
    validate_admin_envelope(
        &verified,
        &snapshot.admin_id,
        snapshot.admin_epoch,
        command.operation_id(),
        &before,
        now_utc,
    )?;
    if verified.operation() != command_operation(&command) {
        return Err(AdminError::OperationMismatch);
    }
    let expires = parse_utc_seconds(verified.not_after()).ok_or(AdminError::Expired)?;
    let projected = snapshot.preview_admin_transaction(&command, verified.nonce(), expires)?;
    if projected.digest()? != verified.after_digest() {
        return Err(AdminError::Authority(AuthorityError::DigestMismatch));
    }
    store.commit_admin_transaction(command, verified.nonce(), expires)?;
    Ok(())
}

fn validate_admin_envelope(
    request: &VerifiedAdminRequest,
    admin_id: &str,
    epoch: u64,
    operation_id: &str,
    before: &str,
    now: &str,
) -> Result<(), AdminError> {
    if request.admin_id() != admin_id || request.admin_epoch() != epoch {
        return Err(AdminError::Authority(AuthorityError::StaleEpoch));
    }
    if request.operation_id() != operation_id || request.before_digest() != before {
        return Err(AdminError::Authority(AuthorityError::DigestMismatch));
    }
    if request.issued_at() > now || request.not_after() < now {
        return Err(AdminError::Expired);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error(transparent)]
    Activation(#[from] ActivationError),
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    #[error("admin request is outside its signed time window")]
    Expired,
    #[error("signed admin operation does not match the requested command")]
    OperationMismatch,
    #[error("root recovery is not provisioned")]
    RecoveryUnavailable,
}

fn command_operation(command: &AuthorityCommand) -> &'static str {
    match command {
        AuthorityCommand::EnrollIssuer { .. } => "enroll_issuer",
        AuthorityCommand::GrantProject { .. } => "grant_project",
        AuthorityCommand::RotateKey { .. } => "rotate_key",
        AuthorityCommand::RevokeKey { .. } => "revoke_key",
        AuthorityCommand::RevokeIssuer { .. } | AuthorityCommand::RetireSubject { .. } => {
            "revoke_issuer"
        }
        AuthorityCommand::RecoverRoot { .. } => "recover_root",
        AuthorityCommand::ConsumeNonce { .. } => "rollback",
    }
}

fn parse_utc_seconds(value: &str) -> Option<i64> {
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + hour * 3600 + minute * 60 + second)
}

fn verify_secure_home(path: &Path) -> Result<(), BootstrapError> {
    if !path.is_absolute() {
        return Err(BootstrapError::InsecureHome);
    }
    std::fs::create_dir_all(path).map_err(AuthorityError::from)?;
    let metadata = std::fs::symlink_metadata(path).map_err(AuthorityError::from)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(BootstrapError::InsecureHome);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(BootstrapError::InsecureHome);
        }
    }
    #[cfg(windows)]
    crate::key_provider::audit_owner_only_path(path).map_err(|_| BootstrapError::InsecureHome)?;
    Ok(())
}

#[cfg(unix)]
fn platform_owner_terminal() -> Result<(std::fs::File, std::fs::File), BootstrapError> {
    use std::os::fd::AsRawFd as _;
    let terminal = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map_err(|_| BootstrapError::NoControllingTty)?;
    let fd = terminal.as_raw_fd();
    let foreground = unsafe { libc::tcgetpgrp(fd) };
    let process_group = unsafe { libc::getpgrp() };
    if foreground < 0 || unsafe { libc::getsid(0) } < 0 || foreground != process_group {
        return Err(BootstrapError::RemoteSession);
    }
    verify_unix_login_provenance(fd)?;
    let output = terminal
        .try_clone()
        .map_err(|_| BootstrapError::NoControllingTty)?;
    Ok((terminal, output))
}

#[cfg(unix)]
fn verify_unix_login_provenance(fd: std::os::fd::RawFd) -> Result<(), BootstrapError> {
    let mut tty_buffer = [0 as libc::c_char; 256];
    if unsafe { libc::ttyname_r(fd, tty_buffer.as_mut_ptr(), tty_buffer.len()) } != 0 {
        return Err(BootstrapError::NoControllingTty);
    }
    let tty = unsafe { std::ffi::CStr::from_ptr(tty_buffer.as_ptr().cast::<std::ffi::c_char>()) }
        .to_string_lossy();
    let tty = tty.strip_prefix("/dev/").unwrap_or(&tty);
    let tty_path =
        std::ffi::CString::new(format!("/dev/{tty}")).map_err(|_| BootstrapError::RemoteSession)?;
    let mut descriptor_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let mut path_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, descriptor_stat.as_mut_ptr()) } != 0
        || unsafe { libc::stat(tty_path.as_ptr(), path_stat.as_mut_ptr()) } != 0
    {
        return Err(BootstrapError::RemoteSession);
    }
    let descriptor_stat = unsafe { descriptor_stat.assume_init() };
    let path_stat = unsafe { path_stat.assume_init() };
    if descriptor_stat.st_uid != unsafe { libc::geteuid() }
        || descriptor_stat.st_dev != path_stat.st_dev
        || descriptor_stat.st_ino != path_stat.st_ino
        || descriptor_stat.st_rdev != path_stat.st_rdev
    {
        return Err(BootstrapError::RemoteSession);
    }
    let who = std::process::Command::new("/usr/bin/who")
        .output()
        .map_err(|_| BootstrapError::RemoteSession)?;
    if !who.status.success() {
        return Err(BootstrapError::RemoteSession);
    }
    let who_output = String::from_utf8_lossy(&who.stdout);
    let username = effective_unix_username()?;
    let ps = if Path::new("/bin/ps").is_file() {
        "/bin/ps"
    } else {
        "/usr/bin/ps"
    };
    let mut pid = unsafe { libc::getppid() };
    let mut ancestry = Vec::new();
    for _ in 0..32 {
        if pid <= 1 {
            break;
        }
        let output = std::process::Command::new(ps)
            .args(["-o", "ppid=", "-o", "comm=", "-p", &pid.to_string()])
            .output()
            .map_err(|_| BootstrapError::RemoteSession)?;
        if !output.status.success() {
            return Err(BootstrapError::RemoteSession);
        }
        let line = String::from_utf8_lossy(&output.stdout);
        let mut fields = line.split_whitespace();
        let parent = fields
            .next()
            .and_then(|value| value.parse::<i32>().ok())
            .ok_or(BootstrapError::RemoteSession)?;
        let command = fields.next().unwrap_or_default().to_ascii_lowercase();
        ancestry.push(command);
        pid = parent;
    }
    if !unix_login_is_affirmatively_local(tty, &username, &who_output)
        || unix_ancestry_is_remote(&ancestry)
    {
        return Err(BootstrapError::RemoteSession);
    }
    Ok(())
}

#[cfg(unix)]
fn unix_ancestry_is_remote(ancestry: &[String]) -> bool {
    ancestry
        .iter()
        .any(|command| command.contains("sshd") || command.contains("mosh-server"))
}

#[cfg(unix)]
fn effective_unix_username() -> Result<String, BootstrapError> {
    let requested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let size = if requested <= 0 {
        16 * 1024
    } else {
        usize::try_from(requested)
            .unwrap_or(16 * 1024)
            .clamp(1024, 1024 * 1024)
    };
    let mut buffer = vec![0u8; size];
    let mut passwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            passwd.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(BootstrapError::RemoteSession);
    }
    let passwd = unsafe { passwd.assume_init() };
    if passwd.pw_name.is_null() {
        return Err(BootstrapError::RemoteSession);
    }
    let username = unsafe { std::ffi::CStr::from_ptr(passwd.pw_name) }
        .to_str()
        .map_err(|_| BootstrapError::RemoteSession)?;
    if username.is_empty() {
        return Err(BootstrapError::RemoteSession);
    }
    Ok(username.to_owned())
}

#[cfg(unix)]
fn unix_login_is_affirmatively_local(tty: &str, username: &str, who: &str) -> bool {
    let matching: Vec<Vec<&str>> = who
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.get(1).copied() == Some(tty))
        .collect();
    if matching.len() != 1 {
        return false;
    }
    let fields = &matching[0];
    if fields.len() < 4 || fields[0] != username {
        return false;
    }
    let host_at = if valid_iso_login_date(fields[2]) && valid_login_time(fields[3]) {
        4
    } else if matches!(
        fields[2],
        "Jan"
            | "Feb"
            | "Mar"
            | "Apr"
            | "May"
            | "Jun"
            | "Jul"
            | "Aug"
            | "Sep"
            | "Oct"
            | "Nov"
            | "Dec"
    ) && fields[3]
        .parse::<u8>()
        .is_ok_and(|day| (1..=31).contains(&day))
        && fields.get(4).is_some_and(|time| valid_login_time(time))
    {
        5
    } else {
        return false;
    };
    match fields.get(host_at..) {
        Some([]) => true,
        Some([host]) => matches!(*host, "(:0)" | "(localhost)" | "(127.0.0.1)" | "(::1)"),
        _ => false,
    }
}

#[cfg(unix)]
fn valid_iso_login_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

#[cfg(unix)]
fn valid_login_time(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|hour| hour < 24)
        && minute.parse::<u8>().is_ok_and(|minute| minute < 60)
}

#[cfg(windows)]
fn platform_owner_terminal() -> Result<(std::fs::File, std::fs::File), BootstrapError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetConsoleProcessList};
    use windows_sys::Win32::System::RemoteDesktop::{
        ProcessIdToSessionId, WTS_CURRENT_SERVER_HANDLE, WTSClientProtocolType, WTSFreeMemory,
        WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };
    let input = std::fs::OpenOptions::new()
        .read(true)
        .open("CONIN$")
        .map_err(|_| BootstrapError::NoControllingTty)?;
    let output = std::fs::OpenOptions::new()
        .write(true)
        .open("CONOUT$")
        .map_err(|_| BootstrapError::NoControllingTty)?;
    let mut mode = 0u32;
    if unsafe { GetConsoleMode(input.as_raw_handle() as _, &mut mode) } == 0
        || unsafe { GetConsoleMode(output.as_raw_handle() as _, &mut mode) } == 0
    {
        return Err(BootstrapError::NoControllingTty);
    }
    let pid = unsafe { GetCurrentProcessId() };
    let mut session = 0u32;
    if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
        return Err(BootstrapError::RemoteSession);
    }
    let active_console_session = unsafe { WTSGetActiveConsoleSessionId() };
    let mut protocol_buffer = std::ptr::null_mut();
    let mut protocol_bytes = 0u32;
    let queried = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            session,
            WTSClientProtocolType,
            &mut protocol_buffer,
            &mut protocol_bytes,
        )
    };
    if queried == 0 || protocol_buffer.is_null() {
        return Err(BootstrapError::RemoteSession);
    }
    if protocol_bytes < std::mem::size_of::<u16>() as u32 {
        unsafe { WTSFreeMemory(protocol_buffer.cast()) };
        return Err(BootstrapError::RemoteSession);
    }
    let protocol = unsafe { *(protocol_buffer as *const u16) };
    unsafe { WTSFreeMemory(protocol_buffer.cast()) };
    let mut console_processes = [0u32; 64];
    let process_count = unsafe {
        GetConsoleProcessList(
            console_processes.as_mut_ptr(),
            console_processes.len() as u32,
        )
    };
    if process_count == 0
        || !console_processes[..usize::min(process_count as usize, console_processes.len())]
            .contains(&pid)
    {
        return Err(BootstrapError::RemoteSession);
    }
    let foreground_window = unsafe { GetForegroundWindow() };
    let mut foreground_pid = 0u32;
    if foreground_window == 0
        || unsafe { GetWindowThreadProcessId(foreground_window, &mut foreground_pid) } == 0
        || foreground_pid == 0
    {
        return Err(BootstrapError::RemoteSession);
    }
    let mut foreground_session = 0u32;
    if unsafe { ProcessIdToSessionId(foreground_pid, &mut foreground_session) } == 0
        || !windows_session_is_local(
            session,
            active_console_session,
            protocol,
            foreground_session,
        )
    {
        return Err(BootstrapError::RemoteSession);
    }
    unsafe fn token_user(process: isize) -> Result<Vec<u8>, BootstrapError> {
        let mut token = 0isize;
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(BootstrapError::RemoteSession);
        }
        let mut size = 0u32;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size) };
        let mut buffer = vec![0u8; size as usize];
        if size == 0
            || unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut size,
                )
            } == 0
        {
            unsafe { CloseHandle(token) };
            return Err(BootstrapError::RemoteSession);
        }
        unsafe { CloseHandle(token) };
        Ok(buffer)
    }
    let foreground_process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, foreground_pid) };
    if foreground_process == 0 {
        return Err(BootstrapError::RemoteSession);
    }
    let current_user = unsafe { token_user(GetCurrentProcess()) }?;
    let foreground_user = unsafe { token_user(foreground_process) };
    unsafe { CloseHandle(foreground_process) };
    let foreground_user = foreground_user?;
    let current = unsafe { std::ptr::read_unaligned(current_user.as_ptr() as *const TOKEN_USER) };
    let foreground =
        unsafe { std::ptr::read_unaligned(foreground_user.as_ptr() as *const TOKEN_USER) };
    if unsafe { EqualSid(current.User.Sid, foreground.User.Sid) } == 0 {
        return Err(BootstrapError::RemoteSession);
    }
    Ok((input, output))
}

#[cfg(windows)]
fn windows_session_is_local(
    process_session: u32,
    active_console_session: u32,
    client_protocol: u16,
    foreground_session: u32,
) -> bool {
    process_session == active_console_session
        && client_protocol == 0
        && foreground_session == process_session
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    struct TestReceiptSigner(SigningKey);
    impl ReceiptSigner for TestReceiptSigner {
        fn key_id(&self) -> &str {
            "test-bootstrap-receipt"
        }
        fn public_key(&self) -> [u8; 32] {
            self.0.verifying_key().to_bytes()
        }
        fn preflight(&self) -> Result<(), crate::receipt::ReceiptError> {
            Ok(())
        }
        fn sign(&self, message: &[u8]) -> Result<[u8; 64], crate::receipt::ReceiptError> {
            Ok(self.0.sign(message).to_bytes())
        }
    }

    #[test]
    fn interactive_proof_requires_platform_attestation_and_exact_phrase() {
        let mut output = Vec::new();
        let mut accepted = std::io::Cursor::new(b"BOOTSTRAP root-1\n".to_vec());
        attest_interactive_owner_with("root-1", || Ok(()), &mut accepted, &mut output).unwrap();
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("BOOTSTRAP root-1")
        );

        let mut ignored_input = std::io::Cursor::new(b"BOOTSTRAP root-1\n".to_vec());
        assert!(matches!(
            attest_interactive_owner_with(
                "root-1",
                || Err(BootstrapError::RemoteSession),
                &mut ignored_input,
                &mut Vec::new(),
            ),
            Err(BootstrapError::RemoteSession)
        ));
        let mut wrong = std::io::Cursor::new(b"yes\n".to_vec());
        assert!(matches!(
            attest_interactive_owner_with("root-1", || Ok(()), &mut wrong, &mut Vec::new(),),
            Err(BootstrapError::ConfirmationRequired)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn os_login_probe_requires_one_well_formed_local_owner_row() {
        assert!(unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "owner pts/4 2026-08-30 10:00\n",
        ));
        assert!(unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "owner pts/4 Aug 30 10:00 (localhost)\n",
        ));
        assert!(!unix_login_is_affirmatively_local("pts/4", "owner", ""));
        assert!(!unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "owner pts/4 malformed\n",
        ));
        assert!(!unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "other pts/4 2026-08-30 10:00\n",
        ));
        assert!(!unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "owner pts/4 2026-08-30 10:00 (203.0.113.9)\n",
        ));
        assert!(!unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "owner pts/4 2026-08-30 10:00\nowner pts/4 2026-08-30 10:01\n",
        ));
        assert!(!unix_login_is_affirmatively_local(
            "pts/4",
            "owner",
            "owner pts/5 2026-08-30 10:00\n",
        ));
        assert!(unix_ancestry_is_remote(&[
            "bash".into(),
            "sshd-session".into()
        ]));
        assert!(!unix_ancestry_is_remote(&["bash".into(), "systemd".into()]));
    }

    #[cfg(windows)]
    #[test]
    fn wts_probe_requires_console_protocol_and_foreground_session() {
        assert!(windows_session_is_local(1, 1, 0, 1));
        assert!(!windows_session_is_local(1, 1, 2, 1));
        assert!(!windows_session_is_local(2, 1, 0, 2));
        assert!(!windows_session_is_local(1, 1, 0, 2));
    }

    #[test]
    fn attested_bootstrap_is_exactly_once() {
        // GitHub's RUNNER_TEMP and checkout ancestry can be writable by other
        // principals, which production correctly rejects as an insecure home.
        #[cfg(unix)]
        let home = {
            use std::os::unix::fs::PermissionsExt;
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in(std::env::var_os("HOME").unwrap())
                .unwrap()
        };
        #[cfg(windows)]
        let home = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        {
            let script = r#"
$directory = [System.IO.DirectoryInfo]::new($env:NANO_TEST_SECURE_HOME)
$acl = $directory.GetAccessControl()
$acl.SetAccessRuleProtection($true, $false)
foreach ($rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleSpecific($rule) }
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$owner = $acl.GetOwner([System.Security.Principal.SecurityIdentifier])
if ($owner -ne $sid) { $acl.SetOwner($sid) }
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
  $sid,
  [System.Security.AccessControl.FileSystemRights]::FullControl,
  [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit',
  [System.Security.AccessControl.PropagationFlags]::None,
[System.Security.AccessControl.AccessControlType]::Allow)
[void]$acl.AddAccessRule($rule)
$directory.SetAccessControl($acl)
"#;
            assert!(
                std::process::Command::new("powershell.exe")
                    .args(["-NoProfile", "-Command", script])
                    .env("NANO_TEST_SECURE_HOME", home.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        let first = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        let first_receipt = first.bootstrap_receipt().unwrap().to_vec();
        verify_bootstrap_receipt(&first_receipt, &signer.public_key()).unwrap();
        let mut tampered = first_receipt.clone();
        let index = tampered.iter().position(|byte| *byte == b'r').unwrap();
        tampered[index] = b's';
        assert!(verify_bootstrap_receipt(&tampered, &signer.public_key()).is_err());
        let receipt_text = String::from_utf8(first_receipt.clone()).unwrap();
        assert!(!receipt_text.contains("reference"));
        assert!(!receipt_text.contains("private"));
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
                &signer,
            ),
            Err(BootstrapError::Authority(AuthorityError::Contention))
        ));
        drop(first);
        let replayed = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        assert_eq!(replayed.bootstrap_receipt(), Some(first_receipt.as_slice()));
        assert_eq!(
            std::fs::read_to_string(home.path().join("activation/authority.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn attested_bootstrap_rejects_role_key_reuse() {
        let home = tempfile::tempdir().unwrap();
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [1; 32], signer.public_key(), [4; 32], "root-1"),
                &signer,
            ),
            Err(BootstrapError::RoleKeyReuse)
        ));
        assert!(!home.path().join("activation").exists());
    }

    #[cfg(unix)]
    #[test]
    fn attested_bootstrap_rejects_insecure_home() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            bootstrap_attested(
                home.path(),
                BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
                &signer,
            ),
            Err(BootstrapError::InsecureHome)
        ));
        assert!(!home.path().join("activation").exists());
    }

    #[cfg(unix)]
    #[test]
    fn attested_bootstrap_repairs_torn_initial_record() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::var_os("HOME").unwrap())
            .unwrap();
        let activation = home.path().join("activation");
        std::fs::create_dir_all(&activation).unwrap();
        std::fs::write(activation.join("authority.jsonl"), b"{\"record_type\":").unwrap();
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        let store = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        assert!(store.bootstrap_receipt().is_some());
        assert_eq!(
            std::fs::read_to_string(activation.join("authority.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn residual_projection_and_prebootstrap_records_block_recovery() {
        use std::os::unix::fs::PermissionsExt;
        for journal in [None, Some(b"{\"record_type\":".as_slice())] {
            let home = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir_in(std::env::var_os("HOME").unwrap())
                .unwrap();
            let activation = home.path().join("activation");
            std::fs::create_dir_all(&activation).unwrap();
            std::fs::write(activation.join("authority.db"), b"residual-projection").unwrap();
            if let Some(journal) = journal {
                std::fs::write(activation.join("authority.jsonl"), journal).unwrap();
            }
            let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
            assert!(matches!(
                bootstrap_attested(
                    home.path(),
                    BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1",),
                    &signer,
                ),
                Err(BootstrapError::AlreadyBootstrapped)
            ));
        }

        let home = tempfile::tempdir().unwrap();
        let activation = home.path().join("activation");
        std::fs::create_dir_all(&activation).unwrap();
        std::fs::write(
            activation.join("authority.jsonl"),
            b"{\"record_type\":\"future\",\"sequence\":1}\n",
        )
        .unwrap();
        assert!(matches!(
            AuthorityStore::open(home.path()),
            Err(AuthorityError::InvalidRecord)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn killed_between_bootstrap_and_receipt_restarts_idempotently() {
        use std::os::unix::fs::PermissionsExt;
        if let Some(home) = std::env::var_os("NANO_BOOTSTRAP_KILL_HOME") {
            let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
            let snapshot = AuthoritySnapshot::empty("root-1", [1; 32])
                .with_recovery_key([2; 32])
                .with_service_keys(signer.public_key(), [4; 32]);
            let receipt = sign_bootstrap_receipt(&snapshot, &signer).unwrap();
            let _ = AuthorityStore::bootstrap_initial_with_fault(
                Path::new(&home),
                snapshot,
                receipt,
                || std::process::abort(),
            );
            unreachable!();
        }
        let home = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::var_os("HOME").unwrap())
            .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("admin::tests::killed_between_bootstrap_and_receipt_restarts_idempotently")
            .arg("--nocapture")
            .env("NANO_BOOTSTRAP_KILL_HOME", home.path())
            .status()
            .unwrap();
        assert!(!status.success());
        let signer = TestReceiptSigner(SigningKey::from_bytes(&[3; 32]));
        let store = bootstrap_attested(
            home.path(),
            BootstrapRequest::new([1; 32], [2; 32], signer.public_key(), [4; 32], "root-1"),
            &signer,
        )
        .unwrap();
        verify_bootstrap_receipt(store.bootstrap_receipt().unwrap(), &signer.public_key()).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.path().join("activation/authority.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}
