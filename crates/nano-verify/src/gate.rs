//! Gate invocation and fail-closed output parsing primitives.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvidence {
    pub exit_code: Option<i64>,
    pub log_digest: Option<String>,
    pub artifact_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateExecution {
    pub outcome: ExecutionGateOutcome,
    pub evidence: GateEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineGateEvidence {
    pub exit_code: Option<i64>,
    pub log_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineGateExecution {
    pub outcome: ExecutionGateOutcome,
    pub evidence: BaselineGateEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionFailClosedReason {
    NoGateOutput,
    Timeout,
    SpawnError(String),
    ArtifactInvalid,
    OutputIncomplete,
    AbnormalTermination,
    InconsistentSummary {
        passed: u64,
        total: u64,
    },
    InconsistentVerdicts {
        reported_passed: u64,
        reported_total: u64,
        expected_passed: u64,
        expected_total: u64,
    },
    UnknownCheckId(String),
    InvalidInventory,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionGateOutcome {
    Green { verdicts: Vec<CheckVerdict> },
    Red { verdicts: Vec<CheckVerdict> },
    FailClosed(ExecutionFailClosedReason),
}

#[derive(Clone)]
pub struct CandidateArtifact {
    workspace: std::sync::Arc<ArtifactWorkspaceInner>,
    path: std::path::PathBuf,
    bytes_sha256: String,
    _seal: CandidateArtifactSeal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidateArtifactSeal;

impl std::fmt::Debug for CandidateArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CandidateArtifact")
            .field("bytes_sha256", &self.bytes_sha256)
            .finish_non_exhaustive()
    }
}

impl PartialEq for CandidateArtifact {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.workspace, &other.workspace)
            && self.path == other.path
            && self.bytes_sha256 == other.bytes_sha256
    }
}

impl Eq for CandidateArtifact {}

impl CandidateArtifact {
    pub fn bytes_sha256(&self) -> &str {
        &self.bytes_sha256
    }

    pub fn read_exact_bytes(&self) -> Result<Vec<u8>, crate::VerifyError> {
        validate_workspace_inner(&self.workspace)?;
        validate_artifact_path(&self.workspace.root, &self.path)?;
        let bytes = std::fs::read(&self.path).map_err(crate::VerifyError::Artifact)?;
        if sha256(&bytes) != self.bytes_sha256 {
            return artifact_invalid("candidate bytes changed");
        }
        Ok(bytes)
    }

    #[cfg(test)]
    pub(crate) fn inert(identity: &str) -> Self {
        let temp = tempfile::tempdir().expect("test temp workspace");
        let root = temp.path().canonicalize().expect("canonical test temp");
        let path = root.join("inert.diff");
        std::fs::write(&path, identity.as_bytes()).expect("write inert candidate");
        Self {
            workspace: std::sync::Arc::new(ArtifactWorkspaceInner { root, _guard: temp }),
            path,
            bytes_sha256: sha256(identity.as_bytes()),
            _seal: CandidateArtifactSeal,
        }
    }
}

pub struct ArtifactWorkspace {
    inner: std::sync::Arc<ArtifactWorkspaceInner>,
    _seal: ArtifactWorkspaceSeal,
}
struct ArtifactWorkspaceInner {
    root: std::path::PathBuf,
    _guard: tempfile::TempDir,
}
struct ArtifactWorkspaceSeal;

impl std::fmt::Debug for ArtifactWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactWorkspace").finish_non_exhaustive()
    }
}
impl PartialEq for ArtifactWorkspace {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.inner, &other.inner)
    }
}
impl Eq for ArtifactWorkspace {}

pub fn create_artifact_workspace() -> Result<ArtifactWorkspace, crate::VerifyError> {
    let parent = std::env::temp_dir();
    let canonical = parent
        .canonicalize()
        .map_err(crate::VerifyError::Artifact)?;
    let lexical_safe = parent.is_absolute()
        && !parent.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        });
    if !lexical_safe || !canonical.is_dir() || path_has_link(&canonical)? {
        return artifact_invalid("unsafe temporary directory");
    }
    let guard = tempfile::Builder::new()
        .prefix("wayland-nano-candidate-")
        .tempdir_in(&canonical)
        .map_err(crate::VerifyError::Artifact)?;
    let root = guard
        .path()
        .canonicalize()
        .map_err(crate::VerifyError::Artifact)?;
    if root.parent() != Some(canonical.as_path()) || path_has_link(&root)? {
        return artifact_invalid("unsafe artifact workspace");
    }
    Ok(ArtifactWorkspace {
        inner: std::sync::Arc::new(ArtifactWorkspaceInner {
            root,
            _guard: guard,
        }),
        _seal: ArtifactWorkspaceSeal,
    })
}

