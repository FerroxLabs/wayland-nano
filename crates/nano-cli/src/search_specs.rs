//! web_search (P1) backend resolution (design §2.3): the `[search] backend`
//! choice (`NANO_SEARCH_BACKEND` until a config file lands — auto default),
//! the FIXED ladder (D1: Flux grounding → Brave → Tavily → typed
//! unavailability, NO keyless floor), key-gated single-host egress policies
//! (§2.4 — a search host enters the allowlist ONLY when its key resolves),
//! and the Flux rung's meter precondition (r2 claude-F2 — unmetered search
//! never runs).
//!
//! Resolution is deterministic: fixed ladder order, fixed env-var lookup
//! order, and the resolved ladder is announced ONCE in logs (backend ids
//! and env-var NAMES, never values). Mid-session re-resolution is out of
//! scope (design §10) — hosts resolve once at start.

use std::sync::Arc;

use nano_core::search::SearchBackendChoice;
use nano_egress::client::EgressClient;
use nano_egress::policy::EgressPolicy;
use nano_model::flux_completions::OpenAiCompletionsClient;
use nano_model::metering::UsageSink;
use nano_tools::web_search::{
    BraveSearchBackend, ChainedSearchBackend, FluxSearchBackend, SearchBackend,
    TavilySearchBackend, UnavailableSearchBackend, WebSearchTool,
};

/// What an unbacked forced invocation names (env-var NAMES, never values).
const LOOKED_FOR: &str = "no search backend resolved (looked for: flux via FLUX_API_KEY + the session meter handle, brave via BRAVE_SEARCH_API_KEY, tavily via TAVILY_API_KEY)";

/// The resolved search surface: the tool (chain or single backend) and the
/// ladder id for diagnostics/logs. `None` from resolution means UNREGISTERED
/// — the don't-register half of the D3 double guard.
pub struct ResolvedSearch {
    pub tool: WebSearchTool,
    pub backend_id: String,
}

/// Resolve the web_search tool from the environment. `meter` is the session
/// `CostMeter`/`UsageSink` handle (P1 §2.5): the Flux rung exists only when
/// BOTH a Flux credential and the handle resolve.
pub fn web_search_tool_from_env(meter: Option<Arc<dyn UsageSink>>) -> Option<ResolvedSearch> {
    let choice = match std::env::var("NANO_SEARCH_BACKEND") {
        Ok(raw) => match SearchBackendChoice::parse(&raw) {
            Some(choice) => choice,
            None => {
                // Fail-closed config posture: an unknown value is never a
                // silent clamp to auto.
                eprintln!(
                    "wayland-nano: NANO_SEARCH_BACKEND {raw:?} is not auto|flux|brave|tavily|off — web_search disabled"
                );
                return None;
            }
        },
        Err(_) => SearchBackendChoice::Auto,
    };
    resolve_with(
        choice,
        meter,
        crate::flux_key::flux_api_key(),
        env_key("BRAVE_SEARCH_API_KEY"),
        env_key("TAVILY_API_KEY"),
    )
}

