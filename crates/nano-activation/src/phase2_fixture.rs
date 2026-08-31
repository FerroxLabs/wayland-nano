//! Evidence-only Phase-2 authority fixture provisioned through Nano reducers.

use crate::admin::{apply_signed_admin, sign_bootstrap_receipt};
use crate::authority::{AuthorityCommand, AuthoritySnapshot};
use crate::enablement::{EnablementCommand, EnablementFault, EnablementStore};
use crate::receipt::{ArtifactIdentity, ReceiptError, ReceiptSigner};
use crate::store::AuthorityStore;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{SecondsFormat, Utc};
use ed25519_dalek::{Signer as _, SigningKey};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const TARGET_SOURCE: &str = "288de9ed3185c91717f8f777c9975c784709e824";
const TARGET_LOCK: &str = "3d6ec29f3b19e0b3778a5de222418ec497eaf79be8e93a92dd120d986bdb930a";
const ADMIN_ID: &str = "phase2-fixture-root";
const PROJECT_ID: &str = "phase2-project";
const DESKTOP_SUBJECT: &str = "phase2-agent";
const DESKTOP_PRINCIPAL: &str = "main";
const CLI_SUBJECT: &str = "main";
const CLI_PRINCIPAL: &str = "main";
const DESKTOP_ISSUER_ID: &str = "wayland-desktop";
const DESKTOP_KEY_ID: &str = "desktop-phase2-fixture-key";
const CLI_ISSUER_ID: &str = "local-cli";
const CLI_KEY_ID: &str = "local-cli-phase2-fixture-key";

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("Phase 2 fixture provisioning is supported only on Windows")]
    UnsupportedPlatform,
    #[error(
        "expected --evidence-root, --home, --private-handoff, --preparation-path, and --not-after exactly once"
    )]
    InvalidArguments,
    #[error("fixture paths are not owner-only canonical local children")]
    InvalidPath,
    #[error("fixture preparation did not match the frozen schema and tuple")]
    InvalidPreparation,
    #[error("fixture checkout HEAD did not match the frozen source")]
    InvalidCheckoutHead,
    #[error("fixture checkout remote did not match the frozen repository")]
    InvalidCheckoutRemote,
    #[error("fixture checkout was dirty")]
    InvalidCheckoutDirty,
    #[error("fixture checkout submodule state was not empty")]
    InvalidCheckoutSubmodule,
    #[error("fixture checkout lock did not match the frozen lock")]
    InvalidCheckoutLock,
    #[error("fixture checkout command could not be evaluated")]
    InvalidCheckoutCommand,
    #[error("fixture executable identity was unstable or mismatched")]
    InvalidExecutable,
    #[error("artifact identity or UTC expiry is invalid")]
    InvalidIdentity,
    #[error("fixture I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("fixture authority operation failed")]
    Authority,
    #[error("fixture serialization failed")]
    Serialization,
}

#[derive(Debug)]
struct Arguments {
    evidence_root: PathBuf,
    home: PathBuf,
    private_handoff: PathBuf,
    artifact: ArtifactIdentity,
    executable_size: u64,
    not_after: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    schema: String,
    frozen_checkout_path: PathBuf,
    executable_path: PathBuf,
    source_commit_sha: String,
    cargo_lock_sha256: String,
    executable_sha256: String,
}

#[derive(Serialize)]
struct PublicOutput {
    schema: &'static str,
    home_identity_sha256: String,
    private_handoff_sha256: String,
    receipt_signer_public_key: String,
    desktop_issuer_id: &'static str,
    desktop_issuer_key_id: &'static str,
    desktop_subject_id: &'static str,
    desktop_principal_id: &'static str,
    local_cli_issuer_id: &'static str,
    local_cli_issuer_key_id: &'static str,
    local_cli_subject_id: &'static str,
    local_cli_principal_id: &'static str,
    project_id: &'static str,
    artifact: ArtifactIdentity,
    executable_size: u64,
    enablement_not_after: String,
    helper_source_commit_sha: &'static str,
    helper_cargo_lock_sha256: &'static str,
    helper_source_dirty: bool,
}

