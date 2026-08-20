use std::{
    cell::Cell,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

static DOWNSTREAM_CARGO_LOCK: Mutex<()> = Mutex::new(());

struct FixtureRoot {
    path: PathBuf,
    target_root: PathBuf,
    next_target: Cell<u32>,
    _guard: MutexGuard<'static, ()>,
}

impl FixtureRoot {
    fn new() -> Self {
        let guard = DOWNSTREAM_CARGO_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp = std::env::temp_dir().canonicalize().unwrap_or_else(|error| {
            panic!("OS temp directory must exist before downstream checks: {error}")
        });
        assert!(
            temp.is_dir(),
            "OS temp directory is not a directory: {}",
            temp.display()
        );
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must follow the Unix epoch")
            .as_nanos();
        let path = temp.join(format!(
            "wayland-nano-wp2-public-contract-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create private downstream fixture root");
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("nano-verify must be inside the workspace crates directory")
            .to_path_buf();
        let target_base = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .map(|target| {
                if target.is_absolute() {
                    target
                } else {
                    workspace_root.join(target)
                }
            })
            .unwrap_or_else(|| workspace_root.join("target"));
        let target_root = target_base.join(format!(
            "w2-{}-{:x}",
            std::process::id(),
            nonce & 0xffff_ffff
        ));
        fs::create_dir_all(&target_base).expect("create downstream target base");
        fs::create_dir(&target_root).expect("create unique short downstream target root");
        Self {
            path,
            target_root,
            next_target: Cell::new(0),
            _guard: guard,
        }
    }

    fn check(&self, name: &str, source: &str) -> Output {
        let project = self.path.join(name);
        fs::create_dir(&project).expect("create downstream project");
        fs::create_dir(project.join("src")).expect("create downstream source directory");

        let dependency_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .expect("canonical nano-verify crate path");
        let dependency_text = dependency_path.to_string_lossy();
        let dependency = dependency_text
            .strip_prefix(r"\\?\")
            .unwrap_or(&dependency_text)
            .replace('\\', "/");
        fs::write(
            project.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\nnano-verify = {{ path = \"{dependency}\" }}\n"
            ),
        )
        .expect("write downstream manifest");
        fs::write(project.join("src/lib.rs"), source).expect("write downstream source");

        let target_index = self.next_target.get();
        self.next_target.set(target_index + 1);
        let target = self.target_root.join(format!("t{target_index}"));
        let stderr = tempfile::tempfile().expect("create bounded downstream stderr capture");
        let stderr_child = stderr
            .try_clone()
            .expect("clone bounded downstream stderr capture");
        let mut child = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["check", "--offline", "--quiet"])
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", &target)
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_child))
            .spawn()
            .expect("launch offline downstream cargo check");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll downstream cargo check") {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("offline downstream cargo check exceeded 180 second bound");
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        let mut stderr_bytes = Vec::new();
        let mut stderr = stderr;
        use std::io::{Read, Seek};
        stderr.rewind().expect("rewind downstream stderr capture");
        stderr
            .take(1024 * 1024)
            .read_to_end(&mut stderr_bytes)
            .expect("read bounded downstream stderr capture");
        Output {
            status,
            stdout: Vec::new(),
            stderr: stderr_bytes,
        }
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap_or_else(|error| {
            panic!(
                "remove serialized downstream fixture {}: {error}",
                self.path.display()
            )
        });
        fs::remove_dir_all(&self.target_root).unwrap_or_else(|error| {
            panic!(
                "remove serialized downstream target root {}: {error}",
                self.target_root.display()
            )
        });
    }
}

fn diagnostics(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_boundary_failure(root: &FixtureRoot, name: &str, source: &str, expected: &[&str]) {
    let output = root.check(name, source);
    let stderr = diagnostics(&output);
    assert!(!output.status.success(), "{name} unexpectedly compiled");
    for false_positive in [
        "failed to get `nano-verify` as a dependency",
        "failed to load source for dependency",
        "no matching package named",
        "failed to parse manifest",
        "could not find `Cargo.toml`",
        "unexpected closing delimiter",
        "unclosed delimiter",
        "expected one of",
        "expected expression",
        "expected item",
        "expected identifier",
        "unknown start of token",
    ] {
        assert!(
            !stderr.contains(false_positive),
            "{name} failed for an unrelated reason ({false_positive}):\n{stderr}"
        );
    }
    assert!(
        expected.iter().any(|needle| stderr.contains(needle)),
        "{name} did not fail at its intended privacy/type/arity boundary; expected one of {expected:?}:\n{stderr}"
    );
}

#[test]
fn downstream_cannot_forge_or_mutate_trusted_types() {
    let root = FixtureRoot::new();
    assert_boundary_failure(
        &root,
        "candidate_artifact_private",
        r#"use nano_verify::CandidateArtifact;
fn attack(mut value: CandidateArtifact) {
    value.bytes_sha256 = String::new();
    let _ = CandidateArtifact {
        workspace: value.workspace,
        path: std::path::PathBuf::new(),
        bytes_sha256: String::new(),
        _seal: value._seal,
    };
}"#,
        &["is private", "E0616", "E0451"],
    );
    assert_boundary_failure(
        &root,
        "workspace_private_and_nonclone",
        r#"use nano_verify::ArtifactWorkspace;
fn attack(mut value: ArtifactWorkspace) {
    let _alias = value.clone();
    value.inner = value.inner.clone();
    let _ = ArtifactWorkspace { inner: value.inner, _seal: value._seal };
}"#,
        &[
            "is private",
            "E0616",
            "E0451",
            "no method named `clone`",
            "E0599",
        ],
    );
    assert_boundary_failure(
        &root,
        "candidate_diff_private",
        r#"use nano_verify::CandidateDiff;
fn attack(mut value: CandidateDiff) {
    value.paths = vec!["escape".into()];
    let _ = CandidateDiff { paths: vec![], bytes_sha256: String::new(), records: vec![] };
}"#,
        &["is private", "E0616", "E0451"],
    );
    assert_boundary_failure(
        &root,
        "expected_change_private",
        r#"use nano_verify::{ChangeKind, ExpectedChange};
fn attack(mut value: ExpectedChange) {
    value.kind = ChangeKind::Delete;
    let _ = ExpectedChange { path: String::new(), kind: ChangeKind::Add, postimage_sha256: None };
}"#,
        &["is private", "E0616", "E0451"],
    );
    assert_boundary_failure(
        &root,
        "manifest_private",
        r#"use nano_verify::ExpectedChangeManifest;
fn attack(mut value: ExpectedChangeManifest) {
    value.diff_digest = String::new();
    let _ = ExpectedChangeManifest { entries: vec![], base_tree_digest: String::new(), diff_digest: String::new() };
}"#,
        &["is private", "E0616", "E0451"],
    );
    assert_boundary_failure(
        &root,
        "outcome_private",
        r#"use nano_verify::{ClimbOutcome, StopReason};
fn attack(mut value: ClimbOutcome) {
    value.stop_reason = StopReason::Solved;
    let _ = ClimbOutcome {
        terminal: nano_verify::TerminalState::Verified,
        score: [1, 1], fails: vec![], rounds_used: 0, escalated: false,
        stop_reason: StopReason::Solved, cost_usd: None, log: vec![],
        accepted_artifact: None,
    };
}"#,
        &["is private", "E0616", "E0451", "cannot construct"],
    );
}

#[test]
fn downstream_cannot_bypass_constructors_or_frozen_signatures() {
    let root = FixtureRoot::new();
    assert_boundary_failure(
        &root,
        "candidate_constructor_private",
        r#"fn attack(workspace: &nano_verify::ArtifactWorkspace) {
    let _ = nano_verify::gate::create_candidate_artifact(workspace, b"diff");
}"#,
        &["is private", "E0603"],
    );
    assert_boundary_failure(
        &root,
        "stale_inconsistent_verdicts",
        r#"use nano_verify::ExecutionFailClosedReason;
fn attack() {
    let _ = ExecutionFailClosedReason::InconsistentVerdicts {
        reported_passed: 1, reported_total: 2, expected_passed: 1,
    };
}"#,
        &["missing field", "E0063", "expected_total"],
    );
    assert_boundary_failure(
        &root,
        "derive_from_bytes",
        r#"fn attack(root: &std::path::Path) {
    let _ = nano_verify::derive_expected_changes(b"diff", root);
}"#,
        &["mismatched types", "E0308", "expected `&CandidateDiff`"],
    );
    assert_boundary_failure(
        &root,
        "derive_from_path",
        r#"fn attack(diff_path: &std::path::Path, root: &std::path::Path) {
    let _ = nano_verify::derive_expected_changes(diff_path, root);
}"#,
        &["mismatched types", "E0308", "expected `&CandidateDiff`"],
    );
    assert_boundary_failure(
        &root,
        "run_climb_rejects_starting_root",
        r#"use nano_verify::*;
struct Fx;
impl Effects for Fx {
    async fn generate(&self, _: &str, _: &str) -> Result<String, VerifyError> { unimplemented!() }
    fn emit_event(&self, _: EngineEvent) {}
    fn now_millis(&self) -> u64 { 0 }
    fn cancellation_requested(&self) -> bool { false }
}
fn attack(g: &GateInvocation, i: &[(String, FailCategory)], w: ArtifactWorkspace, c: &ClimbConfig, root: &std::path::Path) {
    let _ = run_climb("spec", g, i, w, c, &Fx, root);
}"#,
        &["takes 6 arguments", "7 arguments", "E0061"],
    );
    assert_boundary_failure(
        &root,
        "run_climb_rejects_manifest",
        r#"use nano_verify::*;
struct Fx;
impl Effects for Fx {
    async fn generate(&self, _: &str, _: &str) -> Result<String, VerifyError> { unimplemented!() }
    fn emit_event(&self, _: EngineEvent) {}
    fn now_millis(&self) -> u64 { 0 }
    fn cancellation_requested(&self) -> bool { false }
}
fn attack(g: &GateInvocation, i: &[(String, FailCategory)], w: ArtifactWorkspace, c: &ClimbConfig, m: &ExpectedChangeManifest) {
    let _ = run_climb("spec", g, i, w, c, &Fx, m);
}"#,
        &["takes 6 arguments", "7 arguments", "E0061"],
    );
    assert_boundary_failure(
        &root,
        "outcome_has_no_manifest",
        r#"fn attack(outcome: &nano_verify::ClimbOutcome) {
    let _: &nano_verify::ExpectedChangeManifest = outcome.manifest();
}"#,
        &["no method named `manifest`", "E0599"],
    );
}