pub(crate) fn create_candidate_artifact(
    workspace: &ArtifactWorkspace,
    bytes: &[u8],
) -> Result<CandidateArtifact, crate::VerifyError> {
    let parsed = crate::parse_candidate_diff(bytes)?;
    validate_workspace_inner(&workspace.inner)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let name = format!(
        "candidate-{}.diff",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let path = workspace.inner.root.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&path).map_err(crate::VerifyError::Artifact)?;
    std::io::Write::write_all(&mut file, bytes).map_err(crate::VerifyError::Artifact)?;
    file.sync_all().map_err(crate::VerifyError::Artifact)?;
    let path = path.canonicalize().map_err(crate::VerifyError::Artifact)?;
    let artifact = CandidateArtifact {
        workspace: workspace.inner.clone(),
        path,
        bytes_sha256: parsed.bytes_sha256().to_owned(),
        _seal: CandidateArtifactSeal,
    };
    artifact.read_exact_bytes()?;
    Ok(artifact)
}

pub(crate) fn validate_artifact_workspace(
    workspace: &ArtifactWorkspace,
) -> Result<(), crate::VerifyError> {
    validate_workspace_inner(&workspace.inner)
}

#[cfg(test)]
pub(crate) fn mutate_candidate_for_test(artifact: &CandidateArtifact) {
    std::fs::write(&artifact.path, b"mutated after binding\n").unwrap();
}