#[derive(Serialize)]
struct PrivateHandoff {
    schema: &'static str,
    home: PathBuf,
    bootstrap_receipt_path: PathBuf,
    receipt_signer_key_reference: PathBuf,
    desktop_issuer_seed_path: PathBuf,
    local_cli_seed_path: PathBuf,
    local_cli_key_reference: PathBuf,
}

struct FixtureReceiptSigner {
    key: SigningKey,
    key_id: String,
}
impl FixtureReceiptSigner {
    fn new(key: SigningKey) -> Self {
        let fingerprint = Sha256::digest(key.verifying_key().to_bytes());
        Self {
            key,
            key_id: format!("receipt-ed25519-{}", hex(&fingerprint[..16])),
        }
    }
}
impl ReceiptSigner for FixtureReceiptSigner {
    fn key_id(&self) -> &str {
        &self.key_id
    }
    fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }
    fn preflight(&self) -> Result<(), ReceiptError> {
        Ok(())
    }
    fn sign(&self, message: &[u8]) -> Result<[u8; 64], ReceiptError> {
        Ok(self.key.sign(message).to_bytes())
    }
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<String, FixtureError> {
    #[cfg(windows)]
    {
        provision(parse_arguments(args)?)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        Err(FixtureError::UnsupportedPlatform)
    }
}

fn parse_arguments(args: impl IntoIterator<Item = OsString>) -> Result<Arguments, FixtureError> {
    let mut root = None;
    let mut home = None;
    let mut handoff = None;
    let mut preparation = None;
    let mut expiry = None;
    let mut input = args.into_iter();
    while let Some(flag) = input.next() {
        let value = input.next().ok_or(FixtureError::InvalidArguments)?;
        let slot = match flag.to_str() {
            Some("--evidence-root") => &mut root,
            Some("--home") => &mut home,
            Some("--private-handoff") => &mut handoff,
            Some("--preparation-path") => &mut preparation,
            Some("--not-after") => &mut expiry,
            _ => return Err(FixtureError::InvalidArguments),
        };
        if slot.replace(value).is_some() {
            return Err(FixtureError::InvalidArguments);
        }
    }
    let evidence_root =
        canonical_owner_local_dir(PathBuf::from(root.ok_or(FixtureError::InvalidArguments)?))?;
    let home = absent_direct_child(
        &evidence_root,
        PathBuf::from(home.ok_or(FixtureError::InvalidArguments)?),
    )?;
    let private_handoff = absent_direct_child(
        &evidence_root,
        PathBuf::from(handoff.ok_or(FixtureError::InvalidArguments)?),
    )?;
    if home == private_handoff {
        return Err(FixtureError::InvalidPath);
    }
    let preparation_path = canonical_owner_local_file(PathBuf::from(
        preparation.ok_or(FixtureError::InvalidArguments)?,
    ))?;
    if preparation_path.parent() != Some(evidence_root.as_path()) {
        return Err(FixtureError::InvalidPath);
    }
    let preparation: Preparation = serde_json::from_slice(&std::fs::read(preparation_path)?)
        .map_err(|_| FixtureError::InvalidPreparation)?;
    if preparation.schema != "wayland.desktop.nano-phase2-fixture-preparation/v1"
        || preparation.source_commit_sha != TARGET_SOURCE
        || preparation.cargo_lock_sha256 != TARGET_LOCK
    {
        return Err(FixtureError::InvalidPreparation);
    }
    let frozen_checkout = canonical_local(preparation.frozen_checkout_path, true)?;
    if !frozen_checkout.starts_with(&evidence_root) {
        return Err(FixtureError::InvalidPath);
    }
    verify_checkout(&frozen_checkout)?;
    let executable = canonical_local(preparation.executable_path, false)?;
    if !executable.starts_with(&evidence_root) {
        return Err(FixtureError::InvalidPath);
    }
    let (executable_sha256, executable_size) = stable_file_hash(&executable)?;
    if executable_sha256 != preparation.executable_sha256
        || !file_contains_target_identity(&executable)?
    {
        return Err(FixtureError::InvalidExecutable);
    }
    let artifact = ArtifactIdentity {
        source_commit_sha: TARGET_SOURCE.into(),
        cargo_lock_sha256: TARGET_LOCK.into(),
        executable_sha256,
    };
    let not_after = expiry
        .ok_or(FixtureError::InvalidArguments)?
        .into_string()
        .map_err(|_| FixtureError::InvalidArguments)?;
    if !utc_seconds(&not_after) {
        return Err(FixtureError::InvalidIdentity);
    }
    Ok(Arguments {
        evidence_root,
        home,
        private_handoff,
        artifact,
        executable_size,
        not_after,
    })
}

