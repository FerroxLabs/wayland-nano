//! Session token budget configuration (P1 §4.1).
//!
//! Nano has no config-file machinery (env-var convention per
//! `NANO_WEB_FETCH_HOSTS` / `NANO_MCP_SERVERS` / `NANO_MEMORY_WRITE`), so
//! the design's `[budget] session_tokens = N` lands as the namespaced env
//! var [`SESSION_TOKENS_ENV`]. Unset = no cap (back-compat; C11 goal
//! budgets are untouched and orthogonal).

/// The session token cap env var (namespaced per AGENTS.md).
pub const SESSION_TOKENS_ENV: &str = "NANO_BUDGET_SESSION_TOKENS";

/// Resolve the session token cap. Fail-closed: a malformed value is a typed
/// error NAMING the var, never a silently-ignored cap (an operator who
/// asked for a cap and typo'd must not get an uncapped session).
pub fn session_token_cap_from_env() -> Result<Option<u64>, String> {
    match std::env::var(SESSION_TOKENS_ENV) {
        Err(_) => Ok(None),
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            match trimmed.parse::<u64>() {
                Ok(value) if value > 0 => Ok(Some(value)),
                _ => Err(format!(
                    "{SESSION_TOKENS_ENV} must be a positive integer, got {trimmed:?}"
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env-mutating tests must not run in parallel with each other; keep
    // them in one test body so the harness serializes them (the
    // flux_key.rs convention — a shared env var across parallel tests
    // races).
    #[test]
    fn cap_resolution_unset_valid_and_malformed() {
        // Unset = no cap (back-compat).
        unsafe { std::env::remove_var(SESSION_TOKENS_ENV) };
        assert_eq!(session_token_cap_from_env(), Ok(None));

        // A valid positive integer is the cap.
        unsafe { std::env::set_var(SESSION_TOKENS_ENV, "100") };
        let parsed = session_token_cap_from_env();
        unsafe { std::env::remove_var(SESSION_TOKENS_ENV) };
        assert_eq!(parsed, Ok(Some(100)));

        // Fail-closed: a malformed cap is a typed error naming the var,
        // never a silently-uncapped session.
        for bad in ["abc", "-5", "0", "1.5", "  "] {
            unsafe { std::env::set_var(SESSION_TOKENS_ENV, bad) };
            let parsed = session_token_cap_from_env();
            unsafe { std::env::remove_var(SESSION_TOKENS_ENV) };
            if bad == "  " {
                assert_eq!(parsed, Ok(None), "blank = unset");
                continue;
            }
            let err = parsed.expect_err("malformed must be typed");
            assert!(err.contains(SESSION_TOKENS_ENV), "{err}");
        }
    }
}
