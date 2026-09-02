#![cfg(feature = "phase2-fixture")]

use nano_activation::enablement::EnablementStore;
use nano_activation::receipt::ArtifactIdentity;
use nano_activation::store::AuthorityStore;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SOURCE: &str = "288de9ed3185c91717f8f777c9975c784709e824";
const LOCK: &str = "3d6ec29f3b19e0b3778a5de222418ec497eaf79be8e93a92dd120d986bdb930a";

#[test]
fn fixture_uses_reducers_separates_private_handoff_and_enables_exact_artifact() {
    let parent = fixture_tempdir();
    let root = parent.path().join("evidence");
    std::fs::create_dir(&root).unwrap();
    restrict_root(&root);
    let root = std::fs::canonicalize(root).unwrap();
    let home = root.join("fixture-home");
    let handoff = root.join("fixture-private.json");
    let source_checkout =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let checkout = root.join("frozen-checkout");
    assert!(
        std::process::Command::new("git")
            .args([
                "-c",
                "core.autocrlf=false",
                "clone",
                "--quiet",
                "--no-hardlinks",
            ])
            .arg(git_path(&source_checkout))
            .arg(git_path(&checkout))
            .status()
            .unwrap()
            .success()
    );
    // Pin autocrlf in the nested repo: the clone-time `-c` flag does not
    // persist, and any outer-lock change makes the detach checkout rewrite
    // Cargo.lock with ambient config (CRLF on Windows) — the frozen byte-exact
    // lock hash must not depend on ambient git config.
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(git_path(&checkout))
            .args(["config", "core.autocrlf", "false"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(git_path(&checkout))
            .args(["checkout", "--detach", SOURCE])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(git_path(&checkout))
            .args([
                "remote",
                "set-url",
                "origin",
                "https://github.com/FerroxLabs/wayland-nano.git"
            ])
            .status()
            .unwrap()
            .success()
    );
    let artifacts = root.join("artifacts");
    std::fs::create_dir(&artifacts).unwrap();
    let executable = artifacts.join("fixture-runtime.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let executable_hash = hex(&Sha256::digest(std::fs::read(&executable).unwrap()));
    let preparation = root.join("preparation.json");
    std::fs::write(
        &preparation,
        serde_jcs::to_vec(&serde_json::json!({
            "cargo_lock_sha256":LOCK,
            "executable_path":std::fs::canonicalize(&executable).unwrap(),
            "executable_sha256":executable_hash,
            "frozen_checkout_path":std::fs::canonicalize(&checkout).unwrap(),
            "schema":"wayland.desktop.nano-phase2-fixture-preparation/v1",
            "source_commit_sha":SOURCE
        }))
        .unwrap(),
    )
    .unwrap();
    let output = nano_activation::phase2_fixture::run([
        "--evidence-root".into(),
        root.clone().into_os_string(),
        "--home".into(),
        home.clone().into_os_string(),
        "--private-handoff".into(),
        handoff.clone().into_os_string(),
        "--preparation-path".into(),
        std::fs::canonicalize(preparation).unwrap().into_os_string(),
        "--not-after".into(),
        "2099-01-01T00:00:00Z".into(),
    ])
    .unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(output, serde_jcs::to_string(&value).unwrap());
    assert_eq!(value["schema"], "wayland.nano.phase2-fixture/v2");
    assert_eq!(value["artifact"]["source_commit_sha"], SOURCE);
    assert_eq!(value["artifact"]["cargo_lock_sha256"], LOCK);
    assert_eq!(value["local_cli_subject_id"], "main");
    assert_eq!(value["local_cli_principal_id"], "main");
    assert!(!output.contains(&root.to_string_lossy().to_string()));
    assert!(!output.contains("seed_path"));
    assert!(!output.contains("key_reference"));
    assert!(handoff.is_file());
    let private: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&handoff).unwrap()).unwrap();
    assert_eq!(private["home"], home.to_string_lossy().as_ref());

    let store = AuthorityStore::open(&home).unwrap();
    let snapshot = store.snapshot().unwrap();
    let desktop = snapshot.issuers.get("wayland-desktop").unwrap();
    assert_eq!(
        (&desktop.subject_id, &desktop.principal_id),
        (&"phase2-agent".into(), &"main".into())
    );
    let cli = snapshot.issuers.get("local-cli").unwrap();
    assert_eq!(
        (&cli.subject_id, &cli.principal_id),
        (&"main".into(), &"main".into())
    );
    drop(store);
    let artifact: ArtifactIdentity = serde_json::from_value(value["artifact"].clone()).unwrap();
    EnablementStore::open(&home)
        .unwrap()
        .require_enabled(&artifact, [1, 1, 1, 1], "2098-01-01T00:00:00Z")
        .unwrap();
}

#[test]
fn fixture_rejects_unknown_arguments_and_non_child_home() {
    assert!(nano_activation::phase2_fixture::run(["--unknown".into(), "x".into()]).is_err());
}

