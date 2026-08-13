//! P4 §3.3: the bounded input contract for `/review` — the HOST computes
//! the diff, never the review child (the child has no shell). v1 scope:
//! uncommitted working-tree changes (`git diff HEAD`) plus the untracked
//! file enumeration (names only; CONTENTS are not reviewed in v1 and the
//! reviewer's verdict must say so).
//!
//! The invocation is hardened (round-1's "reads only" claim was wrong —
//! git config, attributes, external diff drivers, textconv, and the pager
//! can all execute repository-controlled helpers):
//! - fixed argv: `git --no-pager --no-optional-locks -c core.pager=
//!   -c diff.external= diff --no-ext-diff --no-textconv HEAD` — pager,
//!   external diff, and textconv are FLAG-disabled, not environment-hinted;
//! - scrubbed environment (env-clear + whitelist, never inherit-and-hope):
//!   `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` pointed
//!   at the null device, `HOME` at an empty scratch dir,
//!   `GIT_CEILING_DIRECTORIES` pinning repo discovery to the workspace, plus
//!   the spawn-lookup minimum (`PATH`, and `SYSTEMROOT` on Windows). Every
//!   other variable — every `GIT_*`, `PAGER`, `EDITOR` — is REMOVED;
//! - fixed cwd = the canonicalized workspace root, with a `.git` existence
//!   pre-check so a non-repo workspace never triggers upward discovery;
//! - stdout/stderr consumed with bounded streaming (cap counters while
//!   draining — memory stays bounded, the pipe never deadlocks);
//! - an explicit 10s timeout with TREE kill (Windows: the nano-sandbox Job
//!   Object discipline, §4.3's pattern; unix: process-group SIGKILL);
//! - FAIL CLOSED on any nonzero/unexpected exit, prompt-like stderr, or
//!   timeout: a typed refusal, no bundle.
//!
//! A non-git workspace is a typed `NotAGitWorkspace` (the ACP layer maps it
//! to `InvalidParams` with a bounded reason — §8: capacity and precondition
//! failures are not error-table entries).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// §3.3 caps: the diff is capped at 256 KiB (the shell tool's
/// MAX_OUTPUT_BYTES discipline, shell.rs:106); the untracked-file list at
/// 200 entries.
pub const REVIEW_DIFF_BYTE_CAP: usize = 256 * 1024;
pub const REVIEW_UNTRACKED_CAP: usize = 200;
/// The git invocation's hard timeout.
pub const REVIEW_GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// stderr is bounded (4 KiB kept) — it is only ever diagnostic.
const STDERR_CAP: usize = 4 * 1024;

/// The host-computed review input (§3.3). `truncated`/`omitted_bytes` are
/// the TYPED truncation the reviewer scopes by (verdict
/// `review_truncated`); a present `untracked` list forces the
/// `untracked_unreviewed` verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDiffBundle {
    pub diff: String,
    pub truncated: bool,
    pub omitted_bytes: u64,
    pub untracked: Vec<String>,
    pub untracked_truncated: bool,
}

/// The typed refusals (§3.3: fail closed, no bundle). Display strings are
/// bounded and carry no command text beyond the fixed invocation's nature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDiffError {
    /// Not a git workspace (the `.git` pre-check or git's own 128 refused).
    /// Maps to `InvalidParams` at the ACP boundary (§8).
    NotAGitWorkspace,
    /// Nonzero/unexpected exit, prompt-like stderr, or any other
    /// helper-execution suspicion. The reason is bounded (≤ 4 KiB stderr).
    Refused(String),
    /// The 10s bound tripped; the process tree was killed.
    Timeout,
    /// The git process could not be spawned at all.
    Spawn(String),
}

impl std::fmt::Display for ReviewDiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewDiffError::NotAGitWorkspace => f.write_str(
                "not a git workspace: /review needs a git repository with a HEAD commit",
            ),
            ReviewDiffError::Refused(reason) => {
                write!(f, "review diff refused (fail-closed): {reason}")
            }
            ReviewDiffError::Timeout => {
                f.write_str("review diff timed out (10s); the git process tree was killed")
            }
            ReviewDiffError::Spawn(err) => {
                write!(f, "review diff could not start git: {err}")
            }
        }
    }
}

