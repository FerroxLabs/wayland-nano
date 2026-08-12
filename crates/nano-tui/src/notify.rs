//! OSC 9 desktop notifications (C10 §7), ported-intent from codex
//! `tui/src/notifications/osc9.rs` (see UPSTREAM.md): `\x1b]9;{msg}\x07`,
//! with the tmux DCS-passthrough variant. Deliberately trivial:
//!
//! - fired ONLY for the two-entry allowlist: turn complete (agent idle, the
//!   user-away case) and permission-prompt pending. Nothing else in v1.
//! - the payload passes through the TUI sanitizer before emission — a
//!   notification is a terminal-injection surface like any rendered text;
//! - detection is BEST-EFFORT (no reliable capability handshake exists for
//!   OSC 9): tmux is inferred from the environment, emission is
//!   fire-and-forget, and terminals that silently ignore the sequence are
//!   the expected case, not an error;
//! - the config off-switch is NANO_TUI_NOTIFY=0/off/false (env is the TUI's
//!   only config channel); a disabled notifier emits nothing;
//! - emission NEVER blocks the render loop: one small write to stderr (the
//!   TUI's own terminal channel — ratatui owns stdout), errors ignored.

use std::io::Write;

/// The two-entry allowlist (C10 §7): no other event notifies in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    /// A turn finished and the agent is idle.
    TurnComplete,
    /// A permission/question prompt is waiting on the user.
    PermissionPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notifier {
    enabled: bool,
    tmux: bool,
}

impl Notifier {
    /// From the environment: NANO_TUI_NOTIFY=0|off|false disables; TMUX set
    /// selects the DCS-passthrough encoding.
    pub fn from_env() -> Self {
        let enabled = !matches!(
            std::env::var("NANO_TUI_NOTIFY")
                .map(|v| v.to_ascii_lowercase())
                .as_deref(),
            Ok("0") | Ok("off") | Ok("false")
        );
        Self {
            enabled,
            tmux: std::env::var_os("TMUX").is_some(),
        }
    }

    /// A disabled notifier (tests, and the explicit off-switch path).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            tmux: false,
        }
    }

    /// The encoded sequence for one notification (empty when disabled) —
    /// pure, so the encoding and the allowlist are unit-testable without a
    /// terminal.
    pub fn encode(&self, kind: NotifyKind, message: &str) -> Vec<u8> {
        if !self.enabled {
            return Vec::new();
        }
        let title = match kind {
            NotifyKind::TurnComplete => "wayland-nano: turn complete",
            NotifyKind::PermissionPending => "wayland-nano: approval needed",
        };
        // Sanitize like any rendered model text, then strip the sequence
        // terminators themselves so a payload cannot close its own frame.
        let clean = crate::sanitize::sanitize(message)
            .replace(['\x07', '\x1b'], "")
            .replace('\n', " ");
        let payload = format!("{title} — {clean}");
        let osc = format!("\x1b]9;{payload}\x07");
        if self.tmux {
            // tmux DCS passthrough: every inner ESC doubled, wrapped in
            // DCS tmux; ... ST (codex osc9.rs verbatim intent).
            let inner = osc.replace('\x1b', "\x1b\x1b");
            format!("\x1bPtmux;{inner}\x1b\\").into_bytes()
        } else {
            osc.into_bytes()
        }
    }

    /// Fire-and-forget emission to a sink (production: stderr). Never
    /// blocks the render loop on failure — errors are swallowed by design.
    pub fn notify(&self, sink: &mut dyn Write, kind: NotifyKind, message: &str) {
        let bytes = self.encode(kind, message);
        if !bytes.is_empty() {
            let _ = sink.write_all(&bytes);
            let _ = sink.flush();
        }
    }

    /// Production emission: stderr is the TUI's own terminal channel
    /// (ratatui owns stdout). Gated on stderr being a real terminal — a
    /// piped stderr (tests, redirects) has no one to notify.
    pub fn notify_terminal(&self, kind: NotifyKind, message: &str) {
        use std::io::IsTerminal;
        if !std::io::stderr().is_terminal() {
            return;
        }
        self.notify(&mut std::io::stderr(), kind, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notifier() -> Notifier {
        Notifier {
            enabled: true,
            tmux: false,
        }
    }

    #[test]
    fn hostile_payloads_are_sanitized() {
        // An injected BEL/ESC must not close or smuggle the sequence.
        let bytes = notifier().encode(NotifyKind::TurnComplete, "done\x07\x1b]8;;evil\x1b\\");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("\x1b]9;"));
        // Exactly one BEL (the terminator) and one ESC (the opener).
        assert_eq!(text.matches('\x07').count(), 1);
        assert_eq!(text.matches('\x1b').count(), 1);
        assert!(!text.contains("evil"));
    }

    #[test]
    fn tmux_passthrough_encoding() {
        let tmux = Notifier {
            enabled: true,
            tmux: true,
        };
        let bytes = tmux.encode(NotifyKind::PermissionPending, "fs_write");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("\x1bPtmux;"), "{text:?}");
        assert!(text.ends_with("\x07\x1b\\"), "{text:?}");
        // The inner OSC's ESC is doubled.
        assert!(text.contains("\x1b\x1b]9;"), "{text:?}");
        assert!(text.contains("approval needed — fs_write"));
    }

    #[test]
    fn disabled_emits_nothing() {
        assert!(
            Notifier::disabled()
                .encode(NotifyKind::TurnComplete, "x")
                .is_empty()
        );
        let mut sink = Vec::new();
        Notifier::disabled().notify(&mut sink, NotifyKind::TurnComplete, "x");
        assert!(sink.is_empty());
    }

    #[test]
    fn notify_never_fails_the_caller() {
        struct BrokenSink;
        impl Write for BrokenSink {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("broken"))
            }
        }
        // Must not panic or propagate: fire-and-forget.
        notifier().notify(&mut BrokenSink, NotifyKind::TurnComplete, "done");
    }
}
