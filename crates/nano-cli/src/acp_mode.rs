//! `wayland-nano acp-host` — ACP adapter: Desktop's stdio JSON-RPC protocol driving
//! the real turn engine. This is the zero-Desktop-change integration door.
//!
//! Live I/O design:
//! - A dedicated thread owns stdin and forwards parsed frames over a channel,
//!   so `session/cancel` and permission responses are read *while* a turn
//!   runs (the old sequential loop was deaf mid-turn); a `session/set_mode`
//!   DE-escalation is likewise relayed straight into the session's shared
//!   mode cell so a turn parked at a permission prompt tightens on its very
//!   next approval check (F-C2-1 — escalations are never relayed).
//! - The engine's streaming sink forwards every op as an ACP `session/update`
//!   the moment it is journaled — no after-the-fact batch replay.
//! - Mutating/executing tools go through [`AcpApproval`], whose behavior is
//!   parameterized by the session's permission mode (C2: `read_only` denies
//!   mutations at the gate, `default` prompts the host as before,
//!   `full_auto` auto-approves only provably-contained writes and
//!   sandboxed shell). The prompt path emits `session/request_permission`
//!   and blocks on the client's response. Read-only tools run without
//!   prompting in every mode. Any malformed, absent, or non-allow response
//!   denies the call (fail-closed).
//! - Every ACP session journals its ops to `<nano_home>/sessions/<id>.jsonl`;
//!   `session/load` reads that journal, replays the transcript as historical
//!   `session/update` notifications BEFORE answering, and restores the model
//!   context so the next prompt continues with full prior history. Tool
//!   outputs stay digest-only (the journal never carries payloads), so
//!   restored tool results are marked elided rather than fabricated.
//! - session/new (and session/load) advertise the Flux model catalog PLUS
//!   the validated WAYLAND_NANO_PROVIDERS payload's `<provider>:<model>`
//!   namespaced ids in the `models` block Desktop's picker consumes (C8);
//!   session/set_model switches the session's model (validated against the
//!   advertised catalog — an unknown id is a typed error, never a silent
//!   no-op — and re-resolves the provider credential: missing key → typed
//!   provider_key_missing, expired injected bearer → retryable
//!   oauth_expired, unproven arm → provider_unproven), and the next turn's
//!   model request carries the chosen id's bare model segment.
//! - C8 startup semantics (B2): acp-host starts iff AT LEAST ONE advertised
//!   provider has a usable credential (Flux's three-source order, a payload
//!   provider's env/file key, or an unexpired injected OAuth bearer); the
//!   initial binding is deterministic (Flux first, else catalog order).
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
    ApprovalDecision, ApprovalGate, ModelDriver, ToolExecutor, TurnEngine, TurnState, TypedError,
};
use nano_agent::wiring::{ProviderDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::anthropic_messages::AnthropicMessagesClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::provider_catalog::WireKind;
use nano_model::types::{ContentBlock, Message, Role, ToolCall};
use nano_protocol::acp::{
    AvailableModel, JsonRpcNotification, JsonRpcResponse, NanoErrorExtras, PLAN_MODE_ID,
    agent_capabilities, agent_message_chunk, compaction_notice, error_presentation, prompt_result,
    request_permission_request, request_question_request, session_load_result, session_modes_value,
    session_new_result, set_model_result, tool_call_diff, tool_call_done, tool_call_replay,
    tool_call_update, user_message_chunk,
};
use nano_protocol::permission_mode::PermissionMode;
use nano_session::NanoErrorKind;
use nano_session::SessionState;
use nano_session::op::{Op, OpEnvelope, TodoItem};
use nano_session::reader::read_journal;
use nano_session::writer::JournalWriter;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;
use std::io::{BufRead, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Frames the stdin reader thread forwards to the main loop.
#[derive(Debug)]
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

/// The active session's shared mode cell, exposed to the reader thread for
/// the F-C2-1 mid-park de-escalation relay (see `reader_loop`).
type CurrentMode = Arc<Mutex<Option<Arc<Mutex<PermissionMode>>>>>;

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
    /// The session's permission mode (C2) — a SHARED cell, not a plain
    /// field, so a mid-turn session/set_mode de-escalation is observed by
    /// the running turn's very next approval check (the gate computes
    /// min(captured, current) per approval). Per-session, in-memory, never
    /// persisted: every session starts in `default`; ModeSet journal ops
    /// are audit history only and are never restored on session/load.
    mode: Arc<Mutex<PermissionMode>>,
    /// The session's plan posture (C10 §3) — a SHARED cell like the mode
    /// cell, so a tool-driven mid-turn entry is observed by the running
    /// turn's very next approval check. Orthogonal to the privilege mode:
    /// entering/exiting plan never alters it. Journaled for audit
    /// (Op::PlanSet), never restored on session/load — content replays,
    /// postures don't.
    plan: Arc<Mutex<crate::session_tools::PlanPosture>>,
    /// The session's todo list (C10 §2): journaled content (Op::TodoSet),
    /// restored from the journal on session/load.
    todos: Arc<Mutex<Vec<TodoItem>>>,
    /// Monotonic per-lifetime counter for ModeSet/PlanSet op ids (the id
    /// also carries nanoseconds, so resumes never collide).
    mode_changes: u64,
    /// This session's MCP servers (registered at session/new or session/load
    /// from the mcpServers param + NANO_MCP_SERVERS). Fresh per session;
    /// dropping it kills the stdio children, so nothing leaks across
    /// sessions. Shared with the running turn's MCP-merged executor.
    mcp: Arc<Mutex<McpRegistry>>,
    /// C6: this session's background-task registry. Fresh per session; its
    /// Drop tears every child down (bounded), and the reader thread holds a
    /// handle so session/cancel cascades to children mid-poll.
    tasks: Arc<nano_agent::tasks::TaskRegistry>,
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

/// Wall-clock seconds for bearer-expiry checks (§7).
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub async fn run(nano_home: &std::path::Path) -> std::io::Result<i32> {
    let env_reader = |name: &str| std::env::var(name).ok();
    let now = unix_now_secs();
    // C8 §3: Flux's three-source order first (back-compat), then the
    // validated payload providers (env > file > injected bearer).
    let flux_key = crate::flux_key::flux_api_key();
    let (router, payload_diag) = crate::provider_router::ProviderRouter::from_env();
    if let Some(diag) = payload_diag {
        // payload_invalid: diagnostic-only, never suppresses an otherwise
        // usable Flux startup (codex r2) — and secret-free by construction.
        eprintln!("wayland-nano: {diag}");
    }
    let credentialed = router.credentialed_providers(&env_reader, now);
    // B2 startup semantics (replaces the Flux-only exit-2 gate): start iff
    // AT LEAST ONE advertised provider has a usable credential. Per-provider
    // credential failures NEVER abort startup — they surface at set_model /
    // dispatch as typed errors.
    if flux_key.is_none() && credentialed.is_empty() {
        eprintln!("wayland-nano: {}", router.no_credential_message());
        return Ok(2);
    }
    // Egress stays deny-by-default: Flux's host plus exactly the hosts of
    // providers that are BOTH advertised and credentialed at spawn (their
    // base_urls come from the vendored catalog — the sole endpoint
    // authority; the payload can never add a host).
    let mut policy = nano_egress::policy::EgressPolicy::flux_only();
    for provider in &credentialed {
        policy = policy.allow_url(provider.spec.base_url);
    }

    let reader = std::io::BufReader::new(std::io::stdin());
    let writer = std::io::stdout();
    let home = nano_home.to_path_buf();
    let sessions = home.join("sessions");
    // The model catalog the session advertises and validates set_model
    // against: live GET /v1/models once (cached in-process), vendored
    // fixture when offline/unkeyed. If BOTH fail, advertise only the default
    // model — honest (it really runs) and keeps the host usable. C8: when
    // Flux is uncredentialed the catalog is fixture-only, and the payload
    // providers' namespaced models join the advertisement below.
    let catalog = nano_model::flux_models::ModelCatalog::new(
        EgressClient::new(policy.clone()),
        flux_key.clone(),
    );
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
                "wayland-nano: model catalog unavailable ({}); advertising the default model only",
                nano_egress::redact::sanitize_text(&err.to_string())
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
    // C8 §5: the payload providers' namespaced models join the Flux catalog
    // in the advertisement (Flux ids stay bare; `<provider>:<model>` per
    // Q2, human-friendly display names).
    let mut available = available;
    available.extend(router.advertised_models());
    // B2: the deterministic initial binding. Flux when its credential
    // resolves (back-compat with every existing flow); otherwise the first
    // credentialed provider in catalog-table order, bound to its first
    // advertised model in payload order.
    let default_model: String = router
        .initial_model(flux_key.as_deref(), &env_reader, now)
        .expect("B2 gate passed: a credentialed advertised provider exists");
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
    // NOTE(wiring.rs): drivers carry no model — the model id is a per-turn
    // TurnEngine input (model_name → ModelRequest.model). C8: the turn's
    // PROVIDER BINDING (catalog endpoint fields + resolved credential,
    // memory-only) selects the wire client per prompt; set_model only
    // changes the session's model id, and the binding is re-resolved per
    // turn so credential/expiry changes are observed.
    let driver_policy = policy.clone();
    let make_driver = move |binding: &crate::provider_router::ProviderBinding| {
        let egress = EgressClient::new(driver_policy.clone());
        match binding.wire {
            WireKind::OpenAiCompletions => ProviderDriver::openai(
                FluxCompletionsClient::new(egress)
                    .with_base_url(binding.base_url.clone())
                    .with_api_path(binding.api_path.clone()),
                binding.credential.secret().to_string(),
            ),
            WireKind::AnthropicMessages => ProviderDriver::anthropic(
                AnthropicMessagesClient::new(egress)
                    .with_base_url(binding.base_url.clone())
                    .with_api_path(binding.api_path.clone()),
                binding.credential.secret().to_string(),
            ),
        }
    };
    let make_tools = move |workspace: &std::path::Path,
                           mode: PermissionMode,
                           plan_file: &std::path::Path,
                           diff_hook: Option<DiffHook>|
          -> (
        RealToolExecutor,
        nano_core::permissions::FileSystemSandboxPolicy,
    ) {
        // C2: read_only tightens the TOOL-LAYER profile itself (defense in
        // depth — the gate already denies mutations, and the policy refuses
        // writes even if a call somehow reached the tool). default and
        // full_auto share the identical workspace_write policy: a mode NEVER
        // widens the sandbox.
        let profile = match mode {
            PermissionMode::ReadOnly => nano_core::permissions::PermissionProfile::read_only(),
            PermissionMode::Default | PermissionMode::FullAuto => {
                nano_core::permissions::PermissionProfile::workspace_write()
            }
        };
        let mut policy = profile.file_system_sandbox_policy();
        // C10 §3: the session's plan file is writable at the TOOL layer in
        // every mode (grok's rule: plan-file edits are auto-approved in
        // every permission mode) — the gate's posture check stays the
        // semantic authority over WHEN. The root is ONE fixed file under
        // nano_home, never a workspace widening.
        if let Ok(abs) = nano_core::abs::AbsolutePathBuf::from_absolute_path(plan_file) {
            policy
                .entries
                .push(nano_core::permissions::FileSystemSandboxEntry::new(
                    nano_core::permissions::FileSystemPath::Path { path: abs },
                    nano_core::permissions::FileSystemAccessMode::Write,
                ));
        }
        let fs = FsTools::new(policy.clone(), workspace);
        let shell = ShellTool::new(&home, workspace);
        let mut executor = RealToolExecutor::new(fs, shell, workspace);
        // C4: web_fetch is inert (typed denial) unless NANO_WEB_FETCH_HOSTS
        // configures the second egress policy domain.
        if let Some(fetch) = crate::fetch_specs::web_fetch_tool_from_env() {
            executor = executor.with_web_fetch(fetch);
        }
        // C10 §6: live-wire diffs (never journaled).
        if let Some(hook) = diff_hook {
            executor = executor.with_diff_hook(hook);
        }
        (executor, policy)
    };
    // C2: the full_auto shell gate's sandbox availability probe — the same
    // composition doctor reports. Probed ONCE PER TURN at gate construction
    // and cached in the gate; the shell spawn-time transform stays the
    // fail-closed authority regardless of what this cached value says.
    let probe_home = nano_home.to_path_buf();
    let sandbox_probe = move || platform_sandbox_available(&probe_home);
    // Operator-supplied MCP servers (NANO_MCP_SERVERS) merge into every
    // session alongside the mcpServers param Desktop publishes.
    let env_mcp_specs = crate::mcp_specs::mcp_specs_from_env();
    // C5: memory. Writes are opt-in (NANO_MEMORY_WRITE=1/true); the block
    // cap override is downward-only (a larger value is a typed config error,
    // not a silent clamp — same posture as the C1 overrides).
    let memory_write = std::env::var("NANO_MEMORY_WRITE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let memory_block_cap = match parse_env_u64("NANO_MEMORY_BLOCK_CHARS") {
        Ok(Some(cap)) if cap as usize > nano_agent::memory::MEMORY_BLOCK_CHAR_CAP => {
            eprintln!(
                "wayland-nano: NANO_MEMORY_BLOCK_CHARS is downward-only (max {})",
                nano_agent::memory::MEMORY_BLOCK_CHAR_CAP
            );
            return Ok(2);
        }
        Ok(Some(cap)) => cap as usize,
        Ok(None) => nano_agent::memory::MEMORY_BLOCK_CHAR_CAP,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return Ok(2);
        }
    };
    let memory_config = MemoryHostConfig {
        dir: nano_home.join("memory"),
        write_enabled: memory_write,
        block_cap: memory_block_cap,
    };
    let config = ServeConfig {
        sessions_dir: &sessions,
        default_model: &default_model,
        available_models: &available,
        env_mcp_specs: &env_mcp_specs,
        catalog: &catalog_models,
        window_override,
        limit_override,
        sandbox_probe: &sandbox_probe,
        router: &router,
        journal_append_failer: None,
        memory: &memory_config,
    };
    serve(reader, writer, &config, make_driver, make_tools).await
}

/// C2 §4: is a platform sandbox backend available RIGHT NOW? Unix probes
/// for seatbelt/seccomp; Windows checks the provisioned identity marker
/// (the same composition doctor reports at doctor.rs's sandbox check).
/// Any error or unavailability reads as `false` — the full_auto shell arm
/// falls back to the host prompt, never to an unsandboxed run.
fn platform_sandbox_available(nano_home: &std::path::Path) -> bool {
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
    /// C2: sandbox-availability probe for the full_auto shell arm, run ONCE
    /// PER TURN at gate construction (never per approval). Production wires
    /// the platform probe; tests inject both answers.
    pub sandbox_probe: &'a dyn Fn() -> bool,
    /// C8: the validated provider routing table (payload + catalog). Backs
    /// set_model's typed errors and the per-turn binding re-resolution.
    pub router: &'a crate::provider_router::ProviderRouter,
    /// TEST SEAM (C7): when set, the turn sink consults it before every
    /// journal append; `true` simulates a durable-append failure so the
    /// fail-closed `journal_unavailable` path is wire-testable. Production
    /// wires None.
    #[doc(hidden)]
    pub journal_append_failer: Option<&'a dyn Fn() -> bool>,
    /// C5: cross-session memory. Read/injection is always available over the
    /// user-managed store; the write tools exist only when the operator
    /// opted in (NANO_MEMORY_WRITE).
    pub memory: &'a MemoryHostConfig,
}

/// C5 memory wiring for the ACP host.
pub struct MemoryHostConfig {
    /// The store root (`<nano_home>/memory`).
    pub dir: std::path::PathBuf,
    /// NANO_MEMORY_WRITE: agent-authored memory writes (memory_save /
    /// memory_delete) — default OFF (panel ruling Q1).
    pub write_enabled: bool,
    /// NANO_MEMORY_BLOCK_CHARS: downward-only override of the 24k injected
    /// block cap.
    pub block_cap: usize,
}

/// How a finished prompt answers the client (C7/D1): a normal `stopReason`
/// result for `end_turn`/genuine cancels, or a TYPED JSON-RPC error
/// response for turn-fatal failures and non-cancel engine stops.
enum TurnAnswer {
    Stop(&'static str),
    Typed(TypedError),
}

/// The live-wire diff sink (C10 §6): called with (tool call id, diff) when
/// a fs_write/fs_edit succeeds; serve() forwards it as an ACP diff content
/// block. Live-wire-only, never journaled.
pub type DiffHook = Arc<dyn Fn(&str, &nano_agent::turn::FileDiff) + Send + Sync>;

/// `make_driver`/`make_tools` build a fresh pair per prompt (tools are
/// anchored to the session workspace). `make_driver` takes the turn's
/// freshly-resolved PROVIDER BINDING (C8: catalog endpoint + credential,
/// re-resolved at every prompt so credential/expiry changes are observed).
/// `make_tools` takes the turn's CAPTURED permission mode (C2: read_only
/// builds the tightened profile), the session's plan file (C10: writable at
/// the tool layer in every mode — the gate's posture check is the semantic
/// authority), and the turn's live-wire diff hook; it returns the executor
/// together with the EXACT filesystem policy the executor was built from —
/// the approval gate's advisory containment check must run the same policy
/// value, never a separately reconstructed nominally-equivalent one.
pub async fn serve<R, W, FD, FT, D, T>(
    reader: R,
    writer: W,
    config: &ServeConfig<'_>,
    make_driver: FD,
    make_tools: FT,
) -> std::io::Result<i32>
where
    R: BufRead + Send + 'static,
    W: Write + Send + 'static,
    FD: Fn(&crate::provider_router::ProviderBinding) -> D + Send + Sync + 'static,
    FT: Fn(
        &std::path::Path,
        PermissionMode,
        &std::path::Path,
        Option<DiffHook>,
    ) -> (T, nano_core::permissions::FileSystemSandboxPolicy),
    D: ModelDriver + 'static,
    T: ToolExecutor,
{
    let out = Arc::new(Mutex::new(writer));
    let pending: PendingMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
    // The active session's cancel flag, shared with the reader thread so a
    // session/cancel lands IMMEDIATELY — the main loop cannot relay it while
    // the turn future is mid-poll (tool execution runs synchronously).
    let current_cancel: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
        Arc::new(Mutex::new(None));
    // F-C2-1: the active session's shared mode cell, exposed to the reader
    // thread for the same reason. While a turn is parked inside the approval
    // gate (a synchronous wait the main loop cannot interleave with), a
    // session/set_mode request would sit in the inbound channel until the
    // turn ends — so the reader thread relays a DE-escalation straight into
    // the cell, where the gate's min(captured, current) sees it on the very
    // next approval check. Escalations are NEVER relayed (they must not
    // affect the running turn, and an un-journaled escalation would be a
    // fail-open audit gap); the main loop's validate → journal → mutate →
    // ack sequence still processes the queued request exactly once.
    let current_mode: CurrentMode = Arc::new(Mutex::new(None));
    // C6: the active session's task registry, exposed to the reader thread
    // so session/cancel CASCADES to children mid-poll (set every child flag
    // + terminate registered kill handles — fast, no waits).
    let current_tasks: Arc<Mutex<Option<Arc<nano_agent::tasks::TaskRegistry>>>> =
        Arc::new(Mutex::new(None));
    // The driver factory is Arc'd so the per-turn arm AND each session's
    // task registry (C6: every child builds its own driver on its own
    // thread, bound to the session's provider) share it.
    let make_driver = Arc::new(make_driver);
    // C6: builds a session's child-driver factory by resolving the session
    // model's provider binding NOW (fail-closed: a resolution failure
    // becomes a typed task_spawn error, never a silent fallback onto
    // another provider).
    let task_nano_home = config
        .sessions_dir
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| config.sessions_dir.to_path_buf());
    let make_task_driver_factory = {
        let make_driver = make_driver.clone();
        move |model: &str| -> Arc<dyn Fn() -> Result<Arc<dyn ModelDriver>, String> + Send + Sync> {
            let env_reader = |name: &str| std::env::var(name).ok();
            match config
                .router
                .resolve_binding(model, &env_reader, unix_now_secs())
            {
                Ok(binding) => {
                    let make_driver = make_driver.clone();
                    Arc::new(move || Ok(Arc::new(make_driver(&binding)) as Arc<dyn ModelDriver>))
                }
                Err(err) => {
                    let message = format!("task driver unavailable: {err:?}");
                    Arc::new(move || Err(message.clone()))
                }
            }
        }
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Inbound>();
    std::thread::spawn({
        let pending = pending.clone();
        let current_cancel = current_cancel.clone();
        let current_mode = current_mode.clone();
        let current_tasks = current_tasks.clone();
        move || {
            reader_loop(
                reader,
                tx,
                pending,
                current_cancel,
                current_mode,
                current_tasks,
            )
        }
    });

    let mut session: Option<Session> = None;
    let permission_ids = Arc::new(std::sync::atomic::AtomicU64::new(1));
    let mut stdin_open = true;
    #[allow(clippy::type_complexity)]
    let mut turn: Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = (String, TurnAnswer)> + '_>>,
    > = None;
    let mut prompt_id: Option<serde_json::Value> = None;
    let mut turn_session = String::new();

    loop {
        if !stdin_open && turn.is_none() {
            // C6: the host is exiting — tear the session's children down
            // (bounded; a wedged child detaches and KILL_ON_JOB_CLOSE is
            // the process-exit backstop). The registry Drop would do this
            // too; doing it here keeps the ordering explicit.
            if let Some(active) = session.take() {
                active.tasks.teardown_all();
            }
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
                            // waiting on a permission response. C6: cascades to
                            // children (their own flags + kill handles).
                            if let Some(active) = &session {
                                active.cancel.store(true, Ordering::SeqCst);
                                active.tasks.cancel_all();
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
                                    "wayland-nano takes provider credentials from the environment (injected by the spawning host); no interactive auth",
                                ),
                            )?;
                        }
                        "session/new" => {
                            if turn.is_some() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::TurnInProgress,
                                        "turn in progress",
                                        NanoErrorExtras::default(),
                                    ),
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
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::JournalUnavailable,
                                        format!("cannot open session journal: {err}"),
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            *current_cancel.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(cancel.clone());
                            let mode_cell = Arc::new(Mutex::new(PermissionMode::default()));
                            *current_mode.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(mode_cell.clone());
                            let mcp = session_mcp_registry(&params, config.env_mcp_specs);
                            // C10 §3: the plan posture cell. Fail-closed
                            // construction — a sessions dir that cannot be
                            // canonicalized gets no session (the plan-file
                            // containment check depends on it).
                            let plan = match crate::session_tools::PlanPosture::new(
                                config.sessions_dir,
                                &session_id,
                            ) {
                                Ok(posture) => Arc::new(Mutex::new(posture)),
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32603,
                                            format!("cannot initialize session plan posture: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            let todos = Arc::new(Mutex::new(Vec::new()));
                            // C10 §4: a fresh session's context starts with
                            // the bounded, UNTRUSTED-labeled AGENTS.md block
                            // (rendered fresh per rebuild), nothing else.
                            let context = session_context_prefix(&cwd, &todos, &plan);
                            // C6: replacing a live session tears its children
                            // down first (bounded, then detach).
                            if let Some(old) = session.take() {
                                old.tasks.teardown_all();
                            }
                            let tasks = Arc::new(nano_agent::tasks::TaskRegistry::new(
                                &task_nano_home,
                                &cwd,
                                config.default_model.to_string(),
                                make_task_driver_factory(config.default_model),
                            ));
                            *current_tasks.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(tasks.clone());
                            session = Some(Session {
                                id: session_id.clone(),
                                workspace: cwd,
                                cancel,
                                journal,
                                turn_counter: 0,
                                context,
                                model: config.default_model.to_string(),
                                mode: mode_cell,
                                plan,
                                todos,
                                mode_changes: 0,
                                mcp,
                                tasks,
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
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::TurnInProgress,
                                        "turn in progress",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            let params = params.unwrap_or_default();
                            let Some(session_id) =
                                params.get("sessionId").and_then(|s| s.as_str())
                            else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "session/load requires a sessionId",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            if !is_fs_safe_session_id(session_id) {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "invalid session id",
                                        NanoErrorExtras::default(),
                                    ),
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
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::SessionNotFound,
                                        format!("session not found: {session_id}"),
                                        NanoErrorExtras::default(),
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
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::JournalUnavailable,
                                            format!("session journal unreadable: {err}"),
                                            NanoErrorExtras::default(),
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
                            let context_messages = messages_from_envelopes(&report.envelopes);
                            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            *current_cancel.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(cancel.clone());
                            let mode_cell = Arc::new(Mutex::new(PermissionMode::default()));
                            *current_mode.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(mode_cell.clone());
                            let mcp = session_mcp_registry(&params, config.env_mcp_specs);
                            // C10 §3: same fail-closed posture construction
                            // as session/new. The posture itself is NEVER
                            // restored (content replays, postures don't) —
                            // PlanPosture::new starts inactive.
                            let plan = match crate::session_tools::PlanPosture::new(
                                config.sessions_dir,
                                session_id,
                            ) {
                                Ok(posture) => Arc::new(Mutex::new(posture)),
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32603,
                                            format!("cannot initialize session plan posture: {err}"),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            // C10 §2 (Q2 RULED): the todo list IS content —
                            // folded from the journal last-write-wins and
                            // re-injected as a bounded block on rebuild.
                            let todos = Arc::new(Mutex::new(
                                SessionState::fold(&report.envelopes).todos,
                            ));
                            let mut context = session_context_prefix(&cwd, &todos, &plan);
                            context.extend(context_messages);
                            // C6: replacing a live session tears its children
                            // down first (bounded, then detach).
                            if let Some(old) = session.take() {
                                old.tasks.teardown_all();
                            }
                            let tasks = Arc::new(nano_agent::tasks::TaskRegistry::new(
                                &task_nano_home,
                                &cwd,
                                config.default_model.to_string(),
                                make_task_driver_factory(config.default_model),
                            ));
                            *current_tasks.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(tasks.clone());
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
                                // C2 (panel ruling Q5): the mode is NEVER
                                // restored from the journal — a resumed
                                // session starts in `default`; ModeSet ops
                                // are audit history only and elevated
                                // autonomy always takes a fresh set_mode.
                                mode: mode_cell,
                                plan,
                                todos,
                                mode_changes: 0,
                                mcp,
                                tasks,
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
                        // modelId}); session/set_mode is the mode analogue
                        // (C2: read_only / default / full_auto).
                        "session/set_model" => {
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::NoSession,
                                        "no session: call session/new first",
                                        NanoErrorExtras::default(),
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
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "session/set_model requires a modelId",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            // Fail closed: only ids from the advertised catalog
                            // are routable; anything else is a typed error
                            // (Desktop greps the message for model_not_found).
                            // C8: the advertised set is Flux bare ids PLUS the
                            // payload's `<provider>:<model>` namespaced ids.
                            if !config.available_models.iter().any(|m| m.id == model_id) {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::ModelNotFound,
                                        format!(
                                            "model_not_found: {model_id} is not in the advertised catalog"
                                        ),
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            // C8 §5: parse the namespace → catalog row →
                            // live-proof gate → RE-RESOLVE the credential
                            // (never the advisory hasKey): a mid-session
                            // switch to a provider whose key was not injected
                            // fails closed with a typed error, an expired
                            // bearer with retryable oauth_expired + respawn
                            // hint. Resolution registers the secret with the
                            // sanitization boundary (§8/B4).
                            let env_reader = |name: &str| std::env::var(name).ok();
                            if let Err(err) = config.router.resolve_binding(
                                model_id,
                                &env_reader,
                                unix_now_secs(),
                            ) {
                                write_out(&out, &err.acp_response(id))?;
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
                        // C2 §3 — journal-first, accepted-only. The strict
                        // four-step sequence: validate → durable journal
                        // append → mutate the shared mode cell → ack. A
                        // rejected id journals NOTHING (a rejected change
                        // must not fabricate an audit trail); an append
                        // failure leaves the mode visibly unchanged. Lock
                        // ordering: the journal append+flush happens BEFORE
                        // the brief cell lock, so approval checks never
                        // stall behind persistence. Mid-turn set_mode is
                        // exactly how immediate de-escalation arrives, so
                        // (unlike session/compact) this runs while a turn
                        // is in flight; ModeSet is context-neutral on
                        // replay, so interleaving with turn envelopes is
                        // harmless. While the turn is PARKED inside the
                        // gate's synchronous prompt wait this loop cannot
                        // run at all — the reader thread relays a
                        // de-escalation straight into the cell for that
                        // case (F-C2-1); this arm then journals and re-sets
                        // the same value when the loop regains control.
                        "session/set_mode" => {
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::NoSession,
                                        "no session: call session/new first",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            let params = params.unwrap_or_default();
                            let mode_id = params
                                .get("modeId")
                                .and_then(|m| m.as_str())
                                .unwrap_or("");
                            // C10 §3 (Q1 RULED): "plan" is a PROJECTION of
                            //    the orthogonal posture, not a privilege
                            //    mode. Setting it flips the posture ON
                            //    through the ONE journal-first transition
                            //    and leaves the C2 privilege mode untouched;
                            //    the ack advertises currentModeId "plan" and
                            //    (Q5 discoverability) the plan file path.
                            if mode_id == PLAN_MODE_ID {
                                active.mode_changes += 1;
                                let nanos = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_nanos())
                                    .unwrap_or(0);
                                let op_id =
                                    format!("{}-plan-{nanos}-{}", active.id, active.mode_changes);
                                if let Err(err) = crate::session_tools::set_plan_posture(
                                    &active.plan,
                                    &active.journal,
                                    op_id,
                                    true,
                                ) {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(id, -32603, err),
                                    )?;
                                    continue;
                                }
                                let plan_file = active
                                    .plan
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .plan_file()
                                    .display()
                                    .to_string();
                                write_out(
                                    &out,
                                    &JsonRpcResponse::ok(
                                        id,
                                        serde_json::json!({
                                            "modes": session_modes_value(PLAN_MODE_ID),
                                            "planFile": plan_file
                                        }),
                                    ),
                                )?;
                                continue;
                            }
                            // 1. Validate against the PermissionMode
                            //    vocabulary. Unknown ids — including ids a
                            //    NEWER build would know — are a typed error.
                            let Some(mode) = PermissionMode::parse(mode_id) else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        format!("unknown mode: {mode_id}"),
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            // 2. Journal the accepted change FIRST; the
                            //    append must be durable before the mutation.
                            //    Nanos + the per-lifetime counter keep the op
                            //    id unique across resumes. Setting any
                            //    privilege mode also CLEARS the plan posture
                            //    (exit-by-mode-switch): both ops land durably
                            //    before either cell mutates.
                            active.mode_changes += 1;
                            let nanos = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_nanos())
                                .unwrap_or(0);
                            let envelope = OpEnvelope::new(
                                format!("{}-mode-{nanos}-{}", active.id, active.mode_changes),
                                "now",
                                Op::ModeSet {
                                    mode: mode.id().to_string(),
                                },
                            );
                            let journaled = JournalWriter::open(&active.journal)
                                .and_then(|mut writer| writer.append(&envelope).map(|_| ()));
                            if let Err(err) = journaled {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::JournalUnavailable,
                                        format!("cannot journal mode change: {err}"),
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            let plan_was_active = active
                                .plan
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .active;
                            if plan_was_active
                                && let Err(err) = crate::session_tools::set_plan_posture(
                                    &active.plan,
                                    &active.journal,
                                    format!(
                                        "{}-plan-{nanos}-{}-off",
                                        active.id, active.mode_changes
                                    ),
                                    false,
                                )
                            {
                                // The ModeSet op is journaled but NEITHER
                                // cell has mutated yet: report the failure
                                // with both cells visibly unchanged (the
                                // stranded audit op is context-neutral on
                                // replay, so the journal stays honest).
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(id, -32603, err),
                                )?;
                                continue;
                            }
                            // 3. Mutate the shared cell (brief lock — AFTER
                            //    persistence, never around it). A running
                            //    turn's gate observes a de-escalation on its
                            //    very next approval check.
                            *active.mode.lock().unwrap_or_else(|p| p.into_inner()) = mode;
                            // 4. Ack with the re-advertised privilege mode
                            //    (C10: currentModeId reports the underlying
                            //    mode again on plan exit, so a client never
                            //    reads plan→default as a privilege change).
                            write_out(
                                &out,
                                &JsonRpcResponse::ok(
                                    id,
                                    serde_json::json!({ "modes": session_modes_value(mode.id()) }),
                                ),
                            )?;
                        }
                        "session/prompt" => {
                            if turn.is_some() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::TurnInProgress,
                                        "a turn is already running",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::NoSession,
                                        "no session: call session/new first",
                                        NanoErrorExtras::default(),
                                    ),
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
                            let mut prior_context = active.context.clone();
                            // C1: resolve this turn's context-management
                            // config against the ACTIVE model's catalog
                            // window. Overrides are downward-only; an
                            // override that exceeds the active model's window
                            // is a typed error, never a silent clamp.
                            let catalog_window = nano_model::flux_common::context_window_for(
                                &active.model,
                                config.catalog,
                            );
                            // C5 §6: prepend the memory block, rendered FRESH
                            // from the store at every prompt — never cached
                            // at session open — so a save/delete/hand-edit in
                            // turn N is visible from turn N+1. Read errors
                            // fail open (no block). The ACP seam has no
                            // skills block, so skills_chars is 0 here.
                            if let Some(memory_block) =
                                nano_agent::memory::prepare_memory_context(
                                    &nano_agent::memory::MemoryStore::from_dir(
                                        config.memory.dir.clone(),
                                    ),
                                    catalog_window,
                                    0,
                                    config.memory.block_cap,
                                )
                            {
                                prior_context.insert(0, memory_block);
                            }
                            let compaction = match nano_agent::compact::resolve_compaction_config(
                                catalog_window,
                                config.window_override,
                                config.limit_override,
                            ) {
                                Ok(config) => config,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::InvalidParams,
                                            format!("compaction config: {err}"),
                                            NanoErrorExtras::default(),
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
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::JournalUnavailable,
                                            format!("cannot open session journal: {err}"),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            prompt_id = Some(id.clone());
                            turn_session = active.id.clone();
                            let session_id = active.id.clone();
                            let workspace = active.workspace.clone();
                            let cancel = active.cancel.clone();
                            // The session's current model (set via
                            // session/set_model) is captured NOW: the whole
                            // turn runs on it, and a later switch only takes
                            // effect on the next prompt. C8: resolve the
                            // provider binding (credential re-resolution +
                            // bearer freshness) BEFORE the turn starts — a
                            // vanished key or an expired bearer fails the
                            // prompt with the typed error, never a
                            // half-authed turn.
                            let turn_model = active.model.clone();
                            let env_reader = |name: &str| std::env::var(name).ok();
                            let binding = match config.router.resolve_binding(
                                &turn_model,
                                &env_reader,
                                unix_now_secs(),
                            ) {
                                Ok(binding) => binding,
                                Err(err) => {
                                    write_out(&out, &err.acp_response(id))?;
                                    continue;
                                }
                            };
                            if let Err(err) = binding.check_fresh(unix_now_secs()) {
                                write_out(&out, &err.acp_response(id))?;
                                continue;
                            }
                            // The session's MCP registry: the turn executor
                            // routes mcp__ calls through it (and advertises
                            // its tools to the model) without taking ownership.
                            let turn_mcp = active.mcp.clone();
                            // C2 §3 asymmetric application: the turn captures
                            // a mode SNAPSHOT (its tool-layer profile and its
                            // escalation ceiling) AND a clone of the shared
                            // cell. The gate computes min(captured, current)
                            // per approval — de-escalation is immediate,
                            // escalation waits for the next prompt.
                            let turn_mode = *active.mode.lock().unwrap_or_else(|p| p.into_inner());
                            let mode_cell = active.mode.clone();
                            // C10: the session's plan posture / todo cells
                            // and plan file — shared with the gate (posture
                            // enforcement) and the session-tool wrapper
                            // (todo/plan/question execution).
                            let plan_cell = active.plan.clone();
                            let todos_cell = active.todos.clone();
                            let plan_file = active
                                .plan
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .plan_file()
                                .to_path_buf();
                            let journal_path = active.journal.clone();
                            // The session's task registry (C6): the turn's
                            // executor routes task_* calls through it.
                            let turn_tasks = active.tasks.clone();
                            // The turn future must own its handles: clone the
                            // loop-invariant Arcs before the `async move`.
                            let gate_out = out.clone();
                            let gate_pending = pending.clone();
                            let gate_ids = permission_ids.clone();
                            let sink_out = out.clone();
                            let sandbox_probe = config.sandbox_probe;
                            let memory_dir = config.memory.dir.clone();
                            let memory_write = config.memory.write_enabled;
                            let make_driver = make_driver.clone();
                            let make_tools = &make_tools;
                            let turn_future = async move {
                                // The binding's bare model id goes on the
                                // wire (the namespace is a Nano-side routing
                                // concern, never sent to the provider).
                                let turn_model = binding.model.clone();
                                let driver = make_driver(&binding);
                                // C10 §6: the live-wire diff hook — a
                                // successful fs_write/fs_edit forwards its
                                // structured before/after pair as an ACP
                                // diff content block on the same tool call.
                                let diff_out = sink_out.clone();
                                let diff_session = session_id.clone();
                                let diff_hook: DiffHook =
                                    Arc::new(move |call_id: &str, diff: &nano_agent::turn::FileDiff| {
                                        let mut guard =
                                            diff_out.lock().unwrap_or_else(|p| p.into_inner());
                                        let _ = write_json(
                                            &mut *guard,
                                            &tool_call_diff(
                                                &diff_session,
                                                call_id,
                                                &diff.path,
                                                diff.old_text.as_deref(),
                                                &diff.new_text,
                                            ),
                                        );
                                    });
                                // The executor AND its exact policy: the
                                // gate's advisory containment check runs this
                                // same policy value (shared provenance), and
                                // read_only builds the executor itself on the
                                // tightened profile (defense in depth).
                                let (tools, turn_policy) = make_tools(
                                    &workspace,
                                    turn_mode,
                                    &plan_file,
                                    Some(diff_hook),
                                );
                                // MCP-merged executor: mcp__ names route to the
                                // session registry, everything else to the core
                                // tools; the model sees both tool sets.
                                let mcp_executor = McpToolExecutor::from_shared(turn_mcp, &tools);
                                let mut tool_definitions = v1_tool_definitions();
                                tool_definitions
                                    .extend(mcp_executor.tool_definitions_from_registry());
                                // C5: the memory family routes through its own
                                // chokepoint wrapper (validation + redaction +
                                // caps). Read tools always; write tools only
                                // behind the operator opt-in — the listing the
                                // model sees reflects exactly that.
                                let memory_executor = nano_agent::memory::MemoryToolExecutor::new(
                                    nano_agent::memory::MemoryStore::from_dir(memory_dir),
                                    memory_write,
                                    &mcp_executor,
                                );
                                tool_definitions.extend(
                                    nano_agent::memory::memory_tool_definitions(memory_write),
                                );
                                let gate = AcpApproval {
                                    session_id: session_id.clone(),
                                    out: gate_out,
                                    pending: gate_pending,
                                    next_id: gate_ids,
                                    cancel: cancel.clone(),
                                    captured_mode: turn_mode,
                                    mode_cell,
                                    policy: turn_policy,
                                    // The gate's cwd is the SAME session
                                    // field the tools were built from —
                                    // divergent cwd would split the gate's
                                    // approve-set from the executor's
                                    // write-set on relative paths.
                                    cwd: workspace.clone(),
                                    // Probed once per turn, here at gate
                                    // construction; the spawn-time transform
                                    // stays the fail-closed authority.
                                    sandbox_available: sandbox_probe(),
                                    // C10 §3: the shared posture cell —
                                    // enforcement at the gate, live mid-turn.
                                    plan: plan_cell.clone(),
                                };
                                // C10: the session-owned tools (todo / plan /
                                // ask_user) wrap the MCP-merged executor and
                                // route questions through the gate's ONE ask
                                // channel.
                                let executor = crate::session_tools::SessionTools::new(
                                    &memory_executor,
                                    &gate,
                                    todos_cell,
                                    plan_cell,
                                    journal_path,
                                    session_id.clone(),
                                );
                                // C6: the task family routes through the
                                // session's registry
                                // (spawn/poll/cancel/apply) — outermost, so
                                // task_* calls never reach the inner layers.
                                let executor = nano_agent::tasks::TaskToolExecutor::new(
                                    turn_tasks,
                                    &executor,
                                );
                                tool_definitions
                                    .extend(nano_agent::tasks::task_tool_definitions());
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
                                let journal_failer = config.journal_append_failer;
                                let mut sink = move |envelope: &OpEnvelope| -> bool {
                                    // Journal first: the durable record leads
                                    // the live frame, never the other way. The
                                    // compaction commit protocol reads this
                                    // return value — its in-memory swap only
                                    // happens behind a durable Complete.
                                    let journaled = if journal_failer
                                        .is_some_and(|failer| failer())
                                    {
                                        // C7 test seam: injected failure.
                                        eprintln!(
                                            "wayland-nano: session journal append failed (injected)"
                                        );
                                        false
                                    } else {
                                        match journal_writer.append(envelope) {
                                            Ok(_) => true,
                                            Err(err) => {
                                                eprintln!(
                                                    "wayland-nano: session journal append failed: {err}"
                                                );
                                                false
                                            }
                                        }
                                    };
                                    // C7/D4 fail-closed: an op that could not
                                    // be journaled NEVER becomes a live frame
                                    // — the engine turns a ToolCall/ToolResult
                                    // append failure into a turn-fatal
                                    // journal_unavailable on this `false`.
                                    if !journaled {
                                        return false;
                                    }
                                    let mut guard =
                                        sink_out.lock().unwrap_or_else(|p| p.into_inner());
                                    let _ =
                                        write_op_frame(&mut *guard, &sink_session, envelope);
                                    true
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
                                // C7/D1+D6: turn-fatal failures and non-cancel
                                // engine stops answer with a TYPED error
                                // response; stopReason survives only for
                                // end_turn and genuine cancels. The mislabeled
                                // "cancelled" for engine stops is gone.
                                let answer = match result.state {
                                    TurnState::Complete => TurnAnswer::Stop("end_turn"),
                                    TurnState::Stopped(ref info)
                                        if info.kind == NanoErrorKind::UserCancelled =>
                                    {
                                        TurnAnswer::Stop("cancelled")
                                    }
                                    TurnState::Stopped(info) => {
                                        TurnAnswer::Typed(TypedError::new(info.kind, info.detail))
                                    }
                                    TurnState::Failed(err) => TurnAnswer::Typed(err),
                                    _ => TurnAnswer::Typed(TypedError::new(
                                        NanoErrorKind::Unknown,
                                        "turn ended in an unexpected state",
                                    )),
                                };
                                (result.final_text, answer)
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
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::TurnInProgress,
                                        "turn in progress",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            let Some(active) = session.as_mut() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::NoSession,
                                        "no session: call session/new first",
                                        NanoErrorExtras::default(),
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
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::JournalUnavailable,
                                            format!("cannot open session journal: {err}"),
                                            NanoErrorExtras::default(),
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
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::JournalUnavailable,
                                            format!("session journal unreadable: {err}"),
                                            NanoErrorExtras::default(),
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
                            // C8: compaction runs on the session's bound
                            // provider too — re-resolve the binding (typed
                            // error on a vanished credential, never a
                            // fallback onto another provider).
                            let env_reader = |name: &str| std::env::var(name).ok();
                            let binding = match config.router.resolve_binding(
                                &active.model,
                                &env_reader,
                                unix_now_secs(),
                            ) {
                                Ok(binding) => binding,
                                Err(err) => {
                                    write_out(&out, &err.acp_response(id))?;
                                    continue;
                                }
                            };
                            let driver = make_driver(&binding);
                            let model_name = binding.model.clone();
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
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::CompactionFailed,
                                        format!("compaction failed: {err}"),
                                        NanoErrorExtras::default(),
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
                    None => std::future::pending::<(String, TurnAnswer)>().await,
                }
            }, if turn.is_some() => {
                turn = None;
                let (final_text, answer) = outcome;
                // Fold the just-finished turn into the session's context so
                // the NEXT prompt continues the conversation (same rebuild
                // path session/load uses — one honest code path).
                if let Some(active) = session.as_mut()
                    && active.id == turn_session
                {
                    match read_journal(&active.journal) {
                        Ok(report) => {
                            // C10: refresh the todo cell from the journal
                            // fold (TodoSet ops land via the session tools
                            // mid-turn) and rebuild with the bounded prefix
                            // blocks (AGENTS.md re-read FRESH here, so an
                            // edit between turns is picked up).
                            *active.todos.lock().unwrap_or_else(|p| p.into_inner()) =
                                SessionState::fold(&report.envelopes).todos;
                            let mut context = session_context_prefix(
                                &active.workspace,
                                &active.todos,
                                &active.plan,
                            );
                            context.extend(messages_from_envelopes(&report.envelopes));
                            active.context = context;
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
                // D1 mid-stream semantics: already-produced assistant text is
                // committed BEFORE any error answer — the partial content
                // stays, the error banner follows it (no rollback).
                if !final_text.is_empty() {
                    write_out(&out, &agent_message_chunk(&turn_session, &final_text))?;
                }
                if let Some(id) = prompt_id.take() {
                    if cancel_fired {
                        write_out(&out, &JsonRpcResponse::ok(id, prompt_result("cancelled")))?;
                    } else {
                        match answer {
                            TurnAnswer::Stop(stop_reason) => {
                                write_out(&out, &JsonRpcResponse::ok(id, prompt_result(stop_reason)))?;
                            }
                            TurnAnswer::Typed(err) => {
                                // The wire message is the STATIC table
                                // presentation — never the logs-side detail
                                // (design §7: no provider free-text in
                                // UI-bound frames); typed extras ride in
                                // data.nanoError as closed fields.
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        err.kind,
                                        error_presentation(err.kind),
                                        NanoErrorExtras {
                                            status: err.status,
                                            retry_after_ms: err.retry_after_ms,
                                            host: err.host,
                                        },
                                    ),
                                )?;
                            }
                        }
                    }
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
///
/// Two relays land HERE because the main loop may be parked mid-poll inside
/// the turn (the approval gate waits synchronously on the client):
/// - `session/cancel` fires the session's cancel flag immediately;
/// - `session/set_mode` with a DE-escalating mode id mutates the session's
///   shared mode cell immediately (F-C2-1), so the parked turn's gate sees
///   it via min(captured, current) on its very next approval check.
///
/// The set_mode relay is de-escalation-only and fail-safe by construction:
/// an escalation is never relayed (it must not affect the running turn, and
/// an un-journaled escalation would be a fail-open audit gap), an unknown id
/// relays nothing, and any lock failure relays nothing — the queued request
/// still reaches the main loop, whose validate → journal → mutate → ack
/// sequence remains the single journaling point (the relay only re-sets a
/// value the loop will set again). A relay error can therefore only ever
/// mean MORE prompts, never fewer.
fn reader_loop<R: BufRead>(
    mut reader: R,
    tx: tokio::sync::mpsc::UnboundedSender<Inbound>,
    pending: PendingMap,
    current_cancel: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>>,
    current_mode: CurrentMode,
    current_tasks: Arc<Mutex<Option<Arc<nano_agent::tasks::TaskRegistry>>>>,
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
            (Some(method), Some(id)) => {
                if method == "session/set_mode" {
                    // F-C2-1 de-escalation relay: while the main loop is
                    // parked inside the gate's synchronous prompt wait, this
                    // request would sit in the channel until the turn ends.
                    // A DE-escalation is written straight into the session's
                    // shared mode cell — the running turn's gate observes it
                    // via min(captured, current) on its very next approval
                    // check. Escalations relay NOTHING (min() already keeps
                    // them off the running turn, and the journal-first
                    // discipline in the main loop must remain the only path
                    // that can raise the recorded mode). Any error here
                    // (unknown id, poisoned lock, no session) simply skips
                    // the relay: fail-safe = more prompts, never fewer.
                    let mode_id = value
                        .get("params")
                        .and_then(|p| p.get("modeId"))
                        .and_then(|m| m.as_str());
                    if let Some(mode) = mode_id.and_then(PermissionMode::parse)
                        && let Some(cell) = current_mode
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .as_ref()
                    {
                        let mut guard = cell.lock().unwrap_or_else(|p| p.into_inner());
                        if mode < *guard {
                            *guard = mode;
                        }
                    }
                }
                Inbound::Request {
                    id,
                    method,
                    params: value.get("params").cloned(),
                }
            }
            (Some(method), None) => {
                if method == "session/cancel" {
                    // Fire the flag right here: the main loop may be mid-poll
                    // inside the turn and unable to relay it for whole steps.
                    // C6: cascade to children too — every child flag set and
                    // every registered kill handle terminated (fast, no waits).
                    if let Some(flag) = current_cancel
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                    {
                        flag.store(true, Ordering::SeqCst);
                    }
                    if let Some(tasks) = current_tasks
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .as_ref()
                    {
                        tasks.cancel_all();
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

/// Approval gate (C2 mode-aware). Read-only tools auto-approve in every
/// mode; everything else is decided by the EFFECTIVE mode —
/// `min(captured, current)` under the PermissionMode privilege ordering, so
/// a mid-turn de-escalation tightens the very next check while an
/// escalation never raises a running turn:
/// - `read_only`: deny at the gate (no prompt — the mode categorically
///   forbids the action) and [`ApprovalGate::denial_reason`] names the mode;
/// - `default`: prompt the host (the historical path, verbatim);
/// - `full_auto`: contained fs_write/fs_edit auto-approve (advisory oracle —
///   the tool layer's execution-time check stays authoritative); shell
///   auto-approves iff the per-turn sandbox-availability probe said a
///   backend exists; uncontained/unknown/MCP still prompt.
///
/// The wire-prompt path emits `session/request_permission` and blocks on
/// the client response, which the reader thread routes here by JSON-RPC id.
/// It denies on rejection, malformed responses, disconnect, or cancel
/// (fail-closed) — and every ambiguity in the mode arms falls THROUGH to
/// it, never to a silent approve or silent deny.
struct AcpApproval<W: Write> {
    session_id: String,
    out: Arc<Mutex<W>>,
    pending: PendingMap,
    next_id: Arc<std::sync::atomic::AtomicU64>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// The mode captured at prompt time: the turn's escalation ceiling.
    captured_mode: PermissionMode,
    /// The session's live mode cell: de-escalations land here mid-turn.
    mode_cell: Arc<Mutex<PermissionMode>>,
    /// A clone of the EXACT executor policy built for this turn.
    policy: nano_core::permissions::FileSystemSandboxPolicy,
    /// The same session workspace the tools were constructed with.
    cwd: std::path::PathBuf,
    /// Sandbox-backend availability, probed once per turn at construction.
    sandbox_available: bool,
    /// The session's shared plan-posture cell (C10 §3): while active,
    /// fs_write/fs_edit deny for everything but the session's plan file —
    /// in EVERY C2 mode, including full_auto. Live: a tool-driven mid-turn
    /// entry tightens the very next approval check.
    plan: Arc<Mutex<crate::session_tools::PlanPosture>>,
}

impl<W: Write> AcpApproval<W> {
    /// `min(captured, current)`: effective privilege can only ever be ≤ the
    /// captured privilege — strictly fail-closed, one lock read per call.
    fn effective_mode(&self) -> PermissionMode {
        let current = *self.mode_cell.lock().unwrap_or_else(|p| p.into_inner());
        self.captured_mode.min(current)
    }
}

impl<W: Write> std::fmt::Debug for AcpApproval<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpApproval")
            .field("session_id", &self.session_id)
            .field("captured_mode", &self.captured_mode)
            .field("sandbox_available", &self.sandbox_available)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> ApprovalGate for AcpApproval<W> {
    fn approve(&self, call: &ToolCall) -> ApprovalDecision {
        // 1. Read-only fast-path: unchanged in every mode.
        if is_read_only_tool(&call.name) {
            return ApprovalDecision::Approve;
        }
        // 1b. C10 session tools are auto-allowed in every C2 mode: todo's
        //     write path mutates only journaled session state; enter/exit
        //     plan carry their own posture semantics (the exit's prompt is
        //     the ask channel, not this gate); ask_user IS a prompt — a
        //     permission prompt for the question prompt would be theatre.
        if nano_agent::wiring::SESSION_TOOL_NAMES.contains(&call.name.as_str()) {
            return ApprovalDecision::Approve;
        }
        // 2. C10 §3 plan posture — enforcement AT THE GATE, before the mode
        //    arms, in every C2 mode including full_auto: fs_write/fs_edit
        //    pass ONLY for the session's plan file (a creation-safe
        //    nano_home-containment check, never a workspace exception).
        //    Shell and everything else stay governed by the mode below.
        if let Some(allowed) = crate::session_tools::posture_allows(&self.plan, call, &self.cwd) {
            return if allowed {
                ApprovalDecision::Approve
            } else {
                ApprovalDecision::Deny
            };
        }
        match self.effective_mode() {
            // 2. read_only: categorical denial, no prompt (panel ruling Q3).
            //    The read_only() tool-layer profile backstops this.
            PermissionMode::ReadOnly => ApprovalDecision::Deny,
            // 3. default: today's behavior — everything else asks the host.
            PermissionMode::Default => self.prompt_host(call),
            PermissionMode::FullAuto => match call.name.as_str() {
                // 4a. Contained fs writes auto-approve. Missing/unparseable
                //     path or an uncontained target falls THROUGH to the
                //     host prompt — ambiguity always resolves toward the
                //     human, never to a silent deny or silent approve.
                "fs_write" | "fs_edit" => {
                    let contained = call
                        .arguments
                        .get("path")
                        .and_then(|value| value.as_str())
                        .is_some_and(|path| {
                            self.policy
                                .can_write_path_with_cwd(std::path::Path::new(path), &self.cwd)
                        });
                    if contained {
                        ApprovalDecision::Approve
                    } else {
                        self.prompt_host(call)
                    }
                }
                // 4b. Shell auto-approves only behind a probed sandbox
                //     backend; the spawn-time transform fails closed with
                //     SandboxUnavailable regardless of this cached value.
                "shell" => {
                    if self.sandbox_available {
                        ApprovalDecision::Approve
                    } else {
                        self.prompt_host(call)
                    }
                }
                // 4c. mcp__* and anything unrecognized: containment cannot
                //     be asserted for them, so they always ask.
                _ => self.prompt_host(call),
            },
        }
    }

    fn denial_reason(&self) -> Option<&'static str> {
        // The plan posture reason takes precedence: it is the more specific
        // (and more actionable) explanation when both apply.
        if self.plan.lock().unwrap_or_else(|p| p.into_inner()).active {
            return Some("plan mode is active: writes are restricted to the session's plan file");
        }
        (self.effective_mode() == PermissionMode::ReadOnly)
            .then_some("session is in read_only mode")
    }

    /// C10 §5: the structured question channel. Mints `opt_{i}` wire ids
    /// from the tool's option labels, emits the question over the same
    /// session/request_permission method, and resolves the response back to
    /// the LABEL through the id→label map captured at send time. Never
    /// blocks forever: cancel (100ms poll), the bounded timeout, malformed
    /// responses, and disconnect all fail closed to a typed denial; the
    /// pending-map entry is removed on EVERY exit (a late answer then lands
    /// in the reader's unknown-id arm and is dropped+logged).
    fn ask(&self, call: &ToolCall) -> nano_agent::turn::AskOutcome {
        use nano_agent::turn::AskOutcome;
        if let Err(err) = crate::session_tools::validate_question_args(&call.arguments) {
            return AskOutcome::Denied(format!("malformed question: {err}"));
        }
        let labels = crate::session_tools::question_labels(&call.arguments);
        // Q3 RULED: 5-minute default; `0` means "no timeout, interactive-
        // only" — but ACP is capability-blind (no negotiated client
        // capability exists today), so 0 is NORMALIZED to the default here.
        let timeout_secs = match call
            .arguments
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
        {
            Some(0) | None => 300,
            Some(secs) => secs,
        };
        let title = call
            .arguments
            .get("header")
            .and_then(|h| h.as_str())
            .filter(|h| !h.trim().is_empty())
            .unwrap_or("question")
            .to_string();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        let request = request_question_request(
            id,
            &self.session_id,
            &call.id,
            &title,
            &call.arguments,
            &labels,
        );
        if write_out(&self.out, &request).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return AskOutcome::Denied("host unreachable: cannot emit the question".into());
        }
        // The id→label map captured at send time: the wire carries only the
        // minted id; the label never needs to round-trip.
        let id_to_label: std::collections::HashMap<String, String> = labels
            .iter()
            .enumerate()
            .map(|(i, label)| (format!("opt_{i}"), label.clone()))
            .collect();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        let outcome = loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(response) => break answer_from_response(&response, &id_to_label),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if self.cancel.load(Ordering::SeqCst) {
                        break AskOutcome::Denied("cancelled".into());
                    }
                    if std::time::Instant::now() >= deadline {
                        break AskOutcome::Denied(format!(
                            "question timed out after {timeout_secs}s"
                        ));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break AskOutcome::Denied("host disconnected".into());
                }
            }
        };
        // Removal on EVERY exit (answer, timeout, cancel, disconnect).
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        outcome
    }
}

impl<W: Write> AcpApproval<W> {
    /// The `default`-mode path and the full_auto fall-through: ask the host.
    fn prompt_host(&self, call: &ToolCall) -> ApprovalDecision {
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
/// read-only prefixes and always go through the permission gate. C5:
/// `memory_list`/`memory_read` are read-only; `memory_save`/`memory_delete`
/// mutate the user-managed store and always ask (under read_only they are
/// categorically denied, like every other mutation). C6: task polls
/// (`task_status`/`task_result`/`task_list`) are read-only; task_spawn,
/// task_cancel, and task_apply change live state and always ask.
fn is_read_only_tool(name: &str) -> bool {
    name.starts_with("fs_read")
        || name.starts_with("search")
        || name.starts_with("glob")
        || name.starts_with("memory_list")
        || name.starts_with("memory_read")
        || name.starts_with("task_status")
        || name.starts_with("task_result")
        || name.starts_with("task_list")
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

/// Interprets a QUESTION response (C10 §5): the answer-channel counterpart
/// of [`decision_from_response`]. A `selected` outcome naming a known
/// `opt_{i}` resolves the LABEL through the id→label map captured at send
/// time; a `rejected` outcome (Desktop's Dismiss mapping), a selected
/// `reject` id (the TUI's Esc path), an unknown id, or any malformed shape
/// is a typed denial — fail-closed everywhere.
fn answer_from_response(
    value: &serde_json::Value,
    id_to_label: &std::collections::HashMap<String, String>,
) -> nano_agent::turn::AskOutcome {
    use nano_agent::turn::AskOutcome;
    let Some(outcome) = value.get("result").and_then(|r| r.get("outcome")) else {
        return AskOutcome::Denied("malformed response: no outcome".into());
    };
    let option_id = outcome.get("optionId").and_then(|o| o.as_str());
    match outcome.get("outcome").and_then(|o| o.as_str()) {
        Some("selected") => match option_id {
            Some(nano_protocol::acp::QUESTION_DISMISS_ID) => {
                AskOutcome::Denied("dismissed by the user".into())
            }
            Some(id) => match id_to_label.get(id) {
                Some(label) => AskOutcome::Answered(label.clone()),
                None => AskOutcome::Denied(format!("malformed response: unknown option id {id:?}")),
            },
            None => AskOutcome::Denied("malformed response: no optionId".into()),
        },
        Some("rejected") => AskOutcome::Denied("dismissed by the user".into()),
        _ => AskOutcome::Denied("malformed response: unknown outcome".into()),
    }
}

/// The bounded context prefix prepended at every session context rebuild
/// (C10): the UNTRUSTED-labeled AGENTS.md block (§4, rendered FRESH each
/// rebuild so mid-session edits are picked up next turn), the plan-posture
/// instructions while active (§3 prompt layer — defense in depth, never
/// the mechanism), and the restored todo block (§2 Q2: 50 items / 4k chars,
/// clearly delimited). Empty when nothing applies.
fn session_context_prefix(
    workspace: &std::path::Path,
    todos: &Arc<Mutex<Vec<TodoItem>>>,
    plan: &Arc<Mutex<crate::session_tools::PlanPosture>>,
) -> Vec<Message> {
    let mut messages = Vec::new();
    if let Some(message) = nano_agent::skills::prepare_agents_md_context(workspace) {
        messages.push(message);
    }
    let plan_guard = plan.lock().unwrap_or_else(|p| p.into_inner());
    if plan_guard.active {
        messages.push(Message::system(
            crate::session_tools::plan_mode_instructions(plan_guard.plan_file()),
        ));
    }
    drop(plan_guard);
    let todos_guard = todos.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(block) = crate::session_tools::todo_restore_block(&todos_guard) {
        messages.push(Message::system(block));
    }
    messages
}

/// Builds the `session/load` replay: one notification per historical beat, in
/// journal order — user chunks (Desktop ignores them; real agents emit them),
/// tool cards carrying their FINAL status (plus the trailing update with the
/// output digest), and assistant text chunks. A call whose ToolResult never
/// journaled (crash mid-call) is replayed as failed so no card hangs
/// `in_progress` forever.
fn replay_frames(session_id: &str, envelopes: &[OpEnvelope]) -> Vec<JsonRpcNotification> {
    let results: std::collections::HashMap<&str, (bool, &str, Option<NanoErrorKind>)> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.op {
            Op::ToolResult {
                call_id,
                ok,
                output_digest,
                error_kind,
                ..
            } => Some((call_id.as_str(), (*ok, output_digest.as_str(), *error_kind))),
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
                Some((ok, digest, error_kind)) => {
                    // Live and replayed frames are identical by construction:
                    // both consume the same typed op (C7 §2.1 handoff 3).
                    frames.push(tool_call_replay(
                        session_id,
                        call_id,
                        name,
                        args,
                        *ok,
                        *error_kind,
                    ));
                    frames.push(tool_call_done(
                        session_id,
                        call_id,
                        *ok,
                        digest,
                        *error_kind,
                    ));
                }
                None => {
                    frames.push(tool_call_replay(
                        session_id, call_id, name, args, false, None,
                    ));
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
                error_kind,
                ..
            } => {
                flush_assistant(&mut messages, &mut assistant);
                // [R1] The ONE synthesized-result encoding, shared with the
                // compaction repair pass and repeat-protection skips.
                // C7/D5: a typed failure resumes as `<presentation> [output
                // elided]` so the model still sees WHY the call failed; the
                // kind is journaled, the text re-derives from the table.
                let content = match (ok, error_kind) {
                    (false, Some(kind)) => {
                        format!("{} [output elided]", error_presentation(*kind))
                    }
                    _ => format!(
                        "[tool output elided from journal: ok={ok}, digest={output_digest}]"
                    ),
                };
                messages.push(Message::tool_result(call_id, content, !ok));
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
            error_kind,
            ..
        } => write_json(
            writer,
            &tool_call_done(session_id, call_id, *ok, output_digest, *error_kind),
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

#[cfg(test)]
mod tests {
    //! C2 §9 per-mode gate matrix: for each of {read_only, default,
    //! full_auto} × {fs_read, contained fs_write/fs_edit, uncontained write,
    //! shell, mcp__*} assert approve/prompt/deny per design §4 — with
    //! WIRE-LEVEL assertions on whether a `session/request_permission` frame
    //! was emitted (read_only and contained full_auto writes must emit none;
    //! everything unresolved emits exactly one).

    use super::*;
    use nano_core::permissions::PermissionProfile;

    /// A temp workspace that cleans itself up.
    struct TestWorkspace(std::path::PathBuf);

    fn workspace() -> TestWorkspace {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "nano-c2-gate-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("workspace");
        TestWorkspace(dir)
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A directory outside EVERY writable root on any platform, anchored at
    /// the filesystem root: the workspace_write policy grants write on the
    /// workspace plus the tmp roots only, so a root-anchored path is always
    /// denied. (Never created — denial happens before any fs write.)
    fn outside_root() -> std::path::PathBuf {
        std::env::temp_dir()
            .ancestors()
            .last()
            .expect("filesystem root")
            .join("nano-c2-definitely-outside")
    }

    /// A plan-posture cell for gate tests: rooted at `<workspace>/sessions`
    /// (created on demand), session id "test-session".
    fn test_posture(workspace: &std::path::Path) -> Arc<Mutex<crate::session_tools::PlanPosture>> {
        let sessions = workspace.join("sessions");
        std::fs::create_dir_all(&sessions).expect("sessions dir");
        Arc::new(Mutex::new(
            crate::session_tools::PlanPosture::new(&sessions, "test-session").expect("posture"),
        ))
    }

    /// A gate plus its scripted host: when `answer` is Some, a responder
    /// thread answers every permission request with that optionId; when
    /// None, any prompt would block forever — tests that expect NO prompt
    /// pass None, so an unexpected prompt hangs loudly (test timeout)
    /// instead of passing silently.
    struct TestGate {
        gate: AcpApproval<Vec<u8>>,
        out: Arc<Mutex<Vec<u8>>>,
        mode_cell: Arc<Mutex<PermissionMode>>,
        plan: Arc<Mutex<crate::session_tools::PlanPosture>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        responder: Option<std::thread::JoinHandle<()>>,
    }

    impl TestGate {
        fn new(
            captured: PermissionMode,
            workspace: &std::path::Path,
            sandbox_available: bool,
            answer: Option<&'static str>,
        ) -> Self {
            let out: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
            let pending: PendingMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
            let mode_cell = Arc::new(Mutex::new(captured));
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let responder = answer.map(|option| {
                let pending = pending.clone();
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        let id = pending
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .keys()
                            .next()
                            .copied();
                        match id {
                            Some(id) => {
                                let tx = pending
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .remove(&id);
                                if let Some(tx) = tx {
                                    let _ = tx.send(serde_json::json!({
                                        "result": {"outcome": {"outcome": "selected", "optionId": option}}
                                    }));
                                }
                            }
                            None => std::thread::sleep(std::time::Duration::from_millis(2)),
                        }
                    }
                })
            });
            let plan = test_posture(workspace);
            let gate = AcpApproval {
                session_id: "test-session".into(),
                out: out.clone(),
                pending,
                next_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                captured_mode: captured,
                mode_cell: mode_cell.clone(),
                policy: PermissionProfile::workspace_write().file_system_sandbox_policy(),
                cwd: workspace.to_path_buf(),
                sandbox_available,
                plan: plan.clone(),
            };
            Self {
                gate,
                out,
                mode_cell,
                plan,
                stop,
                responder,
            }
        }

        /// How many `session/request_permission` frames the gate emitted.
        fn prompt_count(&self) -> usize {
            let bytes = self.out.lock().unwrap_or_else(|p| p.into_inner());
            String::from_utf8_lossy(&bytes)
                .matches("session/request_permission")
                .count()
        }

        fn set_mode(&self, mode: PermissionMode) {
            *self.mode_cell.lock().unwrap_or_else(|p| p.into_inner()) = mode;
        }
    }

    impl Drop for TestGate {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(responder) = self.responder.take() {
                let _ = responder.join();
            }
        }
    }

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments,
        }
    }

    fn contained_write(ws: &std::path::Path) -> ToolCall {
        call(
            "fs_write",
            serde_json::json!({"path": ws.join("inside.txt"), "content": "x"}),
        )
    }

    #[test]
    fn read_tools_auto_approve_in_all_modes_without_prompting() {
        let ws = workspace();
        for mode in PermissionMode::ALL {
            for tool in ["fs_read", "search_code", "glob_files"] {
                let rig = TestGate::new(mode, &ws.0, false, None);
                assert_eq!(
                    rig.gate
                        .approve(&call(tool, serde_json::json!({"path": "a"}))),
                    ApprovalDecision::Approve,
                    "{mode:?} must auto-approve {tool}"
                );
                assert_eq!(rig.prompt_count(), 0, "{mode:?}/{tool} must not prompt");
            }
        }
    }

    #[test]
    fn read_only_denies_mutations_at_the_gate_naming_the_mode() {
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::ReadOnly, &ws.0, true, None);
        for denied in [
            contained_write(&ws.0),
            call(
                "fs_edit",
                serde_json::json!({"path": ws.0.join("a.txt"), "old_string": "a", "new_string": "b"}),
            ),
            call("shell", serde_json::json!({"command": "ls"})),
            call("mcp__server__tool", serde_json::json!({})),
        ] {
            assert_eq!(
                rig.gate.approve(&denied),
                ApprovalDecision::Deny,
                "read_only must deny {} at the gate",
                denied.name
            );
        }
        // No prompt was EVER emitted — prompting for categorically forbidden
        // actions would be a dark pattern re-widening the session.
        assert_eq!(rig.prompt_count(), 0);
        // The denial names the mode so the model learns WHY (and the engine
        // appends it to the tool result — pinned in nano-agent turn_tests).
        assert_eq!(
            rig.gate.denial_reason(),
            Some("session is in read_only mode")
        );
    }

    #[test]
    fn default_prompts_for_everything_mutating() {
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("allow"));
        for (name, args) in [
            (
                "fs_write",
                serde_json::json!({"path": ws.0.join("inside.txt"), "content": "x"}),
            ),
            ("shell", serde_json::json!({"command": "ls"})),
            ("mcp__server__tool", serde_json::json!({})),
        ] {
            assert_eq!(
                rig.gate.approve(&call(name, args)),
                ApprovalDecision::Approve,
                "host allowed {name}"
            );
        }
        assert_eq!(rig.prompt_count(), 3, "each mutation asked the host once");
        assert_eq!(rig.gate.denial_reason(), None);
        // A host "deny" answer fails closed.
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("deny"));
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Deny
        );
        assert_eq!(rig.prompt_count(), 1);
    }

    #[test]
    fn full_auto_matrix() {
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("allow"));

        // Contained writes auto-approve — absolute AND relative spellings,
        // and fs_edit (pins the `path` argument name: a rename would flip
        // this to a prompt and fail the test).
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Approve
        );
        assert_eq!(
            rig.gate.approve(&call(
                "fs_edit",
                serde_json::json!({"path": ws.0.join("a.txt"), "old_string": "a", "new_string": "b"})
            )),
            ApprovalDecision::Approve,
            "contained fs_edit must auto-approve (arg-name regression guard)"
        );
        assert_eq!(
            rig.gate.approve(&call(
                "fs_write",
                serde_json::json!({"path": "relative-inside.txt", "content": "x"})
            )),
            ApprovalDecision::Approve,
            "relative contained path must auto-approve"
        );
        assert_eq!(rig.prompt_count(), 0);

        // Uncontained, unparseable, and protected paths fall through to the
        // host prompt — never silent approve, never silent deny. NOTE: the
        // uncontained fixtures anchor at the FILESYSTEM ROOT, not a tempdir
        // sibling — the workspace_write policy includes the tmp roots
        // (SlashTmp/Tmpdir entries), so a tempdir sibling is CONTAINED on
        // unix and this matrix would be testing nothing there.
        for (name, args) in [
            (
                "fs_write",
                serde_json::json!({"path": outside_root().join("escape.txt"), "content": "x"}),
            ),
            (
                "fs_write",
                // Enough `..` to saturate at the filesystem root from any
                // workspace depth (root-clamped on both platforms).
                serde_json::json!({"path": "../../../../../../../../nano-c2-escape.txt", "content": "x"}),
            ),
            (
                "fs_write",
                serde_json::json!({"path": ws.0.join(".git/config"), "content": "x"}),
            ),
            ("fs_write", serde_json::json!({"content": "missing path"})),
            ("fs_write", serde_json::json!({"path": 42, "content": "x"})),
            ("mcp__server__tool", serde_json::json!({})),
            ("future_unknown_tool", serde_json::json!({})),
        ] {
            let before = rig.prompt_count();
            assert_eq!(
                rig.gate.approve(&call(name, args)),
                ApprovalDecision::Approve,
                "host allowed {name} after the fall-through prompt"
            );
            assert_eq!(
                rig.prompt_count(),
                before + 1,
                "{name} must fall through to exactly one host prompt"
            );
        }

        // Shell behind a probed sandbox backend auto-approves; without one
        // it prompts (full containment is enforced at spawn regardless).
        assert_eq!(
            rig.gate
                .approve(&call("shell", serde_json::json!({"command": "ls"}))),
            ApprovalDecision::Approve
        );
        // Adversarial (claude concern 3): a shell call carrying would-be
        // sandbox-relaxing arguments gets NO different treatment — the gate
        // approves on backend availability alone, the spawn-time transform
        // sandboxes regardless, and the tool schema (pinned in the
        // nano-agent wiring tests) has no opt-out surface at all.
        let prompts_before = rig.prompt_count();
        assert_eq!(
            rig.gate.approve(&call(
                "shell",
                serde_json::json!({"command": "ls", "sandbox": "off", "escalate": true})
            )),
            ApprovalDecision::Approve,
            "extra args change nothing: sandboxing is enforced at spawn"
        );
        assert_eq!(rig.prompt_count(), prompts_before);
        let no_sandbox = TestGate::new(PermissionMode::FullAuto, &ws.0, false, Some("allow"));
        assert_eq!(
            no_sandbox
                .gate
                .approve(&call("shell", serde_json::json!({"command": "ls"}))),
            ApprovalDecision::Approve,
            "host allowed the prompt"
        );
        assert_eq!(no_sandbox.prompt_count(), 1);
    }

    #[test]
    fn de_escalation_is_immediate_escalation_never_is() {
        let ws = workspace();
        // Captured full_auto: the contained write auto-approves...
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("allow"));
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Approve
        );
        assert_eq!(rig.prompt_count(), 0);
        // ...flipping the shared cell to default tightens the NEXT check in
        // the SAME running turn...
        rig.set_mode(PermissionMode::Default);
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Approve,
            "host answered the prompt"
        );
        assert_eq!(
            rig.prompt_count(),
            1,
            "de-escalation took effect immediately"
        );
        // ...and flipping to read_only denies outright.
        rig.set_mode(PermissionMode::ReadOnly);
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Deny
        );
        assert_eq!(
            rig.gate.denial_reason(),
            Some("session is in read_only mode")
        );

        // The asymmetric direction: a turn that CAPTURED default never
        // escalates mid-turn, even with the cell at full_auto.
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("allow"));
        rig.set_mode(PermissionMode::FullAuto);
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Approve,
            "host answered the prompt"
        );
        assert_eq!(
            rig.prompt_count(),
            1,
            "mid-turn escalation must wait for the next prompt"
        );
    }

    #[test]
    fn cancel_denies_an_in_flight_prompt() {
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, None);
        let cancel = rig.gate.cancel.clone();
        let stop = rig.stop.clone();
        // Fire the cancel flag once the prompt is in flight (no responder
        // thread for this rig — answer None means no auto-answer; the
        // prompt must end via the cancel flag, not a host reply).
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            cancel.store(true, Ordering::SeqCst);
            stop.store(true, Ordering::SeqCst);
        });
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Deny,
            "cancel during an in-flight prompt denies it"
        );
        canceller.join().unwrap();
        assert_eq!(rig.prompt_count(), 1);
    }

    /// Create a directory link `link` -> `target`: NTFS junction on Windows
    /// (no privilege required), symlink elsewhere. Returns false when the
    /// platform refused — the caller then skips LOUDLY (a scenario whose
    /// subject is missing must fail, but a host that forbids link creation
    /// cannot run the scenario at all; same precedent as
    /// nano-tools/tests/adversarial_fs.rs).
    fn make_dir_link(link: &std::path::Path, target: &std::path::Path) -> bool {
        #[cfg(windows)]
        {
            return std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .expect("spawn mklink")
                .status
                .success();
        }
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(target, link).is_ok();
        }
        #[allow(unreachable_code)]
        false
    }

    /// C2 §9 oracle equivalence, SOUND version: on a STABLE filesystem the
    /// gate's full_auto decision equals the executor's verdict
    /// (`fs.rs:108`) on every case — equivalence of shared oracle inputs
    /// and results, never end-to-end write equality across mutation.
    #[test]
    fn gate_oracle_matches_executor_verdict_on_stable_snapshot() {
        use nano_tools::fs::FsTools;

        let ws = workspace();
        let policy = PermissionProfile::workspace_write().file_system_sandbox_policy();
        let tools = FsTools::new(policy.clone(), &ws.0);
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, false, Some("allow"));

        // A hard-link alias inside the workspace: denied by the policy's
        // fail-closed multi-link rule.
        let alias_target = ws.0.join("alias-target.txt");
        std::fs::write(&alias_target, "x").unwrap();
        let alias = ws.0.join("alias-name.txt");
        std::fs::hard_link(&alias_target, &alias).expect("hard link on this fs");

        // A pre-planted link escape into a POLICY-DENIED existing dir. The
        // target is the workspace's `.git` (read-only by the protected-
        // metadata rule on every platform) — NOT a tempdir sibling, which
        // the tmp-root write entries would make writable on unix.
        let git_dir = ws.0.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let link = ws.0.join("dir-link");
        let link_planted = make_dir_link(&link, &git_dir);

        let mut cases: Vec<(&str, std::path::PathBuf)> = vec![
            ("contained absolute", ws.0.join("inside.txt")),
            ("contained relative", "relative-inside.txt".into()),
            ("uncontained absolute", outside_root().join("escape.txt")),
            (
                "uncontained relative",
                "../../../../../../../../nano-c2-escape.txt".into(),
            ),
            ("protected .git subpath", ws.0.join(".git/config")),
            ("hard-link alias", alias.clone()),
        ];
        if link_planted {
            cases.push(("link escape", link.join("escape.txt")));
        } else {
            panic!("directory link creation refused on this host");
        }

        for (name, path) in cases {
            // The executor's authoritative verdict, computed on the same
            // stable snapshot from the same policy value.
            let executor_verdict = policy.can_write_path_with_cwd(&path, &ws.0);
            let before = rig.prompt_count();
            let decision = rig.gate.approve(&call(
                "fs_write",
                serde_json::json!({"path": path, "content": "x"}),
            ));
            let prompted = rig.prompt_count() == before + 1;
            if executor_verdict {
                assert_eq!(
                    decision,
                    ApprovalDecision::Approve,
                    "{name}: gate must auto-approve what the executor allows"
                );
                assert!(!prompted, "{name}: no prompt for a contained write");
                // And the executor really can write it (same verdict),
                // resolving relative spellings against the workspace
                // exactly as the real executor does (wiring.rs `resolve`).
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ws.0.join(&path)
                };
                assert!(
                    tools.write_file(&resolved, "x").is_ok(),
                    "{name}: executor agrees"
                );
            } else {
                // Gate falls through to the host (answered allow here) — it
                // must NEVER auto-approve what the executor would deny.
                assert!(prompted, "{name}: uncontained write must prompt");
                assert!(
                    tools.write_file(&path, "x").is_err(),
                    "{name}: executor denies"
                );
            }
        }
    }

    /// C2 §9 authority under mutation: the gate's advisory check runs on a
    /// STABLE snapshot; a junction planted BETWEEN approval and execution
    /// re-races the canonicalization — and the execution-time check must
    /// still deny. Raced-state inequality is EXPECTED in the safe
    /// direction (gate approved, executor denied), never forbidden.
    #[test]
    fn executor_stays_authoritative_when_the_fs_mutates_after_approval() {
        use nano_tools::fs::{FsTools, ToolError};

        let ws = workspace();
        let policy = PermissionProfile::workspace_write().file_system_sandbox_policy();
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, false, None);
        let target = ws.0.join("planted").join("escape.txt");
        // The escape target: the workspace's `.git`, read-only by the
        // protected-metadata rule on every platform. (A tempdir sibling is
        // NOT a denied target on unix — the workspace_write policy includes
        // the tmp roots, so the canonicalized path would be writable there.)
        let git_dir = ws.0.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        // 1. Stable snapshot: `planted` does not exist yet — the write is
        //    contained, so the full_auto gate approves without prompting.
        assert_eq!(
            rig.gate.approve(&call(
                "fs_write",
                serde_json::json!({"path": target, "content": "pwn"}),
            )),
            ApprovalDecision::Approve,
            "advisory check approves the in-root path"
        );
        assert_eq!(rig.prompt_count(), 0);

        // 2. Mutation: plant the junction AFTER approval.
        if !make_dir_link(&ws.0.join("planted"), &git_dir) {
            panic!("directory link creation refused on this host");
        }

        // 3. The execution-time check is the authority: it re-runs the
        //    canonicalization and MUST deny the escape.
        let tools = FsTools::new(policy, &ws.0);
        let err = tools
            .write_file(&target, "pwn")
            .expect_err("write through the planted junction must be denied");
        assert!(
            matches!(err, ToolError::WriteDenied(_)),
            "denied with the wrong variant: {err:?}"
        );
        assert!(
            !git_dir.join("escape.txt").exists(),
            "SECURITY HOLE: the write escaped through the planted junction"
        );
    }

    // ── C10 gate tests: plan posture + the question channel ─────────────

    fn question_call(options: &[&str]) -> ToolCall {
        call(
            "ask_user",
            serde_json::json!({
                "question": "Pick one",
                "options": options.iter().map(|l| serde_json::json!({"label": l})).collect::<Vec<_>>()
            }),
        )
    }

    /// C10 §3 matrix: under the ACTIVE posture, in EVERY C2 mode, a
    /// workspace write denies at the gate with no prompt and the plan-file
    /// write auto-approves with no prompt. Session tools auto-allow.
    #[test]
    fn plan_posture_matrix_in_every_mode() {
        let ws = workspace();
        for mode in PermissionMode::ALL {
            let rig = TestGate::new(mode, &ws.0, true, None);
            rig.plan.lock().unwrap().active = true;

            // Workspace write: categorical denial, NO prompt, reason names
            // the posture — in read_only, default, AND full_auto alike.
            assert_eq!(
                rig.gate.approve(&contained_write(&ws.0)),
                ApprovalDecision::Deny,
                "{mode:?}: workspace write must deny under the posture"
            );
            assert_eq!(
                rig.gate.approve(&call(
                    "fs_edit",
                    serde_json::json!({"path": ws.0.join("a.txt"), "old_string": "a", "new_string": "b"})
                )),
                ApprovalDecision::Deny,
                "{mode:?}: workspace edit must deny under the posture"
            );
            // The plan file itself: auto-approved, no prompt.
            let plan_file = rig.plan.lock().unwrap().plan_file().to_path_buf();
            assert_eq!(
                rig.gate.approve(&call(
                    "fs_write",
                    serde_json::json!({"path": plan_file, "content": "plan"})
                )),
                ApprovalDecision::Approve,
                "{mode:?}: the plan file is the one write exception"
            );
            assert_eq!(
                rig.prompt_count(),
                0,
                "{mode:?}: the posture never prompts — deny or auto-approve"
            );
            assert_eq!(
                rig.gate.denial_reason(),
                Some("plan mode is active: writes are restricted to the session's plan file")
            );

            // Session tools auto-allow in every mode (todo's write path is
            // journaled session state; the plan tools carry their own
            // semantics; ask_user IS the prompt).
            for tool in ["todo", "ask_user", "enter_plan_mode", "exit_plan_mode"] {
                assert_eq!(
                    rig.gate.approve(&call(tool, serde_json::json!({}))),
                    ApprovalDecision::Approve,
                    "{mode:?}: {tool} auto-allowed"
                );
            }
            assert_eq!(rig.prompt_count(), 0, "{mode:?}: no prompts at all");
        }
    }

    /// C10 §5: the happy path — a `selected` opt_{i} response resolves to
    /// the option LABEL (the wire carried only the minted id).
    #[test]
    fn ask_happy_path_resolves_the_label() {
        use nano_agent::turn::AskOutcome;
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("opt_1"));
        let outcome = rig.gate.ask(&question_call(&["Alpha", "Beta", "Gamma"]));
        assert_eq!(outcome, AskOutcome::Answered("Beta".to_string()));
        assert_eq!(rig.prompt_count(), 1, "exactly one question frame");
        // The pending entry was removed on exit.
        assert!(rig.gate.pending.lock().unwrap().is_empty());
    }

    /// C10 §5: dismiss (reject id / rejected outcome) and malformed or
    /// unknown-id responses are typed denials — fail closed, never a
    /// fabricated answer.
    #[test]
    fn ask_dismiss_and_unknown_id_deny() {
        use nano_agent::turn::AskOutcome;
        let ws = workspace();
        // The TUI's Esc path: selected + the reject id.
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("reject"));
        assert_eq!(
            rig.gate.ask(&question_call(&["A", "B"])),
            AskOutcome::Denied("dismissed by the user".into())
        );
        // An id outside the minted set: typed denial.
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("opt_9"));
        assert!(matches!(
            rig.gate.ask(&question_call(&["A", "B"])),
            AskOutcome::Denied(ref reason) if reason.contains("unknown option id")
        ));
        // The approval-shaped answer (allow) means nothing to a question.
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("allow"));
        assert!(matches!(
            rig.gate.ask(&question_call(&["A", "B"])),
            AskOutcome::Denied(_)
        ));
    }

    /// C10 §5 (Q3 RULED): the bounded timeout unblocks the turn, removes
    /// the pending-map entry, and a late answer has nowhere to land.
    #[test]
    fn ask_timeout_unblocks_and_clears_pending() {
        use nano_agent::turn::AskOutcome;
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, None);
        let mut q = question_call(&["A", "B"]);
        q.arguments["timeout_seconds"] = serde_json::json!(1);
        let start = std::time::Instant::now();
        let outcome = rig.gate.ask(&q);
        assert!(
            matches!(outcome, AskOutcome::Denied(ref r) if r.contains("timed out")),
            "{outcome:?}"
        );
        assert!(start.elapsed() >= std::time::Duration::from_secs(1));
        assert!(
            rig.gate.pending.lock().unwrap().is_empty(),
            "timeout removes the pending entry (a late answer drops in the reader's unknown-id arm)"
        );
    }

    /// C10 §5: session/cancel mid-question denies within a poll tick.
    #[test]
    fn ask_cancel_denies_within_a_tick() {
        use nano_agent::turn::AskOutcome;
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, None);
        let cancel = rig.gate.cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            cancel.store(true, Ordering::SeqCst);
        });
        let outcome = rig.gate.ask(&question_call(&["A", "B"]));
        assert_eq!(outcome, AskOutcome::Denied("cancelled".into()));
        canceller.join().unwrap();
        assert!(rig.gate.pending.lock().unwrap().is_empty());
    }

    /// A stub inner executor for SessionTools tests (never called by the
    /// session-tool arms).
    #[derive(Debug)]
    struct NoopExecutor;

    #[async_trait::async_trait]
    impl ToolExecutor for NoopExecutor {
        async fn execute(&self, call: &ToolCall) -> nano_agent::turn::ToolOutcome {
            nano_agent::turn::ToolOutcome {
                ok: true,
                output: format!("ran {}", call.name),
                progress: nano_agent::loop_protection::ProgressSignals::default(),
                error_kind: None,
            }
        }
    }

    /// C10 §3/§5: the plan exit ALWAYS asks — even under full_auto (a plan
    /// gate full-auto could blow through is theatre). Approval flips the
    /// posture off through the journaled transition; any other answer
    /// keeps it.
    #[test]
    fn full_auto_does_not_auto_approve_plan_exit() {
        let ws = workspace();
        let journal = ws.0.join("session.jsonl");
        let todos: Arc<Mutex<Vec<nano_session::op::TodoItem>>> = Arc::new(Mutex::new(Vec::new()));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // (a) full_auto + user picks "Keep planning": the QUESTION WAS
        //     ASKED (no auto-approve) and the posture stays.
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("opt_1"));
        rig.plan.lock().unwrap().active = true;
        let inner = NoopExecutor;
        let tools = crate::session_tools::SessionTools::new(
            &inner,
            &rig.gate,
            todos.clone(),
            rig.plan.clone(),
            journal.clone(),
            "test-session".into(),
        );
        let outcome = rt.block_on(tools.execute(&call("exit_plan_mode", serde_json::json!({}))));
        assert!(!outcome.ok, "revise is a typed error: {}", outcome.output);
        assert!(outcome.output.contains("Keep planning"));
        assert!(rig.plan.lock().unwrap().active, "posture stays on revise");
        assert_eq!(rig.prompt_count(), 1, "full_auto still asked the host");
        assert!(
            !nano_session::reader::read_journal(&journal)
                .map(|r| r
                    .envelopes
                    .iter()
                    .any(|e| matches!(e.op, Op::PlanSet { active: false })))
                .unwrap_or(false),
            "no exit transition journaled on revise"
        );

        // (b) approval: posture flips off, PlanSet(false) journaled.
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("opt_0"));
        rig.plan.lock().unwrap().active = true;
        let tools = crate::session_tools::SessionTools::new(
            &inner,
            &rig.gate,
            todos,
            rig.plan.clone(),
            journal.clone(),
            "test-session".into(),
        );
        let outcome = rt.block_on(tools.execute(&call("exit_plan_mode", serde_json::json!({}))));
        assert!(outcome.ok, "approved exit: {}", outcome.output);
        assert!(
            !rig.plan.lock().unwrap().active,
            "posture off after approval"
        );
        let report = nano_session::reader::read_journal(&journal).unwrap();
        assert!(
            report
                .envelopes
                .iter()
                .any(|e| matches!(e.op, Op::PlanSet { active: false })),
            "exit journaled"
        );
        assert_eq!(rig.prompt_count(), 1);
    }

    // ── F-C2-1: the reader-thread de-escalation relay ─────────────────────

    /// Drive reader_loop over a scripted stdin and collect what it forwarded.
    fn run_reader(script: &str, cell: Option<PermissionMode>) -> (Vec<Inbound>, CurrentMode) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Inbound>();
        let pending: PendingMap = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let current_cancel: Arc<Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>> =
            Arc::new(Mutex::new(None));
        let current_mode: CurrentMode = Arc::new(Mutex::new(cell.map(|m| Arc::new(Mutex::new(m)))));
        let current_tasks: Arc<Mutex<Option<Arc<nano_agent::tasks::TaskRegistry>>>> =
            Arc::new(Mutex::new(None));
        reader_loop(
            std::io::Cursor::new(script.as_bytes().to_vec()),
            tx,
            pending,
            current_cancel,
            current_mode.clone(),
            current_tasks,
        );
        // reader_loop returns at EOF; everything it sent is queued on rx.
        let mut forwarded = Vec::new();
        while let Ok(inbound) = rx.try_recv() {
            forwarded.push(inbound);
        }
        (forwarded, current_mode)
    }

    fn cell_value(cell: &CurrentMode) -> PermissionMode {
        *cell
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .expect("session mode cell")
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    fn set_mode_line(id: u64, mode: &str) -> String {
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"session/set_mode\",\"params\":{{\"sessionId\":\"s\",\"modeId\":\"{mode}\"}}}}\n"
        )
    }

    /// F-C2-1 regression (relay leg): a mid-park DE-escalation mutates the
    /// shared mode cell IMMEDIATELY — before the main loop could ever
    /// process the queued request — and the request is still forwarded so
    /// the main loop journals + acks it exactly once.
    #[test]
    fn reader_relays_de_escalation_into_the_mode_cell() {
        let (forwarded, cell) = run_reader(
            &set_mode_line(7, "read_only"),
            Some(PermissionMode::FullAuto),
        );
        assert_eq!(
            cell_value(&cell),
            PermissionMode::ReadOnly,
            "de-escalation must land in the cell mid-poll"
        );
        assert!(
            matches!(&forwarded[..], [Inbound::Request { method, .. }] if method == "session/set_mode"),
            "the request must still reach the main loop for journal+ack: {forwarded:?}"
        );
    }

    /// F-C2-1 regression (deferral leg): an escalation relays NOTHING — the
    /// running turn keeps its captured ceiling and the journal-first main
    /// loop remains the only path that raises the recorded mode.
    #[test]
    fn reader_never_relays_escalation() {
        let (forwarded, cell) = run_reader(
            &set_mode_line(7, "full_auto"),
            Some(PermissionMode::Default),
        );
        assert_eq!(
            cell_value(&cell),
            PermissionMode::Default,
            "escalation must NOT be relayed mid-poll"
        );
        assert_eq!(forwarded.len(), 1, "request still forwarded");

        // Lateral moves relay nothing either (only a strict de-escalation).
        let (_, cell) = run_reader(&set_mode_line(7, "default"), Some(PermissionMode::Default));
        assert_eq!(cell_value(&cell), PermissionMode::Default);
    }

    /// Fail-safe: an unknown mode id relays nothing (the main loop rejects
    /// it with a typed error later); no active session relays nothing and
    /// never panics.
    #[test]
    fn reader_relay_is_fail_safe_on_garbage_and_no_session() {
        let (forwarded, cell) =
            run_reader(&set_mode_line(7, "yolo"), Some(PermissionMode::FullAuto));
        assert_eq!(cell_value(&cell), PermissionMode::FullAuto);
        assert_eq!(forwarded.len(), 1, "unknown id still reaches the main loop");

        let (forwarded, _) = run_reader(&set_mode_line(7, "read_only"), None);
        assert_eq!(forwarded.len(), 1, "no session: forwarded, nothing relayed");
    }
}
