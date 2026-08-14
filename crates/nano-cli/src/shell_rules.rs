//! P4 §2.6: the wiring between nano-core's execrules engine and the two
//! approval gates (the ACP interactive gate in `acp_mode`, the exec
//! non-interactive gate in `exec_mode`).
//!
//! The engine (`nano_core::execrules`) owns parsing, evaluation, and the
//! fail-closed file mechanics; this module owns the host-side seams:
//! grammar selection matched to the executor's shell (design D2 — the SAME
//! `ShellKind::platform_default()` call site the executor uses), the
//! session-start load (§11: config, re-read per session, never journaled
//! as state), the bounded typed-denial message (§8), the disclosed-scope
//! card labels (§2.6), and the amendment flow — the ONLY writer of
//! `rules.toml` — in §11's pinned order: file append+rename (engine),
//! then the `Op::ShellRuleAmended` audit op, then the shared-cell swap.

use std::path::Path;
use std::sync::{Arc, RwLock};

use nano_core::execrules::{
    self, ApprovalRuleChoice, Evaluation, PatternToken, RuleDecision, RuleSet, ShellGrammar,
};
use nano_model::types::ToolCall;
use nano_session::NanoErrorKind;
use nano_session::op::{self, Op, OpEnvelope};
use sha2::Digest;

/// The session's shared rules cell (§2.6: the C2 shared-cell precedent —
/// `ArcSwap` is not in the tree and is not added for one cell). Amended in
/// place under the engine's sidecar lock; read per approval check.
pub type SharedRules = Arc<RwLock<RuleSet>>;

/// The rule grammar matched to the executor's shell. PowerShell is an
/// unwired executor shell (the `{command}`-only schema pin, wiring.rs), so
/// it maps to `None` — rules are never consulted for it and amendments
/// never mint (the §2.4 Prompt floor: no Allow is possible).
pub fn platform_grammar() -> Option<ShellGrammar> {
    match nano_tools::shell::ShellKind::platform_default() {
        #[cfg(windows)]
        nano_tools::shell::ShellKind::Cmd => Some(ShellGrammar::CmdExe),
        // PowerShell is an unwired executor shell (the `{command}`-only
        // schema pin, wiring.rs): rules are never consulted for it and
        // amendments never mint (the §2.4 Prompt floor — no Allow possible).
        #[cfg(windows)]
        nano_tools::shell::ShellKind::PowerShell => None,
        #[cfg(unix)]
        nano_tools::shell::ShellKind::Sh => Some(ShellGrammar::PosixSh),
    }
}

/// Session-start rules load (§2.5/§11), fail-closed: ANY load failure
/// (strict parse, §2.1 validation, ownership/ACL audit, symlink,
/// containment) runs the session with ZERO user rules and returns the loud
/// typed warning (`RuleFileInvalid` presentation + the bounded engine
/// reason); a missing file is the empty set, not an error.
pub fn load_session_rules(nano_home: &Path) -> (RuleSet, Option<String>) {
    match execrules::load_configured_rules(nano_home) {
        Ok(rules) => (rules, None),
        Err(err) => (
            RuleSet::default(),
            Some(format!(
                "{} ({err})",
                nano_session::error_codes::error_presentation(NanoErrorKind::RuleFileInvalid)
            )),
        ),
    }
}

/// Evaluate a gate call against a ruleset. `None` = not a rule-consultable
/// call (non-`shell` tool, missing/non-string `command` argument, unwired
/// shell grammar) — the caller's existing behavior stands unchanged.
pub fn evaluate_set(rules: &RuleSet, call: &ToolCall) -> Option<Evaluation> {
    if call.name != "shell" {
        return None;
    }
    let command = call.arguments.get("command")?.as_str()?;
    let grammar = platform_grammar()?;
    Some(rules.evaluate_with_matches(grammar, command))
}

/// The shared-cell counterpart of [`evaluate_set`] (one lock read per
/// approval check, poison-tolerant like every other session cell).
pub fn evaluate(rules: &RwLock<RuleSet>, call: &ToolCall) -> Option<Evaluation> {
    let set = rules.read().unwrap_or_else(|p| p.into_inner());
    evaluate_set(&set, call)
}

/// The bounded typed denial (§8: "Denied by shell rule #N (`<matched
/// prefix>`).") — rule index + matched prefix only, never the full command
/// beyond what the user already saw in the prompt.
pub fn denial_message(evaluation: &Evaluation) -> String {
    const MAX_MESSAGE_CHARS: usize = 200;
    let message = match evaluation
        .matched
        .iter()
        .find(|matched| matched.decision == RuleDecision::Deny)
    {
        Some(matched) => format!(
            "Denied by shell rule #{} (`{}`).",
            matched.rule_index,
            matched.matched_prefix.join(" ")
        ),
        // A nested-layer deny can carry no segment-level match; the denial
        // stays typed and bounded regardless.
        None => "Denied by shell rule.".to_string(),
    };
    let mut boundary = message.len().min(MAX_MESSAGE_CHARS);
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message[..boundary].to_string()
}

/// The card-option → amendment-choice mapping (§2.6: the arm distinguishes
/// `allow_always_exact` from `allow_always_prefix` from `allow_once`).
pub fn choice_for_option(option_id: &str) -> Option<ApprovalRuleChoice> {
    match option_id {
        nano_protocol::acp::ALLOW_ALWAYS_EXACT_ID => Some(ApprovalRuleChoice::AllowAlwaysExact),
        nano_protocol::acp::ALLOW_ALWAYS_PREFIX_ID => Some(ApprovalRuleChoice::AllowAlwaysPrefix),
        _ => None,
    }
}

