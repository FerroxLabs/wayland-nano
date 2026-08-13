//! Egress policy: deny-by-default host allowlist, extended with
//! path+method-scoped endpoint grants (P3 design §6.3).
//!
//! Provenance: invariants from Wayland Core `wcore-egress` (single chokepoint,
//! fail-closed) — original Nano implementation. Core's allow-all default is
//! deliberately NOT replicated: absent allowlist entries mean denial.

use std::collections::BTreeSet;

use crate::grant::EndpointGrant;
use crate::grant::HttpMethod;
use crate::grant::InvalidEndpoint;
use crate::grant::normalize_endpoint;

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
///
/// Endpoint grants (P3 §6.3): an `EndpointGrant` authorizes EXACTLY ONE
/// (scheme, host, effective port, method, canonical path) tuple — the OAuth
/// trust-chain cap ("token/discovery endpoints only"). A hostname alone never
/// authorizes via grants; grants are added builder-style at construction and
/// there is NO runtime mutation API (the closed-sets posture is unchanged).
#[derive(Debug, Clone, Default)]
pub struct EgressPolicy {
    allowed_hosts: BTreeSet<String>,
    http_allowed_hosts: BTreeSet<String>,
    endpoint_grants: BTreeSet<EndpointGrant>,
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

    /// Grant ONE exact endpoint: `method` on the canonical
    /// scheme/host/effective-port/path tuple of `url` (P3 §6.3). Query,
    /// fragment, and userinfo are typed rejections (callers map
    /// [`InvalidEndpoint`] onto their `InvalidParams`-class error); http URLs
    /// are rejected — grants are https-only.
    pub fn allow_endpoint(
        mut self,
        method: HttpMethod,
        url: &str,
    ) -> Result<Self, InvalidEndpoint> {
        let grant = normalize_endpoint(method, url)?;
        self.endpoint_grants.insert(grant);
        Ok(self)
    }

