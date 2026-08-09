//! `nanok3 protocol-host` — runs the NDJSON host loop over stdin/stdout,
//! driving real turns through the production stack (Flux + tools + sandbox).

use nano_agent::loop_protection::TurnBudget;
use nano_agent::turn::{TurnEngine, TurnState};
use nano_agent::wiring::{FluxDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::types::Usage;
pub use nano_protocol::host::HostExit;
use nano_protocol::host::{HostConfig, run_host_loop};
use nano_protocol::messages::Event;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;

/// MCP server specs from NANOK3_MCP_SERVERS: a JSON array of
/// {"name": str, "command": str, "args": [str]} entries.
fn mcp_specs_from_env() -> Vec<nano_agent::mcp::McpServerSpec> {
    let Ok(raw) = std::env::var("NANOK3_MCP_SERVERS") else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| {
            Some(nano_agent::mcp::McpServerSpec {
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

fn executor_has_registry(executor: &nano_agent::mcp::McpToolExecutor) -> bool {
    !executor.tool_definitions_from_registry().is_empty()
}

fn executor_tool_definitions(
    executor: &nano_agent::mcp::McpToolExecutor,
) -> Vec<nano_model::types::ToolDefinition> {
    executor.tool_definitions_from_registry()
}

pub async fn run(
    nano_home: &std::path::Path,
    workspace: &std::path::Path,
) -> std::io::Result<HostExit> {
    let Some(api_key) = std::env::var("FLUX_API_KEY")
        .ok()
        .or_else(|| std::env::var("FLUX_TEST_KEY").ok())
    else {
        eprintln!("nanok3: FLUX_API_KEY is required for protocol-host mode");
        return Ok(HostExit::Fatal("missing FLUX_API_KEY".into()));
    };

    let policy =
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy();
    let fs = FsTools::new(policy, workspace);
    let shell = ShellTool::new(nano_home, workspace);
    let executor = RealToolExecutor::new(fs, shell, workspace);
    let driver = FluxDriver::new(FluxCompletionsClient::new(EgressClient::flux()), api_key);
    let approve_all = nano_agent::turn::ApproveAll;

    // MCP: register configured servers (failures log, never crash the host).
    let mut registry = nano_agent::mcp::McpRegistry::new();
    for spec in mcp_specs_from_env() {
        if let Err(err) = registry.register(spec) {
            eprintln!("nanok3: MCP server registration failed: {err}");
        }
    }
    let executor = nano_agent::mcp::McpToolExecutor::new(registry, &executor);
    let mcp_definitions = if executor_has_registry(&executor) {
        executor_tool_definitions(&executor)
    } else {
        vec![]
    };

    // Skills: default roots are <nano_home>/skills and <workspace>/.nano/skills.
    let skill_context = nano_agent::skills::prepare_skill_context(&[
        nano_home.join("skills"),
        workspace.join(".nano").join("skills"),
    ]);

    let mut tool_definitions = v1_tool_definitions();
    tool_definitions.extend(mcp_definitions);

    let engine = TurnEngine {
        model: &driver,
        tools: &executor,
        budget: TurnBudget::default(),
        model_name: "flux-auto".into(),
        tool_definitions,
        approval: Some(&approve_all),
    };
    let skill_context = std::sync::Arc::new(skill_context);

    let config = HostConfig::default();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    run_host_loop(&mut reader, &mut writer, &config, |msg_id, content| {
        let engine = &engine;
        let skill_context = std::sync::Arc::clone(&skill_context);
        async move {
            let result = if let Some(context) = skill_context.as_ref() {
                engine
                    .run_turn_with_context(&msg_id, &content, Some(context.clone()))
                    .await
            } else {
                engine.run_turn(&msg_id, &content).await
            };
            let mut events = Vec::new();
            for op in &result.ops {
                match &op.op {
                    nano_session::op::Op::ToolCall {
                        call_id,
                        name,
                        args,
                        ..
                    } => {
                        events.push(Event::ToolRunning {
                            call_id: call_id.clone(),
                            msg_id: msg_id.clone(),
                            tool_name: name.clone(),
                        });
                        events.push(Event::ToolRequest {
                            call_id: call_id.clone(),
                            msg_id: msg_id.clone(),
                            tool: nano_protocol::messages::ToolRequestBody {
                                name: name.clone(),
                                args: args.clone(),
                                category: None,
                                description: None,
                            },
                        });
                    }
                    nano_session::op::Op::ToolResult {
                        call_id,
                        ok,
                        output_digest,
                        ..
                    } => {
                        events.push(Event::ToolResult {
                            call_id: call_id.clone(),
                            msg_id: msg_id.clone(),
                            output: output_digest.clone(),
                            output_type: Some("text".into()),
                            status: if *ok {
                                "success".into()
                            } else {
                                "failure".into()
                            },
                            tool_name: String::new(),
                            metadata: None,
                        });
                    }
                    _ => {}
                }
            }
            if !result.final_text.is_empty() {
                events.insert(
                    0,
                    Event::TextDelta {
                        msg_id: msg_id.clone(),
                        text: result.final_text.clone(),
                    },
                );
            }
            let stop_reason = match result.state {
                TurnState::Complete => "stop".to_string(),
                TurnState::Stopped(reason) | TurnState::Failed(reason) => reason,
                _ => "interrupted".to_string(),
            };
            (events, Option::<Usage>::None, stop_reason)
        }
    })
    .await
}
