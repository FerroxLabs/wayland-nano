//! Frame rendering (ratatui, alternate screen — design doc §3). No
//! markdown in v1: model output renders as plain wrapped text
//! (`Paragraph` + `Wrap`), already sanitized upstream.

use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::modal::ListSelectionView;
use crate::transcript::Cell;

const COMPOSER_MAX_HEIGHT: u16 = 10;

/// C7: kinds that render with the policy-denial glyph.
fn is_policy_denial(kind: nano_session::NanoErrorKind) -> bool {
    use nano_session::NanoErrorKind;
    matches!(
        kind,
        NanoErrorKind::FsReadDenied
            | NanoErrorKind::FsWriteDenied
            | NanoErrorKind::FsSensitiveDenied
            | NanoErrorKind::ApprovalDenied
            | NanoErrorKind::EgressDenied
    )
}

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let composer_height = (app.composer.lines().len() as u16 + 2).clamp(3, COMPOSER_MAX_HEIGHT);
    let [transcript_area, composer_area, status_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .areas(area);

    render_transcript(frame, app, transcript_area);
    render_composer(frame, app, composer_area);

    let status = Paragraph::new(Line::from(Span::styled(
        app.status.line(),
        Style::default().bg(Color::DarkGray),
    )));
    frame.render_widget(status, status_area);

    if let Some((kind, view, detail)) = app.modal_view() {
        render_modal(frame, kind, view, detail, area);
    }
}

