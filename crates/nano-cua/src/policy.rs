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
    if blocked.is_empty() || normalized == blocked {
        return !blocked.is_empty();
    }
    // Substring match on token boundaries so a combo smuggled inside a
    // longer `type` payload ("press ⌘Q to quit") still rejects, without
    // false-positiving on `acmd+q`.
    let boundaries = ['+', ' ', '\t', '\n'];
    let Some(idx) = normalized.find(&blocked) else {
        return false;
    };
    let before_ok = idx == 0
        || normalized[..idx]
            .chars()
            .last()
            .is_some_and(|c| boundaries.contains(&c));
    let end = idx + blocked.len();
    let after_ok = end == normalized.len()
        || normalized[end..]
            .chars()
            .next()
            .is_some_and(|c| boundaries.contains(&c));
    before_ok && after_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KeyMods;

    fn click() -> CuaOp {
        CuaOp::LeftClick {
            x: 10,
            y: 20,
            button: crate::MouseButton::Left,
            mods: KeyMods::default(),
        }
    }
    fn key(keys: &str) -> CuaOp {
        CuaOp::Key {
            keys: keys.into(),
            mods: KeyMods::default(),
        }
    }
    fn ty(text: &str) -> CuaOp {
        CuaOp::Type { text: text.into() }
    }

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
            p.check_op(&key("⌘Q+system"), "app"),
            CuaPolicyOutcome::Reject { .. }
        ));
        assert!(matches!(
            p.check_op(&ty("x\u{1b}"), "app"),
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
        // Until marked, repeated checks keep prompting — the approval
        // response, not the prompt itself, flips the app to seen.
        assert!(matches!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, "app"),
            CuaPolicyOutcome::Prompt { .. }
        ));
        p.mark_app_seen("app");
        assert_eq!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, "app"),
            CuaPolicyOutcome::Allow
        );
        // A fresh policy (a new session) shares nothing — the seen-set
        // is session-scoped and dies with the session.
        let next_session = CuaPolicy::default();
        assert!(matches!(
            next_session.check_op(&CuaOp::Wait { duration_ms: 1 }, "app"),
            CuaPolicyOutcome::Prompt { .. }
        ));
        // Marking is case-insensitive and ignores empty ids.
        p.mark_app_seen("Other.EXE");
        assert_eq!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, "other.exe"),
            CuaPolicyOutcome::Allow
        );
        p.mark_app_seen("");
        assert!(matches!(
            p.check_op(&CuaOp::Wait { duration_ms: 1 }, ""),
            CuaPolicyOutcome::Prompt { .. }
        ));
    }

    // ── Ported donor battery (wcore-cua policy.rs + tests/policy_*),
    // adapted: Suspend → Prompt, no require_approval_for_app, no disk store.

    #[test]
    fn permissive_baseline_allows_known_app() {
        let p = CuaPolicy::permissive();
        assert_eq!(p.check_op(&click(), "AnyApp"), CuaPolicyOutcome::Allow);
    }

    #[test]
    fn forbidden_app_blocks_every_op_kind() {
        let p = CuaPolicy {
            forbidden_apps: vec!["1Password".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p.check_op(&ty("secret"), "1password"),
            CuaPolicyOutcome::Reject { .. }
        ));
        assert!(matches!(
            p.check_op(&click(), "1Password"),
            CuaPolicyOutcome::Reject { .. }
        ));
    }

    #[test]
    fn forbidden_key_combo_rejected_outright() {
        let p = CuaPolicy {
            forbidden_key_combos: vec!["cmd+q+system".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p.check_op(&key("cmd+q+system"), "Finder"),
            CuaPolicyOutcome::Reject { .. }
        ));
    }

    #[test]
    fn empty_app_id_still_checks_combos_before_failing_closed() {
        let p = CuaPolicy {
            forbidden_key_combos: vec!["ctrl+alt+del".into()],
            first_time_per_app_approval: true,
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p.check_op(&key("ctrl+alt+del"), ""),
            CuaPolicyOutcome::Reject { .. }
        ));
        assert!(matches!(
            p.check_op(&click(), ""),
            CuaPolicyOutcome::Prompt { .. }
        ));
    }

    #[test]
    fn empty_app_id_with_forbidden_apps_fails_closed_to_prompt() {
        let p = CuaPolicy {
            forbidden_apps: vec!["1Password".into()],
            first_time_per_app_approval: false,
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p.check_op(&click(), ""),
            CuaPolicyOutcome::Prompt { .. }
        ));
    }

    #[test]
    fn empty_app_id_with_no_app_rules_still_allows() {
        let mut p = CuaPolicy::permissive();
        p.forbidden_key_combos = Vec::new();
        assert_eq!(p.check_op(&click(), ""), CuaPolicyOutcome::Allow);
    }

    #[test]
    fn normalize_combo_handles_unicode_glyphs() {
        assert_eq!(normalize_combo("⌘Q"), "cmd+q");
        assert_eq!(normalize_combo("Cmd+Q"), "cmd+q");
        assert_eq!(normalize_combo("command-Q"), "cmd+q");
        assert_eq!(normalize_combo("^Q"), "ctrl+q");
        assert_eq!(normalize_combo("Q⌘"), "q+cmd");
    }

    // Type-payload smuggling (donor tests/op_type_keycombo_test.rs).

    #[test]
    fn type_payload_control_chars_reject() {
        let p = CuaPolicy::permissive();
        for payload in ["hello\0world", "\x1b[2Jhello", "\x07alert"] {
            assert!(
                matches!(
                    p.check_op(&ty(payload), "Terminal"),
                    CuaPolicyOutcome::Reject { .. }
                ),
                "{payload:?} must reject"
            );
        }
    }

    #[test]
    fn type_payload_smuggled_combos_reject() {
        let p = CuaPolicy {
            forbidden_key_combos: vec!["cmd+q".into()],
            ..CuaPolicy::permissive()
        };
        for payload in ["press ⌘Q to quit", "hit Cmd+Q now", "then command-q"] {
            assert!(
                matches!(
                    p.check_op(&ty(payload), "TextEdit"),
                    CuaPolicyOutcome::Reject { .. }
                ),
                "{payload:?} must reject"
            );
        }
        let p2 = CuaPolicy {
            forbidden_key_combos: vec!["ctrl+q".into()],
            ..CuaPolicy::permissive()
        };
        assert!(matches!(
            p2.check_op(&ty("press ^Q"), "TextEdit"),
            CuaPolicyOutcome::Reject { .. }
        ));
        // Token-boundary discipline: a longer token containing the combo
        // spelling without a boundary is not a combo.
        let p3 = CuaPolicy {
            forbidden_key_combos: vec!["cmd+q".into()],
            ..CuaPolicy::permissive()
        };
        assert_eq!(
            p3.check_op(&ty("acmd+q"), "TextEdit"),
            CuaPolicyOutcome::Allow
        );
    }

    #[test]
    fn type_payload_plain_and_unicode_text_allows() {
        let p = CuaPolicy::permissive();
        assert_eq!(
            p.check_op(&ty("hello world\nsecond line\ttabbed"), "TextEdit"),
            CuaPolicyOutcome::Allow
        );
        assert_eq!(
            p.check_op(&ty("こんにちは 你好 emoji 🚀"), "TextEdit"),
            CuaPolicyOutcome::Allow
        );
    }

    // Default/serde parity (donor tests/policy_default_test.rs).

    #[test]
    fn default_matches_serde_empty_roundtrip() {
        let default = CuaPolicy::default();
        let parsed: CuaPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(default.forbidden_apps, parsed.forbidden_apps);
        assert_eq!(default.forbidden_key_combos, parsed.forbidden_key_combos);
        assert_eq!(
            default.first_time_per_app_approval, parsed.first_time_per_app_approval,
            "first-contact gate must agree between Default and serde empty struct"
        );
        assert!(default.first_time_per_app_approval);
    }

    #[test]
    fn explicit_first_time_flag_survives_serde() {
        let p: CuaPolicy =
            serde_json::from_str(r#"{"first_time_per_app_approval": false}"#).unwrap();
        assert!(!p.first_time_per_app_approval);
        let p: CuaPolicy =
            serde_json::from_str(r#"{"first_time_per_app_approval": true}"#).unwrap();
        assert!(p.first_time_per_app_approval);
        // The serde roundtrip never carries the session-scoped seen-set.
        let mut seen = CuaPolicy::default();
        seen.mark_app_seen("app");
        seen = serde_json::from_str(&serde_json::to_string(&seen).unwrap()).unwrap();
        assert!(matches!(
            seen.check_op(&CuaOp::Wait { duration_ms: 1 }, "app"),
            CuaPolicyOutcome::Prompt { .. }
        ));
    }
}
