//! `wayland-nano acp-host` — ACP adapter: Desktop's stdio JSON-RPC protocol driving
//! the real turn engine. This is the zero-Desktop-change integration door.
//!
//! Live I/O design:
//! - A dedicated thread owns stdin and forwards parsed frames over a channel,
//!   so `session/cancel` and permission responses are read *while* a turn
//!   runs (the old sequential loop was deaf mid-turn).
//! - The engine's streaming sink forwards every op as an ACP `session/update`
//!   the moment it is journaled — no after-the-fact batch replay.
//! - Mutating/executing tools go through [`AcpApproval`], which emits
//!   `session/request_permission` and blocks on the client's response.
//!   Read-only tools run without prompting, matching Desktop's Default mode
//!   (read/search kinds are never confirmation-gated there). Any malformed,
//!   absent, or non-allow response denies the call (fail-closed).
//! - Every ACP session journals its ops to `<nano_home>/sessions/<id>.jsonl`;
//!   `session/load` reads that journal, replays the transcript as historical
//!   `session/update` notifications BEFORE answering, and restores the model
//!   context so the next prompt continues with full prior history. Tool
//!   outputs stay digest-only (the journal never carries payloads), so
//!   restored tool results are marked elided rather than fabricated.
//! - session/new (and session/load) advertise the Flux model catalog in the
//!   `models` block Desktop's picker consumes; session/set_model switches
//!   the session's model (validated against the advertised catalog — an
//!   unknown id is a typed error, never a silent no-op), and the next turn's
//!   model request carries the chosen id.
//! - MCP is per-session: session/new (and session/load) register the
//!   `mcpServers` param (Desktop-published connectors) plus NANO_MCP_SERVERS
//!   (operator-supplied) into a FRESH registry — a session never inherits
//!   another session's servers. Registration failures log and continue; the
//!   turn executor is the MCP-merged one (mcp__ tools are advertised to the
//!   model and routed on calls), and every mcp__ call goes through the
//!   approval gate (mutating-unknown: never auto-approved).

