//! Transcript: committed history cells + one mutable active cell during
//! streaming (design doc §4, `chatwidget.rs:1-17` pattern). Commit pacing is
//! commit-on-arrival — a chunk joins the active cell; any other frame (tool
//! card, turn end, note) commits it. No smooth/catch-up gears in v1.
//!
//! All text entering cells passes through the streaming-safe sanitizer
//! (§5/D2): the active streaming cell owns one [`Sanitizer`] whose state
//! carries across ACP chunk boundaries.

use crate::sanitize::Sanitizer;

/// One committed entry in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// A user prompt (echoed at submit, or replayed on session/load).
    User(String),
    /// Finished assistant text.
    Assistant(String),
    /// A tool call card; `status` is the wire status (in_progress,
    /// completed, failed). `detail` is the sanitized rawInput (one line).
    Tool {
        call_id: String,
        title: String,
        status: String,
        detail: String,
    },
    /// A tool completion update (digest on replay, output live).
    ToolResult { call_id: String, status: String },
    /// A file diff (C10 §6): the changed region with common prefix/suffix
    /// trimmed — `removed`/`added` lines, rendered with -/+ coloring.
    Diff {
        path: String,
        removed: Vec<String>,
        added: Vec<String>,
    },
    /// A typed error cell (C7): table title + actionable hint, icon class
    /// from `kind`/`retryable` at render time (✖ terminal / ↻ retryable /
    /// ⛔ policy-denial). Text is ALWAYS static table presentation or
    /// TUI-side static text — never raw wire strings.
    Error {
        title: String,
        hint: String,
        code_label: String,
        kind: Option<nano_session::NanoErrorKind>,
        retryable: bool,
    },
    /// A TUI-side note: /status, /doctor, errors, lifecycle messages.
    Note(String),
}

/// The mutable streaming cell: chunks append through the carried sanitizer.
#[derive(Debug, Default)]
struct ActiveCell {
    text: String,
    sanitizer: Sanitizer,
}

