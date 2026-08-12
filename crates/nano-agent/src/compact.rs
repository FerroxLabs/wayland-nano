//! Context management (C1): token accounting, trigger math, the compaction
//! call, the canonical compacted-history builder, the repair-pass safety net,
//! and the auto-compaction loop guard.
//!
//! Provenance: adapted from the codex donor (`core/src/compact.rs`,
//! `prompts/templates/compact/prompt.md`, `session/mod.rs` token accounting)
//! — see UPSTREAM.md. The accounting is codex's layered model: the LAST
//! REQUEST's server-reported `Usage.input_tokens` is authoritative, a 25%-
//! inflated 4-bytes/token heuristic covers only the tail appended after the
//! last server sample. The heuristic is best-effort early warning (it can
//! undershoot badly on CJK/base64); the reactive-overflow path in the turn
//! loop is the correctness backstop. That coupling is deliberate.
//!
//! The journal-first commit protocol lives in [`compact_messages`] and is the
//! ONE pipeline for both auto and manual compaction (C1 §6/§7):
//! Begin → summarize → redaction gate → Complete (durable) → in-memory swap.
//! The swap is infallible; the only fallible steps are the journal appends,
//! reported through `emit`'s return value. The reverse order (swap-then-
//! journal) is forbidden.

use crate::turn::ModelDriver;
use nano_model::types::{ContentBlock, Message, ModelError, ModelEvent, ModelRequest, Role, Usage};
use nano_session::op::{CompactionCancelReason, Op};
use nano_session::redaction::{self, RedactionError};

/// Heuristic: 4 bytes per token (codex's `approx_token_count`).
pub const BYTES_PER_TOKEN: u64 = 4;
/// The tail estimate is inflated this much before comparison: the heuristic
/// is an early warning, so it errs toward compacting early.
pub const TAIL_INFLATION_PERCENT: u64 = 25;
/// Auto-compaction triggers at this fraction of the context window (codex's
/// `auto_compact_token_limit` default).
pub const TRIGGER_PERCENT: u64 = 90;
/// Cap on retained real user messages in the compacted history (codex's
/// `COMPACT_USER_MESSAGE_MAX_TOKENS`).
pub const COMPACT_USER_MESSAGE_MAX_TOKENS: u64 = 20_000;
/// Short output budget for the summarization call.
pub const SUMMARY_MAX_TOKENS: u32 = 2_048;
/// Marker prefixing the summary message in the compacted history. Also how
/// the builder recognizes (and excludes) prior summaries when collecting
/// real user messages.
pub const SUMMARY_PREFIX: &str = "[wayland-nano compaction summary]";
/// The guard trips after this many CONSECUTIVE auto-compactions fail to
/// bring the estimate back under the limit.
pub const GUARD_TRIP_AT: u32 = 2;

/// Handoff-summary prompt, adapted from the codex donor
/// (`prompts/templates/compact/prompt.md`) with the C1 §8 credentials
/// exclusion added (defense-in-depth; the redaction gate is the real gate).
pub const SUMMARIZATION_PROMPT: &str = "\
You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary \
for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Critical file paths, identifiers, and any data or references needed to continue

Be concise, structured, and focused on helping the next LLM seamlessly \
continue the work.

NEVER include credentials, API keys, tokens, or any secret values — refer to \
them by name only (e.g. \"the FLUX_API_KEY env var\"). This summary is \
persisted to a session journal.";

/// Estimated token count of a byte length (4 bytes/token, rounded up).
pub fn approx_token_count(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(BYTES_PER_TOKEN)
}

/// Byte weight of one message for the heuristic: text, tool-call
/// names/inputs, and tool-result payloads all count.
fn message_bytes(message: &Message) -> usize {
    message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len(),
            ContentBlock::ToolUse { id, name, input } => {
                id.len() + name.len() + input.to_string().len()
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => tool_use_id.len() + content.len(),
        })
        .sum()
}

/// Whole-history heuristic estimate (used pre-first-server-sample and for
/// the post-compaction recompute).
pub fn estimate_messages_tokens(messages: &[Message]) -> u64 {
    approx_token_count(messages.iter().map(message_bytes).sum())
}

/// Token accounting state (C1 §2): the last server-reported
/// `input_tokens` (a proxy for current context size, NEVER a cumulative
/// total) plus how many messages that sample covered.
#[derive(Debug, Default)]
pub struct TokenTracker {
    last_server_input: Option<u64>,
    covered_messages: usize,
}

