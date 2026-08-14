//! Operator-configured lifecycle command hooks.
//!
//! Hooks execute unsandboxed with the operator's authority. Configuration is
//! therefore accepted only from a canonical, non-symlink regular file inside
//! `nano_home`. Tool policy never applies to hook commands.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const DEFAULT_TIMEOUT_SEC: u64 = 30;
const MAX_TIMEOUT_SEC: u64 = 600;
const SESSION_END_MAX_TIMEOUT_SEC: u64 = 3;
const MAX_REASON_BYTES: usize = 2048;
/// Hook stdout/stderr are hook-controlled: each pipe drains through a
/// capped buffer. Past the cap the drain keeps reading and discards — a
/// full pipe must never deadlock the child — and the hook fails with
/// `HookOutcome::BoundedOutput`, distinct from Timeout.
const MAX_HOOK_OUTPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    Stop,
    SessionStart,
    SessionEnd,
    PreCompact,
    PostCompact,
}

impl HookEvent {
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::PreToolUse | Self::UserPromptSubmit | Self::Stop)
    }

    fn uses_matcher(self) -> bool {
        !matches!(self, Self::UserPromptSubmit | Self::Stop)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    Pass,
    Blocked,
    Failed,
    Timeout,
    /// The hook emitted more than MAX_HOOK_OUTPUT_BYTES on a pipe; the
    /// output was drained-and-discarded past the cap and the hook failed.
    BoundedOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDecision {
    pub handler_id: String,
    pub matcher_input: Option<String>,
    pub outcome: HookOutcome,
    pub duration_ms: u64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HookRun {
    pub decisions: Vec<HookDecision>,
}

impl HookRun {
    pub fn blocking_reason(&self) -> Option<&str> {
        self.decisions.iter().find_map(|decision| {
            matches!(
                decision.outcome,
                HookOutcome::Blocked
                    | HookOutcome::Failed
                    | HookOutcome::Timeout
                    | HookOutcome::BoundedOutput
            )
            .then_some(
                decision
                    .reason
                    .as_deref()
                    .unwrap_or("hook blocked the operation"),
            )
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HookConfigError {
    #[error("hooks config rejected: {0}")]
    Invalid(String),
    #[error("hooks config io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
struct Handler {
    id: String,
    event: HookEvent,
    matcher: Option<Regex>,
    command: String,
    timeout: Duration,
    asynchronous: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HookEngine {
    handlers: Vec<Handler>,
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<MatcherGroup>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatcherGroup {
    matcher: Option<String>,
    hooks: Vec<HandlerConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandlerConfig {
    command: String,
    timeout_sec: Option<u64>,
    #[serde(default, rename = "async")]
    asynchronous: bool,
}

impl HookEngine {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Loads `<nano_home>/hooks.toml`, or the contained `NANO_HOOKS_FILE`
    /// override. Any defect rejects the complete file and returns zero hooks.
    pub fn load(nano_home: &Path) -> Self {
        match Self::load_strict(nano_home) {
            Ok(engine) => engine,
            Err(error) => Self {
                handlers: Vec::new(),
                warnings: vec![error.to_string()],
            },
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    fn load_strict(nano_home: &Path) -> Result<Self, HookConfigError> {
        let home = nano_home.canonicalize()?;
        let path = std::env::var_os("NANO_HOOKS_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("hooks.toml"));
        Self::load_path_strict(&home, &path)
    }

    fn load_path_strict(home: &Path, path: &Path) -> Result<Self, HookConfigError> {
        if !path.exists() {
            return Ok(Self::empty());
        }
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(HookConfigError::Invalid(
                "source must be a non-symlink regular file".into(),
            ));
        }
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(home) {
            return Err(HookConfigError::Invalid(
                "source escapes canonical nano_home".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let metadata = std::fs::metadata(&canonical)?;
            if metadata.uid() != unsafe { libc::geteuid() }
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(HookConfigError::Invalid(
                    "source must be owned by the operator with mode 0600".into(),
                ));
            }
        }
        let text = std::fs::read_to_string(&canonical)?;
        let config: ConfigFile =
            toml::from_str(&text).map_err(|error| HookConfigError::Invalid(error.to_string()))?;
        let mut handlers = Vec::new();
        for (event_name, groups) in config.hooks {
            let event = parse_event(&event_name)?;
            for (group_index, group) in groups.into_iter().enumerate() {
                let matcher = match group.matcher.as_deref() {
                    Some("*") | None => None,
                    Some(pattern) if event.uses_matcher() => {
                        Some(Regex::new(pattern).map_err(|error| {
                            HookConfigError::Invalid(format!(
                                "invalid {event_name} matcher: {error}"
                            ))
                        })?)
                    }
                    Some(_) => None,
                };
                for (handler_index, handler) in group.hooks.into_iter().enumerate() {
                    if handler.command.trim().is_empty() {
                        return Err(HookConfigError::Invalid("hook command is empty".into()));
                    }
                    if event.is_blocking() && handler.asynchronous {
                        return Err(HookConfigError::Invalid(format!(
                            "async is invalid for blocking event {event_name}"
                        )));
                    }
                    let timeout_sec = handler.timeout_sec.unwrap_or(DEFAULT_TIMEOUT_SEC);
                    if !(1..=MAX_TIMEOUT_SEC).contains(&timeout_sec) {
                        return Err(HookConfigError::Invalid(format!(
                            "timeout_sec for {event_name} must be in 1..=600"
                        )));
                    }
                    let timeout_sec = if event == HookEvent::SessionEnd {
                        timeout_sec.min(SESSION_END_MAX_TIMEOUT_SEC)
                    } else {
                        timeout_sec
                    };
                    handlers.push(Handler {
                        id: format!(
                            "hooks.toml:{}:{group_index}:{handler_index}",
                            event_key(event)
                        ),
                        event,
                        matcher: matcher.clone(),
                        command: handler.command,
                        timeout: Duration::from_secs(timeout_sec),
                        asynchronous: handler.asynchronous,
                    });
                }
            }
        }
        Ok(Self {
            handlers,
            warnings: Vec::new(),
        })
    }

    pub async fn run(
        &self,
        event: HookEvent,
        matcher_input: Option<&str>,
        payload: &Value,
    ) -> HookRun {
        let mut run = HookRun::default();
        for handler in self.handlers.iter().filter(|handler| {
            handler.event == event
                && (!event.uses_matcher()
                    || handler.matcher.as_ref().is_none_or(|matcher| {
                        matcher_input.is_some_and(|input| matcher.is_match(input))
                    }))
        }) {
            let decision = execute(handler, payload).await;
            if handler.asynchronous && decision.outcome != HookOutcome::Pass {
                tracing::warn!(handler_id = %decision.handler_id, outcome = ?decision.outcome, "async notify hook did not pass");
            }
            let should_stop = event.is_blocking() && decision.outcome != HookOutcome::Pass;
            run.decisions.push(HookDecision {
                matcher_input: matcher_input.map(str::to_owned),
                ..decision
            });
            if should_stop {
                break;
            }
        }
        run
    }
}

fn parse_event(name: &str) -> Result<HookEvent, HookConfigError> {
    match name {
        "PreToolUse" => Ok(HookEvent::PreToolUse),
        "PostToolUse" => Ok(HookEvent::PostToolUse),
        "UserPromptSubmit" => Ok(HookEvent::UserPromptSubmit),
        "Stop" => Ok(HookEvent::Stop),
        "SessionStart" => Ok(HookEvent::SessionStart),
        "SessionEnd" => Ok(HookEvent::SessionEnd),
        "PreCompact" => Ok(HookEvent::PreCompact),
        "PostCompact" => Ok(HookEvent::PostCompact),
        _ => Err(HookConfigError::Invalid(format!(
            "unknown hook event {name}"
        ))),
    }
}

fn event_key(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreToolUse => "pre_tool_use",
        HookEvent::PostToolUse => "post_tool_use",
        HookEvent::UserPromptSubmit => "user_prompt_submit",
        HookEvent::Stop => "stop",
        HookEvent::SessionStart => "session_start",
        HookEvent::SessionEnd => "session_end",
        HookEvent::PreCompact => "pre_compact",
        HookEvent::PostCompact => "post_compact",
    }
}

async fn execute(handler: &Handler, payload: &Value) -> HookDecision {
    let started = Instant::now();
    let result = execute_command(handler, payload).await;
    let (outcome, reason) = match result {
        Ok(output) if output.status.success() => parse_success(&output.stdout),
        Ok(output) if output.status.code() == Some(2) && handler.event.is_blocking() => (
            HookOutcome::Blocked,
            Some(bound_reason(&String::from_utf8_lossy(&output.stderr))),
        ),
        Ok(_) if handler.event.is_blocking() => {
            (HookOutcome::Failed, Some("hook command failed".into()))
        }
        Ok(_) => (HookOutcome::Failed, None),
        Err(RunError::Timeout) => (HookOutcome::Timeout, Some("hook command timed out".into())),
        Err(RunError::BoundedOutput) => (
            HookOutcome::BoundedOutput,
            Some("hook output exceeded bound".into()),
        ),
        Err(RunError::Spawn) => (
            HookOutcome::Failed,
            Some("hook command could not start".into()),
        ),
    };
    HookDecision {
        handler_id: handler.id.clone(),
        matcher_input: None,
        outcome,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        reason,
    }
}

fn parse_success(stdout: &[u8]) -> (HookOutcome, Option<String>) {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return (HookOutcome::Pass, None);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        decision: String,
        reason: String,
    }
    match serde_json::from_slice::<Output>(stdout) {
        Ok(output) if output.decision == "block" => {
            (HookOutcome::Blocked, Some(bound_reason(&output.reason)))
        }
        _ => (
            HookOutcome::Failed,
            Some("hook returned invalid output".into()),
        ),
    }
}

fn bound_reason(reason: &str) -> String {
    let reason = reason.trim();
    let mut end = reason.len().min(MAX_REASON_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_string()
}

enum RunError {
    Spawn,
    Timeout,
    BoundedOutput,
}

struct DrainedPipe {
    bytes: Vec<u8>,
    exceeded: bool,
}

/// Drains one pipe to EOF through a capped buffer: at most
/// MAX_HOOK_OUTPUT_BYTES are retained; bytes past the cap are read and
/// discarded so a child writing past the cap never blocks on a full pipe.
async fn drain_pipe_capped(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
) -> std::io::Result<DrainedPipe> {
    let mut bytes = Vec::new();
    let mut exceeded = false;
    let mut chunk = [0u8; 8192];
    loop {
        let n = pipe.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        let room = MAX_HOOK_OUTPUT_BYTES.saturating_sub(bytes.len());
        if n > room {
            bytes.extend_from_slice(&chunk[..room]);
            exceeded = true;
        } else {
            bytes.extend_from_slice(&chunk[..n]);
        }
    }
    Ok(DrainedPipe { bytes, exceeded })
}

async fn execute_command(
    handler: &Handler,
    payload: &Value,
) -> Result<std::process::Output, RunError> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", &handler.command]);
        command
    };
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &handler.command]).process_group(0);
        command
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    let job =
        nano_sandbox::job::JobObject::create_without_breakaway().map_err(|_| RunError::Spawn)?;
    #[cfg(windows)]
    let mut child = job
        .spawn_contained(&mut command)
        .map_err(|_| RunError::Spawn)?;
    #[cfg(unix)]
    let mut child = command.spawn().map_err(|_| RunError::Spawn)?;
    let payload = serde_json::to_vec(payload).map_err(|_| RunError::Spawn)?;
    child
        .stdin
        .take()
        .ok_or(RunError::Spawn)?
        .write_all(&payload)
        .await
        .map_err(|_| RunError::Spawn)?;
    let mut stdout = child.stdout.take().ok_or(RunError::Spawn)?;
    let mut stderr = child.stderr.take().ok_or(RunError::Spawn)?;
    let stdout_task = tokio::spawn(async move { drain_pipe_capped(&mut stdout).await });
    let stderr_task = tokio::spawn(async move { drain_pipe_capped(&mut stderr).await });
    let deadline = Instant::now() + handler.timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|_| RunError::Spawn)? {
            break status;
        }
        if Instant::now() >= deadline {
            #[cfg(windows)]
            let _ = job.terminate();
            #[cfg(unix)]
            unsafe {
                // tokio Child::id() is Option<u32>; Some while running.
                if let Some(id) = child.id() {
                    libc::kill(-(id as i32), libc::SIGKILL);
                }
            }
            let _ = child.wait().await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(RunError::Timeout);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let stdout = stdout_task
        .await
        .map_err(|_| RunError::Spawn)?
        .map_err(|_| RunError::Spawn)?;
    let stderr = stderr_task
        .await
        .map_err(|_| RunError::Spawn)?
        .map_err(|_| RunError::Spawn)?;
    if stdout.exceeded || stderr.exceeded {
        return Err(RunError::BoundedOutput);
    }
    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

#[cfg(test)]
mod tests;
