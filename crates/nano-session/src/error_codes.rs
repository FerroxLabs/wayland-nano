//! The C7 error-code table — the single source of truth mapping every
//! [`NanoErrorKind`] to its wire surface, numeric code, retryability, and
//! static UI presentation.
//!
//! Placement note (C7 deviation, recorded in the evidence file): the design
//! sited this module in nano-protocol; it lives HERE, in the serde-only
//! journal-vocabulary crate, so nano-tui can render table presentations
//! without linking the engine closure (which drags ring/C-toolchain into
//! the scoped `cargo check -p nano-tui --target x86_64-unknown-linux-gnu`
//! gate). nano-protocol re-exports the entire surface, so every
//! design-cited path (`nano_protocol::error_codes::*`) still resolves.
//!
//! Rules (design §2/§3/§7):
//! - `spec()` is ONE exhaustive wildcard-free match: a new kind without a
//!   table entry does not compile.
//! - Numeric codes stay standard JSON-RPC (-32602/-32603); typing rides in
//!   `error.data.nanoError` / `_meta.nanoError`, never in private code
//!   blocks.
//! - Presentations are STATIC strings. Provider free-text never reaches a
//!   UI-bound frame; triage detail rides as closed typed fields (`status`,
//!   `retry_after_ms`, egress-redacted `host`) in `data`.
//! - `retryable` means "re-sending the SAME request as-is may succeed";
//!   it must agree with `nano_model::retry::is_retryable` for the model
//!   family (pinned by the agreement test in nano-protocol).
//!
//! The table ALSO feeds the generator (`cargo run -p nano-cli --bin
//! gen_error_table`) via [`render_json`]/[`render_ts`]; the parity test
//! below pins the committed JSON artifact byte-for-byte.

use crate::NanoErrorKind;

/// Where a kind surfaces on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSurface {
    /// A JSON-RPC error response carrying `data.nanoError`.
    ErrorResponse,
    /// A failed `tool_call_update` card (`content` presentation +
    /// `_meta.nanoError`); the numeric code is nominal only.
    ToolCard,
    /// Never an error frame: `stopReason: "cancelled"` on a normal result.
    StopReason,
}

