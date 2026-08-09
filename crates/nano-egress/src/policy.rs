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
#[derive(Debug, Clone, Default)]
pub struct EgressPolicy {
    allowed_hosts: BTreeSet<String>,
}

impl EgressPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        self.allowed_hosts.insert(host.into());
        self
    }

    /// The v1 default: Flux only. Deliberately constructed, never ambient.
    pub fn flux_only() -> Self {
        Self::new().allow_host("api.fluxrouter.ai")
    }

    pub fn decide(&self, url: &str) -> EgressDecision {
        let Some(host) = host_of(url) else {
            return EgressDecision::Deny;
        };
        if self.allowed_hosts.contains(&host) {
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
    }

    #[test]
    fn host_parsing_handles_ports_and_userinfo() {
        let policy = EgressPolicy::flux_only();
        assert!(policy.allows("https://api.fluxrouter.ai:8443/v1/models"));
        assert!(!policy.allows("https://evil.com@api.fluxrouter.ai.evil.com/x"));
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