use nano_agent::loop_protection::TurnBudget;
use nano_agent::mcp::{McpRegistry, McpServerSpec, McpToolExecutor};
use nano_agent::turn::{
    ApprovalDecision, ApprovalGate, ModelDriver, ToolExecutor, TurnEngine, TurnState,
};
use nano_agent::wiring::{FluxDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::types::{ContentBlock, Message, Role, ToolCall};
use nano_protocol::acp::{
    AvailableModel, JsonRpcNotification, JsonRpcResponse, agent_capabilities, agent_message_chunk,
    compaction_notice, prompt_result, request_permission_request, session_load_result,
    session_new_result, set_model_result, tool_call_done, tool_call_replay, tool_call_update,
    user_message_chunk,
};
use nano_session::SessionState;
use nano_session::op::{Op, OpEnvelope};
use nano_session::reader::read_journal;
use nano_session::writer::JournalWriter;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;
use std::io::{BufRead, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Frames the stdin reader thread forwards to the main loop.
enum Inbound {
    Request {
        id: serde_json::Value,
        method: String,
        params: Option<serde_json::Value>,
    },
    Notification {
        method: String,
    },
    /// A line that is not valid JSON (the parse error text).
    Malformed(String),
}

/// Permission waiters keyed by the JSON-RPC id we sent; the reader thread
/// routes matching client responses straight to the blocked gate, so the
/// main loop never has to (it may itself be blocked inside the gate).
type PendingMap =
    Arc<Mutex<std::collections::HashMap<u64, std::sync::mpsc::Sender<serde_json::Value>>>>;

struct Session {
    id: String,
    workspace: std::path::PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Append-only journal for this ACP session (`<sessions_dir>/<id>.jsonl`).
    journal: std::path::PathBuf,
    /// Turns started in this session (restored from the journal on load), so
    /// turn ids — and therefore envelope ids — never collide across resumes.
    turn_counter: u64,
    /// Conversation context rebuilt from the journal; the next prompt starts
    /// with these messages so the model sees the full prior history.
    context: Vec<Message>,
    /// The session's current model id (set via session/set_model, validated
    /// against the advertised catalog). The next turn's model request carries
    /// exactly this id.
    model: String,
    /// This session's MCP servers (registered at session/new or session/load
    /// from the mcpServers param + NANO_MCP_SERVERS). Fresh per session;
    /// dropping it kills the stdio children, so nothing leaks across
    /// sessions. Shared with the running turn's MCP-merged executor.
    mcp: Arc<Mutex<McpRegistry>>,
}

/// ACP session ids are filesystem-safe (they name the journal file) and
/// unique per session without embedding the pid: nanosecond clock plus a
/// process-local counter.
fn new_session_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("wayland-nano-session-{nanos}-{n}")
}

/// Session ids name a journal file directly, so anything that could escape
/// the sessions directory (or not round-trip as a filename) is rejected.
fn is_fs_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub async fn run(nano_home: &std::path::Path) -> std::io::Result<i32> {
    let Some(api_key) = crate::flux_key::flux_api_key() else {
        eprintln!(
            "wayland-nano: FLUX_API_KEY (or FLUX_API_KEY_FILE) is required for acp-host mode"
        );
        return Ok(2);
    };

    let reader = std::io::BufReader::new(std::io::stdin());
    let writer = std::io::stdout();
    let home = nano_home.to_path_buf();
    let sessions = home.join("sessions");
    // The model catalog the session advertises and validates set_model
    // against: live GET /v1/models once (cached in-process), vendored
    // fixture when offline/unkeyed. If BOTH fail, advertise only the default
    // model — honest (it really runs) and keeps the host usable.
    let catalog =
        nano_model::flux_models::ModelCatalog::new(EgressClient::flux(), Some(api_key.clone()));
    // The resolved catalog doubles as the C1 context-window source
    // (`max_input_tokens` per model id; unknown models fall back to 128k).
    let (catalog_models, available): (
        Vec<nano_model::flux_models::FluxModel>,
        Vec<AvailableModel>,
    ) = match catalog.models().await {
        Ok(models) => (
            models.to_vec(),
            models
                .iter()
                .map(|m| AvailableModel {
                    id: m.id.clone(),
                    name: m.name.clone(),
                })
                .collect(),
        ),
        Err(err) => {
            eprintln!(
                "wayland-nano: model catalog unavailable ({err}); advertising the default model only"
            );
            (
                Vec::new(),
                vec![AvailableModel {
                    id: "flux-auto".into(),
                    name: "flux-auto".into(),
                }],
            )
        }
    };
    // C1 config overrides (env is the only config channel that exists).
    // Both are DOWNWARD-ONLY; a non-numeric value is a typed config error,
    // not a silent ignore.
    let window_override = match parse_env_u64("NANO_CONTEXT_WINDOW_TOKENS") {
        Ok(value) => value,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return Ok(2);
        }
    };
    let limit_override = match parse_env_u64("NANO_AUTO_COMPACT_TOKENS") {
        Ok(value) => value,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return Ok(2);
        }
    };
    // NOTE(wiring.rs): FluxDriver carries no model — the model id is a
    // per-turn TurnEngine input (model_name → ModelRequest.model), so model
    // switching needs no driver change. If a future driver needs per-model
    // client state, wiring.rs could grow a `FluxDriver::with_model` or a
    // set_model affordance; until then per-prompt model_name is the least
    // churn and keeps wiring.rs untouched.
    let make_driver = move || {
        FluxDriver::new(
            FluxCompletionsClient::new(EgressClient::flux()),
            api_key.clone(),
        )
    };
    let make_tools = move |workspace: &std::path::Path| {
        let policy = nano_core::permissions::PermissionProfile::workspace_write()
            .file_system_sandbox_policy();
        let fs = FsTools::new(policy, workspace);
        let shell = ShellTool::new(&home, workspace);
        RealToolExecutor::new(fs, shell, workspace)
    };
    // Operator-supplied MCP servers (NANO_MCP_SERVERS) merge into every
    // session alongside the mcpServers param Desktop publishes.
    let env_mcp_specs = crate::mcp_specs::mcp_specs_from_env();
    let config = ServeConfig {
        sessions_dir: &sessions,
        default_model: "flux-auto",
        available_models: &available,
        env_mcp_specs: &env_mcp_specs,
        catalog: &catalog_models,
        window_override,
        limit_override,
    };
    serve(reader, writer, &config, make_driver, make_tools).await
}

/// Parses an optional numeric env override. Unset/empty is `None`; a value
/// that is not a positive integer is a typed config error (fail-closed).
fn parse_env_u64(name: &str) -> Result<Option<u64>, String> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => raw
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("{name} must be a positive integer, got {raw:?}")),
        _ => Ok(None),
    }
}

/// The ACP host loop, generic over the byte streams and the model/tool
/// factories so integration tests can drive it in-process with scripted
/// Configuration bundle for `serve` — keeps the signature under the clippy
/// argument ceiling as capabilities grow.
pub struct ServeConfig<'a> {
    /// Journal root: each ACP session appends to `<sessions_dir>/<id>.jsonl`.
    pub sessions_dir: &'a std::path::Path,
    /// Model a session starts on.
    pub default_model: &'a str,
    /// Catalog advertised in session/new (and session/load) and the set
    /// session/set_model validates against.
    pub available_models: &'a [AvailableModel],
    /// Operator-supplied MCP servers (NANO_MCP_SERVERS in production) merged
    /// into every session's fresh registry.
    pub env_mcp_specs: &'a [McpServerSpec],
    /// Resolved Flux catalog (C1 context-window source); may be empty when
    /// the catalog was unavailable, in which case every model gets the 128k
    /// conservative default.
    pub catalog: &'a [nano_model::flux_models::FluxModel],
    /// NANO_CONTEXT_WINDOW_TOKENS — downward-only override.
    pub window_override: Option<u64>,
    /// NANO_AUTO_COMPACT_TOKENS — downward-only override.
    pub limit_override: Option<u64>,
}