impl TokenTracker {
    /// Records the server sample from one model response. `covered_messages`
    /// is the message count the request carried. A zero sample is no sample
    /// (scripted/fake drivers report 0) and never re-baselines.
    pub fn record_usage(&mut self, usage: &Usage, covered_messages: usize) {
        if usage.input_tokens > 0 {
            self.last_server_input = Some(usage.input_tokens);
            self.covered_messages = covered_messages;
        }
    }

    /// Current context estimate: server sample + inflated tail heuristic for
    /// items appended after the sample; whole-history heuristic when no
    /// sample exists yet (pre-turn on a resumed context).
    pub fn estimate(&self, messages: &[Message]) -> u64 {
        match self.last_server_input {
            Some(server) => {
                let tail = &messages[self.covered_messages.min(messages.len())..];
                let tail = estimate_messages_tokens(tail);
                server + tail * (100 + TAIL_INFLATION_PERCENT) / 100
            }
            None => estimate_messages_tokens(messages),
        }
    }

    /// Post-compaction recompute: drop the (now stale) server sample so the
    /// next estimate is the whole-history heuristic over the small compacted
    /// history; the next real sample re-baselines.
    pub fn reset(&mut self) {
        self.last_server_input = None;
        self.covered_messages = 0;
    }
}

/// 90% trigger over the active model's context window.
pub fn default_auto_compact_limit(context_window: u64) -> u64 {
    context_window * TRIGGER_PERCENT / 100
}

/// Resolved compaction settings for one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub context_window: u64,
    pub auto_compact_limit: u64,
}

/// Typed config rejection: overrides are DOWNWARD-ONLY (C1 §2/§3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactionConfigError {
    #[error(
        "context window override {requested} exceeds the catalog/default window {window} (overrides are downward-only)"
    )]
    WindowOverrideUpward { requested: u64, window: u64 },
    #[error("context window override must be positive")]
    WindowOverrideZero,
    #[error(
        "auto-compact limit override {requested} exceeds the derived limit {limit} (overrides are downward-only)"
    )]
    LimitOverrideUpward { requested: u64, limit: u64 },
    #[error("auto-compact limit override must be positive")]
    LimitOverrideZero,
}

/// Resolve the context window and auto-compact limit for one model. The
/// catalog window comes from `flux_common::context_window_for` (128k default
/// for unknown models). Overrides may only shrink, never grow.
pub fn resolve_compaction_config(
    catalog_window: u64,
    window_override: Option<u64>,
    limit_override: Option<u64>,
) -> Result<CompactionConfig, CompactionConfigError> {
    let window = match window_override {
        Some(0) => return Err(CompactionConfigError::WindowOverrideZero),
        Some(requested) if requested > catalog_window => {
            return Err(CompactionConfigError::WindowOverrideUpward {
                requested,
                window: catalog_window,
            });
        }
        Some(requested) => requested,
        None => catalog_window,
    };
    let derived = default_auto_compact_limit(window);
    let limit = match limit_override {
        Some(0) => return Err(CompactionConfigError::LimitOverrideZero),
        Some(requested) if requested > derived => {
            return Err(CompactionConfigError::LimitOverrideUpward {
                requested,
                limit: derived,
            });
        }
        Some(requested) => requested,
        None => derived,
    };
    Ok(CompactionConfig {
        context_window: window,
        auto_compact_limit: limit,
    })
}

/// Counts CONSECUTIVE auto-compactions with no intervening re-baseline
/// below the trigger (C1 §3). Any estimate below the limit resets the
/// counter, so long productive sessions are never capped; the guard trips
/// only when summarization is genuinely not saving the session. Manual
/// /compact never touches this counter.
#[derive(Debug, Default)]
pub struct AutoCompactGuard {
    consecutive_ineffective: u32,
}

impl AutoCompactGuard {
    /// A (re-)estimate came in below the limit: re-arm the guard.
    pub fn reset(&mut self) {
        self.consecutive_ineffective = 0;
    }

    /// An auto-compaction's post-swap re-estimate is STILL at/above the
    /// limit. Returns true when the guard trips (turn must fail typed).
    pub fn record_ineffective(&mut self) -> bool {
        self.consecutive_ineffective += 1;
        self.consecutive_ineffective >= GUARD_TRIP_AT
    }
}

