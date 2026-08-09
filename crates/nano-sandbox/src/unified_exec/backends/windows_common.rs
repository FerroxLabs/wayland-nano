//! Shared helpers for the elevated runner backend (pipe writer/reader tasks).
//!
//! Provenance: ported from Codex
//! `windows-sandbox-rs/src/unified_exec/backends/windows_common.rs` @ 646f7c0a.
//! Transformations: broadcast -> mpsc (spawn_types surface);
//! WindowsTtyInputNormalizer dropped (tty deferred — D8);
//! ProcessDriver/spawn_from_driver -> crate::spawn_types assembly.

use crate::ipc_framed::EmptyPayload;
use crate::ipc_framed::FramedMessage;
use crate::ipc_framed::IPC_PROTOCOL_VERSION;
use crate::ipc_framed::Message;
use crate::ipc_framed::OutputStream;
use crate::ipc_framed::StdinPayload;
use crate::ipc_framed::decode_bytes;
use crate::ipc_framed::encode_bytes;
use std::fs::File;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

pub fn start_runner_pipe_writer(mut pipe_write: File) -> std::sync::mpsc::Sender<FramedMessage> {
    let (outbound_tx, outbound_rx) = std::sync::mpsc::channel::<FramedMessage>();
    tokio::task::spawn_blocking(move || {
        while let Ok(msg) = outbound_rx.recv() {
            if crate::ipc_framed::write_frame(&mut pipe_write, &msg).is_err() {
                break;
            }
        }
    });
    outbound_tx
}

pub fn start_runner_stdin_writer(
    mut writer_rx: mpsc::Receiver<Vec<u8>>,
    outbound_tx: std::sync::mpsc::Sender<FramedMessage>,
    stdin_open: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        while let Some(bytes) = writer_rx.blocking_recv() {
            let msg = FramedMessage {
                version: IPC_PROTOCOL_VERSION,
                message: Message::Stdin {
                    payload: StdinPayload {
                        data_b64: encode_bytes(&bytes),
                    },
                },
            };
            if outbound_tx.send(msg).is_err() {
                break;
            }
        }
        if stdin_open {
            let _ = outbound_tx.send(FramedMessage {
                version: IPC_PROTOCOL_VERSION,
                message: Message::CloseStdin {
                    payload: EmptyPayload::default(),
                },
            });
        }
    })
}

pub fn start_runner_stdout_reader(
    mut pipe_read: File,
    stdout_tx: mpsc::Sender<Vec<u8>>,
    stderr_tx: Option<mpsc::Sender<Vec<u8>>>,
    exit_tx: oneshot::Sender<i32>,
) {
    std::thread::spawn(move || {
        loop {
            let msg = match crate::ipc_framed::read_frame(&mut pipe_read) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    send_runner_error(
                        "runner pipe closed before exit",
                        &stdout_tx,
                        stderr_tx.as_ref(),
                    );
                    let _ = exit_tx.send(-1);
                    break;
                }
                Err(err) => {
                    send_runner_error(
                        &format!("runner read failed: {err}"),
                        &stdout_tx,
                        stderr_tx.as_ref(),
                    );
                    let _ = exit_tx.send(-1);
                    break;
                }
            };

            match msg.message {
                Message::Output { payload } => {
                    if let Ok(data) = decode_bytes(&payload.data_b64) {
                        match payload.stream {
                            OutputStream::Stdout => {
                                let _ = stdout_tx.blocking_send(data);
                            }
                            OutputStream::Stderr => {
                                if let Some(stderr_tx) = stderr_tx.as_ref() {
                                    let _ = stderr_tx.blocking_send(data);
                                } else {
                                    let _ = stdout_tx.blocking_send(data);
                                }
                            }
                        }
                    }
                }
                Message::Exit { payload } => {
                    let _ = exit_tx.send(payload.exit_code);
                    break;
                }
                Message::Error { payload } => {
                    send_runner_error(&payload.message, &stdout_tx, stderr_tx.as_ref());
                    let _ = exit_tx.send(-1);
                    break;
                }
                Message::SpawnReady { .. }
                | Message::Stdin { .. }
                | Message::CloseStdin { .. }
                | Message::Resize { .. }
                | Message::SpawnRequest { .. }
                | Message::Terminate { .. } => {}
            }
        }
    });
}

fn send_runner_error(
    message: &str,
    stdout_tx: &mpsc::Sender<Vec<u8>>,
    stderr_tx: Option<&mpsc::Sender<Vec<u8>>>,
) {
    let formatted = format!("runner error: {message}\n").into_bytes();
    if let Some(stderr_tx) = stderr_tx {
        let _ = stderr_tx.blocking_send(formatted);
    } else {
        let _ = stdout_tx.blocking_send(formatted);
    }
}