    /// Whether this policy carries any endpoint grants. `EgressClient` uses
    /// this to route grant-bearing requests through the method-aware
    /// per-request redirect gate (`client.rs`).
    pub fn has_endpoint_grants(&self) -> bool {
        !self.endpoint_grants.is_empty()
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

    /// THE decision point (P3 §6.3, r3 codex new-7): the method is MANDATORY
    /// — there is deliberately no method-less overload. Allows iff the host
    /// rule matches (method-agnostic, the pre-P3 semantic) OR an
    /// `EndpointGrant` matches scheme+host+effective-port+method+canonical
    /// path. A request URL carrying query/fragment/userinfo can match only
    /// the host rule, never a grant (the request side runs through the SAME
    /// `normalize_endpoint` as grant construction, which rejects them).
    pub fn decide(&self, url: &str, method: &reqwest::Method) -> EgressDecision {
        if self.allows_host(url) {
            return EgressDecision::Allow;
        }
        if self.endpoint_grants.is_empty() {
            return EgressDecision::Deny;
        }
        let Ok(method) = method.as_str().parse::<HttpMethod>() else {
            // Extension methods can never match a grant.
            return EgressDecision::Deny;
        };
        match normalize_endpoint(method, url) {
            Ok(request) if self.endpoint_grants.contains(&request) => EgressDecision::Allow,
            _ => EgressDecision::Deny,
        }
    }

    /// Host-only introspection (P3 §6.3: the former `allows()` is narrowed —
    /// it CANNOT authorize endpoint grants). Evaluates ONLY the host sets.
    pub fn allows_host(&self, url: &str) -> bool {
        let Some((scheme, _)) = url.split_once("://") else {
            return false;
        };
        let Some(host) = host_of(url) else {
            return false;
        };
        match scheme.to_ascii_lowercase().as_str() {
            "https" => self.allowed_hosts.contains(&host),
            "http" => self.http_allowed_hosts.contains(&host),
            _ => false,
        }
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
        assert!(!policy.allows_host("https://api.fluxrouter.ai/v1/models"));
    }

    #[test]
    fn flux_only_allows_flux_and_denies_rest() {
        let policy = EgressPolicy::flux_only();
        assert!(policy.allows_host("https://api.fluxrouter.ai/v1/chat/completions"));
        assert!(policy.allows_host("https://api.fluxrouter.ai/anthropic/v1/messages"));
        assert!(!policy.allows_host("https://api.openai.com/v1/responses"));
        assert!(!policy.allows_host("http://169.254.169.254/latest/meta-data"));
        // https-only: even the allowlisted Flux host is denied over plain
        // http without the per-host opt-in.
        assert!(!policy.allows_host("http://api.fluxrouter.ai/v1/chat/completions"));
        // non-http(s) schemes fail closed even for allowlisted hosts
        assert!(!policy.allows_host("ftp://api.fluxrouter.ai/v1/models"));
    }

    #[test]
    fn http_requires_the_per_host_opt_in() {
        let policy = EgressPolicy::new().allow_host_with_http("localhost");
        assert!(policy.allows_host("http://localhost:3000/mcp"));
        assert!(policy.allows_host("https://localhost:3000/mcp"));
        // the opt-in is per-host: other hosts stay https-only
        assert!(!policy.allows_host("http://example.com/mcp"));
        let https_only = EgressPolicy::new().allow_host("example.com");
        assert!(https_only.allows_host("https://example.com/"));
        assert!(!https_only.allows_host("http://example.com/"));
    }

    #[test]
    fn host_parsing_handles_ports_and_userinfo() {
        let policy = EgressPolicy::flux_only();
        assert!(policy.allows_host("https://api.fluxrouter.ai:8443/v1/models"));
        assert!(!policy.allows_host("https://evil.com@api.fluxrouter.ai.evil.com/x"));
    }

    #[test]
    fn allow_url_adds_https_hosts_only() {
        let policy = EgressPolicy::new()
            .allow_url("https://api.openai.com/v1")
            .allow_url("https://openrouter.ai/api/v1")
            .allow_url("http://insecure.example.com/v1") // http base: ignored
            .allow_url("not-a-url"); // garbage: ignored
        assert!(policy.allows_host("https://api.openai.com/v1/chat/completions"));
        assert!(policy.allows_host("https://openrouter.ai/api/v1/chat/completions"));
        assert!(!policy.allows_host("http://insecure.example.com/v1/chat/completions"));
        assert!(!policy.allows_host("https://insecure.example.com/v1"));
        assert!(!policy.allows_host("https://api.anthropic.com/v1/messages"));
    }

    #[test]
    fn digest_is_stable_and_short() {
        assert_eq!(path_query_sha256("https://h/v1/models").len(), 16);
        assert_eq!(
            path_query_sha256("https://h/v1/models"),
            path_query_sha256("https://h/v1/models")
        );
    }

    // --- P3 §6.3: endpoint grants at the decision point ---------------------

    #[test]
    fn endpoint_grant_authorizes_exactly_one_tuple() {
        let policy = EgressPolicy::new()
            .allow_endpoint(HttpMethod::Post, "https://as.example/token")
            .expect("grant");
        // The granted tuple allows.
        assert_eq!(
            policy.decide("https://as.example/token", &reqwest::Method::POST),
            EgressDecision::Allow
        );
        // Method mismatch: GET on the token path is a DIFFERENT resource.
        assert_eq!(
            policy.decide("https://as.example/token", &reqwest::Method::GET),
            EgressDecision::Deny
        );
        // Trailing slash is a DIFFERENT resource (no folding).
        assert_eq!(
            policy.decide("https://as.example/token/", &reqwest::Method::POST),
            EgressDecision::Deny
        );
        // Any other path on the same host: denied (the cap is mechanical).
        assert_eq!(
            policy.decide("https://as.example/other", &reqwest::Method::POST),
            EgressDecision::Deny
        );
        // Effective port is key material.
        assert_eq!(
            policy.decide("https://as.example:8443/token", &reqwest::Method::POST),
            EgressDecision::Deny
        );
        // Host case-insensitive; request carrying a query matches NO grant.
        assert_eq!(
            policy.decide("https://AS.example/token", &reqwest::Method::POST),
            EgressDecision::Allow
        );
        assert_eq!(
            policy.decide("https://as.example/token?x=1", &reqwest::Method::POST),
            EgressDecision::Deny
        );
    }

    #[test]
    fn request_side_normalization_matches_grant_side() {
        let policy = EgressPolicy::new()
            .allow_endpoint(
                HttpMethod::Get,
                "https://as.example/.well-known/oauth-authorization-server",
            )
            .expect("grant");
        // Dot segments and unreserved escapes on the REQUEST side resolve to
        // the granted tuple (same normalize_endpoint on both sides).
        assert_eq!(
            policy.decide(
                "https://as.example/x/../.well-known/%6Fauth-authorization-server",
                &reqwest::Method::GET
            ),
            EgressDecision::Allow
        );
        // A reserved escape never becomes a path separator.
        assert_eq!(
            policy.decide("https://as.example/%2Fwell-known", &reqwest::Method::GET),
            EgressDecision::Deny
        );
    }

    #[test]
    fn host_rule_still_authorizes_any_method_and_path() {
        // The pre-P3 semantic is unchanged: an allowlisted host authorizes
        // any path and any method; grants are additive, never narrowing.
        let policy = EgressPolicy::new()
            .allow_host("api.fluxrouter.ai")
            .allow_endpoint(HttpMethod::Post, "https://as.example/token")
            .expect("grant");
        assert_eq!(
            policy.decide(
                "https://api.fluxrouter.ai/v1/chat?x=1",
                &reqwest::Method::POST
            ),
            EgressDecision::Allow
        );
        assert_eq!(
            policy.decide(
                "https://api.fluxrouter.ai/any/path",
                &reqwest::Method::DELETE
            ),
            EgressDecision::Allow
        );
    }

    #[test]
    fn allows_host_cannot_authorize_endpoint_grants() {
        // The narrowed introspection evaluates ONLY the host sets.
        let policy = EgressPolicy::new()
            .allow_endpoint(HttpMethod::Post, "https://as.example/token")
            .expect("grant");
        assert!(!policy.allows_host("https://as.example/token"));
        assert!(policy.has_endpoint_grants());
        assert!(!EgressPolicy::flux_only().has_endpoint_grants());
    }
}
