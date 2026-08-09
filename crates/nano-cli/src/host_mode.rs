//! `nanok3 protocol-host` — runs the NDJSON host loop over stdin/stdout,
//! driving real turns through the production stack (Flux + tools + sandbox).

use nano_agent::loop_protection::TurnBudget;
use nano_agent::turn::{TurnEngine, TurnState};
use nano_agent::wiring::{FluxDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::types::Usage;
use nano_protocol::host::{HostConfig, run_host_loop};
pub use nano_protocol::host::HostExit;
use nano_protocol::messages::Event;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;

pub async fn run(nano_home: &std::path::Path, workspace: &std::path::Path) -> std::io::Result<HostExit> {
    let Some(api_key) = std::env::var("FLUX_API_KEY")
        .ok()
        .or_else(|| std::env::var("FLUX_TEST_KEY").ok())
    else {
        eprintln!("nanok3: FLUX_API_KEY is required for protocol-host mode");
        return Ok(HostExit::Fatal("missing FLUX_API_KEY".into()));
    };

    let policy = nano_core::permissions::PermissionProfile::workspace_write()
        .file_system_sandbox_policy();
    let fs = FsTools::new(policy, workspace);
    let shell = ShellTool::new(nano_home, workspace);
    let executor = RealToolExecutor::new(fs, shell, workspace);
    let driver = FluxDriver::new(FluxCompletionsClient::new(EgressClient::flux()), api_key);
    let approve_all = nano_agent::turn::ApproveAll;

    let engine = TurnEngine {
        model: &driver,
        tools: &executor,
        budget: TurnBudget::default(),
        model_name: "flux-auto".into(),
        tool_definitions: v1_tool_definitions(),
        approval: Some(&approve_all),
    };

    let config = HostConfig::default();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    run_host_loop(&mut reader, &mut writer, &config, |msg_id, content| {
        let engine = &engine;
        async move {
            let result = engine.run_turn(&msg_id, &content).await;
            let mut events = Vec::new();
            for op in &result.ops {
                match &op.op {
                    nano_session::op::Op::ToolCall { call_id, name, args, .. } => {
                        events.push(Event::ToolRunning {
                            call_id: call_id.clone(),
                        });
                        events.push(Event::ToolRequest {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            args: args.clone(),
                        });
                    }
                    nano_session::op::Op::ToolResult { call_id, ok, output_digest, .. } => {
                        events.push(Event::ToolResult {
                            call_id: call_id.clone(),
                            ok: *ok,
                            output: output_digest.clone(),
                        });
                    }
                    _ => {}
                }
            }
            if !result.final_text.is_empty() {
                events.insert(0, Event::TextDelta {
                    text: result.final_text.clone(),
                });
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
