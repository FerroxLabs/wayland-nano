//! C9 §4: sticky model params from the env config channel, shared by the
//! acp host and the protocol host. Out-of-vocabulary values are typed
//! config errors naming the setting — never a silent clamp.

use nano_model::types::{ReasoningEffort, Verbosity};

/// NANO_REASONING_EFFORT: low|medium|high, sticky per session.
pub fn effort_from_env() -> Result<Option<ReasoningEffort>, String> {
    parse("NANO_REASONING_EFFORT", ReasoningEffort::parse)
}

/// NANO_VERBOSITY: low|medium|high, sticky per session.
pub fn verbosity_from_env() -> Result<Option<Verbosity>, String> {
    parse("NANO_VERBOSITY", Verbosity::parse)
}

fn parse<T>(name: &str, parse: impl Fn(&str) -> Option<T>) -> Result<Option<T>, String> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => parse(raw.trim()).map(Some).ok_or_else(|| {
            format!("{name} must be low|medium|high, got {raw:?}; clear the setting to unset")
        }),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    // Env-mutating tests stay OUT of the parallel default run; the parsers
    // are exercised through the ACP wire tests (ServeConfig injection) and
    // here only via the pure function shapes.
    #[test]
    fn parsers_are_strict() {
        assert!(nano_model::types::ReasoningEffort::parse("low").is_some());
        assert!(nano_model::types::ReasoningEffort::parse("turbo").is_none());
        assert!(nano_model::types::Verbosity::parse("high").is_some());
        assert!(nano_model::types::Verbosity::parse("chatty").is_none());
    }
}
