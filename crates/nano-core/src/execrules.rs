//! Fail-closed persistent shell-command rules.
//!
//! `rules.toml` is operator-writable security configuration and is exactly as
//! trustworthy as `nano_home`. Nano detects and refuses insecure paths,
//! permissions, malformed schemas, and invalid rules; a valid rule written by
//! the operator-equivalent principal is intentionally inside the trust model.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAX_COMMAND_BYTES: usize = 4 * 1024;
const MAX_TOKENS: usize = 64;
const MAX_SEGMENTS: usize = 32;
const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 4;
const MAX_REASON_BYTES: usize = 160;

pub const NANO_RULES_FILE_ENV: &str = "NANO_RULES_FILE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Prompt,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PatternToken {
    Single(String),
    Alts(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixRule {
    pub pattern: Vec<PatternToken>,
    #[serde(default)]
    pub exact: bool,
    pub decision: RuleDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub justification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RuleSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    ApprovalCard,
}

impl PrefixRule {
    pub fn validate(&self) -> Result<(), RuleValidationError> {
        let Some(PatternToken::Single(program)) = self.pattern.first() else {
            return Err(RuleValidationError::LiteralProgramRequired);
        };
        if program.is_empty() {
            return Err(RuleValidationError::LiteralProgramRequired);
        }
        for token in &self.pattern {
            match token {
                PatternToken::Single(value) if value.is_empty() => {
                    return Err(RuleValidationError::EmptyToken);
                }
                PatternToken::Alts(values)
                    if values.is_empty() || values.iter().any(String::is_empty) =>
                {
                    return Err(RuleValidationError::EmptyAlternatives);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn matches(&self, argv: &[String]) -> bool {
        if self.pattern.len() > argv.len() || (self.exact && self.pattern.len() != argv.len()) {
            return false;
        }
        self.pattern
            .iter()
            .zip(argv)
            .all(|(pattern, actual)| match pattern {
                PatternToken::Single(value) => value == actual,
                PatternToken::Alts(values) => values.iter().any(|value| value == actual),
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleValidationError {
    LiteralProgramRequired,
    EmptyToken,
    EmptyAlternatives,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGrammar {
    PosixSh,
    CmdExe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComplexReason {
    CommandTooLong,
    TooManyTokens,
    TooManySegments,
    TooManyNodes,
    RecursionTooDeep,
    EmptySegment,
    UnsupportedSyntax,
    CmdAssignment,
    CmdControlKeyword,
    NestedOpaque,
    NestedBoundaryUncertain,
    NestedGrammarUncertain,
    NestedParseDisagreement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenizeOutcome {
    Clean(Vec<Vec<String>>),
    Complex {
        segments: Vec<Vec<String>>,
        reason: ComplexReason,
    },
}

#[derive(Debug, Clone)]
struct ParsedToken {
    value: String,
    span: std::ops::Range<usize>,
}

#[derive(Debug, Clone)]
struct ParsedCommand {
    segments: Vec<Vec<ParsedToken>>,
    reason: Option<ComplexReason>,
}

pub fn tokenize(command: &str, shell: ShellGrammar) -> TokenizeOutcome {
    let parsed = parse(command, shell);
    let segments = parsed
        .segments
        .into_iter()
        .map(|segment| segment.into_iter().map(|token| token.value).collect())
        .collect();
    match parsed.reason {
        Some(reason) => TokenizeOutcome::Complex { segments, reason },
        None => TokenizeOutcome::Clean(segments),
    }
}

fn parse(command: &str, shell: ShellGrammar) -> ParsedCommand {
    if command.len() > MAX_COMMAND_BYTES {
        return ParsedCommand {
            segments: Vec::new(),
            reason: Some(ComplexReason::CommandTooLong),
        };
    }
    let mut segments = vec![Vec::new()];
    let mut reason = command
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n'))
        .then_some(ComplexReason::UnsupportedSyntax);
    let bytes = command.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if let Some(width) = separator_width(&command[index..], shell) {
            if segments.last().is_none_or(Vec::is_empty) {
                reason.get_or_insert(ComplexReason::EmptySegment);
            } else if segments.len() >= MAX_SEGMENTS {
                reason.get_or_insert(ComplexReason::TooManySegments);
                break;
            } else {
                segments.push(Vec::new());
            }
            index += width;
            continue;
        }
        let start = index;
        if shell == ShellGrammar::PosixSh && bytes[index] == b'\'' {
            index += 1;
            let content_start = index;
            while index < bytes.len() && bytes[index] != b'\'' {
                index += 1;
            }
            if index == bytes.len() {
                reason.get_or_insert(ComplexReason::UnsupportedSyntax);
                break;
            }
            let content_end = index;
            let value = &command[content_start..index];
            index += 1;
            if value.is_empty() || !value.bytes().all(is_posix_quoted_literal) {
                reason.get_or_insert(ComplexReason::UnsupportedSyntax);
            }
            if index < bytes.len()
                && !bytes[index].is_ascii_whitespace()
                && separator_width(&command[index..], shell).is_none()
            {
                reason.get_or_insert(ComplexReason::UnsupportedSyntax);
                consume_unsafe_token(bytes, &mut index);
            }
            segments
                .last_mut()
                .expect("initial segment")
                .push(ParsedToken {
                    value: value.to_owned(),
                    span: content_start..content_end,
                });
        } else {
            while index < bytes.len()
                && !bytes[index].is_ascii_whitespace()
                && separator_width(&command[index..], shell).is_none()
            {
                index += 1;
            }
            let value = &command[start..index];
            let safe = value.bytes().all(|byte| literal_allowed(byte, shell));
            if !safe {
                reason.get_or_insert(ComplexReason::UnsupportedSyntax);
            }
            let lower = value.to_ascii_lowercase();
            if shell == ShellGrammar::CmdExe {
                if segments.last().is_some_and(Vec::is_empty) && value.contains('=') {
                    reason.get_or_insert(ComplexReason::CmdAssignment);
                }
                if matches!(lower.as_str(), "for" | "if") {
                    reason.get_or_insert(ComplexReason::CmdControlKeyword);
                }
            }
            segments
                .last_mut()
                .expect("initial segment")
                .push(ParsedToken {
                    value: value.to_owned(),
                    span: start..index,
                });
        }
        let token_count: usize = segments.iter().map(Vec::len).sum();
        if token_count > MAX_TOKENS {
            reason.get_or_insert(ComplexReason::TooManyTokens);
            break;
        }
        if token_count + segments.len() > MAX_NODES {
            reason.get_or_insert(ComplexReason::TooManyNodes);
            break;
        }
    }
    if segments.last().is_none_or(Vec::is_empty) {
        reason.get_or_insert(ComplexReason::EmptySegment);
        segments.retain(|segment| !segment.is_empty());
    }
    ParsedCommand { segments, reason }
}

fn consume_unsafe_token(bytes: &[u8], index: &mut usize) {
    while *index < bytes.len() && !bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
}

fn separator_width(input: &str, shell: ShellGrammar) -> Option<usize> {
    if input.starts_with("&&") || input.starts_with("||") {
        Some(2)
    } else if input.starts_with('|')
        || (shell == ShellGrammar::PosixSh && input.starts_with(';'))
        || (shell == ShellGrammar::CmdExe && input.starts_with('&'))
    {
        Some(1)
    } else {
        None
    }
}

fn literal_allowed(byte: u8, shell: ShellGrammar) -> bool {
    byte.is_ascii_alphanumeric()
        || b"._/=+@-".contains(&byte)
        || (shell == ShellGrammar::PosixSh && b",:".contains(&byte))
        || (shell == ShellGrammar::CmdExe && b"\\:".contains(&byte))
}

fn is_posix_quoted_literal(byte: u8) -> bool {
    byte >= 0x20 && byte != b'\'' && byte != b'\n' && byte != b'\r'
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    pub rule_index: usize,
    pub matched_prefix: Vec<String>,
    pub decision: RuleDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub decision: Option<RuleDecision>,
    pub matched: Vec<RuleMatch>,
    pub amendable: bool,
    pub complex_reason: Option<ComplexReason>,
}

impl Evaluation {
    pub fn verdict(&self) -> RuleVerdict {
        match self.decision {
            Some(RuleDecision::Allow) => RuleVerdict::Allow,
            Some(RuleDecision::Deny) => RuleVerdict::Deny,
            Some(RuleDecision::Prompt) | None => RuleVerdict::Prompt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleVerdict {
    Allow,
    Prompt,
    Deny,
}

#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<PrefixRule>,
    by_program: HashMap<String, Vec<usize>>,
}

impl RuleSet {
    pub fn new(rules: Vec<PrefixRule>) -> Result<Self, RuleValidationError> {
        let mut by_program: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, rule) in rules.iter().enumerate() {
            rule.validate()?;
            let PatternToken::Single(program) = &rule.pattern[0] else {
                unreachable!("validated first token")
            };
            by_program.entry(program.clone()).or_default().push(index);
        }
        Ok(Self { rules, by_program })
    }

    pub fn rules(&self) -> &[PrefixRule] {
        &self.rules
    }

    pub fn evaluate(&self, shell_kind: ShellGrammar, command: &str) -> RuleVerdict {
        self.evaluate_with_matches(shell_kind, command).verdict()
    }

    pub fn evaluate_with_matches(&self, shell: ShellGrammar, command: &str) -> Evaluation {
        let mut budget = RecursionBudget::default();
        self.evaluate_layer(command, shell, 0, &mut budget)
    }

    fn evaluate_layer(
        &self,
        command: &str,
        shell: ShellGrammar,
        depth: usize,
        budget: &mut RecursionBudget,
    ) -> Evaluation {
        if depth > MAX_DEPTH {
            return prompt_complex(ComplexReason::RecursionTooDeep);
        }
        if budget.bytes.saturating_add(command.len()) > MAX_COMMAND_BYTES {
            return prompt_complex(ComplexReason::CommandTooLong);
        }
        budget.bytes += command.len();
        let parsed = parse(command, shell);
        let mut evaluation = Evaluation {
            decision: Some(RuleDecision::Allow),
            matched: Vec::new(),
            amendable: parsed.reason.is_none(),
            complex_reason: parsed.reason,
        };
        if parsed.segments.is_empty() {
            evaluation.decision = Some(RuleDecision::Prompt);
            return evaluation;
        }
        for segment in &parsed.segments {
            budget.tokens += segment.len();
            budget.nodes += segment.len() + 1;
            budget.segments += 1;
            if budget.tokens > MAX_TOKENS {
                merge_complex(&mut evaluation, ComplexReason::TooManyTokens);
                break;
            }
            if budget.nodes > MAX_NODES {
                merge_complex(&mut evaluation, ComplexReason::TooManyNodes);
                break;
            }
            if budget.segments > MAX_SEGMENTS {
                merge_complex(&mut evaluation, ComplexReason::TooManySegments);
                break;
            }
            let argv: Vec<String> = segment.iter().map(|token| token.value.clone()).collect();
            let (decision, mut matched) = self.evaluate_segment(&argv);
            merge_opinion(&mut evaluation.decision, decision);
            evaluation.matched.append(&mut matched);
            if let Some(nested) = nested_command(command, segment, shell) {
                match nested {
                    Nested::Static { raw, grammar } => {
                        let inner = self.evaluate_layer(raw, grammar, depth + 1, budget);
                        merge_evaluation(&mut evaluation, inner);
                    }
                    Nested::Opaque(reason) => merge_complex(&mut evaluation, reason),
                }
            }
        }
        if evaluation.complex_reason.is_some() && evaluation.decision != Some(RuleDecision::Deny) {
            evaluation.decision = Some(RuleDecision::Prompt);
        }
        evaluation
    }

    fn evaluate_segment(&self, argv: &[String]) -> (Option<RuleDecision>, Vec<RuleMatch>) {
        let Some(program) = argv.first() else {
            return (None, Vec::new());
        };
        let mut decision = None;
        let mut matches = Vec::new();
        for index in self.by_program.get(program).into_iter().flatten().copied() {
            let rule = &self.rules[index];
            if rule.matches(argv) {
                decision = Some(decision.map_or(rule.decision, |current: RuleDecision| {
                    current.max(rule.decision)
                }));
                matches.push(RuleMatch {
                    rule_index: index,
                    matched_prefix: argv[..rule.pattern.len()].to_vec(),
                    decision: rule.decision,
                });
            }
        }
        (decision, matches)
    }
}

#[derive(Default)]
struct RecursionBudget {
    bytes: usize,
    tokens: usize,
    segments: usize,
    nodes: usize,
}

enum Nested<'a> {
    Static { raw: &'a str, grammar: ShellGrammar },
    Opaque(ComplexReason),
}

fn nested_command<'a>(
    source: &'a str,
    segment: &[ParsedToken],
    outer: ShellGrammar,
) -> Option<Nested<'a>> {
    let program = segment.first()?.value.to_ascii_lowercase();
    let flag = segment.get(1).map(|token| token.value.to_ascii_lowercase());
    let is_pipe_stdin = segment.get(1).is_some_and(|token| token.value == "-");
    let basename = program.rsplit(['/', '\\']).next().unwrap_or(&program);
    let is_pathed = basename.len() != program.len();
    if matches!(basename, "pwsh" | "iex" | "invoke-expression")
        || (is_pathed
            && matches!(
                basename,
                "powershell" | "powershell.exe" | "cmd" | "cmd.exe" | "bash" | "sh"
            ))
    {
        return Some(Nested::Opaque(ComplexReason::NestedOpaque));
    }
    let grammar = match program.as_str() {
        "cmd" | "cmd.exe" if flag.as_deref() == Some("/c") => ShellGrammar::CmdExe,
        "bash" | "sh" if flag.as_deref() == Some("-c") => ShellGrammar::PosixSh,
        "powershell" | "powershell.exe" => {
            return Some(Nested::Opaque(
                if matches!(flag.as_deref(), Some("-enc" | "-encodedcommand")) {
                    ComplexReason::NestedOpaque
                } else {
                    ComplexReason::NestedGrammarUncertain
                },
            ));
        }
        "cmd" | "cmd.exe" | "bash" | "sh" if is_pipe_stdin => {
            return Some(Nested::Opaque(ComplexReason::NestedOpaque));
        }
        "cmd" | "cmd.exe" | "bash" | "sh" => {
            return Some(Nested::Opaque(ComplexReason::NestedOpaque));
        }
        _ => return None,
    };
    let Some(inner) = segment.get(2) else {
        return Some(Nested::Opaque(ComplexReason::NestedBoundaryUncertain));
    };
    if segment.len() != 3 {
        return Some(Nested::Opaque(ComplexReason::NestedBoundaryUncertain));
    }
    let raw = &source[inner.span.clone()];
    if outer != grammar && separator_width(raw, outer) != separator_width(raw, grammar) {
        return Some(Nested::Opaque(ComplexReason::NestedParseDisagreement));
    }
    Some(Nested::Static { raw, grammar })
}

fn merge_opinion(current: &mut Option<RuleDecision>, next: Option<RuleDecision>) {
    *current = match (*current, next) {
        (Some(RuleDecision::Deny), _) | (_, Some(RuleDecision::Deny)) => Some(RuleDecision::Deny),
        (Some(RuleDecision::Allow), Some(RuleDecision::Allow)) => Some(RuleDecision::Allow),
        _ => Some(RuleDecision::Prompt),
    };
}

fn merge_evaluation(current: &mut Evaluation, mut inner: Evaluation) {
    merge_opinion(&mut current.decision, inner.decision);
    current.matched.append(&mut inner.matched);
    current.amendable &= inner.amendable;
    current.complex_reason = current.complex_reason.or(inner.complex_reason);
}

fn merge_complex(evaluation: &mut Evaluation, reason: ComplexReason) {
    evaluation.amendable = false;
    evaluation.complex_reason.get_or_insert(reason);
    if evaluation.decision != Some(RuleDecision::Deny) {
        evaluation.decision = Some(RuleDecision::Prompt);
    }
}

fn prompt_complex(reason: ComplexReason) -> Evaluation {
    Evaluation {
        decision: Some(RuleDecision::Prompt),
        matched: Vec::new(),
        amendable: false,
        complex_reason: Some(reason),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmendmentKind {
    Exact,
    Prefix,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRuleChoice {
    AllowOnce,
    #[default]
    AllowAlwaysExact,
    AllowAlwaysPrefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedAmendment {
    pub rules: Vec<PrefixRule>,
    pub scope_text: String,
}

pub fn mint_approval_rule(
    command: &str,
    shell: ShellGrammar,
    choice: ApprovalRuleChoice,
    prompt_match: Option<&RuleMatch>,
    added_at: Option<String>,
) -> Result<Option<MintedAmendment>, RuleStoreError> {
    let kind = match choice {
        ApprovalRuleChoice::AllowOnce => return Ok(None),
        ApprovalRuleChoice::AllowAlwaysExact => AmendmentKind::Exact,
        ApprovalRuleChoice::AllowAlwaysPrefix => AmendmentKind::Prefix,
    };
    let added_at = added_at.ok_or_else(|| invalid("persistent approval requires added_at"))?;
    mint_amendment(command, shell, kind, prompt_match, added_at).map(Some)
}

pub fn mint_amendment(
    command: &str,
    shell: ShellGrammar,
    kind: AmendmentKind,
    prompt_match: Option<&RuleMatch>,
    added_at: String,
) -> Result<MintedAmendment, RuleStoreError> {
    if added_at.trim().is_empty() {
        return Err(invalid("approval-card amendment requires added_at"));
    }
    if !RuleSet::default()
        .evaluate_with_matches(shell, command)
        .amendable
    {
        return Err(RuleStoreError::CommandNotAmendable);
    }
    let TokenizeOutcome::Clean(segments) = tokenize(command, shell) else {
        return Err(RuleStoreError::CommandNotAmendable);
    };
    if segments.is_empty() {
        return Err(RuleStoreError::CommandNotAmendable);
    }
    let mut rules = Vec::with_capacity(segments.len());
    for segment in &segments {
        let tokens = match kind {
            AmendmentKind::Exact => segment.clone(),
            AmendmentKind::Prefix => prompt_match
                .filter(|matched| {
                    matched.decision == RuleDecision::Prompt
                        && matched.matched_prefix.first() == segment.first()
                })
                .map_or_else(
                    || vec![segment[0].clone()],
                    |matched| matched.matched_prefix.clone(),
                ),
        };
        rules.push(PrefixRule {
            pattern: tokens.iter().cloned().map(PatternToken::Single).collect(),
            exact: kind == AmendmentKind::Exact,
            decision: RuleDecision::Allow,
            justification: None,
            added_at: Some(added_at.clone()),
            source: Some(RuleSource::ApprovalCard),
        });
    }
    let scope_text = match (kind, rules.as_slice()) {
        (AmendmentKind::Exact, [_]) => {
            format!("only this exact argv: {}", shell_words(&segments[0]))
        }
        (AmendmentKind::Exact, _) => format!(
            "only these exact argv segments: {}",
            segments
                .iter()
                .map(|segment| shell_words(segment))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        (AmendmentKind::Prefix, _) => rules
            .iter()
            .map(prefix_scope_text)
            .collect::<Vec<_>>()
            .join("; "),
    };
    Ok(MintedAmendment { rules, scope_text })
}

fn shell_words(words: &[String]) -> String {
    words.join(" ")
}

fn pattern_words(pattern: &[PatternToken]) -> String {
    pattern
        .iter()
        .filter_map(|token| match token {
            PatternToken::Single(value) => Some(value.as_str()),
            PatternToken::Alts(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn prefix_scope_text(rule: &PrefixRule) -> String {
    if rule.pattern.len() == 1 {
        format!("any future `{}` command", pattern_words(&rule.pattern))
    } else {
        format!(
            "any future command whose argv starts with `{}`, including with extra flags",
            pattern_words(&rule.pattern)
        )
    }
}

#[derive(Debug)]
pub enum RuleStoreError {
    RuleFileInvalid { reason: String },
    LockBusy,
    Io(io::Error),
    CommandNotAmendable,
}

impl std::fmt::Display for RuleStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuleFileInvalid { reason } => write!(
                formatter,
                "shell rules file is invalid or insecurely configured; running with no saved rules: {reason}"
            ),
            Self::LockBusy => formatter.write_str("shell rules file is locked by another writer"),
            Self::Io(error) => write!(formatter, "shell rules I/O failed: {error}"),
            Self::CommandNotAmendable => formatter.write_str(
                "shell command uses complex or opaque syntax and cannot be saved as a rule",
            ),
        }
    }
}

impl std::error::Error for RuleStoreError {}

impl From<io::Error> for RuleStoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<PrefixRule>,
}

pub fn rules_path(
    nano_home: &Path,
    override_path: Option<&Path>,
) -> Result<PathBuf, RuleStoreError> {
    let canonical_home = dunce::canonicalize(nano_home).map_err(invalid_io)?;
    let requested = override_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| nano_home.join("rules.toml"));
    let parent = requested
        .parent()
        .ok_or_else(|| invalid("rules path has no parent"))?;
    let canonical_parent = dunce::canonicalize(parent).map_err(invalid_io)?;
    if !canonical_parent.starts_with(&canonical_home) {
        return Err(invalid("rules path escapes canonical nano_home"));
    }
    let file_name = requested
        .file_name()
        .ok_or_else(|| invalid("rules path has no file name"))?;
    Ok(canonical_parent.join(file_name))
}

/// Resolve the default rule file or the operator-provided `NANO_RULES_FILE`.
/// The override relocates the file only within canonical `nano_home`.
pub fn configured_rules_path(nano_home: &Path) -> Result<PathBuf, RuleStoreError> {
    let override_path = std::env::var_os(NANO_RULES_FILE_ENV).map(PathBuf::from);
    rules_path(nano_home, override_path.as_deref())
}

pub fn load_configured_rules(nano_home: &Path) -> Result<RuleSet, RuleStoreError> {
    let path = configured_rules_path(nano_home)?;
    if !path.exists() {
        return Ok(RuleSet::default());
    }
    load_rules_at(&path)
}

pub fn load_rules(
    nano_home: &Path,
    override_path: Option<&Path>,
) -> Result<RuleSet, RuleStoreError> {
    let path = rules_path(nano_home, override_path)?;
    if !path.exists() {
        return Ok(RuleSet::default());
    }
    load_rules_at(&path)
}

fn load_rules_at(path: &Path) -> Result<RuleSet, RuleStoreError> {
    let link_meta = fs::symlink_metadata(path).map_err(invalid_io)?;
    if link_meta.file_type().is_symlink() || !link_meta.file_type().is_file() {
        return Err(invalid("rules path is not a non-symlink regular file"));
    }
    verify_owner_only(path, &link_meta)?;
    let mut file = open_no_follow(path)?;
    let opened_meta = file.metadata().map_err(invalid_io)?;
    if !opened_meta.file_type().is_file() {
        return Err(invalid("opened rules object is not a regular file"));
    }
    let post_meta = fs::symlink_metadata(path).map_err(invalid_io)?;
    if post_meta.file_type().is_symlink() || !post_meta.file_type().is_file() {
        return Err(invalid(
            "rules path changed to a non-regular file during open",
        ));
    }
    verify_owner_only(path, &post_meta)?;
    let post_file = open_no_follow(path)?;
    if !same_open_file(&file, &post_file).map_err(invalid_io)? {
        return Err(invalid(
            "rules file changed between metadata check and open",
        ));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents).map_err(invalid_io)?;
    parse_rule_file(&contents)
}

fn parse_rule_file(contents: &str) -> Result<RuleSet, RuleStoreError> {
    let file: RuleFile = toml::from_str(contents)
        .map_err(|error| invalid(format!("strict TOML parse failed: {error}")))?;
    RuleSet::new(file.rule).map_err(|error| invalid(format!("rule validation failed: {error:?}")))
}

pub fn append_amendment(
    nano_home: &Path,
    override_path: Option<&Path>,
    amendment: &MintedAmendment,
) -> Result<RuleSet, RuleStoreError> {
    if amendment.rules.is_empty() {
        return Err(invalid("amendment must contain at least one rule"));
    }
    verify_platform_acl_available()?;
    let path = rules_path(nano_home, override_path)?;
    let lock_path = sidecar_lock_path(&path);
    let _lock = FileLock::try_acquire(&lock_path)?;
    let mut rules = if path.exists() {
        load_rules_at(&path)?.rules
    } else {
        Vec::new()
    };
    for rule in &amendment.rules {
        rule.validate()
            .map_err(|error| invalid(format!("amendment validation failed: {error:?}")))?;
        if rule.decision != RuleDecision::Allow
            || rule.source != Some(RuleSource::ApprovalCard)
            || rule
                .pattern
                .iter()
                .any(|token| !matches!(token, PatternToken::Single(_)))
        {
            return Err(invalid(
                "amendments must be allow-only, card-sourced, literal rules",
            ));
        }
    }
    rules.extend(amendment.rules.iter().cloned());
    let encoded = toml::to_string_pretty(&RuleFile { rule: rules })
        .map_err(|error| invalid(format!("rule serialization failed: {error}")))?;
    atomic_owner_only_write(&path, encoded.as_bytes())?;
    load_rules_at(&path)
}

pub fn append_configured_amendment(
    nano_home: &Path,
    amendment: &MintedAmendment,
) -> Result<RuleSet, RuleStoreError> {
    let override_path = std::env::var_os(NANO_RULES_FILE_ENV).map(PathBuf::from);
    append_amendment(nano_home, override_path.as_deref(), amendment)
}

fn sidecar_lock_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rules.toml");
    path.with_file_name(format!("{name}.lock"))
}

fn invalid(reason: impl Into<String>) -> RuleStoreError {
    let mut reason = reason.into();
    if reason.len() > MAX_REASON_BYTES {
        let mut boundary = MAX_REASON_BYTES;
        while !reason.is_char_boundary(boundary) {
            boundary -= 1;
        }
        reason.truncate(boundary);
    }
    RuleStoreError::RuleFileInvalid { reason }
}

fn invalid_io(error: io::Error) -> RuleStoreError {
    invalid(error.to_string())
}

#[cfg(unix)]
fn verify_owner_only(_path: &Path, metadata: &fs::Metadata) -> Result<(), RuleStoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mode = metadata.permissions().mode();
    let uid = metadata.uid();
    let effective_uid = unsafe { libc::geteuid() };
    if !unix_owner_only(mode, uid, effective_uid) {
        return Err(if mode & 0o077 != 0 {
            invalid("rules file must have owner-only mode 0600")
        } else {
            invalid("rules file must be owned by the current operator")
        });
    }
    Ok(())
}

#[cfg(unix)]
fn unix_owner_only(mode: u32, uid: u32, effective_uid: u32) -> bool {
    mode & 0o077 == 0 && uid == effective_uid
}

#[cfg(windows)]
fn verify_owner_only(_path: &Path, _metadata: &fs::Metadata) -> Result<(), RuleStoreError> {
    Err(invalid(
        "Windows owner-only ACL verification is unavailable; install the P2a ACL audit helper",
    ))
}

#[cfg(unix)]
fn verify_platform_acl_available() -> Result<(), RuleStoreError> {
    Ok(())
}

#[cfg(windows)]
fn verify_platform_acl_available() -> Result<(), RuleStoreError> {
    Err(invalid(
        "Windows owner-only ACL verification is unavailable; install the P2a ACL audit helper",
    ))
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> Result<File, RuleStoreError> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(invalid_io)
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> Result<File, RuleStoreError> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(invalid_io)
}

#[cfg(unix)]
fn same_open_file(before: &File, after: &File) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let before = before.metadata()?;
    let after = after.metadata()?;
    Ok(before.dev() == after.dev() && before.ino() == after.ino())
}

#[cfg(windows)]
fn same_open_file(before: &File, after: &File) -> io::Result<bool> {
    Ok(file_identity(before)? == file_identity(after)?)
}

#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<(u32, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

fn atomic_owner_only_write(path: &Path, contents: &[u8]) -> Result<(), RuleStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("rules path has no parent"))?;
    let temp_path = parent.join(format!(".rules.toml.tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temp_path, path)?;
        File::open(parent)?.sync_all()?;
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.map_err(RuleStoreError::Io)
}

#[cfg(unix)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn try_acquire(path: &Path) -> Result<Self, RuleStoreError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        lock_file(&file)?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unlock_file(&self.file);
    }
}

#[cfg(unix)]
fn lock_file(file: &File) -> Result<(), RuleStoreError> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
        Err(RuleStoreError::LockBusy)
    } else {
        Err(RuleStoreError::Io(io::Error::last_os_error()))
    }
}

#[cfg(unix)]
fn unlock_file(file: &File) {
    use std::os::fd::AsRawFd;
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(windows)]
fn lock_file(file: &File) -> Result<(), RuleStoreError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    };
    let mut overlapped = lock_overlapped();
    let ok = unsafe {
        LockFileEx(
            file.as_raw_handle() as _,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING)
            | Some(windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION) => {
                Err(RuleStoreError::LockBusy)
            }
            _ => Err(RuleStoreError::Io(error)),
        }
    }
}

#[cfg(windows)]
fn unlock_file(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    let mut overlapped = lock_overlapped();
    unsafe { UnlockFileEx(file.as_raw_handle() as _, 0, 1, 0, &mut overlapped) };
}

#[cfg(windows)]
fn lock_overlapped() -> windows_sys::Win32::System::IO::OVERLAPPED {
    const LOCK_OFFSET: u64 = 0xFFFF_FFFF_FFFF_F000;
    let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.Anonymous.Anonymous.Offset = LOCK_OFFSET as u32;
    overlapped.Anonymous.Anonymous.OffsetHigh = (LOCK_OFFSET >> 32) as u32;
    overlapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_contention_is_typed_and_release_allows_reacquire() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rules.toml.lock");
        let first = FileLock::try_acquire(&path).unwrap();
        assert!(matches!(
            FileLock::try_acquire(&path),
            Err(RuleStoreError::LockBusy)
        ));
        drop(first);
        drop(FileLock::try_acquire(&path).unwrap());
    }

    #[test]
    fn open_file_identity_detects_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rules.toml");
        fs::write(&path, "first").unwrap();
        let first = File::open(&path).unwrap();
        let replacement = temp.path().join("replacement.toml");
        fs::write(&replacement, "second").unwrap();
        #[cfg(unix)]
        let second = {
            fs::rename(&replacement, &path).unwrap();
            File::open(&path).unwrap()
        };
        #[cfg(windows)]
        let second = File::open(&replacement).unwrap();
        assert!(!same_open_file(&first, &second).unwrap());
    }

    #[test]
    fn recursion_and_global_node_caps_are_prompt_floors() {
        let rules = RuleSet::default();
        let mut budget = RecursionBudget::default();
        assert_eq!(
            rules
                .evaluate_layer("echo", ShellGrammar::PosixSh, MAX_DEPTH + 1, &mut budget)
                .complex_reason,
            Some(ComplexReason::RecursionTooDeep)
        );
        let mut budget = RecursionBudget {
            nodes: MAX_NODES,
            ..RecursionBudget::default()
        };
        assert_eq!(
            rules
                .evaluate_layer("echo", ShellGrammar::PosixSh, 0, &mut budget)
                .complex_reason,
            Some(ComplexReason::TooManyNodes)
        );
    }

    #[test]
    fn typed_invalid_reason_truncation_is_utf8_safe() {
        let RuleStoreError::RuleFileInvalid { reason } = invalid("é".repeat(200)) else {
            panic!("expected RuleFileInvalid");
        };
        assert!(reason.len() <= MAX_REASON_BYTES);
        assert!(reason.is_char_boundary(reason.len()));
    }

    #[cfg(unix)]
    #[test]
    fn unix_owner_check_rejects_a_different_uid() {
        assert!(unix_owner_only(0o600, 1000, 1000));
        assert!(!unix_owner_only(0o600, 1001, 1000));
        assert!(!unix_owner_only(0o606, 1000, 1000));
    }
}
