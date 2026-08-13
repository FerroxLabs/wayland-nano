//! The C7 error-code table surface. The table itself lives in
//! `nano-session::error_codes` (the serde-only journal-vocabulary crate) so
//! nano-tui can render table presentations without linking the engine
//! closure; this module re-exports the full surface so every design-cited
//! path (`nano_protocol::error_codes::*`) resolves unchanged.

pub use nano_session::error_codes::{
    ALL_KINDS, ErrorSpec, ErrorSurface, error_presentation, render_json, render_ts, spec,
};

#[cfg(test)]
mod tests {
    use super::*;
    use nano_session::NanoErrorKind;

    /// C7 §6/§8 — retry agreement, iterated FROM THE SOURCE ENUM SIDE: an
    /// explicit match constructs one instance per `ModelError` variant (a
    /// newly added variant breaks THIS test's compilation), and every
    /// nested `EgressError` variant is covered too. The table's retryable
    /// flag must equal the engine's own classification verdict throughout.
    /// (Post-C9 oracle: `classify`, not `is_retryable` — the reconnect class
    /// IS retried, by the bounded reconnect loop rather than the fast
    /// RetryPolicy, so `is_retryable` alone would under-report it.)
    #[test]
    fn table_retryable_agrees_with_engine_retry_policy() {
        use nano_agent::error_map::kind_of_model;
        use nano_egress::client::EgressError;
        use nano_model::retry::classify;
        use nano_model::types::ModelError;

        let mut cases: Vec<ModelError> = Vec::new();
        // One explicit arm per variant — compile-time exhaustiveness over
        // the source enum. (The anchor values themselves are never used;
        // the arms push fresh instances.)
        let push = |anchor: &ModelError, cases: &mut Vec<ModelError>| match anchor {
            ModelError::Auth { .. } => cases.push(ModelError::Auth {
                message: "bad key".into(),
                status: Some(401),
            }),
            ModelError::RateLimited { .. } => cases.push(ModelError::RateLimited {
                retry_after_ms: Some(500),
            }),
            ModelError::ContextOverflow(_) => {
                cases.push(ModelError::ContextOverflow("too long".into()))
            }
            ModelError::Entitlement(_) => cases.push(ModelError::Entitlement("plan".into())),
            ModelError::Server { .. } => {
                cases.push(ModelError::Server {
                    status: 503,
                    message: "oops".into(),
                });
                cases.push(ModelError::Server {
                    status: 400,
                    message: "bad".into(),
                });
            }
            ModelError::Transport { .. } => cases.push(ModelError::Transport {
                phase: nano_model::types::TransportPhase::Connect,
                message: "reset".into(),
            }),
            ModelError::Protocol(_) => cases.push(ModelError::Protocol("bad json".into())),
            ModelError::OutputSchema(_) => {
                cases.push(ModelError::OutputSchema("field missing".into()))
            }
            ModelError::UnsupportedParam { .. } => cases.push(ModelError::UnsupportedParam {
                param: "verbosity".into(),
                surface: "anthropic".into(),
                message: "known-unsupported here".into(),
            }),
            ModelError::Cancelled => cases.push(ModelError::Cancelled),
            ModelError::Egress(_) => {
                for err in [
                    EgressError::Denied {
                        method: "POST".into(),
                        host: "example.com".into(),
                        digest: "x".into(),
                    },
                    EgressError::Transport("reset".into()),
                    EgressError::HttpStatus {
                        status: 503,
                        host: "example.com".into(),
                        digest: "x".into(),
                    },
                    EgressError::HttpStatus {
                        status: 400,
                        host: "example.com".into(),
                        digest: "x".into(),
                    },
                    EgressError::PrivateAddress {
                        host: "example.com".into(),
                    },
                    EgressError::CredentialsRejected {
                        host: "example.com".into(),
                    },
                    EgressError::ContentTypeMissing {
                        host: "example.com".into(),
                    },
                    EgressError::ContentTypeDenied {
                        host: "example.com".into(),
                        media_type: "text/html".into(),
                    },
                    EgressError::InvalidRedirect {
                        host: "example.com".into(),
                    },
                    EgressError::RedirectLimit {
                        host: "example.com".into(),
                    },
                ] {
                    cases.push(ModelError::Egress(err));
                }
            }
        };
        for anchor in [
            ModelError::Auth {
                message: String::new(),
                status: None,
            },
            ModelError::RateLimited {
                retry_after_ms: None,
            },
            ModelError::ContextOverflow(String::new()),
            ModelError::Entitlement(String::new()),
            ModelError::Server {
                status: 500,
                message: String::new(),
            },
            ModelError::Transport {
                phase: nano_model::types::TransportPhase::MidStream,
                message: String::new(),
            },
            ModelError::Protocol(String::new()),
            ModelError::OutputSchema(String::new()),
            ModelError::UnsupportedParam {
                param: String::new(),
                surface: String::new(),
                message: String::new(),
            },
            ModelError::Cancelled,
            ModelError::Egress(EgressError::Transport(String::new())),
        ] {
            push(&anchor, &mut cases);
        }
        assert_eq!(cases.len(), 21);
        for err in &cases {
            if matches!(err, ModelError::Cancelled) {
                continue; // never an error kind — stopReason:"cancelled"
            }
            let kind = kind_of_model(err);
            assert_eq!(
                spec(kind).retryable,
                classify(err).is_some(),
                "{err:?} → {kind:?}: table and retry.rs disagree"
            );
        }
    }

    /// The shim must not drift from the canonical module.
    #[test]
    fn shim_re_exports_the_canonical_table() {
        assert_eq!(ALL_KINDS.len(), 50);
        assert_eq!(
            spec(NanoErrorKind::ModelRateLimited).title,
            nano_session::error_codes::spec(NanoErrorKind::ModelRateLimited).title
        );
    }
}
