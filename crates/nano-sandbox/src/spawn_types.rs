//! Spawn session types: the stdio/channel surface of a spawned sandbox
//! session.
//!
//! Design decision (recorded): the donor threads its full `ProcessHandle`
//! (PTY ownership, resize hooks, abort handles, killer box) through the spawn
//! stack. Track B v1 spawns non-interactive sessions (`tty: false`), so we
//! lift only the used surface — stdin writer, stdin close, terminate request —
//! into a Nano-owned handle. If ConPTY interactive sessions land later, this
//! handle grows; the channel contract stays.
//!
//! Provenance: surface extracted from Codex `codex-rs/utils/pty/src/process.rs`
//! (`SpawnedProcess`, `ProcessHandle`) @ 646f7c0a — Nano-owned minimal
//! implementation, not a port of internals.

use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// Handle for driving a spawned (non-interactive) sandbox session.
pub struct SandboxSessionHandle {
    writer_tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    terminator: Mutex<Option<Box<dyn FnMut() + Send + Sync>>>,
}

impl std::fmt::Debug for SandboxSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxSessionHandle").finish_non_exhaustive()
    }
}

impl SandboxSessionHandle {
    pub fn new(
        writer_tx: mpsc::Sender<Vec<u8>>,
        terminator: impl FnMut() + Send + Sync + 'static,
    ) -> Self {
        Self {
            writer_tx: Mutex::new(Some(writer_tx)),
            terminator: Mutex::new(Some(Box::new(terminator))),
        }
    }

    /// Sender half of the session stdin channel.
    pub fn writer_sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.writer_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("session writer requested after close")
    }

    /// Closes the session's stdin (drops the sender).
    pub fn close_stdin(&self) {
        let _ = self
            .writer_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    /// Requests termination of the session process tree.
    pub fn request_terminate(&self) {
        let mut terminator = self
            .terminator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(terminate) = terminator.as_mut() {
            terminate();
            // One-shot semantics: repeated Ctrl+C does not re-fire.
            *terminator = None;
        }
    }
}

/// Return value from spawn helpers: session handle plus output/exit channels.
#[derive(Debug)]
pub struct SpawnedProcess {
    pub session: SandboxSessionHandle,
    pub stdout_rx: mpsc::Receiver<Vec<u8>>,
    pub stderr_rx: mpsc::Receiver<Vec<u8>>,
    pub exit_rx: oneshot::Receiver<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_handle_write_close_terminate_lifecycle() {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(4);
        let terminated = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let terminated2 = std::sync::Arc::clone(&terminated);
        let session = SandboxSessionHandle::new(tx, move || {
            terminated2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let writer = session.writer_sender();
        writer.send(b"hello".to_vec()).await.unwrap();
        assert_eq!(rx.recv().await.as_deref(), Some(&b"hello"[..]));

        session.close_stdin();
        drop(writer);
        assert!(rx.recv().await.is_none(), "closed stdin must end the channel");

        session.request_terminate();
        session.request_terminate(); // one-shot: second call is a no-op
        assert_eq!(terminated.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