fn validate_workspace_inner(inner: &ArtifactWorkspaceInner) -> Result<(), crate::VerifyError> {
    let canonical = inner
        .root
        .canonicalize()
        .map_err(crate::VerifyError::Artifact)?;
    if canonical != inner.root || !canonical.is_dir() || path_has_link(&canonical)? {
        return artifact_invalid("artifact workspace changed");
    }
    Ok(())
}
fn validate_artifact_path(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<(), crate::VerifyError> {
    let canonical = path.canonicalize().map_err(crate::VerifyError::Artifact)?;
    if canonical == root
        || !canonical.starts_with(root)
        || canonical != path
        || path_has_link(&canonical)?
    {
        return artifact_invalid("unsafe candidate artifact");
    }
    let meta = std::fs::symlink_metadata(path).map_err(crate::VerifyError::Artifact)?;
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return artifact_invalid("candidate is not regular");
    }
    Ok(())
}
fn path_has_link(path: &std::path::Path) -> Result<bool, crate::VerifyError> {
    for current in path.ancestors().filter(|p| p.is_absolute()) {
        let meta = std::fs::symlink_metadata(current).map_err(crate::VerifyError::Artifact)?;
        if meta.file_type().is_symlink() {
            return Ok(true);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            if meta.file_attributes() & 0x400 != 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn artifact_invalid<T>(message: &str) -> Result<T, crate::VerifyError> {
    Err(crate::VerifyError::Artifact(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.to_owned(),
    )))
}
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// One gate invocation. argv ONLY — no shell string ever reaches the OS
/// (gate-runner.cts:116-123). Artifact path is appended as the final argv token at
/// spawn time (gate-runner.cts:98,119); it is NOT part of the closure digest.
#[derive(Debug, Clone)]
pub struct GateInvocation {
    pub argv: Vec<std::ffi::OsString>,
    pub cwd: std::path::PathBuf, // materialized from CwdPolicy by the caller
    /// Declared env from the closure; spawn = env_clear + baseline allowlist + this.
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub timeout: std::time::Duration, // default 400s (gate-runner.cts:33-34)
    pub gate_id: String,              // registry key this invocation was built from
}

/// Cooperative cancellation handle for a running gate subprocess.
#[derive(Clone, Debug, Default)]
pub struct GateCancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl GateCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// Execute one gate subprocess. The production implementation is materialized by
/// Plan 04-04 after its real-process contract has been observed failing.
pub async fn run_gate(
    inv: &GateInvocation,
    artifact_path: &std::path::Path,
    inventory: &[(String, FailCategory)],
) -> GateOutcome {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    const STDOUT_CAP: usize = 16 * 1024 * 1024;
    if validate_inventory(inventory).is_err() {
        return GateOutcome::FailClosed(FailClosedReason::InvalidInventory);
    }
    let Some((program, args)) = inv.argv.split_first() else {
        return spawn_failure("empty invocation");
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .arg(artifact_path)
        .current_dir(&inv.cwd)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    for name in baseline_environment() {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(inv.env.iter().cloned());

    #[cfg(windows)]
    let job = match WindowsJob::create() {
        Ok(job) => {
            job.prepare(&mut command);
            job
        }
        Err(_) => return spawn_failure("containment unavailable"),
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return spawn_failure("process spawn failed"),
    };
    #[cfg(windows)]
    if job.assign_and_resume(&child).is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return spawn_failure("containment assignment failed");
    }
    #[cfg(unix)]
    let process_group = child.id().map(|id| id as i32);

    let Some(mut stdout) = child.stdout.take() else {
        terminate_tree(
            &mut child,
            #[cfg(windows)]
            &job,
            #[cfg(unix)]
            process_group,
        )
        .await;
        return spawn_failure("stdout unavailable");
    };
    let collect = async {
        let mut captured = Vec::with_capacity(8192);
        let mut chunk = [0_u8; 8192];
        loop {
            let read = stdout.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            let remaining = STDOUT_CAP.saturating_sub(captured.len());
            captured.extend_from_slice(&chunk[..read.min(remaining)]);
        }
        Ok::<_, std::io::Error>(captured)
    };
    let execution = async {
        let (captured, waited) = tokio::join!(collect, child.wait());
        waited?;
        captured
    };
    let captured = match tokio::time::timeout(inv.timeout, execution).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            terminate_tree(
                &mut child,
                #[cfg(windows)]
                &job,
                #[cfg(unix)]
                process_group,
            )
            .await;
            return spawn_failure("process wait failed");
        }
        Err(_) => {
            terminate_tree(
                &mut child,
                #[cfg(windows)]
                &job,
                #[cfg(unix)]
                process_group,
            )
            .await;
            return GateOutcome::FailClosed(FailClosedReason::Timeout);
        }
    };
    parse_gate_output(&String::from_utf8_lossy(&captured), inventory)
}

pub async fn run_gate_execution(
    inv: &GateInvocation,
    artifact: &CandidateArtifact,
    inventory: &[(String, FailCategory)],
) -> GateExecution {
    run_gate_execution_with_cancellation(inv, artifact, inventory, None).await
}

pub async fn run_gate_execution_with_cancellation(
    inv: &GateInvocation,
    artifact: &CandidateArtifact,
    inventory: &[(String, FailCategory)],
    cancellation: Option<&GateCancellation>,
) -> GateExecution {
    let artifact_sha256 = artifact.bytes_sha256.clone();
    if validate_inventory(inventory).is_err() {
        return GateExecution {
            outcome: ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::InvalidInventory),
            evidence: GateEvidence {
                exit_code: None,
                log_digest: None,
                artifact_sha256,
            },
        };
    }
    if artifact.read_exact_bytes().is_err() {
        return GateExecution {
            outcome: ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::ArtifactInvalid),
            evidence: GateEvidence {
                exit_code: None,
                log_digest: None,
                artifact_sha256,
            },
        };
    }
    let capture = capture_complete(inv, &artifact.path, cancellation).await;
    let (outcome, exit_code, log_digest) = execution_result(capture, inventory);
    if artifact.read_exact_bytes().is_err() {
        return GateExecution {
            outcome: ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::ArtifactInvalid),
            evidence: GateEvidence {
                exit_code: None,
                log_digest: None,
                artifact_sha256,
            },
        };
    }
    GateExecution {
        outcome,
        evidence: GateEvidence {
            exit_code,
            log_digest,
            artifact_sha256,
        },
    }
}

pub async fn run_gate_baseline_execution(
    inv: &GateInvocation,
    run_artifact: &std::path::Path,
    inventory: &[(String, FailCategory)],
) -> BaselineGateExecution {
    if validate_inventory(inventory).is_err() {
        return BaselineGateExecution {
            outcome: ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::InvalidInventory),
            evidence: BaselineGateEvidence {
                exit_code: None,
                log_digest: None,
            },
        };
    }
    let valid = run_artifact.canonicalize().ok().is_some_and(|p| {
        p == run_artifact && (p.is_file() || p.is_dir()) && path_has_link(&p).ok() == Some(false)
    });
    if !valid {
        return BaselineGateExecution {
            outcome: ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::ArtifactInvalid),
            evidence: BaselineGateEvidence {
                exit_code: None,
                log_digest: None,
            },
        };
    }
    let (outcome, exit_code, log_digest) =
        execution_result(capture_complete(inv, run_artifact, None).await, inventory);
    BaselineGateExecution {
        outcome,
        evidence: BaselineGateEvidence {
            exit_code,
            log_digest,
        },
    }
}

