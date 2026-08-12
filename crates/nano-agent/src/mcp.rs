//! MCP integration for the agent loop: server registry, tool exposure, and
//! execution routing through the ToolExecutor seam.
//!
//! Rules: MCP tools are namespaced `mcp__<server>__<tool>`; servers get no
//! security bypass (stdio children die with the registry — no orphans);
//! at-least-once retry semantics are surfaced, never hidden.

use crate::loop_protection::ProgressSignals;
use crate::turn::ToolExecutor;
use crate::turn::ToolOutcome;
use nano_mcp::client::{McpClient, McpError};
use nano_mcp::protocol::McpToolDescriptor;
use nano_model::types::{ToolCall, ToolDefinition};

#[derive(Clone)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub struct McpRegistry {
    servers: Vec<(McpServerSpec, McpClient)>,
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRegistry")
            .field("servers", &self.servers.len())
            .finish()
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self { servers: vec![] }
    }

    /// Connects to a server and registers its tools. A failed handshake fails
    /// the registration, never the runtime.
    pub fn register(&mut self, spec: McpServerSpec) -> Result<usize, McpError> {
        let client = McpClient::connect(&spec.command, &spec.args, &spec.env)?;
        let tools = {
            // list_tools needs &mut client; connect returns initialized client.
            let mut client = client;
            let tools = client.list_tools()?;
            self.servers.push((spec, client));
            tools
        };
        Ok(tools.len())
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// The MCP tool surface as agent-facing tool definitions, namespaced.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut out = Vec::new();
        for (spec, client) in &self.servers {
            let tools = client.cached_tools();
            for tool in tools {
                out.push(namespaced_definition(&spec.name, tool));
            }
        }
        out
    }

    #[cfg(test)]
    fn resolve(&self, namespaced: &str) -> Option<(String, String)> {
        let (server, tool) = namespaced.strip_prefix("mcp__")?.split_once("__")?;
        self.servers
            .iter()
            .find(|(spec, _)| spec.name == server)
            .map(|_| (server.to_string(), tool.to_string()))
    }
}

fn namespaced_definition(server: &str, tool: &McpToolDescriptor) -> ToolDefinition {
    ToolDefinition {
        name: format!("mcp__{server}__{}", tool.name),
        description: format!(
            "[MCP {server}] {}",
            tool.description.clone().unwrap_or_default()
        ),
        input_schema: tool
            .input_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({"type": "object"})),
    }
}

/// ToolExecutor that routes `mcp__` calls to the registry and defers
/// everything else to the wrapped executor.
#[derive(Debug)]
pub struct McpToolExecutor<'a> {
    registry: std::sync::Arc<std::sync::Mutex<McpRegistry>>,
    inner: &'a dyn ToolExecutor,
}

impl<'a> McpToolExecutor<'a> {
    pub fn new(registry: McpRegistry, inner: &'a dyn ToolExecutor) -> Self {
        Self {
            registry: std::sync::Arc::new(std::sync::Mutex::new(registry)),
            inner,
        }
    }

    /// Shares an existing registry (e.g. the per-session registry the ACP
    /// host builds at session/new) instead of taking ownership of a fresh
    /// one, so every turn of the session routes through the same servers.
    pub fn from_shared(
        registry: std::sync::Arc<std::sync::Mutex<McpRegistry>>,
        inner: &'a dyn ToolExecutor,
    ) -> Self {
        Self { registry, inner }
    }

    /// The namespaced MCP tool definitions from the registry.
    pub fn tool_definitions_from_registry(&self) -> Vec<ToolDefinition> {
        self.registry.lock().unwrap().tool_definitions()
    }
}

