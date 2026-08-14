//! `NanoErrorKind` — the closed, journaled vocabulary of typed error
//! conditions (C7). The kind is part of the journaled op vocabulary, so it
//! lives in the journal crate; every crate in the error-surfacing chain
//! already depends on nano-session.
//!
//! Wire/journal form is snake_case via serde. The `Unknown` variant is the
//! forward-tolerance escape hatch (same discipline as `Op::Unknown`): a
//! journal or frame written by a NEWER build carrying a kind this build
//! does not know deserializes as `Unknown` instead of breaking the fold.
//! Classification rule (design §2/D2): unknown kinds are TERMINAL in both
//! UIs and never auto-retry.

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NanoErrorKind {
    // ── model family (turn-fatal; JSON-RPC error responses) ─────────────
    ModelAuth,
    ModelRateLimited,
    /// Second overflow (or compaction disabled) only — the first overflow
    /// routes to reactive compaction and never surfaces.
    ModelContextOverflow,
    /// The reactive/auto compaction call itself failed.
    CompactionFailed,
    ModelEntitlement,
    // serde's snake_case would render these "model_server5xx"; the table's
    // wire form is pinned with an explicit rename.
    #[serde(rename = "model_server_5xx")]
    ModelServer5xx,
    #[serde(rename = "model_server_4xx")]
    ModelServer4xx,
    ModelTransport,
    ModelProtocol,
    /// Structured-output validation still failing after the one allowed
    /// re-ask (C9 §4.3).
    ModelOutputSchema,
    /// A requested parameter is known-unsupported on this (surface, model)
    /// — rejected before network I/O (C9 §4, ladder rung 3).
    ModelUnsupportedParam,
    EgressDenied,
    // ── tool family (failed tool cards) ─────────────────────────────────
    FsReadDenied,
    FsWriteDenied,
    FsSensitiveDenied,
    FsIo,
    FsEdit,
    ShellSpawn,
    SandboxUnavailable,
    /// The SetupErrorCode family (provisioning-time diagnostics). No emitter
    /// exists on the ACP path yet — the family reaches turns only as
    /// `ShellError::SandboxUnavailable` strings (design §1.1d); the kind is
    /// defined so the table is complete when a typed carrier lands.
    SandboxSetup,
    McpServer,
    McpProtocol,
    McpOutputBounded,
    McpTransport,
    McpTimeout,
    McpResourceUnsupported,
    McpResourceDenied,
    McpContentUnsupported,
    McpElicitationUnsupported,
    McpAuthorizationRequired,
    McpCredstoreUnavailable,
    // serde's snake_case would render this "mcp_o_auth_failed"; the wire
    // form is pinned with an explicit rename (the model_server_5xx pattern).
    #[serde(rename = "mcp_oauth_failed")]
    McpOAuthFailed,
    UnknownTool,
    MissingArgs,
    ApprovalDenied,
    HookBlocked,
    /// P4 §8: write/read/kill against an unknown or exited PTY session
    /// (§4.4). The exit code rides the tool-card detail when known; the
    /// presentation stays static.
    PtySessionGone,
    /// P4 review mode §8: the review turn finished but its findings output
    /// failed the strict JSON parse AND the plain-text fallback (empty).
    /// Never retryable — the model output will not improve on resend.
    ReviewParseFailed,
    /// P4 §8/§2.6: a `Deny` shell rule matched the command (any segment,
    /// any mode). Never retryable — resending the identical call denies
    /// again; the operator edits rules.toml to change the outcome.
    ShellRuleDenied,
    /// P4 §8/§2.5: `rules.toml` failed the fail-closed load gates (strict
    /// parse, §2.1 validation, ownership/ACL audit, non-symlink,
    /// containment) at load or amendment; the session runs with zero saved
    /// rules. Never retryable — the file must be fixed or removed.
    RuleFileInvalid,
    // ── engine stops (turn-fatal, NOT user cancels) ─────────────────────
    BudgetExhausted,
    /// P1 §4.1: the session token cap stopped the turn (a zero output
    /// reservation = hard stop). Distinct from `BudgetExhausted` (the
    /// per-turn step/tool-call budget): the wire contract carries
    /// `{type: budget_exceeded, limit, observed, reason}` and the session
    /// stays alive for `/budget continue`, read-only commands, and `/quit`.
    BudgetExceeded,
    NoProgress,
    RepeatForceStop,
    /// The ONLY kind that is never an error: it surfaces as
    /// `stopReason: "cancelled"`.
    UserCancelled,
    // ── request/session level (JSON-RPC error responses) ────────────────
    JournalUnavailable,
    SessionNotFound,
    ModelNotFound,
    TurnInProgress,
    NoSession,
    InvalidParams,
    /// A `_wayland/session/fork` failure (C11): guard-busy, missing parent,
    /// or journal I/O — the static presentation is the wire surface; the
    /// detail stays logs-side.
    SessionForkFailed,
    /// A `_wayland/goal/*` failure (C11): transition rejection or journal
    /// I/O — same static-presentation discipline.
    GoalOpFailed,
    // ── vision intake family (P2a §7; all fail-closed, never retryable) ───
    /// An image-bearing prompt/request against a current model leaf that is
    /// not vision-proven in the static catalog (§6.2 rungs 1+3). Zero egress
    /// precedes this rejection.
    ModelLacksVision,
    /// Sniff failure, claim-vs-sniff mismatch, corrupt/truncated decode,
    /// structural-parse failure, or a contained decoder panic (§4.3).
    ImageInvalid,
    /// The sniffed format is outside the closed intake set {png, jpeg, gif,
    /// webp} (SVG/BMP/TIFF are rejected by construction, §4.3 step 2).
    ImageUnsupportedFormat,
    /// File bytes, header-declared pixels/dimensions, or the post-ladder
    /// wire payload exceeded a §4.2 cap (binary units throughout).
    ImageTooLarge,
    /// More than 16 images or more than 50 MiB aggregate in one prompt
    /// (§4.2 count/aggregate caps).
    ImageTooMany,
    /// Resume rehydration found a digest with no verifiable blob, or a
    /// malformed digest (§5.3) — resume continues with a loud placeholder.
    AttachmentMissing,
    /// Attachment store open/audit/IO failure, incl. the Windows ACL audit
    /// (§5.5) and GC lock failure (§5.4). Fail-closed.
    AttachmentStoreError,
    /// Checkpoint storage or its required system-Git backend is unavailable.
    CheckpointUnavailable,
    /// The requested checkpoint is unknown or has been evicted.
    CheckpointNotFound,
    /// A restore failed after its durable begin marker was appended.
    CheckpointRestoreFailed,
    /// A kind written by a newer build (forward tolerance — deserialize
    /// only; never constructed by this build's mappers).
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&NanoErrorKind::ModelRateLimited).unwrap(),
            "\"model_rate_limited\""
        );
        assert_eq!(
            serde_json::to_string(&NanoErrorKind::ModelServer5xx).unwrap(),
            "\"model_server_5xx\""
        );
        assert_eq!(
            serde_json::from_str::<NanoErrorKind>("\"model_server_4xx\"").unwrap(),
            NanoErrorKind::ModelServer4xx
        );
        assert_eq!(
            serde_json::from_str::<NanoErrorKind>("\"approval_denied\"").unwrap(),
            NanoErrorKind::ApprovalDenied
        );
    }

    #[test]
    fn unknown_kind_deserializes_to_the_unknown_variant() {
        assert_eq!(
            serde_json::from_str::<NanoErrorKind>("\"kind_from_the_future\"").unwrap(),
            NanoErrorKind::Unknown
        );
    }
}
