use crate::CuaOp;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuaPolicyOutcome {
    Allow,
    Reject { reason: String },
    Prompt { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuaPolicy {
    #[serde(default)]
    pub forbidden_apps: Vec<String>,
    #[serde(default = "built_in_forbidden_combos")]
    pub forbidden_key_combos: Vec<String>,
    #[serde(default = "yes")]
    pub first_time_per_app_approval: bool,
    #[serde(skip)]
    seen_apps: Arc<Mutex<HashSet<String>>>,
}
fn yes() -> bool {
    true
}
fn built_in_forbidden_combos() -> Vec<String> {
    vec![
        "ctrl+alt+del".into(),
        "cmd+q+system".into(),
        "super+l".into(),
    ]
}

impl Default for CuaPolicy {
    fn default() -> Self {
        Self {
            forbidden_apps: Vec::new(),
            forbidden_key_combos: built_in_forbidden_combos(),
            first_time_per_app_approval: true,
            seen_apps: Arc::default(),
        }
    }
}
impl CuaPolicy {
    pub fn permissive() -> Self {
        Self {
            first_time_per_app_approval: false,
            ..Self::default()
        }
    }
    pub fn mark_app_seen(&self, app_id: &str) {
        if !app_id.is_empty() {
            self.seen_apps.lock().insert(app_id.to_ascii_lowercase());
        }
    }
    pub fn check_op(&self, op: &CuaOp, app_id: &str) -> CuaPolicyOutcome {
        if self
            .forbidden_apps
            .iter()
            .any(|app| app.eq_ignore_ascii_case(app_id))
        {
            return CuaPolicyOutcome::Reject {
                reason: "frontmost app is forbidden".into(),
            };
        }
        let payload = match op {
            CuaOp::Key { keys, .. } => Some(keys.as_str()),
            CuaOp::Type { text } => {
                if text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    return CuaPolicyOutcome::Reject {
                        reason: "typed text contains a control character".into(),
                    };
                }
                Some(text.as_str())
            }
            _ => None,
        };
        if let Some(payload) = payload {
            let normalized = normalize_combo(payload);
            if self
                .forbidden_key_combos
                .iter()
                .any(|blocked| matches_combo(blocked, &normalized))
            {
                return CuaPolicyOutcome::Reject {
                    reason: "forbidden key combination".into(),
                };
            }
        }
        let has_app_rule = self.first_time_per_app_approval || !self.forbidden_apps.is_empty();
        if app_id.is_empty() && has_app_rule {
            return CuaPolicyOutcome::Prompt {
                reason: "frontmost app is unresolved".into(),
            };
        }
        if self.first_time_per_app_approval
            && !self.seen_apps.lock().contains(&app_id.to_ascii_lowercase())
        {
            return CuaPolicyOutcome::Prompt {
                reason: format!("first contact with app {app_id}"),
            };
        }
        CuaPolicyOutcome::Allow
    }
}

fn normalize_combo(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '⌘' => out.push_str("+cmd+"),
            '⌥' => out.push_str("+alt+"),
            '⇧' => out.push_str("+shift+"),
            '⌃' | '^' => out.push_str("+ctrl+"),
            ' ' | '\t' | '-' | '_' => out.push('+'),
            c => out.extend(c.to_lowercase()),
        }
    }
    let out = out
        .replace("command", "cmd")
        .replace("option", "alt")
        .replace("windows", "win");
    out.split('+')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("+")
}
fn matches_combo(blocked: &str, normalized: &str) -> bool {
    let blocked = normalize_combo(blocked);
    normalized == blocked
        || normalized.split_whitespace().any(|part| {
            part.trim_matches(|c: char| c.is_ascii_punctuation() && c != '+') == blocked
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyMods;
    #[test]
    fn forbidden_app_and_unknown_fail_closed() {
        let p = CuaPolicy {
            forbidden_apps: vec!["vault.exe".into()],
            ..Default::default()
        };
        assert!(matches!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, "VAULT.EXE"),
            CuaPolicyOutcome::Reject { .. }
        ));
        assert!(matches!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, ""),
            CuaPolicyOutcome::Prompt { .. }
        ));
    }
    #[test]
    fn combo_glyphs_and_control_chars_reject() {
        let p = CuaPolicy::default();
        assert!(matches!(
            p.check_op(
                &CuaOp::Key {
                    keys: "⌘Q+system".into(),
                    mods: KeyMods::default()
                },
                "app"
            ),
            CuaPolicyOutcome::Reject { .. }
        ));
        assert!(matches!(
            p.check_op(
                &CuaOp::Type {
                    text: "x\u{1b}".into()
                },
                "app"
            ),
            CuaPolicyOutcome::Reject { .. }
        ));
    }
    #[test]
    fn seen_set_is_memory_only() {
        let p = CuaPolicy::default();
        assert!(matches!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, "app"),
            CuaPolicyOutcome::Prompt { .. }
        ));
        p.mark_app_seen("app");
        assert_eq!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, "app"),
            CuaPolicyOutcome::Allow
        );
    }
}