impl std::error::Error for ReviewDiffError {}

/// Compute the §3.3 bundle for `workspace`. Synchronous and bounded; the
/// ACP handler calls it directly (the 10s timeout is the worst case).
pub fn compute_review_bundle(workspace: &Path) -> Result<ReviewDiffBundle, ReviewDiffError> {
    let canonical = workspace
        .canonicalize()
        .map_err(|e| ReviewDiffError::Refused(format!("workspace cannot be canonicalized: {e}")))?;
    // The .git pre-check pins repo resolution to the workspace: no repo at
    // the root means we NEVER let git discover a parent repo (worktrees
    // carry a .git FILE; both forms count).
    if canonical.join(".git").symlink_metadata().is_err() {
        return Err(ReviewDiffError::NotAGitWorkspace);
    }
    let scratch = ScratchHome::new()?;

    let diff_args: [&str; 10] = [
        "--no-pager",
        "--no-optional-locks",
        "-c",
        "core.pager=",
        "-c",
        "diff.external=",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "HEAD",
    ];
    let diff_run = run_hardened_git(&canonical, scratch.path(), &diff_args)?;
    if diff_run.timed_out {
        return Err(ReviewDiffError::Timeout);
    }
    let stderr = String::from_utf8_lossy(&diff_run.stderr);
    match diff_run.exit_code {
        Some(0) => {}
        // A race with the pre-check (repo removed mid-call) still maps to
        // the typed non-git refusal.
        Some(128) if stderr.contains("not a git repository") => {
            return Err(ReviewDiffError::NotAGitWorkspace);
        }
        other => {
            return Err(ReviewDiffError::Refused(format!(
                "git diff exited {other:?}: {}",
                stderr.trim()
            )));
        }
    }
    if looks_like_prompt(&stderr) {
        return Err(ReviewDiffError::Refused(
            "git emitted prompt-like output on stderr; refusing to trust the diff".into(),
        ));
    }

    // Untracked enumeration (names only — contents are NOT reviewed in v1).
    let untracked_run = run_hardened_git(
        &canonical,
        scratch.path(),
        &[
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.pager=",
            "ls-files",
            "--others",
            "--exclude-standard",
        ],
    )?;
    if untracked_run.timed_out {
        return Err(ReviewDiffError::Timeout);
    }
    if untracked_run.exit_code != Some(0) {
        return Err(ReviewDiffError::Refused(format!(
            "git ls-files exited {:?}: {}",
            untracked_run.exit_code,
            String::from_utf8_lossy(&untracked_run.stderr).trim()
        )));
    }
    let untracked_text = String::from_utf8_lossy(&untracked_run.stdout);
    let mut untracked: Vec<String> = Vec::new();
    let mut untracked_truncated = false;
    for line in untracked_text.lines() {
        if untracked.len() >= REVIEW_UNTRACKED_CAP {
            untracked_truncated = true;
            break;
        }
        let name = line.trim();
        if !name.is_empty() {
            untracked.push(name.to_string());
        }
    }

    let total = diff_run.stdout_total;
    let truncated = total > REVIEW_DIFF_BYTE_CAP as u64;
    let diff = String::from_utf8_lossy(&diff_run.stdout).into_owned();
    Ok(ReviewDiffBundle {
        diff,
        truncated,
        omitted_bytes: total.saturating_sub(REVIEW_DIFF_BYTE_CAP as u64),
        untracked,
        untracked_truncated,
    })
}

/// The prompt-like stderr heuristic (§3.3: "any stderr prompt-like output
/// ⇒ typed refusal"). git's credential/pager helpers ask for input on the
/// terminal; with the scrubbed environment none should ever run, so a
/// prompt-shaped line means the hardening was bypassed — fail closed.
fn looks_like_prompt(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    const PROMPT_MARKERS: &[&str] = &[
        "password",
        "passphrase",
        "username",
        "credential",
        "are you sure",
        "(yes/no)",
        "y/n",
        "press enter",
        "terminal prompts disabled",
    ];
    PROMPT_MARKERS.iter().any(|marker| lower.contains(marker))
}