fn render_transcript(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for cell in app.transcript.cells() {
        match cell {
            Cell::User(text) => {
                push_prefixed(&mut lines, "› ", text, Style::default().fg(Color::Cyan));
            }
            Cell::Assistant(text) => {
                for line in text.lines() {
                    lines.push(Line::from(line.to_string()));
                }
            }
            Cell::Tool {
                title,
                status,
                detail,
                ..
            } => {
                lines.push(Line::from(vec![
                    Span::styled("⚒ ", Style::default().fg(Color::Yellow)),
                    Span::styled(title.clone(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(format!(" [{status}]"), Style::default().fg(Color::Yellow)),
                ]));
                if !detail.is_empty() && detail != "null" {
                    lines.push(Line::from(Span::styled(
                        format!("  {detail}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            Cell::ToolResult { status, .. } => {
                lines.push(Line::from(Span::styled(
                    format!("  ↳ {status}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Cell::Error {
                title,
                hint,
                code_label,
                kind,
                retryable,
            } => {
                // C7: one icon per class — ⛔ policy-denial / ↻ retryable /
                // ✖ terminal. All text is static table/TUI presentation,
                // sanitized upstream.
                let icon = if kind.is_some_and(is_policy_denial) {
                    "⛔"
                } else if *retryable {
                    "↻"
                } else {
                    "✖"
                };
                let color = if *retryable {
                    Color::Yellow
                } else {
                    Color::Red
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{icon} "), Style::default().fg(color)),
                    Span::styled(
                        title.clone(),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  [{code_label}]"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
                if !hint.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("  {hint}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            Cell::Note(text) => {
                push_prefixed(&mut lines, "· ", text, Style::default().fg(Color::DarkGray));
            }
            // C10 §6: the human-facing diff — changed region with -/+
            // coloring (the TUI is the v1 renderer; Desktop's ACP adapter
            // preserves the block but has no diff renderer yet, §10).
            Cell::Diff {
                path,
                removed,
                added,
            } => {
                lines.push(Line::from(Span::styled(
                    format!("  ± {path} (+{}/-{})", added.len(), removed.len()),
                    Style::default().fg(Color::DarkGray),
                )));
                for line in removed {
                    lines.push(Line::from(Span::styled(
                        format!("    - {line}"),
                        Style::default().fg(Color::Red),
                    )));
                }
                for line in added {
                    lines.push(Line::from(Span::styled(
                        format!("    + {line}"),
                        Style::default().fg(Color::Green),
                    )));
                }
            }
        }
    }
    if let Some(active) = app.transcript.active_text() {
        for line in active.lines() {
            lines.push(Line::from(line.to_string()));
        }
        // Streaming cursor marker so a mid-stream cell reads as live.
        lines.push(Line::from(Span::styled(
            "▌",
            Style::default().fg(Color::Green),
        )));
    }

    // In-app transcript pager (alternate screen means no native scrollback,
    // design §3). Lines are pre-wrapped to the area width so the scroll
    // math is EXACT (Paragraph::scroll on Wrap counts word-wrapped rows,
    // which cannot be computed without reimplementing reflow). Lines that
    // fit keep their styling; longer lines flatten to plain wrapped chunks.
    let width = area.width.max(1) as usize;
    let mut physical: Vec<Line<'static>> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= width {
            physical.push(line);
        } else {
            let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let mut start = 0;
            // Char-boundary chunks: exact for ASCII (the common case); wide
            // chars may render past the last column and truncate visually —
            // cosmetic only, v1.
            let chars: Vec<char> = plain.chars().collect();
            while start < chars.len() {
                let end = (start + width).min(chars.len());
                let chunk: String = chars[start..end].iter().collect();
                physical.push(Line::from(chunk));
                start = end;
            }
        }
    }
    let total = physical.len();
    let visible = area.height as usize;
    let max_scroll = total.saturating_sub(visible);
    let scroll_up = app.transcript.scroll_up().min(max_scroll);
    let offset = max_scroll.saturating_sub(scroll_up) as u16;

    let paragraph = Paragraph::new(physical).scroll((offset, 0));
    frame.render_widget(paragraph, area);
}

fn push_prefixed(lines: &mut Vec<Line<'static>>, prefix: &str, text: &str, style: Style) {
    let mut first = true;
    for line in text.lines() {
        if first {
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), style),
                Span::raw(line.to_string()),
            ]));
            first = false;
        } else {
            lines.push(Line::from(format!("  {line}")));
        }
    }
    if text.is_empty() {
        lines.push(Line::from(Span::styled(prefix.to_string(), style)));
    }
}

fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" › ", Style::default().fg(Color::Green)));
    let inner = block.inner(area);
    let text: Vec<Line> = app
        .composer
        .lines()
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    frame.render_widget(Paragraph::new(text).block(block), area);

    // Terminal cursor tracks the composer unless a modal owns input.
    if !app.modal_open() {
        let (row, col) = app.composer.cursor();
        let visible_rows = inner.height as usize;
        if row < visible_rows {
            frame.set_cursor_position(ratatui::layout::Position::new(
                inner.x + col as u16,
                inner.y + row as u16,
            ));
        }
    }
}

/// The generic ListSelectionView surface: centered overlay, title bar,
/// optional detail line, scrolling rows.
fn render_modal(
    frame: &mut Frame,
    kind: &str,
    view: &ListSelectionView,
    detail: Option<&str>,
    area: Rect,
) {
    // F-10: each item renders as one name row plus one row per
    // description, so the modal height is computed over RENDERED rows,
    // not item indices.
    let rendered_rows: usize = view
        .items
        .iter()
        .map(|item| 1 + usize::from(item.description.is_some()))
        .sum();
    let rows = rendered_rows.max(1) as u16;
    let detail_rows = u16::from(detail.is_some());
    let height = (rows + 4 + detail_rows)
        .min(area.height.saturating_sub(2))
        .max(5);
    let width = area.width.saturating_sub(8).clamp(30, area.width);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {} ", view.title));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [detail_area, list_area, hint_area] = Layout::vertical([
        Constraint::Length(detail_rows),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    if let Some(detail) = detail {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                detail.to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            detail_area,
        );
    }

    let visible = list_area.height as usize;
    let selected = view.selected_index();
    // F-10: the scroll window is row-aware — an item's name row and its
    // description row scroll together, and the window always contains the
    // selected item's rows (previously the window was item-indexed, which
    // clipped two-row options such as the ask_user Dismiss row).
    let mut lines = Vec::new();
    let mut selected_start = 0usize;
    let mut selected_rows = 1usize;
    for (index, item) in view.items.iter().enumerate() {
        if index == selected {
            selected_start = lines.len();
        }
        let selected_row = view.has_selection() && index == selected;
        let marker = if selected_row { "› " } else { "  " };
        let name_style = if selected_row {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(marker, Style::default().fg(Color::Green)),
            Span::styled(item.name.clone(), name_style),
        ];
        if item.is_current {
            spans.push(Span::styled(
                " (current)",
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
        if let Some(description) = &item.description {
            lines.push(Line::from(Span::styled(
                format!("    {description}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if index == selected {
            selected_rows = lines.len() - selected_start;
        }
    }
    let start_row = if selected_rows >= visible {
        // The selected item alone fills/exceeds the viewport: show its
        // first row.
        selected_start
    } else {
        (selected_start + selected_rows).saturating_sub(visible)
    };
    let lines: Vec<Line> = lines.into_iter().skip(start_row).take(visible).collect();
    frame.render_widget(Paragraph::new(lines), list_area);

    let hint = match kind {
        "approval" => " Enter decides · Esc denies ",
        "session" => " Move to select · Enter resumes · Esc cancels ",
        _ => " Enter selects · Esc cancels ",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        hint_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal::ListItem;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn two_row_option(id: &str, name: &str) -> ListItem {
        ListItem {
            id: id.into(),
            name: name.into(),
            description: Some("kind".into()),
            is_current: false,
        }
    }

    fn question_view() -> ListSelectionView {
        // The F-10 scenario: 3 minted options + Dismiss, each rendered as
        // TWO rows (name + kind).
        ListSelectionView::new(
            "Pick a color?",
            vec![
                two_row_option("opt_0", "Red"),
                two_row_option("opt_1", "Green"),
                two_row_option("opt_2", "Blue"),
                two_row_option("reject", "Dismiss"),
            ],
        )
    }

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        let mut text = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    /// F-10 regression: in a short terminal the viewport must scroll by
    /// rendered rows so the selected last option (with its two rows) is
    /// visible. Previously the window was item-indexed and the Dismiss row
    /// was clipped out even when selected.
    #[test]
    fn modal_viewport_keeps_selected_two_row_option_visible() {
        let mut view = question_view();
        for _ in 0..3 {
            view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("terminal");
        terminal
            .draw(|frame| render_modal(frame, "question", &view, None, frame.area()))
            .expect("draw");
        let screen = screen_text(&terminal);
        assert!(screen.contains("Dismiss"), "selected option: {screen}");
        assert!(
            screen.contains("› Dismiss"),
            "selection marker on the last option: {screen}"
        );
    }

    /// F-10 companion: the modal height is computed over rendered rows, so
    /// in a tall terminal all four two-row options render without clipping.
    #[test]
    fn modal_height_counts_rendered_rows() {
        let view = question_view();
        let mut terminal = Terminal::new(TestBackend::new(60, 30)).expect("terminal");
        terminal
            .draw(|frame| render_modal(frame, "question", &view, None, frame.area()))
            .expect("draw");
        let screen = screen_text(&terminal);
        for name in ["Red", "Green", "Blue", "Dismiss"] {
            assert!(screen.contains(name), "{name} visible: {screen}");
        }
    }
}
