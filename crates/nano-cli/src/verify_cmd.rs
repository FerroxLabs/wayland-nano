use std::io::Write;
use std::path::{Path, PathBuf};

const DEFAULT_DEADLINE_MS: u64 = 600_000;
const MAX_DEADLINE_MS: u64 = 3_600_000;
const MAX_ESCALATION_MODELS: usize = 4;
const EMPTY_REGISTRY_BOOTSTRAP: &[u8] = b"{\"gates\":{},\"requirements\":{},\"schema\":1}";
const USAGE: &str = "usage: wayland-nano verify --requirement <id> [--gate <gate-id>] [--task <text>] [--budget N] --cheap-model <id> --escalation-model <id> [--escalation-model <id> ...] [--receipt-out <path>] [--deadline-ms N] [--json] | verify --verify-receipt <path> [--json]";

pub struct VerifyEvents<W: Write> {
    out: W,
    run_id: String,
    seq: u64,
}

impl<W: Write> VerifyEvents<W> {
    pub fn new(out: W, run_id: String) -> Self {
        Self {
            out,
            run_id,
            seq: 0,
        }
    }

    fn emit(&mut self, body: serde_json::Value) {
        let mut frame = serde_json::json!({
            "v": 1,
            "session_id": self.run_id,
            "seq": self.seq,
        });
        if let (Some(frame), Some(body)) = (frame.as_object_mut(), body.as_object()) {
            frame.extend(body.clone());
        }
        self.seq = self.seq.saturating_add(1);
        if let Ok(mut line) = serde_json::to_vec(&frame) {
            line.push(b'\n');
            let _ = self.out.write_all(&line);
            let _ = self.out.flush();
        }
    }

    pub fn verify_started(&mut self, requirement: &str, gate_id: &str) {
        self.emit(serde_json::json!({"type":"verify_started", "requirement":requirement, "gate_id":gate_id}));
    }

    pub fn check_verdict(&mut self, verdict: &nano_verify::CheckVerdict) {
        self.emit(serde_json::json!({"type":"check_verdict", "id":verdict.id, "category":verdict.category, "passed":verdict.passed}));
    }

    pub fn climb_update(&mut self, entry: &nano_verify::LogEntry) {
        self.emit(serde_json::json!({"type":"climb_update", "phase":entry.phase, "score":entry.score, "accepted":entry.accepted, "code":entry.code}));
    }

    pub fn apply_started(&mut self, gate_id: &str) {
        self.emit(
            serde_json::json!({"type":"apply_started", "gate_id":gate_id, "code":"apply_started"}),
        );
    }

    pub fn apply_verified(&mut self, gate_id: &str, changed_files: usize) {
        self.emit(serde_json::json!({"type":"apply_verified", "gate_id":gate_id, "changed_files":changed_files, "code":"apply_verified"}));
    }

    pub fn receipt_minted(&mut self, receipt: &nano_verify::Receipt, _path: &Path) {
        self.emit(serde_json::json!({"type":"receipt_minted", "requirement":receipt.requirement, "gate_id":receipt.gate_id}));
    }

    pub fn verify_completed(&mut self, terminal: &nano_verify::TerminalState, exit_code: i32) {
        let terminal = match terminal {
            nano_verify::TerminalState::Verified => "verified",
            nano_verify::TerminalState::CriteriaChecked => "criteria_checked",
            nano_verify::TerminalState::SelfChecked => "self_checked",
            nano_verify::TerminalState::NeedsEscalation => "needs_escalation",
            nano_verify::TerminalState::Blocked(_) => "blocked",
            nano_verify::TerminalState::Cancelled => "cancelled",
            nano_verify::TerminalState::TimedOut => "timed_out",
            nano_verify::TerminalState::PermissionDenied => "permission_denied",
            nano_verify::TerminalState::CrashedRecovered => "crashed_recovered",
            nano_verify::TerminalState::Superseded => "superseded",
        };
        self.emit(serde_json::json!({"type":"verify_completed", "terminal":terminal, "exit_code":exit_code}));
    }

    pub fn error(&mut self, code: &str) {
        let code = match code {
            "generation_failed" | "artifact_failed" | "git_failed" | "store_failed" => code,
            _ => "runtime_failed",
        };
        self.emit(serde_json::json!({"type":"error", "code":code}));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyMode {
    Mint {
        requirement: String,
        gate: Option<String>,
        task: Option<String>,
        budget: Option<u32>,
        cheap_model: String,
        escalation_models: Vec<String>,
        deadline_ms: u64,
        receipt_out: Option<PathBuf>,
        json: bool,
    },
    CheckReceipt {
        path: PathBuf,
        json: bool,
    },
    RunOnly {
        gate: String,
        deadline_ms: u64,
        json: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyParams {
    pub mode: VerifyMode,
}

#[derive(Default)]
struct ParsedArgs {
    requirement: Option<String>,
    gate: Option<String>,
    task: Option<String>,
    budget: Option<u32>,
    cheap_model: Option<String>,
    escalation_models: Vec<String>,
    deadline_ms: Option<u64>,
    receipt_out: Option<PathBuf>,
    verify_receipt: Option<PathBuf>,
    run_only: bool,
    json: bool,
}

pub fn parse_args(args: &[String]) -> Result<VerifyParams, i32> {
    parse_args_inner(args).map_err(|()| {
        eprintln!("{USAGE}");
        2
    })
}

fn parse_args_inner(args: &[String]) -> Result<VerifyParams, ()> {
    let mut parsed = ParsedArgs::default();
    let mut index = 0;
    while let Some(flag) = args.get(index) {
        let take = |index: &mut usize| -> Result<String, ()> {
            *index += 1;
            let value = args.get(*index).ok_or(())?;
            if value.is_empty() {
                return Err(());
            }
            Ok(value.clone())
        };
        match flag.as_str() {
            "--requirement" if parsed.requirement.is_none() => {
                parsed.requirement = Some(take(&mut index)?);
            }
            "--gate" if parsed.gate.is_none() => parsed.gate = Some(take(&mut index)?),
            "--task" if parsed.task.is_none() => parsed.task = Some(take(&mut index)?),
            "--budget" if parsed.budget.is_none() => {
                let value = take(&mut index)?.parse::<u32>().map_err(|_| ())?;
                if value == 0 {
                    return Err(());
                }
                parsed.budget = Some(value);
            }
            "--cheap-model" if parsed.cheap_model.is_none() => {
                parsed.cheap_model = Some(take(&mut index)?);
            }
            "--escalation-model" => {
                let value = take(&mut index)?;
                if parsed.escalation_models.len() == MAX_ESCALATION_MODELS
                    || parsed.escalation_models.contains(&value)
                {
                    return Err(());
                }
                parsed.escalation_models.push(value);
            }
            "--deadline-ms" if parsed.deadline_ms.is_none() => {
                let value = take(&mut index)?.parse::<u64>().map_err(|_| ())?;
                if value == 0 || value > MAX_DEADLINE_MS {
                    return Err(());
                }
                parsed.deadline_ms = Some(value);
            }
            "--receipt-out" if parsed.receipt_out.is_none() => {
                parsed.receipt_out = Some(PathBuf::from(take(&mut index)?));
            }
            "--verify-receipt" if parsed.verify_receipt.is_none() => {
                parsed.verify_receipt = Some(PathBuf::from(take(&mut index)?));
            }
            "--run-only" if !parsed.run_only => parsed.run_only = true,
            "--json" if !parsed.json => parsed.json = true,
            _ => return Err(()),
        }
        index += 1;
    }

    if let Some(path) = parsed.verify_receipt {
        if parsed.requirement.is_some()
            || parsed.gate.is_some()
            || parsed.task.is_some()
            || parsed.budget.is_some()
            || parsed.cheap_model.is_some()
            || !parsed.escalation_models.is_empty()
            || parsed.deadline_ms.is_some()
            || parsed.receipt_out.is_some()
            || parsed.run_only
        {
            return Err(());
        }
        return Ok(VerifyParams {
            mode: VerifyMode::CheckReceipt {
                path,
                json: parsed.json,
            },
        });
    }

    let deadline_ms = parsed.deadline_ms.unwrap_or(DEFAULT_DEADLINE_MS);
    if parsed.run_only {
        if parsed.requirement.is_some()
            || parsed.task.is_some()
            || parsed.budget.is_some()
            || parsed.cheap_model.is_some()
            || !parsed.escalation_models.is_empty()
            || parsed.receipt_out.is_some()
        {
            return Err(());
        }
        return Ok(VerifyParams {
            mode: VerifyMode::RunOnly {
                gate: parsed.gate.ok_or(())?,
                deadline_ms,
                json: parsed.json,
            },
        });
    }

    Ok(VerifyParams {
        mode: VerifyMode::Mint {
            requirement: parsed.requirement.ok_or(())?,
            gate: parsed.gate,
            task: parsed.task,
            budget: parsed.budget,
            cheap_model: parsed.cheap_model.ok_or(())?,
            escalation_models: if parsed.escalation_models.is_empty() {
                return Err(());
            } else {
                parsed.escalation_models
            },
            deadline_ms,
            receipt_out: parsed.receipt_out,
            json: parsed.json,
        },
    })
}

#[allow(async_fn_in_trait, dead_code, clippy::result_unit_err)]
pub trait VerifyRuntime {
    fn now_millis(&self) -> u64;
    async fn generate(&self, _model: &str, _prompt: &str) -> Result<String, ()> {
        Err(())
    }
    fn canonical_repo_root(&self, workspace: &Path) -> Result<PathBuf, ()> {
        canonical_repo_root(workspace)
    }
    fn temp_preflight(&self) -> Result<(), ()> {
        temp_preflight()
    }
    fn load_registry(
        &self,
        repo_root: &Path,
        gate: Option<&str>,
        requirement: Option<&str>,
    ) -> Result<Option<nano_verify::GateRegistry>, i32> {
        load_requested_registry(repo_root, gate, requirement)
    }
    fn load_receipt_registry(&self, repo_root: &Path) -> Result<nano_verify::GateRegistry, ()> {
        nano_verify::load_registry(repo_root).map_err(|_| ())
    }
    fn resolve_artifact(&self, root: &Path, relative: &Path) -> Result<PathBuf, ()> {
        confined_existing(root, relative)
    }
    fn inventory(&self, card: &Path) -> Result<Vec<(String, nano_verify::FailCategory)>, ()> {
        nano_verify::check_inventory(card).map_err(|_| ())
    }
    fn receipt_budget_ms(&self) -> Result<u64, ()> {
        receipt_budget_ms()
    }
    fn receipt_worktree_path(&self) -> Result<PathBuf, ()> {
        receipt_worktree_path()
    }
    fn add_receipt_worktree(
        &self,
        repo_root: &Path,
        worktree: &Path,
        fix_commit: &str,
        timeout_ms: u64,
    ) -> Result<(), ()> {
        git_success_bounded(
            repo_root,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.to_string_lossy(),
                fix_commit,
            ],
            timeout_ms,
        )
    }
    fn verify_receipt_worktree(
        &self,
        worktree: &Path,
        fix_commit: &str,
        timeout_ms: u64,
    ) -> Result<(), ()> {
        verify_receipt_worktree(worktree, fix_commit, timeout_ms)
    }
    fn cleanup_receipt_worktree(
        &self,
        repo_root: &Path,
        worktree: &Path,
        timeout_ms: u64,
    ) -> Result<(), ()> {
        cleanup_receipt_worktree(repo_root, worktree, timeout_ms)
    }
    async fn run_gate(
        &self,
        invocation: &nano_verify::GateInvocation,
        artifact: &Path,
        inventory: &[(String, nano_verify::FailCategory)],
    ) -> nano_verify::ExecutionGateOutcome;
    async fn run_baseline_gate(
        &self,
        invocation: &nano_verify::GateInvocation,
        artifact: &Path,
        inventory: &[(String, nano_verify::FailCategory)],
    ) -> nano_verify::BaselineGateExecution {
        nano_verify::run_gate_baseline_execution(invocation, artifact, inventory).await
    }
}

struct ProductionRuntime {
    epoch: std::time::Instant,
}

impl ProductionRuntime {
    fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),
        }
    }
}