#[async_trait::async_trait]
impl ToolExecutor for McpToolExecutor<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        if !call.name.starts_with("mcp__") {
            return self.inner.execute(call).await;
        }
        let mut registry = self.registry.lock().unwrap();
        let Some(index) = registry
            .servers
            .iter()
            .position(|(spec, _)| call.name.starts_with(&format!("mcp__{}__", spec.name)))
        else {
            return ToolOutcome {
                ok: false,
                output: format!("unknown MCP tool: {}", call.name),
                progress: ProgressSignals::default(),
                error_kind: Some(nano_session::NanoErrorKind::UnknownTool),
            };
        };
        let tool = call
            .name
            .strip_prefix(&format!("mcp__{}__", registry.servers[index].0.name))
            .unwrap_or(&call.name)
            .to_string();
        let (_, client) = &mut registry.servers[index];
        match client.call_tool_mutable(&tool, call.arguments.clone()) {
            Ok(result) => ToolOutcome {
                ok: !result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                output: serde_json::to_string_pretty(&result).unwrap_or_default(),
                progress: ProgressSignals {
                    new_information: true,
                    process_outcome_changed: true,
                    ..Default::default()
                },
                // An isError payload is the server reporting a tool-level
                // failure — typed as mcp_server (design §3).
                error_kind: result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    .then_some(nano_session::NanoErrorKind::McpServer),
            },
            Err(err) => ToolOutcome {
                ok: false,
                output: err.to_string(),
                progress: ProgressSignals::default(),
                error_kind: Some(crate::error_map::kind_of_mcp(&err)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespacing_shapes() {
        let descriptor = McpToolDescriptor {
            name: "echo".into(),
            description: Some("echo back".into()),
            input_schema: None,
        };
        let def = namespaced_definition("fs", &descriptor);
        assert_eq!(def.name, "mcp__fs__echo");
        assert!(def.description.contains("[MCP fs]"));
    }

    #[test]
    fn resolve_rejects_unknown_namespaces() {
        let registry = McpRegistry::new();
        assert!(registry.resolve("mcp__missing__tool").is_none());
        assert!(registry.resolve("not-mcp").is_none());
        assert!(registry.resolve("mcp__no_tool_sep").is_none());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::loop_protection::ProgressSignals;
    use crate::turn::ToolExecutor;

    #[tokio::test]
    async fn executor_routes_namespaced_call_to_server() {
        // Full round-trip through a real stdio fake server (powershell on
        // Windows, sh on unix — same JSON-RPC line protocol both ways).
        #[cfg(windows)]
        let (command, args) = {
            let script = r#"
$reader = [System.Console]::In
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { break }
    $obj = $line | ConvertFrom-Json
    if ($obj.method -eq "initialize") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"protocolVersion`":`"2025-03-26`",`"capabilities`":{},`"serverInfo`":{`"name`":`"fake`",`"version`":`"0`"}}}")
    } elseif ($obj.method -eq "tools/list") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"tools`":[{`"name`":`"echo`",`"description`":`"echoes`"}]}}")
    } elseif ($obj.method -eq "tools/call") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"content`":`"pong`",`"isError`":false}}")
    }
}
"#;
            (
                "powershell.exe".to_string(),
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    script.to_string(),
                ],
            )
        };
        #[cfg(unix)]
        let (command, args) = {
            let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *'"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" ;;
        *'"tools/list"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echoes"}]}}\n' "$id" ;;
        *'"tools/call"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"content":"pong","isError":false}}\n' "$id" ;;
    esac
done
"#;
            ("sh".to_string(), vec!["-c".to_string(), script.to_string()])
        };
        let mut registry = McpRegistry::new();
        let registered = registry.register(McpServerSpec {
            name: "fake".into(),
            command,
            args,
            env: vec![],
        });
        assert_eq!(registered.expect("register"), 1);
        assert_eq!(registry.tool_definitions().len(), 1);
        assert_eq!(registry.tool_definitions()[0].name, "mcp__fake__echo");

        #[derive(Debug)]
        struct Noop;
        #[async_trait::async_trait]
        impl ToolExecutor for Noop {
            async fn execute(&self, _call: &ToolCall) -> crate::turn::ToolOutcome {
                crate::turn::ToolOutcome {
                    ok: false,
                    output: "should not route here".into(),
                    progress: ProgressSignals::default(),
                    error_kind: None,
                }
            }
        }
        let executor = McpToolExecutor::new(registry, &Noop);
        let outcome = executor
            .execute(&ToolCall {
                id: "c1".into(),
                name: "mcp__fake__echo".into(),
                arguments: serde_json::json!({"text": "ping"}),
            })
            .await;
        assert!(outcome.ok, "mcp call must succeed: {}", outcome.output);
        assert!(outcome.output.contains("pong"));
    }
}
