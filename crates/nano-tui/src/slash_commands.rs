//! Slash commands (composer prefix `/`): `/model`, `/mode`, `/plan`,
//! `/todo`, `/status`, `/doctor`, `/compact`, `/quit` — the v1 set (design
//! doc §4 + C10).
//!
//! `/status` and `/doctor` data path (normative, panel condition C1): both
//! run `wayland-nano doctor` as a SHORT-LIVED SUBPROCESS and render the
//! result; the TUI never links nano-cli and never reimplements doctor's
//! probes. `/compact` (C1) sends `session/compact` over the ACP wire — the
//! compaction itself runs engine-side in the acp-host. `/mode` (C2) opens a
//! picker over the advertised `availableModes` and sends `session/set_mode`;
//! the name matches the wire noun end-to-end (`/permissions` is reserved
//! for a future granular policy editor).

/// Parsed composer submission starting with `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// Open the model picker (advertised catalog → session/set_model).
    Model,
    /// Open the permission-mode picker (advertised modes → session/set_mode).
    Mode,
    /// Enter plan mode (C10): sends session/set_mode {modeId:"plan"} over
    /// the wire — the TUI is an ACP client, so this is NOT a local-only
    /// operation (the /model → session/set_model precedent). The host's ack
    /// carries the plan file path, printed for discoverability (Q5).
    Plan,
    /// Print the session todo list (C10), tracked from `todo` tool_call
    /// frames (no new wire affordance in v1).
    Todo,
    /// Model + wire state + doctor summary.
    Status,
    /// Full doctor output.
    Doctor,
    /// Compact the session context now (engine-side, journaled).
    Compact,
    /// Goal lifecycle mirror (C11): `/goal [status|pause|resume|cancel]` —
    /// sends the `_wayland/goal/*` ACP extension methods; the state machine
    /// lives engine-side. The payload is the subcommand (default "status").
    Goal(String),
    /// Shut down (cancel any turn, kill the host, restore the terminal).
    Quit,
    /// Unknown `/...` input — reported, never executed.
    Unknown(String),
}

/// Parse a submitted composer line. Returns `None` when the submission is
/// not a slash command (ordinary prompt text).
pub fn parse(text: &str) -> Option<SlashCommand> {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let command = trimmed.split_whitespace().next().unwrap_or(trimmed);
    Some(match command {
        "/model" => SlashCommand::Model,
        "/mode" => SlashCommand::Mode,
        "/plan" => SlashCommand::Plan,
        "/todo" => SlashCommand::Todo,
        "/status" => SlashCommand::Status,
        "/doctor" => SlashCommand::Doctor,
        "/compact" => SlashCommand::Compact,
        "/goal" => SlashCommand::Goal(
            trimmed
                .split_whitespace()
                .nth(1)
                .unwrap_or("status")
                .to_string(),
        ),
        "/quit" => SlashCommand::Quit,
        other => SlashCommand::Unknown(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_v1_set() {
        assert_eq!(parse("/model"), Some(SlashCommand::Model));
        assert_eq!(parse("/mode"), Some(SlashCommand::Mode));
        assert_eq!(parse("/plan"), Some(SlashCommand::Plan));
        assert_eq!(parse("/todo"), Some(SlashCommand::Todo));
        assert_eq!(parse("/status"), Some(SlashCommand::Status));
        assert_eq!(parse("/doctor"), Some(SlashCommand::Doctor));
        assert_eq!(parse("/compact"), Some(SlashCommand::Compact));
        assert_eq!(
            parse("/goal"),
            Some(SlashCommand::Goal("status".to_string()))
        );
        assert_eq!(
            parse("/goal pause"),
            Some(SlashCommand::Goal("pause".to_string()))
        );
        assert_eq!(parse("/quit"), Some(SlashCommand::Quit));
        assert_eq!(
            parse("/bogus"),
            Some(SlashCommand::Unknown("/bogus".into()))
        );
        // The reserved name stays UNKNOWN — it is not an alias for /mode.
        assert_eq!(
            parse("/permissions"),
            Some(SlashCommand::Unknown("/permissions".into()))
        );
    }

    #[test]
    fn non_slash_is_a_prompt() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("  indented"), None);
        assert_eq!(parse("path/to/file"), None);
    }

    #[test]
    fn slash_only_is_unknown() {
        assert_eq!(parse("/"), Some(SlashCommand::Unknown("/".into())));
    }
}