impl VerifyRuntime for ProductionRuntime {
    fn now_millis(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    async fn generate(&self, model: &str, prompt: &str) -> Result<String, ()> {
        production_generate(model, prompt).await
    }

    async fn run_gate(
        &self,
        invocation: &nano_verify::GateInvocation,
        artifact: &Path,
        inventory: &[(String, nano_verify::FailCategory)],
    ) -> nano_verify::ExecutionGateOutcome {
        nano_verify::run_gate_baseline_execution(invocation, artifact, inventory)
            .await
            .outcome
    }
}

pub async fn run(_home: &Path, workspace: &Path, params: &VerifyParams) -> i32 {
    let mut events = VerifyEvents::new(
        std::io::stdout(),
        format!(
            "wayland-nano-verify-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ),
    );
    run_with_runtime_and_events(
        _home,
        workspace,
        params,
        &ProductionRuntime::new(),
        &mut events,
    )
    .await
}

#[cfg(test)]
async fn run_with<R: VerifyRuntime>(workspace: &Path, params: &VerifyParams, runtime: &R) -> i32 {
    run_with_runtime(
        &workspace.join(".nano-test-home"),
        workspace,
        params,
        runtime,
    )
    .await
}

/// Deterministic integration seam for the offline verify fixture battery.
pub async fn run_with_runtime<R: VerifyRuntime>(
    home: &Path,
    workspace: &Path,
    params: &VerifyParams,
    runtime: &R,
) -> i32 {
    let mut events = VerifyEvents::new(std::io::sink(), "wayland-nano-verify-test".into());
    run_with_runtime_and_events(home, workspace, params, runtime, &mut events).await
}

pub async fn run_with_runtime_and_events<R: VerifyRuntime, W: Write>(
    home: &Path,
    workspace: &Path,
    params: &VerifyParams,
    runtime: &R,
    events: &mut VerifyEvents<W>,
) -> i32 {
    match &params.mode {
        VerifyMode::CheckReceipt { path, json } => {
            check_receipt(workspace, path, *json, runtime).await
        }
        VerifyMode::Mint {
            requirement,
            gate,
            task,
            budget,
            cheap_model,
            escalation_models,
            deadline_ms,
            receipt_out,
            json: _,
        } => {
            let Some(deadline) = runtime.now_millis().checked_add(*deadline_ms) else {
                return 2;
            };
            mint_until_materializer(
                workspace,
                requirement,
                gate.as_deref(),
                task.as_deref(),
                *budget,
                cheap_model,
                escalation_models,
                nano_verify::RunDeadline {
                    monotonic_millis: deadline,
                },
                home,
                receipt_out.as_deref(),
                runtime,
                events,
            )
            .await
        }
        VerifyMode::RunOnly {
            gate,
            deadline_ms,
            json,
        } => run_only(workspace, gate, *deadline_ms, *json, runtime).await,
    }
}

async fn check_receipt<R: VerifyRuntime>(
    workspace: &Path,
    path: &Path,
    json: bool,
    runtime: &R,
) -> i32 {
    let Ok(bytes) = std::fs::read(path) else {
        return 2;
    };
    let receipt = match receipt_structure(&bytes) {
        Ok(receipt) => receipt,
        Err(verdict) => return finish_receipt(verdict, receipt_identity(&bytes), json),
    };
    let identity = Some((receipt.requirement.clone(), receipt.fix_commit.clone()));
    let Ok(repo_root) = runtime.canonical_repo_root(workspace) else {
        return finish_receipt(nano_verify::VerifyVerdict::Unverifiable, identity, json);
    };
    let Ok(registry) = runtime.load_receipt_registry(&repo_root) else {
        return finish_receipt(nano_verify::VerifyVerdict::Unverifiable, identity, json);
    };
    if let Err(verdict) = preflight_verdict(nano_verify::preflight_receipt(
        &repo_root, &bytes, &registry,
    )) {
        return finish_receipt(verdict, identity, json);
    }
    let Some(entry) = registry.gates.get(&receipt.gate_id) else {
        return finish_receipt(nano_verify::VerifyVerdict::GateMismatch, identity, json);
    };
    if receipt.test != entry.script {
        return finish_receipt(nano_verify::VerifyVerdict::GateMismatch, identity, json);
    }
    let verdict = rerun_ready_receipt(&repo_root, &receipt, &registry, runtime).await;
    finish_receipt(verdict, identity, json)
}

async fn rerun_ready_receipt<R: VerifyRuntime>(
    repo_root: &Path,
    receipt: &nano_verify::Receipt,
    registry: &nano_verify::GateRegistry,
    runtime: &R,
) -> nano_verify::VerifyVerdict {
    use nano_verify::VerifyVerdict::{GateMismatch, Unverifiable, Valid};

    let Ok(budget_ms) = runtime.receipt_budget_ms() else {
        return Unverifiable;
    };
    let Some(deadline) = runtime.now_millis().checked_add(budget_ms) else {
        return Unverifiable;
    };
    if runtime.temp_preflight().is_err() {
        return Unverifiable;
    }
    let Ok(worktree) = runtime.receipt_worktree_path() else {
        return Unverifiable;
    };

    let provisional = async {
        let remaining = receipt_remaining(runtime, deadline)?;
        runtime.add_receipt_worktree(repo_root, &worktree, &receipt.fix_commit, remaining)?;
        let remaining = receipt_remaining(runtime, deadline)?;
        runtime.verify_receipt_worktree(&worktree, &receipt.fix_commit, remaining)?;
        let entry = registry.gates.get(&receipt.gate_id).ok_or(())?;
        let artifact = runtime.resolve_artifact(&worktree, Path::new(&entry.run_artifact))?;
        let inventory = runtime.inventory(&worktree.join(&entry.card))?;
        let remaining = receipt_remaining(runtime, deadline)?;
        let cwd = match entry.closure.cwd_policy {
            nano_verify::CwdPolicy::RepoRoot => worktree.clone(),
            nano_verify::CwdPolicy::GateDir => worktree
                .join(&entry.card)
                .parent()
                .map(Path::to_path_buf)
                .ok_or(())?,
        };
        let invocation = nano_verify::GateInvocation {
            argv: entry.closure.argv.iter().map(Into::into).collect(),
            cwd,
            env: entry
                .closure
                .env
                .iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            timeout: std::time::Duration::from_millis(remaining.min(400_000)),
            gate_id: receipt.gate_id.clone(),
        };
        let outcome = runtime.run_gate(&invocation, &artifact, &inventory).await;
        receipt_remaining(runtime, deadline)?;
        Ok::<_, ()>(match outcome {
            nano_verify::ExecutionGateOutcome::Green { .. } => Valid,
            nano_verify::ExecutionGateOutcome::Red { .. } => GateMismatch,
            nano_verify::ExecutionGateOutcome::FailClosed(_) => Unverifiable,
        })
    }
    .await
    .unwrap_or(Unverifiable);

    // Cleanup is mandatory even after the verification budget expires. Give the
    // bounded Git cleanup sequence its own finite slice of the configured budget;
    // otherwise a timeout would reduce cleanup to a guaranteed one-millisecond race.
    let cleanup_budget = budget_ms.min(30_000);
    if runtime
        .cleanup_receipt_worktree(repo_root, &worktree, cleanup_budget)
        .is_err()
    {
        Unverifiable
    } else {
        provisional
    }
}

fn receipt_remaining<R: VerifyRuntime>(runtime: &R, deadline: u64) -> Result<u64, ()> {
    deadline
        .checked_sub(runtime.now_millis())
        .filter(|remaining| *remaining > 0)
        .ok_or(())
}

fn receipt_budget_ms() -> Result<u64, ()> {
    const DEFAULT: u64 = 120_000;
    const MAX: u64 = 600_000;
    match std::env::var("NANO_VERIFY_RECEIPT_BUDGET_MS") {
        Ok(value) => value
            .parse::<u64>()
            .ok()
            .filter(|value| (1..=MAX).contains(value))
            .ok_or(()),
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT),
        Err(std::env::VarError::NotUnicode(_)) => Err(()),
    }
}

fn receipt_worktree_path() -> Result<PathBuf, ()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = canonical_env_dir("TEMP")?;
    if !is_f_drive(&root) {
        return Err(());
    }
    let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = native_windows_path(root.join(format!(
        "wayland-nano-receipt-{}-{nonce}",
        std::process::id()
    )));
    if path.exists() {
        return Err(());
    }
    Ok(path)
}

fn verify_receipt_worktree(worktree: &Path, fix_commit: &str, timeout_ms: u64) -> Result<(), ()> {
    let head = git_output_bounded(worktree, &["rev-parse", "HEAD"], timeout_ms)?;
    if String::from_utf8(head).map_err(|_| ())?.trim() != fix_commit {
        return Err(());
    }
    let status = git_output_bounded(worktree, &["status", "--porcelain=v1"], timeout_ms)?;
    if !status.is_empty() {
        return Err(());
    }
    Ok(())
}

fn cleanup_receipt_worktree(repo_root: &Path, worktree: &Path, timeout_ms: u64) -> Result<(), ()> {
    let path_text = worktree.to_string_lossy();
    let _ = git_success_bounded(
        repo_root,
        &["worktree", "remove", "--force", &path_text],
        timeout_ms,
    );
    git_success_bounded(repo_root, &["worktree", "prune"], timeout_ms)?;
    if worktree.exists() {
        return Err(());
    }
    let listed = git_output_bounded(repo_root, &["worktree", "list", "--porcelain"], timeout_ms)?;
    let expected = normalized_path_text(worktree);
    let residue = String::from_utf8(listed)
        .map_err(|_| ())?
        .lines()
        .any(|line| {
            line.strip_prefix("worktree ")
                .is_some_and(|path| normalized_path_text(Path::new(path)) == expected)
        });
    if residue {
        return Err(());
    }
    Ok(())
}

fn normalized_path_text(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    return value.to_ascii_lowercase();
    #[cfg(not(windows))]
    value
}

fn git_success_bounded(repo: &Path, args: &[&str], timeout_ms: u64) -> Result<(), ()> {
    git_output_bounded(repo, args, timeout_ms).map(|_| ())
}

fn git_output_bounded(repo: &Path, args: &[&str], timeout_ms: u64) -> Result<Vec<u8>, ()> {
    use std::process::Stdio;

    if timeout_ms == 0 {
        return Err(());
    }
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|_| ())? {
            Some(status) => {
                let mut stdout = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    std::io::Read::read_to_end(&mut pipe, &mut stdout).map_err(|_| ())?;
                }
                return status.success().then_some(stdout).ok_or(());
            }
            None if started.elapsed() >= std::time::Duration::from_millis(timeout_ms) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

fn receipt_structure(bytes: &[u8]) -> Result<nano_verify::Receipt, nano_verify::VerifyVerdict> {
    use nano_verify::VerifyVerdict::{NeverRed, Unverifiable};

    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Unverifiable)?;
    let object = value.as_object().ok_or(Unverifiable)?;
    const TOP: [&str; 9] = [
        "schema",
        "requirement",
        "test",
        "gate_id",
        "gate_closure_digest",
        "failing_run",
        "fix_commit",
        "minted_at",
        "minted_by",
    ];
    if object.keys().any(|key| !TOP.contains(&key.as_str()))
        || object.get("schema").and_then(serde_json::Value::as_u64) != Some(1)
    {
        return Err(Unverifiable);
    }
    let failing = object
        .get("failing_run")
        .and_then(serde_json::Value::as_object);
    const FAILING: [&str; 3] = ["exit_code", "log_digest", "observed_at_commit"];
    if failing.is_some_and(|fields| fields.keys().any(|key| !FAILING.contains(&key.as_str()))) {
        return Err(Unverifiable);
    }
    let receipt: nano_verify::Receipt = serde_json::from_value(value).map_err(|_| NeverRed)?;
    if receipt.failing_run.exit_code == 0
        || !lower_hex(&receipt.failing_run.log_digest, 64)
        || !lower_hex(&receipt.failing_run.observed_at_commit, 40)
        || !lower_hex(&receipt.fix_commit, 40)
        || receipt.requirement.is_empty()
        || receipt.test.is_empty()
        || receipt.gate_id.is_empty()
        || receipt.gate_closure_digest.is_empty()
    {
        return Err(NeverRed);
    }
    Ok(receipt)
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn preflight_verdict(
    preflight: nano_verify::ReceiptPreflight,
) -> Result<(), nano_verify::VerifyVerdict> {
    use nano_verify::{ReceiptPreflight as P, VerifyVerdict as V};
    match preflight {
        P::Ready => Ok(()),
        P::NeverRed => Err(V::NeverRed),
        P::FabricatedCommit => Err(V::FabricatedCommit),
        P::GateMismatch => Err(V::GateMismatch),
        P::AncestryUnproven => Err(V::AncestryUnproven),
        P::Unverifiable => Err(V::Unverifiable),
    }
}

fn receipt_identity(bytes: &[u8]) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    Some((
        value.get("requirement")?.as_str()?.to_owned(),
        value.get("fix_commit")?.as_str()?.to_owned(),
    ))
}

fn finish_receipt(
    verdict: nano_verify::VerifyVerdict,
    identity: Option<(String, String)>,
    json: bool,
) -> i32 {
    if json {
        let (requirement, fix_commit) = identity.unwrap_or_default();
        if let Ok(line) = serde_json::to_string(&serde_json::json!({
            "schema": "nano.receipt-verdict/1",
            "decision": verdict,
            "requirement": requirement,
            "fix_commit": fix_commit,
            "re_derived": true,
        })) {
            println!("{line}");
        }
    }
    if verdict == nano_verify::VerifyVerdict::Valid {
        0
    } else {
        6
    }
}