struct GitRun {
    stdout: Vec<u8>,
    /// TOTAL bytes the child wrote to stdout (≥ stdout.len()).
    stdout_total: u64,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    timed_out: bool,
}

/// The hardened spawn: fixed argv, scrubbed environment, canonical cwd,
/// bounded streaming, 10s timeout + tree kill. Fails closed everywhere.
fn run_hardened_git(
    cwd: &Path,
    scratch_home: &Path,
    args: &[&str],
) -> Result<GitRun, ReviewDiffError> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // env-clear + whitelist (§3.3), never inherit-and-hope.
    command.env_clear();
    // Spawn lookup needs PATH (exec resolution reads the CHILD env on
    // unix); Windows additionally needs SYSTEMROOT for any process.
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    #[cfg(windows)]
    if let Ok(root) = std::env::var("SYSTEMROOT") {
        command.env("SYSTEMROOT", root);
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command.env("GIT_CONFIG_GLOBAL", null_device);
    command.env("GIT_CONFIG_SYSTEM", null_device);
    command.env("HOME", scratch_home);
    // Repo discovery is pinned to the workspace: never ascend above it
    // (the .git pre-check makes ascent moot; this is the in-env backstop).
    command.env("GIT_CEILING_DIRECTORIES", cwd);
    // A group leader so the timeout can kill the whole tree on unix.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);

    // Windows: the child spawns inside a fresh Job Object (the §4.3
    // discipline); the guard bounds the tree's lifetime to this call
    // (KILL_ON_JOB_CLOSE on drop, explicit terminate on timeout).
    let (mut child, job) = spawn_contained(&mut command)?;
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    // Bounded streaming: drain both pipes on their own threads, KEEPING at
    // most the caps while COUNTING everything (draining past the cap keeps
    // the pipe from deadlocking the writer without an unbounded read).
    let stdout_thread = std::thread::spawn(move || drain_capped(&mut stdout, REVIEW_DIFF_BYTE_CAP));
    let stderr_thread = std::thread::spawn(move || drain_capped(&mut stderr, STDERR_CAP));

    let deadline = std::time::Instant::now() + REVIEW_GIT_TIMEOUT;
    let (exit_code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), false),
            Ok(None) if std::time::Instant::now() >= deadline => {
                kill_tree(&mut child, pid, &job);
                // Reap so the process never lingers as a zombie handle.
                let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while std::time::Instant::now() < reap_deadline {
                    if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                break (None, true);
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(err) => {
                kill_tree(&mut child, pid, &job);
                return Err(ReviewDiffError::Refused(format!("git wait failed: {err}")));
            }
        }
    };
    let (stdout, stdout_total) = stdout_thread
        .join()
        .map_err(|_| ReviewDiffError::Refused("stdout reader panicked".into()))?;
    let (stderr, _) = stderr_thread
        .join()
        .map_err(|_| ReviewDiffError::Refused("stderr reader panicked".into()))?;
    Ok(GitRun {
        stdout,
        stdout_total,
        stderr,
        exit_code,
        timed_out,
    })
}

/// Read to EOF, retaining only the first `cap` bytes; returns (kept,
/// total). Memory-bounded regardless of the writer.
fn drain_capped(reader: &mut impl Read, cap: usize) -> (Vec<u8>, u64) {
    let mut kept = Vec::with_capacity(cap.min(64 * 1024));
    let mut total: u64 = 0;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                total = total.saturating_add(n as u64);
                let room = cap.saturating_sub(kept.len());
                if room > 0 {
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    }
    (kept, total)
}

/// The spawned child's tree-lifetime guard. Windows: the Job Object (drop
/// = KILL_ON_JOB_CLOSE). unix: unit — the timeout path kills the child's
/// process GROUP (the child is a group leader, `process_group(0)`).
struct TreeGuard {
    #[cfg(windows)]
    job: nano_sandbox::job::JobObject,
}

