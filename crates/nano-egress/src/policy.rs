//! Egress policy: deny-by-default host allowlist.
//!
//! Provenance: invariants from Wayland Core `wcore-egress` (single chokepoint,
//! fail-closed) — original Nano implementation. Core's allow-all default is
//! deliberately NOT replicated: absent allowlist entries mean denial.

use std::collections::BTreeSet;

/// Outcome of an egress policy decision for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressDecision {
    Allow,
    Deny,
}

/// Fail-closed host policy. Empty = deny everything.
///
/// Scheme hardening (C4 §3.5): `https` only by default; `http` requires a
/// per-host opt-in (`allow_host_with_http`) so local endpoints such as
/// `http://localhost:PORT` MCP servers stay possible without opening http
/// for the world. Every other scheme (ftp, file, …) is denied.
#[derive(Debug, Clone, Default)]
pub struct EgressPolicy {
    allowed_hosts: BTreeSet<String>,
    http_allowed_hosts: BTreeSet<String>,
}

impl EgressPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow `host` over https only.
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        self.allowed_hosts.insert(host.into());
        self
    }

    /// Allow `host` over https AND plain http (the per-host http opt-in).
    pub fn allow_host_with_http(mut self, host: impl Into<String>) -> Self {
        let host = host.into();
        self.allowed_hosts.insert(host.clone());
        self.http_allowed_hosts.insert(host);
        self
    }

    /// The v1 default: Flux only. Deliberately constructed, never ambient.
    pub fn flux_only() -> Self {
        Self::new().allow_host("api.fluxrouter.ai")
    }

    /// Allow the host of an absolute https URL (C8: the multi-provider
    /// policy is built from the vendored catalog's `base_url` fields, the
    /// sole endpoint authority). A non-https or hostless URL adds NOTHING —
    /// fail-closed by construction.
    pub fn allow_url(self, url: &str) -> Self {
        match url.split_once("://") {
            Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => match host_of(url) {
                Some(host) if !host.is_empty() => self.allow_host(host),
                _ => self,
            },
            _ => self,
        }
    }

    pub fn decide(&self, url: &str) -> EgressDecision {
        let Some((scheme, _)) = url.split_once("://") else {
            return EgressDecision::Deny;
        };
        let Some(host) = host_of(url) else {
            return EgressDecision::Deny;
        };
        let allowed = match scheme.to_ascii_lowercase().as_str() {
            "https" => self.allowed_hosts.contains(&host),
            "http" => self.http_allowed_hosts.contains(&host),
            _ => false,
        };
        if allowed {
            EgressDecision::Allow
        } else {
            EgressDecision::Deny
        }
    }

    pub fn allows(&self, url: &str) -> bool {
        matches!(self.decide(url), EgressDecision::Allow)
    }
}

fn host_of(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme.split('/').next()?;
    // Strip userinfo (rejected anyway by callers) and port.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    if let Some(rest) = host_port.strip_prefix('[') {
        let (host, _) = rest.split_once(']')?;
        return Some(host.to_string());
    }
    Some(
        host_port
            .rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(host_port, |(host, _)| host)
            .to_ascii_lowercase(),
    )
}

/// sha256 of the path+query for observability without logging payloads.
pub fn path_query_sha256(url: &str) -> String {
    use std::fmt::Write;
    let path = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let path = path.split('/').skip(1).collect::<Vec<_>>().join("/");
    let digest = Sha256::digest(path.as_bytes());
    let mut short = String::new();
    for ch in digest.chars().take(16) {
        let _ = write!(short, "{ch}");
    }
    short
}

// Minimal sha256 wrapper to keep the digest logic in one place.
struct Sha256;
impl Sha256 {
    fn digest(data: &[u8]) -> String {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(data);
        format!("{:x}", h.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_by_default() {
        let policy = EgressPolicy::new();
        assert!(!policy.allows("https://api.fluxrouter.ai/v1/models"));
    }

    #[test]
    fn flux_only_allows_flux_and_denies_rest() {
        let policy = EgressPolicy::flux_only();
        assert!(policy.allows("https://api.fluxrouter.ai/v1/chat/completions"));
        assert!(policy.allows("https://api.fluxrouter.ai/anthropic/v1/messages"));
        assert!(!policy.allows("https://api.openai.com/v1/responses"));
        assert!(!policy.allows("http://169.254.169.254/latest/meta-data"));
        // https-only: even the allowlisted Flux host is denied over plain
        // http without the per-host opt-in.
        assert!(!policy.allows("http://api.fluxrouter.ai/v1/chat/completions"));
        // non-http(s) schemes fail closed even for allowlisted hosts
        assert!(!policy.allows("ftp://api.fluxrouter.ai/v1/models"));
    }

    #[test]
    fn http_requires_the_per_host_opt_in() {
        let policy = EgressPolicy::new().allow_host_with_http("localhost");
        assert!(policy.allows("http://localhost:3000/mcp"));
        assert!(policy.allows("https://localhost:3000/mcp"));
        // the opt-in is per-host: other hosts stay https-only
        assert!(!policy.allows("http://example.com/mcp"));
        let https_only = EgressPolicy::new().allow_host("example.com");
        assert!(https_only.allows("https://example.com/"));
        assert!(!https_only.allows("http://example.com/"));
    }

    #[test]
    fn host_parsing_handles_ports_and_userinfo() {
        let policy = EgressPolicy::flux_only();
        assert!(policy.allows("https://api.fluxrouter.ai:8443/v1/models"));
        assert!(!policy.allows("https://evil.com@api.fluxrouter.ai.evil.com/x"));
    }

    #[test]
    fn allow_url_adds_https_hosts_only() {
        let policy = EgressPolicy::new()
            .allow_url("https://api.openai.com/v1")
            .allow_url("https://openrouter.ai/api/v1")
            .allow_url("http://insecure.example.com/v1") // http base: ignored
            .allow_url("not-a-url"); // garbage: ignored
        assert!(policy.allows("https://api.openai.com/v1/chat/completions"));
        assert!(policy.allows("https://openrouter.ai/api/v1/chat/completions"));
        assert!(!policy.allows("http://insecure.example.com/v1/chat/completions"));
        assert!(!policy.allows("https://insecure.example.com/v1"));
        assert!(!policy.allows("https://api.anthropic.com/v1/messages"));
    }

    #[test]
    fn digest_is_stable_and_short() {
        assert_eq!(path_query_sha256("https://h/v1/models").len(), 16);
        assert_eq!(
            path_query_sha256("https://h/v1/models"),
            path_query_sha256("https://h/v1/models")
        );
    }
}