fn provision(args: Arguments) -> Result<String, FixtureError> {
    let staging = unique_staging(&args.evidence_root)?;
    let result = provision_staging(&args, &staging);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn provision_staging(args: &Arguments, staging: &Path) -> Result<String, FixtureError> {
    let keys = staging.join("fixture-keys");
    create_owner_only_dir(&keys)?;
    let admin = signing_key()?;
    let recovery = signing_key()?;
    let receipt = FixtureReceiptSigner::new(signing_key()?);
    let local_cli = signing_key()?;
    let desktop = signing_key()?;
    write_seed(&keys, "admin-root.seed", &admin)?;
    write_seed(&keys, "recovery-root.seed", &recovery)?;
    write_seed(&keys, "receipt-signer.seed", &receipt.key)?;
    write_seed(&keys, "local-cli.seed", &local_cli)?;
    write_seed(&keys, "desktop-issuer.seed", &desktop)?;
    let activation = staging.join("activation");
    create_owner_only_dir(&activation)?;
    let final_keys = args.home.join("fixture-keys");
    write_key_reference(
        &activation,
        "receipt-signer.keyref",
        &final_keys.join("receipt-signer.seed"),
        "receipt_signer",
    )?;
    write_key_reference(
        &activation,
        "local-cli.keyref",
        &final_keys.join("local-cli.seed"),
        "local_cli_issuer",
    )?;
    let snapshot = AuthoritySnapshot::empty(ADMIN_ID, admin.verifying_key().to_bytes())
        .with_recovery_key(recovery.verifying_key().to_bytes())
        .with_service_keys(receipt.public_key(), local_cli.verifying_key().to_bytes());
    let bootstrap_receipt =
        sign_bootstrap_receipt(&snapshot, &receipt).map_err(|_| FixtureError::Authority)?;
    write_owner_only(
        &activation.join("bootstrap-receipt.json"),
        &bootstrap_receipt,
    )?;
    let mut store = AuthorityStore::bootstrap_initial(staging, snapshot, bootstrap_receipt)
        .map_err(|_| FixtureError::Authority)?;
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    apply_issuer(
        &mut store,
        &admin,
        IssuerSpec {
            issuer: DESKTOP_ISSUER_ID,
            key_id: DESKTOP_KEY_ID,
            subject: DESKTOP_SUBJECT,
            principal: DESKTOP_PRINCIPAL,
            public_key: desktop.verifying_key().to_bytes(),
            suffix: "desktop",
        },
        &now,
        &args.not_after,
    )?;
    apply_issuer(
        &mut store,
        &admin,
        IssuerSpec {
            issuer: CLI_ISSUER_ID,
            key_id: CLI_KEY_ID,
            subject: CLI_SUBJECT,
            principal: CLI_PRINCIPAL,
            public_key: local_cli.verifying_key().to_bytes(),
            suffix: "cli",
        },
        &now,
        &args.not_after,
    )?;
    let authority = store.snapshot().map_err(|_| FixtureError::Authority)?;
    drop(store);
    let enablement = EnablementCommand {
        operation_id: "fixture-enable-exact-artifact".into(),
        enabled: true,
        artifact: args.artifact.clone(),
        admin_epoch: authority.admin_epoch,
        issuer_epoch: 1,
        grant_epoch: 1,
        revocation_epoch: 1,
        not_after: args.not_after.clone(),
    };
    let enabled = EnablementStore::open(staging).map_err(|_| FixtureError::Authority)?;
    let before = enabled
        .state_digest()
        .map_err(|_| FixtureError::Authority)?;
    let raw = sign_admin_envelope(
        &admin,
        "enable_artifact",
        &enablement.operation_id,
        "fixture-admin-nonce-enable",
        &before,
        &enablement.digest(),
        (&now, &args.not_after),
    )?;
    enabled
        .apply_signed(&raw, &enablement, &now, EnablementFault::None)
        .map_err(|_| FixtureError::Authority)?;
    enabled
        .require_enabled(&args.artifact, [authority.admin_epoch, 1, 1, 1], &now)
        .map_err(|_| FixtureError::Authority)?;
    drop(enabled);
    drop(admin);
    drop(recovery);
    drop(receipt);
    drop(local_cli);
    drop(desktop);
    verify_replay(staging, &args.artifact, &now)?;
    if test_fault("after_reducers") {
        return Err(FixtureError::Authority);
    }

    let handoff = PrivateHandoff {
        schema: "wayland.nano.phase2-fixture-private/v1",
        home: args.home.clone(),
        bootstrap_receipt_path: args.home.join("activation/bootstrap-receipt.json"),
        receipt_signer_key_reference: args.home.join("activation/receipt-signer.keyref"),
        desktop_issuer_seed_path: args.home.join("fixture-keys/desktop-issuer.seed"),
        local_cli_seed_path: args.home.join("fixture-keys/local-cli.seed"),
        local_cli_key_reference: args.home.join("activation/local-cli.keyref"),
    };
    let handoff_bytes = serde_jcs::to_vec(&handoff).map_err(|_| FixtureError::Serialization)?;
    write_owner_only(&staging.join("private-handoff.publish"), &handoff_bytes)?;
    crate::key_provider::audit_owner_only_path(staging).map_err(|_| FixtureError::InvalidPath)?;
    crate::key_provider::audit_owner_only_path(&staging.join("private-handoff.publish"))
        .map_err(|_| FixtureError::InvalidPath)?;
    let helper = crate::build_identity::compiled();
    let output = PublicOutput {
        schema: "wayland.nano.phase2-fixture/v2",
        home_identity_sha256: hex(&Sha256::digest(
            args.home.as_os_str().to_string_lossy().as_bytes(),
        )),
        private_handoff_sha256: hex(&Sha256::digest(&handoff_bytes)),
        receipt_signer_public_key: URL_SAFE_NO_PAD.encode(
            authority
                .receipt_signer_public_key
                .ok_or(FixtureError::Authority)?,
        ),
        desktop_issuer_id: DESKTOP_ISSUER_ID,
        desktop_issuer_key_id: DESKTOP_KEY_ID,
        desktop_subject_id: DESKTOP_SUBJECT,
        desktop_principal_id: DESKTOP_PRINCIPAL,
        local_cli_issuer_id: CLI_ISSUER_ID,
        local_cli_issuer_key_id: CLI_KEY_ID,
        local_cli_subject_id: CLI_SUBJECT,
        local_cli_principal_id: CLI_PRINCIPAL,
        project_id: PROJECT_ID,
        artifact: args.artifact.clone(),
        executable_size: args.executable_size,
        enablement_not_after: args.not_after.clone(),
        helper_source_commit_sha: helper.source_commit_sha,
        helper_cargo_lock_sha256: helper.cargo_lock_sha256,
        helper_source_dirty: helper.source_dirty,
    };
    let public = serde_jcs::to_string(&output).map_err(|_| FixtureError::Serialization)?;
    std::fs::rename(staging, &args.home)?;
    let moved_handoff = args.home.join("private-handoff.publish");
    if test_fault("after_home_publish") {
        let _ = std::fs::remove_dir_all(&args.home);
        return Err(FixtureError::Io(std::io::Error::other(
            "injected publication failure",
        )));
    }
    if let Err(error) = std::fs::rename(&moved_handoff, &args.private_handoff) {
        let _ = std::fs::remove_dir_all(&args.home);
        return Err(error.into());
    }
    Ok(public)
}

struct IssuerSpec<'a> {
    issuer: &'a str,
    key_id: &'a str,
    subject: &'a str,
    principal: &'a str,
    public_key: [u8; 32],
    suffix: &'a str,
}
fn apply_issuer(
    store: &mut AuthorityStore,
    admin: &SigningKey,
    spec: IssuerSpec<'_>,
    now: &str,
    expiry: &str,
) -> Result<(), FixtureError> {
    apply_command(
        store,
        admin,
        AuthorityCommand::EnrollIssuer {
            operation_id: format!("fixture-enroll-{}", spec.suffix),
            issuer_id: spec.issuer.into(),
            subject_id: spec.subject.into(),
            principal_id: spec.principal.into(),
            key_id: spec.key_id.into(),
            public_key: spec.public_key,
        },
        now,
        expiry,
        &format!("fixture-admin-nonce-{}-enroll", spec.suffix),
    )?;
    apply_command(
        store,
        admin,
        AuthorityCommand::GrantProject {
            operation_id: format!("fixture-grant-{}", spec.suffix),
            issuer_id: spec.issuer.into(),
            subject_id: spec.subject.into(),
            principal_id: spec.principal.into(),
            project_id: PROJECT_ID.into(),
        },
        now,
        expiry,
        &format!("fixture-admin-nonce-{}-grant", spec.suffix),
    )
}

