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

/// The latest rate-limit snapshot (C9 §5): optional fields render
/// individually; "unknown" on absence, never an interpolated guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitView {
    pub requests_remaining: Option<u64>,
    pub requests_limit: Option<u64>,
    pub tokens_remaining: Option<u64>,
    pub tokens_limit: Option<u64>,
}

impl RateLimitView {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.requests_remaining.is_some() || self.requests_limit.is_some() {
            parts.push(format!(
                "req {}/{}",
                self.requests_remaining
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                self.requests_limit
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into())
            ));
        }
        if self.tokens_remaining.is_some() || self.tokens_limit.is_some() {
            parts.push(format!(
                "tok {}/{}",
                self.tokens_remaining
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into()),
                self.tokens_limit
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into())
            ));
        }
        if parts.is_empty() {
            "unknown".to_string()
        } else {
            parts.join(" ")
        }
    }
}

/// The session meter's status payload (P1 §5), rendered with the
/// cost-truth honesty rule: NEVER `$0.000` for an unpriced provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetView {
    pub session_tokens: u64,
    pub microcents: u64,
    pub priced: bool,
    /// (limit, observed) when a session cap is configured.
    pub cap: Option<(u64, u64)>,
}

impl BudgetView {
    /// `Σ 12.3k tok | $0.041 est` when priced, `Σ 12.3k tok | unpriced`
    /// when not; the cap position appends as `(90/100)` when configured.
    pub fn summary(&self) -> String {
        let cost = if self.priced {
            // Integer microcents → dollars (1 microcent = 1e-8 USD); the
            // "est." label keeps the static-table honesty posture (D10).
            format!("${:.3} est", self.microcents as f64 / 100_000_000.0)
        } else {
            "unpriced".to_string()
        };
        let cap = self
            .cap
            .map(|(limit, observed)| format!(" ({observed}/{limit})"))
            .unwrap_or_default();
        format!(
            "Σ {} tok | {cost}{cap}",
            humanize_tokens(self.session_tokens)
        )
    }
}

/// Compact token counts for the status line: 12.3k / 1.2M style.
fn humanize_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
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
    /// C9 §3.4: steers queued but not yet drained/dropped (the pending-
    /// input indicator).
    pub pending_steers: usize,
    /// C9 §2.2: (attempt, next_delay_ms, deadline_remaining_ms) while the
    /// host is in a reconnect sleep; cleared when the turn ends.
    pub reconnect: Option<(u64, u64, u64)>,
    /// C9 §5: the latest coalesced rate-limit snapshot, if any.
    pub rate_limit: Option<RateLimitView>,
    /// P1 §5: the session meter's budget/cost view (same optional-slot
    /// pattern as `rate_limit`).
    pub budget: Option<BudgetView>,
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
            pending_steers: 0,
            reconnect: None,
            rate_limit: None,
            budget: None,
        }
    }
}