#[test]
fn fixture_failures_publish_neither_home_handoff_nor_staging() {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    for fault in ["after_reducers", "after_home_publish"] {
        let case = prepared_case();
        // SAFETY: this test serializes access to the process environment.
        unsafe { std::env::set_var("NANO_PHASE2_FIXTURE_TEST_FAULT", fault) };
        let result = nano_activation::phase2_fixture::run([
            "--evidence-root".into(),
            case.root.clone().into_os_string(),
            "--home".into(),
            case.home.clone().into_os_string(),
            "--private-handoff".into(),
            case.handoff.clone().into_os_string(),
            "--preparation-path".into(),
            std::fs::canonicalize(&case.preparation)
                .unwrap()
                .into_os_string(),
            "--not-after".into(),
            "2099-01-01T00:00:00Z".into(),
        ]);
        unsafe { std::env::remove_var("NANO_PHASE2_FIXTURE_TEST_FAULT") };
        assert!(result.is_err(), "{fault} must fail closed");
        assert!(!case.home.exists(), "{fault} published a home");
        assert!(!case.handoff.exists(), "{fault} published a handoff");
        assert!(
            std::fs::read_dir(&case.root).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".nano-phase2-staging-")),
            "{fault} left staging residue"
        );
    }
}

#[test]
fn fixture_binary_is_absent_without_its_nondefault_feature() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert!(manifest.contains("default = []"));
    assert!(manifest.contains("required-features = [\"phase2-fixture\"]"));
}

struct PreparedCase {
    _parent: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    handoff: PathBuf,
    preparation: PathBuf,
}

fn prepared_case() -> PreparedCase {
    let parent = fixture_tempdir();
    let root = parent.path().join("evidence");
    std::fs::create_dir(&root).unwrap();
    restrict_root(&root);
    let root = std::fs::canonicalize(root).unwrap();
    let checkout = root.join("frozen-checkout");
    let source_checkout =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    assert!(
        std::process::Command::new("git")
            .args([
                "-c",
                "core.autocrlf=false",
                "clone",
                "--quiet",
                "--no-hardlinks",
            ])
            .arg(git_path(&source_checkout))
            .arg(git_path(&checkout))
            .status()
            .unwrap()
            .success()
    );
    // Pin autocrlf in the nested repo: the clone-time `-c` flag does not
    // persist, and any outer-lock change makes the detach checkout rewrite
    // Cargo.lock with ambient config (CRLF on Windows) — the frozen byte-exact
    // lock hash must not depend on ambient git config.
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(git_path(&checkout))
            .args(["config", "core.autocrlf", "false"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(git_path(&checkout))
            .args(["checkout", "--detach", SOURCE])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(git_path(&checkout))
            .args([
                "remote",
                "set-url",
                "origin",
                "https://github.com/FerroxLabs/wayland-nano.git",
            ])
            .status()
            .unwrap()
            .success()
    );
    let artifacts = root.join("artifacts");
    std::fs::create_dir(&artifacts).unwrap();
    let executable = artifacts.join("fixture-runtime.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let executable_hash = hex(&Sha256::digest(std::fs::read(&executable).unwrap()));
    let preparation = root.join("preparation.json");
    std::fs::write(
        &preparation,
        serde_jcs::to_vec(&serde_json::json!({
            "cargo_lock_sha256":LOCK,
            "executable_path":std::fs::canonicalize(&executable).unwrap(),
            "executable_sha256":executable_hash,
            "frozen_checkout_path":std::fs::canonicalize(&checkout).unwrap(),
            "schema":"wayland.desktop.nano-phase2-fixture-preparation/v1",
            "source_commit_sha":SOURCE
        }))
        .unwrap(),
    )
    .unwrap();
    PreparedCase {
        home: root.join("fixture-home"),
        handoff: root.join("fixture-private.json"),
        preparation,
        root,
        _parent: parent,
    }
}

#[cfg(windows)]
fn fixture_tempdir() -> tempfile::TempDir {
    normalize_test_default_owner();
    std::env::var_os("WN_VERIFY_TEMP").map_or_else(
        || tempfile::tempdir().unwrap(),
        |root| {
            tempfile::Builder::new()
                .tempdir_in(PathBuf::from(root))
                .unwrap()
        },
    )
}

#[cfg(windows)]
fn normalize_test_default_owner() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_OWNER, TOKEN_QUERY,
        TOKEN_USER, TokenOwner, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    static NORMALIZED: OnceLock<()> = OnceLock::new();
    NORMALIZED.get_or_init(|| unsafe {
        let mut token = 0;
        assert_ne!(
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_DEFAULT | TOKEN_QUERY,
                &mut token,
            ),
            0
        );
        let mut size = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        assert!(size >= std::mem::size_of::<TOKEN_USER>() as u32);
        let mut buffer = vec![0_u8; size as usize];
        assert_ne!(
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ),
            0
        );
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let owner = TOKEN_OWNER {
            Owner: user.User.Sid,
        };
        assert_ne!(
            SetTokenInformation(
                token,
                TokenOwner,
                (&raw const owner).cast(),
                std::mem::size_of::<TOKEN_OWNER>() as u32,
            ),
            0
        );
        assert_ne!(CloseHandle(token), 0);
    });
}

#[cfg(not(windows))]
fn fixture_tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[cfg(unix)]
fn restrict_root(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(windows)]
fn restrict_root(path: &Path) {
    let output = std::process::Command::new("whoami.exe")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .unwrap();
    let row = String::from_utf8(output.stdout).unwrap();
    let sid = row.trim().split(',').nth(1).unwrap().trim_matches('"');
    assert!(
        std::process::Command::new("icacls.exe")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:(OI)(CI)F")])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(windows)]
fn git_path(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    text.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_owned(), PathBuf::from)
}
#[cfg(unix)]
fn git_path(path: &Path) -> PathBuf {
    path.to_owned()
}