fn apply_command(
    store: &mut AuthorityStore,
    admin: &SigningKey,
    command: AuthorityCommand,
    now: &str,
    not_after: &str,
    nonce: &str,
) -> Result<(), FixtureError> {
    let snapshot = store.snapshot().map_err(|_| FixtureError::Authority)?;
    let before = snapshot.digest().map_err(|_| FixtureError::Authority)?;
    let expires = chrono::DateTime::parse_from_rfc3339(not_after)
        .map_err(|_| FixtureError::InvalidIdentity)?
        .timestamp();
    let after = snapshot
        .preview_admin_transaction(&command, nonce, expires)
        .and_then(|s| s.digest())
        .map_err(|_| FixtureError::Authority)?;
    let operation = match command {
        AuthorityCommand::EnrollIssuer { .. } => "enroll_issuer",
        AuthorityCommand::GrantProject { .. } => "grant_project",
        _ => return Err(FixtureError::Authority),
    };
    let raw = sign_admin_envelope(
        admin,
        operation,
        command.operation_id(),
        nonce,
        &before,
        &after,
        (now, not_after),
    )?;
    apply_signed_admin(store, &raw, command, now).map_err(|_| FixtureError::Authority)
}

fn sign_admin_envelope(
    key: &SigningKey,
    operation: &str,
    operation_id: &str,
    nonce: &str,
    before: &str,
    after: &str,
    validity: (&str, &str),
) -> Result<Vec<u8>, FixtureError> {
    let (now, not_after) = validity;
    let mut value = json!({"admin_epoch":1,"admin_id":ADMIN_ID,"after_digest":after,"alg":"Ed25519",
        "before_digest":before,"issued_at":now,"key_id":"phase2-fixture-admin-key","nonce":nonce,
        "not_after":not_after,"operation":operation,"operation_id":operation_id,
        "reason":"Phase 2 exact-artifact evidence fixture","schema":"wayland.nano.admin-request/v1"});
    let canonical = serde_jcs::to_vec(&value).map_err(|_| FixtureError::Serialization)?;
    let mut message = b"WAYLAND-NANO-ADMIN\0v1\0".to_vec();
    message.extend_from_slice(&canonical);
    value
        .as_object_mut()
        .ok_or(FixtureError::Serialization)?
        .insert(
            "signature".into(),
            Value::String(URL_SAFE_NO_PAD.encode(key.sign(&message).to_bytes())),
        );
    serde_jcs::to_vec(&value).map_err(|_| FixtureError::Serialization)
}