impl Status {
    /// The one-line status bar content: model | mode | wire | session |
    /// doctor | commands (C2 adds the mode slot). C9 slots (reconnect
    /// banner, pending steers, rate limits) append before the hints.
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
        let reconnect = self
            .reconnect
            .map(|(attempt, next_delay_ms, _)| {
                format!(
                    " | reconnecting (attempt {attempt}, next {}s)",
                    next_delay_ms / 1000
                )
            })
            .unwrap_or_default();
        let steers = if self.pending_steers > 0 {
            format!(" | {} steer(s) queued", self.pending_steers)
        } else {
            String::new()
        };
        let rate_limit = self
            .rate_limit
            .as_ref()
            .map(|r| format!(" | rl: {}", r.summary()))
            .unwrap_or_default();
        let budget = self
            .budget
            .as_ref()
            .map(|b| format!(" | {}", b.summary()))
            .unwrap_or_default();
        format!(
            " {} | {} | {} | {}{}{}{}{}{}{} | /model /mode /plan /todo /status /doctor /sessions /resume /compact /budget /quit ",
            self.model,
            self.mode,
            self.wire,
            short_session,
            doctor,
            todo,
            reconnect,
            steers,
            rate_limit,
            budget
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
        // C9: "unknown" on absence — never an interpolated guess.
        lines.push(format!(
            "ratelimit: {}",
            self.rate_limit
                .as_ref()
                .map(RateLimitView::summary)
                .unwrap_or_else(|| "unknown".to_string())
        ));
        // P1 §5: same honesty rule as the status line — never a fake $0.
        lines.push(format!(
            "budget:   {}",
            self.budget
                .as_ref()
                .map(BudgetView::summary)
                .unwrap_or_else(|| "unknown".to_string())
        ));
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

#[cfg(test)]
mod c9_tests {
    use super::*;

    #[test]
    fn rate_limit_renders_unknown_on_absence_never_a_guess() {
        let status = Status::default();
        assert!(status.report().contains("ratelimit: unknown"));
        assert!(!status.line().contains("rl:"));
    }

    #[test]
    fn partial_snapshot_renders_partial_fields() {
        let view = RateLimitView {
            requests_remaining: Some(90),
            requests_limit: Some(100),
            tokens_remaining: None,
            tokens_limit: None,
        };
        assert_eq!(view.summary(), "req 90/100");
        let empty = RateLimitView {
            requests_remaining: None,
            requests_limit: None,
            tokens_remaining: None,
            tokens_limit: None,
        };
        assert_eq!(empty.summary(), "unknown");
    }

    #[test]
    fn reconnect_banner_and_pending_steers_render() {
        let status = Status {
            reconnect: Some((2, 10_000, 280_000)),
            pending_steers: 2,
            ..Default::default()
        };
        let line = status.line();
        assert!(
            line.contains("reconnecting (attempt 2, next 10s)"),
            "{line}"
        );
        assert!(line.contains("2 steer(s) queued"), "{line}");
    }
}

#[cfg(test)]
mod p1_budget_tests {
    use super::*;

    /// P1 §5 honesty rule (test-asserted per design §8): `priced: false`
    /// renders the literal `unpriced`, NEVER `$0.000`.
    #[test]
    fn unpriced_renders_the_literal_never_zero_dollars() {
        let view = BudgetView {
            session_tokens: 12_300,
            microcents: 0,
            priced: false,
            cap: None,
        };
        let summary = view.summary();
        assert!(summary.contains("unpriced"), "{summary}");
        assert!(!summary.contains("$0.000"), "{summary}");
        assert!(summary.contains("Σ 12.3k tok"), "{summary}");
    }

    /// Priced sessions render dollars with the "est." honesty label.
    #[test]
    fn priced_renders_dollars_with_the_est_label() {
        let view = BudgetView {
            session_tokens: 999,
            microcents: 4_100_000, // $0.041
            priced: true,
            cap: None,
        };
        let summary = view.summary();
        assert!(summary.contains("$0.041 est"), "{summary}");
        assert!(summary.contains("Σ 999 tok"), "{summary}");
        // Known-free ($0 real, priced) is distinct from unpriced.
        let free = BudgetView {
            session_tokens: 1,
            microcents: 0,
            priced: true,
            cap: None,
        };
        assert!(free.summary().contains("$0.000 est"), "{}", free.summary());
        assert!(!free.summary().contains("unpriced"));
    }

    /// The cap position renders (observed/limit); the status line and the
    /// /status report carry the budget slot.
    #[test]
    fn cap_position_and_status_slots_render() {
        let mut status = Status::default();
        assert!(status.report().contains("budget:   unknown"));
        assert!(!status.line().contains("Σ"));
        status.budget = Some(BudgetView {
            session_tokens: 1_500_000,
            microcents: 0,
            priced: false,
            cap: Some((100, 90)),
        });
        let line = status.line();
        assert!(line.contains("Σ 1.5M tok | unpriced (90/100)"), "{line}");
        assert!(line.contains("/budget"), "command hint: {line}");
        assert!(
            status
                .report()
                .contains("budget:   Σ 1.5M tok | unpriced (90/100)"),
            "{}",
            status.report()
        );
    }
}
