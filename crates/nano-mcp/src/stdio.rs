//! Stdio transport: line-delimited JSON-RPC over a child process's pipes.
//!
//! v1 spawns children directly through std::process (the sandboxed spawn
//! path plugs in here when elevated provisioning exists — the seam is
//! `spawn_fn` so tests and future containment share one transport).

use crate::client::McpError;
use crate::protocol::JsonRpcResponse;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl StdioTransport {
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, McpError> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| McpError::Transport(format!("spawn {command}: {e}")))?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout piped"));
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    #[cfg(test)]
    pub fn from_pipes(child: Child, stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        }
    }

    pub fn send_line(&mut self, line: &str) -> Result<(), McpError> {
        writeln!(self.stdin, "{line}")
            .and_then(|_| self.stdin.flush())
            .map_err(|e| McpError::Transport(format!("write: {e}")))
    }

    /// Reads one JSON-RPC response, skipping notification lines.
    pub fn read_response(&mut self) -> Result<JsonRpcResponse, McpError> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .stdout
                .read_line(&mut line)
                .map_err(|e| McpError::Transport(format!("read: {e}")))?;
            if n == 0 {
                return Err(McpError::Transport("server closed stdout".into()));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(trimmed)
                .map_err(|e| McpError::Protocol(format!("bad json line: {e}: {trimmed}")))?;
            if value.get("method").is_some() && value.get("id").is_none() {
                continue; // server notification: skip
            }
            return serde_json::from_value(value)
                .map_err(|e| McpError::Protocol(format!("bad response shape: {e}")));
        }
    }

    pub fn try_kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.try_kill();
    }
}