fn verify_replay(home: &Path, artifact: &ArtifactIdentity, now: &str) -> Result<(), FixtureError> {
    let store = AuthorityStore::open(home).map_err(|_| FixtureError::Authority)?;
    let snapshot = store.snapshot().map_err(|_| FixtureError::Authority)?;
    store
        .authorize(
            DESKTOP_ISSUER_ID,
            DESKTOP_KEY_ID,
            DESKTOP_SUBJECT,
            DESKTOP_PRINCIPAL,
            PROJECT_ID,
        )
        .map_err(|_| FixtureError::Authority)?;
    store
        .authorize(
            CLI_ISSUER_ID,
            CLI_KEY_ID,
            CLI_SUBJECT,
            CLI_PRINCIPAL,
            PROJECT_ID,
        )
        .map_err(|_| FixtureError::Authority)?;
    drop(store);
    EnablementStore::open(home)
        .map_err(|_| FixtureError::Authority)?
        .require_enabled(artifact, [snapshot.admin_epoch, 1, 1, 1], now)
        .map_err(|_| FixtureError::Authority)
}

fn verify_checkout(checkout: &Path) -> Result<(), FixtureError> {
    if git_stdout(checkout, &["rev-parse", "HEAD"])? != TARGET_SOURCE {
        return Err(FixtureError::InvalidCheckoutHead);
    }
    if git_stdout(checkout, &["remote", "get-url", "origin"])?
        != "https://github.com/FerroxLabs/wayland-nano.git"
    {
        return Err(FixtureError::InvalidCheckoutRemote);
    }
    if !git_stdout(
        checkout,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty()
    {
        return Err(FixtureError::InvalidCheckoutDirty);
    }
    if !git_stdout(checkout, &["submodule", "status", "--recursive"])?.is_empty() {
        return Err(FixtureError::InvalidCheckoutSubmodule);
    }
    if stable_file_hash(&checkout.join("Cargo.lock"))?.0 != TARGET_LOCK {
        return Err(FixtureError::InvalidCheckoutLock);
    }
    Ok(())
}

fn git_stdout(checkout: &Path, args: &[&str]) -> Result<String, FixtureError> {
    let checkout = external_git_path(checkout);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(FixtureError::InvalidCheckoutCommand);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(windows)]
fn external_git_path(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    text.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_owned(), PathBuf::from)
}
#[cfg(unix)]
fn external_git_path(path: &Path) -> PathBuf {
    path.to_owned()
}

