//! `wayland-nano rules` (P4 §9): print the parsed shell-rule table and the
//! file it came from — codex's `execpolicy check` debugging surface,
//! minimal form. READ-ONLY: it never creates, repairs, or writes the file
//! (the amendment path is the only writer, §2.5), and the fail-closed load
//! gates apply in full — an invalid or insecurely configured file is
//! reported with the `RuleFileInvalid` presentation, exit 1.

use std::io::Write;
use std::path::Path;

use nano_core::execrules::{self, PatternToken, RuleDecision};

pub fn run(nano_home: &Path, out: &mut dyn Write) -> i32 {
    let path = match execrules::configured_rules_path(nano_home) {
        Ok(path) => path,
        Err(err) => return invalid(&err),
    };
    let _ = writeln!(out, "rules file: {}", path.display());
    match execrules::load_configured_rules(nano_home) {
        Ok(rules) => {
            if rules.rules().is_empty() {
                let _ = writeln!(out, "(no rules)");
            }
            for (index, rule) in rules.rules().iter().enumerate() {
                let _ = writeln!(
                    out,
                    "#{index}\t{}\t{}\t{}\t{}",
                    decision_id(rule.decision),
                    if rule.exact { "exact" } else { "prefix" },
                    pattern_words(&rule.pattern),
                    rule.source.map_or("operator", |_| "approval_card"),
                );
            }
            0
        }
        Err(err) => invalid(&err),
    }
}

fn invalid(err: &execrules::RuleStoreError) -> i32 {
    eprintln!(
        "wayland-nano: {} ({err})",
        nano_session::error_codes::error_presentation(nano_session::NanoErrorKind::RuleFileInvalid)
    );
    1
}

fn decision_id(decision: RuleDecision) -> &'static str {
    match decision {
        RuleDecision::Allow => "allow",
        RuleDecision::Prompt => "prompt",
        RuleDecision::Deny => "deny",
    }
}

fn pattern_words(pattern: &[PatternToken]) -> String {
    pattern
        .iter()
        .map(|token| match token {
            PatternToken::Single(value) => value.clone(),
            PatternToken::Alts(alts) => format!("({})", alts.join("|")),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
