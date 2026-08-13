//! MCP server-spec sources shared by the host modes: `NANO_MCP_SERVERS`
//! (operator-supplied) and the ACP session/new|session/load `mcpServers`
//! param (Desktop-published connectors). Both host modes honor both sources;
//! registration failures are logged by the caller, never fatal.
//!
//! §6.1 (F-P3-1) transport dispatch: a `{command}` entry is a stdio server,
//! a `{url}` entry an HTTP one (https ONLY — a plain-http url is a typed
//! InvalidParams rejection at parse, matching `EgressPolicy`'s
//! https-default). HTTP specs parse, mint instance identity, and arm the
//! session egress policy ([`allow_http_mcp_origins`]), but REGISTRATION is
//! a typed refusal (`mcp_transport`) until the dispatcher-bound HTTP
//! connection lands — fail-closed, never a silent skip.

use nano_agent::mcp::{McpServerSpec, SpecSource, Transport};

/// The snake_case wire token of an error kind (for typed-refusal log lines;
/// the mcp_specs `McpElicitationUnsupported` precedent).
pub(crate) fn kind_token(kind: nano_session::NanoErrorKind) -> String {
    serde_json::to_string(&kind)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

/// §6.1: an MCP server url must be an absolute https URL with a host —
/// anything else (plain http, another scheme, unparseable, hostless) is a
/// typed rejection at parse. The url crate normalizes the scheme to
/// lowercase for special schemes, so `HTTPS://` is accepted canonicalized.
fn is_https_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some())
}