/// Windows: suspended spawn + assign + resume through the nano-sandbox
/// JobObject (the §4.3 discipline — no assign-after-start window). The
/// spawn is FAIL-CLOSED on containment failure: a child that could not be
/// job-assigned is killed, never run uncontained.
#[cfg(windows)]
fn spawn_contained(command: &mut Command) -> Result<(Child, TreeGuard), ReviewDiffError> {
    use std::os::windows::process::CommandExt;
    let job = nano_sandbox::job::JobObject::create_without_breakaway()
        .map_err(|e| ReviewDiffError::Refused(format!("job object: {e}")))?;
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    let mut child = command
        .spawn()
        .map_err(|e| ReviewDiffError::Spawn(e.to_string()))?;
    match job.assign_and_resume_process(child.id()) {
        // Assigned AND resumed: contained.
        Ok(true) => Ok((child, TreeGuard { job })),
        // Resumed WITHOUT containment (nested-job rejection): fail closed.
        Ok(false) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(ReviewDiffError::Refused(
                "job-object assignment refused (nested job); the git process was killed, never run uncontained".into(),
            ))
        }
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(ReviewDiffError::Refused(format!(
                "job-object assign/resume failed: {err}"
            )))
        }
    }
}

#[cfg(not(windows))]
fn spawn_contained(command: &mut Command) -> Result<(Child, TreeGuard), ReviewDiffError> {
    let child = command
        .spawn()
        .map_err(|e| ReviewDiffError::Spawn(e.to_string()))?;
    Ok((child, TreeGuard {}))
}

/// Tree kill after the timeout (§3.3). Windows: terminate the job (every
/// direct descendant dies). unix: SIGKILL the child's process group, then
/// the child itself as the backstop.
#[cfg(windows)]
fn kill_tree(child: &mut Child, _pid: u32, guard: &TreeGuard) {
    let _ = guard.job.terminate();
    let _ = child.kill();
}