enum CompleteCapture {
    Complete {
        bytes: Vec<u8>,
        exit_code: Option<i64>,
    },
    Timeout,
    Spawn,
    Incomplete,
    Abnormal,
    Cancelled,
}

async fn capture_complete(
    inv: &GateInvocation,
    artifact_path: &std::path::Path,
    cancellation: Option<&GateCancellation>,
) -> CompleteCapture {
    use tokio::io::AsyncReadExt as _;
    const CAP: usize = 16 * 1024 * 1024;
    let Some((program, args)) = inv.argv.split_first() else {
        return CompleteCapture::Spawn;
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .arg(artifact_path)
        .current_dir(&inv.cwd)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    for name in baseline_environment() {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.envs(inv.env.iter().cloned());
    #[cfg(windows)]
    let job = match WindowsJob::create() {
        Ok(job) => {
            job.prepare(&mut command);
            job
        }
        Err(_) => return CompleteCapture::Spawn,
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return CompleteCapture::Spawn,
    };
    #[cfg(windows)]
    if job.assign_and_resume(&child).is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return CompleteCapture::Spawn;
    }
    #[cfg(unix)]
    let process_group = child.id().map(|id| id as i32);
    let Some(mut stdout) = child.stdout.take() else {
        return CompleteCapture::Spawn;
    };
    let collect = async move {
        let mut bytes = Vec::new();
        let mut buf = [0u8; 8192];
        let mut overflow = false;
        loop {
            let read = stdout.read(&mut buf).await.map_err(|_| ())?;
            if read == 0 {
                break;
            }
            let room = CAP.saturating_add(1).saturating_sub(bytes.len());
            bytes.extend_from_slice(&buf[..read.min(room)]);
            overflow |= read > room || bytes.len() > CAP;
        }
        Ok::<_, ()>((bytes, overflow))
    };
    let result = {
        let execution = async {
            let (output, status) = tokio::join!(collect, child.wait());
            Ok::<_, ()>((output.map_err(|_| ())?, status.map_err(|_| ())?))
        };
        let cancellation_requested = async {
            loop {
                if cancellation.is_some_and(GateCancellation::is_cancelled) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::pin!(execution);
        tokio::select! {
            result = tokio::time::timeout(inv.timeout, &mut execution) => Some(result),
            () = cancellation_requested, if cancellation.is_some() => None,
        }
    };
    match result {
        None => {
            terminate_tree(
                &mut child,
                #[cfg(windows)]
                &job,
                #[cfg(unix)]
                process_group,
            )
            .await;
            CompleteCapture::Cancelled
        }
        Some(Ok(Ok(((_bytes, true), _)))) => CompleteCapture::Incomplete,
        Some(Ok(Ok(((bytes, false), status)))) => match status.code() {
            Some(code) => CompleteCapture::Complete {
                bytes,
                exit_code: Some(i64::from(code)),
            },
            None => CompleteCapture::Abnormal,
        },
        Some(Ok(Err(_))) => CompleteCapture::Incomplete,
        Some(Err(_)) => {
            terminate_tree(
                &mut child,
                #[cfg(windows)]
                &job,
                #[cfg(unix)]
                process_group,
            )
            .await;
            CompleteCapture::Timeout
        }
    }
}

fn execution_result(
    capture: CompleteCapture,
    inventory: &[(String, FailCategory)],
) -> (ExecutionGateOutcome, Option<i64>, Option<String>) {
    match capture {
        CompleteCapture::Complete { bytes, exit_code } => {
            let digest = Some(sha256(&bytes));
            (
                parse_execution_output(&String::from_utf8_lossy(&bytes), inventory),
                exit_code,
                digest,
            )
        }
        CompleteCapture::Timeout => (
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::Timeout),
            None,
            None,
        ),
        CompleteCapture::Spawn => (
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::SpawnError(
                "process spawn failed".into(),
            )),
            None,
            None,
        ),
        CompleteCapture::Incomplete => (
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::OutputIncomplete),
            None,
            None,
        ),
        CompleteCapture::Abnormal => (
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::AbnormalTermination),
            None,
            None,
        ),
        CompleteCapture::Cancelled => (
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::Cancelled),
            None,
            None,
        ),
    }
}