fn env_key(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The resolution core, key-material explicit so tests inject without env
/// mutation (deterministic resolution: fixed ladder, fixed lookup order).
fn resolve_with(
    choice: SearchBackendChoice,
    meter: Option<Arc<dyn UsageSink>>,
    flux_key: Option<String>,
    brave_key: Option<String>,
    tavily_key: Option<String>,
) -> Option<ResolvedSearch> {
    match choice {
        SearchBackendChoice::Off => {
            eprintln!("wayland-nano: web_search disabled ([search] backend = off)");
            None
        }
        SearchBackendChoice::Auto => {
            let mut tiers: Vec<Arc<dyn SearchBackend>> = Vec::new();
            let mut ids: Vec<&str> = Vec::new();
            if let Some(backend) = flux_rung(flux_key, meter) {
                ids.push("flux");
                tiers.push(backend);
            }
            if let Some(backend) = brave_rung(brave_key) {
                ids.push("brave");
                tiers.push(backend);
            }
            if let Some(backend) = tavily_rung(tavily_key) {
                ids.push("tavily");
                tiers.push(backend);
            }
            if tiers.is_empty() {
                // Nothing resolved: UNREGISTERED (the don't-register rule).
                return None;
            }
            // Belt-and-braces tail (D3): a forced invocation that outlives
            // the ladder hits typed Unavailability, never a panic.
            tiers.push(Arc::new(UnavailableSearchBackend::new(LOOKED_FOR)));
            eprintln!(
                "wayland-nano: web_search backend ladder: {}",
                ids.join(" → ")
            );
            Some(ResolvedSearch {
                tool: WebSearchTool::new(Arc::new(ChainedSearchBackend::new(tiers))),
                backend_id: ids.join("+"),
            })
        }
        SearchBackendChoice::Flux => match flux_rung(flux_key, meter) {
            Some(backend) => {
                eprintln!("wayland-nano: web_search backend: flux");
                Some(ResolvedSearch {
                    tool: WebSearchTool::new(backend),
                    backend_id: "flux".into(),
                })
            }
            // Explicit selection fails typed and LOUD — never a silent
            // fall into another tier (design §2.3).
            None => {
                eprintln!(
                    "wayland-nano: [search] backend = flux but no Flux credential and session meter handle resolved — web_search disabled"
                );
                None
            }
        },
        SearchBackendChoice::Brave => match brave_rung(brave_key) {
            Some(backend) => {
                eprintln!("wayland-nano: web_search backend: brave");
                Some(ResolvedSearch {
                    tool: WebSearchTool::new(backend),
                    backend_id: "brave".into(),
                })
            }
            None => {
                eprintln!(
                    "wayland-nano: [search] backend = brave but BRAVE_SEARCH_API_KEY is unset — web_search disabled"
                );
                None
            }
        },
        SearchBackendChoice::Tavily => match tavily_rung(tavily_key) {
            Some(backend) => {
                eprintln!("wayland-nano: web_search backend: tavily");
                Some(ResolvedSearch {
                    tool: WebSearchTool::new(backend),
                    backend_id: "tavily".into(),
                })
            }
            None => {
                eprintln!(
                    "wayland-nano: [search] backend = tavily but TAVILY_API_KEY is unset — web_search disabled"
                );
                None
            }
        },
    }
}

/// The Flux rung: credential AND meter handle (r2 claude-F2), zero new
/// egress surface (the Flux base is already allowlisted). No handle ⇒ the
/// backend refuses typed at construction — this rung simply isn't built.
fn flux_rung(
    flux_key: Option<String>,
    meter: Option<Arc<dyn UsageSink>>,
) -> Option<Arc<dyn SearchBackend>> {
    let key = flux_key?;
    let backend = FluxSearchBackend::new(
        OpenAiCompletionsClient::new(EgressClient::flux()),
        key,
        meter,
    )
    .ok()?;
    Some(Arc::new(backend))
}

/// Brave: the single-host policy is constructed exactly here, exactly when
/// the key resolves (key-gated allowlist, §2.4).
fn brave_rung(key: Option<String>) -> Option<Arc<dyn SearchBackend>> {
    let key = key?;
    let client = EgressClient::new(single_host_policy(BraveSearchBackend::HOST));
    Some(Arc::new(BraveSearchBackend::new(client, key)))
}

fn tavily_rung(key: Option<String>) -> Option<Arc<dyn SearchBackend>> {
    let key = key?;
    let client = EgressClient::new(single_host_policy(TavilySearchBackend::HOST));
    Some(Arc::new(TavilySearchBackend::new(client, key)))
}

/// One host, https only — the whole egress widening a search backend gets.
fn single_host_policy(host: &str) -> EgressPolicy {
    EgressPolicy::new().allow_host(host)
}

#[cfg(test)]
mod tests {
    //! Resolution battery: pure-key injection (NO env mutation — the flux
    //! key channel's own tests mutate the same vars and must never race).
    use super::*;

    fn meter() -> Option<Arc<dyn UsageSink>> {
        Some(Arc::new(nano_model::metering::StubCostMeter::new()))
    }

    #[test]
    fn auto_with_nothing_resolved_is_unregistered() {
        assert!(resolve_with(SearchBackendChoice::Auto, meter(), None, None, None).is_none());
    }

    #[test]
    fn off_is_loud_and_unregistered_even_with_keys() {
        assert!(
            resolve_with(
                SearchBackendChoice::Off,
                meter(),
                Some("sk".into()),
                Some("sk".into()),
                Some("sk".into()),
            )
            .is_none()
        );
    }

    #[test]
    fn flux_rung_needs_the_meter_handle() {
        // Key but NO handle: the rung is skipped in auto…
        let resolved = resolve_with(
            SearchBackendChoice::Auto,
            None,
            Some("sk".into()),
            Some("sk".into()),
            None,
        )
        .expect("brave still resolves");
        assert_eq!(resolved.backend_id, "brave");
        // …and explicit flux is a loud typed nothing.
        assert!(
            resolve_with(
                SearchBackendChoice::Flux,
                None,
                Some("sk".into()),
                None,
                None
            )
            .is_none()
        );
        // Handle present: the rung leads the ladder.
        let resolved = resolve_with(
            SearchBackendChoice::Auto,
            meter(),
            Some("sk".into()),
            Some("sk".into()),
            None,
        )
        .expect("flux + brave");
        assert_eq!(resolved.backend_id, "flux+brave");
        assert_eq!(resolved.tool.backend_id(), "chained");
    }

    #[test]
    fn explicit_backends_select_exactly_one() {
        let resolved = resolve_with(
            SearchBackendChoice::Flux,
            meter(),
            Some("sk".into()),
            None,
            None,
        )
        .expect("explicit flux");
        assert_eq!(resolved.backend_id, "flux");
        assert_eq!(resolved.tool.backend_id(), "flux");
        // Explicit selection without its credential: typed loud nothing,
        // never a silent fall into another tier.
        assert!(
            resolve_with(
                SearchBackendChoice::Brave,
                meter(),
                Some("sk".into()),
                None,
                Some("sk".into())
            )
            .is_none()
        );
        assert!(resolve_with(SearchBackendChoice::Tavily, meter(), None, None, None).is_none());
    }

    #[test]
    fn ladder_order_is_fixed() {
        let resolved = resolve_with(
            SearchBackendChoice::Auto,
            meter(),
            Some("sk".into()),
            Some("sk".into()),
            Some("sk".into()),
        )
        .expect("full ladder");
        assert_eq!(resolved.backend_id, "flux+brave+tavily");
        let resolved = resolve_with(
            SearchBackendChoice::Auto,
            meter(),
            None,
            None,
            Some("sk".into()),
        )
        .expect("tavily only");
        assert_eq!(resolved.backend_id, "tavily");
    }

    /// Key-gated allowlist (§2.4): the Brave/Tavily policy domains carry
    /// exactly one host each — and never the Flux base or anything else.
    #[test]
    fn single_host_policies_are_exact() {
        let brave = single_host_policy(BraveSearchBackend::HOST);
        assert!(brave.allows_host("https://api.search.brave.com/res/v1/web/search?q=x"));
        assert!(!brave.allows_host("https://api.tavily.com/search"));
        assert!(!brave.allows_host("https://api.fluxrouter.ai/v1/chat/completions"));
        assert!(!brave.allows_host("https://example.com/"));
        // https-only without the per-host opt-in.
        assert!(!brave.allows_host("http://api.search.brave.com/res/v1/web/search?q=x"));
        let tavily = single_host_policy(TavilySearchBackend::HOST);
        assert!(tavily.allows_host("https://api.tavily.com/search"));
        assert!(!tavily.allows_host("https://api.search.brave.com/"));
    }
}