fn args_of(v: &serde_json::Value) -> Vec<String> {
    v.get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The §6.1 transport dispatch for one config entry. `None` = the entry is
/// refused or skipped (the log line is the surface; fail-soft per entry is
/// this file's contract — a bad connector must never fail the session). The
/// Stdio `env` is left empty here; the ACP source patches it from the
/// entry's `env` array.
fn transport_from_entry(v: &serde_json::Value) -> Option<Transport> {
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if let Some(command) = v.get("command").and_then(|c| c.as_str()) {
        return Some(Transport::Stdio {
            command: command.to_string(),
            args: args_of(v),
            env: Vec::new(),
        });
    }
    // Non-stdio entry. P3 §5.5: the one honest declaration channel — a
    // server that explicitly declares `requires: ["elicitation"]` on a
    // non-stdio transport is refused TYPED (McpElicitationUnsupported) at
    // configuration time, before any connection is opened.
    let requires_elicitation = v
        .get("requires")
        .and_then(|r| r.as_array())
        .is_some_and(|r| r.iter().any(|x| x.as_str() == Some("elicitation")));
    if requires_elicitation {
        eprintln!(
            "wayland-nano: refusing MCP server '{name}': {} — elicitation over a non-stdio transport",
            kind_token(nano_session::NanoErrorKind::McpElicitationUnsupported)
        );
        return None;
    }
    let Some(url) = v.get("url").and_then(|u| u.as_str()) else {
        eprintln!(
            "wayland-nano: skipping MCP server '{name}': entry carries neither `command` (stdio) nor `url` (http)"
        );
        return None;
    };
    // §6.1: https ONLY — a plain-http (or other-scheme / unparseable /
    // hostless) url is a TYPED InvalidParams rejection at parse; the entry
    // is never registered.
    if !is_https_url(url) {
        eprintln!(
            "wayland-nano: refusing MCP server '{name}': {} — MCP server url must be a valid https URL",
            kind_token(nano_session::NanoErrorKind::InvalidParams)
        );
        return None;
    }
    Some(Transport::Http {
        url: url.to_string(),
    })
}

/// One entry → one spec, shared by both sources. Nameless entries are
/// skipped with a log line; transport dispatch is [`transport_from_entry`].
fn spec_from_entry(
    v: &serde_json::Value,
    source: SpecSource,
    acp_env: bool,
) -> Option<McpServerSpec> {
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
    if name.is_empty() {
        eprintln!("wayland-nano: skipping MCP server entry without a name");
        return None;
    }
    let mut transport = transport_from_entry(v)?;
    if acp_env && let Transport::Stdio { env, .. } = &mut transport {
        *env = v
            .get("env")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| {
                        Some((
                            x.get("name")?.as_str()?.to_string(),
                            x.get("value")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    Some(McpServerSpec {
        name: name.to_string(),
        transport,
        source,
    })
}

/// MCP server specs from NANO_MCP_SERVERS: a JSON array of
/// {"name": str, "command": str, "args": [str]} (stdio) or {"name": str,
/// "url": "https://…"} (HTTP, §6.1) entries. Provenance is
/// `SpecSource::Config` (§2.7 — it rides the instance-id hash).
pub fn mcp_specs_from_env() -> Vec<McpServerSpec> {
    let Ok(raw) = std::env::var("NANO_MCP_SERVERS") else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| spec_from_entry(&v, SpecSource::Config, false))
        .collect()
}

/// MCP server specs from an ACP session/new or session/load params object:
/// the `mcpServers` array Desktop publishes its connectors through
/// (mcpSessionConfig.ts `AcpSessionMcpServer`). `{command}` entries are
/// stdio; `{url}` entries are HTTP (§6.1 — https only, plain http is a
/// typed InvalidParams rejection at parse). Malformed entries are skipped
/// with a log line — a bad connector must never fail the session.
pub fn mcp_specs_from_acp_params(params: &serde_json::Value) -> Vec<McpServerSpec> {
    let Some(servers) = params.get("mcpServers").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|v| spec_from_entry(v, SpecSource::Desktop, true))
        .collect()
}

/// §6.1 last bullet: every configured HTTP MCP server's ORIGIN joins the
/// session egress policy at construction (the https hosts set;
/// `EgressPolicy::allow_url` is fail-closed on non-https, and parse already
/// refused those). The grant is inert until the dispatcher HTTP binding
/// lands — registration of an HTTP spec is a typed refusal today — but
/// deny-by-default is unchanged: no other origin gets a socket.
pub fn allow_http_mcp_origins(
    policy: nano_egress::policy::EgressPolicy,
    specs: &[McpServerSpec],
) -> nano_egress::policy::EgressPolicy {
    specs
        .iter()
        .fold(policy, |policy, spec| match &spec.transport {
            Transport::Http { url } => policy.allow_url(url),
            Transport::Stdio { .. } => policy,
        })
}

/// Registers every spec into a fresh registry, logging failures and
/// continuing — a server that will not spawn must never take the host down
/// (fail-soft registration; calls to missing tools still fail closed in the
/// executor).
pub fn register_all(
    specs: impl IntoIterator<Item = McpServerSpec>,
) -> nano_agent::mcp::McpRegistry {
    register_all_with(specs, None)
}

/// P3: registration with the ToolSearch fair-share count fixed up front
/// (§3.1 — admission is registration-order independent) and the optional
/// elicitation handler factory installed BEFORE any connection opens (§5.2 —
/// the capability is advertised only when a handler actually exists).
pub fn register_all_with(
    specs: impl IntoIterator<Item = McpServerSpec>,
    factory: Option<nano_agent::mcp::ElicitationHandlerFactory>,
) -> nano_agent::mcp::McpRegistry {
    let specs: Vec<McpServerSpec> = specs.into_iter().collect();
    let mut registry = nano_agent::mcp::McpRegistry::new();
    registry.set_configured_server_count(specs.len());
    registry.set_elicitation_handler_factory(factory);
    for spec in specs {
        let name = spec.name.clone();
        if let Err(err) = registry.register(spec) {
            eprintln!("wayland-nano: MCP server '{name}' registration failed: {err}");
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_params_parse_stdio_and_http_skip_the_rest() {
        let params = serde_json::json!({
            "mcpServers": [
                {
                    "name": "fake",
                    "command": "powershell.exe",
                    "args": ["-NoProfile", "-Command", "script"],
                    "env": [{"name": "A", "value": "1"}, {"name": "BROKEN"}]
                },
                { "type": "http", "name": "remote", "url": "https://example.invalid/mcp" },
                { "args": [] },
                "not-an-object"
            ]
        });
        let specs = mcp_specs_from_acp_params(&params);
        assert_eq!(specs.len(), 2, "the stdio and the https entries parse");
        assert_eq!(specs[0].name, "fake");
        assert_eq!(
            specs[0].transport,
            Transport::Stdio {
                command: "powershell.exe".to_string(),
                args: vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "script".to_string()
                ],
                env: vec![("A".to_string(), "1".to_string())],
            }
        );
        // §2.7 provenance: ACP params are Desktop-published connectors.
        assert_eq!(specs[0].source, SpecSource::Desktop);
        // §6.1: the {url} entry is an HTTP spec now (no longer skipped).
        assert_eq!(specs[1].name, "remote");
        assert_eq!(
            specs[1].transport,
            Transport::Http {
                url: "https://example.invalid/mcp".to_string()
            }
        );
        assert_eq!(specs[1].source, SpecSource::Desktop);

        // Absent or non-array mcpServers is simply no servers.
        assert!(mcp_specs_from_acp_params(&serde_json::json!({})).is_empty());
    }

    /// P3 §5.5: a non-stdio server that EXPLICITLY declares
    /// `requires: ["elicitation"]` is refused at configuration time (typed
    /// McpElicitationUnsupported in the log line) — before any connection.
    /// The refusal applies on the §6.1 HTTP path too; a plain `{url}` https
    /// entry parses.
    #[test]
    fn http_elicitation_requirement_is_refused_at_config_time() {
        let params = serde_json::json!({
            "mcpServers": [
                { "type": "http", "name": "elicit-remote", "url": "https://example.invalid/mcp", "requires": ["elicitation"] },
                { "type": "http", "name": "plain-remote", "url": "https://example.invalid/mcp" }
            ]
        });
        let specs = mcp_specs_from_acp_params(&params);
        assert_eq!(specs.len(), 1, "only the requires-declaring entry refuses");
        assert_eq!(specs[0].name, "plain-remote");
        assert!(matches!(specs[0].transport, Transport::Http { .. }));
        assert!(
            mcp_specs_from_acp_params(&serde_json::json!({ "mcpServers": "bogus" })).is_empty()
        );
    }

    /// §6.1: plain http is a TYPED rejection at parse (never registered) —
    /// on both sources. Other non-https schemes refuse the same way.
    #[test]
    fn plain_http_url_is_a_typed_parse_rejection() {
        let params = serde_json::json!({
            "mcpServers": [
                { "name": "cleartext", "url": "http://example.invalid/mcp" },
                { "name": "other-scheme", "url": "ftp://example.invalid/mcp" },
                { "name": "not-a-url", "url": "example.invalid/mcp" },
                { "name": "ok", "url": "https://example.invalid/mcp" }
            ]
        });
        let specs = mcp_specs_from_acp_params(&params);
        assert_eq!(specs.len(), 1, "only the https entry survives parse");
        assert_eq!(specs[0].name, "ok");
    }

    /// §6.1: NANO_MCP_SERVERS gains the url form — {url} https entries
    /// parse into HTTP specs with Config provenance.
    #[test]
    fn env_source_parses_http_entries() {
        // mcp_specs_from_env reads the process env; drive the shared entry
        // parser directly (the env read itself is a one-liner).
        let entry = serde_json::json!({ "name": "remote", "url": "https://example.invalid/mcp" });
        let spec = spec_from_entry(&entry, SpecSource::Config, false).expect("https parses");
        assert_eq!(
            spec.transport,
            Transport::Http {
                url: "https://example.invalid/mcp".to_string()
            }
        );
        assert_eq!(spec.source, SpecSource::Config);
        // Plain http refuses on the env path too.
        let cleartext = serde_json::json!({ "name": "bad", "url": "http://example.invalid/" });
        assert!(spec_from_entry(&cleartext, SpecSource::Config, false).is_none());
        // An entry with neither command nor url is skipped.
        let empty = serde_json::json!({ "name": "empty" });
        assert!(spec_from_entry(&empty, SpecSource::Config, false).is_none());
    }

    /// §6.1 last bullet: HTTP origins join the session policy (https hosts
    /// set); stdio specs add nothing; deny-by-default is untouched.
    #[test]
    fn egress_arm_allows_http_origins_only() {
        let specs = vec![
            McpServerSpec {
                name: "remote".into(),
                transport: Transport::Http {
                    url: "https://mcp.example.invalid/mcp/".into(),
                },
                source: SpecSource::Config,
            },
            McpServerSpec {
                name: "local".into(),
                transport: Transport::Stdio {
                    command: "srv".into(),
                    args: vec![],
                    env: vec![],
                },
                source: SpecSource::Config,
            },
        ];
        let policy = allow_http_mcp_origins(nano_egress::policy::EgressPolicy::new(), &specs);
        assert!(policy.allows_host("https://mcp.example.invalid/mcp/"));
        assert!(
            !policy.allows_host("https://other.invalid/"),
            "no other origin gets a socket"
        );
        assert!(
            !policy.allows_host("http://mcp.example.invalid/"),
            "the http set is never armed by an https spec"
        );
    }
}