/// `make_driver`/`make_tools` build a fresh pair per prompt (tools are
/// anchored to the session workspace).
pub async fn serve<R, W, FD, FT, D, T>(
    reader: R,
    writer: W,
    config: &ServeConfig<'_>,
    make_driver: FD,
    make_tools: FT,
) -> std::io::Result<i32>
where
    R: BufRead + Send + 'static,
    W: Write + Send,
    FD: Fn() -> D,
    FT: Fn(&std::path::Path) -> T,
    D: ModelDriver,
    T: ToolExecutor,
{
    let out = Arc::new(Mutex::new(writer));
    let pending: PendingMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
    // The active session's cancel flag, shared with the reader thread so a
    // session/cancel lands IMMEDIATELY — the main loop cannot relay it while
    // the turn future is mid-poll (tool execution runs synchronously).
    let current_cancel: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
        Arc::new(Mutex::new(None));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Inbound>();
    std::thread::spawn({
        let pending = pending.clone();
        let current_cancel = current_cancel.clone();
        move || reader_loop(reader, tx, pending, current_cancel)
    });

    let mut session: Option<Session> = None;
    let permission_ids = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let mut stdin_open = true;
    #[allow(clippy::type_complexity)]
    let mut turn: Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = (String, String)> + '_>>,
    > = None;
    let mut prompt_id: Option<serde_json::Value> = None;
    let mut turn_session = String::new();

    loop {
        if !stdin_open && turn.is_none() {
            return Ok(0); // stdin closed and no turn in flight: clean exit
        }
        // Biased: inbound frames (cancel, responses) are handled before the
        // turn makes progress whenever both are ready, so a cancel that
        // landed mid-turn is seen at the engine's very next flag check.
        tokio::select! {
            biased;
            inbound = rx.recv(), if stdin_open => {
                let Some(inbound) = inbound else {
                    stdin_open = false;
                    continue;
                };
                match inbound {
                    Inbound::Malformed(err) => {
                        write_out(
                            &out,
                            &JsonRpcResponse::err(
                                serde_json::Value::Null,
                                -32700,
                                format!("parse error: {err}"),
                            ),
                        )?;
                    }
                    Inbound::Notification { method } => {
                        if method == "session/cancel" {
                            // Step-boundary cancel: the engine checks the flag
                            // between steps and the approval gate polls it while
                            // waiting on a permission response.
                            if let Some(active) = &session {
                                active.cancel.store(true, Ordering::SeqCst);
                            }
                        }
                    }
                    Inbound::Request { id, method, params } => match method.as_str() {
                        "initialize" => {
                            write_out(&out, &JsonRpcResponse::ok(id, agent_capabilities()))?;
                        }
                        "authenticate" => {
                            write_out(
                                &out,
                                &JsonRpcResponse::err(
                                    id,
                                    -32602,
                                    "wayland-nano uses FLUX_API_KEY from the environment; no interactive auth",
                                ),
                            )?;
                        }
                        "session/new" => {
                            if turn.is_some() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32602, "turn in progress"),
                                )?;
                                continue;
                            }
                            let params = params.unwrap_or_default();
                            let cwd = params
                                .get("cwd")
                                .and_then(|c| c.as_str())
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                            let session_id = new_session_id();
                            let journal = config.sessions_dir.join(format!("{session_id}.jsonl"));
                            // Fail closed: a session we cannot journal is a
                            // session we could not honestly resume later.
                            let journaled = JournalWriter::open(&journal).and_then(|mut w| {
                                w.append(&OpEnvelope::new(
                                    format!("{session_id}-begin-1"),
                                    "now",
                                    Op::SessionBegin {
                                        session_id: session_id.clone(),
                                        cwd: cwd.display().to_string(),
                                    },
                                ))
                            });
                            if let Err(err) = journaled {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32603,
                                        format!("cannot open session journal: {err}"),
                                    ),
                                )?;
                                continue;
                            }
                            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            *current_cancel.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(cancel.clone());
                            let mcp = session_mcp_registry(&params, config.env_mcp_specs);
                            session = Some(Session {
                                id: session_id.clone(),
                                workspace: cwd,
                                cancel,
                                journal,
                                turn_counter: 0,
                                context: Vec::new(),
                                model: config.default_model.to_string(),
                                mcp,
                            });
                            write_out(
                                &out,
                                &JsonRpcResponse::ok(
                                    id,
                                    session_new_result(
                                        &session_id,
                                        config.default_model,
                                        config.available_models,
                                    ),
                                ),
                            )?;
                        }
                        "session/load" => {
                            if turn.is_some() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32602, "turn in progress"),
                                )?;
                                continue;
                            }
                            let params = params.unwrap_or_default();
                            let Some(session_id) =
                                params.get("sessionId").and_then(|s| s.as_str())
                            else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        "session/load requires a sessionId",
                                    ),
                                )?;
                                continue;
                            };
                            if !is_fs_safe_session_id(session_id) {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32602, "invalid session id"),
                                )?;
                                continue;
                            }
                            let cwd = params
                                .get("cwd")
                                .and_then(|c| c.as_str())
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                            let journal = config.sessions_dir.join(format!("{session_id}.jsonl"));
                            // Unknown session: typed error, no fallback theatre —
                            // Desktop catches this and self-heals via session/new
                            // (AcpConnection.ts `resumeSession`).
                            if !journal.exists() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        format!("session not found: {session_id}"),
                                    ),
                                )?;
                                continue;
                            }
                            // A corrupt journal fails loudly (never silently
                            // resumes from a partial or forged history).
                            let report = match read_journal(&journal) {
                                Ok(report) => report,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32603,
                                            format!("session journal unreadable: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            // Replay the transcript BEFORE the response: Desktop
                            // holds its bootstrap gate until session/load
                            // resolves, so these historical updates are not
                            // re-persisted as fresh rows.
                            for notification in replay_frames(session_id, &report.envelopes) {
                                write_out(&out, &notification)?;
                            }
                            let turn_counter = report
                                .envelopes
                                .iter()
                                .filter(|e| matches!(e.op, Op::TurnBegin { .. }))
                                .count() as u64;
                            let begin_count = report
                                .envelopes
                                .iter()
                                .filter(|e| matches!(e.op, Op::SessionBegin { .. }))
                                .count();
                            // A fresh SessionBegin marks the resume in the
                            // journal itself (audit trail, and it refreshes cwd).
                            if let Ok(mut writer) = JournalWriter::open(&journal) {
                                let _ = writer.append(&OpEnvelope::new(
                                    format!("{session_id}-begin-{}", begin_count + 1),
                                    "now",
                                    Op::SessionBegin {
                                        session_id: session_id.to_string(),
                                        cwd: cwd.display().to_string(),
                                    },
                                ));
                            }
                            let context = messages_from_envelopes(&report.envelopes);
                            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            *current_cancel.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(cancel.clone());
                            let mcp = session_mcp_registry(&params, config.env_mcp_specs);
                            session = Some(Session {
                                id: session_id.to_string(),
                                workspace: cwd,
                                cancel,
                                journal,
                                turn_counter,
                                context,
                                // The journal does not persist the model pick,
                                // so a resumed session restarts on the default.
                                // Follow-up: journal an Op::SetModel so a resume
                                // restores the user's choice.
                                model: config.default_model.to_string(),
                                mcp,
                            });
                            write_out(
                                &out,
                                &JsonRpcResponse::ok(
                                    id,
                                    session_load_result(config.default_model, config.available_models),
                                ),
                            )?;
                        }
                        // Desktop's model picker sends session/set_model
                        // (AcpConnection.ts `setModel`, params {sessionId,
                        // modelId}); session/set_mode is the mode analogue and
                        // only the advertised "default" mode exists.
                        "session/set_model" => {
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        "no session: call session/new first",
                                    ),
                                )?;
                                continue;
                            };
                            let params = params.unwrap_or_default();
                            let model_id = params
                                .get("modelId")
                                .and_then(|m| m.as_str())
                                .unwrap_or("");
                            if model_id.is_empty() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        "session/set_model requires a modelId",
                                    ),
                                )?;
                                continue;
                            }
                            // Fail closed: only ids from the advertised catalog
                            // are routable; anything else is a typed error
                            // (Desktop greps the message for model_not_found).
                            if !config.available_models.iter().any(|m| m.id == model_id) {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        format!(
                                            "model_not_found: {model_id} is not in the advertised catalog"
                                        ),
                                    ),
                                )?;
                                continue;
                            }
                            active.model = model_id.to_string();
                            write_out(
                                &out,
                                &JsonRpcResponse::ok(
                                    id,
                                    set_model_result(model_id, config.available_models),
                                ),
                            )?;
                        }
                        "session/set_mode" => {
                            if session.is_none() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        "no session: call session/new first",
                                    ),
                                )?;
                                continue;
                            }
                            let params = params.unwrap_or_default();
                            let mode_id = params
                                .get("modeId")
                                .and_then(|m| m.as_str())
                                .unwrap_or("");
                            if mode_id != "default" {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        format!("unknown mode: {mode_id}"),
                                    ),
                                )?;
                                continue;
                            }
                            write_out(&out, &JsonRpcResponse::ok(id, serde_json::json!({})))?;
                        }
                        "session/prompt" => {
                            if turn.is_some() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32602, "a turn is already running"),
                                )?;
                                continue;
                            }
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32602, "no session: call session/new first"),
                                )?;
                                continue;
                            };
                            let params = params.unwrap_or_default();
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
                            // A fresh prompt starts un-cancelled; a cancel that
                            // landed between turns must not poison this one.
                            active.cancel.store(false, Ordering::SeqCst);
                            active.turn_counter += 1;
                            let turn_id = format!("{}-turn-{}", active.id, active.turn_counter);
                            let prior_context = active.context.clone();
                            // C1: resolve this turn's context-management
                            // config against the ACTIVE model's catalog
                            // window. Overrides are downward-only; an
                            // override that exceeds the active model's window
                            // is a typed error, never a silent clamp.
                            let catalog_window = nano_model::flux_common::context_window_for(
                                &active.model,
                                config.catalog,
                            );
                            let compaction = match nano_agent::compact::resolve_compaction_config(
                                catalog_window,
                                config.window_override,
                                config.limit_override,
                            ) {
                                Ok(config) => config,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32602,
                                            format!("compaction config: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            // Fail closed: a turn we cannot journal is a turn
                            // that would silently break a later resume.
                            let journal_writer = match JournalWriter::open(&active.journal) {
                                Ok(writer) => writer,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32603,
                                            format!("cannot open session journal: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            prompt_id = Some(id);
                            turn_session = active.id.clone();
                            let session_id = active.id.clone();
                            let workspace = active.workspace.clone();
                            let cancel = active.cancel.clone();
                            // The session's current model (set via
                            // session/set_model) is captured NOW: the whole
                            // turn runs on it, and a later switch only takes
                            // effect on the next prompt.
                            let turn_model = active.model.clone();
                            // The session's MCP registry: the turn executor
                            // routes mcp__ calls through it (and advertises
                            // its tools to the model) without taking ownership.
                            let turn_mcp = active.mcp.clone();
                            // The turn future must own its handles: clone the
                            // loop-invariant Arcs before the `async move`.
                            let gate_out = out.clone();
                            let gate_pending = pending.clone();
                            let gate_ids = permission_ids.clone();
                            let sink_out = out.clone();
                            let make_driver = &make_driver;
                            let make_tools = &make_tools;
                            let turn_future = async move {
                                let driver = make_driver();
                                let tools = make_tools(&workspace);
                                // MCP-merged executor: mcp__ names route to the
                                // session registry, everything else to the core
                                // tools; the model sees both tool sets.
                                let executor = McpToolExecutor::from_shared(turn_mcp, &tools);
                                let mut tool_definitions = v1_tool_definitions();
                                tool_definitions
                                    .extend(executor.tool_definitions_from_registry());
                                let gate = AcpApproval {
                                    session_id: session_id.clone(),
                                    out: gate_out,
                                    pending: gate_pending,
                                    next_id: gate_ids,
                                    cancel: cancel.clone(),
                                };
                                let engine = TurnEngine {
                                    model: &driver,
                                    tools: &executor,
                                    budget: TurnBudget::default(),
                                    model_name: turn_model,
                                    tool_definitions,
                                    approval: Some(&gate),
                                    compaction: Some(compaction),
                                };
                                let sink_session = session_id.clone();
                                let mut journal_writer = journal_writer;
                                let mut sink = move |envelope: &OpEnvelope| -> bool {
                                    // Journal first: the durable record leads
                                    // the live frame, never the other way. The
                                    // compaction commit protocol reads this
                                    // return value — its in-memory swap only
                                    // happens behind a durable Complete.
                                    let journaled = match journal_writer.append(envelope) {
                                        Ok(_) => true,
                                        Err(err) => {
                                            eprintln!(
                                                "wayland-nano: session journal append failed: {err}"
                                            );
                                            false
                                        }
                                    };
                                    let mut guard =
                                        sink_out.lock().unwrap_or_else(|p| p.into_inner());
                                    let _ =
                                        write_op_frame(&mut *guard, &sink_session, envelope);
                                    journaled
                                };
                                let result = engine
                                    .run_turn_streaming_with_context(
                                        &turn_id,
                                        &text,
                                        prior_context,
                                        Some(cancel.as_ref()),
                                        &mut sink,
                                    )
                                    .await;
                                let stop_reason = match result.state {
                                    TurnState::Complete => "end_turn",
                                    TurnState::Stopped(_) => "cancelled",
                                    _ => "error",
                                };
                                (result.final_text, stop_reason.to_string())
                            };
                            turn = Some(Box::pin(turn_future));
                        }
                        // Manual /compact (C1 §7): the SAME compaction
                        // pipeline as the auto path, engine-side, journaled
                        // identically (Begin/Complete + notices), never
                        // counted toward the auto-compaction loop guard.
                        // NOTE(deviation from the design doc's transport
                        // rationale): the doc assumed the TUI links an
                        // in-process acp-host; it does not (subprocess +
                        // JSON-RPC, and nano-tui links no engine crates), so
                        // the command rides a `session/compact` request
                        // exactly parallel to session/set_model.
                        "session/compact" => {
                            if turn.is_some() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32602, "turn in progress"),
                                )?;
                                continue;
                            }
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        "no session: call session/new first",
                                    ),
                                )?;
                                continue;
                            };
                            // Fail closed: a compaction we cannot journal must
                            // never swap the in-memory history.
                            let mut writer = match JournalWriter::open(&active.journal) {
                                Ok(writer) => writer,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32603,
                                            format!("cannot open session journal: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            let report = match read_journal(&active.journal) {
                                Ok(report) => report,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32603,
                                            format!("session journal unreadable: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            let sequence = report
                                .envelopes
                                .iter()
                                .filter(|e| matches!(e.op, Op::CompactionBegin { .. }))
                                .count()
                                + 1;
                            let compaction_id = format!("{}-compact-{sequence}", active.id);
                            let covers_op_ids =
                                report.envelopes.iter().map(|e| e.id.clone()).collect();
                            let changed_files: Vec<String> =
                                SessionState::fold(&report.envelopes)
                                    .changed_files
                                    .into_iter()
                                    .collect();
                            let driver = make_driver();
                            let model_name = active.model.clone();
                            let session_id = active.id.clone();
                            let mut context = std::mem::take(&mut active.context);
                            let mut op_sequence = 0u32;
                            let notice_id = compaction_id.clone();
                            let notice_session = session_id.clone();
                            let notice_out = out.clone();
                            let mut emit = |op: Op| -> bool {
                                op_sequence += 1;
                                let status = match &op {
                                    Op::CompactionBegin { .. } => Some("begin"),
                                    Op::CompactionComplete { .. } => Some("complete"),
                                    Op::CompactionCancel { .. } => Some("cancel"),
                                    _ => None,
                                };
                                let envelope = OpEnvelope::new(
                                    format!("{notice_id}-op-{op_sequence}"),
                                    "now",
                                    op,
                                );
                                let journaled = match writer.append(&envelope) {
                                    Ok(_) => true,
                                    Err(err) => {
                                        eprintln!(
                                            "wayland-nano: session journal append failed: {err}"
                                        );
                                        false
                                    }
                                };
                                if let Some(status) = status {
                                    let mut guard = notice_out
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner());
                                    let _ = write_json(
                                        &mut *guard,
                                        &compaction_notice(&notice_session, status),
                                    );
                                }
                                journaled
                            };
                            let outcome = nano_agent::compact::compact_messages(
                                &driver,
                                &model_name,
                                &mut context,
                                &compaction_id,
                                covers_op_ids,
                                changed_files,
                                &mut emit,
                            )
                            .await;
                            // On failure the context came back untouched
                            // (compact_messages never swaps without a durable
                            // Complete); on success it IS the compacted one.
                            active.context = context;
                            match outcome {
                                Ok(()) => write_out(
                                    &out,
                                    &JsonRpcResponse::ok(
                                        id,
                                        serde_json::json!({ "compacted": true }),
                                    ),
                                )?,
                                Err(err) => write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32603,
                                        format!("compaction failed: {err}"),
                                    ),
                                )?,
                            }
                        }
                        other => {
                            write_out(&out, &JsonRpcResponse::method_not_found(id, other))?;
                        }
                    },
                }
            }
            // select! evaluates the branch expression even when the
            // precondition disables it — so this must not panic on None.
            outcome = async {
                match turn.as_mut() {
                    Some(active) => active.await,
                    None => std::future::pending::<(String, String)>().await,
                }
            }, if turn.is_some() => {
                turn = None;
                let (final_text, stop_reason) = outcome;
                // Fold the just-finished turn into the session's context so
                // the NEXT prompt continues the conversation (same rebuild
                // path session/load uses — one honest code path).
                if let Some(active) = session.as_mut()
                    && active.id == turn_session
                {
                    match read_journal(&active.journal) {
                        Ok(report) => {
                            active.context = messages_from_envelopes(&report.envelopes);
                        }
                        Err(err) => {
                            eprintln!("wayland-nano: session journal re-read failed: {err}");
                        }
                    }
                }
                // A cancel that landed during the turn's final stretch (after
                // the engine's last flag check) still answers cancelled.
                let cancel_fired = session
                    .as_ref()
                    .is_some_and(|s| s.cancel.load(Ordering::SeqCst));
                let stop_reason = if cancel_fired { "cancelled".to_string() } else { stop_reason };
                if !final_text.is_empty() {
                    write_out(&out, &agent_message_chunk(&turn_session, &final_text))?;
                }
                if let Some(id) = prompt_id.take() {
                    write_out(&out, &JsonRpcResponse::ok(id, prompt_result(&stop_reason)))?;
                }
            }
        }
    }
}

