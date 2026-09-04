//! `wayland-nano protocol-host` — runs the NDJSON host loop over stdin/stdout,
//! driving real turns through the production stack (Flux + tools + sandbox).

use nano_activation::policy::EffectiveCapability;
use nano_agent::loop_protection::TurnBudget;
use nano_agent::turn::{TurnEngine, TurnState};
use nano_agent::wiring::{FluxDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::types::Usage;
pub use nano_protocol::host::HostExit;
use nano_protocol::host::{HostConfig, run_host_loop};
use nano_protocol::messages::{ErrorBody, Event};
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
    activation_request: Option<&std::path::Path>,
) -> std::io::Result<HostExit> {
    let Some(request_path) = activation_request else {
        return Ok(HostExit::Fatal(
            "protocol-host requires --activation-request".into(),
        ));
    };
    let raw = match std::fs::read(request_path) {
        Ok(value) => value,
        Err(_) => return Ok(HostExit::Fatal("activation request unavailable".into())),
    };
    let activation_gate = match nano_cli::activation::SharedAdmission::open_production(nano_home) {
        Ok(value) => value,
        Err(error) => return Ok(HostExit::Fatal(error)),
    };
    let activation_token =
        match activation_gate.admit_transport(&raw, &nano_cli::activation::now_utc()) {
            Ok(nano_cli::activation::TransportAdmission::Activation(token)) => {
                nano_cli::activation::emit_receipt(token.receipt().as_bytes());
                *token
            }
            Ok(_) => {
                return Ok(HostExit::Fatal(
                    "activation request has wrong method".into(),
                ));
            }
            Err(error) => {
                if let Some(receipt) = error.receipt() {
                    nano_cli::activation::emit_receipt(receipt);
                }
                return Ok(HostExit::Fatal(format!(
                    "activation refused: {}",
                    error.reason()
                )));
            }
        };
    if activation_gate
        .bind_session(&activation_token, "protocol-host")
        .is_err()
        || activation_gate.recheck_session("protocol-host").is_err()
        || activation_gate
            .mark_dispatch_eligible(&activation_token)
            .is_err()
    {
        return Ok(HostExit::Fatal("activation session binding failed".into()));
    }
    let Some(api_key) = nano_cli::flux_key::flux_api_key() else {
        eprintln!(
            "wayland-nano: FLUX_API_KEY (or FLUX_API_KEY_FILE) is required for protocol-host mode"
        );
        return Ok(HostExit::Fatal("missing FLUX_API_KEY".into()));
    };

    let capabilities = activation_token.policy().capabilities();
    let policy = if capabilities.contains(&EffectiveCapability::FilesystemWrite) {
        nano_core::permissions::PermissionProfile::workspace_write().file_system_sandbox_policy()
    } else {
        nano_core::permissions::PermissionProfile::read_only().file_system_sandbox_policy()
    };
    let fs = FsTools::new(policy.clone(), workspace);
    let shell = ShellTool::new(nano_home, workspace);
    let mut executor = RealToolExecutor::new(fs, shell, workspace);
    // C4: web_fetch is inert (typed denial) unless NANO_WEB_FETCH_HOSTS
    // configures the second egress policy domain.
    if capabilities.contains(&EffectiveCapability::NetworkEgress)
        && let Some(fetch) = nano_cli::fetch_specs::web_fetch_tool_from_env()
    {
        executor = executor.with_web_fetch(fetch);
    }
    // P4 §5.5: the repo_map index rides the same policy + workspace; a
    // construction failure leaves the slot empty (typed denial on call).
    match capabilities
        .contains(&EffectiveCapability::FilesystemRead)
        .then(|| nano_tools::repomap::RepoMapTool::new(&policy, workspace))
        .transpose()
    {
        Ok(Some(tool)) => executor = executor.with_repo_map(tool),
        Ok(None) => {}
        Err(error) => eprintln!("wayland-nano: repo_map index unavailable: {error}"),
    }
    // P1: web_search — the key-gated ladder, resolved ONCE at host start.
    // The meter handle is Lane B's session CostMeter; the stub stands the
    // seam up (no pricing, no cap authority) until it lands.
    let search_meter: Arc<dyn nano_model::metering::UsageSink> =
        Arc::new(nano_model::metering::StubCostMeter::new());
    let search = capabilities
        .contains(&EffectiveCapability::NetworkEgress)
        .then(|| nano_cli::search_specs::web_search_tool_from_env(Some(search_meter.clone())))
        .flatten();
    if let Some(resolved) = &search {
        executor = executor.with_web_search(resolved.tool.clone(), search_meter.clone());
    }
    // MCP: configured servers register below (failures log, never crash the
    // host). P3 §6.1: every configured HTTP MCP server's ORIGIN also joins
    // this host's egress policy at construction (inert until the dispatcher
    // HTTP binding lands; deny-by-default otherwise unchanged).
    let mut mcp_specs = if capabilities.contains(&EffectiveCapability::McpInvoke) {
        nano_cli::mcp_specs::mcp_specs_from_env()
    } else {
        Vec::new()
    };
    // S8 activation: installed MCP plugins merge into the SAME registry
    // path as the operator specs (containment/egress/approval posture is
    // identical). A corrupt plugin store is a typed startup refusal (fail
    // closed); an absent store resolves empty.
    match capabilities
        .contains(&EffectiveCapability::McpInvoke)
        .then(|| nano_cli::plugin_cmds::plugin_mcp_specs(nano_home))
        .transpose()
    {
        Ok(Some(specs)) => mcp_specs.extend(specs),
        Ok(None) => {}
        Err(err) => {
            eprintln!("wayland-nano: plugin store unreadable; refusing to start: {err}");
            return Ok(HostExit::Fatal(format!("plugin store: {err}")));
        }
    }
    let driver_policy = nano_cli::mcp_specs::allow_http_mcp_origins(
        nano_egress::policy::EgressPolicy::flux_only(),
        &mcp_specs,
    );
    let driver = FluxDriver::new(
        FluxCompletionsClient::new(EgressClient::new(driver_policy)),
        api_key,
    );

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    run_admitted_with(
        nano_home,
        workspace,
        activation_gate,
        activation_token,
        true,
        driver,
        executor,
        search.is_some(),
        mcp_specs,
        &mut reader,
        &mut writer,
        || Ok(()),
    )
    .await
}

