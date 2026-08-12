//! `wayland-nano protocol-host` — runs the NDJSON host loop over stdin/stdout,
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
    let Some(api_key) = nano_cli::flux_key::flux_api_key() else {
        eprintln!(
            "wayland-nano: FLUX_API_KEY (or FLUX_API_KEY_FILE) is required for protocol-host mode"
        );
        return Ok(HostExit::Fatal("missing FLUX_API_KEY".into()));
    };

    let policy =
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy();
    let fs = FsTools::new(policy, workspace);
    let shell = ShellTool::new(nano_home, workspace);
    let mut executor = RealToolExecutor::new(fs, shell, workspace);
    // C4: web_fetch is inert (typed denial) unless NANO_WEB_FETCH_HOSTS
    // configures the second egress policy domain.
    if let Some(fetch) = nano_cli::fetch_specs::web_fetch_tool_from_env() {
        executor = executor.with_web_fetch(fetch);
    }
    let driver = FluxDriver::new(FluxCompletionsClient::new(EgressClient::flux()), api_key);
    let approve_all = nano_agent::turn::ApproveAll;

    // MCP: register configured servers (failures log, never crash the host).
    let registry = nano_cli::mcp_specs::register_all(nano_cli::mcp_specs::mcp_specs_from_env());
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
        compaction: None,
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
