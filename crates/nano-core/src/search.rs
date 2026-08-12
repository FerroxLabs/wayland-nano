//! web_search backend selection vocabulary (P1 design §2.3). Config-layer
//! vocabulary ONLY: the host resolution logic (env channels, key gating,
//! the fallback ladder) lives in nano-cli; the backends live in nano-tools.
//!
//! The ladder is fixed (D1): Flux grounding → Brave → Tavily → typed
//! unavailability. No keyless floor, ever. The MCP tier is deferred to P3
//! (Q1 ruling) and deliberately has no variant here.

/// `[search] backend = auto|flux|brave|tavily|off` (P1 design §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackendChoice {
    /// Walk the fixed ladder: Flux (credential + meter handle) → Brave (key)
    /// → Tavily (key) → unregistered.
    Auto,
    /// Exactly the Flux grounding backend — failure is typed, never a
    /// silent fall into the next tier.
    Flux,
    /// Exactly Brave (`BRAVE_SEARCH_API_KEY`).
    Brave,
    /// Exactly Tavily (`TAVILY_API_KEY`).
    Tavily,
    /// Loud disabled state: the tool is unregistered and a forced
    /// invocation returns a typed unavailability error.
    Off,
}

impl SearchBackendChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            SearchBackendChoice::Auto => "auto",
            SearchBackendChoice::Flux => "flux",
            SearchBackendChoice::Brave => "brave",
            SearchBackendChoice::Tavily => "tavily",
            SearchBackendChoice::Off => "off",
        }
    }

    /// Strict parser for config channels: an unknown value is None (the
    /// call site raises a typed config error), never a silent clamp.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(SearchBackendChoice::Auto),
            "flux" => Some(SearchBackendChoice::Flux),
            "brave" => Some(SearchBackendChoice::Brave),
            "tavily" => Some(SearchBackendChoice::Tavily),
            "off" => Some(SearchBackendChoice::Off),
            _ => None,
        }
    }
}

impl Default for SearchBackendChoice {
    /// Unset = auto: the ladder walks from whatever resolves.
    fn default() -> Self {
        SearchBackendChoice::Auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips_and_rejects_unknown() {
        for choice in [
            SearchBackendChoice::Auto,
            SearchBackendChoice::Flux,
            SearchBackendChoice::Brave,
            SearchBackendChoice::Tavily,
            SearchBackendChoice::Off,
        ] {
            assert_eq!(SearchBackendChoice::parse(choice.as_str()), Some(choice));
        }
        assert_eq!(
            SearchBackendChoice::parse(" FLUX "),
            Some(SearchBackendChoice::Flux)
        );
        assert_eq!(SearchBackendChoice::parse("duckduckgo"), None);
        assert_eq!(SearchBackendChoice::parse(""), None);
        assert_eq!(SearchBackendChoice::parse("mcp"), None);
        assert_eq!(SearchBackendChoice::default(), SearchBackendChoice::Auto);
    }
}
