//! Shell tool: command execution through the sandboxed capture path.
//!
//! Architecture rules:
//! - commands execute ONLY through nano-sandbox (restricted spawn) — never a
//!   raw std::process::Command anywhere in this crate;
//! - the model is told exactly which execution environment is active
//!   (shell selection policy: native cmd by default, PowerShell on request);
//! - outputs are bounded, timeouts enforced, and the result carries the
//!   shell identity for protocol reporting.

use nano_core::abs::AbsolutePathBuf;
use nano_core::permissions::PermissionProfile;
use nano_sandbox::capture::{CaptureResult, run_windows_sandbox_capture};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// Native Windows cmd.exe — the v1 default.
    Cmd,
    /// PowerShell 7 when available, Windows PowerShell otherwise.
    PowerShell,
}

impl ShellKind {
    pub fn describe(self) -> &'static str {
        match self {
            ShellKind::Cmd => "cmd.exe (native Windows)",
            ShellKind::PowerShell => "PowerShell (native Windows)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    pub shell: ShellKind,
    pub duration: std::time::Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("sandbox spawn failed: {0}")]
    Spawn(String),
}

const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub struct ShellTool {
    nano_home: std::path::PathBuf,
    workspace: std::path::PathBuf,
}

impl ShellTool {
    pub fn new(nano_home: &Path, workspace: &Path) -> Self {
        Self {
            nano_home: nano_home.to_path_buf(),
            workspace: workspace.to_path_buf(),
        }
    }

    fn argv(shell: ShellKind, command: &str) -> Vec<String> {
        match shell {
            ShellKind::Cmd => vec!["cmd.exe".into(), "/c".into(), command.into()],
            ShellKind::PowerShell => vec![
                "powershell.exe".into(),
                "-NoProfile".into(),
                "-Command".into(),
                command.into(),
            ],
        }
    }

    /// Executes `command` contained to the workspace (workspace-write
    /// profile), with output bounds and an optional timeout.
    pub fn run(
        &self,
        shell: ShellKind,
        command: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<ShellOutput, ShellError> {
        let started = std::time::Instant::now();
        let roots = [AbsolutePathBuf::from_absolute_path(&self.workspace)
            .map_err(|e| ShellError::Spawn(format!("workspace root: {e}")))?];
        let result: CaptureResult = run_windows_sandbox_capture(
            &PermissionProfile::workspace_write(),
            &roots,
            &self.nano_home,
            Self::argv(shell, command),
            &self.workspace,
            HashMap::new(),
            timeout.map(|t| t.as_millis() as u64),
            None,
            false,
        )
        .map_err(|e| ShellError::Spawn(format!("{e:#}")))?;

        let (stdout, out_trunc) = truncate_lossy(result.stdout, MAX_OUTPUT_BYTES);
        let (stderr, err_trunc) = truncate_lossy(result.stderr, MAX_OUTPUT_BYTES);

        Ok(ShellOutput {
            exit_code: result.exit_code,
            stdout,
            stderr,
            timed_out: result.timed_out,
            truncated: out_trunc || err_trunc,
            shell,
            duration: started.elapsed(),
        })
    }
}

fn truncate_lossy(bytes: Vec<u8>, max: usize) -> (String, bool) {
    let text = String::from_utf8_lossy(&bytes);
    if text.len() <= max {
        return (text.into_owned(), false);
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (format!("{}…[truncated]", &text[..cut]), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("nano-home");
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        (tmp, home, ws)
    }

    #[test]
    fn cmd_echo_returns_zero_and_output() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Cmd,
                "echo nanok3-shell",
                Some(std::time::Duration::from_secs(60)),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout.contains("nanok3-shell"),
            "stdout: {}",
            out.stdout
        );
        assert!(!out.timed_out);
        assert!(matches!(out.shell, ShellKind::Cmd));
    }

    #[test]
    fn cmd_write_inside_workspace_lands() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Cmd,
                "echo data > shell-out.txt && type shell-out.txt",
                Some(std::time::Duration::from_secs(60)),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 0);
        assert!(ws.join("shell-out.txt").exists());
        assert!(out.stdout.contains("data"));
    }

    #[test]
    fn nonzero_exit_code_surfaces() {
        let (_tmp, home, ws) = fixture();
        let tool = ShellTool::new(&home, &ws);
        let out = tool
            .run(
                ShellKind::Cmd,
                "exit 3",
                Some(std::time::Duration::from_secs(60)),
            )
            .expect("spawn");
        assert_eq!(out.exit_code, 3);
    }
}