async fn run_only<R: VerifyRuntime>(
    workspace: &Path,
    gate_id: &str,
    deadline_ms: u64,
    json: bool,
    runtime: &R,
) -> i32 {
    let Some(monotonic_millis) = runtime.now_millis().checked_add(deadline_ms) else {
        return 2;
    };
    let deadline = nano_verify::RunDeadline { monotonic_millis };
    let Ok(repo_root) = runtime.canonical_repo_root(workspace) else {
        return 2;
    };
    if runtime.temp_preflight().is_err() {
        return 3;
    }
    if expired(runtime, deadline) {
        return 3;
    }
    let Ok(Some(registry)) = runtime.load_registry(&repo_root, Some(gate_id), None) else {
        return 2;
    };
    let Some(entry) = registry.gates.get(gate_id) else {
        return 2;
    };
    if expired(runtime, deadline) {
        return 3;
    }
    let Ok(artifact) = runtime.resolve_artifact(&repo_root, Path::new(&entry.run_artifact)) else {
        return 2;
    };
    let Ok(inventory) = runtime.inventory(&repo_root.join(&entry.card)) else {
        return 2;
    };
    let Some(remaining_ms) = remaining(runtime, deadline) else {
        return 3;
    };
    let cwd = match entry.closure.cwd_policy {
        nano_verify::CwdPolicy::RepoRoot => repo_root.clone(),
        nano_verify::CwdPolicy::GateDir => repo_root
            .join(&entry.card)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| repo_root.clone()),
    };
    let invocation = nano_verify::GateInvocation {
        argv: entry.closure.argv.iter().map(Into::into).collect(),
        cwd,
        env: entry
            .closure
            .env
            .iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        timeout: std::time::Duration::from_millis(remaining_ms.min(400_000)),
        gate_id: gate_id.to_owned(),
    };
    let outcome = runtime.run_gate(&invocation, &artifact, &inventory).await;
    if expired(runtime, deadline) {
        return 3;
    }
    let (exit, outcome_name, verdicts) = match outcome {
        nano_verify::ExecutionGateOutcome::Green { verdicts } => (0, "green", verdicts),
        nano_verify::ExecutionGateOutcome::Red { verdicts } => (3, "red", verdicts),
        nano_verify::ExecutionGateOutcome::FailClosed(_) => (3, "fail_closed", Vec::new()),
    };
    if json {
        let closed: Vec<_> = verdicts
            .iter()
            .map(|verdict| {
                serde_json::json!({
                    "id": verdict.id,
                    "category": verdict.category,
                    "passed": verdict.passed,
                })
            })
            .collect();
        if let Ok(line) = serde_json::to_string(
            &serde_json::json!({"v":1,"outcome":outcome_name,"verdicts":closed}),
        ) {
            println!("{line}");
        }
    }
    exit
}

fn expired<R: VerifyRuntime>(runtime: &R, deadline: nano_verify::RunDeadline) -> bool {
    runtime.now_millis() >= deadline.monotonic_millis
}

fn remaining<R: VerifyRuntime>(runtime: &R, deadline: nano_verify::RunDeadline) -> Option<u64> {
    deadline
        .monotonic_millis
        .checked_sub(runtime.now_millis())
        .filter(|remaining| *remaining > 0)
}

fn canonical_repo_root(workspace: &Path) -> Result<PathBuf, ()> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace)
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    let root = PathBuf::from(String::from_utf8(output.stdout).map_err(|_| ())?.trim())
        .canonicalize()
        .map_err(|_| ())?;
    if !is_f_drive(&root) {
        return Err(());
    }
    Ok(root)
}

fn temp_preflight() -> Result<(), ()> {
    let temp = canonical_env_dir("TEMP")?;
    let tmp = canonical_env_dir("TMP")?;
    if temp != tmp || !is_f_drive(&temp) {
        return Err(());
    }
    Ok(())
}

fn canonical_env_dir(name: &str) -> Result<PathBuf, ()> {
    let raw = std::env::var_os(name).ok_or(())?;
    let path = PathBuf::from(raw);
    if !path.is_absolute()
        || !path.is_dir()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(());
    }
    let canonical = path.canonicalize().map_err(|_| ())?;
    Ok(canonical)
}

#[cfg(windows)]
fn is_f_drive(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(b'F' | b'f') | std::path::Prefix::VerbatimDisk(b'F' | b'f'))
    )
}

#[cfg(not(windows))]
fn is_f_drive(path: &Path) -> bool {
    path.is_absolute()
}

fn confined_existing(root: &Path, relative: &Path) -> Result<PathBuf, ()> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(());
    }
    let canonical_root = root.canonicalize().map_err(|_| ())?;
    let path = canonical_root
        .join(relative)
        .canonicalize()
        .map_err(|_| ())?;
    if !path.starts_with(&canonical_root) || !(path.is_file() || path.is_dir()) {
        return Err(());
    }
    Ok(path)
}