#[allow(clippy::manual_saturating_arithmetic)] // frozen contract requires checked subtraction
fn parse_execution_output(
    stdout: &str,
    inventory: &[(String, FailCategory)],
) -> ExecutionGateOutcome {
    match parse_gate_output(stdout, inventory) {
        GateOutcome::Green { verdicts } => ExecutionGateOutcome::Green { verdicts },
        GateOutcome::Red { verdicts } => ExecutionGateOutcome::Red { verdicts },
        GateOutcome::FailClosed(FailClosedReason::InconsistentSummary { passed, total }) => {
            let expected_total = u64::try_from(inventory.len()).unwrap_or(u64::MAX);
            let known: std::collections::BTreeSet<_> = stdout
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("FAIL ")
                        .and_then(|rest| rest.split_whitespace().next())
                })
                .filter(|id| inventory.iter().any(|(known, _)| known == id))
                .collect();
            let expected_passed = expected_total
                .checked_sub(u64::try_from(known.len()).unwrap_or(u64::MAX))
                .unwrap_or_default();
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::InconsistentVerdicts {
                reported_passed: passed,
                reported_total: total,
                expected_passed,
                expected_total,
            })
        }
        GateOutcome::FailClosed(FailClosedReason::NoGateOutput) => {
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::NoGateOutput)
        }
        GateOutcome::FailClosed(FailClosedReason::Timeout) => {
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::Timeout)
        }
        GateOutcome::FailClosed(FailClosedReason::SpawnError(v)) => {
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::SpawnError(v))
        }
        GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(v)) => {
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::UnknownCheckId(v))
        }
        GateOutcome::FailClosed(FailClosedReason::InvalidInventory) => {
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::InvalidInventory)
        }
    }
}

fn baseline_environment() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &[
            "PATH",
            "HOME",
            "TMPDIR",
            "TEMP",
            "TMP",
            "SYSTEMROOT",
            "PATHEXT",
            "USERPROFILE",
            "COMSPEC",
        ]
    }
    #[cfg(not(windows))]
    {
        &["PATH", "HOME", "TMPDIR", "TEMP", "TMP"]
    }
}

fn spawn_failure(message: &'static str) -> GateOutcome {
    GateOutcome::FailClosed(FailClosedReason::SpawnError(message.to_owned()))
}

#[cfg(unix)]
async fn terminate_tree(child: &mut tokio::process::Child, process_group: Option<i32>) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    const SIGKILL: i32 = 9;
    if let Some(group) = process_group {
        unsafe {
            kill(-group, SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[cfg(windows)]
async fn terminate_tree(child: &mut tokio::process::Child, job: &WindowsJob) {
    let _ = job.terminate();
    let _ = child.wait().await;
}

#[cfg(windows)]
struct WindowsJob(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl WindowsJob {
    fn create() -> std::io::Result<Self> {
        use std::os::windows::io::FromRawHandle as _;
        use windows_sys::Win32::System::JobObjects::*;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> isize;
        }
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw as _) });
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of_mut!(limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    fn prepare(&self, command: &mut tokio::process::Command) {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        command.as_std_mut().creation_flags(CREATE_SUSPENDED);
    }

    fn assign_and_resume(&self, child: &tokio::process::Child) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        let process = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("missing process handle"))?
            as HANDLE;
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle() as HANDLE, process) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtResumeProcess(process: HANDLE) -> i32;
        }
        if unsafe { NtResumeProcess(process) } < 0 {
            return Err(std::io::Error::other("resume failed"));
        }
        Ok(())
    }

    fn terminate(&self) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        if unsafe { TerminateJobObject(self.0.as_raw_handle() as HANDLE, 1) } == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailCategory {
    Structure,
    Value,
    Relation,
    Grounding,
    Execution,
    Security,
}

/// Per-check verdict for the FULL inventory — not just failures
/// (audit seam #1, spec-coldread-audit.md:263; WP3 assumption SPEC-WP3:12).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckVerdict {
    pub id: String,
    pub category: FailCategory,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailClosedReason {
    NoGateOutput,
    Timeout,
    SpawnError(String),
    InconsistentSummary { passed: u64, total: u64 },
    UnknownCheckId(String),
    InvalidInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    Green { verdicts: Vec<CheckVerdict> },
    Red { verdicts: Vec<CheckVerdict> },
    FailClosed(FailClosedReason),
}

