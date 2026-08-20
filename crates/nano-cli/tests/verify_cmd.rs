use nano_cli::verify_cmd::{
    VerifyEvents, VerifyMode, VerifyParams, VerifyRuntime, run_with_runtime_and_events,
};
use nano_verify::{
    BaselineGateEvidence, BaselineGateExecution, CheckVerdict, CwdPolicy, ExecutionGateOutcome,
    FailCategory, GateClosure, GateRegistry, GateRegistryEntry, ToolPin, closure_digest,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/verify");
const EMPTY_BOOTSTRAP: &[u8] = b"{\"gates\":{},\"requirements\":{},\"schema\":1}";
fn production_source() -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/verify_cmd.rs"))
        .unwrap()
}

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn unique_root(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let temp = std::fs::canonicalize(std::env::var_os("TEMP").expect("TEMP required")).unwrap();
    assert_eq!(
        temp,
        std::fs::canonicalize(std::env::var_os("TMP").unwrap()).unwrap()
    );
    assert!(
        temp.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .starts_with("F:\\")
    );
    temp.join(format!(
        "wn-wp3-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct FixtureRepo {
    root: PathBuf,
    receipts: PathBuf,
    observed: String,
    fix: String,
    red_fix: String,
    digest: String,
}

impl Drop for FixtureRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn materialize_fixture_repo(label: &str) -> FixtureRepo {
    assert_eq!(
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../gates/registry.json"))
            .unwrap(),
        EMPTY_BOOTSTRAP
    );
    let base = unique_root(label);
    let root = base.join("r");
    let receipts = base.join("p");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("gates/fixture-add")).unwrap();
    std::fs::create_dir_all(&receipts).unwrap();
    let broken = std::fs::read_to_string(Path::new(FIXTURES).join("repo/src-broken/lib.rs"))
        .unwrap()
        .replace("\r\n", "\n");
    std::fs::write(root.join("src/lib.rs"), broken).unwrap();
    for rel in [
        "gates/fixture-add/gate.sh",
        "gates/fixture-add/gate.ps1",
        "gates/fixture-add/card.md",
    ] {
        let bytes = std::fs::read(Path::new(FIXTURES).join("repo").join(rel)).unwrap();
        std::fs::write(
            root.join(rel),
            String::from_utf8(bytes).unwrap().replace("\r\n", "\n"),
        )
        .unwrap();
    }
    #[cfg(windows)]
    let (interpreter, script) = (
        "powershell.exe".to_owned(),
        "gates/fixture-add/gate.ps1".to_owned(),
    );
    #[cfg(not(windows))]
    let (interpreter, script) = ("bash".to_owned(), "gates/fixture-add/gate.sh".to_owned());
    let closure = GateClosure {
        argv: vec![interpreter.clone(), script.clone()],
        env: BTreeMap::new(),
        cwd_policy: CwdPolicy::RepoRoot,
        wrapped_tools: vec![ToolPin {
            name: interpreter,
            version: "fixture".into(),
        }],
    };
    let digest = closure_digest(&closure).unwrap();
    let registry = GateRegistry {
        schema: 1,
        gates: BTreeMap::from([(
            "fixture-add".into(),
            GateRegistryEntry {
                card: "gates/fixture-add/card.md".into(),
                script: script.clone(),
                closure,
                closure_digest: digest.clone(),
                run_artifact: "src/lib.rs".into(),
            },
        )]),
        requirements: BTreeMap::from([("CLI-05".into(), "fixture-add".into())]),
    };
    std::fs::create_dir_all(root.join("gates")).unwrap();
    std::fs::write(
        root.join("gates/registry.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    git(&root, &["config", "user.name", "WP3 Fixture"]);
    git(&root, &["config", "user.email", "wp3@example.invalid"]);
    git(&root, &["add", "-f", "."]);
    git(&root, &["commit", "-q", "-m", "A broken"]);
    let observed = git(&root, &["rev-parse", "HEAD"]);
    let fixed = std::fs::read(Path::new(FIXTURES).join("repo/src-fixed/lib.rs")).unwrap();
    assert!(String::from_utf8_lossy(&fixed).contains("a + b"));
    std::fs::write(root.join("src/lib.rs"), fixed).unwrap();
    assert!(
        std::fs::read_to_string(root.join("src/lib.rs"))
            .unwrap()
            .contains("a + b")
    );
    git(&root, &["add", "-f", "src/lib.rs"]);
    git(&root, &["commit", "-q", "-m", "B fixed"]);
    let fix = git(&root, &["rev-parse", "HEAD"]);
    std::fs::write(
        root.join("src/lib.rs"),
        std::fs::read(Path::new(FIXTURES).join("repo/src-broken/lib.rs")).unwrap(),
    )
    .unwrap();
    git(&root, &["add", "-f", "src/lib.rs"]);
    git(&root, &["commit", "-q", "-m", "C red again"]);
    let red_fix = git(&root, &["rev-parse", "HEAD"]);
    for entry in std::fs::read_dir(Path::new(FIXTURES).join("receipts")).unwrap() {
        let entry = entry.unwrap();
        let text = std::fs::read_to_string(entry.path())
            .unwrap()
            .replace("gates/fixture-add/gate.sh", &script)
            .replace("{{OBSERVED}}", &observed)
            .replace("{{FIX}}", &fix)
            .replace("{{DIGEST}}", &digest);
        let text = if entry.file_name() == "rerun-red.receipt.json" {
            text.replace(
                &format!("\"fix_commit\":\"{observed}\""),
                &format!("\"fix_commit\":\"{red_fix}\""),
            )
        } else {
            text
        };
        std::fs::write(receipts.join(entry.file_name()), text).unwrap();
    }
    FixtureRepo {
        root,
        receipts,
        observed,
        fix,
        red_fix,
        digest,
    }
}

fn check(repo: &FixtureRepo, name: &str) -> (i32, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args(["verify", "--verify-receipt"])
        .arg(repo.receipts.join(name))
        .arg("--json")
        .current_dir(&repo.root)
        .output()
        .unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap(), value)
}

fn assert_decision(repo: &FixtureRepo, name: &str, code: i32, decision: &str) {
    let (actual, body) = check(repo, name);
    assert_eq!(actual, code, "{body}");
    assert_eq!(body["decision"], decision);
    assert_eq!(body["re_derived"], true);
}

struct Scripted {
    diff: String,
    generated: std::sync::atomic::AtomicU64,
    baseline: std::sync::atomic::AtomicU64,
    gates: std::sync::atomic::AtomicU64,
}
impl VerifyRuntime for Scripted {
    fn now_millis(&self) -> u64 {
        1
    }
    fn temp_preflight(&self) -> Result<(), ()> {
        Ok(())
    }
    async fn generate(&self, _: &str, _: &str) -> Result<String, ()> {
        self.generated
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.diff.clone())
    }
    async fn run_gate(
        &self,
        _: &nano_verify::GateInvocation,
        artifact: &Path,
        _: &[(String, FailCategory)],
    ) -> ExecutionGateOutcome {
        self.gates.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if std::fs::read_to_string(artifact).unwrap().contains("a + b") {
            ExecutionGateOutcome::Green {
                verdicts: vec![CheckVerdict {
                    id: "FX-01".into(),
                    category: FailCategory::Value,
                    passed: true,
                }],
            }
        } else {
            ExecutionGateOutcome::Red {
                verdicts: vec![CheckVerdict {
                    id: "FX-01".into(),
                    category: FailCategory::Value,
                    passed: false,
                }],
            }
        }
    }
    async fn run_baseline_gate(
        &self,
        _: &nano_verify::GateInvocation,
        _: &Path,
        _: &[(String, FailCategory)],
    ) -> BaselineGateExecution {
        self.baseline
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        BaselineGateExecution {
            outcome: ExecutionGateOutcome::Red {
                verdicts: vec![CheckVerdict {
                    id: "FX-01".into(),
                    category: FailCategory::Value,
                    passed: false,
                }],
            },
            evidence: BaselineGateEvidence {
                exit_code: Some(7),
                log_digest: Some("a".repeat(64)),
            },
        }
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn verify_full_flow_green_mints_receipt() {
    let source = production_source();
    assert!(
        source.contains("if entry.postimage_sha256().is_some()"),
        "materializer deletion rejection missing"
    );
    assert!(
        source.contains("&[\"cat-file\", \"-e\", &format!(\":{}\", entry.path())]"),
        "materializer indexed deletion probe missing"
    );
    assert!(
        source.contains("&[\"diff\", \"--cached\", \"--name-status\", \"-z\", \"--no-renames\"]"),
        "materializer rename/copy rejection missing"
    );
    assert!(
        source.contains(".any(|path| !candidate_path_allowed(path, authority))"),
        "materializer protected-path guard missing"
    );
    let _guard = serial();
    let repo = materialize_fixture_repo("mint");
    let raw = git(
        &repo.root,
        &["diff", &repo.observed, &repo.fix, "--", "src/lib.rs"],
    );
    let diff = raw
        .lines()
        .filter(|line| !line.starts_with("index "))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let scripted = Scripted {
        diff,
        generated: 0.into(),
        baseline: 0.into(),
        gates: 0.into(),
    };
    git(&repo.root, &["checkout", "-q", &repo.observed]);
    let home = repo.root.parent().unwrap().join("h");
    let out = repo.root.parent().unwrap().join("out.json");
    let loaded = nano_verify::load_registry(&repo.root).unwrap();
    assert_eq!(
        nano_verify::check_inventory(&repo.root.join(&loaded.gates["fixture-add"].card))
            .unwrap()
            .len(),
        1
    );
    let params = VerifyParams {
        mode: VerifyMode::Mint {
            requirement: "CLI-05".into(),
            gate: Some("fixture-add".into()),
            task: None,
            budget: Some(1),
            cheap_model: "fixture-cheap".into(),
            escalation_models: vec!["fixture-escalate".into()],
            deadline_ms: 60_000,
            receipt_out: Some(out.clone()),
            json: true,
        },
    };
    let mut event_bytes = Vec::new();
    let mut events = VerifyEvents::new(&mut event_bytes, "fixture-run".into());
    let code =
        run_with_runtime_and_events(&home, &repo.root, &params, &scripted, &mut events).await;
    drop(events);
    assert_eq!(
        code,
        0,
        "counts={}/{}/{} head={} status={} log={}",
        scripted.generated.load(std::sync::atomic::Ordering::SeqCst),
        scripted.baseline.load(std::sync::atomic::Ordering::SeqCst),
        scripted.gates.load(std::sync::atomic::Ordering::SeqCst),
        git(&repo.root, &["rev-parse", "HEAD"]),
        git(&repo.root, &["status", "--porcelain=v1"]),
        git(&repo.root, &["log", "--oneline", "-3"])
    );
    let stored = home.join("receipts/CLI-05.receipt.json");
    assert_eq!(
        std::fs::read(&stored).unwrap(),
        std::fs::read(&out).unwrap()
    );
    let receipt: nano_verify::Receipt =
        serde_json::from_slice(&std::fs::read(stored).unwrap()).unwrap();
    assert_eq!(receipt.failing_run.observed_at_commit, repo.observed);
    assert_eq!(receipt.fix_commit, git(&repo.root, &["rev-parse", "HEAD"]));
    assert_eq!(receipt.test, loaded.gates["fixture-add"].script);
    let frames: Vec<serde_json::Value> = String::from_utf8(event_bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let types: Vec<_> = frames
        .iter()
        .map(|frame| frame["type"].as_str().unwrap())
        .collect();
    assert_eq!(types.first(), Some(&"verify_started"));
    assert!(
        types
            .windows(2)
            .any(|pair| pair == ["apply_started", "apply_verified"])
    );
    assert_eq!(
        &types[types.len() - 2..],
        ["receipt_minted", "verify_completed"]
    );
    for (seq, frame) in frames.iter().enumerate() {
        assert_eq!(frame["v"], 1);
        assert_eq!(frame["seq"], seq);
        let text = frame.to_string();
        assert!(!text.contains("gate.ps1") && !text.contains("a + b") && !text.contains(FIXTURES));
    }
}

#[test]
fn verify_authored_defect_red_identifiers_only() {
    let mut bytes = Vec::new();
    let mut events = VerifyEvents::new(&mut bytes, "fixture".into());
    events.check_verdict(&CheckVerdict {
        id: "FX-01".into(),
        category: FailCategory::Value,
        passed: false,
    });
    drop(events);
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let keys: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["category", "id", "passed", "seq", "session_id", "type", "v"]
    );
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("gate.sh") && !text.contains("a + b") && !text.contains(FIXTURES));
}

#[test]
fn verify_receipt_roundtrip_valid() {
    assert!(
        production_source()
            .contains(".cleanup_receipt_worktree(repo_root, &worktree, cleanup_budget)"),
        "cleanup guard missing"
    );
    let _g = serial();
    let r = materialize_fixture_repo("valid");
    let bytes = std::fs::read(r.receipts.join("valid.receipt.json")).unwrap();
    let registry = nano_verify::load_registry(&r.root).unwrap();
    assert_eq!(
        nano_verify::preflight_receipt(&r.root, &bytes, &registry),
        nano_verify::ReceiptPreflight::Ready
    );
    assert_decision(&r, "valid.receipt.json", 0, "valid");
}
#[test]
fn verify_receipt_tampered_fails_closed() {
    let _g = serial();
    let r = materialize_fixture_repo("tamper");
    assert_decision(&r, "tampered-structure.receipt.json", 6, "never-red");
}
#[test]
fn verify_receipt_fabricated_commit() {
    let _g = serial();
    let r = materialize_fixture_repo("fabricated");
    assert_decision(&r, "fabricated.receipt.json", 6, "fabricated-commit");
}
#[test]
fn verify_receipt_unknown_field_fails_closed() {
    let _g = serial();
    let r = materialize_fixture_repo("unknown");
    assert_decision(&r, "extra-field.receipt.json", 6, "unverifiable");
}
#[test]
fn verify_receipt_green_only_is_never_red() {
    let _g = serial();
    let r = materialize_fixture_repo("green");
    assert_decision(&r, "green-only.receipt.json", 6, "never-red");
}
#[test]
fn verify_receipt_gate_pin_drift() {
    let _g = serial();
    let r = materialize_fixture_repo("pin");
    assert_decision(&r, "gate-pin-drift.receipt.json", 6, "gate-mismatch");
}

#[test]
fn verify_exit_code_matrix() {
    assert!(
        production_source().contains("return if exit == 0 { 3 } else { exit };"),
        "deadline/exit classification guard missing"
    );
    for args in [
        vec!["verify"],
        vec!["verify", "--deadline-ms", "0"],
        vec![
            "verify",
            "--verify-receipt",
            "missing",
            "--requirement",
            "X",
        ],
        vec!["verify", "--gate", "x", "--run-only", "--cheap-model", "x"],
    ] {
        assert_eq!(
            Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
                .args(args)
                .status()
                .unwrap()
                .code(),
            Some(2)
        );
    }
}

#[test]
fn verify_red_run_writes_no_receipt() {
    assert!(
        production_source().contains("if exit != 0 {"),
        "verified-before-receipt guard missing"
    );
    let _g = serial();
    let r = materialize_fixture_repo("red-no-receipt");
    let home = r.root.parent().unwrap().join("h");
    let out = r.root.parent().unwrap().join("out");
    std::fs::write(&out, b"sentinel").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
        .args([
            "verify",
            "--requirement",
            "CLI-05",
            "--gate",
            "fixture-add",
            "--cheap-model",
            "invalid",
            "--escalation-model",
            "invalid",
            "--receipt-out",
        ])
        .arg(&out)
        .env("NANO_HOME", &home)
        .current_dir(&r.root)
        .status()
        .unwrap();
    assert_ne!(status.code(), Some(0));
    assert_eq!(std::fs::read(out).unwrap(), b"sentinel");
    assert!(!home.join("receipts/CLI-05.receipt.json").exists());
}

#[test]
fn verify_receipt_ancestry_unproven() {
    let _g = serial();
    let r = materialize_fixture_repo("ancestor");
    assert_decision(&r, "ancestry-unproven.receipt.json", 6, "ancestry-unproven");
}
#[test]
fn verify_receipt_rerun_red_is_gate_mismatch() {
    assert!(
        production_source().contains("nano_verify::ExecutionGateOutcome::Green { .. } => Valid"),
        "Ready-to-Valid rerun guard missing"
    );
    let _g = serial();
    let r = materialize_fixture_repo("rerun");
    assert_decision(&r, "rerun-red.receipt.json", 6, "gate-mismatch");
}

#[test]
fn verify_run_only_resolves_artifact_and_exit_codes() {
    let _g = serial();
    let r = materialize_fixture_repo("run-only");
    let run = |root: &Path| {
        Command::new(env!("CARGO_BIN_EXE_wayland-nano"))
            .args(["verify", "--gate", "fixture-add", "--run-only", "--json"])
            .current_dir(root)
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(run(&r.root), Some(3));
    git(&r.root, &["checkout", "-q", &r.fix]);
    assert_eq!(run(&r.root), Some(0));
    let mut registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(r.root.join("gates/registry.json")).unwrap())
            .unwrap();
    registry["gates"]["fixture-add"]["run_artifact"] = "../escape".into();
    std::fs::write(
        r.root.join("gates/registry.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    assert_eq!(run(&r.root), Some(2));
}

#[test]
fn fixture_materializes_real_history_and_resolves_templates() {
    let _g = serial();
    let r = materialize_fixture_repo("fixture");
    assert_ne!(r.observed, r.fix);
    assert_ne!(r.fix, r.red_fix);
    assert_eq!(r.digest.len(), 64);
    for e in std::fs::read_dir(&r.receipts).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        assert!(!text.contains("{{"));
    }
}
