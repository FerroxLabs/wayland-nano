//! The composer: a single multi-line editor (design doc §4).
//!
//! Keymap (v1): Enter sends, Alt+Enter / Ctrl+J inserts a newline, arrows /
//! Home / End move, Backspace / Delete edit. Bracketed paste inserts the
//! paste verbatim after control-char rejection (paste bombs: C0/C1/ESC are
//! stripped by the same full-family sanitizer that guards rendered output).

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::sanitize;

/// What a key meant to the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerAction {
    /// The key was consumed as an edit/navigation; nothing else to do.
    Handled,
    /// Enter (plain): submit the current text.
    Submit,
}

/// Multi-line plain-text editor. Lines are stored without `\n`; the cursor
/// is a (row, char-column) pair, always valid.
/// Byte offset of `char_col` within `line` (end of string past the last char).
fn byte_index(line: &str, char_col: usize) -> usize {
    line.char_indices()
        .nth(char_col)
        .map(|(i, _)| i)
        .unwrap_or(line.len())
}

#[derive(Debug, Default)]
pub struct Composer {
    lines: Vec<String>,
    row: usize,
    col: usize, // in chars, not bytes
}

impl Composer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    /// The full text, lines joined with `\n`.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Current contents for rendering.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Cursor as (row, col-in-chars) for the renderer.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Take the text for submission, resetting to one empty line.
    pub fn take_submission(&mut self) -> String {
        let text = self.text();
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
        text
    }

    /// Handle a key. Returns [`ComposerAction::Submit`] on plain Enter.
    pub fn handle_key(&mut self, key: KeyEvent) -> ComposerAction {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter if alt || ctrl => {
                // Alt+Enter / Ctrl+Enter: newline.
                self.insert_newline();
                ComposerAction::Handled
            }
            KeyCode::Enter => ComposerAction::Submit,
            // Ctrl+J is the classic newline insert (arrives as Char('j')+CTRL
            // on some terminals, as a raw \n key on others).
            KeyCode::Char('j') if ctrl => {
                self.insert_newline();
                ComposerAction::Handled
            }
            KeyCode::Char(c) => {
                self.insert_char(c);
                ComposerAction::Handled
            }
            KeyCode::Backspace => {
                self.backspace();
                ComposerAction::Handled
            }
            KeyCode::Delete => {
                self.delete_forward();
                ComposerAction::Handled
            }
            KeyCode::Left => {
                self.move_left();
                ComposerAction::Handled
            }
            KeyCode::Right => {
                self.move_right();
                ComposerAction::Handled
            }
            KeyCode::Up => {
                self.move_up();
                ComposerAction::Handled
            }
            KeyCode::Down => {
                self.move_down();
                ComposerAction::Handled
            }
            KeyCode::Home => {
                self.col = 0;
                ComposerAction::Handled
            }
            KeyCode::End => {
                self.col = self.current_line_len();
                ComposerAction::Handled
            }
            _ => ComposerAction::Handled,
        }
    }

    /// Insert a bracketed paste. The paste is sanitized one-shot (control
    /// chars rejected — paste-bomb mitigation, design doc §8): only `\n`
    /// and `\t` survive, and no escape sequence can land in the buffer.
    pub fn insert_paste(&mut self, text: &str) {
        let clean = sanitize::sanitize(text);
        for c in clean.chars() {
            if c == '\n' {
                self.insert_newline();
            } else {
                self.insert_char(c);
            }
        }
    }

    fn current_line_len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    fn insert_char(&mut self, c: char) {
        // Never let a control char reach the buffer, even via a code path
        // that skipped sanitization. `\t` is the one allowed control (D2's
        // allowed set); typed control keys arrive as KeyCode, not Char.
        if c.is_control() && c != '\t' {
            return;
        }
        let line = &mut self.lines[self.row];
        let idx = byte_index(line, self.col);
        line.insert(idx, c);
        self.col += 1;
    }

    fn insert_newline(&mut self) {
        let line = &mut self.lines[self.row];
        let idx = byte_index(line, self.col);
        let tail = line.split_off(idx);
        self.lines.insert(self.row + 1, tail);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let idx = byte_index(line, self.col - 1);
            let end = byte_index(line, self.col);
            line.replace_range(idx..end, "");
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.current_line_len();
            self.lines[self.row].push_str(&line);
        }
    }

    fn delete_forward(&mut self) {
        if self.col < self.current_line_len() {
            let line = &mut self.lines[self.row];
            let idx = byte_index(line, self.col);
            let end = byte_index(line, self.col + 1);
            line.replace_range(idx..end, "");
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.current_line_len();
        }
    }

    fn move_right(&mut self) {
        if self.col < self.current_line_len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.current_line_len());
        }
    }

    fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.current_line_len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn type_str(composer: &mut Composer, text: &str) {
        for c in text.chars() {
            composer.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn enter_submits_alt_enter_newlines() {
        let mut c = Composer::new();
        type_str(&mut c, "hello");
        assert_eq!(c.handle_key(alt(KeyCode::Enter)), ComposerAction::Handled);
        type_str(&mut c, "world");
        assert_eq!(c.text(), "hello\nworld");
        assert_eq!(c.handle_key(key(KeyCode::Enter)), ComposerAction::Submit);
        assert_eq!(c.take_submission(), "hello\nworld");
        assert!(c.is_empty());
    }

    #[test]
    fn ctrl_j_newlines() {
        let mut c = Composer::new();
        type_str(&mut c, "a");
        c.handle_key(ctrl(KeyCode::Char('j')));
        type_str(&mut c, "b");
        assert_eq!(c.text(), "a\nb");
    }

    #[test]
    fn backspace_merges_lines() {
        let mut c = Composer::new();
        type_str(&mut c, "ab");
        c.handle_key(alt(KeyCode::Enter));
        type_str(&mut c, "cd");
        c.handle_key(key(KeyCode::Left));
        c.handle_key(key(KeyCode::Left));
        c.handle_key(key(KeyCode::Backspace)); // join
        assert_eq!(c.text(), "abcd");
        assert_eq!(c.cursor(), (0, 2));
        c.handle_key(key(KeyCode::Backspace));
        assert_eq!(c.text(), "acd");
    }

    #[test]
    fn delete_forward_merges_next_line() {
        let mut c = Composer::new();
        type_str(&mut c, "ab");
        c.handle_key(alt(KeyCode::Enter));
        type_str(&mut c, "cd");
        c.handle_key(key(KeyCode::Up));
        c.handle_key(key(KeyCode::End));
        c.handle_key(key(KeyCode::Delete));
        assert_eq!(c.text(), "abcd");
    }

    #[test]
    fn cursor_clamps_on_vertical_moves() {
        let mut c = Composer::new();
        type_str(&mut c, "long line");
        c.handle_key(alt(KeyCode::Enter));
        type_str(&mut c, "s");
        c.handle_key(key(KeyCode::Up));
        assert_eq!(c.cursor(), (0, 1));
    }

    #[test]
    fn paste_strips_control_chars_and_escapes() {
        let mut c = Composer::new();
        // Paste bomb: CSI + OSC + raw C0 controls must not land in the
        // buffer; newlines and tabs survive as real edits.
        c.insert_paste("rm -rf /\x1b[2J\x1b]0;pwned\x07\nnext\x00line\ttab");
        assert_eq!(c.text(), "rm -rf /\nnextline\ttab");
    }

    #[test]
    fn typed_control_chars_are_rejected() {
        let mut c = Composer::new();
        // KeyCode::Char of a control char (some terminals deliver these).
        c.handle_key(key(KeyCode::Char('\u{1b}')));
        c.handle_key(key(KeyCode::Char('\u{7}')));
        assert!(c.is_empty());
    }

    #[test]
    fn unicode_editing_is_char_based() {
        let mut c = Composer::new();
        type_str(&mut c, "héllo");
        c.handle_key(key(KeyCode::Left));
        c.handle_key(key(KeyCode::Left));
        c.handle_key(key(KeyCode::Backspace)); // delete 'l' before cursor
        assert_eq!(c.text(), "hélo");
    }
}