#[derive(Debug, Default)]
pub struct Transcript {
    cells: Vec<Cell>,
    active: Option<ActiveCell>,
    /// Lines scrolled up from the bottom (in-app pager, PgUp/PgDn). 0 = tail.
    scroll_up: usize,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The active cell's sanitized-so-far text (for rendering).
    pub fn active_text(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.text.as_str())
    }

    pub fn scroll_up(&self) -> usize {
        self.scroll_up
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll_up = self.scroll_up.saturating_add_signed(delta);
    }

    /// Any new content returns the view to the tail (codex behavior).
    pub fn follow_tail(&mut self) {
        self.scroll_up = 0;
    }

    /// A streamed assistant chunk arrived. Commit-on-arrival: joins the
    /// active cell (sanitized through the carried state).
    pub fn push_agent_chunk(&mut self, chunk: &str) {
        self.follow_tail();
        let active = self.active.get_or_insert_with(ActiveCell::default);
        let clean = active.sanitizer.push(chunk);
        active.text.push_str(&clean);
    }

    /// Commit the active cell (turn end, or before a non-chunk frame).
    /// End-of-stream: any unterminated escape pending in the sanitizer is
    /// dropped, never forwarded.
    pub fn commit_active(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.sanitizer.finish();
            if !active.text.is_empty() {
                self.cells.push(Cell::Assistant(active.text));
            }
        }
    }

    pub fn push_user(&mut self, text: &str) {
        self.commit_active();
        self.follow_tail();
        self.cells.push(Cell::User(crate::sanitize::sanitize(text)));
    }

    pub fn push_tool_call(&mut self, call_id: &str, title: &str, status: &str, raw_input: &str) {
        self.commit_active();
        self.follow_tail();
        self.cells.push(Cell::Tool {
            call_id: call_id.to_string(),
            title: crate::sanitize::sanitize(title),
            status: status.to_string(),
            detail: crate::sanitize::sanitize(raw_input),
        });
    }

    pub fn push_tool_result(
        &mut self,
        call_id: &str,
        status: &str,
        raw_output: &str,
        nano_error: Option<crate::acp_client::NanoErrorPayload>,
    ) {
        self.commit_active();
        self.follow_tail();
        // Update the matching card's status so it never hangs in_progress.
        for cell in self.cells.iter_mut().rev() {
            if let Cell::Tool {
                call_id: id,
                status: s,
                ..
            } = cell
                && id == call_id
            {
                *s = status.to_string();
                break;
            }
        }
        // C7: a typed failure renders the table presentation (title — hint)
        // instead of the meaningless `failed: len:N` digest.
        if let Some(payload) = nano_error {
            let presentation = nano_session::error_codes::error_presentation(payload.kind);
            self.cells.push(Cell::ToolResult {
                call_id: call_id.to_string(),
                status: format!("{status}: {}", crate::sanitize::sanitize(&presentation)),
            });
            return;
        }
        let output = crate::sanitize::sanitize(raw_output);
        if !output.is_empty() {
            self.cells.push(Cell::ToolResult {
                call_id: call_id.to_string(),
                status: format!("{status}: {output}"),
            });
        }
    }

    /// C10 §6: a diff content block from the wire. The model-facing
    /// rawOutput stays terse; this is the human-facing review surface.
    /// Renders the changed region only (common prefix/suffix trimmed),
    /// bounded to 30 displayed lines with an elision marker.
    pub fn push_tool_diff(&mut self, path: &str, old_text: Option<&str>, new_text: &str) {
        self.commit_active();
        self.follow_tail();
        let old: Vec<String> = old_text
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default();
        let new: Vec<String> = new_text.lines().map(str::to_string).collect();
        // Trim the common prefix and suffix: the diff is the middle.
        let mut start = 0;
        while start < old.len() && start < new.len() && old[start] == new[start] {
            start += 1;
        }
        let mut end = 0;
        while end < old.len() - start
            && end < new.len() - start
            && old[old.len() - 1 - end] == new[new.len() - 1 - end]
        {
            end += 1;
        }
        let mut removed: Vec<String> = old[start..old.len() - end].to_vec();
        let mut added: Vec<String> = new[start..new.len() - end].to_vec();
        const MAX_REGION: usize = 30;
        if removed.len() + added.len() > MAX_REGION {
            let elided = removed.len() + added.len() - MAX_REGION;
            removed.truncate(MAX_REGION / 2);
            added.truncate(MAX_REGION / 2);
            added.push(format!("…[{elided} more lines elided]"));
        }
        let clean = |lines: &[String]| {
            lines
                .iter()
                .map(|l| crate::sanitize::sanitize(l))
                .collect::<Vec<_>>()
        };
        self.cells.push(Cell::Diff {
            path: crate::sanitize::sanitize(path),
            removed: clean(&removed),
            added: clean(&added),
        });
    }

    /// A typed error cell (C7). All text passes the fail-closed sanitizer —
    /// even though callers only pass static table/TUI strings.
    pub fn push_error(
        &mut self,
        title: &str,
        hint: &str,
        code_label: &str,
        kind: Option<nano_session::NanoErrorKind>,
        retryable: bool,
    ) {
        self.commit_active();
        self.follow_tail();
        self.cells.push(Cell::Error {
            title: crate::sanitize::sanitize(title),
            hint: crate::sanitize::sanitize(hint),
            code_label: crate::sanitize::sanitize(code_label),
            kind,
            retryable,
        });
    }

    pub fn push_note(&mut self, text: &str) {
        self.commit_active();
        self.follow_tail();
        self.cells.push(Cell::Note(crate::sanitize::sanitize(text)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_join_active_then_commit() {
        let mut t = Transcript::new();
        t.push_agent_chunk("Hello, ");
        t.push_agent_chunk("world");
        assert_eq!(t.active_text(), Some("Hello, world"));
        t.commit_active();
        assert_eq!(t.active_text(), None);
        assert_eq!(t.cells(), &[Cell::Assistant("Hello, world".to_string())]);
    }

    #[test]
    fn non_chunk_frame_commits_active_first() {
        let mut t = Transcript::new();
        t.push_agent_chunk("partial answer");
        t.push_tool_call("c1", "fs_write", "in_progress", "{}");
        assert_eq!(t.active_text(), None);
        assert!(matches!(t.cells()[0], Cell::Assistant(_)));
        assert!(matches!(t.cells()[1], Cell::Tool { .. }));
    }

    #[test]
    fn streaming_sanitizer_carries_state_across_chunks() {
        let mut t = Transcript::new();
        t.push_agent_chunk("safe \x1b[38;5");
        t.push_agent_chunk(";9m more");
        t.commit_active();
        assert_eq!(t.cells(), &[Cell::Assistant("safe  more".to_string())]);
    }

    #[test]
    fn tool_result_updates_card_status() {
        let mut t = Transcript::new();
        t.push_tool_call("c1", "shell", "in_progress", "{}");
        t.push_tool_result("c1", "completed", "ok", None);
        match &t.cells()[0] {
            Cell::Tool { status, .. } => assert_eq!(status, "completed"),
            other => panic!("expected tool cell, got {other:?}"),
        }
    }

    /// C7: a typed failure shows the table presentation, not the digest.
    #[test]
    fn typed_tool_failure_renders_presentation_not_digest() {
        let mut t = Transcript::new();
        t.push_tool_call("c1", "fs_write", "in_progress", "{}");
        t.push_tool_result(
            "c1",
            "failed",
            "len:21",
            Some(crate::acp_client::NanoErrorPayload {
                kind: nano_session::NanoErrorKind::ApprovalDenied,
                retryable: false,
            }),
        );
        match &t.cells()[1] {
            Cell::ToolResult { status, .. } => assert_eq!(status, "failed: Denied by user"),
            other => panic!("expected tool result cell, got {other:?}"),
        }
    }

    /// C7: error cells sanitize their text (defense in depth — the inputs
    /// are static strings, but the sanitizer contract holds for every cell).
    #[test]
    fn error_cell_text_is_sanitized() {
        let mut t = Transcript::new();
        t.push_error(
            "Bogus \x1b[31mtitle",
            "hint \x1b]0;x\x07",
            "-32603",
            None,
            false,
        );
        match &t.cells()[0] {
            Cell::Error {
                title,
                hint,
                code_label,
                retryable,
                ..
            } => {
                assert_eq!(title, "Bogus title");
                assert_eq!(hint, "hint ");
                assert_eq!(code_label, "-32603");
                assert!(!retryable);
            }
            other => panic!("expected error cell, got {other:?}"),
        }
    }

    #[test]
    fn torn_replay_leaves_no_hanging_card_visible_text() {
        // A tool_call whose completion never arrived (crash mid-call on the
        // engine side) renders as-is without panic.
        let mut t = Transcript::new();
        t.push_user("do a thing");
        t.push_tool_call("c9", "shell", "in_progress", "{\"command\":\"ls\"}");
        t.commit_active();
        assert_eq!(t.cells().len(), 2);
    }

    #[test]
    fn diff_renders_the_changed_region() {
        let mut t = Transcript::new();
        t.push_tool_diff(
            "src/main.rs",
            Some(
                "fn main() {
    old();
}
",
            ),
            "fn main() {
    new();
}
",
        );
        match &t.cells()[0] {
            Cell::Diff {
                path,
                removed,
                added,
            } => {
                assert_eq!(path, "src/main.rs");
                assert_eq!(removed, &["    old();"]);
                assert_eq!(added, &["    new();"]);
            }
            other => panic!("expected diff cell, got {other:?}"),
        }
        // Whole-file add: no removed lines.
        let mut t = Transcript::new();
        t.push_tool_diff(
            "new.rs", None, "a
b
",
        );
        match &t.cells()[0] {
            Cell::Diff { removed, added, .. } => {
                assert!(removed.is_empty());
                assert_eq!(added.len(), 2);
            }
            other => panic!("expected diff cell, got {other:?}"),
        }
    }

    #[test]
    fn scroll_is_bounded_below() {
        let mut t = Transcript::new();
        t.scroll_by(-10);
        assert_eq!(t.scroll_up(), 0);
        t.scroll_by(5);
        assert_eq!(t.scroll_up(), 5);
        t.push_note("x");
        assert_eq!(t.scroll_up(), 0, "new content returns to tail");
    }
}