#[test]
fn supported_downstream_surface_compiles() {
    let root = FixtureRoot::new();
    let output = root.check(
        "supported_surface",
        r#"use nano_verify::*;

fn inspect_diff(diff: &CandidateDiff) {
    let _: &[String] = diff.paths();
    let _: &str = diff.bytes_sha256();
}

fn inspect_manifest(manifest: &ExpectedChangeManifest) {
    let _: &str = manifest.base_tree_digest();
    let _: &str = manifest.diff_digest();
    for entry in manifest.entries() {
        let _: &str = entry.path();
        let _: ChangeKind = entry.kind();
        let _: Option<&str> = entry.postimage_sha256();
    }
}

fn inspect_artifact(artifact: &CandidateArtifact) {
    let _: &str = artifact.bytes_sha256();
    let _: Result<Vec<u8>, VerifyError> = artifact.read_exact_bytes();
}

fn inspect_outcome(outcome: &ClimbOutcome) {
    let _: &TerminalState = outcome.terminal();
    let _: [i64; 2] = outcome.score();
    let _: &[String] = outcome.fails();
    let _: u32 = outcome.rounds_used();
    let _: bool = outcome.escalated();
    let _: StopReason = outcome.stop_reason();
    let _: Option<f64> = outcome.cost_usd();
    let _: &[LogEntry] = outcome.log();
    let _: Option<&CandidateArtifact> = outcome.accepted_artifact();
    if let Some(artifact) = outcome.accepted_artifact() {
        inspect_artifact(artifact);
    }
}

fn supported(inv: &GateInvocation, path: &std::path::Path, inventory: &[(String, FailCategory)], root: &std::path::Path) {
    let diff = parse_candidate_diff(b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n").unwrap();
    inspect_diff(&diff);
    let manifest = derive_expected_changes(&diff, root).unwrap();
    inspect_manifest(&manifest);
    let _workspace = create_artifact_workspace().unwrap();
    let _future = run_gate(inv, path, inventory);
}
"#,
    );
    let stderr = diagnostics(&output);
    assert!(
        output.status.success(),
        "supported downstream API failed to compile:\n{stderr}"
    );
}
