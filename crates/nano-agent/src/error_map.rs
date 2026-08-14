//! Variant → [`NanoErrorKind`] mappers (C7 design §2.1 step 2). Every mapper
//! is an exhaustive wildcard-free match: a new source-enum variant that
//! lacks a mapping does not compile. No stringly-typed re-derivation happens
//! anywhere downstream of these functions.

use nano_model::types::ModelError;
use nano_session::NanoErrorKind;

use crate::turn::TypedError;

/// Model-family mapping, including the nested egress forms.
pub fn kind_of_model(err: &ModelError) -> NanoErrorKind {
    match err {
        ModelError::Auth { .. } => NanoErrorKind::ModelAuth,
        ModelError::RateLimited { .. } => NanoErrorKind::ModelRateLimited,
        ModelError::ContextOverflow(_) => NanoErrorKind::ModelContextOverflow,
        ModelError::Entitlement(_) => NanoErrorKind::ModelEntitlement,
        ModelError::Server { status, .. } => kind_of_status(*status),
        // F-18: a provider 404 is the typed model_not_found kind on the
        // wire, not the model_server_4xx bucket.
        ModelError::ModelNotFound { .. } => NanoErrorKind::ModelNotFound,
        // F-P5-1: a format rejection is request-side (4xx semantics) even
        // when the edge mislabeled it 5xx; terminal, never retryable.
        ModelError::InvalidRequest { .. } => NanoErrorKind::ModelServer4xx,
        ModelError::Transport { .. } => NanoErrorKind::ModelTransport,
        ModelError::Protocol(_) => NanoErrorKind::ModelProtocol,
        // C9: structured-output rejection after the one re-ask, and the
        // pre-network unsupported-parameter rejection — both terminal.
        ModelError::OutputSchema(_) => NanoErrorKind::ModelOutputSchema,
        ModelError::UnsupportedParam { .. } => NanoErrorKind::ModelUnsupportedParam,
        // ModelError::Cancelled is deliberately never an error kind on the
        // wire — it is stopReason:"cancelled" (design §3).
        ModelError::Cancelled => NanoErrorKind::UserCancelled,
        ModelError::Egress(err) => kind_of_egress(err),
    }
}

/// Builds the turn-fatal [`TypedError`] for a model failure: the kind plus
/// the CLOSED typed extras the wire may carry (design §2/D2). The free-text
/// detail stays logs/model-side only (§7 — provider prose never reaches a
/// UI-bound frame).
pub fn typed_error_of_model(err: &ModelError) -> TypedError {
    let mut typed = TypedError::new(kind_of_model(err), format!("model call failed: {err}"));
    match err {
        ModelError::RateLimited { retry_after_ms } => {
            typed.retry_after_ms = *retry_after_ms;
        }
        ModelError::Server { status, .. } => {
            typed.status = Some(*status);
        }
        // F-18: the 404 status rides as the closed typed extra.
        ModelError::ModelNotFound { status, .. } => {
            typed.status = Some(*status);
        }
        ModelError::InvalidRequest { status, .. } => {
            typed.status = Some(*status);
        }
        ModelError::Egress(nano_egress::client::EgressError::Denied { host, .. }) => {
            // host_display strips userinfo at construction (nano-egress).
            typed.host = Some(host.clone());
        }
        ModelError::Egress(nano_egress::client::EgressError::HttpStatus { status, .. }) => {
            typed.status = Some(*status);
        }
        _ => {}
    }
    typed
}

/// Egress failures surface as policy denials, HTTP status classes, or
/// transport errors — the variants are redacted by construction
/// (nano-egress carries method/host/digest only).
pub fn kind_of_egress(err: &nano_egress::client::EgressError) -> NanoErrorKind {
    use nano_egress::client::EgressError;
    match err {
        EgressError::Denied { .. }
        | EgressError::PrivateAddress { .. }
        | EgressError::CredentialsRejected { .. }
        | EgressError::ContentTypeMissing { .. }
        | EgressError::ContentTypeDenied { .. }
        | EgressError::InvalidRedirect { .. }
        | EgressError::RedirectLimit { .. } => NanoErrorKind::EgressDenied,
        EgressError::HttpStatus { status, .. } => kind_of_status(*status),
        EgressError::Transport(_) => NanoErrorKind::ModelTransport,
    }
}

fn kind_of_status(status: u16) -> NanoErrorKind {
    if status >= 500 {
        NanoErrorKind::ModelServer5xx
    } else {
        NanoErrorKind::ModelServer4xx
    }
}