/// Why a compaction attempt failed. Every variant fails closed: the turn
/// fails safe, and history is never swapped without a durable journaled
/// CompactionComplete.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactionError {
    #[error("journal write failed during the compaction commit protocol")]
    JournalWrite,
    #[error("summarization model call failed: {0}")]
    ModelFailed(String),
    #[error("compaction call still overflows after pair-preserving escalation down to one item")]
    OverflowEscalationExhausted,
    #[error("summary rejected by the pre-persistence secret scan")]
    RedactionHit,
    #[error("secret scanner error (fail-closed): {0}")]
    RedactorError(String),
    #[error("summarization returned an empty summary")]
    EmptySummary,
}

impl CompactionError {
    /// The bounded, non-sensitive journal reason for this failure.
    pub fn cancel_reason(&self) -> CompactionCancelReason {
        match self {
            CompactionError::JournalWrite => CompactionCancelReason::JournalWriteFailed,
            CompactionError::ModelFailed(_) | CompactionError::EmptySummary => {
                CompactionCancelReason::ModelFailed
            }
            CompactionError::OverflowEscalationExhausted => {
                CompactionCancelReason::OverflowEscalationExhausted
            }
            CompactionError::RedactionHit => CompactionCancelReason::RedactionHit,
            CompactionError::RedactorError(_) => CompactionCancelReason::RedactorError,
        }
    }
}

/// The journal-first compaction commit protocol (C1 §6) — the ONE pipeline
/// for auto and manual compaction:
///
/// 1. journal `CompactionBegin` BEFORE the summarization call;
/// 2. summarize, then the mandatory pre-persistence redaction gate;
/// 3. journal `CompactionComplete` (which the caller's `emit` makes durable
///    — the journal writer fsyncs per append);
/// 4. ONLY THEN swap the in-memory history (infallible plain assignment).
///
/// `emit` appends one op and returns whether it is durably journaled. Any
/// failure journals `CompactionCancel` (best-effort — the journal may be the
/// thing that failed) and returns a typed error with `messages` untouched.
pub async fn compact_messages(
    model: &dyn ModelDriver,
    model_name: &str,
    messages: &mut Vec<Message>,
    compaction_id: &str,
    covers_op_ids: Vec<String>,
    changed_files: Vec<String>,
    emit: &mut dyn FnMut(Op) -> bool,
) -> Result<(), CompactionError> {
    if !emit(Op::CompactionBegin {
        compaction_id: compaction_id.to_string(),
    }) {
        return Err(CompactionError::JournalWrite);
    }
    let summary = match summarize(model, model_name, messages).await {
        Ok(summary) => summary,
        Err(err) => {
            let _ = emit(Op::CompactionCancel {
                compaction_id: compaction_id.to_string(),
                reason: err.cancel_reason(),
            });
            return Err(err);
        }
    };
    match redaction::scan_for_secrets(&summary) {
        Ok(()) => {}
        Err(RedactionError::Secret(_)) => {
            let _ = emit(Op::CompactionCancel {
                compaction_id: compaction_id.to_string(),
                reason: CompactionCancelReason::RedactionHit,
            });
            return Err(CompactionError::RedactionHit);
        }
        Err(scanner) => {
            let _ = emit(Op::CompactionCancel {
                compaction_id: compaction_id.to_string(),
                reason: CompactionCancelReason::RedactorError,
            });
            return Err(CompactionError::RedactorError(scanner.to_string()));
        }
    }
    if !emit(Op::CompactionComplete {
        compaction_id: compaction_id.to_string(),
        summary: summary.clone(),
        covers_op_ids,
        changed_files,
    }) {
        let _ = emit(Op::CompactionCancel {
            compaction_id: compaction_id.to_string(),
            reason: CompactionCancelReason::JournalWriteFailed,
        });
        return Err(CompactionError::JournalWrite);
    }
    // Complete is durable; the swap itself is infallible.
    *messages = build_compacted_history(std::mem::take(messages), &summary);
    Ok(())
}