/// Builds one session's MCP registry: a FRESH registry per session/new or
/// session/load (a session never inherits another session's servers, and the
/// dropped registry kills its stdio children), merging operator-supplied
/// NANO_MCP_SERVERS specs with the Desktop-published mcpServers param.
/// Registration failures log to stderr and continue — the session proceeds
/// with whatever registered (possibly only core tools).
fn session_mcp_registry(
    params: &serde_json::Value,
    env_mcp_specs: &[McpServerSpec],
) -> Arc<Mutex<McpRegistry>> {
    let specs = env_mcp_specs
        .iter()
        .cloned()
        .chain(crate::mcp_specs::mcp_specs_from_acp_params(params));
    Arc::new(Mutex::new(crate::mcp_specs::register_all(specs)))
}

/// Owns stdin on its own thread: parses each line and forwards it. Client
/// *responses* (id, no method) that match a pending permission request are
/// delivered straight to the waiting gate; everything else goes to the main
/// loop. EOF or a dead receiver ends the thread.
fn reader_loop<R: BufRead>(
    mut reader: R,
    tx: tokio::sync::mpsc::UnboundedSender<Inbound>,
    pending: PendingMap,
    current_cancel: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(err) => {
                if tx.send(Inbound::Malformed(err.to_string())).is_err() {
                    break;
                }
                continue;
            }
        };
        let method = value
            .get("method")
            .and_then(|m| m.as_str())
            .map(str::to_string);
        let id = value.get("id").cloned().filter(|i| !i.is_null());
        let inbound = match (method, id) {
            (Some(method), Some(id)) => Inbound::Request {
                id,
                method,
                params: value.get("params").cloned(),
            },
            (Some(method), None) => {
                if method == "session/cancel" {
                    // Fire the flag right here: the main loop may be mid-poll
                    // inside the turn and unable to relay it for whole steps.
                    if let Some(flag) = current_cancel
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                    {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
                Inbound::Notification { method }
            }
            (None, Some(id)) => {
                if let Some(key) = id.as_u64() {
                    let waiter = pending
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .remove(&key);
                    if let Some(waiter) = waiter {
                        let _ = waiter.send(value);
                    }
                }
                // Unsolicited or unknown-id responses carry nothing we need.
                continue;
            }
            (None, None) => continue,
        };
        if tx.send(inbound).is_err() {
            break;
        }
    }
}

