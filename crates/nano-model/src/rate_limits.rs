//! Rate-limit snapshot parsing (C9, Q4 RULED): a header-family-generic
//! parser shipped fixture-driven with the OpenAI-family header names. A
//! sanitized live Flux capture upgrades the Flux family later with zero API
//! change; until then Flux surfacing is explicitly unsupported-observation.
//!
//! Fail-closed rules:
//! - fields are optional and individually parsed: a partially valid header
//!   set yields a snapshot with the valid fields populated and the rest
//!   `None`, never all-or-nothing;
//! - fully absent or garbage headers yield `None` — never a fabricated
//!   snapshot; UIs render "unknown" on absence;
//! - the snapshot is observability only. No control-flow decision consumes
//!   it (Retry-After in retry.rs remains the sole timing authority).

use serde::Deserialize;
use serde::Serialize;

/// OpenAI-family header names (the shipped fixture-driven family).
pub const HEADER_LIMIT_REQUESTS: &str = "x-ratelimit-limit-requests";
pub const HEADER_LIMIT_TOKENS: &str = "x-ratelimit-limit-tokens";
pub const HEADER_REMAINING_REQUESTS: &str = "x-ratelimit-remaining-requests";
pub const HEADER_REMAINING_TOKENS: &str = "x-ratelimit-remaining-tokens";
pub const HEADER_RESET_REQUESTS: &str = "x-ratelimit-reset-requests";
pub const HEADER_RESET_TOKENS: &str = "x-ratelimit-reset-tokens";

/// One rate-limit observation. Every field is optional except the capture
/// time and scope: a snapshot preserves WHAT was seen, WHEN, and for which
/// resource family — partial data included, nothing interpolated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitSnapshot {
    /// Capture time (ms since Unix epoch) at the adapter boundary.
    pub captured_at_ms: u64,
    /// Resource scope / family identity (e.g. "openai-default").
    pub scope: Option<String>,
    pub requests_limit: Option<u64>,
    pub requests_remaining: Option<u64>,
    pub requests_reset_ms: Option<u64>,
    pub tokens_limit: Option<u64>,
    pub tokens_remaining: Option<u64>,
    pub tokens_reset_ms: Option<u64>,
}

impl RateLimitSnapshot {
    fn empty(captured_at_ms: u64, scope: Option<String>) -> Self {
        Self {
            captured_at_ms,
            scope,
            requests_limit: None,
            requests_remaining: None,
            requests_reset_ms: None,
            tokens_limit: None,
            tokens_remaining: None,
            tokens_reset_ms: None,
        }
    }

    /// A snapshot with no parsed fields is NO snapshot (never fabricated).
    fn has_any_field(&self) -> bool {
        self.requests_limit.is_some()
            || self.requests_remaining.is_some()
            || self.requests_reset_ms.is_some()
            || self.tokens_limit.is_some()
            || self.tokens_remaining.is_some()
            || self.tokens_reset_ms.is_some()
    }
}

/// Parse one header value as an integer; garbage parses to `None` (that
/// field only — the rest of the set still parses).
fn parse_u64(raw: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    raw.and_then(|v| v.to_str().ok())
        .map(str::trim)
        .and_then(|s| s.parse::<u64>().ok())
}

/// Parse a reset header: plain integer milliseconds, or an OpenAI-style
/// duration string ("250ms", "2s", "1m"). Garbage → `None`.
fn parse_reset_ms(raw: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    let s = raw.and_then(|v| v.to_str().ok()).map(str::trim)?;
    if let Ok(ms) = s.parse::<u64>() {
        return Some(ms);
    }
    if let Some(num) = s.strip_suffix("ms") {
        return num.trim().parse::<u64>().ok();
    }
    if let Some(num) = s.strip_suffix('s') {
        return num.trim().parse::<u64>().ok()?.checked_mul(1_000);
    }
    if let Some(num) = s.strip_suffix('m') {
        return num.trim().parse::<u64>().ok()?.checked_mul(60_000);
    }
    None
}

/// Capture time in ms since the Unix epoch (injected into parsers so
/// fixtures stay deterministic).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Parse the OpenAI-family rate-limit headers from a response. Absent or
/// fully-garbage sets yield `None`; a partial set yields a partial snapshot.
pub fn parse_headers(
    headers: &reqwest::header::HeaderMap,
    captured_at_ms: u64,
) -> Option<RateLimitSnapshot> {
    let get = |name: &str| headers.get(name);
    let mut snapshot = RateLimitSnapshot::empty(captured_at_ms, Some("openai-default".into()));
    snapshot.requests_limit = parse_u64(get(HEADER_LIMIT_REQUESTS));
    snapshot.requests_remaining = parse_u64(get(HEADER_REMAINING_REQUESTS));
    snapshot.requests_reset_ms = parse_reset_ms(get(HEADER_RESET_REQUESTS));
    snapshot.tokens_limit = parse_u64(get(HEADER_LIMIT_TOKENS));
    snapshot.tokens_remaining = parse_u64(get(HEADER_REMAINING_TOKENS));
    snapshot.tokens_reset_ms = parse_reset_ms(get(HEADER_RESET_TOKENS));
    snapshot.has_any_field().then_some(snapshot)
}