/// One summarization call against the CURRENT model: prompt appended as a
/// synthetic user message to a clone of the history, ordinary
/// `ModelDriver::complete`, no streaming. Overflow during the compaction
/// call escalates codex-style: remove the oldest item pair-preservingly and
/// retry; give up at one item with a typed error.
pub async fn summarize(
    model: &dyn ModelDriver,
    model_name: &str,
    messages: &[Message],
) -> Result<String, CompactionError> {
    let mut working: Vec<Message> = messages.to_vec();
    loop {
        let mut request_messages = working.clone();
        request_messages.push(Message::user(SUMMARIZATION_PROMPT));
        let request = ModelRequest {
            model: model_name.to_string(),
            messages: request_messages,
            tools: Vec::new(),
            max_tokens: Some(SUMMARY_MAX_TOKENS),
            stream: false,
            ..Default::default()
        };
        match model.complete(&request).await {
            Ok(response) => {
                let text: String = response
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        ModelEvent::TextDelta(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Err(CompactionError::EmptySummary);
                }
                return Ok(trimmed.to_string());
            }
            Err(ModelError::ContextOverflow(_)) => {
                if working.len() <= 1 {
                    return Err(CompactionError::OverflowEscalationExhausted);
                }
                remove_oldest_pair_preserving(&mut working);
            }
            Err(err) => return Err(CompactionError::ModelFailed(err.to_string())),
        }
    }
}

/// Drops the oldest message; when it carried tool uses (or orphaned tool
/// results land at the head), the now-unpaired head tool messages go with
/// it, so the escalation never strands a pair.
fn remove_oldest_pair_preserving(messages: &mut Vec<Message>) {
    if messages.is_empty() {
        return;
    }
    messages.remove(0);
    while let Some(head) = messages.first() {
        let orphan = head.role == Role::Tool
            && head.content.iter().all(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => !messages.iter().any(|m| {
                    m.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if id == tool_use_id))
                }),
                _ => false,
            });
        if orphan {
            messages.remove(0);
        } else {
            break;
        }
    }
}

/// The ONE canonical compacted-history builder (C1 §5), called by BOTH the
/// live compaction path and the journal replay fold, so live and resumed
/// contexts are byte-identical by construction:
///
/// 1. all REAL user messages, verbatim, newest-first collection under an
///    estimated-token cap (the message crossing the cap keeps only its tail,
///    cut at a UTF-8 char boundary — fully deterministic); prior summaries
///    (SUMMARY_PREFIX) and mid-turn system nags are excluded;
/// 2. the summary appended as a final user-role message with SUMMARY_PREFIX;
/// 3. assistant messages, tool calls, and tool outputs discarded wholesale
///    (pairing preserved by removal);
/// 4. the pair-invariant repair pass as safety net.
pub fn build_compacted_history(messages: Vec<Message>, summary: &str) -> Vec<Message> {
    let mut budget_bytes = (COMPACT_USER_MESSAGE_MAX_TOKENS * BYTES_PER_TOKEN) as usize;
    let mut kept: Vec<Message> = Vec::new();
    for message in messages.iter().rev() {
        if message.role != Role::User {
            continue; // rule 3 + mid-turn system nags are not user intent
        }
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        if text.is_empty() || text.starts_with(SUMMARY_PREFIX) {
            continue; // prior summaries are replaced, never stacked
        }
        if text.len() <= budget_bytes {
            kept.push(message.clone());
            budget_bytes -= text.len();
        } else {
            // The overflow message keeps its TAIL (the part nearest to the
            // present), cut at a char boundary — deterministic, so live and
            // replay produce byte-identical output.
            let mut start = text.len() - budget_bytes;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            kept.push(Message::user(&text[start..]));
            break;
        }
    }
    kept.reverse();
    kept.push(Message::user(format!("{SUMMARY_PREFIX}\n{summary}")));
    normalize_history(kept)
}

/// Pair-invariant repair pass (codex's `normalize_history` equivalent): every
/// tool_use gets a tool_result (missing ones are synthesized through the ONE
/// shared encoding, `Message::tool_result`) and orphan results are dropped.
/// The compacted history should never need this — it is the safety net, also
/// exercised directly against synthetic tears in tests.
pub fn normalize_history(messages: Vec<Message>) -> Vec<Message> {
    let has_use = |messages: &[Message], id: &str| {
        messages.iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { id: use_id, .. } if use_id == id))
        })
    };
    let has_result = |messages: &[Message], id: &str| {
        messages.iter().any(|m| {
            m.content.iter().any(
                |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id),
            )
        })
    };
    // Drop orphan results (no matching tool_use) and synthesize missing
    // results, decided against the pre-repair snapshot.
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let snapshot = messages.clone();
    for message in messages {
        let missing: Vec<String> = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, .. } if !has_result(&snapshot, id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let is_orphan_result = message.role == Role::Tool
            && message.content.iter().all(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => !has_use(&snapshot, tool_use_id),
                _ => false,
            });
        if is_orphan_result {
            continue;
        }
        out.push(message);
        for id in missing {
            out.push(Message::tool_result(
                id,
                "[tool result missing: the call did not complete]",
                true,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests;
