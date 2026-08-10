//! `nanok3 acp-host` — ACP adapter: Desktop's stdio JSON-RPC protocol driving
//! the real turn engine. This is the zero-Desktop-change integration door.

use nano_agent::loop_protection::TurnBudget;
use nano_agent::turn::TurnEngine;
use nano_agent::wiring::{FluxDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::types::Usage;
use nano_protocol::acp::{
    JsonRpcRequest, JsonRpcResponse, agent_capabilities,
    agent_message_chunk, prompt_result, session_new_result, tool_call_done, tool_call_update,
};
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;
use std::io::{BufRead, Write};

struct Session {
    id: String,
    workspace: std::path::PathBuf,
}

pub async fn run(nano_home: &std::path::Path) -> std::io::Result<i32> {
    let Some(api_key) = crate::flux_key::flux_api_key() else {
        eprintln!(
            "nanok3: FLUX_API_KEY (or FLUX_API_KEY_FILE) is required for acp-host mode"
        );
        return Ok(2);
    };

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();

    let mut session: Option<Session> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(0); // stdin closed: clean exit
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(err) => {
                write_json(
                    &mut writer,
                    &JsonRpcResponse::err(
                        serde_json::Value::Null,
                        -32700,
                        format!("parse error: {err}"),
                    ),
                )?;
                continue;
            }
        };

        match request.method.as_str() {
            "initialize" => {
                write_json(
                    &mut writer,
                    &JsonRpcResponse::ok(request.id, agent_capabilities()),
                )?;
            }
            "authenticate" => {
                write_json(
                    &mut writer,
                    &JsonRpcResponse::err(
                        request.id,
                        -32602,
                        "nanok3 uses FLUX_API_KEY from the environment; no interactive auth",
                    ),
                )?;
            }
            "session/new" => {
                let params = request.params.unwrap_or_default();
                let cwd = params
                    .get("cwd")
                    .and_then(|c| c.as_str())
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let id = format!("nanok3-session-{}", std::process::id());
                session = Some(Session {
                    id: id.clone(),
                    workspace: cwd,
                });
                write_json(
                    &mut writer,
                    &JsonRpcResponse::ok(request.id, session_new_result(&id)),
                )?;
            }
            "session/prompt" => {
                let Some(active) = session.as_ref() else {
                    write_json(
                        &mut writer,
                        &JsonRpcResponse::err(request.id, -32602, "no session: call session/new first"),
                    )?;
                    continue;
                };
                let params = request.params.clone().unwrap_or_default();
                let text = params
                    .get("prompt")
                    .and_then(|p| p.as_array())
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let session_id = active.id.clone();
                let workspace = active.workspace.clone();

                let (_events, usage, stop_reason) =
                    run_acp_turn(&mut writer, &session_id, &workspace, nano_home, &api_key, &text)
                        .await?;

                let _ = usage;
                write_json(
                    &mut writer,
                    &JsonRpcResponse::ok(request.id, prompt_result(&stop_reason)),
                )?;
            }
            "session/cancel" => {
                // Notification: no response. v1 cancellation is step-boundary
                // in the engine; a fired flag ends the current turn early.
            }
            other => {
                write_json(
                    &mut writer,
                    &JsonRpcResponse::method_not_found(request.id, other),
                )?;
            }
        }
    }
}

/// Runs one turn, streaming ACP session/update notifications for every event.
async fn run_acp_turn<W: Write>(
    writer: &mut W,
    session_id: &str,
    workspace: &std::path::Path,
    nano_home: &std::path::Path,
    api_key: &str,
    input: &str,
) -> std::io::Result<((), Option<Usage>, String)> {
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

    let result = engine.run_turn(session_id, input).await;

    for op in &result.ops {
        match &op.op {
            nano_session::op::Op::ToolCall {
                call_id, name, args, ..
            } => {
                write_json(
                    writer,
                    &tool_call_update(session_id, call_id, name, args),
                )?;
            }
            nano_session::op::Op::ToolResult {
                call_id, ok, output_digest, ..
            } => {
                write_json(
                    writer,
                    &tool_call_done(session_id, call_id, *ok, output_digest),
                )?;
            }
            _ => {}
        }
    }
    if !result.final_text.is_empty() {
        write_json(
            writer,
            &agent_message_chunk(session_id, &result.final_text),
        )?;
    }

    let stop_reason = match result.state {
        nano_agent::turn::TurnState::Complete => "end_turn".to_string(),
        nano_agent::turn::TurnState::Stopped(_) => "cancelled".to_string(),
        _ => "error".to_string(),
    };
    Ok(((), None, stop_reason))
}

fn write_json<W: Write, T: serde::Serialize>(writer: &mut W, value: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).unwrap_or_default();
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}