fn file_contains_target_identity(path: &Path) -> Result<bool, FixtureError> {
    let bytes = std::fs::read(path)?;
    Ok(bytes
        .windows(TARGET_SOURCE.len())
        .any(|window| window == TARGET_SOURCE.as_bytes())
        && bytes
            .windows(TARGET_LOCK.len())
            .any(|window| window == TARGET_LOCK.as_bytes()))
}

fn stable_file_hash(path: &Path) -> Result<(String, u64), FixtureError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FixtureError::InvalidPath);
    }
    let mut file = File::open(path)?;
    let before = file_identity(&file)?;
    let size = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let after = file_identity(&file)?;
    let reopened = File::open(path)?;
    if before != after || before != file_identity(&reopened)? {
        return Err(FixtureError::InvalidExecutable);
    }
    Ok((hex(&hasher.finalize()), size))
}
#[cfg(unix)]
fn file_identity(file: &File) -> Result<(u64, u64, u64, i64, i64), FixtureError> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata()?;
    Ok((m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec()))
}
#[cfg(windows)]
fn file_identity(file: &File) -> Result<(u32, u64, u64, u64), FixtureError> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    let write = (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
        | u64::from(info.ftLastWriteTime.dwLowDateTime);
    Ok((info.dwVolumeSerialNumber, index, size, write))
}

