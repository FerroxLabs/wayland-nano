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
    pub wire: WireState,
    /// Last doctor run's summary line (`summary: N fail, M warn`), if any.
    pub doctor_summary: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            session_id: None,
            model: "?".to_string(),
            models: Vec::new(),
            wire: WireState::Connecting,
            doctor_summary: None,
        }
    }
}

impl Status {
    /// The one-line status bar content.
    pub fn line(&self) -> String {
        let session = self.session_id.as_deref().unwrap_or("none");
        let short_session: String = session.chars().take(28).collect();
        let doctor = self
            .doctor_summary
            .as_deref()
            .map(|s| format!(" | doctor: {s}"))
            .unwrap_or_default();
        format!(
            " {} | {} | {}{} | /model /status /doctor /quit ",
            self.model, self.wire, short_session, doctor
        )
    }

    /// The multi-line `/status` transcript cell.
    pub fn report(&self) -> String {
        let mut lines = vec![
            format!("model:   {}", self.model),
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
            wire: WireState::Ready,
            session_id: Some("wayland-nano-session-recorded".into()),
            ..Default::default()
        };
        let line = s.line();
        assert!(line.contains("flux-auto"));
        assert!(line.contains("ready"));
    }

    #[test]
    fn report_includes_doctor_summary_when_present() {
        let mut s = Status::default();
        assert!(!s.report().contains("doctor:"));
        s.doctor_summary = Some("summary: 0 fail, 1 warn".into());
        assert!(s.report().contains("doctor:  summary: 0 fail, 1 warn"));
    }
}