pub fn kind_of_tool(err: &nano_tools::fs::ToolError) -> NanoErrorKind {
    use nano_tools::fs::ToolError;
    match err {
        ToolError::ReadDenied(_) => NanoErrorKind::FsReadDenied,
        ToolError::WriteDenied(_) => NanoErrorKind::FsWriteDenied,
        ToolError::SensitiveDenied(_) => NanoErrorKind::FsSensitiveDenied,
        ToolError::Io(_) => NanoErrorKind::FsIo,
        ToolError::Edit(_) => NanoErrorKind::FsEdit,
        // A bad read window is a model-correctable argument error.
        ToolError::InvalidWindow(_) => NanoErrorKind::MissingArgs,
        // P4 (§5.5): malformed non-window arguments (e.g. repo_map's
        // path_glob) are typed InvalidParams, model-correctable.
        ToolError::InvalidParams(_) => NanoErrorKind::InvalidParams,
    }
}

pub fn kind_of_shell(err: &nano_tools::shell::ShellError) -> NanoErrorKind {
    use nano_tools::shell::ShellError;
    match err {
        ShellError::Spawn(_) => NanoErrorKind::ShellSpawn,
        ShellError::SandboxUnavailable(_) => NanoErrorKind::SandboxUnavailable,
    }
}

pub fn kind_of_mcp(err: &nano_mcp::client::McpError) -> NanoErrorKind {
    use nano_mcp::client::McpError;
    match err {
        McpError::Server { .. } => NanoErrorKind::McpServer,
        McpError::Transport(_) => NanoErrorKind::McpTransport,
        McpError::Protocol(_) => NanoErrorKind::McpProtocol,
        McpError::Timeout(_) => NanoErrorKind::McpTimeout,
        McpError::OutputBounded(_) => NanoErrorKind::McpOutputBounded,
        // P3 §2.4: cancel is terminal and surfaces as stopReason, never an
        // error card.
        McpError::Cancelled => NanoErrorKind::UserCancelled,
        // P3 §4.2/§4.3: typed refusals before any wire write / at the
        // content boundary — the new kinds, not the generic server bucket.
        McpError::ResourceUnsupported => NanoErrorKind::McpResourceUnsupported,
        McpError::ContentUnsupported => NanoErrorKind::McpContentUnsupported,
        // S5 Leg B: unix containment failure is fail-closed, never generic.
        McpError::SandboxUnavailable(_) => NanoErrorKind::SandboxUnavailable,
        McpError::Egress(err) => kind_of_egress(err),
    }
}

/// web_fetch failures (C4): argument validation is a model-correctable
/// error; egress failures map through the egress table.
pub fn kind_of_web_fetch(err: &nano_tools::web::WebFetchError) -> NanoErrorKind {
    use nano_tools::web::WebFetchError;
    match err {
        WebFetchError::Args(_) => NanoErrorKind::MissingArgs,
        WebFetchError::Egress(err) => kind_of_egress(err),
    }
}

