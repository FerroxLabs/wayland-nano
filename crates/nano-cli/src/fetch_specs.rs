//! web_fetch (C4) host configuration: `NANO_WEB_FETCH_HOSTS`, a
//! comma-separated list of exact hosts (https only; a `http://host` entry
//! marks the per-host http opt-in for local endpoints). Empty or unset = no
//! fetch hosts = the tool denies everything — the fetch egress policy is a
//! SECOND domain, separate from the Flux API allowlist, deny-by-default.

use nano_egress::client::EgressClient;
use nano_egress::policy::EgressPolicy;
use nano_tools::web::WebFetchTool;

/// Build the web_fetch tool from `NANO_WEB_FETCH_HOSTS`. Returns None when
/// no hosts are configured (the executor then answers web_fetch with a
/// typed "not configured" denial).
pub fn web_fetch_tool_from_env() -> Option<WebFetchTool> {
    let raw = std::env::var("NANO_WEB_FETCH_HOSTS").ok()?;
    policy_from_hosts_str(&raw).map(|policy| WebFetchTool::new(EgressClient::new(policy)))
}

/// Parse the hosts list into the second-domain policy. `http://host` marks
/// the per-host http opt-in; bare hosts are https-only.
fn policy_from_hosts_str(raw: &str) -> Option<EgressPolicy> {
    let mut policy = EgressPolicy::new();
    let mut any = false;
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        policy = if let Some(host) = entry.strip_prefix("http://") {
            policy.allow_host_with_http(host)
        } else {
            policy.allow_host(entry)
        };
        any = true;
    }
    any.then_some(policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_blank_list_yields_no_policy() {
        assert!(policy_from_hosts_str("").is_none());
        assert!(policy_from_hosts_str("  , ,").is_none());
    }

    #[test]
    fn hosts_build_a_second_domain_policy() {
        let policy =
            policy_from_hosts_str("example.com, http://localhost").expect("hosts configured");
        assert!(policy.allows("https://example.com/"));
        assert!(
            !policy.allows("http://example.com/"),
            "bare hosts are https-only"
        );
        assert!(
            policy.allows("http://localhost:3000/mcp"),
            "http opt-in per host"
        );
        assert!(policy.allows("https://localhost/mcp"));
        // the second domain never widens to unlisted hosts
        assert!(!policy.allows("https://api.fluxrouter.ai/"));
        assert!(!policy.allows("https://other.example/"));
    }
}
