//! Status-line state (design doc §4): model + wire state from ACP frames,
//! plus the latest doctor summary (C1 — from the short-lived doctor
//! subprocess, never an engine link).

/// Where the wire to acp-host stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireState {
    Connecting,
    Ready,
    TurnRunning,
    AwaitingApproval,
    Disconnected,
}

impl std::fmt::Display for WireState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            WireState::Connecting => "connecting",
            WireState::Ready => "ready",
            WireState::TurnRunning => "turn running",
            WireState::AwaitingApproval => "awaiting approval",
            WireState::Disconnected => "disconnected",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    pub session_id: Option<String>,
    pub model: String,
    pub models: Vec<(String, String)>,
    /// The session's permission mode id (C2), from the advertised `modes`
    /// block; `?` until the session advertises one.
    pub mode: String,
    /// The advertised availableModes (id, name) the /mode picker lists.
    pub modes: Vec<(String, String)>,
    pub wire: WireState,
    /// Last doctor run's summary line (`summary: N fail, M warn`), if any.
    pub doctor_summary: Option<String>,
    /// Open todo items (C10 §2 status-line count), tracked from `todo`
    /// tool_call frames. None = no list seen this session.
    pub todo_open: Option<usize>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            session_id: None,
            model: "?".to_string(),
            models: Vec::new(),
            mode: "?".to_string(),
            modes: Vec::new(),
            wire: WireState::Connecting,
            doctor_summary: None,
            todo_open: None,
        }
    }
}

impl Status {
    /// The one-line status bar content: model | mode | wire | session |
    /// doctor | commands (C2 adds the mode slot).
    pub fn line(&self) -> String {
        let session = self.session_id.as_deref().unwrap_or("none");
        // 12 chars leaves room for the mode slot; the commands hint trails
        // last so a narrow terminal clips hints, never state.
        let short_session: String = session.chars().take(12).collect();
        let doctor = self
            .doctor_summary
            .as_deref()
            .map(|s| format!(" | doctor: {s}"))
            .unwrap_or_default();
        let todo = self
            .todo_open
            .map(|n| format!(" | todo: {n} open"))
            .unwrap_or_default();
        format!(
            " {} | {} | {} | {}{}{} | /model /mode /plan /todo /status /doctor /compact /quit ",
            self.model, self.mode, self.wire, short_session, doctor, todo
        )
    }

    /// The multi-line `/status` transcript cell.
    pub fn report(&self) -> String {
        let mut lines = vec![
            format!("model:   {}", self.model),
            format!("mode:    {}", self.mode),
            format!("wire:    {}", self.wire),
            format!("session: {}", self.session_id.as_deref().unwrap_or("none")),
            format!(
                "catalog: {}",
                if self.models.is_empty() {
                    "(none advertised)".to_string()
                } else {
                    self.models
                        .iter()
                        .map(|(id, _)| id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
            format!(
                "modes:   {}",
                if self.modes.is_empty() {
                    "(none advertised)".to_string()
                } else {
                    self.modes
                        .iter()
                        .map(|(id, _)| id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        ];
        if let Some(summary) = &self.doctor_summary {
            lines.push(format!("doctor:  {summary}"));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_shows_model_and_wire() {
        let s = Status {
            model: "flux-auto".into(),
            mode: "default".into(),
            wire: WireState::Ready,
            session_id: Some("wayland-nano-session-recorded".into()),
            ..Default::default()
        };
        let line = s.line();
        assert!(line.contains("flux-auto"));
        assert!(line.contains("default"), "mode slot: {line}");
        assert!(line.contains("ready"));
    }

    #[test]
    fn status_line_carries_the_mode_and_mode_hint() {
        let s = Status {
            mode: "full_auto".into(),
            ..Default::default()
        };
        let line = s.line();
        assert!(line.contains("full_auto"), "{line}");
        assert!(line.contains("/mode"), "command hint: {line}");
    }

    #[test]
    fn report_includes_doctor_summary_when_present() {
        let mut s = Status::default();
        assert!(!s.report().contains("doctor:"));
        s.doctor_summary = Some("summary: 0 fail, 1 warn".into());
        assert!(s.report().contains("doctor:  summary: 0 fail, 1 warn"));
    }
}
