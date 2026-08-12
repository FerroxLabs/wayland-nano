//! Internal event bus (design doc §4): a small `AppEvent` enum with a
//! cloneable sender (the `app_event_sender.rs` pattern). ACP frames are NOT
//! on this bus — the loop selects on `acp.next_frame()` directly, matching
//! the codex app.rs channel split (bus / engine stream / terminal events).

use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

/// Everything on the internal bus: terminal input (posted by the key-reader
/// thread), coalesced redraws, doctor subprocess results, shutdown.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A key press from the terminal.
    Key(KeyEvent),
    /// Bracketed paste (coalesced by the terminal into one event).
    Paste(String),
    /// Terminal resized (cols, rows); may be 0x0 mid resize-storm.
    Resize(u16, u16),
    /// A coalesced redraw request from the frame scheduler.
    Redraw,
    /// A short-lived `wayland-nano doctor` subprocess finished (C1).
    DoctorDone { output: String, exit_code: i32 },
    /// Graceful shutdown requested.
    Quit,
}

/// Cloneable sender half: widgets and background threads hand events back
/// to the loop without knowing the loop.
#[derive(Clone, Debug)]
pub struct AppEventSender {
    tx: mpsc::UnboundedSender<AppEvent>,
}

impl AppEventSender {
    pub fn new(tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self { tx }
    }

    /// Post an event; false when the loop is already gone.
    pub fn send(&self, event: AppEvent) -> bool {
        self.tx.send(event).is_ok()
    }
}
