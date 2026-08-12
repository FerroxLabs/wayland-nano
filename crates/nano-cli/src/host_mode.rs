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
use std::sync::{Arc, Mutex};

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
    // P1: web_search — the key-gated ladder, resolved ONCE at host start.
    // The meter handle is Lane B's session CostMeter; the stub stands the
    // seam up (no pricing, no cap authority) until it lands.
    let search_meter: Arc<dyn nano_model::metering::UsageSink> =
        Arc::new(nano_model::metering::StubCostMeter::new());
    let search = nano_cli::search_specs::web_search_tool_from_env(Some(search_meter.clone()));
    if let Some(resolved) = &search {
        executor = executor.with_web_search(resolved.tool.clone(), search_meter.clone());
    }
    let driver = FluxDriver::new(FluxCompletionsClient::new(EgressClient::flux()), api_key);

    // C10: the session-owned tools need a session cell set even here (the
    // protocol host has no ACP session — one fixed id journals todo/plan
    // ops under nano_home/sessions, journal-first exactly like acp-host).
    // The plan posture is enforced by PlanAwareApproval; questions are
    // Unavailable on this transport (typed error, posture stays).
    let sessions_dir = nano_home.join("sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    let plan = nano_cli::session_tools::PlanPosture::new(&sessions_dir, "protocol-host")
        .map(|posture| Arc::new(Mutex::new(posture)))
        .map_err(std::io::Error::other)?;
    let todos: Arc<Mutex<Vec<nano_session::op::TodoItem>>> = Arc::new(Mutex::new(Vec::new()));
    let journal = sessions_dir.join("protocol-host.jsonl");
    let gate = nano_cli::session_tools::PlanAwareApproval::new(plan.clone(), workspace);

    // MCP: register configured servers (failures log, never crash the host).
    let registry = nano_cli::mcp_specs::register_all(nano_cli::mcp_specs::mcp_specs_from_env());
    let mcp_executor = nano_agent::mcp::McpToolExecutor::new(registry, &executor);
    let mcp_definitions = if executor_has_registry(&mcp_executor) {
        executor_tool_definitions(&mcp_executor)
    } else {
        vec![]
    };
    let executor = nano_cli::session_tools::SessionTools::new(
        &mcp_executor,
        &gate,
        todos.clone(),
        plan.clone(),
        journal,
        "protocol-host".into(),
    );

    // C5: cross-session memory. The store is <nano_home>/memory; read tools
    // and injection are always on over the user-managed store, write tools
    // only behind NANO_MEMORY_WRITE. The block is re-rendered FRESH every
    // turn (never cached at startup).
    let memory_write = std::env::var("NANO_MEMORY_WRITE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let memory_store = nano_agent::memory::MemoryStore::new(nano_home);
    let executor =
        nano_agent::memory::MemoryToolExecutor::new(memory_store.clone(), memory_write, &executor);

    // Skills: default roots are <nano_home>/skills and <workspace>/.nano/skills.
    let skill_context = nano_agent::skills::prepare_skill_context(&[
        nano_home.join("skills"),
        workspace.join(".nano").join("skills"),
    ]);

    let mut tool_definitions = v1_tool_definitions(search.is_some());
    tool_definitions.extend(mcp_definitions);
    tool_definitions.extend(nano_agent::memory::memory_tool_definitions(memory_write));

    let engine = TurnEngine {
        model: &driver,
        tools: &executor,
        budget: TurnBudget::default(),
        model_name: "flux-auto".into(),
        tool_definitions,
        approval: Some(&gate),
        compaction: None,
        robustness: {
            // C9 §4: the env config channel, same typed-error discipline as
            // the acp host. An invalid value fails the host LOUDLY at
            // startup, never a silent clamp.
            match (
                nano_cli::model_params::effort_from_env(),
                nano_cli::model_params::verbosity_from_env(),
            ) {
                (Ok(reasoning_effort), Ok(verbosity)) => nano_agent::turn::TurnRobustness {
                    reasoning_effort,
                    verbosity,
                    ..Default::default()
                },
                (Err(err), _) | (_, Err(err)) => {
                    eprintln!("wayland-nano: {err}");
                    return Ok(HostExit::Fatal(format!("config: {err}")));
                }
            }
        },
    };
    let skill_context = std::sync::Arc::new(skill_context);
    let plan_cell = plan;
    let todo_cell = todos;

    let config = HostConfig::default();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    run_host_loop(&mut reader, &mut writer, &config, |msg_id, content| {
        let engine = &engine;
        let skill_context = std::sync::Arc::clone(&skill_context);
        let plan_cell = plan_cell.clone();
        let todo_cell = todo_cell.clone();
        let memory_store = memory_store.clone();
        async move {
            // Fresh per-turn context blocks: the C5 memory block is
            // re-rendered from the store every turn (a save/delete/hand-edit
            // in turn N is visible from turn N+1; the combined ceiling uses
            // the conservative 128k window and the skills block's size);
            // C10: the AGENTS.md block is re-read every turn, the plan
            // instructions ride while the posture is active, and the todo
            // list renders while non-empty.
            let skills_chars = match skill_context.as_ref() {
                Some(message) => message
                    .content
                    .iter()
                    .map(|b| match b {
                        nano_model::types::ContentBlock::Text { text } => text.len(),
                        _ => 0,
                    })
                    .sum::<usize>(),
                None => 0,
            };
            let mut context = Vec::new();
            if let Some(memory_block) = nano_agent::memory::prepare_memory_context(
                &memory_store,
                128_000,
                skills_chars,
                nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
            ) {
                context.push(memory_block);
            }
            if let Some(message) = nano_agent::skills::prepare_agents_md_context(workspace) {
                context.push(message);
            }
            {
                let plan = plan_cell.lock().unwrap_or_else(|p| p.into_inner());
                if plan.active {
                    context.push(nano_model::types::Message::system(
                        nano_cli::session_tools::plan_mode_instructions(plan.plan_file()),
                    ));
                }
            }
            {
                let todos = todo_cell.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(block) = nano_cli::session_tools::todo_restore_block(&todos) {
                    context.push(nano_model::types::Message::system(block));
                }
            }
            if let Some(skill) = skill_context.as_ref() {
                context.push(skill.clone());
            }
            let result = engine
                .run_turn_with_context_messages(&msg_id, &content, context)
                .await;
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
                TurnState::Stopped(info) => info.detail,
                TurnState::Failed(err) => err.detail,
                _ => "interrupted".to_string(),
            };
            (events, Option::<Usage>::None, stop_reason)
        }
    })
    .await
}