/// Approval gate that asks the ACP host before any mutating or executing
/// tool runs. Emits `session/request_permission` and blocks on the client
/// response, which the reader thread routes here by JSON-RPC id. Denies on
/// rejection, malformed responses, disconnect, or cancel (fail-closed).
struct AcpApproval<W: Write> {
    session_id: String,
    out: Arc<Mutex<W>>,
    pending: PendingMap,
    next_id: Arc<std::sync::atomic::AtomicU64>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl<W: Write> std::fmt::Debug for AcpApproval<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpApproval")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> ApprovalGate for AcpApproval<W> {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision {
        if is_read_only_tool(&call.name) {
            return ApprovalDecision::Approve;
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        let request =
            request_permission_request(id, &self.session_id, &call.id, &call.name, &call.arguments);
        if write_out(&self.out, &request).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return ApprovalDecision::Deny; // cannot even ask: fail closed
        }
        let decision = loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(response) => break decision_from_response(&response),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.cancel.load(Ordering::SeqCst) {
                        break ApprovalDecision::Deny;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break ApprovalDecision::Deny;
                }
            }
        };
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        decision
    }
}

/// Read-only tools run without a Desktop prompt, matching Desktop's Default
/// mode (its trusted/accept-edits auto-approve sets cover read/search, and
/// plain read activity is never confirmation-gated). Anything that mutates
/// (`fs_write`/`fs_edit`) or executes (`shell`) must ask — and so must every
/// `mcp__*` call: MCP tools are mutating-unknown, so they never match the
/// read-only prefixes and always go through the permission gate.
fn is_read_only_tool(name: &str) -> bool {
    name.starts_with("fs_read") || name.starts_with("search") || name.starts_with("glob")
}

