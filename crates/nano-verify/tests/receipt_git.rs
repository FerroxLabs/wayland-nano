use std::{collections::BTreeMap, fs, path::Path, process::Command};

use nano_verify::{
    receipt::{
        FailingRun, Receipt, ReceiptPreflight, VerifyVerdict, canonical_receipt, mint_receipt,
        preflight_receipt,
    },
    registry::{CwdPolicy, GateClosure, GateRegistry, GateRegistryEntry, closure_digest},
};

struct Repo {
    dir: tempfile::TempDir,
    observed: String,
    fix: String,
    unrelated: String,
    blob: String,
    registry: GateRegistry,
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn fixture() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    fs::create_dir_all(dir.path().join("tests")).unwrap();
    fs::write(dir.path().join("tests/red.rs"), "red\n").unwrap();
    git(dir.path(), &["add", "tests/red.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Nano Test",
            "-c",
            "user.email=nano@example.invalid",
            "commit",
            "-q",
            "-m",
            "observed",
        ],
    );
    let observed = git(dir.path(), &["rev-parse", "HEAD"]);
    let blob = git(dir.path(), &["rev-parse", "HEAD:tests/red.rs"]);
    fs::write(dir.path().join("fixed.txt"), "fixed\n").unwrap();
    git(dir.path(), &["add", "fixed.txt"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Nano Test",
            "-c",
            "user.email=nano@example.invalid",
            "commit",
            "-q",
            "-m",
            "fix",
        ],
    );
    let fix = git(dir.path(), &["rev-parse", "HEAD"]);
    git(dir.path(), &["checkout", "-q", "--orphan", "unrelated"]);
    git(dir.path(), &["rm", "-q", "-rf", "."]);
    fs::write(dir.path().join("other.txt"), "other\n").unwrap();
    git(dir.path(), &["add", "other.txt"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Nano Test",
            "-c",
            "user.email=nano@example.invalid",
            "commit",
            "-q",
            "-m",
            "unrelated",
        ],
    );
    let unrelated = git(dir.path(), &["rev-parse", "HEAD"]);
    let closure = GateClosure {
        argv: vec!["gate.cmd".into()],
        env: BTreeMap::new(),
        cwd_policy: CwdPolicy::RepoRoot,
        wrapped_tools: vec![],
    };
    let digest = closure_digest(&closure).unwrap();
    let entry = GateRegistryEntry {
        card: "gate.md".into(),
        script: "gate.cmd".into(),
        closure,
        closure_digest: digest,
        run_artifact: "run".into(),
    };
    let registry = GateRegistry {
        schema: 1,
        gates: BTreeMap::from([("gate-a".into(), entry)]),
        requirements: BTreeMap::from([("RCPT-01".into(), "gate-a".into())]),
    };
    Repo {
        dir,
        observed,
        fix,
        unrelated,
        blob,
        registry,
    }
}

fn receipt(repo: &Repo) -> Receipt {
    Receipt {
        schema: 1,
        requirement: "RCPT-01".into(),
        test: "tests/red.rs".into(),
        gate_id: "gate-a".into(),
        gate_closure_digest: repo.registry.gates["gate-a"].closure_digest.clone(),
        failing_run: FailingRun {
            exit_code: 1,
            log_digest: "a".repeat(64),
            observed_at_commit: repo.observed.clone(),
        },
        fix_commit: repo.fix.clone(),
        minted_at: "2026-08-17T00:00:00Z".into(),
        minted_by: "wayland-nano 0.1.1".into(),
    }
}
fn bytes(value: &Receipt) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn mint_and_preflight_ready_in_repo() {
    let r = fixture();
    let v = mint_receipt(receipt(&r)).unwrap();
    assert_eq!(
        preflight_receipt(r.dir.path(), &canonical_receipt(&v).unwrap(), &r.registry),
        ReceiptPreflight::Ready
    );
    assert_ne!(
        format!("{:?}", ReceiptPreflight::Ready),
        format!("{:?}", VerifyVerdict::Valid)
    );
}
#[test]
fn mint_outside_repo_never_claims_verified() {
    let r = fixture();
    let outside = tempfile::tempdir().unwrap();
    let v = mint_receipt(receipt(&r)).unwrap();
    assert_eq!(
        preflight_receipt(outside.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::Unverifiable
    );
}
#[test]
fn preflight_unverifiable_is_terminal() {
    let r = fixture();
    let outside = tempfile::tempdir().unwrap();
    assert_eq!(
        preflight_receipt(outside.path(), &bytes(&receipt(&r)), &r.registry),
        ReceiptPreflight::Unverifiable
    );
}
#[test]
fn preflight_never_red_on_zero_exit() {
    let r = fixture();
    let mut v = receipt(&r);
    v.failing_run.exit_code = 0;
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::NeverRed
    );
    v.failing_run.exit_code = 1;
    v.failing_run.log_digest.clear();
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::NeverRed
    );
}
#[test]
fn preflight_fabricated_commit() {
    let r = fixture();
    let mut v = receipt(&r);
    v.fix_commit = "f".repeat(40);
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::FabricatedCommit
    );
    v = receipt(&r);
    v.failing_run.observed_at_commit = "f".repeat(40);
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::FabricatedCommit
    );
}
#[test]
fn preflight_rejects_unproven_ancestry_or_test_path() {
    let r = fixture();
    let mut v = receipt(&r);
    v.fix_commit = r.blob.clone();
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::FabricatedCommit
    );
    v = receipt(&r);
    v.fix_commit = r.unrelated.clone();
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::AncestryUnproven
    );
    v = receipt(&r);
    v.test = "tests/absent.rs".into();
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::AncestryUnproven
    );
}
#[test]
fn preflight_gate_mismatch_unknown_or_unmapped_gate() {
    let r = fixture();
    let mut v = receipt(&r);
    v.gate_id = "unknown".into();
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::GateMismatch
    );
    v = receipt(&r);
    let mut registry = r.registry.clone();
    registry
        .requirements
        .insert("RCPT-01".into(), "other".into());
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &registry),
        ReceiptPreflight::GateMismatch
    );
}
#[test]
fn preflight_gate_mismatch_digest_drift() {
    let r = fixture();
    let mut v = receipt(&r);
    v.gate_closure_digest = "b".repeat(64);
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::GateMismatch
    );
    let mut registry = r.registry.clone();
    registry.gates.get_mut("gate-a").unwrap().closure_digest = "c".repeat(64);
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&receipt(&r)), &registry),
        ReceiptPreflight::GateMismatch
    );
}
#[test]
fn preflight_unknown_schema_and_extra_fields_unverifiable() {
    let r = fixture();
    let mut v = receipt(&r);
    v.schema = 2;
    assert_eq!(
        preflight_receipt(r.dir.path(), &bytes(&v), &r.registry),
        ReceiptPreflight::Unverifiable
    );
    let mut value = serde_json::to_value(receipt(&r)).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("extra".into(), serde_json::json!(true));
    assert_eq!(
        preflight_receipt(
            r.dir.path(),
            &serde_json::to_vec(&value).unwrap(),
            &r.registry
        ),
        ReceiptPreflight::Unverifiable
    );
}