/// The protocol-host production core with transport/model/tool dependencies
/// injected for offline entrypoint evidence.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn run_admitted_with<R, W, D, T, FB>(
    nano_home: &std::path::Path,
    workspace: &std::path::Path,
    activation_gate: nano_cli::activation::SharedAdmission,
    activation_token: nano_activation::admission::AdmittedToken,
    activation_is_bound: bool,
    driver: D,
    executor: T,
    web_search_backed: bool,
    mcp_specs: Vec<nano_agent::mcp::McpServerSpec>,
    reader: &mut R,
    writer: &mut W,
    before_memory_policy: FB,
) -> std::io::Result<HostExit>
where
    R: std::io::BufRead,
    W: std::io::Write,
    D: nano_agent::turn::ModelDriver,
    T: nano_agent::turn::ToolExecutor,
    FB: FnOnce() -> std::io::Result<()>,
{
    if !activation_is_bound
        && (activation_gate
            .bind_session(&activation_token, "protocol-host")
            .is_err()
            || activation_gate.recheck_session("protocol-host").is_err()
            || activation_gate
                .mark_dispatch_eligible(&activation_token)
                .is_err())
    {
        return Ok(HostExit::Fatal("activation session binding failed".into()));
    }
    let capabilities = activation_token.policy().capabilities();

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
    // P3 §3.3: this host's session journal appends route through the one
    // coordinator, exactly like the ACP host.
    let coordinator = nano_session::JournalCoordinator::open(&journal)
        .map(std::sync::Arc::new)
        .map_err(std::io::Error::other)?;
    let begin_id = format!(
        "protocol-host-begin-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    coordinator
        .append(&nano_session::OpEnvelope::new(
            begin_id,
            chrono::Utc::now().to_rfc3339(),
            nano_session::Op::SessionBegin {
                session_id: "protocol-host".into(),
                cwd: workspace.display().to_string(),
            },
        ))
        .and_then(|appended| {
            appended.then_some(()).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, "duplicate session begin")
            })
        })?;
    let resolved_memory = nano_cli::memory_policy::resolve(nano_home)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let memory_seam = match nano_cli::memory_seam::start_entrypoint_after_begin(
        nano_home,
        "protocol-host",
        &activation_token,
        &resolved_memory,
        coordinator.clone(),
        before_memory_policy,
        |_| {},
    ) {
        Ok(seam) => seam,
        Err(error) => return Ok(HostExit::Fatal(error.message)),
    };
    // S7: open the workspace checkpoint store at this journal-open site and
    // run the kill-mid-restore recovery sweep over the persisted tail (this
    // host's journal is a fixed name, so a kill-mid-restore from a previous
    // process IS this journal's tail) BEFORE the first turn. `None` is the
    // typed, loud skip — nothing checkpoint-related registers.
    let journal_tail = nano_session::read_journal(&journal)
        .map(|report| report.envelopes)
        .unwrap_or_default();
    let checkpoint_store = capabilities
        .contains(&EffectiveCapability::CheckpointMutate)
        .then(|| {
            nano_cli::acp_mode::open_checkpoint_store(
                nano_home,
                workspace,
                &coordinator,
                "protocol-host",
                &journal_tail,
            )
        })
        .flatten();
    let gate = nano_cli::session_tools::PlanAwareApproval::new(plan.clone(), workspace);

    // MCP: register configured servers (parsed above for the §6.1 egress
    // arm). P3 §5.2: the elicitation bridge is installed with an
    // auto-decline ask (this host's gate declares questions Unavailable);
    // the registry is shared with the session-tool wrapper so tool_search
    // hydration and resources are visible to both.
    let elicitation: Option<nano_agent::mcp::ElicitationHandlerFactory> = (!mcp_specs.is_empty())
        .then(|| {
            let coordinator = coordinator.clone();
            std::sync::Arc::new(
                move |server_id: &str, display_name: &str, interrupted_call| {
                    // §2.7 (F-P3-3): server_id is the instance id.
                    let bridge =
                        std::sync::Arc::new(nano_agent::elicitation::ElicitationBridge::new(
                            server_id.to_string(),
                            display_name.to_string(),
                            "protocol-host".to_string(),
                            coordinator.clone(),
                            interrupted_call,
                            std::sync::Arc::new(|_| {
                                nano_agent::elicitation::ElicitAskOutcome::Unavailable
                            }),
                        ));
                    nano_agent::mcp::ElicitationHandlerParts {
                        handler: bridge.clone(),
                        slot_retired_hook: bridge.slot_retired_hook(),
                    }
                },
            ) as nano_agent::mcp::ElicitationHandlerFactory
        });
    let mcp_registry = std::sync::Arc::new(std::sync::Mutex::new(
        nano_cli::mcp_specs::register_all_with(mcp_specs, elicitation),
    ));
    {
        let registry = mcp_registry.lock().unwrap_or_else(|p| p.into_inner());
        for warning in &registry.startup_warnings {
            eprintln!("wayland-nano: {warning}");
        }
    }
    let (artifact, epochs) = match nano_cli::activation::runtime_authority(&activation_token) {
        Ok(value) => value,
        Err(_) => return Ok(HostExit::Fatal("activation receipt incomplete".into())),
    };
    let activation_effects = nano_agent::activation_effects::ActivationEffectExecutor::new_live(
        executor,
        activation_token.clone(),
        nano_home,
        artifact,
        epochs,
    );
    let delegated = match nano_cli::activation::delegated_authority(&activation_token, nano_home) {
        Ok(value) => value,
        Err(_) => return Ok(HostExit::Fatal("activation receipt incomplete".into())),
    };
    let mcp_executor =
        nano_agent::mcp::McpToolExecutor::from_shared(mcp_registry.clone(), &activation_effects)
            .with_activation_authority(delegated);
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
        coordinator.clone(),
        "protocol-host".into(),
    );
    // P3 §3.2/§4.3: the MCP session tools on this host too.
    let executor = nano_agent::mcp_session_tools::McpSessionToolExecutor::new(
        Some(mcp_registry.clone()),
        coordinator.clone(),
        "protocol-host".into(),
        &executor,
    );
    // S7: the workspace checkpoint tools — journal-first through the
    // session's coordinator against the store opened (and recovery-swept)
    // above. The gate (PlanAwareApproval) approves create/list/restore on
    // this trust-all surface but denies restore under the plan posture.
    // Registered ONLY when the store opened — `None` was a typed, loud skip.
    let checkpoint_executor;
    let executor: &dyn nano_agent::turn::ToolExecutor = match &checkpoint_store {
        Some(store) => {
            checkpoint_executor = nano_agent::checkpoint_tools::CheckpointToolExecutor::new(
                store.clone(),
                coordinator,
                "protocol-host".into(),
                &executor,
            );
            &checkpoint_executor
        }
        None => &executor,
    };

    // Phase 2: protocol-host is persistent and authenticated, but legacy
    // filesystem/T2 memory remains quarantined until its later migration.
    // Keep a forced-call backstop without opening the store.
    let quarantined_memory;
    let scoped_memory;
    let executor: &dyn nano_agent::turn::ToolExecutor = if let Some(seam) = memory_seam.as_deref() {
        scoped_memory = nano_cli::memory_seam::MemorySeamExecutor::new(seam, executor);
        &scoped_memory
    } else {
        quarantined_memory = nano_agent::memory::MemoryToolExecutor::quarantined(executor);
        &quarantined_memory
    };

    // Skills: default roots are <nano_home>/skills and <workspace>/.nano/skills;
    // installed skills plugins join the same discovery roots. Same fail-closed
    // discipline as the MCP specs above: corrupt store = typed refusal.
    let mut skill_roots = vec![
        nano_home.join("skills"),
        workspace.join(".nano").join("skills"),
    ];
    match nano_cli::plugin_cmds::plugin_skill_roots(nano_home) {
        Ok(roots) => skill_roots.extend(roots),
        Err(err) => {
            eprintln!("wayland-nano: plugin store unreadable; refusing to start: {err}");
            return Ok(HostExit::Fatal(format!("plugin store: {err}")));
        }
    }
    let skill_context = nano_agent::skills::prepare_skill_context(&skill_roots);

    let mut tool_definitions = v1_tool_definitions(web_search_backed, false);
    tool_definitions.retain(|definition| {
        let needed = match definition.name.as_str() {
            "fs_read" | "fs_list" | "search" | "repo_map" | "view_image" => {
                Some(EffectiveCapability::FilesystemRead)
            }
            "fs_write" | "fs_edit" => Some(EffectiveCapability::FilesystemWrite),
            "shell" => Some(EffectiveCapability::ShellExecute),
            "web_fetch" | "web_search" => Some(EffectiveCapability::NetworkEgress),
            _ => None,
        };
        needed.is_none_or(|capability| capabilities.contains(&capability))
    });
    tool_definitions.extend(mcp_definitions);
    if memory_seam.is_some() {
        tool_definitions.extend(nano_cli::memory_seam::tool_definitions());
    }
    {
        let registry = mcp_registry.lock().unwrap_or_else(|p| p.into_inner());
        let listing = registry
            .has_deferred_tools()
            .then(|| registry.deferred_source_listing());
        let resources_available = registry.has_resources_capability();
        drop(registry);
        tool_definitions.extend(nano_agent::wiring::mcp_session_tool_definitions(
            listing.as_deref(),
            resources_available,
        ));
    }
    // S7: the checkpoint definitions advertise exactly when the store
    // opened — the executor wrap above is the servicing half.
    if checkpoint_store.is_some() && capabilities.contains(&EffectiveCapability::CheckpointMutate) {
        tool_definitions.extend(nano_agent::wiring::checkpoint_tool_definitions());
    }
    // S9: the protocol host advertises the CUA surface, but its gate
    // (PlanAwareApproval) has NO prompt channel and CUA always prompts
    // (§2.2) — every cua_* call denies there. The engine seam stays unwired
    // on this host (no bridge), so even a gate bypass could never dispatch:
    // the engine journals a failed CuaBackendUnavailable pair instead.
    if capabilities.contains(&EffectiveCapability::ComputerUse) {
        tool_definitions.extend(nano_agent::cua::cua_tool_definitions());
    }

    let engine = TurnEngine {
        model: &driver,
        tools: &executor,
        budget: TurnBudget {
            max_steps: u32::try_from(activation_token.policy().budgets().max_turns)
                .unwrap_or(u32::MAX),
            max_tool_calls: u32::try_from(activation_token.policy().budgets().max_tool_calls)
                .unwrap_or(u32::MAX),
            max_wall_time: std::time::Duration::from_millis(
                activation_token.policy().budgets().wall_clock_ms,
            ),
        },
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

    run_host_loop(reader, writer, &config, |msg_id, content| {
        let engine = &engine;
        let skill_context = std::sync::Arc::clone(&skill_context);
        let plan_cell = plan_cell.clone();
        let todo_cell = todo_cell.clone();
        let turn_memory_seam = memory_seam.clone();
        async move {
            // C10: the AGENTS.md block is re-read every turn, the plan
            // instructions ride while the posture is active, and the todo
            // list renders while non-empty.
            let mut context = Vec::new();
            if let Some(seam) = turn_memory_seam.as_ref() {
                match seam.context_block(&content) {
                    Ok(Some(block)) => {
                        context.push(nano_model::types::Message::system(block));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let code = serde_json::to_value(error.kind)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_else(|| "activation_continuity_not_enabled".into());
                        return (
                            vec![Event::Error {
                                error: ErrorBody {
                                    code,
                                    message: format!("memory recall failed: {error}"),
                                    retryable: false,
                                },
                                msg_id,
                            }],
                            Option::<Usage>::None,
                            "error".into(),
                        );
                    }
                }
                if let Err(error) = seam.ingest_user_turn(&msg_id, &content) {
                    let code = serde_json::to_value(error.kind)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "continuity_not_enabled".into());
                    return (
                        vec![Event::Error {
                            error: ErrorBody {
                                code,
                                message: error.message,
                                retryable: false,
                            },
                            msg_id,
                        }],
                        Option::<Usage>::None,
                        "error".into(),
                    );
                }
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