/// Interprets a `session/request_permission` response. Approves only an
/// explicit `selected` outcome naming an `allow*` option — Desktop's own
/// resolver maps user choice to exactly this shape (`AcpConnection.ts`
/// `handlePermissionRequest`). Everything else denies.
fn decision_from_response(value: &serde_json::Value) -> ApprovalDecision {
    let outcome = value.get("result").and_then(|r| r.get("outcome"));
    let Some(outcome) = outcome else {
        return ApprovalDecision::Deny;
    };
    if outcome.get("outcome").and_then(|o| o.as_str()) != Some("selected") {
        return ApprovalDecision::Deny;
    }
    match outcome.get("optionId").and_then(|o| o.as_str()) {
        Some(option) if option.starts_with("allow") => ApprovalDecision::Approve,
        _ => ApprovalDecision::Deny,
    }
}

/// Builds the `session/load` replay: one notification per historical beat, in
/// journal order — user chunks (Desktop ignores them; real agents emit them),
/// tool cards carrying their FINAL status (plus the trailing update with the
/// output digest), and assistant text chunks. A call whose ToolResult never
/// journaled (crash mid-call) is replayed as failed so no card hangs
/// `in_progress` forever.
fn replay_frames(session_id: &str, envelopes: &[OpEnvelope]) -> Vec<JsonRpcNotification> {
    let results: std::collections::HashMap<&str, (bool, &str)> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.op {
            Op::ToolResult {
                call_id,
                ok,
                output_digest,
                ..
            } => Some((call_id.as_str(), (*ok, output_digest.as_str()))),
            _ => None,
        })
        .collect();
    let mut frames = Vec::new();
    for envelope in envelopes {
        match &envelope.op {
            Op::TurnBegin { input, .. } => {
                frames.push(user_message_chunk(session_id, input));
            }
            Op::AssistantText { text, .. } => {
                frames.push(agent_message_chunk(session_id, text));
            }
            Op::ToolCall {
                call_id,
                name,
                args,
                ..
            } => match results.get(call_id.as_str()) {
                Some((ok, digest)) => {
                    frames.push(tool_call_replay(session_id, call_id, name, args, *ok));
                    frames.push(tool_call_done(session_id, call_id, *ok, digest));
                }
                None => {
                    frames.push(tool_call_replay(session_id, call_id, name, args, false));
                }
            },
            // Historical compaction events surface as the same system notices
            // the live path emits (C1 §7).
            Op::CompactionComplete { .. } => {
                frames.push(compaction_notice(session_id, "complete"));
            }
            Op::CompactionCancel { .. } => {
                frames.push(compaction_notice(session_id, "cancel"));
            }
            _ => {}
        }
    }
    frames
}

