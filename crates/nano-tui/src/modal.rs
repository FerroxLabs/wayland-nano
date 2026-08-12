//! One generic list-selection modal (design doc §4, the
//! `bottom_pane/list_selection_view.rs` pattern) reused for the approval
//! dialog and the model picker.
//!
//! Approval contract (`approval_overlay.rs` pattern): a selection ALWAYS
//! emits an explicit decision event — there is no TUI-side auto-approve
//! (panel condition C5); Esc maps to the caller-supplied cancel choice
//! (deny, for approvals).

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;

/// One row in the modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// Stable identifier handed back on selection (wire optionId / modelId).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Optional second line (dimmed).
    pub description: Option<String>,
    /// Rendered as the current/highlighted entry.
    pub is_current: bool,
}

/// Outcome of feeding a key to the modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalOutcome {
    /// Still open.
    Open,
    /// Enter chose an item (its `id`).
    Selected(String),
    /// Esc cancelled.
    Cancelled,
}

/// Generic selection list with scrolling.
#[derive(Debug, Clone)]
pub struct ListSelectionView {
    pub title: String,
    pub items: Vec<ListItem>,
    selected: usize,
}

impl ListSelectionView {
    pub fn new(title: impl Into<String>, items: Vec<ListItem>) -> Self {
        let selected = items.iter().position(|i| i.is_current).unwrap_or(0);
        Self {
            title: title.into(),
            items,
            selected,
        }
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ModalOutcome::Open
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1).min(self.items.len() - 1);
                }
                ModalOutcome::Open
            }
            KeyCode::Home => {
                self.selected = 0;
                ModalOutcome::Open
            }
            KeyCode::End => {
                if !self.items.is_empty() {
                    self.selected = self.items.len() - 1;
                }
                ModalOutcome::Open
            }
            KeyCode::Enter => match self.items.get(self.selected) {
                Some(item) => ModalOutcome::Selected(item.id.clone()),
                None => ModalOutcome::Cancelled,
            },
            KeyCode::Esc => ModalOutcome::Cancelled,
            _ => ModalOutcome::Open,
        }
    }
}

/// The approval dialog's normalized request model (design doc §4: title +
/// rawInput + allow/deny options from the wire).
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// The JSON-RPC id of the session/request_permission request — the
    /// response is keyed by exactly this id.
    pub request_id: u64,
    pub title: String,
    pub raw_input: String,
    pub options: Vec<ListItem>,
}

impl ApprovalRequest {
    /// Build the modal for this request.
    pub fn view(&self) -> ListSelectionView {
        ListSelectionView::new(format!("Approve? {}", self.title), self.options.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn items() -> Vec<ListItem> {
        vec![
            ListItem {
                id: "allow".into(),
                name: "Allow once".into(),
                description: None,
                is_current: false,
            },
            ListItem {
                id: "deny".into(),
                name: "Deny".into(),
                description: None,
                is_current: false,
            },
        ]
    }

    #[test]
    fn navigation_is_bounded() {
        let mut v = ListSelectionView::new("t", items());
        assert_eq!(v.handle_key(key(KeyCode::Up)), ModalOutcome::Open);
        assert_eq!(v.selected_index(), 0);
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        assert_eq!(v.selected_index(), 1, "clamped at last item");
    }

    #[test]
    fn enter_emits_explicit_selection() {
        let mut v = ListSelectionView::new("t", items());
        v.handle_key(key(KeyCode::Down));
        assert_eq!(
            v.handle_key(key(KeyCode::Enter)),
            ModalOutcome::Selected("deny".into())
        );
    }

    #[test]
    fn esc_cancels() {
        let mut v = ListSelectionView::new("t", items());
        assert_eq!(v.handle_key(key(KeyCode::Esc)), ModalOutcome::Cancelled);
    }

    #[test]
    fn empty_list_enter_cancels() {
        let mut v = ListSelectionView::new("t", vec![]);
        assert_eq!(v.handle_key(key(KeyCode::Enter)), ModalOutcome::Cancelled);
    }

    #[test]
    fn current_item_preselected() {
        let mut its = items();
        its[1].is_current = true;
        let v = ListSelectionView::new("t", its);
        assert_eq!(v.selected_index(), 1);
    }
}