impl GateOutcome {
    pub fn score(&self) -> (i64, i64) {
        match self {
            Self::Green { verdicts } | Self::Red { verdicts } => {
                let passed = verdicts.iter().filter(|verdict| verdict.passed).count();
                (
                    i64::try_from(passed).unwrap_or(i64::MAX),
                    i64::try_from(verdicts.len()).unwrap_or(i64::MAX),
                )
            }
            Self::FailClosed(_) => (0, 1),
        }
    }

    pub fn fails(&self) -> Vec<String> {
        match self {
            Self::Green { .. } => Vec::new(),
            Self::Red { verdicts } => verdicts
                .iter()
                .filter(|verdict| !verdict.passed)
                .map(|verdict| format!("{} {}", verdict.id, verdict.category.as_str()))
                .collect(),
            Self::FailClosed(reason) => vec![reason.sentinel().to_owned()],
        }
    }
}

/// PURE. stdout + the card's check inventory → outcome. Never panics.
pub fn parse_gate_output(stdout: &str, inventory: &[(String, FailCategory)]) -> GateOutcome {
    if validate_inventory(inventory).is_err() {
        return GateOutcome::FailClosed(FailClosedReason::InvalidInventory);
    }
    let mut summary = None;
    let mut failures: Vec<(&str, FailCategory)> = Vec::new();

    for line in stdout.lines() {
        if let Some(parsed) = scan_summary(line) {
            summary = Some(parsed);
        }
        if let Some(rest) = line.strip_prefix("FAIL ") {
            let mut fields = rest.split_whitespace();
            let id = fields.next().unwrap_or("");
            let Some(category) = fields.next().and_then(FailCategory::parse) else {
                return GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(bounded_id(id)));
            };
            if !valid_check_id(id) || fields.next().is_some() {
                return GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(bounded_id(id)));
            }
            let Some((_, declared_category)) = inventory.iter().find(|(known, _)| known == id)
            else {
                return GateOutcome::FailClosed(FailClosedReason::UnknownCheckId(id.to_owned()));
            };
            if *declared_category != category
                || failures.iter().any(|(failed_id, _)| *failed_id == id)
            {
                let (passed, total) = summary.unwrap_or((0, 0));
                return GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                    passed,
                    total,
                });
            }
            failures.push((id, category));
        }
    }

    let Some((passed, total)) = summary else {
        return GateOutcome::FailClosed(FailClosedReason::NoGateOutput);
    };
    let inventory_total = u64::try_from(inventory.len()).unwrap_or(u64::MAX);
    let fail_count = u64::try_from(failures.len()).unwrap_or(u64::MAX);
    if inventory.is_empty()
        || total != inventory_total
        || passed != total.checked_sub(fail_count).unwrap_or(u64::MAX)
    {
        return GateOutcome::FailClosed(FailClosedReason::InconsistentSummary { passed, total });
    }

    let verdicts = inventory
        .iter()
        .map(|(id, category)| CheckVerdict {
            id: id.clone(),
            category: *category,
            passed: !failures.iter().any(|(failed_id, _)| failed_id == id),
        })
        .collect();
    if failures.is_empty() {
        GateOutcome::Green { verdicts }
    } else {
        GateOutcome::Red { verdicts }
    }
}

impl FailCategory {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "structure" => Some(Self::Structure),
            "value" => Some(Self::Value),
            "relation" => Some(Self::Relation),
            "grounding" => Some(Self::Grounding),
            "execution" => Some(Self::Execution),
            "security" => Some(Self::Security),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Value => "value",
            Self::Relation => "relation",
            Self::Grounding => "grounding",
            Self::Execution => "execution",
            Self::Security => "security",
        }
    }
}

impl FailClosedReason {
    const fn sentinel(&self) -> &'static str {
        match self {
            Self::NoGateOutput => "<no gate output>",
            Self::Timeout => "<gate timeout>",
            Self::SpawnError(_) => "<gate spawn error>",
            Self::InconsistentSummary { .. } => "<inconsistent gate summary>",
            Self::UnknownCheckId(_) => "<unknown check id>",
            Self::InvalidInventory => "<invalid gate inventory>",
        }
    }
}