/// Rebuilds model-consumable conversation context from journaled ops. Tool
/// payloads are NOT persisted (digest-only journals), so a restored tool
/// result carries an explicit elision marker instead of the original output:
/// the model sees that the call happened and whether it succeeded, never a
/// fabricated payload.
/// Rebuilds model-consumable conversation context from journaled ops. Tool
/// payloads are NOT persisted (digest-only journals), so a restored tool
/// result carries an explicit elision marker instead of the original output:
/// the model sees that the call happened and whether it succeeded, never a
/// fabricated payload. `CompactionComplete` folds through the canonical
/// builder (C1 §6), so a resumed context is byte-identical to the live
/// post-compaction one over the compacted prefix. Pub for the C1 replay /
/// fault-injection tests; not part of the wire surface.
pub fn messages_from_envelopes(envelopes: &[OpEnvelope]) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut assistant: Vec<ContentBlock> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let flush_assistant = |messages: &mut Vec<Message>, assistant: &mut Vec<ContentBlock>| {
        if !assistant.is_empty() {
            messages.push(Message {
                role: Role::Assistant,
                content: std::mem::take(assistant),
            });
        }
    };
    for envelope in envelopes {
        if !seen.insert(&envelope.id) {
            continue; // idempotent fold: duplicate ids never double-apply
        }
        match &envelope.op {
            Op::TurnBegin { input, .. } => {
                flush_assistant(&mut messages, &mut assistant);
                messages.push(Message::user(input.clone()));
            }
            Op::AssistantText { text, .. } => {
                assistant.push(ContentBlock::Text { text: text.clone() });
            }
            Op::ToolCall {
                call_id,
                name,
                args,
                ..
            } => {
                assistant.push(ContentBlock::ToolUse {
                    id: call_id.clone(),
                    name: name.clone(),
                    input: args.clone(),
                });
            }
            Op::ToolResult {
                call_id,
                ok,
                output_digest,
                ..
            } => {
                flush_assistant(&mut messages, &mut assistant);
                // [R1] The ONE synthesized-result encoding, shared with the
                // compaction repair pass and repeat-protection skips.
                messages.push(Message::tool_result(
                    call_id,
                    format!("[tool output elided from journal: ok={ok}, digest={output_digest}]"),
                    !ok,
                ));
            }
            Op::TurnEnd { .. } => flush_assistant(&mut messages, &mut assistant),
            // [R1/R2] The canonical replay arm: fold the SAME
            // build_compacted_history the live path used, with the journaled
            // summary; later envelopes append after it. The pending-assistant
            // flush BEFORE the builder call is mandatory — without it the
            // builder sees a history missing the in-flight assistant
            // message. covers_op_ids is audit metadata only; the builder's
            // own rules decide what survives, identically live and on replay.
            // Forged/malformed compaction ops are tolerated: no panics, and
            // real user messages survive the builder by construction.
            Op::CompactionComplete { summary, .. } => {
                flush_assistant(&mut messages, &mut assistant);
                messages = nano_agent::compact::build_compacted_history(
                    std::mem::take(&mut messages),
                    summary,
                );
            }
            _ => {}
        }
    }
    flush_assistant(&mut messages, &mut assistant);
    messages
}

