//! The 401 recovery seam (C9, Q5 RULED): `AuthRefresh` lives here in
//! nano-model, on the credential provider, so C8's OAuth work plugs in
//! WITHOUT nano-agent depending on OAuth machinery. nano-agent holds only
//! the per-turn one-shot retry state machine.
//!
//! Fail-closed contract:
//! - at most one model-call retry per turn, only after a `Refreshed`
//!   outcome, only for HTTP 401;
//! - Flux API keys are static: the v1 provider returns `NotRefreshable`,
//!   so a 401 takes ZERO retries and surfaces the typed Auth error;
//! - a second 401 after a successful refresh is terminal, and so is any
//!   refresh failure;
//! - no key material ever crosses this seam: outcomes carry no credentials,
//!   and `Failed` carries a sanitized reason only.

/// The outcome of one credential refresh attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Fresh credentials are in effect; the failed request may be retried
    /// exactly once.
    Refreshed,
    /// The credential cannot be refreshed (e.g. a static API key): the 401
    /// is terminal, zero retries.
    NotRefreshable,
    /// The refresh itself failed (sanitized reason — never key material).
    /// Terminal.
    Failed(String),
}

/// Credential providers that can recover from an HTTP 401 implement this.
/// C8 implements it for OAuth; the static Flux key never does.
#[async_trait::async_trait]
pub trait AuthRefresh: std::fmt::Debug + Send + Sync {
    async fn refresh(&self) -> RefreshOutcome;
}

/// The v1 Flux credential: a static environment API key with no refresh
/// path (`acp_mode.rs`: "wayland-nano uses FLUX_API_KEY from the
/// environment; no interactive auth"). 401 → zero retries.
#[derive(Debug, Default)]
pub struct StaticApiKey;

#[async_trait::async_trait]
impl AuthRefresh for StaticApiKey {
    async fn refresh(&self) -> RefreshOutcome {
        RefreshOutcome::NotRefreshable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_key_is_never_refreshable() {
        assert_eq!(StaticApiKey.refresh().await, RefreshOutcome::NotRefreshable);
    }
}
