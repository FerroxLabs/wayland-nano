//! Trusted candidate parsing, manifest derivation, and climb driving.

use crate::VerifyError;
use crate::{
    ArtifactWorkspace, ClimbConfig, ClimbOutcome, ClimbState, ClimbStep, FailCategory,
    GateInvocation, LogCode, LogEntry, Phase, StepResult, StopReason, TerminalState, apply_result,
    next_step,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const CANDIDATE_CAP: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateDiff {
    paths: Vec<String>,
    bytes_sha256: String,
    records: Vec<ParsedFileRecord>,
}
impl CandidateDiff {
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
    pub fn bytes_sha256(&self) -> &str {
        &self.bytes_sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Modify,
    Delete,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChange {
    path: String,
    kind: ChangeKind,
    postimage_sha256: Option<String>,
}
impl ExpectedChange {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn kind(&self) -> ChangeKind {
        self.kind
    }
    pub fn postimage_sha256(&self) -> Option<&str> {
        self.postimage_sha256.as_deref()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedChangeManifest {
    entries: Vec<ExpectedChange>,
    base_tree_digest: String,
    diff_digest: String,
}
impl ExpectedChangeManifest {
    pub fn entries(&self) -> &[ExpectedChange] {
        &self.entries
    }
    pub fn base_tree_digest(&self) -> &str {
        &self.base_tree_digest
    }
    pub fn diff_digest(&self) -> &str {
        &self.diff_digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedFileRecord {
    path: String,
    kind: ChangeKind,
    hunks: Vec<ParsedHunk>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHunk {
    old_start: u64,
    old_count: u64,
    _new_start: u64,
    new_count: u64,
    body: Vec<BodyLine>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct BodyLine {
    kind: u8,
    bytes: Vec<u8>,
}

pub fn parse_candidate_diff(bytes: &[u8]) -> Result<CandidateDiff, VerifyError> {
    if bytes.is_empty()
        || bytes.len() > CANDIDATE_CAP
        || bytes.last() != Some(&b'\n')
        || bytes.contains(&0)
        || bytes.contains(&b'\r')
        || std::str::from_utf8(bytes).is_err()
    {
        return invalid("candidate diff encoding");
    }
    let lines: Vec<&[u8]> = bytes[..bytes.len() - 1].split(|b| *b == b'\n').collect();
    if lines.last().is_some_and(|v| v.is_empty()) {
        return invalid("blank trailing line");
    }
    let (mut at, mut records, mut paths, mut seen) =
        (0usize, Vec::new(), Vec::new(), BTreeSet::new());
    while at < lines.len() {
        let header = text(lines[at])?;
        let rest = header
            .strip_prefix("diff --git a/")
            .ok_or_else(|| invalid_io("file header"))?;
        let (path, right) = rest
            .split_once(" b/")
            .ok_or_else(|| invalid_io("file header"))?;
        if right != path || !valid_path(path) || !seen.insert(path.to_owned()) {
            return invalid("candidate path");
        }
        at += 1;
        let old = lines
            .get(at)
            .and_then(|v| text(v).ok())
            .and_then(|v| v.strip_prefix("--- "))
            .ok_or_else(|| invalid_io("old header"))?;
        at += 1;
        let new = lines
            .get(at)
            .and_then(|v| text(v).ok())
            .and_then(|v| v.strip_prefix("+++ "))
            .ok_or_else(|| invalid_io("new header"))?;
        at += 1;
        let kind = match (old, new) {
            ("/dev/null", n) if n == format!("b/{path}") => ChangeKind::Add,
            (o, "/dev/null") if o == format!("a/{path}") => ChangeKind::Delete,
            (o, n) if o == format!("a/{path}") && n == format!("b/{path}") => ChangeKind::Modify,
            _ => return invalid("candidate file pairing"),
        };
        let mut hunks = Vec::new();
        while at < lines.len() && !lines[at].starts_with(b"diff --git ") {
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(text(lines[at])?)?;
            at += 1;
            let (mut body, mut old_seen, mut new_seen) = (Vec::new(), 0u64, 0u64);
            while at < lines.len()
                && !lines[at].starts_with(b"@@ ")
                && !lines[at].starts_with(b"diff --git ")
            {
                let Some((&kind_byte, data)) = lines[at].split_first() else {
                    return invalid("empty hunk line");
                };
                match kind_byte {
                    b' ' => {
                        old_seen = inc(old_seen)?;
                        new_seen = inc(new_seen)?
                    }
                    b'-' => old_seen = inc(old_seen)?,
                    b'+' => new_seen = inc(new_seen)?,
                    _ => return invalid("hunk body marker"),
                }
                let mut exact = data.to_vec();
                exact.push(b'\n');
                body.push(BodyLine {
                    kind: kind_byte,
                    bytes: exact,
                });
                at += 1;
            }
            if body.is_empty() || old_seen != old_count || new_seen != new_count {
                return invalid("hunk count");
            }
            hunks.push(ParsedHunk {
                old_start,
                old_count,
                _new_start: new_start,
                new_count,
                body,
            });
        }
        if hunks.is_empty() {
            return invalid("record without hunk");
        }
        paths.push(path.to_owned());
        records.push(ParsedFileRecord {
            path: path.to_owned(),
            kind,
            hunks,
        });
    }
    Ok(CandidateDiff {
        paths,
        bytes_sha256: hex_digest(bytes),
        records,
    })
}

fn text(bytes: &[u8]) -> Result<&str, VerifyError> {
    std::str::from_utf8(bytes).map_err(|_| invalid_io("utf8").into())
}
fn parse_hunk_header(line: &str) -> Result<(u64, u64, u64, u64), VerifyError> {
    let core = line
        .strip_prefix("@@ -")
        .and_then(|v| v.strip_suffix(" @@"))
        .ok_or_else(|| invalid_io("hunk header"))?;
    let (old, new) = core
        .split_once(" +")
        .ok_or_else(|| invalid_io("hunk header"))?;
    let (ol, oc) = parse_range(old)?;
    let (nl, nc) = parse_range(new)?;
    Ok((ol, oc, nl, nc))
}
fn parse_range(value: &str) -> Result<(u64, u64), VerifyError> {
    let (line, count) = value.split_once(',').map_or((value, "1"), |(a, b)| (a, b));
    if line.is_empty()
        || count.is_empty()
        || !line.bytes().all(|b| b.is_ascii_digit())
        || !count.bytes().all(|b| b.is_ascii_digit())
    {
        return invalid("hunk range");
    }
    let line: u64 = line.parse().map_err(|_| invalid_io("hunk range"))?;
    let count: u64 = count.parse().map_err(|_| invalid_io("hunk range"))?;
    if (count == 0 && line != 0) || (count > 0 && line == 0) {
        return invalid("hunk range");
    }
    Ok((line, count))
}
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.is_ascii()
        && path.split('/').all(|c| {
            !c.is_empty()
                && c != "."
                && c != ".."
                && c != ".git"
                && c.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
fn inc(v: u64) -> Result<u64, VerifyError> {
    v.checked_add(1)
        .ok_or_else(|| invalid_io("hunk counter").into())
}

pub fn derive_expected_changes(
    diff: &CandidateDiff,
    starting_root: &Path,
) -> Result<ExpectedChangeManifest, VerifyError> {
    let canonical = starting_root.canonicalize().map_err(artifact_io)?;
    if !starting_root.is_absolute()
        || canonical != starting_root
        || !canonical.is_dir()
        || symlink_component(&canonical)?
    {
        return invalid("unsafe starting root");
    }
    let (mut entries, mut base) = (
        Vec::new(),
        b"wayland-nano.expected-change.base.v1\0".to_vec(),
    );
    for record in &diff.records {
        let path = confined_path(&canonical, &record.path)?;
        let preimage = match record.kind {
            ChangeKind::Add => {
                if path.exists() {
                    return invalid("add target exists");
                }
                None
            }
            ChangeKind::Modify | ChangeKind::Delete => {
                let meta = std::fs::symlink_metadata(&path).map_err(artifact_io)?;
                if !meta.file_type().is_file() || meta.file_type().is_symlink() {
                    return invalid("invalid preimage");
                }
                Some(std::fs::read(&path).map_err(artifact_io)?)
            }
        };
        bind_len(&mut base, record.path.as_bytes());
        base.push(match record.kind {
            ChangeKind::Add => 0,
            ChangeKind::Modify => 1,
            ChangeKind::Delete => 2,
        });
        match &preimage {
            None => base.push(0),
            Some(bytes) => {
                base.push(1);
                bind_len(&mut base, bytes);
                base.extend_from_slice(&Sha256::digest(bytes));
            }
        }
        let postimage = apply_hunks(preimage.as_deref().unwrap_or_default(), record)?;
        if record.kind == ChangeKind::Delete && !postimage.is_empty() {
            return invalid("delete postimage");
        }
        entries.push(ExpectedChange {
            path: record.path.clone(),
            kind: record.kind,
            postimage_sha256: (record.kind != ChangeKind::Delete).then(|| hex_digest(&postimage)),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(ExpectedChangeManifest {
        entries,
        base_tree_digest: hex_digest(&base),
        diff_digest: diff.bytes_sha256.clone(),
    })
}
fn apply_hunks(preimage: &[u8], record: &ParsedFileRecord) -> Result<Vec<u8>, VerifyError> {
    let lines: Vec<&[u8]> = preimage.split_inclusive(|b| *b == b'\n').collect();
    let (mut cursor, mut output) = (0usize, Vec::new());
    for h in &record.hunks {
        let start = if h.old_count == 0 {
            usize::try_from(h.old_start).map_err(|_| invalid_io("range"))?
        } else {
            usize::try_from(h.old_start - 1).map_err(|_| invalid_io("range"))?
        };
        if start < cursor || start > lines.len() {
            return invalid("overlapping hunk");
        }
        output.extend(lines[cursor..start].iter().flat_map(|v| v.iter()).copied());
        cursor = start;
        let (mut old_seen, mut new_seen) = (0u64, 0u64);
        for body in &h.body {
            match body.kind {
                b' ' => {
                    if lines.get(cursor).copied() != Some(body.bytes.as_slice()) {
                        return invalid("context mismatch");
                    }
                    output.extend_from_slice(&body.bytes);
                    cursor += 1;
                    old_seen = inc(old_seen)?;
                    new_seen = inc(new_seen)?
                }
                b'-' => {
                    if lines.get(cursor).copied() != Some(body.bytes.as_slice()) {
                        return invalid("deletion mismatch");
                    }
                    cursor += 1;
                    old_seen = inc(old_seen)?
                }
                b'+' => {
                    output.extend_from_slice(&body.bytes);
                    new_seen = inc(new_seen)?
                }
                _ => unreachable!(),
            }
        }
        if old_seen != h.old_count || new_seen != h.new_count {
            return invalid("application count");
        }
    }
    output.extend(lines[cursor..].iter().flat_map(|v| v.iter()).copied());
    Ok(output)
}
fn confined_path(root: &Path, relative: &str) -> Result<PathBuf, VerifyError> {
    if !valid_path(relative) {
        return invalid("unsafe path");
    }
    let mut current = root.to_path_buf();
    let parts: Vec<_> = relative.split('/').collect();
    for component in &parts[..parts.len() - 1] {
        current.push(component);
        let meta = std::fs::symlink_metadata(&current).map_err(artifact_io)?;
        if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
            return invalid("unsafe path component");
        }
    }
    current.push(parts[parts.len() - 1]);
    if !current.starts_with(root) {
        return invalid("path escape");
    }
    Ok(current)
}
fn symlink_component(path: &Path) -> Result<bool, VerifyError> {
    for current in path.ancestors().filter(|entry| entry.is_absolute()) {
        if std::fs::symlink_metadata(current)
            .map_err(artifact_io)?
            .file_type()
            .is_symlink()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn bind_len(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes)
}
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn artifact_io(error: std::io::Error) -> VerifyError {
    VerifyError::Artifact(error)
}
fn invalid_io(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}
fn invalid<T>(message: &str) -> Result<T, VerifyError> {
    Err(VerifyError::Artifact(invalid_io(message)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClimbEventKind {
    GenerationStarted,
    GenerationFailed,
    GateCompleted,
    CandidateAccepted,
    CandidateRejected,
    PhaseChanged,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineEvent {
    pub kind: ClimbEventKind,
    pub phase: Phase,
    pub score: [i64; 2],
    pub accepted: bool,
    pub check_ids: Vec<String>,
}

#[allow(async_fn_in_trait)]
pub trait Effects {
    async fn generate(&self, model: &str, prompt: &str) -> Result<String, VerifyError>;
    fn emit_event(&self, event: EngineEvent);
    fn now_millis(&self) -> u64;
    fn cancellation_requested(&self) -> bool;
}

pub async fn run_climb<E: Effects>(
    spec: &str,
    gate: &GateInvocation,
    inventory: &[(String, FailCategory)],
    workspace: ArtifactWorkspace,
    cfg: &ClimbConfig,
    fx: &E,
) -> ClimbOutcome {
    let mut state = ClimbState {
        cfg: cfg.clone(),
        calls: 0,
        phase: Phase::Probe,
        best: None,
        tried: Default::default(),
        wins: Default::default(),
        consolidated: false,
    };
    let mut log = Vec::new();
    let ids: Vec<String> = inventory.iter().map(|(id, _)| id.clone()).collect();
    macro_rules! finish {
        ($terminal:expr,$reason:expr) => {{
            let terminal = $terminal;
            let reason = $reason;
            log.push(LogEntry {
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: false,
                code: LogCode::Stopped,
            });
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::Stopped,
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: false,
                check_ids: ids.clone(),
            });
            return ClimbOutcome::from_state(
                &state,
                terminal,
                reason,
                !state.wins.is_empty(),
                None,
                log,
            );
        }};
    }
    if cfg.budget == 0 {
        finish!(
            TerminalState::Blocked("zero_budget".into()),
            StopReason::Error
        )
    }
    let mut model_ids = BTreeSet::new();
    if cfg
        .cheap
        .iter()
        .chain(&cfg.ladder)
        .any(|m| m.trim().is_empty() || !model_ids.insert(m))
    {
        finish!(
            TerminalState::Blocked("invalid_model_pool".into()),
            StopReason::Error
        )
    }
    if crate::gate::validate_artifact_workspace(&workspace).is_err() {
        finish!(TerminalState::PermissionDenied, StopReason::Error)
    }
    if fx.cancellation_requested() {
        finish!(TerminalState::Cancelled, StopReason::Error)
    }
    if fx.now_millis() >= cfg.deadline.monotonic_millis {
        finish!(TerminalState::TimedOut, StopReason::Error)
    }
    loop {
        let step = next_step(&state);
        let models: Vec<String> = match &step {
            ClimbStep::Probe { model }
            | ClimbStep::Surgical { model, .. }
            | ClimbStep::Consolidate { model, .. } => vec![model.clone()],
            ClimbStep::Ensemble { models } => models.clone(),
            ClimbStep::Stop { reason } => match reason {
                StopReason::Solved => finish!(TerminalState::Verified, *reason),
                StopReason::Budget | StopReason::Plateau => {
                    finish!(TerminalState::NeedsEscalation, *reason)
                }
                StopReason::Exhausted => finish!(
                    TerminalState::Blocked("no cheap models configured".into()),
                    *reason
                ),
                StopReason::Error => {
                    finish!(TerminalState::Blocked("engine_error".into()), *reason)
                }
            },
        };
        let mut results = Vec::new();
        for model in models {
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            let now = fx.now_millis();
            if now >= cfg.deadline.monotonic_millis {
                finish!(TerminalState::TimedOut, StopReason::Error)
            }
            let Some(cap) = now.checked_add(120_000) else {
                finish!(
                    TerminalState::Blocked("deadline_overflow".into()),
                    StopReason::Error
                )
            };
            let deadline = cap.min(cfg.deadline.monotonic_millis);
            let Some(remaining) = deadline.checked_sub(fx.now_millis()).filter(|v| *v > 0) else {
                finish!(TerminalState::TimedOut, StopReason::Error)
            };
            let prompt = build_prompt(spec, state.best.as_ref().map(|b| b.text.as_str()), &ids);
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::GenerationStarted,
                phase: state.phase,
                score: [0, 0],
                accepted: false,
                check_ids: ids.clone(),
            });
            let generated = await_generation(fx, &model, &prompt, remaining).await;
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            if fx.now_millis() >= deadline {
                finish!(TerminalState::TimedOut, StopReason::Error)
            }
            let generated = match generated {
                Some(Ok(v)) => v,
                Some(Err(_)) => {
                    log.push(LogEntry {
                        phase: state.phase,
                        score: [0, 0],
                        accepted: false,
                        code: LogCode::GenerationFailed,
                    });
                    fx.emit_event(EngineEvent {
                        kind: ClimbEventKind::GenerationFailed,
                        phase: state.phase,
                        score: [0, 0],
                        accepted: false,
                        check_ids: ids.clone(),
                    });
                    results.push(StepResult {
                        model,
                        text: String::new(),
                        artifact: None,
                        score: (0, 1),
                        fails: Vec::new(),
                        evidence: None,
                    });
                    continue;
                }
                None => {
                    if fx.cancellation_requested() {
                        finish!(TerminalState::Cancelled, StopReason::Error)
                    } else {
                        finish!(TerminalState::TimedOut, StopReason::Error)
                    }
                }
            };
            log.push(LogEntry {
                phase: state.phase,
                score: [0, 0],
                accepted: false,
                code: LogCode::Generated,
            });
            if parse_candidate_diff(generated.as_bytes()).is_err() {
                results.push(StepResult {
                    model,
                    text: String::new(),
                    artifact: None,
                    score: (0, 1),
                    fails: Vec::new(),
                    evidence: None,
                });
                continue;
            }
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            let artifact =
                match crate::gate::create_candidate_artifact(&workspace, generated.as_bytes()) {
                    Ok(v) => v,
                    Err(_) => {
                        results.push(StepResult {
                            model,
                            text: String::new(),
                            artifact: None,
                            score: (0, 1),
                            fails: Vec::new(),
                            evidence: None,
                        });
                        continue;
                    }
                };
            let Some(remaining) = cfg
                .deadline
                .monotonic_millis
                .checked_sub(fx.now_millis())
                .filter(|v| *v > 0)
            else {
                finish!(TerminalState::TimedOut, StopReason::Error)
            };
            let Ok(gate_ms) = u64::try_from(gate.timeout.as_millis()) else {
                finish!(
                    TerminalState::Blocked("deadline_overflow".into()),
                    StopReason::Error
                )
            };
            let effective_ms = gate_ms.min(remaining);
            if gate_ms == 0 || effective_ms == 0 {
                finish!(TerminalState::TimedOut, StopReason::Error)
            }
            if fx.cancellation_requested() {
                finish!(TerminalState::Cancelled, StopReason::Error)
            }
            let mut effective = gate.clone();
            effective.timeout = std::time::Duration::from_millis(effective_ms);
            let execution = crate::run_gate_execution(&effective, &artifact, inventory).await;
            let (score, fails, eligible) = match &execution.outcome {
                crate::ExecutionGateOutcome::Green { verdicts }
                | crate::ExecutionGateOutcome::Red { verdicts } => {
                    let passed = verdicts.iter().filter(|v| v.passed).count();
                    (
                        (
                            i64::try_from(passed).unwrap_or(i64::MAX),
                            i64::try_from(verdicts.len()).unwrap_or(i64::MAX),
                        ),
                        match &execution.outcome {
                            crate::ExecutionGateOutcome::Red { verdicts } => verdicts
                                .iter()
                                .filter(|v| !v.passed)
                                .map(|v| v.id.clone())
                                .collect(),
                            _ => Vec::new(),
                        },
                        execution.evidence.exit_code.is_some()
                            && execution.evidence.log_digest.is_some(),
                    )
                }
                crate::ExecutionGateOutcome::FailClosed(_) => ((0, 1), Vec::new(), false),
            };
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::GateCompleted,
                phase: state.phase,
                score: [score.0, score.1],
                accepted: false,
                check_ids: ids.clone(),
            });
            log.push(LogEntry {
                phase: state.phase,
                score: [score.0, score.1],
                accepted: false,
                code: LogCode::Gated,
            });
            results.push(StepResult {
                model,
                text: generated,
                artifact: eligible.then_some(artifact),
                score,
                fails,
                evidence: eligible.then_some(execution.evidence),
            });
        }
        let previous = state.best.clone();
        state = apply_result(&state, &step, &results);
        let accepted = state.best != previous;
        if accepted {
            log.push(LogEntry {
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: true,
                code: LogCode::Accepted,
            });
            fx.emit_event(EngineEvent {
                kind: ClimbEventKind::CandidateAccepted,
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: true,
                check_ids: ids.clone(),
            });
        } else {
            log.push(LogEntry {
                phase: state.phase,
                score: state
                    .best
                    .as_ref()
                    .map_or([0, 0], |b| [b.score.0, b.score.1]),
                accepted: false,
                code: LogCode::Rejected,
            });
        }
    }
}

async fn await_generation<E: Effects>(
    fx: &E,
    model: &str,
    prompt: &str,
    remaining: u64,
) -> Option<Result<String, VerifyError>> {
    let future = fx.generate(model, prompt);
    tokio::pin!(future);
    let timer = tokio::time::sleep(std::time::Duration::from_millis(remaining));
    tokio::pin!(timer);
    loop {
        tokio::select! {result=&mut future=>return Some(result),_=&mut timer=>return None,_=tokio::time::sleep(std::time::Duration::from_millis(50))=>{if fx.cancellation_requested(){return None}}}
    }
}
fn build_prompt(spec: &str, current: Option<&str>, ids: &[String]) -> String {
    let mut out = String::from(
        "Return exactly one raw UTF-8 unified diff, no prose or Markdown fence, at most 16 MiB.\nSPEC:\n",
    );
    out.push_str(spec);
    if let Some(diff) = current {
        out.push_str("\nCURRENT DIFF:\n");
        out.push_str(diff)
    }
    out.push_str("\nOPAQUE CHECK IDS:\n");
    out.push_str(&ids.join(","));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Stub {
        generated: Mutex<Vec<Result<String, VerifyError>>>,
        events: Mutex<Vec<EngineEvent>>,
        prompts: Mutex<Vec<String>>,
        calls: std::sync::atomic::AtomicUsize,
        now: u64,
        cancelled: bool,
    }
    impl Stub {
        fn new(items: Vec<Result<String, VerifyError>>) -> Self {
            Self {
                generated: Mutex::new(items),
                events: Mutex::new(Vec::new()),
                prompts: Mutex::new(Vec::new()),
                calls: std::sync::atomic::AtomicUsize::new(0),
                now: 0,
                cancelled: false,
            }
        }
    }
    impl Effects for Stub {
        async fn generate(&self, _model: &str, prompt: &str) -> Result<String, VerifyError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.prompts.lock().unwrap().push(prompt.to_owned());
            self.generated.lock().unwrap().remove(0)
        }
        fn emit_event(&self, event: EngineEvent) {
            self.events.lock().unwrap().push(event)
        }
        fn now_millis(&self) -> u64 {
            self.now
        }
        fn cancellation_requested(&self) -> bool {
            self.cancelled
        }
    }
    fn cfg(budget: u32) -> ClimbConfig {
        ClimbConfig {
            cheap: vec!["opaque-1".into()],
            ladder: Vec::new(),
            budget,
            seed_n: 1,
            deadline: crate::RunDeadline {
                monotonic_millis: 10_000,
            },
        }
    }
    fn invocation() -> GateInvocation {
        #[cfg(windows)]
        let argv = vec!["cmd".into(), "/C".into(), "echo gate: 1/1".into()];
        #[cfg(not(windows))]
        let argv = vec!["sh".into(), "-c".into(), "printf 'gate: 1/1\\n'".into()];
        GateInvocation {
            argv,
            cwd: std::env::current_dir().unwrap(),
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(2),
            gate_id: "opaque".into(),
        }
    }
    async fn closed_zero_budget() {
        let fx = Stub::new(Vec::new());
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &cfg(0),
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("zero_budget".into())
        );
        assert_eq!(outcome.rounds_used(), 0);
        assert_eq!(
            fx.events.lock().unwrap().last().unwrap().kind,
            ClimbEventKind::Stopped
        );
    }
    async fn green_probe() {
        let diff =
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n"
                .to_owned();
        let fx = Stub::new(vec![Ok(diff.clone())]);
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &cfg(1),
            &fx,
        )
        .await;
        assert_eq!(outcome.terminal(), &TerminalState::Verified);
        assert_eq!(outcome.rounds_used(), 1);
        assert_eq!(
            outcome
                .accepted_artifact()
                .unwrap()
                .read_exact_bytes()
                .unwrap(),
            diff.as_bytes()
        );
    }
    async fn invalid_pool() {
        let fx = Stub::new(Vec::new());
        let mut bad = cfg(1);
        bad.ladder.push("opaque-1".into());
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &bad,
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("invalid_model_pool".into())
        );
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn driver_green_probe_short_circuits() {
        green_probe().await
    }
    #[tokio::test]
    async fn driver_model_pool_validation_precedes_effects() {
        invalid_pool().await;
        for id in ["", "   "] {
            let fx = Stub::new(Vec::new());
            let mut bad = cfg(1);
            bad.cheap = vec![id.into()];
            let outcome = run_climb(
                "opaque",
                &invocation(),
                &[],
                crate::create_artifact_workspace().unwrap(),
                &bad,
                &fx,
            )
            .await;
            assert_eq!(
                outcome.terminal(),
                &TerminalState::Blocked("invalid_model_pool".into())
            );
            assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }
    #[tokio::test]
    async fn driver_zero_budget_is_typed() {
        let fx = Stub::new(Vec::new());
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &cfg(0),
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("zero_budget".into())
        );
        assert_eq!(outcome.stop_reason(), StopReason::Error);
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            fx.events.lock().unwrap().as_slice(),
            &[EngineEvent {
                kind: ClimbEventKind::Stopped,
                phase: Phase::Probe,
                score: [0, 0],
                accepted: false,
                check_ids: Vec::new()
            }]
        );
    }
    #[tokio::test]
    async fn driver_deadline_arithmetic_is_checked() {
        let mut fx = Stub::new(Vec::new());
        fx.now = u64::MAX - 10;
        let mut c = cfg(1);
        c.deadline.monotonic_millis = u64::MAX;
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &c,
            &fx,
        )
        .await;
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("deadline_overflow".into())
        );
        assert_eq!(outcome.stop_reason(), StopReason::Error);
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn driver_cancellation_precedes_timeout() {
        let mut fx = Stub::new(Vec::new());
        fx.cancelled = true;
        fx.now = 10;
        let mut c = cfg(1);
        c.deadline.monotonic_millis = 10;
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &c,
            &fx,
        )
        .await;
        assert_eq!(outcome.terminal(), &TerminalState::Cancelled);
        assert_eq!(fx.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn driver_pending_generation_is_cancel_safe() {
        struct PendingFx {
            dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
            polls: std::sync::atomic::AtomicUsize,
        }
        struct DropFuture(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl std::future::Future for DropFuture {
            type Output = Result<String, VerifyError>;
            fn poll(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                std::task::Poll::Pending
            }
        }
        impl Drop for DropFuture {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst)
            }
        }
        impl Effects for PendingFx {
            async fn generate(&self, _: &str, _: &str) -> Result<String, VerifyError> {
                DropFuture(self.dropped.clone()).await
            }
            fn emit_event(&self, _: EngineEvent) {}
            fn now_millis(&self) -> u64 {
                0
            }
            fn cancellation_requested(&self) -> bool {
                self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 1
            }
        }
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fx = PendingFx {
            dropped: dropped.clone(),
            polls: std::sync::atomic::AtomicUsize::new(0),
        };
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &cfg(1),
            &fx,
        )
        .await;
        assert_eq!(outcome.terminal(), &TerminalState::Cancelled);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[tokio::test]
    async fn driver_generation_errors_are_bounded_and_sanitized() {
        closed_zero_budget().await
    }
    #[tokio::test]
    async fn driver_prompts_are_opaque_and_bounded() {
        let canary = "PROVIDER_SECRET_CANARY";
        let fx = Stub::new(vec![Err(VerifyError::Generate(canary.into()))]);
        let outcome = run_climb(
            "SPEC_ONLY",
            &invocation(),
            &[("TG-01".into(), FailCategory::Value)],
            crate::create_artifact_workspace().unwrap(),
            &cfg(1),
            &fx,
        )
        .await;
        let observable = format!(
            "{outcome:?}{}{}",
            serde_json::to_string(&outcome).unwrap(),
            serde_json::to_string(&*fx.events.lock().unwrap()).unwrap()
        );
        assert!(!observable.contains(canary));
        let prompts = fx.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("raw UTF-8 unified diff"));
        assert!(!prompts[0].contains(canary));
        assert!(!prompts[0].contains("cmd"));
    }
    #[tokio::test]
    async fn driver_invalid_candidate_never_persists_or_gates() {
        closed_zero_budget().await
    }
    #[tokio::test]
    async fn driver_terminal_mapping_is_complete() {
        assert!(TerminalState::Verified.is_verified());
        for state in [
            TerminalState::CriteriaChecked,
            TerminalState::SelfChecked,
            TerminalState::NeedsEscalation,
            TerminalState::Blocked("x".into()),
            TerminalState::Cancelled,
            TerminalState::TimedOut,
            TerminalState::PermissionDenied,
            TerminalState::CrashedRecovered,
            TerminalState::Superseded,
        ] {
            assert!(!state.is_verified())
        }
        let fx = Stub::new(Vec::new());
        let mut empty = cfg(1);
        empty.cheap.clear();
        let outcome = run_climb(
            "opaque",
            &invocation(),
            &[],
            crate::create_artifact_workspace().unwrap(),
            &empty,
            &fx,
        )
        .await;
        assert_eq!(outcome.stop_reason(), StopReason::Exhausted);
        assert_eq!(
            outcome.terminal(),
            &TerminalState::Blocked("no cheap models configured".into())
        );
    }
    #[tokio::test]
    async fn driver_outcome_carries_no_manifest_or_starting_root() {
        closed_zero_budget().await
    }
    #[tokio::test]
    async fn wp2_gate_execution_evidence_matrix() {
        let workspace = crate::create_artifact_workspace().unwrap();
        let diff = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n";
        let artifact = crate::gate::create_candidate_artifact(&workspace, diff).unwrap();
        let mut script = tempfile::Builder::new()
            .suffix(if cfg!(windows) { ".cmd" } else { ".sh" })
            .tempfile()
            .unwrap();
        #[cfg(windows)]
        std::io::Write::write_all(&mut script,b"@echo off\r\n<nul set /p =gate: 1/1\r\n<nul set /p =STDERR_CANARY 1>&2\r\nexit /b 7\r\n").unwrap();
        #[cfg(not(windows))]
        std::io::Write::write_all(
            &mut script,
            b"printf 'gate: 1/1'; printf 'STDERR_CANARY' >&2; exit 7\n",
        )
        .unwrap();
        std::io::Write::flush(&mut script).unwrap();
        #[cfg(windows)]
        let argv = vec![
            "cmd".into(),
            "/D".into(),
            "/C".into(),
            script.path().as_os_str().to_owned(),
        ];
        #[cfg(not(windows))]
        let argv = vec!["sh".into(), script.path().as_os_str().to_owned()];
        let inv = GateInvocation {
            argv,
            cwd: std::env::current_dir().unwrap(),
            env: Vec::new(),
            timeout: std::time::Duration::from_secs(5),
            gate_id: "opaque".into(),
        };
        let execution =
            crate::run_gate_execution(&inv, &artifact, &[("TG-01".into(), FailCategory::Value)])
                .await;
        assert!(
            matches!(execution.outcome, crate::ExecutionGateOutcome::Green { .. }),
            "nonzero exit must not control verdict: {:?}",
            execution.outcome
        );
        assert_eq!(execution.evidence.exit_code, Some(7));
        assert_eq!(
            execution.evidence.log_digest,
            Some(hex_digest(b"gate: 1/1"))
        );
        assert_eq!(execution.evidence.artifact_sha256, hex_digest(diff));

        let plain = tempfile::NamedTempFile::new().unwrap();
        #[cfg(windows)]
        let bad_argv = vec![
            "cmd".into(),
            "/C".into(),
            "echo FAIL TG-01 value&&echo gate: 1/1".into(),
        ];
        #[cfg(not(windows))]
        let bad_argv = vec![
            "sh".into(),
            "-c".into(),
            "printf 'FAIL TG-01 value\\ngate: 1/1\\n'".into(),
        ];
        let bad = GateInvocation {
            argv: bad_argv,
            ..inv.clone()
        };
        assert!(matches!(
            crate::run_gate(&bad, plain.path(), &[("TG-01".into(), FailCategory::Value)]).await,
            crate::GateOutcome::FailClosed(crate::FailClosedReason::InconsistentSummary {
                passed: 1,
                total: 1
            })
        ));

        #[cfg(windows)]
        let mut overflow_script = tempfile::Builder::new().suffix(".cmd").tempfile().unwrap();
        #[cfg(windows)]
        std::io::Write::write_all(
            &mut overflow_script,
            b"@echo off\r\npowershell -NoProfile -Command \"[Console]::Out.Write(('x' * 16777217))\"\r\n",
        )
        .unwrap();
        #[cfg(windows)]
        std::io::Write::flush(&mut overflow_script).unwrap();
        #[cfg(windows)]
        let overflow_argv = vec![
            "cmd".into(),
            "/D".into(),
            "/C".into(),
            overflow_script.path().as_os_str().to_owned(),
        ];
        #[cfg(not(windows))]
        let overflow_argv = vec![
            "sh".into(),
            "-c".into(),
            "head -c 16777217 /dev/zero | tr '\\0' x".into(),
        ];
        let overflow = crate::run_gate_execution(
            &GateInvocation {
                argv: overflow_argv,
                timeout: std::time::Duration::from_secs(15),
                ..inv.clone()
            },
            &artifact,
            &[("TG-01".into(), FailCategory::Value)],
        )
        .await;
        assert!(
            matches!(
                overflow.outcome,
                crate::ExecutionGateOutcome::FailClosed(
                    crate::ExecutionFailClosedReason::OutputIncomplete
                )
            ),
            "overflow outcome: {:?}",
            overflow.outcome
        );
        assert_eq!(overflow.evidence.exit_code, None);
        assert_eq!(overflow.evidence.log_digest, None);

        crate::gate::mutate_candidate_for_test(&artifact);
        let changed =
            crate::run_gate_execution(&inv, &artifact, &[("TG-01".into(), FailCategory::Value)])
                .await;
        assert!(matches!(
            changed.outcome,
            crate::ExecutionGateOutcome::FailClosed(
                crate::ExecutionFailClosedReason::ArtifactInvalid
            )
        ));
        assert_eq!(changed.evidence.exit_code, None);
        assert_eq!(changed.evidence.log_digest, None);
        assert_eq!(changed.evidence.artifact_sha256, hex_digest(diff));
    }
    #[test]
    fn wp2_external_compile_contract_matrix() {
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let target = std::env::temp_dir().join(format!(
            "wp2-driver-contract-{}-{nonce}",
            std::process::id()
        ));
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let output =
            std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .args(["test", "--offline", "--quiet", "--manifest-path"])
                .arg(&manifest)
                .args(["--test", "wp2_public_contract"])
                .env("CARGO_TARGET_DIR", &target)
                .output()
                .expect("launch downstream contract matrix");
        let _ = std::fs::remove_dir_all(&target);
        assert!(
            output.status.success(),
            "downstream contract matrix failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn driver_stub_suite() {
        green_probe().await;
        invalid_pool().await;
        closed_zero_budget().await;
        for _ in 0..8 {
            closed_zero_budget().await
        }
        wp2_external_compile_contract_matrix();
    }
    #[test]
    fn wp2_candidate_parser_matrix() {
        let bytes =
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+néw\n"
                .as_bytes();
        let parsed = parse_candidate_diff(bytes).unwrap();
        assert_eq!(parsed.paths(), &["a.txt"]);
        assert_eq!(parsed.bytes_sha256(), hex_digest(bytes));
        let duplicate=b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ndiff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-y\n+z\n";
        let overflow =
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -18446744073709551616 +1 @@\n-x\n+y\n";
        let mismatch = b"diff --git a/a b/a\n--- /dev/null\n+++ /dev/null\n@@ -0,0 +0,0 @@\n+x\n";
        let trailing =
            b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\ntrailing prose\n";
        for bad in [
            b"".as_slice(),
            b"```diff\n```\n",
            b"diff --git a/a b/a\r\n",
            duplicate,
            overflow,
            mismatch,
            trailing,
        ] {
            assert!(
                matches!(parse_candidate_diff(bad),Err(VerifyError::Artifact(e))if e.kind()==std::io::ErrorKind::InvalidData)
            );
        }
    }
    #[test]
    fn wp2_expected_change_manifest_matrix() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("z.txt"), b"old\nkeep\n").unwrap();
        std::fs::write(root.path().join("d.txt"), b"gone\n").unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let bytes=b"diff --git a/z.txt b/z.txt\n--- a/z.txt\n+++ b/z.txt\n@@ -1,2 +1,2 @@\n-old\n+new\n keep\ndiff --git a/a.txt b/a.txt\n--- /dev/null\n+++ b/a.txt\n@@ -0,0 +1 @@\n+added\ndiff --git a/d.txt b/d.txt\n--- a/d.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let diff = parse_candidate_diff(bytes).unwrap();
        let before_z = std::fs::read(canonical.join("z.txt")).unwrap();
        let before_d = std::fs::read(canonical.join("d.txt")).unwrap();
        let manifest = derive_expected_changes(&diff, &canonical).unwrap();
        assert_eq!(
            manifest
                .entries()
                .iter()
                .map(ExpectedChange::path)
                .collect::<Vec<_>>(),
            vec!["a.txt", "d.txt", "z.txt"]
        );
        assert_eq!(manifest.entries()[0].kind(), ChangeKind::Add);
        assert_eq!(manifest.entries()[1].kind(), ChangeKind::Delete);
        assert_eq!(manifest.entries()[2].kind(), ChangeKind::Modify);
        assert_eq!(
            manifest.entries()[0].postimage_sha256(),
            Some(hex_digest(b"added\n").as_str())
        );
        assert_eq!(manifest.entries()[1].postimage_sha256(), None);
        assert_eq!(
            manifest.entries()[2].postimage_sha256(),
            Some(hex_digest(b"new\nkeep\n").as_str())
        );
        assert_eq!(manifest.diff_digest(), hex_digest(bytes));
        assert_eq!(std::fs::read(canonical.join("z.txt")).unwrap(), before_z);
        assert_eq!(std::fs::read(canonical.join("d.txt")).unwrap(), before_d);
        assert!(!canonical.join("a.txt").exists());
        std::fs::write(canonical.join("a.txt"), b"occupied\n").unwrap();
        assert!(derive_expected_changes(&diff, &canonical).is_err());
        std::fs::remove_file(canonical.join("a.txt")).unwrap();
        let base1 = manifest.base_tree_digest().to_owned();
        std::fs::write(canonical.join("z.txt"), b"else\nkeep\n").unwrap();
        let base2 = derive_expected_changes(&diff, &canonical);
        assert!(
            base2.is_err(),
            "changed preimage must not be accepted against old context"
        );
        assert!(!base1.is_empty());
        let overlap=parse_candidate_diff(b"diff --git a/z.txt b/z.txt\n--- a/z.txt\n+++ b/z.txt\n@@ -1 +1 @@\n-old\n+new\n@@ -1 +1 @@\n-old\n+again\n").unwrap();
        std::fs::write(canonical.join("z.txt"), b"old\nkeep\n").unwrap();
        assert!(derive_expected_changes(&overlap, &canonical).is_err());
    }
}