impl ErrorSurface {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorSurface::ErrorResponse => "error_response",
            ErrorSurface::ToolCard => "tool_card",
            ErrorSurface::StopReason => "stop_reason",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorSpec {
    pub surface: ErrorSurface,
    /// Standard JSON-RPC code for `ErrorResponse` kinds. Nominal (-32603)
    /// for tool-card kinds, which are never error responses.
    pub wire_code: i64,
    /// "Re-sending the SAME request as-is may succeed."
    pub retryable: bool,
    /// Static UI title — the only text a UI needs for this kind.
    pub title: &'static str,
    /// Static actionable hint (empty when there is nothing useful to add).
    pub hint: &'static str,
}

const fn response(
    wire_code: i64,
    retryable: bool,
    title: &'static str,
    hint: &'static str,
) -> ErrorSpec {
    ErrorSpec {
        surface: ErrorSurface::ErrorResponse,
        wire_code,
        retryable,
        title,
        hint,
    }
}

const fn card(title: &'static str, hint: &'static str) -> ErrorSpec {
    ErrorSpec {
        surface: ErrorSurface::ToolCard,
        wire_code: -32603,
        retryable: false,
        title,
        hint,
    }
}

/// The transient tool-card kinds (mcp transport/timeout): a fresh identical
/// call MAY succeed.
const fn card_retryable(title: &'static str, hint: &'static str) -> ErrorSpec {
    ErrorSpec {
        surface: ErrorSurface::ToolCard,
        wire_code: -32603,
        retryable: true,
        title,
        hint,
    }
}

/// The one exhaustive mapping. A new `NanoErrorKind` variant fails THIS
/// match at compile time — the table can never silently under-cover.
pub fn spec(kind: NanoErrorKind) -> ErrorSpec {
    match kind {
        NanoErrorKind::ModelAuth => response(
            -32603,
            false,
            "Authentication failed",
            "Check the provider's API key, then retry",
        ),
        NanoErrorKind::ModelRateLimited => response(
            -32603,
            true,
            "Rate limited",
            "Retrying automatically; wait a moment",
        ),
        NanoErrorKind::ModelContextOverflow => response(
            -32603,
            false,
            "Context too long",
            "Start a new session or /compact",
        ),
        NanoErrorKind::CompactionFailed => response(
            -32603,
            false,
            "Context compaction failed",
            "Start a new session",
        ),
        NanoErrorKind::ModelEntitlement => response(
            -32603,
            false,
            "Model not entitled",
            "Switch model or check plan",
        ),
        NanoErrorKind::ModelServer5xx => response(
            -32603,
            true,
            "Provider server error (5xx)",
            "Transient; retry",
        ),
        NanoErrorKind::ModelServer4xx => response(
            -32603,
            false,
            "Provider rejected the request (4xx)",
            "Check model and account",
        ),
        NanoErrorKind::ModelTransport => response(
            -32603,
            true,
            "Network error reaching provider",
            "Check connectivity, then retry",
        ),
        NanoErrorKind::ModelProtocol => response(
            -32603,
            false,
            "Malformed provider stream",
            "Report; not retryable",
        ),
        NanoErrorKind::ModelOutputSchema => response(
            -32603,
            false,
            "Structured output rejected",
            "Loosen the schema or retry without it",
        ),
        NanoErrorKind::ModelUnsupportedParam => response(
            -32603,
            false,
            "Parameter unsupported on this model",
            "Clear the named setting, then retry",
        ),
        NanoErrorKind::EgressDenied => response(
            -32603,
            false,
            "Blocked by egress policy",
            "Report this — a Flux-policy trip is a bug",
        ),
        NanoErrorKind::FsReadDenied
        | NanoErrorKind::FsWriteDenied
        | NanoErrorKind::FsSensitiveDenied => card(
            "Denied by policy",
            "Path is outside the allowed set; ask the user",
        ),
        NanoErrorKind::FsIo => card("File operation failed", ""),
        NanoErrorKind::FsEdit => card("Edit matched 0 or ambiguous ranges", "Re-read the file"),
        NanoErrorKind::ShellSpawn => card("Sandboxed launch failed", ""),
        NanoErrorKind::SandboxUnavailable => {
            card("Sandbox unavailable — command not run (fail-closed)", "")
        }
        NanoErrorKind::SandboxSetup => card("Sandbox setup failed", ""),
        NanoErrorKind::McpServer => card("MCP server reported an error", ""),
        NanoErrorKind::McpProtocol => card("MCP server sent a malformed response", ""),
        NanoErrorKind::McpOutputBounded => card("MCP server output exceeded the bound", ""),
        NanoErrorKind::McpTransport => card_retryable("MCP server unreachable", ""),
        NanoErrorKind::McpTimeout => card_retryable("MCP server timed out", ""),
        NanoErrorKind::McpResourceUnsupported => {
            card("This MCP server doesn't expose resources.", "")
        }
        NanoErrorKind::McpResourceDenied => card(
            "That resource URI wasn't advertised by the server; list resources first.",
            "",
        ),
        NanoErrorKind::McpContentUnsupported => {
            card("Binary resource content isn't supported yet.", "")
        }
        NanoErrorKind::McpElicitationUnsupported => card(
            "This MCP server needs a question flow Nano doesn't support on this transport.",
            "",
        ),
        NanoErrorKind::McpAuthorizationRequired => {
            card("Login required: nano auth login <server>.", "")
        }
        NanoErrorKind::McpCredstoreUnavailable => card(
            "Secure credential storage is unavailable; provision <VAR>_FILE (0600) or fix the keyring.",
            "",
        ),
        NanoErrorKind::McpOAuthFailed => card("OAuth login failed (<bounded reason>).", ""),
        NanoErrorKind::UnknownTool => card("Unknown tool", ""),
        NanoErrorKind::MissingArgs => card("Bad tool arguments", ""),
        NanoErrorKind::ApprovalDenied => card("Denied by user", ""),
        NanoErrorKind::HookBlocked => card("Blocked by lifecycle hook", ""),
        // P4 §8: tool-card surface (a failed pty_* call), never retryable —
        // the session is gone; re-sending the identical call cannot succeed.
        NanoErrorKind::PtySessionGone => card("That terminal session has exited", ""),
        // P4 review mode §8: tool-card surface on the review_result notice,
        // never retryable — the model's report will not improve on resend.
        NanoErrorKind::ReviewParseFailed => {
            card("The review finished but its report couldn't be parsed", "")
        }
        // P4 §8: a Deny rule matched — the static card names the kind; the
        // bounded rule index + matched prefix ride the tool-result text.
        NanoErrorKind::ShellRuleDenied => card(
            "Denied by a shell rule",
            "The saved rules.toml denies this command; edit or remove the rule",
        ),
        // P4 §8: session-start/amendment load refusal (§2.5). The session
        // continues with zero saved rules; the presentation is the loud
        // half of the fail-closed posture (also the /doctor line's detail).
        NanoErrorKind::RuleFileInvalid => card(
            "Shell rules file is invalid or insecurely configured; running with no saved rules",
            "Fix or remove rules.toml (strict TOML, owner-only permissions)",
        ),
        NanoErrorKind::BudgetExhausted => response(
            -32603,
            false,
            "Turn stopped: budget exhausted",
            "Start a new session or narrow the task",
        ),
        NanoErrorKind::BudgetExceeded => response(
            -32603,
            false,
            "Session token budget exceeded",
            "/budget continue <tokens> grants more, or start a new session",
        ),
        NanoErrorKind::NoProgress => response(
            -32603,
            false,
            "Turn stopped: no observable progress",
            "Rephrase or narrow the task",
        ),
        NanoErrorKind::RepeatForceStop => response(
            -32603,
            false,
            "Turn stopped: repeated identical tool call",
            "Redirect the model with a new instruction",
        ),
        NanoErrorKind::UserCancelled => ErrorSpec {
            surface: ErrorSurface::StopReason,
            wire_code: 0,
            retryable: false,
            title: "Cancelled",
            hint: "",
        },
        NanoErrorKind::JournalUnavailable => response(
            -32603,
            false,
            "Session storage unavailable",
            "Check disk and permissions; do not retry blindly",
        ),
        NanoErrorKind::SessionNotFound => response(-32602, false, "Session not found", ""),
        NanoErrorKind::ModelNotFound => response(
            -32602,
            false,
            "Model not available",
            "Pick a model from the advertised catalog",
        ),
        NanoErrorKind::TurnInProgress => response(
            -32602,
            false,
            "A turn is already running",
            "Wait for it or cancel it first",
        ),
        NanoErrorKind::NoSession => response(-32602, false, "No session", "Call session/new first"),
        NanoErrorKind::InvalidParams => response(-32602, false, "Invalid request parameters", ""),
        NanoErrorKind::SessionForkFailed => response(
            -32603,
            false,
            "Session fork failed",
            "Check the session id; retry when no turn is running",
        ),
        NanoErrorKind::GoalOpFailed => response(
            -32603,
            false,
            "Goal operation failed",
            "Check goal status, then retry",
        ),
        // ── P2a §7 vision intake family ─────────────────────────────────
        // Intake/rung rejections of `session/prompt`: request-level
        // rejections (-32602), same family as InvalidParams/ModelNotFound.
        NanoErrorKind::ModelLacksVision => response(
            -32602,
            false,
            "This model can't process images",
            "Switch to a vision-capable model (/model)",
        ),
        NanoErrorKind::ImageInvalid => response(
            -32602,
            false,
            "The image couldn't be read",
            "Corrupt or not really an image",
        ),
        NanoErrorKind::ImageUnsupportedFormat => response(
            -32602,
            false,
            "That image format isn't supported",
            "Convert to PNG or JPEG and retry",
        ),
        NanoErrorKind::ImageTooLarge => response(
            -32602,
            false,
            "The image is too large",
            "Limit: 50 MiB file / 768 KiB after compression",
        ),
        NanoErrorKind::ImageTooMany => response(
            -32602,
            false,
            "Too many images in one prompt",
            "Limit: 16 images",
        ),
        // Resume-degradation kinds surface via session/update notices, never
        // as error responses; the tool-card surface carries the static
        // presentation (the SandboxSetup precedent).
        NanoErrorKind::AttachmentMissing => card(
            "An earlier attachment couldn't be restored from the attachment store",
            "",
        ),
        NanoErrorKind::AttachmentStoreError => card(
            "The attachment store is unavailable or insecurely configured",
            "",
        ),
        NanoErrorKind::CheckpointUnavailable => card(
            "Workspace checkpoints are unavailable",
            "Check system Git, checkpoint storage permissions, and store contention",
        ),
        NanoErrorKind::CheckpointNotFound => card(
            "Checkpoint not found",
            "It may have been evicted by checkpoint retention",
        ),
        NanoErrorKind::CheckpointRestoreFailed => card(
            "Checkpoint restore did not complete",
            "Resume the session to run checkpoint recovery",
        ),
        // Unknown kinds classify TERMINAL in both clients and never retry
        // (design §2/D2 forward-compat rule).
        NanoErrorKind::Unknown => response(
            -32603,
            false,
            "Unknown engine error",
            "Report; not retryable",
        ),
    }
}

/// Every kind the table covers, in declaration order — feeds the generator.
/// The pinned-count test below guards against forgetful updates.
pub const ALL_KINDS: &[NanoErrorKind] = &[
    NanoErrorKind::ModelAuth,
    NanoErrorKind::ModelRateLimited,
    NanoErrorKind::ModelContextOverflow,
    NanoErrorKind::CompactionFailed,
    NanoErrorKind::ModelEntitlement,
    NanoErrorKind::ModelServer5xx,
    NanoErrorKind::ModelServer4xx,
    NanoErrorKind::ModelTransport,
    NanoErrorKind::ModelProtocol,
    NanoErrorKind::ModelOutputSchema,
    NanoErrorKind::ModelUnsupportedParam,
    NanoErrorKind::EgressDenied,
    NanoErrorKind::FsReadDenied,
    NanoErrorKind::FsWriteDenied,
    NanoErrorKind::FsSensitiveDenied,
    NanoErrorKind::FsIo,
    NanoErrorKind::FsEdit,
    NanoErrorKind::ShellSpawn,
    NanoErrorKind::SandboxUnavailable,
    NanoErrorKind::SandboxSetup,
    NanoErrorKind::McpServer,
    NanoErrorKind::McpProtocol,
    NanoErrorKind::McpOutputBounded,
    NanoErrorKind::McpTransport,
    NanoErrorKind::McpTimeout,
    NanoErrorKind::McpResourceUnsupported,
    NanoErrorKind::McpResourceDenied,
    NanoErrorKind::McpContentUnsupported,
    NanoErrorKind::McpElicitationUnsupported,
    NanoErrorKind::McpAuthorizationRequired,
    NanoErrorKind::McpCredstoreUnavailable,
    NanoErrorKind::McpOAuthFailed,
    NanoErrorKind::UnknownTool,
    NanoErrorKind::MissingArgs,
    NanoErrorKind::ApprovalDenied,
    NanoErrorKind::HookBlocked,
    NanoErrorKind::PtySessionGone,
    NanoErrorKind::ReviewParseFailed,
    NanoErrorKind::ShellRuleDenied,
    NanoErrorKind::RuleFileInvalid,
    NanoErrorKind::BudgetExhausted,
    NanoErrorKind::BudgetExceeded,
    NanoErrorKind::NoProgress,
    NanoErrorKind::RepeatForceStop,
    NanoErrorKind::UserCancelled,
    NanoErrorKind::JournalUnavailable,
    NanoErrorKind::SessionNotFound,
    NanoErrorKind::ModelNotFound,
    NanoErrorKind::TurnInProgress,
    NanoErrorKind::NoSession,
    NanoErrorKind::InvalidParams,
    NanoErrorKind::SessionForkFailed,
    NanoErrorKind::GoalOpFailed,
    NanoErrorKind::ModelLacksVision,
    NanoErrorKind::ImageInvalid,
    NanoErrorKind::ImageUnsupportedFormat,
    NanoErrorKind::ImageTooLarge,
    NanoErrorKind::ImageTooMany,
    NanoErrorKind::AttachmentMissing,
    NanoErrorKind::AttachmentStoreError,
    NanoErrorKind::CheckpointUnavailable,
    NanoErrorKind::CheckpointNotFound,
    NanoErrorKind::CheckpointRestoreFailed,
];

/// The static, provider-free presentation for one kind: title plus the
/// actionable hint when the table carries one.
pub fn error_presentation(kind: NanoErrorKind) -> String {
    let spec = spec(kind);
    if spec.hint.is_empty() {
        spec.title.to_string()
    } else {
        format!("{} — {}", spec.title, spec.hint)
    }
}

/// The table as a serializable document (artifact shape shared by the JSON
/// snapshot and the TS module).
fn table_document() -> serde_json::Value {
    let errors: Vec<serde_json::Value> = ALL_KINDS
        .iter()
        .map(|kind| {
            let spec = spec(*kind);
            serde_json::json!({
                "kind": kind,
                "surface": spec.surface.as_str(),
                "wireCode": spec.wire_code,
                "retryable": spec.retryable,
                "title": spec.title,
                "hint": spec.hint,
            })
        })
        .collect();
    serde_json::json!({
        "version": 1,
        "source": "crates/nano-session/src/error_codes.rs",
        "generator": "cargo run -p nano-cli --bin gen_error_table",
        "errors": errors,
    })
}

/// Canonical JSON rendering — THE pinned pretty-printer (`to_string_pretty`
/// plus a trailing newline). The generator and the parity test both call
/// this, so the drift tripwire fires only on semantic drift.
pub fn render_json() -> String {
    let mut out = serde_json::to_string_pretty(&table_document()).expect("table serializes");
    out.push('\n');
    out
}

/// Canonical TypeScript rendering — a DATA-ONLY module (no generated
/// classification logic that could diverge), deterministic from the table.
/// Emitted in the Desktop repo's oxfmt style (single quotes, 2-space,
/// trailing commas) so `oxfmt --check` passes without a rewrite pass.
pub fn render_ts() -> String {
    fn ts_str(s: &str) -> String {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
    let kinds: Vec<String> = ALL_KINDS
        .iter()
        .map(|kind| {
            let kind_str = serde_json::to_value(kind).expect("kind serializes");
            format!(
                "  | {}",
                ts_str(kind_str.as_str().expect("kind is a string"))
            )
        })
        .collect();
    let entries: Vec<String> = ALL_KINDS
        .iter()
        .map(|kind| {
            let spec = spec(*kind);
            let kind_str = serde_json::to_value(kind).expect("kind serializes");
            let kind_str = kind_str.as_str().expect("kind is a string");
            format!(
                "  {{\n    kind: {},\n    surface: {},\n    wireCode: {},\n    retryable: {},\n    title: {},\n    hint: {},\n  }},",
                ts_str(kind_str),
                ts_str(spec.surface.as_str()),
                spec.wire_code,
                spec.retryable,
                ts_str(spec.title),
                ts_str(spec.hint),
            )
        })
        .collect();
    format!(
        "// GENERATED by gen_error_table (cargo run -p nano-cli --bin gen_error_table) — do not edit.\n\
         // Source of truth: crates/nano-session/src/error_codes.rs (wayland-nano repo).\n\
         \n\
         export type NanoErrorKind =\n{};\n\
         \n\
         export type NanoErrorSurface = 'error_response' | 'tool_card' | 'stop_reason';\n\
         \n\
         export interface NanoErrorSpec {{\n\
         \x20 kind: NanoErrorKind;\n\
         \x20 surface: NanoErrorSurface;\n\
         \x20 wireCode: number;\n\
         \x20 retryable: boolean;\n\
         \x20 title: string;\n\
         \x20 hint: string;\n\
         }}\n\
         \n\
         export const NANO_ERROR_SPECS: readonly NanoErrorSpec[] = [\n{}\n];\n\
         \n\
         export const NANO_ERROR_BY_KIND: Readonly<Record<string, NanoErrorSpec>> = Object.fromEntries(\n\
         \x20 NANO_ERROR_SPECS.map((s) => [s.kind, s])\n\
         );\n",
        kinds.join("\n"),
        entries.join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned count: adding a kind without updating ALL_KINDS fails here;
    /// adding one without a spec fails to compile (exhaustive match above).
    /// P2a §7 added the seven vision-intake kinds (41 → 48); P3 §7 added the
    /// seven MCP-ecosystem kinds (48 → 55); the RC2 wiring pass added P4's
    /// PtySessionGone (55 → 56); the P4 review-mode merge added
    /// ReviewParseFailed (56 → 57); the P4 rules wiring (F-P4-1) added
    /// ShellRuleDenied + RuleFileInvalid (57 → 59); the S4 hooks merge
    /// added HookBlocked (59 → 60); the S7 checkpoints merge added
    /// CheckpointUnavailable + CheckpointNotFound + CheckpointRestoreFailed
    /// (60 → 63).
    #[test]
    fn all_kinds_count_is_pinned() {
        assert_eq!(ALL_KINDS.len(), 63);
    }

    /// P3 §12 [r2 codex-F16]: symbolic wire names are the compatibility
    /// contract — no two kinds may share a snake_case wire name.
    #[test]
    fn no_duplicate_wire_names() {
        let mut seen = std::collections::HashSet::new();
        for kind in ALL_KINDS {
            let wire = serde_json::to_value(kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind is a string")
                .to_string();
            assert!(seen.insert(wire.clone()), "duplicate wire name: {wire}");
        }
        // The forward-tolerance escape hatch is part of the enum but not
        // the advertised table; it must still have a unique wire name.
        let unknown = serde_json::to_value(NanoErrorKind::Unknown)
            .expect("kind serializes")
            .as_str()
            .expect("kind is a string")
            .to_string();
        assert!(
            seen.insert(unknown.clone()),
            "duplicate wire name: {unknown}"
        );
    }

    /// P3 §12: wire-form pins for the seven new snake_case names.
    #[test]
    fn p3_wire_names_are_pinned() {
        let wire = |kind: NanoErrorKind| {
            serde_json::to_value(kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind is a string")
                .to_string()
        };
        assert_eq!(
            wire(NanoErrorKind::McpResourceUnsupported),
            "mcp_resource_unsupported"
        );
        assert_eq!(
            wire(NanoErrorKind::McpResourceDenied),
            "mcp_resource_denied"
        );
        assert_eq!(
            wire(NanoErrorKind::McpContentUnsupported),
            "mcp_content_unsupported"
        );
        assert_eq!(
            wire(NanoErrorKind::McpElicitationUnsupported),
            "mcp_elicitation_unsupported"
        );
        assert_eq!(
            wire(NanoErrorKind::McpAuthorizationRequired),
            "mcp_authorization_required"
        );
        assert_eq!(
            wire(NanoErrorKind::McpCredstoreUnavailable),
            "mcp_credstore_unavailable"
        );
        assert_eq!(wire(NanoErrorKind::McpOAuthFailed), "mcp_oauth_failed");
        // Every new kind is a non-retryable tool card (§7 table).
        for kind in [
            NanoErrorKind::McpResourceUnsupported,
            NanoErrorKind::McpResourceDenied,
            NanoErrorKind::McpContentUnsupported,
            NanoErrorKind::McpElicitationUnsupported,
            NanoErrorKind::McpAuthorizationRequired,
            NanoErrorKind::McpCredstoreUnavailable,
            NanoErrorKind::McpOAuthFailed,
        ] {
            let spec = spec(kind);
            assert!(!spec.retryable, "{kind:?} must not be retryable");
            assert_eq!(spec.surface, ErrorSurface::ToolCard, "{kind:?} surface");
        }
    }

    /// P4 §8 (RC2 wiring pass): the PtySessionGone wire form and spec are
    /// pinned — a non-retryable tool card named `pty_session_gone`.
    #[test]
    fn p4_pty_session_gone_is_pinned() {
        let wire = serde_json::to_value(NanoErrorKind::PtySessionGone)
            .expect("kind serializes")
            .as_str()
            .expect("kind is a string")
            .to_string();
        assert_eq!(wire, "pty_session_gone");
        assert_eq!(
            serde_json::from_str::<NanoErrorKind>("\"pty_session_gone\"").unwrap(),
            NanoErrorKind::PtySessionGone
        );
        let spec = spec(NanoErrorKind::PtySessionGone);
        assert!(!spec.retryable);
        assert_eq!(spec.surface, ErrorSurface::ToolCard);
    }

    /// P4 §8 (F-P4-1 rules wiring): the two rule-DSL kinds are pinned —
    /// non-retryable tool cards named `shell_rule_denied` /
    /// `rule_file_invalid`.
    #[test]
    fn p4_rule_kinds_are_pinned() {
        for (kind, wire_name) in [
            (NanoErrorKind::ShellRuleDenied, "shell_rule_denied"),
            (NanoErrorKind::RuleFileInvalid, "rule_file_invalid"),
        ] {
            let wire = serde_json::to_value(kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind is a string")
                .to_string();
            assert_eq!(wire, wire_name);
            assert_eq!(
                serde_json::from_str::<NanoErrorKind>(&format!("\"{wire_name}\"")).unwrap(),
                kind
            );
            let spec = spec(kind);
            assert!(!spec.retryable, "{wire_name} must not be retryable");
            assert_eq!(spec.surface, ErrorSurface::ToolCard, "{wire_name} surface");
        }
    }

    /// The drift alarm: the committed JSON artifact must be byte-identical
    /// to the canonical rendering. Regenerate with
    /// `cargo run -p nano-cli --bin gen_error_table`.
    #[test]
    fn json_artifact_matches_the_table() {
        // Line endings are checkout-platform dependent (CRLF on Windows
        // autocrlf); the drift alarm pins CONTENT, so compare normalized.
        assert_eq!(
            render_json().replace("\r\n", "\n"),
            include_str!("../contracts/nano-error-codes.json").replace("\r\n", "\n"),
            "contracts/nano-error-codes.json is stale — run cargo run -p nano-cli --bin gen_error_table"
        );
    }

    #[test]
    fn every_spec_is_internally_consistent() {
        for kind in ALL_KINDS {
            let spec = spec(*kind);
            match spec.surface {
                ErrorSurface::ErrorResponse => assert!(
                    spec.wire_code == -32602 || spec.wire_code == -32603,
                    "{kind:?}: error responses use standard codes only"
                ),
                ErrorSurface::ToolCard => assert!(
                    !spec.retryable
                        || matches!(
                            kind,
                            NanoErrorKind::McpTransport | NanoErrorKind::McpTimeout
                        ),
                    "{kind:?}: only mcp transport/timeout tool errors are retryable"
                ),
                ErrorSurface::StopReason => {
                    assert_eq!(*kind, NanoErrorKind::UserCancelled);
                }
            }
            assert!(!spec.title.is_empty(), "{kind:?}: title is mandatory");
        }
    }

    #[test]
    fn mcp_transient_kinds_are_retryable_cards() {
        assert!(spec(NanoErrorKind::McpTransport).retryable);
        assert!(spec(NanoErrorKind::McpTimeout).retryable);
        assert_eq!(
            spec(NanoErrorKind::McpTransport).surface,
            ErrorSurface::ToolCard
        );
    }

    #[test]
    fn unknown_kind_is_terminal() {
        let spec = spec(NanoErrorKind::Unknown);
        assert!(!spec.retryable);
        assert_eq!(spec.surface, ErrorSurface::ErrorResponse);
    }

    #[test]
    fn ts_rendering_is_deterministic_and_data_only() {
        let ts = render_ts();
        assert_eq!(ts, render_ts());
        assert!(ts.starts_with("// GENERATED by gen_error_table"));
        assert!(ts.contains("'model_rate_limited'"));
        assert!(ts.contains("export const NANO_ERROR_SPECS"));
        // No classification logic: only data declarations.
        assert!(!ts.contains("function"));
        assert!(!ts.contains("=> {"));
    }
}
