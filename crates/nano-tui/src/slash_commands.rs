//! Slash commands (composer prefix `/`): `/model`, `/status`, `/doctor`,
//! `/compact`, `/quit` — the v1 set (design doc §4).
//!
//! `/status` and `/doctor` data path (normative, panel condition C1): both
//! run `wayland-nano doctor` as a SHORT-LIVED SUBPROCESS and render the
//! result; the TUI never links nano-cli and never reimplements doctor's
//! probes. `/compact` (C1) sends `session/compact` over the ACP wire — the
//! compaction itself runs engine-side in the acp-host.

/// Parsed composer submission starting with `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// Open the model picker (advertised catalog → session/set_model).
    Model,
    /// Model + wire state + doctor summary.
    Status,
    /// Full doctor output.
    Doctor,
    /// Compact the session context now (engine-side, journaled).
    Compact,
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
        "/status" => SlashCommand::Status,
        "/doctor" => SlashCommand::Doctor,
        "/compact" => SlashCommand::Compact,
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
        assert_eq!(parse("/status"), Some(SlashCommand::Status));
        assert_eq!(parse("/doctor"), Some(SlashCommand::Doctor));
        assert_eq!(parse("/compact"), Some(SlashCommand::Compact));
        assert_eq!(parse("/quit"), Some(SlashCommand::Quit));
        assert_eq!(
            parse("/bogus"),
            Some(SlashCommand::Unknown("/bogus".into()))
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
