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
    UnknownTool,
    MissingArgs,
    ApprovalDenied,
    // ── engine stops (turn-fatal, NOT user cancels) ─────────────────────
    BudgetExhausted,
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