/// Mint the two card labels carrying the disclosed future-match scope in
/// words (§2.6). `(None, None)` when the command is Complex/unamendable —
/// the always-options are then ABSENT from the card (a Complex command can
/// never be silently persisted).
pub fn card_scopes(command: &str, evaluation: &Evaluation) -> (Option<String>, Option<String>) {
    if !evaluation.amendable {
        return (None, None);
    }
    let Some(grammar) = platform_grammar() else {
        return (None, None);
    };
    let prompt_match = evaluation
        .matched
        .iter()
        .find(|matched| matched.decision == RuleDecision::Prompt);
    let mint = |choice| {
        execrules::mint_approval_rule(command, grammar, choice, prompt_match, Some(added_at_now()))
            .ok()
            .flatten()
            .map(|minted| bound_label(&minted.scope_text))
    };
    (
        mint(ApprovalRuleChoice::AllowAlwaysExact),
        mint(ApprovalRuleChoice::AllowAlwaysPrefix),
    )
}

/// Card labels stay one-line and bounded; the minted RULE is the
/// authority, the label is display-only.
fn bound_label(scope_text: &str) -> String {
    const MAX_LABEL_CHARS: usize = 160;
    if scope_text.chars().count() <= MAX_LABEL_CHARS {
        return scope_text.to_string();
    }
    let truncated: String = scope_text.chars().take(MAX_LABEL_CHARS).collect();
    format!("{truncated}…")
}

fn added_at_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    nano_agent::cron::rfc3339_minute(secs)
}

fn single_tokens(rule: &execrules::PrefixRule) -> Vec<String> {
    rule.pattern
        .iter()
        .filter_map(|token| match token {
            PatternToken::Single(value) => Some(value.clone()),
            // Minted rules are Single-only (§2.6: Alts are never
            // synthesized); an Alts token here means a non-card caller —
            // refused by the journal-payload bound check below.
            PatternToken::Alts(_) => None,
        })
        .collect()
}

/// The §2.6 amendment flow — the ONLY writer of `rules.toml`, invoked only
/// from the approval card's `allow_always_*` selection. The amended rule is
/// minted from the parsed tokens of the exact approved command (never free
/// text), appended under the engine's cross-process sidecar lock
/// (reload→validate→append→fsync→rename→reload-verify), THEN audited to the
/// journal, THEN swapped into the running session's cell (§11 ordering: a
/// failure after the file write leaves a rule without its audit op — the
/// SAFE direction — and an unswapped cell).
///
/// Errors are bounded strings for the host's loud warning; the one-shot
/// approval the user already granted stands, but NOTHING is persisted
/// unless the whole sequence completes.
pub fn amend(
    nano_home: &Path,
    rules: &SharedRules,
    coordinator: &nano_session::JournalCoordinator,
    session_id: &str,
    command: &str,
    choice: ApprovalRuleChoice,
) -> Result<(), String> {
    let grammar = platform_grammar()
        .ok_or_else(|| "shell rules are unavailable for this shell".to_string())?;
    // Re-evaluate against the LIVE cell: the mint reflects the ruleset the
    // session actually runs, even if a concurrent amendment landed while
    // the card was open.
    let evaluation = {
        let set = rules.read().unwrap_or_else(|p| p.into_inner());
        set.evaluate_with_matches(grammar, command)
    };
    let prompt_match = evaluation
        .matched
        .iter()
        .find(|matched| matched.decision == RuleDecision::Prompt);
    let minted =
        execrules::mint_approval_rule(command, grammar, choice, prompt_match, Some(added_at_now()))
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "nothing to amend".to_string())?;
    // Journal-payload bounds BEFORE any write (§2.6: the op carries ≤ 8
    // tokens × 128 chars; an over-bounds amendment is refused whole, never
    // half-written or half-journaled).
    for rule in &minted.rules {
        let tokens = single_tokens(rule);
        if tokens.len() != rule.pattern.len()
            || tokens.len() > op::MAX_RULE_AMEND_TOKENS
            || tokens
                .iter()
                .any(|token| token.chars().count() > op::MAX_RULE_AMEND_TOKEN_CHARS)
        {
            return Err(
                "command exceeds the journaled rule size bound; approve once instead".to_string(),
            );
        }
    }
    // 1. File append under the sidecar lock (reload→validate→append→
    //    fsync→rename→reload-verify, all inside the engine).
    let new_set = execrules::append_configured_amendment(nano_home, &minted)
        .map_err(|err| err.to_string())?;
    // 2. The audit op(s) — one per minted rule (a compound mints one exact
    //    rule per segment), each carrying the post-append file digest.
    let path = execrules::configured_rules_path(nano_home).map_err(|err| err.to_string())?;
    let bytes = std::fs::read(&path).map_err(|err| format!("rules digest read failed: {err}"))?;
    let digest: String = sha2::Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for (segment, rule) in minted.rules.iter().enumerate() {
        let amendment_id = format!("{session_id}-rule-{nanos}-{segment}");
        let prefix = single_tokens(rule);
        op::validate_shell_rule_amended(&amendment_id, &prefix, &digest)
            .map_err(|reason| format!("amendment audit payload invalid: {reason}"))?;
        let envelope = OpEnvelope::new(
            amendment_id.clone(),
            "now",
            Op::ShellRuleAmended {
                amendment_id,
                prefix,
                exact: rule.exact,
                rule_digest: digest.clone(),
            },
        );
        coordinator
            .append(&envelope)
            .map_err(|err| format!("rule saved but the audit op failed to journal: {err}"))?;
    }
    // 3. The cell swap LAST — the running session honors the new ruleset
    //    only after the file and the journal both carry it.
    *rules.write().unwrap_or_else(|p| p.into_inner()) = new_set;
    Ok(())
}