/// Writes the ACP `session/update` frame for one journaled op, live.
fn write_op_frame<W: Write>(
    writer: &mut W,
    session_id: &str,
    envelope: &OpEnvelope,
) -> std::io::Result<()> {
    match &envelope.op {
        Op::ToolCall {
            call_id,
            name,
            args,
            ..
        } => write_json(writer, &tool_call_update(session_id, call_id, name, args)),
        Op::ToolResult {
            call_id,
            ok,
            output_digest,
            ..
        } => write_json(
            writer,
            &tool_call_done(session_id, call_id, *ok, output_digest),
        ),
        // C1 §7: compaction lifecycle events surface as session/update
        // notices so both UIs can render the event in the transcript.
        Op::CompactionBegin { .. } => write_json(writer, &compaction_notice(session_id, "begin")),
        Op::CompactionComplete { .. } => {
            write_json(writer, &compaction_notice(session_id, "complete"))
        }
        Op::CompactionCancel { .. } => write_json(writer, &compaction_notice(session_id, "cancel")),
        _ => Ok(()),
    }
}

fn write_out<W: Write, T: serde::Serialize>(out: &Mutex<W>, value: &T) -> std::io::Result<()> {
    let mut guard = out.lock().unwrap_or_else(|p| p.into_inner());
    write_json(&mut *guard, value)
}

fn write_json<W: Write, T: serde::Serialize>(writer: &mut W, value: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).unwrap_or_default();
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}