fn canonical_owner_local_dir(path: PathBuf) -> Result<PathBuf, FixtureError> {
    let canonical = canonical_local(path, true)?;
    crate::key_provider::audit_owner_only_path(&canonical)
        .map_err(|_| FixtureError::InvalidPath)?;
    Ok(canonical)
}
fn canonical_owner_local_file(path: PathBuf) -> Result<PathBuf, FixtureError> {
    let canonical = canonical_local(path, false)?;
    crate::key_provider::audit_owner_only_path(&canonical)
        .map_err(|_| FixtureError::InvalidPath)?;
    Ok(canonical)
}
fn canonical_local(path: PathBuf, directory: bool) -> Result<PathBuf, FixtureError> {
    if !path.is_absolute() || has_reparse_chain(&path)? {
        return Err(FixtureError::InvalidPath);
    }
    let canonical = std::fs::canonicalize(&path)?;
    if canonical != path
        || has_reparse_chain(&canonical)?
        || std::fs::metadata(&canonical)?.is_dir() != directory
        || !is_local(&canonical)
    {
        return Err(FixtureError::InvalidPath);
    }
    Ok(canonical)
}
fn absent_direct_child(root: &Path, path: PathBuf) -> Result<PathBuf, FixtureError> {
    if !path.is_absolute()
        || path.exists()
        || path.parent() != Some(root)
        || path.file_name().is_none()
    {
        return Err(FixtureError::InvalidPath);
    }
    Ok(path)
}
fn unique_staging(root: &Path) -> Result<PathBuf, FixtureError> {
    for _ in 0..16 {
        let mut random = [0_u8; 16];
        fill_random(&mut random)?;
        let path = root.join(format!(".nano-phase2-staging-{}", hex(&random)));
        match create_owner_only_dir(&path) {
            Ok(()) => return Ok(path),
            Err(FixtureError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(FixtureError::InvalidPath)
}

fn test_fault(point: &str) -> bool {
    cfg!(debug_assertions)
        && std::env::var("NANO_PHASE2_FIXTURE_TEST_FAULT").as_deref() == Ok(point)
}

fn signing_key() -> Result<SigningKey, FixtureError> {
    let mut seed = [0_u8; 32];
    fill_random(&mut seed)?;
    let key = SigningKey::from_bytes(&seed);
    seed.fill(0);
    Ok(key)
}
#[cfg(unix)]
fn fill_random(bytes: &mut [u8]) -> Result<(), FixtureError> {
    File::open("/dev/urandom")?.read_exact(bytes)?;
    Ok(())
}
#[cfg(windows)]
fn fill_random(bytes: &mut [u8]) -> Result<(), FixtureError> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(FixtureError::Io(std::io::Error::other("system RNG failed")))
    }
}
fn write_seed(root: &Path, name: &str, key: &SigningKey) -> Result<(), FixtureError> {
    let mut bytes = key.to_bytes();
    let result = write_owner_only(&root.join(name), &bytes);
    bytes.fill(0);
    result
}
fn write_key_reference(
    root: &Path,
    name: &str,
    seed: &Path,
    role: &str,
) -> Result<(), FixtureError> {
    let bytes = serde_jcs::to_vec(&json!({"provider":"file","reference":seed,"role":role}))
        .map_err(|_| FixtureError::Serialization)?;
    write_owner_only(&root.join(name), &bytes)
}
fn create_owner_only_dir(path: &Path) -> Result<(), FixtureError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)?;
    }
    #[cfg(windows)]
    {
        std::fs::create_dir(path)?;
        crate::key_provider::audit_owner_only_path(path).map_err(|_| FixtureError::InvalidPath)?;
    }
    Ok(())
}
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), FixtureError> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    restrict(path, false)
}
#[cfg(unix)]
fn restrict(path: &Path, directory: bool) -> Result<(), FixtureError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )?;
    Ok(())
}
#[cfg(windows)]
fn restrict(path: &Path, directory: bool) -> Result<(), FixtureError> {
    let _ = directory;
    crate::key_provider::audit_owner_only_path(path).map_err(|_| FixtureError::InvalidPath)
}
#[cfg(unix)]
fn has_reparse_chain(path: &Path) -> Result<bool, FixtureError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
#[cfg(windows)]
fn has_reparse_chain(path: &Path) -> Result<bool, FixtureError> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE: u32 = 0x400;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if matches!(component, std::path::Component::Normal(_))
            && std::fs::symlink_metadata(&current)?.file_attributes() & REPARSE != 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}
#[cfg(unix)]
fn is_local(_path: &Path) -> bool {
    false
}
#[cfg(windows)]
fn is_local(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_FIXED: u32 = 3;
    let text = path.as_os_str().to_string_lossy();
    let root = if text.starts_with(r"\\?\") {
        text.get(4..7)
    } else {
        text.get(0..3)
    };
    let Some(root) = root else {
        return false;
    };
    if root.as_bytes().get(1) != Some(&b':') {
        return false;
    }
    let mut wide: Vec<u16> = std::ffi::OsStr::new(root).encode_wide().collect();
    wide.push(0);
    unsafe { GetDriveTypeW(wide.as_ptr()) == DRIVE_FIXED }
}
fn utc_seconds(value: &str) -> bool {
    value.len() == 20 && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