#[cfg(unix)]
fn kill_tree(child: &mut Child, pid: u32, _guard: &TreeGuard) {
    // kill(-pgid) targets the group the child leads (process_group(0) at
    // spawn); a negative pid never names a single unrelated process.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

/// The empty scratch HOME for the scrubbed environment (§3.3): a fresh
/// empty dir per invocation, removed on drop. git finds no config, no
/// credentials, no helpers there.
struct ScratchHome {
    path: PathBuf,
}

impl ScratchHome {
    fn new() -> Result<Self, ReviewDiffError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "wayland-nano-review-home-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path)
            .map_err(|e| ReviewDiffError::Refused(format!("scratch HOME: {e}")))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture git repo: one committed file, then a working-tree edit.
    /// git is a hard dependency of this feature (non-git workspaces are a
    /// typed refusal, per contract) — a missing git binary fails the test,
    /// never skips it.
    fn fixture_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .env(
                    "GIT_CONFIG_SYSTEM",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .output()
                .expect("git runs");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "review@test.invalid"]);
        git(&["config", "user.name", "review test"]);
        std::fs::write(repo.join("code.rs"), "fn answer() -> u32 { 42 }\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        (tmp, repo)
    }

    #[test]
    fn diff_of_working_tree_changes() {
        let (_tmp, repo) = fixture_repo();
        std::fs::write(repo.join("code.rs"), "fn answer() -> u32 { 41 }\n").unwrap();
        let bundle = compute_review_bundle(&repo).expect("bundle computes");
        assert!(bundle.diff.contains("diff --git"), "{}", bundle.diff);
        assert!(bundle.diff.contains("41"), "{}", bundle.diff);
        assert!(!bundle.truncated);
        assert_eq!(bundle.omitted_bytes, 0);
        assert!(bundle.untracked.is_empty());
    }

    #[test]
    fn non_git_workspace_is_typed_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("code.rs"), "x").unwrap();
        assert_eq!(
            compute_review_bundle(&plain),
            Err(ReviewDiffError::NotAGitWorkspace)
        );
    }

    #[test]
    fn untracked_files_enumerated_contents_not_reviewed() {
        let (_tmp, repo) = fixture_repo();
        std::fs::write(repo.join("untracked.rs"), "fn new() {}\n").unwrap();
        let bundle = compute_review_bundle(&repo).expect("bundle computes");
        assert_eq!(bundle.untracked, vec!["untracked.rs".to_string()]);
        assert!(!bundle.untracked_truncated);
        // The untracked file's CONTENT never enters the diff.
        assert!(!bundle.diff.contains("fn new()"), "{}", bundle.diff);
    }

    /// §13 isolation battery / §14 promotion gate: a fixture repo whose
    /// `.git/config` points core.pager and diff.external at a canary
    /// executable, with a textconv attribute — the canary must NEVER
    /// execute (fs oracle: no marker file), and the diff still returns.
    #[test]
    fn git_helpers_never_execute() {
        let (_tmp, repo) = fixture_repo();
        let marker = repo.join("canary-ran.marker");
        // The canary: any invocation writes the marker. .bat on Windows,
        // sh elsewhere; git runs pager/external commands through the shell.
        let (canary, ext) = if cfg!(windows) {
            (repo.join("canary.bat"), "bat")
        } else {
            (repo.join("canary.sh"), "sh")
        };
        if cfg!(windows) {
            std::fs::write(
                &canary,
                format!(
                    "@echo off\r\necho ran> \"{}\"\r\n",
                    marker.display().to_string().replace('/', "\\")
                ),
            )
            .unwrap();
        } else {
            std::fs::write(
                &canary,
                format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
            )
            .unwrap();
            // The canary must be EXECUTABLE — the leg proves the hardening
            // (flags/env), not a filesystem accident, keeps it from running.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&canary, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let mut config = std::fs::read_to_string(repo.join(".git/config")).unwrap();
        // git config values use C-style escapes — forward slashes keep the
        // Windows path literal parseable.
        let canary_cfg = repo.join("canary").display().to_string().replace('\\', "/");
        config.push_str(&format!(
            "[core]\n\tpager = {canary_cfg}.{ext}\n[diff]\n\texternal = {canary_cfg}.{ext}\n[diff \"canary\"]\n\ttextconv = {canary_cfg}.{ext}\n",
        ));
        std::fs::write(repo.join(".git/config"), config).unwrap();
        // The textconv attribute: route *.rs through the canary textconv.
        std::fs::write(repo.join(".gitattributes"), "*.rs diff=canary\n").unwrap();
        std::fs::write(repo.join("code.rs"), "fn answer() -> u32 { 41 }\n").unwrap();

        let bundle = compute_review_bundle(&repo).expect("diff returns, helpers disabled");
        assert!(bundle.diff.contains("41"), "{}", bundle.diff);
        assert!(
            !marker.exists(),
            "the canary helper executed — PROMOTION GATE FAILURE"
        );
    }

    /// §13: a diff past 256 KiB is TYPED truncation — the bundle carries
    /// {truncated: true, omitted_bytes}, never a silent partial.
    #[test]
    fn oversized_diff_is_typed_truncation() {
        let (_tmp, repo) = fixture_repo();
        let big_line = "x".repeat(1024);
        let mut content = String::new();
        for _ in 0..600 {
            content.push_str(&big_line);
            content.push('\n');
        }
        std::fs::write(repo.join("code.rs"), &content).unwrap();
        let bundle = compute_review_bundle(&repo).expect("bundle computes");
        assert!(bundle.truncated, "past the 256 KiB cap");
        assert!(bundle.diff.len() <= REVIEW_DIFF_BYTE_CAP);
        assert!(bundle.omitted_bytes > 0);
        assert!(
            bundle.diff.len() as u64 + bundle.omitted_bytes > REVIEW_DIFF_BYTE_CAP as u64,
            "omitted bytes accounted"
        );
    }

    #[test]
    fn prompt_like_stderr_detection() {
        assert!(looks_like_prompt("Password for 'https://x':"));
        assert!(looks_like_prompt("Enter passphrase for key:"));
        assert!(looks_like_prompt("terminal prompts disabled"));
        assert!(!looks_like_prompt("warning: LF will be replaced by CRLF"));
        assert!(!looks_like_prompt(""));
    }

    #[test]
    fn drain_capped_bounds_memory_and_counts() {
        let data = vec![7u8; 100_000];
        let mut slice = &data[..];
        let (kept, total) = drain_capped(&mut slice, 1000);
        assert_eq!(kept.len(), 1000);
        assert_eq!(total, 100_000);
    }
}
