//! Stdio transport: spawn a child process and split its pipes for the
//! full-duplex dispatcher (design note §2.2).
//!
//! v1 spawns children directly through std::process. Containment (the
//! nano-sandbox job-object spawn, `spawn_process_with_pipes_contained`) is
//! the containment lane's seam — it plugs in at `spawn` without any change
//! to the dispatcher, which consumes only the pipe/child split below.

use crate::client::McpError;
use std::io::BufReader;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// The owned halves of a spawned child, one per dispatcher thread:
/// the writer thread owns `stdin`, the reader thread owns `stdout`, and the
/// supervisor owns `child` (it is the SOLE owner of child-kill, §2.3).
pub struct TransportParts {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: BufReader<ChildStdout>,
}

pub struct StdioTransport {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
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
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(stdout),
        })
    }

    #[cfg(test)]
    pub fn from_pipes(child: Child, stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(BufReader::new(stdout)),
        }
    }

    /// Splits the transport into the per-thread parts. After the split the
    /// `StdioTransport` shell is inert (its Drop kills nothing; the
    /// connection's supervisor owns the child from here on).
    pub fn into_parts(mut self) -> TransportParts {
        TransportParts {
            child: self.child.take().expect("child present"),
            stdin: self.stdin.take().expect("stdin present"),
            stdout: self.stdout.take().expect("stdout present"),
        }
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // Only fires for a transport that was never split (e.g. a spawn
        // followed by a connect-time failure before Connection::spawn).
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