/// Parse a rate-limit stream event payload (Responses-surface mid-stream
/// observations). Accepts either an event carrying a `rate_limits` object or
/// a bare snapshot object; the same partial-field rules apply.
pub fn parse_stream_event(
    event: &serde_json::Value,
    captured_at_ms: u64,
) -> Option<RateLimitSnapshot> {
    let payload = event.get("rate_limits").unwrap_or(event);
    let get_u64 = |key: &str| payload.get(key).and_then(|v| v.as_u64());
    let mut snapshot = RateLimitSnapshot::empty(
        captured_at_ms,
        payload
            .get("scope")
            .and_then(|s| s.as_str())
            .map(str::to_string),
    );
    snapshot.requests_limit = get_u64("requests_limit");
    snapshot.requests_remaining = get_u64("requests_remaining");
    snapshot.requests_reset_ms = get_u64("requests_reset_ms");
    snapshot.tokens_limit = get_u64("tokens_limit");
    snapshot.tokens_remaining = get_u64("tokens_remaining");
    snapshot.tokens_reset_ms = get_u64("tokens_reset_ms");
    snapshot.has_any_field().then_some(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn full_openai_family_set_parses_all_fields() {
        let map = headers(&[
            ("x-ratelimit-limit-requests", "1000"),
            ("x-ratelimit-remaining-requests", "999"),
            ("x-ratelimit-reset-requests", "1s"),
            ("x-ratelimit-limit-tokens", "50000"),
            ("x-ratelimit-remaining-tokens", "49000"),
            ("x-ratelimit-reset-tokens", "250ms"),
        ]);
        let snapshot = parse_headers(&map, 1_700_000_000_000).expect("full set parses");
        assert_eq!(snapshot.captured_at_ms, 1_700_000_000_000);
        assert_eq!(snapshot.scope.as_deref(), Some("openai-default"));
        assert_eq!(snapshot.requests_limit, Some(1000));
        assert_eq!(snapshot.requests_remaining, Some(999));
        assert_eq!(snapshot.requests_reset_ms, Some(1_000));
        assert_eq!(snapshot.tokens_limit, Some(50_000));
        assert_eq!(snapshot.tokens_remaining, Some(49_000));
        assert_eq!(snapshot.tokens_reset_ms, Some(250));
    }

    #[test]
    fn partial_set_yields_partial_snapshot_never_all_or_nothing() {
        let map = headers(&[
            ("x-ratelimit-limit-requests", "1000"),
            ("x-ratelimit-remaining-requests", "garbage"),
            ("x-ratelimit-reset-tokens", "2m"),
        ]);
        let snapshot = parse_headers(&map, 42).expect("partial set parses");
        assert_eq!(snapshot.requests_limit, Some(1000));
        assert_eq!(snapshot.requests_remaining, None);
        assert_eq!(snapshot.tokens_reset_ms, Some(120_000));
        assert_eq!(snapshot.tokens_limit, None);
    }

    #[test]
    fn absent_or_garbage_headers_yield_none() {
        assert_eq!(parse_headers(&headers(&[]), 0), None);
        let garbage = headers(&[
            ("x-ratelimit-limit-requests", "lots"),
            ("x-ratelimit-remaining-tokens", "-3"),
        ]);
        assert_eq!(parse_headers(&garbage, 0), None);
    }

    #[test]
    fn stream_event_payload_parses_with_scope() {
        let event = serde_json::json!({
            "type": "response.rate_limits",
            "rate_limits": {
                "scope": "account",
                "tokens_limit": 100000,
                "tokens_remaining": 99500
            }
        });
        let snapshot = parse_stream_event(&event, 7).expect("event parses");
        assert_eq!(snapshot.scope.as_deref(), Some("account"));
        assert_eq!(snapshot.tokens_limit, Some(100_000));
        assert_eq!(snapshot.tokens_remaining, Some(99_500));
        assert_eq!(snapshot.requests_limit, None);
        assert_eq!(snapshot.captured_at_ms, 7);
        assert_eq!(
            parse_stream_event(&serde_json::json!({"type": "other"}), 0),
            None
        );
    }
}