pub fn load_requested_registry(
    repo_root: &Path,
    requested_gate: Option<&str>,
    requested_requirement: Option<&str>,
) -> Result<Option<nano_verify::GateRegistry>, i32> {
    let bytes = std::fs::read(repo_root.join("gates/registry.json")).map_err(|_| 2)?;
    if bytes == EMPTY_REGISTRY_BOOTSTRAP {
        return if requested_gate.is_some() || requested_requirement.is_some() {
            Err(2)
        } else {
            Ok(None)
        };
    }
    nano_verify::load_registry(repo_root)
        .map(Some)
        .map_err(|_| 2)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    #[test]
    fn confined_existing_accepts_nonverbatim_windows_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("artifact.txt"), b"ok").unwrap();
        let lexical =
            std::path::PathBuf::from(root.path().to_string_lossy().trim_start_matches(r"\\?\"));
        let resolved =
            super::confined_existing(&lexical, std::path::Path::new("artifact.txt")).unwrap();
        assert_eq!(std::fs::read(resolved).unwrap(), b"ok");
    }

    use super::{
        EMPTY_REGISTRY_BOOTSTRAP, VerifyEvents, VerifyMode, VerifyParams, load_requested_registry,
        parse_args,
    };
    use std::path::{Path, PathBuf};

    fn portable_test_temp_root() -> PathBuf {
        ["TEMP", "TMP"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .find(|path| path.is_dir())
            .unwrap_or_else(std::env::temp_dir)
    }

    fn portable_tempdir(prefix: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(portable_test_temp_root())
            .expect("create test directory under the platform temp root")
    }

    fn receipt_bytes(exit_code: i64, log_digest: &str) -> Vec<u8> {
        serde_json::to_vec(&nano_verify::Receipt {
            schema: 1,
            requirement: "CLI-01".into(),
            test: "gates/demo/gate.cmd".into(),
            gate_id: "demo".into(),
            gate_closure_digest: "a".repeat(64),
            failing_run: nano_verify::FailingRun {
                exit_code,
                log_digest: log_digest.into(),
                observed_at_commit: "b".repeat(40),
            },
            fix_commit: "c".repeat(40),
            minted_at: "2026-08-21T00:00:00Z".into(),
            minted_by: "wayland-nano 0.3.0".into(),
        })
        .unwrap()
    }

    #[test]
    fn receipt_preflight_structure_obeys_parse_then_red_order() {
        assert_eq!(
            super::receipt_structure(b"{not-json"),
            Err(nano_verify::VerifyVerdict::Unverifiable)
        );
        let unknown = br#"{"schema":1,"unknown":true}"#;
        assert_eq!(
            super::receipt_structure(unknown),
            Err(nano_verify::VerifyVerdict::Unverifiable)
        );
        assert_eq!(
            super::receipt_structure(&receipt_bytes(0, &"a".repeat(64))),
            Err(nano_verify::VerifyVerdict::NeverRed)
        );
        assert_eq!(
            super::receipt_structure(&receipt_bytes(1, "malformed")),
            Err(nano_verify::VerifyVerdict::NeverRed)
        );
    }

    #[test]
    fn receipt_preflight_maps_every_imported_terminal_verdict() {
        use nano_verify::{ReceiptPreflight as P, VerifyVerdict as V};
        assert_eq!(super::preflight_verdict(P::NeverRed), Err(V::NeverRed));
        assert_eq!(
            super::preflight_verdict(P::FabricatedCommit),
            Err(V::FabricatedCommit)
        );
        assert_eq!(
            super::preflight_verdict(P::GateMismatch),
            Err(V::GateMismatch)
        );
        assert_eq!(
            super::preflight_verdict(P::AncestryUnproven),
            Err(V::AncestryUnproven)
        );
        assert_eq!(
            super::preflight_verdict(P::Unverifiable),
            Err(V::Unverifiable)
        );
        assert_eq!(super::preflight_verdict(P::Ready), Ok(()));
    }

    #[test]
    fn changed_structurally_valid_log_digest_is_provenance_not_offline_proof() {
        let first = super::receipt_structure(&receipt_bytes(1, &"1".repeat(64))).unwrap();
        let changed = super::receipt_structure(&receipt_bytes(1, &"2".repeat(64))).unwrap();
        assert_ne!(first.failing_run.log_digest, changed.failing_run.log_digest);
    }

    struct ReceiptRerunRuntime {
        repo_root: PathBuf,
        times: std::sync::Mutex<std::collections::VecDeque<u64>>,
        outcome: nano_verify::ExecutionGateOutcome,
        add_ok: bool,
        identity_ok: bool,
        cleanup_ok: bool,
        gate_calls: std::sync::atomic::AtomicUsize,
        cleanup_calls: std::sync::atomic::AtomicUsize,
    }

    impl super::VerifyRuntime for ReceiptRerunRuntime {
        fn now_millis(&self) -> u64 {
            self.times.lock().unwrap().pop_front().unwrap_or(u64::MAX)
        }

        fn temp_preflight(&self) -> Result<(), ()> {
            self.repo_root.is_dir().then_some(()).ok_or(())
        }

        fn receipt_budget_ms(&self) -> Result<u64, ()> {
            Ok(100)
        }

        fn receipt_worktree_path(&self) -> Result<std::path::PathBuf, ()> {
            Ok(self.repo_root.clone())
        }

        fn add_receipt_worktree(&self, _: &Path, _: &Path, _: &str, _: u64) -> Result<(), ()> {
            self.add_ok.then_some(()).ok_or(())
        }

        fn verify_receipt_worktree(&self, _: &Path, _: &str, _: u64) -> Result<(), ()> {
            self.identity_ok.then_some(()).ok_or(())
        }

        fn cleanup_receipt_worktree(&self, _: &Path, _: &Path, _: u64) -> Result<(), ()> {
            self.cleanup_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.cleanup_ok.then_some(()).ok_or(())
        }

        async fn run_gate(
            &self,
            _: &nano_verify::GateInvocation,
            _: &Path,
            _: &[(String, nano_verify::FailCategory)],
        ) -> nano_verify::ExecutionGateOutcome {
            self.gate_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.outcome.clone()
        }
    }

    fn rerun_runtime(
        repo_root: &Path,
        outcome: nano_verify::ExecutionGateOutcome,
        add_ok: bool,
        identity_ok: bool,
        cleanup_ok: bool,
    ) -> ReceiptRerunRuntime {
        ReceiptRerunRuntime {
            repo_root: repo_root.to_owned(),
            times: std::sync::Mutex::new([0, 1, 2, 3, 4].into()),
            outcome,
            add_ok,
            identity_ok,
            cleanup_ok,
            gate_calls: Default::default(),
            cleanup_calls: Default::default(),
        }
    }

    fn rerun_inputs() -> (nano_verify::Receipt, nano_verify::GateRegistry) {
        let receipt: nano_verify::Receipt =
            serde_json::from_slice(&receipt_bytes(1, &"a".repeat(64))).unwrap();
        let closure = nano_verify::GateClosure {
            argv: vec!["gates/demo/gate.cmd".into()],
            env: Default::default(),
            cwd_policy: nano_verify::CwdPolicy::RepoRoot,
            wrapped_tools: Vec::new(),
        };
        let registry = nano_verify::GateRegistry {
            schema: 1,
            gates: std::collections::BTreeMap::from([(
                "demo".into(),
                nano_verify::GateRegistryEntry {
                    card: "gates/demo/card.md".into(),
                    script: "gates/demo/gate.cmd".into(),
                    closure_digest: nano_verify::closure_digest(&closure).unwrap(),
                    closure,
                    run_artifact: "artifact".into(),
                },
            )]),
            requirements: std::collections::BTreeMap::from([("CLI-01".into(), "demo".into())]),
        };
        (receipt, registry)
    }

    #[tokio::test]
    async fn receipt_rerun_green_is_valid_and_red_is_gate_mismatch() {
        let repo = fixture_repo();
        let (receipt, registry) = rerun_inputs();
        let green = rerun_runtime(
            repo.path(),
            nano_verify::ExecutionGateOutcome::Green {
                verdicts: Vec::new(),
            },
            true,
            true,
            true,
        );
        assert_eq!(
            super::rerun_ready_receipt(repo.path(), &receipt, &registry, &green).await,
            nano_verify::VerifyVerdict::Valid
        );
        assert_eq!(
            green
                .cleanup_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );

        let red = rerun_runtime(
            repo.path(),
            nano_verify::ExecutionGateOutcome::Red {
                verdicts: Vec::new(),
            },
            true,
            true,
            true,
        );
        assert_eq!(
            super::rerun_ready_receipt(repo.path(), &receipt, &registry, &red).await,
            nano_verify::VerifyVerdict::GateMismatch
        );
        assert_eq!(
            red.cleanup_calls.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn receipt_rerun_spawn_probe_and_cleanup_fail_closed_without_residue() {
        let repo = fixture_repo();
        let (receipt, registry) = rerun_inputs();
        for runtime in [
            rerun_runtime(
                repo.path(),
                nano_verify::ExecutionGateOutcome::Green {
                    verdicts: Vec::new(),
                },
                false,
                true,
                true,
            ),
            rerun_runtime(
                repo.path(),
                nano_verify::ExecutionGateOutcome::Green {
                    verdicts: Vec::new(),
                },
                true,
                false,
                true,
            ),
            rerun_runtime(
                repo.path(),
                nano_verify::ExecutionGateOutcome::Green {
                    verdicts: Vec::new(),
                },
                true,
                true,
                false,
            ),
        ] {
            assert_eq!(
                super::rerun_ready_receipt(repo.path(), &receipt, &registry, &runtime).await,
                nano_verify::VerifyVerdict::Unverifiable
            );
            assert_eq!(
                runtime
                    .cleanup_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                1
            );
        }
    }

    #[tokio::test]
    async fn receipt_rerun_timeout_starts_no_gate_and_still_cleans_up() {
        let repo = fixture_repo();
        let (receipt, registry) = rerun_inputs();
        let runtime = ReceiptRerunRuntime {
            repo_root: repo.path().to_owned(),
            times: std::sync::Mutex::new([0, 100].into()),
            outcome: nano_verify::ExecutionGateOutcome::Green {
                verdicts: Vec::new(),
            },
            add_ok: true,
            identity_ok: true,
            cleanup_ok: true,
            gate_calls: Default::default(),
            cleanup_calls: Default::default(),
        };
        assert_eq!(
            super::rerun_ready_receipt(repo.path(), &receipt, &registry, &runtime).await,
            nano_verify::VerifyVerdict::Unverifiable
        );
        assert_eq!(
            runtime.gate_calls.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            runtime
                .cleanup_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[test]
    fn receipt_rerun_cleanup_removes_real_detached_worktree_and_registration() {
        let repo = fixture_repo();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(repo.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["config", "user.name", "WP3 Test"]);
        git(&["config", "user.email", "wp3@example.invalid"]);
        git(&["add", "."]);
        git(&["commit", "-m", "fixture"]);
        let head = git(&["rev-parse", "HEAD"]);
        let worktree = repo.path().with_extension("detached");

        super::git_success_bounded(
            repo.path(),
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.to_string_lossy(),
                &head,
            ],
            10_000,
        )
        .unwrap();
        super::verify_receipt_worktree(&worktree, &head, 10_000).unwrap();
        super::cleanup_receipt_worktree(repo.path(), &worktree, 10_000).unwrap();
        assert!(!worktree.exists());
        let listed = git(&["worktree", "list", "--porcelain"]);
        assert!(
            !listed
                .replace('\\', "/")
                .contains(&worktree.to_string_lossy().replace('\\', "/"))
        );
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn events_emit_only_closed_fields_with_monotonic_sequence() {
        let mut bytes = Vec::new();
        let mut events = VerifyEvents::new(&mut bytes, "wayland-nano-verify-test".into());
        events.verify_started("CLI-01", "fixture");
        events.check_verdict(&nano_verify::CheckVerdict {
            id: "CLI-01".into(),
            category: nano_verify::FailCategory::Security,
            passed: false,
        });
        events.climb_update(&nano_verify::LogEntry {
            phase: nano_verify::Phase::Surgical,
            score: [1, 2],
            accepted: true,
            code: nano_verify::LogCode::Accepted,
        });
        events.apply_started("fixture");
        events.apply_verified("fixture", 2);
        let receipt = nano_verify::Receipt {
            schema: 1,
            requirement: "CLI-01".into(),
            test: "fixture".into(),
            gate_id: "fixture".into(),
            gate_closure_digest: "a".repeat(64),
            failing_run: nano_verify::FailingRun {
                exit_code: 1,
                log_digest: "b".repeat(64),
                observed_at_commit: "c".repeat(40),
            },
            fix_commit: "d".repeat(40),
            minted_at: "2026-08-21T00:00:00Z".into(),
            minted_by: "wayland-nano".into(),
        };
        events.receipt_minted(&receipt, Path::new("F:/secret/receipt.json"));
        events.verify_completed(&nano_verify::TerminalState::Verified, 0);
        events.error("git_failed");
        drop(events);

        let frames: Vec<serde_json::Value> = String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(frames.len(), 8);
        for (seq, frame) in frames.iter().enumerate() {
            assert_eq!(frame["v"], 1);
            assert_eq!(frame["session_id"], "wayland-nano-verify-test");
            assert_eq!(frame["seq"], seq);
        }
        assert_eq!(
            sorted_keys(&frames[1]),
            vec!["category", "id", "passed", "seq", "session_id", "type", "v"]
        );
        assert_eq!(
            sorted_keys(&frames[2]),
            vec![
                "accepted",
                "code",
                "phase",
                "score",
                "seq",
                "session_id",
                "type",
                "v"
            ]
        );
        assert_eq!(
            sorted_keys(&frames[5]),
            vec!["gate_id", "requirement", "seq", "session_id", "type", "v"]
        );
        let output = serde_json::to_string(&frames).unwrap();
        for secret in [
            "provider",
            "prompt",
            "F:/secret",
            "diff",
            "argv",
            "2026-08-21",
        ] {
            assert!(!output.contains(secret), "leaked {secret}");
        }
    }

    fn sorted_keys(value: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    struct ScriptedRuntime {
        times: std::sync::Mutex<std::collections::VecDeque<u64>>,
        outcome: nano_verify::ExecutionGateOutcome,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl super::VerifyRuntime for ScriptedRuntime {
        fn now_millis(&self) -> u64 {
            self.times.lock().unwrap().pop_front().unwrap_or(u64::MAX)
        }

        fn canonical_repo_root(&self, workspace: &Path) -> Result<PathBuf, ()> {
            workspace.canonicalize().map_err(|_| ())
        }

        fn temp_preflight(&self) -> Result<(), ()> {
            portable_test_temp_root().is_dir().then_some(()).ok_or(())
        }

        async fn run_gate(
            &self,
            _: &nano_verify::GateInvocation,
            _: &Path,
            _: &[(String, nano_verify::FailCategory)],
        ) -> nano_verify::ExecutionGateOutcome {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.outcome.clone()
        }
    }

    fn fixture_repo() -> tempfile::TempDir {
        let root = portable_tempdir("wp3-02-");
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root.path())
            .status()
            .unwrap();
        let gate = root.path().join("gates/demo");
        std::fs::create_dir_all(&gate).unwrap();
        std::fs::create_dir_all(root.path().join("artifact")).unwrap();
        std::fs::write(gate.join("gate.cmd"), "").unwrap();
        std::fs::write(
            gate.join("card.md"),
            "---\nchecks:\n  - { id: CLI-01, category: structure, desc: demo }\n---\n",
        )
        .unwrap();
        let closure = nano_verify::GateClosure {
            argv: vec!["gates/demo/gate.cmd".into()],
            env: Default::default(),
            cwd_policy: nano_verify::CwdPolicy::RepoRoot,
            wrapped_tools: Vec::new(),
        };
        let registry = serde_json::json!({"schema":1,"gates":{"demo":{"card":"gates/demo/card.md","script":"gates/demo/gate.cmd","closure":closure,"closure_digest":nano_verify::closure_digest(&closure).unwrap(),"run_artifact":"artifact"}},"requirements":{"CLI-01":"demo"}});
        std::fs::write(
            root.path().join("gates/registry.json"),
            serde_json::to_vec(&registry).unwrap(),
        )
        .unwrap();
        root
    }

    #[tokio::test]
    async fn run_only_maps_green_red_and_expiry_without_extra_spawn() {
        let repo = fixture_repo();
        let params = VerifyParams {
            mode: VerifyMode::RunOnly {
                gate: "demo".into(),
                deadline_ms: 10,
                json: false,
            },
        };
        let green = ScriptedRuntime {
            times: std::sync::Mutex::new([10, 11, 12, 13, 14].into()),
            outcome: nano_verify::ExecutionGateOutcome::Green {
                verdicts: vec![nano_verify::CheckVerdict {
                    id: "CLI-01".into(),
                    category: nano_verify::FailCategory::Structure,
                    passed: true,
                }],
            },
            calls: Default::default(),
        };
        assert_eq!(super::run_with(repo.path(), &params, &green).await, 0);
        assert_eq!(green.calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let expired = ScriptedRuntime {
            times: std::sync::Mutex::new([10, 11, 20].into()),
            outcome: nano_verify::ExecutionGateOutcome::Green {
                verdicts: Vec::new(),
            },
            calls: Default::default(),
        };
        assert_eq!(super::run_with(repo.path(), &params, &expired).await, 3);
        assert_eq!(expired.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn deadline_overflow_is_usage_before_spawn() {
        let repo = fixture_repo();
        let params = VerifyParams {
            mode: VerifyMode::RunOnly {
                gate: "demo".into(),
                deadline_ms: 10,
                json: false,
            },
        };
        let runtime = ScriptedRuntime {
            times: std::sync::Mutex::new([u64::MAX - 5].into()),
            outcome: nano_verify::ExecutionGateOutcome::Red {
                verdicts: Vec::new(),
            },
            calls: Default::default(),
        };
        assert_eq!(super::run_with(repo.path(), &params, &runtime).await, 2);
        assert_eq!(runtime.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn receipt_entry_classification_unreadable_is_usage() {
        let params = VerifyParams {
            mode: VerifyMode::CheckReceipt {
                path: Path::new("missing.receipt.json").into(),
                json: false,
            },
        };
        let runtime = ScriptedRuntime {
            times: Default::default(),
            outcome: nano_verify::ExecutionGateOutcome::Red {
                verdicts: Vec::new(),
            },
            calls: Default::default(),
        };
        assert_eq!(
            super::run_with(Path::new("F:/"), &params, &runtime).await,
            2
        );
    }

    #[test]
    fn landed_contract_import_probe() {
        use nano_verify::{
            ArtifactWorkspace, BaselineGateEvidence, BaselineGateExecution, CandidateArtifact,
            CheckVerdict, ClimbConfig, Effects, ExpectedChangeManifest, FailingRun, GateClosure,
            GateEvidence, GateExecution, GateInvocation, GateRegistry, LogEntry, Receipt,
            RunDeadline, TerminalState, VerifyVerdict, canonical_receipt,
            create_artifact_workspace, derive_expected_changes, gate_for_requirement,
            load_registry, mint_receipt, parse_candidate_diff, preflight_receipt, read_receipt,
            run_climb, run_gate_baseline_execution, run_gate_execution, write_receipt,
        };
        let imported_types = [
            std::any::type_name::<Receipt>(),
            std::any::type_name::<CheckVerdict>(),
            std::any::type_name::<VerifyVerdict>(),
            std::any::type_name::<TerminalState>(),
            std::any::type_name::<LogEntry>(),
            std::any::type_name::<RunDeadline>(),
            std::any::type_name::<ClimbConfig>(),
            std::any::type_name::<GateClosure>(),
            std::any::type_name::<GateRegistry>(),
            std::any::type_name::<GateInvocation>(),
            std::any::type_name::<GateExecution>(),
            std::any::type_name::<GateEvidence>(),
            std::any::type_name::<BaselineGateExecution>(),
            std::any::type_name::<BaselineGateEvidence>(),
            std::any::type_name::<CandidateArtifact>(),
            std::any::type_name::<ArtifactWorkspace>(),
            std::any::type_name::<ExpectedChangeManifest>(),
            std::any::type_name::<FailingRun>(),
        ];
        assert!(
            imported_types
                .iter()
                .all(|name| name.starts_with("nano_verify::"))
        );
        let _ = (
            canonical_receipt,
            create_artifact_workspace,
            derive_expected_changes,
            gate_for_requirement,
            load_registry,
            mint_receipt,
            parse_candidate_diff,
            preflight_receipt,
            read_receipt,
            run_climb::<ProbeEffects>,
            run_gate_baseline_execution,
            run_gate_execution,
            write_receipt,
        );

        struct ProbeEffects;
        impl Effects for ProbeEffects {
            async fn generate(&self, _: &str, _: &str) -> Result<String, nano_verify::VerifyError> {
                unreachable!()
            }
            fn emit_event(&self, _: nano_verify::EngineEvent) {}
            fn now_millis(&self) -> u64 {
                0
            }
            fn cancellation_requested(&self) -> bool {
                false
            }
        }
    }

    #[test]
    fn bootstrap_is_byte_exact_and_rejects_requested_work() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bytes = std::fs::read(root.join("gates/registry.json")).unwrap();
        assert_eq!(bytes, EMPTY_REGISTRY_BOOTSTRAP);
        assert_eq!(load_requested_registry(&root, None, None).unwrap(), None);
        assert_eq!(load_requested_registry(&root, Some("demo"), None), Err(2));
        assert_eq!(load_requested_registry(&root, None, Some("CLI-01")), Err(2));
    }

    #[test]
    fn bootstrap_nonempty_registry_uses_canonical_loader() {
        let temp = tempfile::tempdir().unwrap();
        let gate_dir = temp.path().join("gates/demo");
        std::fs::create_dir_all(&gate_dir).unwrap();
        std::fs::create_dir_all(temp.path().join("artifact")).unwrap();
        std::fs::write(gate_dir.join("gate.cmd"), "").unwrap();
        std::fs::write(
            gate_dir.join("card.md"),
            "---\nchecks:\n  - { id: CLI-01, category: structure, desc: demo }\n---\n",
        )
        .unwrap();
        let closure = nano_verify::GateClosure {
            argv: vec!["gates/demo/gate.cmd".to_owned()],
            env: Default::default(),
            cwd_policy: nano_verify::CwdPolicy::RepoRoot,
            wrapped_tools: Vec::new(),
        };
        let registry = serde_json::json!({
            "schema": 1,
            "gates": {"demo": {
                "card": "gates/demo/card.md",
                "script": "gates/demo/gate.cmd",
                "closure": closure,
                "closure_digest": nano_verify::closure_digest(&closure).unwrap(),
                "run_artifact": "artifact"
            }},
            "requirements": {"CLI-01": "demo"}
        });
        std::fs::write(
            temp.path().join("gates/registry.json"),
            serde_json::to_vec(&registry).unwrap(),
        )
        .unwrap();
        let loaded = load_requested_registry(temp.path(), Some("demo"), None)
            .unwrap()
            .unwrap();
        assert!(loaded.gates.contains_key("demo"));

        let path = temp.path().join("gates/registry.json");
        let drifted = std::fs::read_to_string(&path).unwrap().replace(
            &nano_verify::closure_digest(&closure).unwrap(),
            &"0".repeat(64),
        );
        std::fs::write(path, drifted).unwrap();
        assert_eq!(
            load_requested_registry(temp.path(), Some("demo"), None),
            Err(2)
        );
    }

    #[test]
    fn parse_accepts_all_three_closed_modes_and_defaults() {
        let mint = parse_args(&args(&[
            "--requirement",
            "CLI-01",
            "--cheap-model",
            "cheap",
            "--escalation-model",
            "strong",
        ]))
        .unwrap();
        assert!(matches!(
            mint.mode,
            VerifyMode::Mint {
                budget: None,
                deadline_ms: 600_000,
                json: false,
                ..
            }
        ));

        let check = parse_args(&args(&["--verify-receipt", "receipt.json", "--json"])).unwrap();
        assert!(matches!(
            check.mode,
            VerifyMode::CheckReceipt { json: true, .. }
        ));

        let run = parse_args(&args(&[
            "--gate",
            "demo",
            "--run-only",
            "--deadline-ms",
            "42",
            "--json",
        ]))
        .unwrap();
        assert!(matches!(
            run.mode,
            VerifyMode::RunOnly {
                deadline_ms: 42,
                json: true,
                ..
            }
        ));
    }

    #[test]
    fn parse_preserves_mint_values_and_escalation_order() {
        let parsed = parse_args(&args(&[
            "--requirement",
            "CLI-01",
            "--gate",
            "demo",
            "--task",
            "repair",
            "--budget",
            "7",
            "--cheap-model",
            "cheap",
            "--escalation-model",
            "one",
            "--escalation-model",
            "two",
            "--escalation-model",
            "three",
            "--escalation-model",
            "four",
            "--deadline-ms",
            "3600000",
            "--receipt-out",
            "out.json",
            "--json",
        ]))
        .unwrap();
        let VerifyMode::Mint {
            requirement,
            gate,
            task,
            budget,
            cheap_model,
            escalation_models,
            deadline_ms,
            receipt_out,
            json,
        } = parsed.mode
        else {
            panic!("mint")
        };
        assert_eq!(requirement, "CLI-01");
        assert_eq!(gate.as_deref(), Some("demo"));
        assert_eq!(task.as_deref(), Some("repair"));
        assert_eq!(budget, Some(7));
        assert_eq!(cheap_model, "cheap");
        assert_eq!(escalation_models, ["one", "two", "three", "four"]);
        assert_eq!(deadline_ms, 3_600_000);
        assert_eq!(receipt_out.unwrap().to_string_lossy(), "out.json");
        assert!(json);
    }

    #[test]
    fn parse_rejects_missing_values_unknowns_and_duplicate_singletons() {
        for bad in [
            vec![],
            vec!["--unknown"],
            vec!["positional"],
            vec!["--requirement"],
            vec!["--requirement", ""],
            vec!["--requirement", "R", "--requirement", "R2"],
            vec!["--gate", "g", "--gate", "h", "--run-only"],
            vec!["--task", "a", "--task", "b"],
            vec!["--budget", "1", "--budget", "2"],
            vec!["--cheap-model", "a", "--cheap-model", "b"],
            vec!["--receipt-out", "a", "--receipt-out", "b"],
            vec!["--verify-receipt", "a", "--verify-receipt", "b"],
            vec!["--run-only", "--run-only"],
            vec!["--json", "--json"],
        ] {
            assert_eq!(parse_args(&args(&bad)), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn parse_rejects_invalid_budget_deadline_and_ladder_values() {
        for bad in [
            vec![
                "--requirement",
                "R",
                "--budget",
                "0",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--budget",
                "4294967296",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--deadline-ms",
                "0",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--deadline-ms",
                "3600001",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--deadline-ms",
                "18446744073709551616",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--deadline-ms",
                "1",
                "--deadline-ms",
                "2",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--cheap-model",
                "",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--cheap-model",
                "c",
                "--escalation-model",
                "",
            ],
            vec![
                "--requirement",
                "R",
                "--cheap-model",
                "c",
                "--escalation-model",
                "e",
                "--escalation-model",
                "e",
            ],
            vec![
                "--requirement",
                "R",
                "--cheap-model",
                "c",
                "--escalation-model",
                "1",
                "--escalation-model",
                "2",
                "--escalation-model",
                "3",
                "--escalation-model",
                "4",
                "--escalation-model",
                "5",
            ],
        ] {
            assert_eq!(parse_args(&args(&bad)), Err(2), "{bad:?}");
        }
    }

    #[test]
    fn parse_rejects_cross_mode_flags_and_incomplete_modes() {
        for bad in [
            vec!["--requirement", "R", "--cheap-model", "c"],
            vec!["--requirement", "R", "--escalation-model", "e"],
            vec!["--cheap-model", "c", "--escalation-model", "e"],
            vec!["--verify-receipt", "r", "--requirement", "R"],
            vec!["--verify-receipt", "r", "--gate", "g"],
            vec!["--verify-receipt", "r", "--task", "t"],
            vec!["--verify-receipt", "r", "--budget", "1"],
            vec!["--verify-receipt", "r", "--cheap-model", "c"],
            vec!["--verify-receipt", "r", "--escalation-model", "e"],
            vec!["--verify-receipt", "r", "--receipt-out", "o"],
            vec!["--verify-receipt", "r", "--deadline-ms", "1"],
            vec!["--verify-receipt", "r", "--run-only"],
            vec!["--run-only"],
            vec!["--gate", "g"],
            vec!["--gate", "g", "--run-only", "--requirement", "R"],
            vec!["--gate", "g", "--run-only", "--task", "t"],
            vec!["--gate", "g", "--run-only", "--budget", "1"],
            vec!["--gate", "g", "--run-only", "--cheap-model", "c"],
            vec!["--gate", "g", "--run-only", "--escalation-model", "e"],
            vec!["--gate", "g", "--run-only", "--receipt-out", "o"],
        ] {
            assert_eq!(parse_args(&args(&bad)), Err(2), "{bad:?}");
        }
    }

    mod baseline {
        fn verdict(id: &str, passed: bool) -> nano_verify::CheckVerdict {
            nano_verify::CheckVerdict {
                id: id.into(),
                category: nano_verify::FailCategory::Structure,
                passed,
            }
        }

        #[test]
        fn only_complete_nonzero_red_with_lower_hex_digest_is_eligible() {
            let inventory = vec![("A".into(), nano_verify::FailCategory::Structure)];
            let execution = nano_verify::BaselineGateExecution {
                outcome: nano_verify::ExecutionGateOutcome::Red {
                    verdicts: vec![verdict("A", false)],
                },
                evidence: nano_verify::BaselineGateEvidence {
                    exit_code: Some(7),
                    log_digest: Some("a".repeat(64)),
                },
            };
            assert!(super::super::eligible_baseline(&execution, &inventory));

            for execution in [
                nano_verify::BaselineGateExecution {
                    outcome: nano_verify::ExecutionGateOutcome::Green {
                        verdicts: vec![verdict("A", true)],
                    },
                    evidence: execution.evidence.clone(),
                },
                nano_verify::BaselineGateExecution {
                    outcome: nano_verify::ExecutionGateOutcome::Red { verdicts: vec![] },
                    evidence: execution.evidence.clone(),
                },
                nano_verify::BaselineGateExecution {
                    outcome: execution.outcome.clone(),
                    evidence: nano_verify::BaselineGateEvidence {
                        exit_code: Some(0),
                        log_digest: execution.evidence.log_digest.clone(),
                    },
                },
                nano_verify::BaselineGateExecution {
                    outcome: execution.outcome.clone(),
                    evidence: nano_verify::BaselineGateEvidence {
                        exit_code: Some(7),
                        log_digest: None,
                    },
                },
            ] {
                assert!(!super::super::eligible_baseline(&execution, &inventory));
            }
        }
    }

    mod materializer_confinement {
        use std::path::PathBuf;

        fn authority(target: &str, target_is_dir: bool) -> super::super::MaterializerAuthority {
            super::super::MaterializerAuthority {
                run_artifact: target.into(),
                run_artifact_is_dir: target_is_dir,
                protected: vec![
                    "gates/registry.json".into(),
                    "gates/demo/card.md".into(),
                    "gates/demo/gate.ps1".into(),
                    "receipts".into(),
                    ".verify-control".into(),
                ],
            }
        }

        #[test]
        fn component_confinement_and_symmetric_protection_are_closed() {
            let directory = authority("artifact", true);
            assert!(super::super::candidate_path_allowed(
                "artifact/fix.txt",
                &directory
            ));
            for denied in [
                "artifact",
                "artifact2/fix.txt",
                "artifact/../gates/registry.json",
                "artifact\\fix.txt",
                ".git/config",
                "artifact/.git/config",
                "gates",
                "gates/registry.json.bak",
            ] {
                assert!(
                    !super::super::candidate_path_allowed(denied, &directory),
                    "{denied}"
                );
            }

            let file = authority("artifact.txt", false);
            assert!(super::super::candidate_path_allowed("artifact.txt", &file));
            assert!(!super::super::candidate_path_allowed(
                "artifact.txt/child",
                &file
            ));

            let protected_target = authority("gates", true);
            assert!(!super::super::candidate_path_allowed(
                "gates/new.txt",
                &protected_target
            ));
        }

        #[test]
        fn canonical_changed_path_digest_is_order_independent() {
            let first = vec!["z.txt".into(), "artifact/a.txt".into()];
            let second = vec!["artifact/a.txt".into(), "z.txt".into()];
            assert_eq!(
                super::super::canonical_changed_paths(&first).unwrap(),
                super::super::canonical_changed_paths(&second).unwrap()
            );
            assert!(super::super::canonical_changed_paths(&["a".into(), "a".into()]).is_err());
        }

        #[test]
        fn authority_paths_are_repo_relative_components() {
            for denied in ["", ".", "..", "/abs", "C:/drive", "//server/share", "a//b"] {
                let authority = authority(denied, true);
                assert!(!super::super::candidate_path_allowed("a/file", &authority));
            }
            let _: PathBuf = authority("artifact", true).run_artifact.into();
        }

        #[test]
        fn runtime_authority_protects_repo_local_outputs_and_ignores_external_paths() {
            let repo = super::portable_tempdir("wp3-authority-repo-");
            std::fs::create_dir_all(repo.path().join("gates/demo")).unwrap();
            std::fs::create_dir_all(repo.path().join("artifact")).unwrap();
            std::fs::create_dir_all(repo.path().join("outputs")).unwrap();
            std::fs::create_dir_all(repo.path().join("temp-control")).unwrap();
            std::fs::create_dir_all(repo.path().join("local-home")).unwrap();
            for path in [
                "gates/registry.json",
                "gates/demo/card.md",
                "gates/demo/gate.ps1",
            ] {
                std::fs::write(repo.path().join(path), b"x").unwrap();
            }
            let entry = nano_verify::GateRegistryEntry {
                card: "gates/demo/card.md".into(),
                script: "gates/demo/gate.ps1".into(),
                closure: nano_verify::GateClosure {
                    argv: vec![],
                    env: Default::default(),
                    cwd_policy: nano_verify::CwdPolicy::RepoRoot,
                    wrapped_tools: vec![],
                },
                closure_digest: "a".repeat(64),
                run_artifact: "artifact".into(),
            };
            let outside = super::portable_tempdir("wp3-authority-outside-");
            let output = repo.path().join("outputs/receipt.json");
            let authority = super::super::materializer_authority(
                repo.path(),
                &entry,
                &repo.path().join("local-home"),
                Some(&output),
                &[outside.path().to_owned()],
                &repo.path().join("temp-control"),
            )
            .unwrap();
            for denied in [
                "gates/registry.json",
                "gates/demo/card.md",
                "gates/demo/gate.ps1",
                "local-home/receipts",
                "outputs/receipt.json",
                "temp-control",
            ] {
                assert!(
                    !super::super::candidate_path_allowed(denied, &authority),
                    "{denied}"
                );
            }
            assert!(super::super::candidate_path_allowed(
                "artifact/ok.txt",
                &authority
            ));
            assert!(
                authority
                    .protected
                    .iter()
                    .all(|path| !path.contains(outside.path().to_string_lossy().as_ref()))
            );
        }
    }

    mod materializer_transaction {
        use std::path::Path;

        struct AdvancingClock(std::sync::atomic::AtomicU64);

        impl super::super::VerifyRuntime for AdvancingClock {
            fn now_millis(&self) -> u64 {
                self.0.fetch_add(1_000, std::sync::atomic::Ordering::SeqCst)
            }

            async fn run_gate(
                &self,
                _: &nano_verify::GateInvocation,
                _: &Path,
                _: &[(String, nano_verify::FailCategory)],
            ) -> nano_verify::ExecutionGateOutcome {
                unreachable!()
            }
        }

        fn git(root: &Path, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().into()
        }

        fn repo() -> tempfile::TempDir {
            let dir = super::portable_tempdir("wp3-materializer-");
            git(dir.path(), &["init", "-q"]);
            git(
                dir.path(),
                &["config", "user.email", "verify@example.invalid"],
            );
            git(dir.path(), &["config", "user.name", "Wayland Nano Verify"]);
            git(dir.path(), &["config", "core.autocrlf", "false"]);
            std::fs::create_dir(dir.path().join("artifact")).unwrap();
            std::fs::write(dir.path().join("artifact/base.txt"), b"old\n").unwrap();
            git(dir.path(), &["add", "artifact/base.txt"]);
            git(dir.path(), &["commit", "-qm", "base"]);
            dir
        }

        #[test]
        fn coherent_apply_commit_is_bound_to_sealed_manifest() {
            let repo = repo();
            let start = git(repo.path(), &["rev-parse", "HEAD"]);
            let bytes = b"diff --git a/artifact/base.txt b/artifact/base.txt\n--- a/artifact/base.txt\n+++ b/artifact/base.txt\n@@ -1 +1 @@\n-old\n+new\n";
            let authority = super::super::MaterializerAuthority {
                run_artifact: "artifact".into(),
                run_artifact_is_dir: true,
                protected: vec!["gates/registry.json".into()],
            };
            let parsed = nano_verify::parse_candidate_diff(bytes).unwrap();
            nano_verify::derive_expected_changes(&parsed, &repo.path().canonicalize().unwrap())
                .unwrap();
            let runtime = super::super::ProductionRuntime::new();
            let deadline = nano_verify::RunDeadline {
                monotonic_millis: 60_000,
            };
            let committed = super::super::materialize_candidate(
                repo.path(),
                &start,
                bytes,
                &authority,
                "wayland-nano verify fix",
                &runtime,
                deadline,
                |_| {},
            )
            .unwrap();
            assert_eq!(git(repo.path(), &["rev-parse", "HEAD^1"]), start);
            assert_eq!(
                git(repo.path(), &["rev-parse", "HEAD"]),
                committed.fix_commit
            );
            assert_eq!(
                std::fs::read(repo.path().join("artifact/base.txt")).unwrap(),
                b"new\n"
            );
            assert!(
                git(
                    repo.path(),
                    &["status", "--porcelain=v1", "--untracked-files=all"]
                )
                .is_empty()
            );
        }

        #[test]
        fn precommit_failure_restores_exact_start() {
            let repo = repo();
            let start = git(repo.path(), &["rev-parse", "HEAD"]);
            let bytes = b"diff --git a/artifact/base.txt b/artifact/base.txt\n--- a/artifact/base.txt\n+++ b/artifact/base.txt\n@@ -1 +1 @@\n-not-the-preimage\n+new\n";
            let authority = super::super::MaterializerAuthority {
                run_artifact: "artifact".into(),
                run_artifact_is_dir: true,
                protected: vec![],
            };
            let runtime = super::super::ProductionRuntime::new();
            let deadline = nano_verify::RunDeadline {
                monotonic_millis: 60_000,
            };
            assert!(
                super::super::materialize_candidate(
                    repo.path(),
                    &start,
                    bytes,
                    &authority,
                    "fix",
                    &runtime,
                    deadline,
                    |_| {},
                )
                .is_err()
            );
            assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), start);
            assert_eq!(
                std::fs::read(repo.path().join("artifact/base.txt")).unwrap(),
                b"old\n"
            );
            assert!(
                git(
                    repo.path(),
                    &["status", "--porcelain=v1", "--untracked-files=all"]
                )
                .is_empty()
            );
        }

        #[test]
        fn shared_deadline_expiry_between_git_operations_rolls_back() {
            let repo = repo();
            let start = git(repo.path(), &["rev-parse", "HEAD"]);
            let bytes = b"diff --git a/artifact/base.txt b/artifact/base.txt\n--- a/artifact/base.txt\n+++ b/artifact/base.txt\n@@ -1 +1 @@\n-old\n+new\n";
            let authority = super::super::MaterializerAuthority {
                run_artifact: "artifact".into(),
                run_artifact_is_dir: true,
                protected: vec![],
            };
            let clock = AdvancingClock(0.into());
            let mut apply_started = false;
            let mut apply_verified = false;
            assert!(
                super::super::materialize_candidate(
                    repo.path(),
                    &start,
                    bytes,
                    &authority,
                    "fix",
                    &clock,
                    nano_verify::RunDeadline {
                        monotonic_millis: 7_000
                    },
                    |event| match event {
                        super::super::MaterializerEvent::ApplyStarted => apply_started = true,
                        super::super::MaterializerEvent::ApplyVerified(_) => apply_verified = true,
                    },
                )
                .is_err()
            );
            assert!(
                apply_started,
                "deadline must advance across the bounded stdin Git seam"
            );
            assert!(
                !apply_verified,
                "expiry must prevent staged verification/commit"
            );
            assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), start);
            assert_eq!(
                std::fs::read(repo.path().join("artifact/base.txt")).unwrap(),
                b"old\n"
            );
            assert!(
                git(
                    repo.path(),
                    &["status", "--porcelain=v1", "--untracked-files=all"]
                )
                .is_empty()
            );
        }

        #[test]
        fn advancing_clock_blocks_climb_read_and_resolve_to_gate_boundaries() {
            let clock = AdvancingClock(0.into());
            let deadline = nano_verify::RunDeadline {
                monotonic_millis: 5,
            };
            let first_called = std::cell::Cell::new(false);
            let second_called = std::cell::Cell::new(false);
            assert!(
                super::super::scheduled_before_deadline(&clock, deadline, || {
                    first_called.set(true);
                    Ok(())
                })
                .is_ok()
            );
            assert!(
                super::super::scheduled_before_deadline(&clock, deadline, || {
                    second_called.set(true);
                    Ok(())
                })
                .is_err()
            );
            assert!(first_called.get());
            assert!(!second_called.get());
        }
    }

    mod mint_flow {
        #[test]
        fn climb_config_preserves_caller_model_order_deadline_and_budget() {
            let cfg = super::super::climb_config(
                "cheap",
                &["first".into(), "second".into()],
                Some(9),
                nano_verify::RunDeadline {
                    monotonic_millis: 42,
                },
            );
            assert_eq!(cfg.cheap, ["cheap"]);
            assert_eq!(cfg.ladder, ["first", "second"]);
            assert_eq!(cfg.budget, 9);
            assert_eq!(cfg.deadline.monotonic_millis, 42);
        }

        #[test]
        fn production_request_is_one_user_message_without_tools_or_streaming() {
            let request = super::super::generation_request("wire-model", "repair this");
            assert_eq!(request.model, "wire-model");
            assert_eq!(
                request.messages,
                [nano_model::types::Message::user("repair this")]
            );
            assert!(request.tools.is_empty());
            assert!(!request.stream);
            assert!(request.system.is_none());
        }

        #[test]
        fn generation_collects_text_deltas_only() {
            use nano_model::types::{ModelEvent, Usage};
            let events = vec![
                ModelEvent::ReasoningDelta("secret reasoning".into()),
                ModelEvent::TextDelta("one".into()),
                ModelEvent::Usage(Usage::default()),
                ModelEvent::TextDelta("two".into()),
                ModelEvent::Done {
                    stop_reason: "done".into(),
                },
            ];
            assert_eq!(super::super::collect_text(&events), "onetwo");
        }

        #[test]
        fn terminal_mapping_is_closed() {
            use nano_verify::TerminalState::*;
            assert_eq!(super::super::mint_terminal_exit(&Verified), 0);
            assert_eq!(super::super::mint_terminal_exit(&Cancelled), 1);
            assert_eq!(super::super::mint_terminal_exit(&CrashedRecovered), 1);
            assert_eq!(super::super::mint_terminal_exit(&TimedOut), 3);
            assert_eq!(super::super::mint_terminal_exit(&Blocked("x".into())), 3);
        }
    }
}

struct RuntimeEffects<'a, R> {
    runtime: &'a R,
}

impl<R: VerifyRuntime> nano_verify::Effects for RuntimeEffects<'_, R> {
    async fn generate(
        &self,
        model: &str,
        prompt: &str,
    ) -> Result<String, nano_verify::VerifyError> {
        self.runtime
            .generate(model, prompt)
            .await
            .map_err(|()| nano_verify::VerifyError::Generate("generation_failed".into()))
    }

    fn emit_event(&self, _event: nano_verify::EngineEvent) {}

    fn now_millis(&self) -> u64 {
        self.runtime.now_millis()
    }

    fn cancellation_requested(&self) -> bool {
        false
    }
}

fn generation_request(model: &str, prompt: &str) -> nano_model::types::ModelRequest {
    nano_model::types::ModelRequest {
        model: model.to_owned(),
        messages: vec![nano_model::types::Message::user(prompt)],
        ..Default::default()
    }
}

fn collect_text(events: &[nano_model::types::ModelEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            nano_model::types::ModelEvent::TextDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

async fn production_generate(model: &str, prompt: &str) -> Result<String, ()> {
    use nano_agent::turn::ModelDriver;
    use nano_model::provider_catalog::WireKind;

    let (router, diagnostic) = crate::provider_router::ProviderRouter::from_env();
    if diagnostic.is_some() {
        return Err(());
    }
    let env_reader = |name: &str| std::env::var(name).ok();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?
        .as_secs();
    let binding = router
        .resolve_binding(model, &env_reader, now)
        .map_err(|_| ())?;
    let policy = nano_egress::policy::EgressPolicy::new().allow_url(&binding.base_url);
    let egress = nano_egress::client::EgressClient::new(policy);
    let driver = match binding.wire {
        WireKind::OpenAiCompletions => {
            let client = nano_model::flux_completions::FluxCompletionsClient::new(egress)
                .with_base_url(binding.base_url)
                .with_api_path(binding.api_path);
            nano_agent::wiring::ProviderDriver::openai(
                client,
                binding.credential.secret().to_owned(),
            )
        }
        WireKind::AnthropicMessages => {
            let client = nano_model::anthropic_messages::AnthropicMessagesClient::new(egress)
                .with_base_url(binding.base_url)
                .with_api_path(binding.api_path);
            nano_agent::wiring::ProviderDriver::anthropic(
                client,
                binding.credential.secret().to_owned(),
            )
        }
    };
    let response = driver
        .complete(&generation_request(&binding.model, prompt))
        .await
        .map_err(|_| ())?;
    Ok(collect_text(&response.events))
}

fn climb_config(
    cheap_model: &str,
    escalation_models: &[String],
    budget: Option<u32>,
    deadline: nano_verify::RunDeadline,
) -> nano_verify::ClimbConfig {
    nano_verify::ClimbConfig {
        cheap: vec![cheap_model.to_owned()],
        ladder: escalation_models.to_vec(),
        budget: budget.unwrap_or(8),
        seed_n: 1,
        deadline,
    }
}

fn eligible_baseline(
    execution: &nano_verify::BaselineGateExecution,
    inventory: &[(String, nano_verify::FailCategory)],
) -> bool {
    let nano_verify::ExecutionGateOutcome::Red { verdicts } = &execution.outcome else {
        return false;
    };
    let complete = verdicts.len() == inventory.len()
        && inventory.iter().all(|(id, category)| {
            verdicts
                .iter()
                .any(|verdict| verdict.id == *id && verdict.category == *category)
        });
    complete
        && execution.evidence.exit_code.is_some_and(|code| code != 0)
        && execution
            .evidence
            .log_digest
            .as_deref()
            .is_some_and(|digest| lower_hex(digest, 64))
}

fn mint_terminal_exit(terminal: &nano_verify::TerminalState) -> i32 {
    match terminal {
        nano_verify::TerminalState::Verified => 0,
        nano_verify::TerminalState::Cancelled | nano_verify::TerminalState::CrashedRecovered => 1,
        _ => 3,
    }
}

fn finish_mint<W: Write>(
    events: &mut VerifyEvents<W>,
    terminal: nano_verify::TerminalState,
    exit_code: i32,
) -> i32 {
    events.verify_completed(&terminal, exit_code);
    exit_code
}

fn scheduled_before_deadline<R: VerifyRuntime, T>(
    runtime: &R,
    deadline: nano_verify::RunDeadline,
    operation: impl FnOnce() -> Result<T, ()>,
) -> Result<T, ()> {
    remaining(runtime, deadline).ok_or(())?;
    operation()
}

#[derive(Debug, Clone)]
struct MaterializerAuthority {
    run_artifact: String,
    run_artifact_is_dir: bool,
    protected: Vec<String>,
}

fn resolve_repo_local(root: &Path, path: &Path) -> Result<Option<String>, ()> {
    let candidate = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let mut ancestor = candidate.clone();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(ancestor.file_name().ok_or(())?.to_owned());
        ancestor = ancestor.parent().ok_or(())?.to_owned();
    }
    let resolved_parent = ancestor.canonicalize().map_err(|_| ())?;
    let root = root.canonicalize().map_err(|_| ())?;
    let resolved = suffix
        .into_iter()
        .rev()
        .fold(resolved_parent, |path, component| path.join(component));
    let Ok(relative) = resolved.strip_prefix(&root) else {
        return Ok(None);
    };
    let relative = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(value) => value.to_str().ok_or(()),
            _ => Err(()),
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("/");
    if valid_repo_relative(&relative) {
        Ok(Some(relative))
    } else {
        Err(())
    }
}

fn materializer_authority(
    repo_root: &Path,
    entry: &nano_verify::GateRegistryEntry,
    home: &Path,
    receipt_out: Option<&Path>,
    detached_or_control: &[PathBuf],
    temp_control_parent: &Path,
) -> Result<MaterializerAuthority, ()> {
    let run_artifact = resolve_repo_local(repo_root, Path::new(&entry.run_artifact))?.ok_or(())?;
    let mut protected = Vec::new();
    for path in [
        PathBuf::from("gates/registry.json"),
        PathBuf::from(&entry.card),
        PathBuf::from(&entry.script),
    ]
    .into_iter()
    .chain(std::iter::once(home.join("receipts")))
    .chain(receipt_out.map(Path::to_path_buf))
    .chain(detached_or_control.iter().cloned())
    .chain(std::iter::once(temp_control_parent.to_owned()))
    {
        if let Some(relative) = resolve_repo_local(repo_root, &path)? {
            protected.push(relative);
        }
    }
    protected.sort();
    protected.dedup();
    Ok(MaterializerAuthority {
        run_artifact_is_dir: repo_root.join(&run_artifact).is_dir(),
        run_artifact,
        protected,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MaterializedCommit {
    fix_commit: String,
    diff_digest: String,
    base_tree_digest: String,
    changed_paths_digest: String,
}

enum MaterializerEvent {
    ApplyStarted,
    ApplyVerified(usize),
}

fn valid_repo_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.starts_with('/')
        && !path.contains(':')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && part != ".git")
}

fn component_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|rest| rest.starts_with('/'))
        || right
            .strip_prefix(left)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn candidate_path_allowed(path: &str, authority: &MaterializerAuthority) -> bool {
    if !valid_repo_relative(path) || !valid_repo_relative(&authority.run_artifact) {
        return false;
    }
    let protected_target = authority
        .protected
        .iter()
        .any(|item| !valid_repo_relative(item) || component_overlap(&authority.run_artifact, item));
    if protected_target
        || authority
            .protected
            .iter()
            .any(|item| component_overlap(path, item))
    {
        return false;
    }
    if authority.run_artifact_is_dir {
        path.strip_prefix(&authority.run_artifact)
            .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
    } else {
        path == authority.run_artifact
    }
}

fn canonical_changed_paths(paths: &[String]) -> Result<(Vec<String>, String), ()> {
    use sha2::{Digest, Sha256};
    let mut sorted = paths.to_vec();
    sorted.sort();
    if sorted.is_empty()
        || sorted.windows(2).any(|pair| pair[0] == pair[1])
        || sorted.iter().any(|path| !valid_repo_relative(path))
    {
        return Err(());
    }
    let bytes = serde_json::to_vec(&sorted).map_err(|_| ())?;
    Ok((sorted, format!("{:x}", Sha256::digest(bytes))))
}

fn git_with_input(
    repo: &Path,
    args: &[&str],
    input: &[u8],
    timeout_ms: u64,
) -> Result<Vec<u8>, ()> {
    use std::process::Stdio;
    if timeout_ms == 0 {
        return Err(());
    }
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    child
        .stdin
        .take()
        .ok_or(())?
        .write_all(input)
        .map_err(|_| ())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(|_| ())? {
            Some(status) => {
                let mut stdout = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    std::io::Read::read_to_end(&mut pipe, &mut stdout).map_err(|_| ())?;
                }
                return status.success().then_some(stdout).ok_or(());
            }
            None if started.elapsed() >= std::time::Duration::from_millis(timeout_ms) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

fn staged_operations<R: VerifyRuntime>(
    repo: &Path,
    runtime: &R,
    deadline: nano_verify::RunDeadline,
) -> Result<Vec<(String, nano_verify::ChangeKind)>, ()> {
    let bytes = git_output_bounded(
        repo,
        &["diff", "--cached", "--name-status", "-z", "--no-renames"],
        remaining(runtime, deadline).ok_or(())?,
    )?;
    let fields: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    if fields.last() != Some(&&[][..]) || (fields.len() - 1) % 2 != 0 {
        return Err(());
    }
    let mut changes = Vec::new();
    for pair in fields[..fields.len() - 1].chunks_exact(2) {
        let status = std::str::from_utf8(pair[0]).map_err(|_| ())?;
        let path = std::str::from_utf8(pair[1]).map_err(|_| ())?.to_owned();
        let kind = match status {
            "A" => nano_verify::ChangeKind::Add,
            "M" => nano_verify::ChangeKind::Modify,
            "D" => nano_verify::ChangeKind::Delete,
            _ => return Err(()),
        };
        changes.push((path, kind));
    }
    changes.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(changes)
}

fn verify_manifest_state<R: VerifyRuntime>(
    repo: &Path,
    manifest: &nano_verify::ExpectedChangeManifest,
    runtime: &R,
    deadline: nano_verify::RunDeadline,
) -> Result<(), ()> {
    use sha2::{Digest, Sha256};
    let actual = staged_operations(repo, runtime, deadline)?;
    let expected: Vec<_> = manifest
        .entries()
        .iter()
        .map(|entry| (entry.path().to_owned(), entry.kind()))
        .collect();
    if actual.is_empty() || actual != expected {
        return Err(());
    }
    for entry in manifest.entries() {
        match entry.kind() {
            nano_verify::ChangeKind::Delete => {
                if entry.postimage_sha256().is_some()
                    || git_output_bounded(
                        repo,
                        &["cat-file", "-e", &format!(":{}", entry.path())],
                        remaining(runtime, deadline).ok_or(())?,
                    )
                    .is_ok()
                {
                    return Err(());
                }
            }
            nano_verify::ChangeKind::Add | nano_verify::ChangeKind::Modify => {
                let indexed = git_output_bounded(
                    repo,
                    &["show", &format!(":{}", entry.path())],
                    remaining(runtime, deadline).ok_or(())?,
                )?;
                let digest = format!("{:x}", Sha256::digest(&indexed));
                if entry.postimage_sha256() != Some(digest.as_str())
                    || std::fs::read(repo.join(entry.path())).map_err(|_| ())? != indexed
                    || !repo.join(entry.path()).is_file()
                {
                    return Err(());
                }
            }
        }
    }
    if !git_text(
        repo,
        &["diff", "--name-only"],
        remaining(runtime, deadline).ok_or(())?,
    )?
    .is_empty()
        || !git_text(
            repo,
            &["ls-files", "--others", "--exclude-standard"],
            remaining(runtime, deadline).ok_or(())?,
        )?
        .is_empty()
    {
        return Err(());
    }
    Ok(())
}

fn rollback_materializer(repo: &Path, starting_commit: &str) -> Result<(), ()> {
    git_success_bounded(repo, &["reset", "--hard", starting_commit], 10_000)?;
    git_success_bounded(
        repo,
        &["read-tree", "--reset", "-u", starting_commit],
        10_000,
    )?;
    git_success_bounded(repo, &["checkout-index", "--all", "--force"], 10_000)?;
    require_identity(
        repo,
        starting_commit,
        &git_text(repo, &["rev-parse", "HEAD^{tree}"], 10_000)?,
        10_000,
    )
}

fn verify_committed_manifest<R: VerifyRuntime>(
    repo: &Path,
    starting_commit: &str,
    fix_commit: &str,
    manifest: &nano_verify::ExpectedChangeManifest,
    runtime: &R,
    deadline: nano_verify::RunDeadline,
) -> Result<(), ()> {
    use sha2::{Digest, Sha256};
    let bytes = git_output_bounded(
        repo,
        &[
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            starting_commit,
            fix_commit,
        ],
        remaining(runtime, deadline).ok_or(())?,
    )?;
    let fields: Vec<&[u8]> = bytes.split(|byte| *byte == 0).collect();
    if fields.last() != Some(&&[][..]) || (fields.len() - 1) % 2 != 0 {
        return Err(());
    }
    let mut actual = Vec::new();
    for pair in fields[..fields.len() - 1].chunks_exact(2) {
        let status = std::str::from_utf8(pair[0]).map_err(|_| ())?;
        let path = std::str::from_utf8(pair[1]).map_err(|_| ())?.to_owned();
        let kind = match status {
            "A" => nano_verify::ChangeKind::Add,
            "M" => nano_verify::ChangeKind::Modify,
            "D" => nano_verify::ChangeKind::Delete,
            _ => return Err(()),
        };
        actual.push((path, kind));
    }
    actual.sort_by(|a, b| a.0.cmp(&b.0));
    let expected: Vec<_> = manifest
        .entries()
        .iter()
        .map(|entry| (entry.path().to_owned(), entry.kind()))
        .collect();
    if actual != expected {
        return Err(());
    }
    for entry in manifest.entries() {
        match entry.kind() {
            nano_verify::ChangeKind::Delete => {
                if entry.postimage_sha256().is_some()
                    || git_output_bounded(
                        repo,
                        &["cat-file", "-e", &format!("{fix_commit}:{}", entry.path())],
                        remaining(runtime, deadline).ok_or(())?,
                    )
                    .is_ok()
                {
                    return Err(());
                }
            }
            nano_verify::ChangeKind::Add | nano_verify::ChangeKind::Modify => {
                let blob = git_output_bounded(
                    repo,
                    &["show", &format!("{fix_commit}:{}", entry.path())],
                    remaining(runtime, deadline).ok_or(())?,
                )?;
                let digest = format!("{:x}", Sha256::digest(blob));
                if entry.postimage_sha256() != Some(digest.as_str()) {
                    return Err(());
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn materialize_candidate<R: VerifyRuntime>(
    repo: &Path,
    starting_commit: &str,
    accepted_bytes: &[u8],
    authority: &MaterializerAuthority,
    commit_message: &str,
    runtime: &R,
    deadline: nano_verify::RunDeadline,
    mut event: impl FnMut(MaterializerEvent),
) -> Result<MaterializedCommit, ()> {
    use sha2::{Digest, Sha256};
    let canonical = repo.canonicalize().map_err(|_| ())?;
    let repo = canonical.as_path();
    if accepted_bytes.len() > 16 * 1024 * 1024 {
        return Err(());
    }
    let starting_tree = git_text(
        repo,
        &["rev-parse", "HEAD^{tree}"],
        remaining(runtime, deadline).ok_or(())?,
    )?;
    require_identity(
        repo,
        starting_commit,
        &starting_tree,
        remaining(runtime, deadline).ok_or(())?,
    )?;
    let parsed = nano_verify::parse_candidate_diff(accepted_bytes).map_err(|_| ())?;
    let bytes_digest = format!("{:x}", Sha256::digest(accepted_bytes));
    if parsed.bytes_sha256() != bytes_digest {
        return Err(());
    }
    let manifest = nano_verify::derive_expected_changes(&parsed, repo).map_err(|_| ())?;
    if manifest.diff_digest() != parsed.bytes_sha256() {
        return Err(());
    }
    require_identity(
        repo,
        starting_commit,
        &starting_tree,
        remaining(runtime, deadline).ok_or(())?,
    )?;
    let (paths, changed_paths_digest) = canonical_changed_paths(parsed.paths())?;
    if paths
        .iter()
        .any(|path| !candidate_path_allowed(path, authority))
    {
        return Err(());
    }

    let transaction = (|| {
        remaining(runtime, deadline).ok_or(())?;
        git_with_input(
            repo,
            &["apply", "--check", "--index", "--whitespace=error-all", "-"],
            accepted_bytes,
            remaining(runtime, deadline).ok_or(())?,
        )?;
        event(MaterializerEvent::ApplyStarted);
        remaining(runtime, deadline).ok_or(())?;
        git_with_input(
            repo,
            &["apply", "--index", "--whitespace=error-all", "-"],
            accepted_bytes,
            remaining(runtime, deadline).ok_or(())?,
        )?;
        if git_text(
            repo,
            &["rev-parse", "HEAD"],
            remaining(runtime, deadline).ok_or(())?,
        )? != starting_commit
        {
            return Err(());
        }
        verify_manifest_state(repo, &manifest, runtime, deadline)?;
        event(MaterializerEvent::ApplyVerified(manifest.entries().len()));
        remaining(runtime, deadline).ok_or(())?;
        git_success_bounded(
            repo,
            &["commit", "-m", commit_message],
            remaining(runtime, deadline).ok_or(())?,
        )?;
        Ok::<_, ()>(())
    })();
    if transaction.is_err() {
        rollback_materializer(repo, starting_commit)?;
        return Err(());
    }

    let postcommit = (|| {
        let fix_commit = git_text(
            repo,
            &["rev-parse", "HEAD"],
            remaining(runtime, deadline).ok_or(())?,
        )?;
        if git_text(
            repo,
            &["rev-parse", "HEAD^1"],
            remaining(runtime, deadline).ok_or(())?,
        )? != starting_commit
        {
            return Err(());
        }
        remaining(runtime, deadline).ok_or(())?;
        verify_committed_manifest(
            repo,
            starting_commit,
            &fix_commit,
            &manifest,
            runtime,
            deadline,
        )?;
        if !git_text(
            repo,
            &["status", "--porcelain=v1", "--untracked-files=all"],
            remaining(runtime, deadline).ok_or(())?,
        )?
        .is_empty()
        {
            return Err(());
        }
        if git_text(
            repo,
            &["rev-parse", "HEAD"],
            remaining(runtime, deadline).ok_or(())?,
        )? != fix_commit
            || !git_text(
                repo,
                &["status", "--porcelain=v1", "--untracked-files=all"],
                remaining(runtime, deadline).ok_or(())?,
            )?
            .is_empty()
        {
            return Err(());
        }
        Ok::<_, ()>(fix_commit)
    })();
    let fix_commit = match postcommit {
        Ok(fix_commit) => fix_commit,
        Err(()) => {
            rollback_materializer(repo, starting_commit)?;
            return Err(());
        }
    };
    Ok(MaterializedCommit {
        fix_commit,
        diff_digest: manifest.diff_digest().to_owned(),
        base_tree_digest: manifest.base_tree_digest().to_owned(),
        changed_paths_digest,
    })
}

#[allow(clippy::too_many_arguments)]
async fn mint_until_materializer<R: VerifyRuntime>(
    workspace: &Path,
    requirement: &str,
    requested_gate: Option<&str>,
    task: Option<&str>,
    budget: Option<u32>,
    cheap_model: &str,
    escalation_models: &[String],
    deadline: nano_verify::RunDeadline,
    home: &Path,
    receipt_out: Option<&Path>,
    runtime: &R,
    events: &mut VerifyEvents<impl Write>,
) -> i32 {
    let Ok(repo_root) = runtime.canonical_repo_root(workspace) else {
        return 2;
    };
    if runtime.temp_preflight().is_err() || remaining(runtime, deadline).is_none() {
        return 3;
    }
    let Some(registry) = runtime
        .load_registry(&repo_root, requested_gate, Some(requirement))
        .ok()
        .flatten()
    else {
        return 2;
    };
    let selected = requested_gate
        .and_then(|id| registry.gates.get(id).map(|entry| (id, entry)))
        .or_else(|| nano_verify::gate_for_requirement(&registry, requirement).ok());
    let Some((gate_id, entry)) = selected else {
        return 2;
    };
    events.verify_started(requirement, gate_id);
    let Some(identity_budget) = remaining(runtime, deadline) else {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    };
    let Ok((starting_commit, starting_tree)) = clean_identity(&repo_root, identity_budget) else {
        return finish_mint(
            events,
            nano_verify::TerminalState::Blocked("git_failed".into()),
            3,
        );
    };
    let Ok(baseline_root) = baseline_worktree_path() else {
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    let baseline_result = async {
        git_success_bounded(
            &repo_root,
            &[
                "worktree",
                "add",
                "--detach",
                &baseline_root.to_string_lossy(),
                &starting_commit,
            ],
            remaining(runtime, deadline).ok_or(())?,
        )?;
        let baseline_root = baseline_root.canonicalize().map_err(|_| ())?;
        require_identity(
            &baseline_root,
            &starting_commit,
            &starting_tree,
            remaining(runtime, deadline).ok_or(())?,
        )?;
        let artifact = runtime.resolve_artifact(&baseline_root, Path::new(&entry.run_artifact))?;
        let inventory = runtime.inventory(&baseline_root.join(&entry.card))?;
        let invocation = registry_invocation(
            &baseline_root,
            gate_id,
            entry,
            remaining(runtime, deadline).ok_or(())?,
        )?;
        let execution = runtime
            .run_baseline_gate(&invocation, &artifact, &inventory)
            .await;
        require_identity(
            &baseline_root,
            &starting_commit,
            &starting_tree,
            remaining(runtime, deadline).ok_or(())?,
        )?;
        require_identity(
            &repo_root,
            &starting_commit,
            &starting_tree,
            remaining(runtime, deadline).ok_or(())?,
        )?;
        if !eligible_baseline(&execution, &inventory) {
            return Err(());
        }
        let nano_verify::ExecutionGateOutcome::Red { verdicts } = execution.outcome else {
            return Err(());
        };
        Ok((inventory, execution.evidence, verdicts))
    }
    .await;
    let cleanup = remaining(runtime, deadline)
        .ok_or(())
        .and_then(|budget| cleanup_receipt_worktree(&repo_root, &baseline_root, budget));
    let Ok((inventory, baseline_evidence, baseline_verdicts)) = baseline_result else {
        return finish_mint(
            events,
            nano_verify::TerminalState::Blocked("baseline_failed".into()),
            3,
        );
    };
    for verdict in &baseline_verdicts {
        events.check_verdict(verdict);
    }
    if cleanup.is_err()
        || remaining(runtime, deadline)
            .and_then(|budget| {
                require_identity(&repo_root, &starting_commit, &starting_tree, budget).ok()
            })
            .is_none()
        || remaining(runtime, deadline).is_none()
    {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let Some(remaining_ms) = remaining(runtime, deadline) else {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    };
    let Ok(invocation) = registry_invocation(&repo_root, gate_id, entry, remaining_ms) else {
        return finish_mint(
            events,
            nano_verify::TerminalState::Blocked("gate_failed".into()),
            3,
        );
    };
    let Ok(temp_control_parent) = std::env::temp_dir().canonicalize() else {
        events.error("artifact_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    let Ok(artifact_workspace) = scheduled_before_deadline(runtime, deadline, || {
        nano_verify::create_artifact_workspace().map_err(|_| ())
    }) else {
        if remaining(runtime, deadline).is_none() {
            return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
        }
        events.error("artifact_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    let cfg = climb_config(cheap_model, escalation_models, budget, deadline);
    let effects = RuntimeEffects { runtime };
    let outcome = nano_verify::run_climb(
        task.unwrap_or(requirement),
        &invocation,
        &inventory,
        artifact_workspace,
        &cfg,
        &effects,
    )
    .await;
    for entry in outcome.log() {
        events.climb_update(entry);
    }
    let exit = mint_terminal_exit(outcome.terminal());
    let Some(accepted) = outcome.accepted_artifact() else {
        let terminal_exit = if exit == 0 { 3 } else { exit };
        events.verify_completed(outcome.terminal(), terminal_exit);
        return if exit == 0 { 3 } else { exit };
    };
    if exit != 0 {
        events.verify_completed(outcome.terminal(), exit);
        return exit;
    }
    let Ok(bytes) = scheduled_before_deadline(runtime, deadline, || {
        accepted.read_exact_bytes().map_err(|_| ())
    }) else {
        if remaining(runtime, deadline).is_none() {
            return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
        }
        events.error("artifact_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    let Ok(authority) = materializer_authority(
        &repo_root,
        entry,
        home,
        receipt_out,
        std::slice::from_ref(&baseline_root),
        &temp_control_parent,
    ) else {
        events.error("artifact_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    let Ok(committed) = materialize_candidate(
        &repo_root,
        &starting_commit,
        &bytes,
        &authority,
        "wayland-nano verify: materialize accepted candidate",
        runtime,
        deadline,
        |event| match event {
            MaterializerEvent::ApplyStarted => events.apply_started(gate_id),
            MaterializerEvent::ApplyVerified(count) => events.apply_verified(gate_id, count),
        },
    ) else {
        events.error("git_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    let Some(remaining_ms) = remaining(runtime, deadline) else {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    };
    let Ok(invocation) = registry_invocation(&repo_root, gate_id, entry, remaining_ms) else {
        return finish_mint(
            events,
            nano_verify::TerminalState::Blocked("gate_failed".into()),
            3,
        );
    };
    if remaining(runtime, deadline).is_none() {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let Ok(artifact) = runtime.resolve_artifact(&repo_root, Path::new(&entry.run_artifact)) else {
        return finish_mint(
            events,
            nano_verify::TerminalState::Blocked("artifact_failed".into()),
            3,
        );
    };
    if remaining(runtime, deadline).is_none() {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let rerun = runtime.run_gate(&invocation, &artifact, &inventory).await;
    let coherence =
        remaining(runtime, deadline).and_then(|budget| clean_identity(&repo_root, budget).ok());
    if coherence.is_none()
        || !matches!(rerun, nano_verify::ExecutionGateOutcome::Green { .. })
        || coherence
            .map(|(head, _)| head != committed.fix_commit)
            .unwrap_or(true)
    {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let (Some(exit_code), Some(log_digest)) =
        (baseline_evidence.exit_code, baseline_evidence.log_digest)
    else {
        return finish_mint(
            events,
            nano_verify::TerminalState::Blocked("baseline_failed".into()),
            3,
        );
    };
    let receipt = nano_verify::Receipt {
        schema: 1,
        requirement: requirement.to_owned(),
        test: entry.script.clone(),
        gate_id: gate_id.to_owned(),
        gate_closure_digest: entry.closure_digest.clone(),
        failing_run: nano_verify::FailingRun {
            exit_code,
            log_digest,
            observed_at_commit: starting_commit,
        },
        fix_commit: committed.fix_commit,
        minted_at: "1970-01-01T00:00:00Z".into(),
        minted_by: format!("wayland-nano {}", env!("CARGO_PKG_VERSION")),
    };
    let Ok(receipt) = nano_verify::mint_receipt(receipt) else {
        events.error("store_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    if remaining(runtime, deadline).is_none() {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let store = home.join("receipts");
    if std::fs::create_dir_all(&store).is_err() {
        events.error("store_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    }
    if remaining(runtime, deadline).is_none() {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let Ok(store_path) = nano_verify::write_receipt(&store, &receipt) else {
        events.error("store_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    if remaining(runtime, deadline).is_none() {
        return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
    }
    let Ok(bytes) = std::fs::read(&store_path) else {
        events.error("store_failed");
        return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
    };
    if let Some(output) = receipt_out {
        if remaining(runtime, deadline).is_none() {
            return finish_mint(events, nano_verify::TerminalState::TimedOut, 3);
        }
        if crate::exec_mode::atomic_replace_write(output, &bytes).is_err() {
            events.error("store_failed");
            return finish_mint(events, nano_verify::TerminalState::CrashedRecovered, 1);
        }
    }
    events.receipt_minted(&receipt, &store_path);
    events.verify_completed(&nano_verify::TerminalState::Verified, 0);
    0
}

fn registry_invocation(
    root: &Path,
    gate_id: &str,
    entry: &nano_verify::GateRegistryEntry,
    remaining_ms: u64,
) -> Result<nano_verify::GateInvocation, ()> {
    if remaining_ms == 0 {
        return Err(());
    }
    let cwd = match entry.closure.cwd_policy {
        nano_verify::CwdPolicy::RepoRoot => root.to_owned(),
        nano_verify::CwdPolicy::GateDir => root
            .join(&entry.card)
            .parent()
            .map(Path::to_path_buf)
            .ok_or(())?,
    };
    Ok(nano_verify::GateInvocation {
        argv: entry.closure.argv.iter().map(Into::into).collect(),
        cwd,
        env: entry
            .closure
            .env
            .iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        timeout: std::time::Duration::from_millis(remaining_ms.min(400_000)),
        gate_id: gate_id.to_owned(),
    })
}

fn baseline_worktree_path() -> Result<PathBuf, ()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let root = canonical_env_dir("TEMP")?;
    Ok(native_windows_path(root.join(format!(
        "wayland-nano-baseline-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))))
}

#[cfg(windows)]
fn native_windows_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or(path)
}

#[cfg(not(windows))]
fn native_windows_path(path: PathBuf) -> PathBuf {
    path
}

fn clean_identity(repo: &Path, timeout_ms: u64) -> Result<(String, String), ()> {
    let commit = git_text(repo, &["rev-parse", "HEAD"], timeout_ms)?;
    let tree = git_text(repo, &["rev-parse", "HEAD^{tree}"], timeout_ms)?;
    if !git_text(
        repo,
        &["status", "--porcelain=v1", "--untracked-files=all"],
        timeout_ms,
    )?
    .is_empty()
    {
        return Err(());
    }
    Ok((commit, tree))
}

fn require_identity(repo: &Path, commit: &str, tree: &str, timeout_ms: u64) -> Result<(), ()> {
    let actual = clean_identity(repo, timeout_ms)?;
    if actual.0 == commit && actual.1 == tree {
        Ok(())
    } else {
        Err(())
    }
}

fn git_text(repo: &Path, args: &[&str], timeout_ms: u64) -> Result<String, ()> {
    let bytes = git_output_bounded(repo, args, timeout_ms)?;
    String::from_utf8(bytes)
        .map(|text| text.trim().to_owned())
        .map_err(|_| ())
}