fn scan_summary(line: &str) -> Option<(u64, u64)> {
    let line = line.trim_start();
    let colon = line.find(':')?;
    let label = &line[..colon];
    if !label.ends_with("gate")
        || label.is_empty()
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return None;
    }

    let mut rest = line[colon + 1..].trim_start();
    let passed_end = rest.bytes().take_while(u8::is_ascii_digit).count();
    if passed_end == 0 {
        return None;
    }
    let passed = rest[..passed_end].parse().ok()?;
    rest = rest[passed_end..].trim_start();
    rest = rest.strip_prefix('/')?.trim_start();
    let total_end = rest.bytes().take_while(u8::is_ascii_digit).count();
    if total_end == 0 {
        return None;
    }
    let total = rest[..total_end].parse().ok()?;
    Some((passed, total))
}

fn valid_check_id(id: &str) -> bool {
    let Some((prefix, digits)) = id.split_once('-') else {
        return false;
    };
    (2..=4).contains(&prefix.len())
        && prefix.bytes().all(|byte| byte.is_ascii_uppercase())
        && digits.len() == 2
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn validate_inventory(inventory: &[(String, FailCategory)]) -> Result<(), ()> {
    if inventory.is_empty() {
        return Err(());
    }
    let mut ids = std::collections::BTreeSet::new();
    if inventory
        .iter()
        .any(|(id, _)| !valid_check_id(id) || !ids.insert(id.as_str()))
    {
        return Err(());
    }
    Ok(())
}

fn bounded_id(id: &str) -> String {
    if valid_check_id(id) {
        id.to_owned()
    } else {
        "<malformed>".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> Vec<(String, FailCategory)> {
        vec![
            ("TG-01".into(), FailCategory::Value),
            ("TG-02".into(), FailCategory::Execution),
            ("TG-03".into(), FailCategory::Structure),
            ("TG-04".into(), FailCategory::Security),
        ]
    }

    #[test]
    fn wp2_workspace_candidate_confinement_matrix() {
        let workspace = create_artifact_workspace().unwrap();
        let bytes =
            b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let artifact = create_candidate_artifact(&workspace, bytes).unwrap();
        assert_eq!(artifact.read_exact_bytes().unwrap(), bytes);
        assert_eq!(artifact.bytes_sha256(), sha256(bytes));
        assert!(!format!("{artifact:?}").contains("candidate-"));
    }

    #[test]
    fn detailed_verdict_coherence_maps_exact_fields() {
        let detailed = parse_execution_output("FAIL TG-01 value\ngate: 4/4\n", &inventory());
        assert_eq!(
            detailed,
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::InconsistentVerdicts {
                reported_passed: 4,
                reported_total: 4,
                expected_passed: 3,
                expected_total: 4
            })
        );
    }

    fn verdicts(failed: &[&str]) -> Vec<CheckVerdict> {
        inventory()
            .into_iter()
            .map(|(id, category)| CheckVerdict {
                passed: !failed.contains(&id.as_str()),
                id,
                category,
            })
            .collect()
    }

    #[test]
    fn parse_summary_last_match_wins() {
        let actual = parse_gate_output("gate: 4/4\nFAIL TG-02 execution\ngate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-02"])
            }
        );
        assert_eq!(actual.score(), (3, 4));
    }

    #[test]
    fn parse_prefixed_slug_summary() {
        let actual =
            parse_gate_output("gate :3/4\nFAIL TG-04 security\nmy-gate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-04"])
            }
        );
    }

    #[test]
    fn parse_no_summary_fails_closed() {
        assert_eq!(
            parse_gate_output("FAIL TG-03 structure", &inventory()),
            GateOutcome::FailClosed(FailClosedReason::NoGateOutput)
        );
    }

    #[test]
    fn parse_empty_stdout_fails_closed() {
        let actual = parse_gate_output(" \r\n\t", &inventory());
        assert_eq!(
            actual,
            GateOutcome::FailClosed(FailClosedReason::NoGateOutput)
        );
        assert_eq!(actual.score(), (0, 1));
        assert_eq!(actual.fails(), vec!["<no gate output>"]);
    }

    #[test]
    fn parse_fail_v2_canonical() {
        let actual = parse_gate_output("FAIL TG-03 structure\ngate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-03"])
            }
        );
        assert_eq!(actual.fails(), vec!["TG-03 structure"]);
    }

    #[test]
    fn parse_fail_v2_whitespace_collapses() {
        let actual = parse_gate_output("FAIL   TG-03   structure  \ngate: 3 / 4", &inventory());
        assert_eq!(actual.fails(), vec!["TG-03 structure"]);
    }

    #[test]
    fn parse_unknown_fail_id_fails_closed() {
        let actual = parse_gate_output("FAIL ZZ-99 value\ngate: 3/4", &inventory());
        assert_eq!(
            actual,
            GateOutcome::FailClosed(FailClosedReason::UnknownCheckId("ZZ-99".into()))
        );
        assert_eq!(actual.fails(), vec!["<unknown check id>"]);
    }

    #[test]
    fn parse_reconstructs_full_verdict_inventory() {
        let green = parse_gate_output("gate: 4/4", &inventory());
        assert_eq!(
            green,
            GateOutcome::Green {
                verdicts: verdicts(&[])
            }
        );
        assert_eq!(green.score(), (4, 4));
        assert!(green.fails().is_empty());

        let red = parse_gate_output(
            "FAIL TG-01 value\nFAIL TG-04 security\ngate: 2/4",
            &inventory(),
        );
        assert_eq!(
            red,
            GateOutcome::Red {
                verdicts: verdicts(&["TG-01", "TG-04"])
            }
        );
        assert_eq!(red.score(), (2, 4));
        assert_eq!(red.fails(), vec!["TG-01 value", "TG-04 security"]);
    }

    #[test]
    fn summary_inventory_mismatch_fails_closed() {
        assert_eq!(
            parse_gate_output("gate: 4/5", &inventory()),
            GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                passed: 4,
                total: 5
            })
        );
        assert_eq!(
            parse_gate_output("FAIL TG-01 value\ngate: 4/4", &inventory()),
            GateOutcome::FailClosed(FailClosedReason::InconsistentSummary {
                passed: 4,
                total: 4
            })
        );
        assert_eq!(
            parse_gate_output("gate: 0/0", &[]),
            GateOutcome::FailClosed(FailClosedReason::InvalidInventory)
        );
    }

    #[test]
    fn gate_execution_fixture() {
        let Ok(mode) = std::env::var("NANO_VERIFY_UNIT_GATE_MODE") else {
            return;
        };
        let artifact = std::env::args_os().next_back().unwrap();
        match mode.as_str() {
            "mutate" => std::fs::write(artifact, b"mutated by gate\n").unwrap(),
            "sleep" => std::thread::sleep(std::time::Duration::from_secs(30)),
            _ => panic!("unknown unit gate mode"),
        }
        println!("gate: 4/4");
    }

    fn fixture_invocation(mode: &str) -> GateInvocation {
        GateInvocation {
            argv: vec![
                std::env::current_exe().unwrap().into_os_string(),
                "--exact".into(),
                "gate::tests::gate_execution_fixture".into(),
                "--nocapture".into(),
            ],
            cwd: std::env::current_dir().unwrap(),
            env: vec![("NANO_VERIFY_UNIT_GATE_MODE".into(), mode.into())],
            timeout: std::time::Duration::from_secs(5),
            gate_id: "unit-fixture".into(),
        }
    }

    #[tokio::test]
    async fn post_execution_artifact_mutation_fails_closed() {
        let workspace = create_artifact_workspace().unwrap();
        let artifact = create_candidate_artifact(
            &workspace,
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        let execution =
            run_gate_execution(&fixture_invocation("mutate"), &artifact, &inventory()).await;
        assert!(matches!(
            execution.outcome,
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::ArtifactInvalid)
        ));
        assert_eq!(execution.evidence.exit_code, None);
        assert_eq!(execution.evidence.log_digest, None);
    }

    #[tokio::test]
    async fn cancellation_poll_is_ten_ms_and_teardown_is_bounded() {
        let workspace = create_artifact_workspace().unwrap();
        let artifact = create_candidate_artifact(
            &workspace,
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        let cancellation = GateCancellation::new();
        let trigger = cancellation.clone();
        let trigger = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let signalled = std::time::Instant::now();
            trigger.cancel();
            signalled
        });
        let execution = run_gate_execution_with_cancellation(
            &fixture_invocation("sleep"),
            &artifact,
            &inventory(),
            Some(&cancellation),
        )
        .await;
        let elapsed_after_signal = trigger.await.unwrap().elapsed();
        assert!(matches!(
            execution.outcome,
            ExecutionGateOutcome::FailClosed(ExecutionFailClosedReason::Cancelled)
        ));
        assert!(
            elapsed_after_signal <= std::time::Duration::from_millis(150),
            "cancelled process-tree teardown took {elapsed_after_signal:?}"
        );
    }
}
