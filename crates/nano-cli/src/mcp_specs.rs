//! MCP server-spec sources shared by the host modes: `NANO_MCP_SERVERS`
//! (operator-supplied) and the ACP session/new|session/load `mcpServers`
//! param (Desktop-published connectors). Both host modes honor both sources;
//! registration failures are logged by the caller, never fatal.

use nano_agent::mcp::McpServerSpec;

/// MCP server specs from NANO_MCP_SERVERS: a JSON array of
/// {"name": str, "command": str, "args": [str]} entries.
pub fn mcp_specs_from_env() -> Vec<McpServerSpec> {
    let Ok(raw) = std::env::var("NANO_MCP_SERVERS") else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            Some(McpServerSpec {
                name: v.get("name")?.as_str()?.to_string(),
                command: v.get("command")?.as_str()?.to_string(),
                args: v
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                env: vec![],
            })
        })
        .collect()
}

/// MCP server specs from an ACP session/new or session/load params object:
/// the `mcpServers` array Desktop publishes its connectors through
/// (mcpSessionConfig.ts `AcpSessionMcpServer`). Only stdio entries carry a
/// `command`; http/sse entries (a `url` instead) are skipped with a log line
/// because we advertise stdio-only mcpCapabilities. Malformed entries are
/// skipped the same way — a bad connector must never fail the session.
pub fn mcp_specs_from_acp_params(params: &serde_json::Value) -> Vec<McpServerSpec> {
    let Some(servers) = params.get("mcpServers").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|v| {
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name.is_empty() {
                eprintln!("wayland-nano: skipping MCP server entry without a name");
                return None;
            }
            let Some(command) = v.get("command").and_then(|c| c.as_str()) else {
                eprintln!(
                    "wayland-nano: skipping MCP server '{name}': only stdio servers (with a command) are supported"
                );
                return None;
            };
            Some(McpServerSpec {
                name: name.to_string(),
                command: command.to_string(),
                args: v
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                env: v
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
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Registers every spec into a fresh registry, logging failures and
/// continuing — a server that will not spawn must never take the host down
/// (fail-soft registration; calls to missing tools still fail closed in the
/// executor).
pub fn register_all(
    specs: impl IntoIterator<Item = McpServerSpec>,
) -> nano_agent::mcp::McpRegistry {
    let mut registry = nano_agent::mcp::McpRegistry::new();
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
    fn acp_params_parse_stdio_and_skip_the_rest() {
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
        assert_eq!(specs.len(), 1, "only the well-formed stdio entry parses");
        assert_eq!(specs[0].name, "fake");
        assert_eq!(specs[0].command, "powershell.exe");
        assert_eq!(specs[0].args, vec!["-NoProfile", "-Command", "script"]);
        assert_eq!(specs[0].env, vec![("A".to_string(), "1".to_string())]);

        // Absent or non-array mcpServers is simply no servers.
        assert!(mcp_specs_from_acp_params(&serde_json::json!({})).is_empty());
        assert!(
            mcp_specs_from_acp_params(&serde_json::json!({ "mcpServers": "bogus" })).is_empty()
        );
    }
}