/// web_search failures (P1): argument validation is model-correctable;
/// unavailability (and the unmetered-construction refusal) is the
/// fail-closed policy posture — the same kind the unconfigured web_fetch
/// arm uses; backend failures route through the model/egress tables;
/// cancellation is the one kind that is never an error on the wire.
pub fn kind_of_web_search(err: &nano_tools::web_search::WebSearchError) -> NanoErrorKind {
    use nano_tools::web_search::{BackendErrorKind, WebSearchError};
    match err {
        WebSearchError::Args(_) => NanoErrorKind::MissingArgs,
        WebSearchError::Unavailable(_) | WebSearchError::Unmetered(_) => {
            NanoErrorKind::EgressDenied
        }
        WebSearchError::Cancelled => NanoErrorKind::UserCancelled,
        WebSearchError::Backend { kind, .. } => match kind {
            BackendErrorKind::Model(err) => kind_of_model(err),
            BackendErrorKind::Egress(err) => kind_of_egress(err),
            BackendErrorKind::Parse(_) => NanoErrorKind::ModelProtocol,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_extras_are_closed_and_bounded() {
        let err = ModelError::RateLimited {
            retry_after_ms: Some(750),
        };
        let typed = typed_error_of_model(&err);
        assert_eq!(typed.kind, NanoErrorKind::ModelRateLimited);
        assert_eq!(typed.retry_after_ms, Some(750));
        assert!(typed.status.is_none());
        // The detail is logs-side; it MAY carry provider text, the wire
        // never does (the wire builders take only the closed fields).
        assert!(typed.detail.contains("rate limited"));

        let err = ModelError::Server {
            status: 503,
            message: "provider prose that must never reach a UI".into(),
        };
        let typed = typed_error_of_model(&err);
        assert_eq!(typed.kind, NanoErrorKind::ModelServer5xx);
        assert_eq!(typed.status, Some(503));

        // F-18: a provider 404 surfaces as the typed model_not_found kind
        // (the wire shape Desktop's fallback keys on), with the 404 status
        // as the closed extra — never the model_server_4xx bucket.
        let err = ModelError::ModelNotFound {
            status: 404,
            message: "retired upstream".into(),
        };
        let typed = typed_error_of_model(&err);
        assert_eq!(typed.kind, NanoErrorKind::ModelNotFound);
        assert_eq!(typed.status, Some(404));
    }

    /// Variant pins for the tool-side families (compile-time exhaustive
    /// matches above; these pin the actual assignments).
    #[test]
    fn tool_shell_mcp_variant_pins() {
        use nano_mcp::client::McpError;
        use nano_tools::fs::ToolError;
        use nano_tools::shell::ShellError;

        assert_eq!(
            kind_of_tool(&ToolError::ReadDenied("x".into())),
            NanoErrorKind::FsReadDenied
        );
        assert_eq!(
            kind_of_tool(&ToolError::WriteDenied("x".into())),
            NanoErrorKind::FsWriteDenied
        );
        assert_eq!(
            kind_of_tool(&ToolError::SensitiveDenied("x".into())),
            NanoErrorKind::FsSensitiveDenied
        );
        assert_eq!(
            kind_of_tool(&ToolError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "gone"
            ))),
            NanoErrorKind::FsIo
        );
        assert_eq!(
            kind_of_tool(&ToolError::Edit("x".into())),
            NanoErrorKind::FsEdit
        );
        assert_eq!(
            kind_of_tool(&ToolError::InvalidWindow("x".into())),
            NanoErrorKind::MissingArgs
        );
        assert_eq!(
            kind_of_tool(&ToolError::InvalidParams("x".into())),
            NanoErrorKind::InvalidParams
        );

        assert_eq!(
            kind_of_shell(&ShellError::Spawn("x".into())),
            NanoErrorKind::ShellSpawn
        );
        assert_eq!(
            kind_of_shell(&ShellError::SandboxUnavailable("x".into())),
            NanoErrorKind::SandboxUnavailable
        );

        assert_eq!(
            kind_of_mcp(&McpError::Server {
                code: -1,
                message: "x".into()
            }),
            NanoErrorKind::McpServer
        );
        assert_eq!(
            kind_of_mcp(&McpError::Transport("x".into())),
            NanoErrorKind::McpTransport
        );
        assert_eq!(
            kind_of_mcp(&McpError::Protocol("x".into())),
            NanoErrorKind::McpProtocol
        );
        assert_eq!(
            kind_of_mcp(&McpError::Timeout(30)),
            NanoErrorKind::McpTimeout
        );
        assert_eq!(
            kind_of_mcp(&McpError::OutputBounded(9)),
            NanoErrorKind::McpOutputBounded
        );
        // P3 pins: the three dispatcher variants added with the full-duplex
        // connection land on their exact kinds.
        assert_eq!(
            kind_of_mcp(&McpError::Cancelled),
            NanoErrorKind::UserCancelled
        );
        assert_eq!(
            kind_of_mcp(&McpError::ResourceUnsupported),
            NanoErrorKind::McpResourceUnsupported
        );
        assert_eq!(
            kind_of_mcp(&McpError::ContentUnsupported),
            NanoErrorKind::McpContentUnsupported
        );
    }

    /// P1 web_search pins: the C7 table is REUSED — no new kinds (args →
    /// missing_args, unavailable/unmetered → egress_denied like the
    /// unconfigured web_fetch arm, backend kinds route through the
    /// model/egress tables, cancelled → user_cancelled).
    #[test]
    fn web_search_variant_pins() {
        use nano_model::types::ModelError;
        use nano_tools::web_search::{BackendErrorKind, WebSearchError};

        assert_eq!(
            kind_of_web_search(&WebSearchError::Args("x".into())),
            NanoErrorKind::MissingArgs
        );
        assert_eq!(
            kind_of_web_search(&WebSearchError::Unavailable("x".into())),
            NanoErrorKind::EgressDenied
        );
        assert_eq!(
            kind_of_web_search(&WebSearchError::Unmetered("x".into())),
            NanoErrorKind::EgressDenied
        );
        assert_eq!(
            kind_of_web_search(&WebSearchError::Cancelled),
            NanoErrorKind::UserCancelled
        );
        assert_eq!(
            kind_of_web_search(&WebSearchError::Backend {
                backend: "flux".into(),
                kind: BackendErrorKind::Model(ModelError::Transport {
                    phase: nano_model::types::TransportPhase::Connect,
                    message: "x".into(),
                }),
            }),
            NanoErrorKind::ModelTransport
        );
        assert_eq!(
            kind_of_web_search(&WebSearchError::Backend {
                backend: "brave".into(),
                kind: BackendErrorKind::Egress(nano_egress::client::EgressError::HttpStatus {
                    status: 500,
                    host: "h".into(),
                    digest: "d".into(),
                }),
            }),
            NanoErrorKind::ModelServer5xx
        );
        assert_eq!(
            kind_of_web_search(&WebSearchError::Backend {
                backend: "flux".into(),
                kind: BackendErrorKind::Parse("x".into()),
            }),
            NanoErrorKind::ModelProtocol
        );
    }
}
