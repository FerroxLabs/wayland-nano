//! Exec orchestration (C11 §2): the full `wayland-nano exec` run — session
//! bootstrap via the ONE honest path, the SessionGuard, the executor stack
//! (core tools → cronjob → MCP → goal control channel), the goal driver,
//! and the exit-code / `--output-last-message` discipline. Generic over
//! driver/tool factories and the output sink so integration tests drive it
//! in-process with scripted doubles (the same discipline as ACP `serve`).

use crate::exec_mode::{
    ExecApproval, ExecEvents, ExecParams, atomic_replace_write, exit_code_for_goal,
    exit_code_for_turn, goal_turn_stop, run_exec_turn,
};
use nano_agent::bootstrap::{BootstrappedSession, bootstrap_session, session_guard_registry};
use nano_agent::goal::{
    GoalControl, GoalDriveOutcome, GoalToolExecutor, GoalTurnOutcome, drive_goal,
    goal_complete_tool_definition,
};
use nano_agent::turn::TurnState;
use nano_protocol::permission_mode::PermissionMode;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

/// The full exec run. Returns the exit code (§2.2 matrix).
#[allow(clippy::too_many_arguments)]
pub async fn run_exec_with<W, FD, FD2, FT, D, T>(
    sessions_dir: &Path,
    nano_home: &Path,
    workspace: &Path,
    params: &ExecParams,
    model_name: &str,
    make_driver: FD,
    // P5 §4: the ladder's per-candidate factory — SAME driver output type,
    // but production builds it with `RetryConfig::single_attempt` so every
    // physical attempt is visible to (and countable against) the global
    // three-attempt budget. Pins/implicit/goal turns keep `make_driver`.
    make_ladder_driver: FD2,
    make_tools: FT,
    // P1: whether the executor stack carries a resolved web_search backend
    // (the make_tools closure attached it) — the advertised surface must
    // match the executor's capability exactly (design §2.3 double guard).
    web_search_backed: bool,
    sandbox_available: bool,
    mcp_specs: &[nano_agent::mcp::McpServerSpec],
    // P5 §1: the resolved routing decision (journaled per turn; the
    // auto_client_side fail-closed refusal is handled before any turn runs).
    routing: &crate::exec_mode::ExecRouting,
    out: W,
) -> i32
where
    W: Write + Send,
    FD: Fn() -> D,
    FD2: Fn() -> D,
    FT: Fn(&Path, PermissionMode) -> (T, nano_core::permissions::FileSystemSandboxPolicy),
    D: nano_agent::turn::ModelDriver,
    T: nano_agent::turn::ToolExecutor,
{
    // 1. Resolve + bootstrap the session (the ONE honest bootstrap path).
    let (seed, resumed) = match crate::exec_mode::resolve_seed(sessions_dir, &params.resume) {
        Ok(seed) => seed,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return 2;
        }
    };
    // F-P4-3: a RESUMED session is owned BEFORE bootstrap appends the
    // resume marker — resuming a session another host owns is a typed
    // refusal, never a second writer. (Id validation/existence stay with
    // bootstrap: an unsafe or missing id skips ownership here and fails
    // typed below without creating a file.) A fresh session's id is minted
    // inside bootstrap, so its ownership follows; the fresh nanosecond id
    // is unguessable, so no contender can hold that lock first.
    let pre_ownership = match &seed {
        nano_agent::bootstrap::SessionSeed::Resume(id) => {
            let journal = sessions_dir.join(format!("{id}.jsonl"));
            if !nano_agent::bootstrap::is_fs_safe_session_id(id) || !journal.exists() {
                None
            } else {
                match session_guard_registry().try_own(&journal) {
                    Ok(ownership) => Some(ownership),
                    Err(err) => {
                        eprintln!("wayland-nano: {err}");
                        return 2;
                    }
                }
            }
        }
        nano_agent::bootstrap::SessionSeed::New => None,
    };
    let session = match bootstrap_session(sessions_dir, workspace, seed) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return 2;
        }
    };
    let _ownership = match pre_ownership {
        Some(ownership) => ownership,
        None => match session_guard_registry().try_own(&session.journal_path) {
            Ok(ownership) => ownership,
            Err(err) => {
                eprintln!("wayland-nano: {err}");
                return 2;
            }
        },
    };
    // 2. The SessionGuard: exec holds it for its whole run (one exclusion
    //    for turns, forks, and cron fires). On the owned journal this is the
    //    in-process layer only — the lifetime ownership above is the OS
    //    layer. Busy is a typed error, exit 2.
    let _guard = match session_guard_registry().try_acquire(&session.journal_path) {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return 2;
        }
    };

    let events = Arc::new(Mutex::new(ExecEvents::new(out, session.session_id.clone())));
    events
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .session_started(&workspace.display().to_string(), params.mode, resumed);

    // P3 §3.3: the exec session's appends route through the one coordinator.
    let journal = match nano_session::JournalCoordinator::open(&session.journal_path) {
        Ok(coordinator) => Arc::new(coordinator),
        Err(err) => {
            events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .error(&format!("cannot open session journal: {err}"));
            return 2;
        }
    };
    let journal_sequence = Arc::new(AtomicU64::new(1));
    let context = crate::acp_mode::messages_from_envelopes(&session.envelopes);

    // P5 §4.1: reconcile a kill-interrupted routed turn — journal the §3.5
    // estimate receipts for in-flight attempts (a consumed attempt is never
    // free). Fail-closed: a journal that cannot take the reconciliation
    // receipt gets no run.
    if let Some(open) = &session.state.open_turn
        && let Some(turn_routing) = session.state.routing.get(&open.turn_id)
        && turn_routing.snapshot.is_some()
    {
        let sink = crate::auto_routing::CoordinatorRoutingSink(journal.clone());
        if let Err(err) = crate::auto_routing::reconcile_interrupted(
            &sink,
            &open.turn_id,
            turn_routing,
            &open.input,
        ) {
            events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .error(&format!("cannot reconcile interrupted routing: {err}"));
            return 2;
        }
    }

    // P5 §5/§3: auto_client_side turns run the §4 ladder over the
    // catalog-admitted candidates. Exec turns always advertise the v1 tool
    // surface, so capability proof comes from the vendored tool-capability
    // catalog (layered with the S1 evidence-capture probe arm). When NO
    // candidate is admissible the turn fails closed with the typed
    // capability-empty refusal BEFORE any transmission (journaled: the
    // snapshot carries the rejected rungs).
    let mut auto_plan: Option<crate::auto_routing::CandidatePlan> = None;
    if routing.mode == nano_session::RoutingMode::AutoClientSide {
        let requirements = crate::auto_routing::requirements_of(
            &[],
            &context,
            &[nano_model::types::ToolDefinition {
                name: "exec_tool_surface".to_string(),
                description: "exec always advertises the v1 tool surface".to_string(),
                input_schema: serde_json::Value::Null,
            }],
        );
        let vision = match nano_model::vision_catalog::VisionCatalog::vendored() {
            Ok(catalog) => catalog,
            Err(err) => {
                eprintln!("wayland-nano: vision catalog unavailable: {err}");
                return 2;
            }
        };
        let tool_catalog = match nano_model::tool_capability::ToolCapabilityCatalog::vendored() {
            Ok(catalog) => crate::auto_routing::ProbeToolCatalog {
                inner: catalog,
                probe: routing.tools_probe,
            },
            Err(err) => {
                eprintln!("wayland-nano: tool capability catalog unavailable: {err}");
                return 2;
            }
        };
        let router = crate::provider_router::ProviderRouter::default();
        let candidate_inputs = crate::auto_routing::CandidateInputs {
            router: &router,
            get_env: &|_| None,
            now_unix_secs: 0,
            flux_credentialed: true, // run() resolved the Flux key
            flux_advertised: &[],
            vision: &vision,
            tools: &tool_catalog,
            approved_leaves: &[],
            requirements,
        };
        let plan = crate::auto_routing::construct_candidates(&candidate_inputs);
        let turn_id = format!("{}-turn-{}", session.session_id, session.turn_counter + 1);
        let routing_sink = crate::auto_routing::CoordinatorRoutingSink(journal.clone());
        if !crate::auto_routing::journal_snapshot(
            &routing_sink,
            &turn_id,
            routing.mode,
            &routing.reference,
            crate::auto_routing::ATTEMPT_BUDGET,
            plan.candidates.clone(),
            crate::auto_routing::snapshot_digest(&candidate_inputs, &plan),
        ) {
            events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .error("cannot journal the routing snapshot");
            return 2;
        }
        if plan.admitted.is_empty() {
            let err =
                crate::provider_router::ProviderError::capability_empty(if requirements.tools {
                    "tool-use"
                } else {
                    "image_in"
                });
            events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .error(&nano_egress::redact::sanitize_text(&err.message));
            // A pre-dispatch routing refusal is a model-class failure (exit 1).
            return finish_exec(1, &params.output_last_message, "", &events);
        }
        if params.goal.is_some() {
            // Goal-mode Auto needs a PER-TURN ladder (the §4 budget is per
            // Auto turn); v1 builds the ladder for plain turns only. Typed
            // usage refusal, never a silent budget share across goal turns.
            eprintln!(
                "wayland-nano: --auto routing is not supported with --goal yet (plain exec turns only)"
            );
            return 2;
        }
        auto_plan = Some(plan);
    }

    // 3. Executor stack: core tools → cronjob store tool → MCP merge.
    //    (The goal control channel wraps this per-goal below; without a
    //    goal the `goal_complete` definition is never advertised.)
    let (tools, policy) = make_tools(workspace, params.mode);
    let cron_store = nano_agent::cron::JsonCronStore::new(nano_home);
    // F-6: journal-first create/delete ride the session's coordinator. The
    // exec GATE typed-denies every cronjob action on this non-interactive
    // surface (create/delete would prompt; list stays out of v1 headless
    // scope) — the executor is wired so the denial is a gate decision, not
    // an unwired-tool hole.
    let with_cron = nano_agent::cron::CronjobExecutor::new(
        &tools,
        &cron_store,
        session.session_id.clone(),
        &journal,
    );
    // P3: the registry is SHARED (executor + session-tool wrapper), the
    // elicitation bridge auto-declines on this non-interactive surface
    // (§5.2: would-prompt ⇒ deny), and the journaled hydration state is
    // re-applied through the §3.4 canonical digest gate (mismatch ⇒
    // drop-and-notify on stderr; churn ⇒ pinned Deferred for the session).
    let mcp_registry = {
        let elicitation: Option<nano_agent::mcp::ElicitationHandlerFactory> =
            if mcp_specs.is_empty() {
                None
            } else {
                let coordinator = journal.clone();
                let session_id = session.session_id.clone();
                Some(std::sync::Arc::new(
                    move |server_id: &str, display_name: &str, interrupted_call| {
                        // §2.7 (F-P3-3): server_id is the instance id.
                        let bridge =
                            std::sync::Arc::new(nano_agent::elicitation::ElicitationBridge::new(
                                server_id.to_string(),
                                display_name.to_string(),
                                session_id.clone(),
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
                ))
            };
        let registry = crate::mcp_specs::register_all_with(mcp_specs.to_vec(), elicitation);
        Arc::new(Mutex::new(registry))
    };
    {
        let mut registry = mcp_registry.lock().unwrap_or_else(|p| p.into_inner());
        for warning in registry.startup_warnings.clone() {
            eprintln!("wayland-nano: {warning}");
        }
        for notice in registry.resume_hydration(
            &session.state.mcp_hydrated,
            &session.state.mcp_tools_digest,
            &session.state.mcp_recent_digests,
        ) {
            eprintln!("wayland-nano: {notice}");
        }
    }
    let with_mcp = nano_agent::mcp::McpToolExecutor::from_shared(mcp_registry.clone(), &with_cron);
    let with_mcp = nano_agent::mcp_session_tools::McpSessionToolExecutor::new(
        (!mcp_specs.is_empty()).then(|| mcp_registry.clone()),
        journal.clone(),
        session.session_id.clone(),
        &with_mcp,
    );
    let mut extra_definitions = vec![nano_agent::cron::cronjob_tool_definition()];
    {
        let registry = mcp_registry.lock().unwrap_or_else(|p| p.into_inner());
        extra_definitions.extend(registry.tool_definitions());
    }
    // P3 §3.2/§4.3: advertise the MCP session tools only when the live
    // registry warrants them (deferred inventory / resources capability).
    {
        let registry = mcp_registry.lock().unwrap_or_else(|p| p.into_inner());
        let listing = registry
            .has_deferred_tools()
            .then(|| registry.deferred_source_listing());
        let resources_available = registry.has_resources_capability();
        drop(registry);
        extra_definitions.extend(nano_agent::wiring::mcp_session_tool_definitions(
            listing.as_deref(),
            resources_available,
        ));
    }

    // P4 §2.6/§11: session-start rules load, fail-closed — an invalid or
    // insecurely configured rules.toml runs this exec with zero user rules
    // and a loud stderr warning, never a partial trust.
    let (rules, rules_warning) = crate::shell_rules::load_session_rules(nano_home);
    if let Some(warning) = rules_warning {
        eprintln!("wayland-nano: {warning}");
    }
    let gate = ExecApproval {
        mode: params.mode,
        policy,
        cwd: workspace.to_path_buf(),
        sandbox_available,
        events: events.clone(),
        // P2a §9.1: sticky-OR fold of the resumed journal — an
        // image-influenced session clamps protected trust mutations to
        // DENY on this non-interactive surface.
        image_influenced: crate::acp_mode::image_influenced_from_envelopes(&session.envelopes),
        rules,
        rule_denial: std::sync::Arc::new(std::sync::Mutex::new(None)),
    };

    let live_goal = session
        .state
        .goal
        .clone()
        .filter(|goal| !goal.is_terminal());

    // 4a. Plain exec (no --goal): one turn to completion. A live paused
    // goal stays paused — only `exec --goal` resumes it.
    if params.goal.is_none() {
        // P5 §4: an auto turn rides the ladder built from the journaled
        // snapshot's admitted candidates (one single-attempt driver per
        // candidate — the driver reads the model id from the request,
        // which the ladder rewrites per rung); pins/implicit dispatch
        // exactly like pre-P5. The ladder turn id is the turn's id (both
        // derive from turn_counter + 1 — the turn has not advanced since
        // the snapshot was journaled above).
        let driver = match &auto_plan {
            Some(plan) => {
                let turn_id = format!("{}-turn-{}", session.session_id, session.turn_counter + 1);
                let routing_sink = crate::auto_routing::CoordinatorRoutingSink(journal.clone());
                let ladder_candidates = plan
                    .admitted
                    .iter()
                    .map(|candidate| crate::auto_routing::LadderCandidate {
                        plan: candidate.clone(),
                        // §4: single-attempt retry posture — every physical
                        // attempt is visible to the budget.
                        transport: crate::auto_routing::DriverTransport(make_ladder_driver()),
                    })
                    .collect();
                let ladder = crate::auto_routing::Ladder::new(
                    &turn_id,
                    routing.mode,
                    &routing.reference,
                    ladder_candidates,
                    Arc::new(routing_sink),
                    // Exec wires no pricing catalog; the §6 alias
                    // provenance-only rule stands (unpriced).
                    None,
                    false,
                    crate::auto_routing::ATTEMPT_BUDGET,
                    0,
                );
                crate::auto_routing::PromptDriver::Auto(crate::auto_routing::AutoDriver::new(
                    ladder,
                ))
            }
            None => crate::auto_routing::PromptDriver::Pinned(make_driver()),
        };
        let outcome = run_plain_turn(
            &driver,
            &with_mcp,
            &gate,
            model_name,
            &session,
            context,
            &journal,
            &events,
            &extra_definitions,
            web_search_backed,
            &params.prompt,
            routing.mode,
        )
        .await;
        let exit = exit_code_for_turn(&outcome.state);
        if exit != 0 {
            let reason = match &outcome.state {
                // C7 typed states: the events channel carries the static
                // table presentation, never the logs-side detail.
                TurnState::Failed(err) => nano_session::error_codes::error_presentation(err.kind),
                TurnState::Stopped(info) => {
                    nano_session::error_codes::error_presentation(info.kind)
                }
                other => other.label().to_string(),
            };
            events
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .error(&reason);
        }
        return finish_exec(
            exit,
            &params.output_last_message,
            &outcome.final_text,
            &events,
        );
    }

    // 4b. Goal exec: begin a fresh goal or explicitly resume the paused one.
    let (goal_id, objective, budgets) = match (params.goal.clone(), live_goal) {
        // Explicit resume of an interrupted goal (normalized to paused on
        // load). The journaled goal is authoritative; the params' objective
        // and budgets are ignored on a resume.
        (Some(_), Some(goal)) => {
            if let Err(err) = crate::exec_mode::resume_goal(
                &journal,
                &journal_sequence,
                &session.session_id,
                &goal.goal_id,
            ) {
                events.lock().unwrap_or_else(|p| p.into_inner()).error(&err);
                return 2;
            }
            (goal.goal_id, goal.objective, goal.budgets)
        }
        (Some(spec), None) => {
            match crate::exec_mode::begin_goal(
                &journal,
                &journal_sequence,
                &session.session_id,
                &spec.objective,
                &spec.budgets,
            ) {
                Ok(goal_id) => (goal_id, spec.objective, spec.budgets),
                Err(err) => {
                    events.lock().unwrap_or_else(|p| p.into_inner()).error(&err);
                    return 2;
                }
            }
        }
        (None, _) => unreachable!("the plain path above returns for --goal-less runs"),
    };

    let control = GoalControl::new(&goal_id);
    let executor = GoalToolExecutor::new(
        &with_mcp,
        Some(control.clone()),
        journal.clone(),
        session.session_id.clone(),
        journal_sequence.clone(),
    );
    let mut goal_definitions = extra_definitions.clone();
    goal_definitions.push(goal_complete_tool_definition());

    let mut turn_counter = session.turn_counter;
    let clock = crate::exec_mode::system_clock();
    let routing_mode = routing.mode;
    let drive = drive_goal(
        journal.clone(),
        &journal_sequence,
        &session.session_id,
        &goal_id,
        &objective,
        &budgets,
        control.clone(),
        &clock,
        |prompt: String| {
            turn_counter += 1;
            let counter = turn_counter;
            let journal_now = journal.clone();
            let events_now = events.clone();
            let defs_now = goal_definitions.clone();
            let driver = make_driver();
            let session_id = session.session_id.clone();
            let journal_path = session.journal_path.clone();
            let gate_ref = &gate;
            let executor_ref = &executor;
            async move {
                // One honest code path: the turn's context is rebuilt from
                // the journal, exactly like session/load and acp's
                // between-turn rebuild.
                let context = nano_session::read_journal(&journal_path)
                    .map(|report| crate::acp_mode::messages_from_envelopes(&report.envelopes))
                    .unwrap_or_default();
                let turn_id = format!("{}-turn-{}", session_id, counter);
                let view = BootstrappedSession {
                    session_id,
                    journal_path,
                    envelopes: Vec::new(),
                    state: nano_session::SessionState::new(),
                    turn_counter: counter,
                };
                let outcome = run_exec_turn(
                    &driver,
                    executor_ref,
                    gate_ref,
                    model_name,
                    &view,
                    &turn_id,
                    &prompt,
                    context,
                    journal_now,
                    events_now,
                    &defs_now,
                    web_search_backed,
                    routing_mode,
                )
                .await;
                GoalTurnOutcome {
                    stop: goal_turn_stop(&outcome.state),
                    usage: outcome.usage,
                }
            }
        },
    )
    .await;

    let exit = exit_code_for_goal(&drive);
    if exit != 0 {
        let message = match &drive {
            GoalDriveOutcome::Blocked { reason } => format!("goal blocked: {reason:?}"),
            GoalDriveOutcome::Paused => "goal paused".to_string(),
            GoalDriveOutcome::EngineError => "goal engine error".to_string(),
            GoalDriveOutcome::TurnBudgetTrip => "turn-level budget trip".to_string(),
            GoalDriveOutcome::Complete { .. } => unreachable!(),
        };
        events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .error(&message);
    }
    // On completion the trusted summary is the final assistant text; a
    // non-terminal exit writes nothing (the file stays untouched).
    let last_text = match &drive {
        GoalDriveOutcome::Complete { summary } => summary.clone(),
        _ => String::new(),
    };
    finish_exec(exit, &params.output_last_message, &last_text, &events)
}

#[allow(clippy::too_many_arguments)]
async fn run_plain_turn<D, T, W>(
    driver: &D,
    executor: &T,
    gate: &ExecApproval<W>,
    model_name: &str,
    session: &BootstrappedSession,
    context: Vec<nano_model::types::Message>,
    journal: &Arc<nano_session::JournalCoordinator>,
    events: &Arc<Mutex<ExecEvents<W>>>,
    extra_definitions: &[nano_model::types::ToolDefinition],
    web_search_backed: bool,
    prompt: &str,
    routing_mode: nano_session::RoutingMode,
) -> crate::exec_mode::ExecTurnOutcome
where
    D: nano_agent::turn::ModelDriver,
    T: nano_agent::turn::ToolExecutor,
    W: Write + Send,
{
    let turn_id = format!("{}-turn-{}", session.session_id, session.turn_counter + 1);
    run_exec_turn(
        driver,
        executor,
        gate,
        model_name,
        session,
        &turn_id,
        prompt,
        context,
        journal.clone(),
        events.clone(),
        extra_definitions,
        web_search_backed,
        routing_mode,
    )
    .await
}

/// The exit path: `--output-last-message` is written ONLY on exit 0, via
/// the atomic replace-existing primitive; a failed run leaves any
/// pre-existing file untouched and never writes a partial one.
fn finish_exec<W: Write + Send>(
    exit: i32,
    output_last_message: &Option<PathBuf>,
    final_text: &str,
    events: &Arc<Mutex<ExecEvents<W>>>,
) -> i32 {
    if exit == 0
        && let Some(path) = output_last_message
        && let Err(err) = atomic_replace_write(path, final_text.as_bytes())
    {
        events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .error(&format!("cannot write --output-last-message: {err}"));
        return 2;
    }
    exit
}

/// Production entry: Flux driver + real tools, JSONL on stdout. Requires
/// the Flux key via the standard resolution chain (never embedded).
pub async fn run(nano_home: &Path, workspace: &Path, params: &ExecParams) -> i32 {
    let Some(api_key) = crate::flux_key::flux_api_key() else {
        eprintln!("wayland-nano: FLUX_API_KEY (or FLUX_API_KEY_FILE) is required for exec mode");
        return 2;
    };
    // P5 §1: exec routing — explicit `--model` pin > configured
    // NANO_DEFAULT_MODEL pin > explicit Auto opt-in (`--auto` /
    // NANO_ROUTING_AUTO) > implicit `flux-auto` passthrough. Malformed env
    // values are typed config errors (exit 2), never silent defaults. Exec
    // dispatches on the Flux wire only: namespaced references are a typed
    // usage error here (the acp-host owns the C8 namespaced surface).
    let bare_only = |value: &str, flag: &str| -> Result<String, i32> {
        match crate::provider_router::ProviderRouter::parse_model_id(value) {
            Ok(crate::provider_router::ModelRef::Flux(_)) => Ok(value.to_string()),
            _ => {
                eprintln!(
                    "wayland-nano: {flag} accepts bare Flux model ids only in exec mode, got {value:?}"
                );
                Err(2)
            }
        }
    };
    let auto_opt_in = match crate::auto_routing::parse_auto_opt_in(
        std::env::var(crate::auto_routing::AUTO_ROUTING_ENV).ok(),
    ) {
        Ok(env_value) => params.auto || env_value,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return 2;
        }
    };
    let configured_default = match crate::auto_routing::parse_configured_default(
        std::env::var(crate::auto_routing::DEFAULT_MODEL_ENV).ok(),
    ) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return 2;
        }
    };
    // S1 evidence-capture arm (NANO_AUTO_TOOLS_PROBE): same fail-closed
    // typed parse discipline — malformed is a config error, never a default.
    let tools_probe = match crate::auto_routing::parse_tools_probe(
        std::env::var(crate::auto_routing::AUTO_TOOLS_PROBE_ENV).ok(),
    ) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return 2;
        }
    };
    let (source, reference) = match (&params.model, &configured_default) {
        (Some(model), _) => match bare_only(model, "--model") {
            Ok(model) => (crate::auto_routing::ModelSource::ExplicitPin, model),
            Err(code) => return code,
        },
        (None, Some(default)) => match bare_only(default, crate::auto_routing::DEFAULT_MODEL_ENV) {
            Ok(default) => (crate::auto_routing::ModelSource::ConfiguredDefault, default),
            Err(code) => return code,
        },
        (None, None) => (
            crate::auto_routing::ModelSource::ImplicitDefault,
            crate::auto_routing::FLUX_AUTO.to_string(),
        ),
    };
    let routing = crate::exec_mode::ExecRouting {
        mode: crate::auto_routing::resolve_routing(source, &reference, auto_opt_in).mode,
        reference: reference.clone(),
        tools_probe,
    };
    let sessions_dir = nano_home.join("sessions");
    let home = nano_home.to_path_buf();
    // P3 §6.1: parse the operator MCP specs once — every configured HTTP
    // MCP server's ORIGIN joins exec's egress policy at construction (inert
    // until the dispatcher HTTP binding lands; deny-by-default unchanged).
    let mcp_specs = crate::mcp_specs::mcp_specs_from_env();
    let driver_policy = crate::mcp_specs::allow_http_mcp_origins(
        nano_egress::policy::EgressPolicy::flux_only(),
        &mcp_specs,
    );
    let ladder_policy = driver_policy.clone();
    let ladder_key = api_key.clone();
    let make_driver = move || {
        nano_agent::wiring::FluxDriver::new(
            nano_model::flux_completions::FluxCompletionsClient::new(
                nano_egress::client::EgressClient::new(driver_policy.clone()),
            ),
            api_key.clone(),
        )
    };
    // P5 §4: ladder candidates dispatch with a single-attempt retry
    // posture — every physical attempt is visible to (and countable
    // against) the global three-attempt budget.
    let make_ladder_driver = move || {
        nano_agent::wiring::FluxDriver::new(
            nano_model::flux_completions::FluxCompletionsClient::new(
                nano_egress::client::EgressClient::new(ladder_policy.clone()),
            )
            .with_retry_config(nano_model::retry::RetryConfig::single_attempt()),
            ladder_key.clone(),
        )
    };
    // P1: web_search — the key-gated ladder, resolved ONCE at host start.
    // The meter handle is Lane B's session CostMeter; the stub stands the
    // seam up (no pricing, no cap authority) until it lands.
    let search_meter: Arc<dyn nano_model::metering::UsageSink> =
        Arc::new(nano_model::metering::StubCostMeter::new());
    let search = crate::search_specs::web_search_tool_from_env(Some(search_meter.clone()));
    let search_tool = search.as_ref().map(|resolved| resolved.tool.clone());
    let tools_search = search_tool.clone();
    let tools_meter = search_meter.clone();
    let make_tools = move |workspace: &Path,
                           mode: PermissionMode|
          -> (
        nano_agent::wiring::RealToolExecutor,
        nano_core::permissions::FileSystemSandboxPolicy,
    ) {
        // C2: read_only tightens the tool-layer profile itself (defense in
        // depth); default and full_auto share the identical workspace_write
        // policy — a mode NEVER widens the sandbox.
        let profile = match mode {
            PermissionMode::ReadOnly => nano_core::permissions::PermissionProfile::read_only(),
            PermissionMode::Default | PermissionMode::FullAuto => {
                nano_core::permissions::PermissionProfile::workspace_write()
            }
        };
        let policy = profile.file_system_sandbox_policy();
        let fs = nano_tools::fs::FsTools::new(policy.clone(), workspace);
        let shell = nano_tools::shell::ShellTool::new(&home, workspace);
        let mut executor = nano_agent::wiring::RealToolExecutor::new(fs, shell, workspace);
        if let Some(fetch) = crate::fetch_specs::web_fetch_tool_from_env() {
            executor = executor.with_web_fetch(fetch);
        }
        // P1: web_search with the session meter handle (design §2.5).
        if let Some(tool) = &tools_search {
            executor = executor.with_web_search(tool.clone(), tools_meter.clone());
        }
        // P4 §5.5: the repo_map index rides the same policy + cwd; a
        // construction failure leaves the slot empty (typed denial on
        // call, never a silent skip).
        match nano_tools::repomap::RepoMapTool::new(&policy, workspace) {
            Ok(tool) => executor = executor.with_repo_map(tool),
            Err(error) => eprintln!("wayland-nano: repo_map index unavailable: {error}"),
        }
        (executor, policy)
    };
    let sandbox_available = platform_sandbox_available(nano_home);
    run_exec_with(
        &sessions_dir,
        nano_home,
        workspace,
        params,
        &routing.reference,
        make_driver,
        make_ladder_driver,
        make_tools,
        search_tool.is_some(),
        sandbox_available,
        &mcp_specs,
        &routing,
        std::io::stdout(),
    )
    .await
}

/// C2 §4 sandbox probe (same composition acp_mode and doctor report).
fn platform_sandbox_available(nano_home: &Path) -> bool {
    #[cfg(unix)]
    {
        let _ = nano_home;
        nano_sandbox::get_platform_sandbox(true).is_some()
    }
    #[cfg(windows)]
    {
        nano_sandbox::identity::sandbox_setup_is_complete(nano_home)
    }
}
