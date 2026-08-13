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
use nano_agent::steer::{EnqueueAck, SteerHandle};
use nano_agent::turn::{
    ApprovalDecision, ApprovalGate, ModelDriver, ToolExecutor, TurnEngine, TurnRobustness,
    TurnState, TypedError,
};
use nano_agent::wiring::{ProviderDriver, RealToolExecutor, v1_tool_definitions};
use nano_egress::client::EgressClient;
use nano_model::anthropic_messages::AnthropicMessagesClient;
use nano_model::flux_completions::FluxCompletionsClient;
use nano_model::provider_catalog::WireKind;
use nano_model::types::{ContentBlock, Message, ModelObservation, Role, ToolCall};
use nano_protocol::acp::{
    AvailableModel, JsonRpcNotification, JsonRpcResponse, NanoErrorExtras, PLAN_MODE_ID,
    acp_blocks_to_content_blocks, agent_capabilities, agent_message_chunk,
    attachment_missing_notice, budget_clamp_notice, budget_notice, budget_warn_notice,
    compaction_notice, error_presentation, param_inert_notice, prompt_result, rate_limit_notice,
    reconnect_notice, request_permission_request, request_question_request, session_load_result,
    session_modes_value, session_new_result, set_model_result, steer_dropped_notice,
    steer_queued_result, tool_call_diff, tool_call_done, tool_call_replay, tool_call_update,
    user_message_chunk,
};
use nano_protocol::permission_mode::PermissionMode;
use nano_session::NanoErrorKind;
use nano_session::SessionState;
use nano_session::op::{InputBlock, Op, OpEnvelope, TodoItem};
use nano_session::reader::read_journal;
use nano_tools::fs::FsTools;
use nano_tools::shell::ShellTool;
// ── LANE-A BOUNDARY (P2a) ────────────────────────────────────────────────
// Lane A owns: the §4 loader (nano-tools/src/image.rs), the §5 digest-keyed
// attachment blob store (nano-session/src/attachment_store.rs), the §6.3
// vision catalog (nano-model/src/vision_catalog.rs), and the seven new §7
// error kinds (NanoErrorKind::{ModelLacksVision, ImageInvalid,
// ImageUnsupportedFormat, ImageTooLarge, ImageTooMany, AttachmentMissing,
// AttachmentStoreError}). This module CONSUMES those APIs and defines none
// of them.
use nano_session::attachment_store::{
    AttachmentStore, BlobReadError, attachment_unavailable_placeholder, is_valid_digest,
};
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
    /// P3 §3.3: the session's single journal-append authority. EVERY append
    /// routes through it (turn sink, mode/plan/todo ops, rollups, hydration,
    /// elicitation, grants, compaction) — serialization is its one mutex.
    coordinator: Arc<nano_session::JournalCoordinator>,
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
    /// P1 §3.2: the session cost meter — Arc-shared into the turn engine
    /// (reservation/clamp) and every C6 child context (live rollup), and
    /// the budget authority behind warn/stop/grant. `None` = pre-P1
    /// unmetered posture.
    meter: Option<nano_agent::cost::CostMeter>,
    /// P2a §9.1: the sticky image-influenced flag — set whenever a turn's
    /// ASSEMBLED CONTEXT contains any `ContentBlock::Image` (current prompt
    /// or replayed history) OR an image-influenced compaction summary; OR'd
    /// with every observed `CompactionComplete.image_influenced` (§8 part 2
    /// flow-back); reconstructed on session/load as the sticky-OR over ALL
    /// journaled records plus uncompacted manifests. NEVER cleared
    /// mid-session: image-borne instructions can persist in summary text
    /// after pixels are gone. The §9.1 approval clamp reads it — protected
    /// trust mutations always take the human prompt while set, in EVERY
    /// mode including full_auto.
    image_influenced: Arc<std::sync::atomic::AtomicBool>,
    /// P4 §4.4: the session-owned PTY registry. Its Drop terminates every
    /// live session's direct-descendant tree; session replacement calls
    /// `terminate_all` explicitly (the tasks-registry discipline).
    pty: Arc<nano_tools::pty::PtySessionManager>,
}

/// P1 §3.3: the parent-journal rollup sink — appends
/// `Op::ChildUsageRollup` durably (journal-first at the reconciliation
/// boundary). P3 §3.3: it routes through the session's JournalCoordinator —
/// the single append authority — so a rollup can never interleave into a
/// compaction critical section. A retried append of the stable
/// `{task_id}-rollup-1` envelope id is idempotent (`Ok(false)` = already
/// durable).
fn child_rollup_sink(
    coordinator: Arc<nano_session::JournalCoordinator>,
) -> nano_agent::tasks::RollupSink {
    Arc::new(move |envelope: &OpEnvelope| -> bool {
        match coordinator.append(envelope) {
            Ok(_) => true,
            Err(err) => {
                eprintln!("wayland-nano: child usage rollup append failed: {err}");
                false
            }
        }
    })
}

/// F-36 (P3 §6.3) — THE OAuth `record_grant` producer. Without this hook the
/// login flow journals nothing and the replay surface
/// (`SessionState.mcp_oauth_grants`) is dead. The chain, in order:
///
/// 1. CHECKED conversion `flow::GrantEndpoint{HttpMethod}` →
///    `op::GrantEndpoint{GrantMethod}`: the journal vocabulary admits GET
///    and POST only; every other method converts to `GrantMethod::Unknown`
///    so the §6.3 validator REJECTS it (never silently journals an
///    unenforceable grant).
/// 2. `validate_oauth_grant` (the journal-side §6.3 bounds + the §2.7
///    instance-id shape on `server_id`, F-P3-3).
/// 3. Append `Op::McpOauthGrant` through the session's JournalCoordinator —
///    the single append authority. The envelope id IS the grant's
///    idempotence key, so a retried login's re-append is the coordinator's
///    `Ok(false)` (already durable), never a duplicate.
///
/// Any failure aborts the login BEFORE the scoped client is built (the
/// flow's journal-first ordering): validation rejections surface as
/// `grant_rejected`, append failures as `journal_unavailable`.
pub fn oauth_grant_recorder(
    coordinator: Arc<nano_session::JournalCoordinator>,
) -> impl Fn(&nano_mcp::oauth::flow::GrantRecord) -> Result<(), nano_mcp::oauth::OAuthError> {
    move |record| {
        use nano_mcp::oauth::{FailReason, OAuthError};
        let endpoints: Vec<nano_session::op::GrantEndpoint> = record
            .endpoints
            .iter()
            .map(|endpoint| nano_session::op::GrantEndpoint {
                method: match endpoint.method {
                    nano_egress::grant::HttpMethod::Get => nano_session::op::GrantMethod::Get,
                    nano_egress::grant::HttpMethod::Post => nano_session::op::GrantMethod::Post,
                    // Checked conversion: anything outside the journal's
                    // method vocabulary becomes Unknown, which
                    // validate_oauth_grant rejects below.
                    _ => nano_session::op::GrantMethod::Unknown,
                },
                path: endpoint.path.clone(),
            })
            .collect();
        nano_session::op::validate_oauth_grant(
            // §2.7 (F-P3-3): the grant's server_id MUST be the
            // registry-minted instance id — it keys credential storage
            // (§6: keyring account = instance id). The §6.1 CLI login
            // wiring (a separate finding) passes the receipt's
            // instance_id; a display-name key is refused fail-closed here.
            &record.server_id,
            &record.as_origin,
            &record.issuer,
            &endpoints,
        )
        .map_err(|rule| {
            eprintln!("wayland-nano: OAuth grant rejected ({rule})");
            OAuthError::Failed {
                reason: FailReason::GrantRejected,
            }
        })?;
        coordinator
            .append(&OpEnvelope::new(
                record.grant_id.clone(),
                "now",
                Op::McpOauthGrant {
                    grant_id: record.grant_id.clone(),
                    server_id: record.server_id.clone(),
                    as_origin: record.as_origin.clone(),
                    issuer: record.issuer.clone(),
                    endpoints,
                },
            ))
            .map_err(|err| {
                eprintln!("wayland-nano: OAuth grant journal append failed: {err}");
                OAuthError::Failed {
                    reason: FailReason::JournalUnavailable,
                }
            })?;
        Ok(())
    }
}

/// P1 §3.2: build the session meter when the host carries a pricing
/// catalog (else the pre-P1 unmetered posture). The pricing provider key
/// comes from the session model's namespace (bare ids are Flux, whose
/// vendored catalog id is `flux-router`); per turn the engine rebinds to
/// the resolved binding's provider (`CostMeter::with_provider`).
fn session_meter(
    pricing: &Option<std::sync::Arc<nano_model::pricing::PricingCatalog>>,
    budget_cap: Option<u64>,
    model: &str,
) -> Option<nano_agent::cost::CostMeter> {
    let catalog = pricing.clone()?;
    let provider = match crate::provider_router::ProviderRouter::parse_model_id(model) {
        Ok(crate::provider_router::ModelRef::Namespaced { provider, .. }) => provider,
        _ => "flux-router".to_string(),
    };
    Some(nano_agent::cost::CostMeter::new(
        provider, catalog, budget_cap,
    ))
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
    // C9 §4: sticky model params from the env config channel. Invalid
    // values are typed config errors naming the setting, never clamps.
    let reasoning_effort = match parse_env_effort() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return Ok(2);
        }
    };
    let verbosity = match parse_env_verbosity() {
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
    // P1: web_search — the key-gated ladder, resolved ONCE at host start
    // (design §2.3: mid-session re-resolution is out of scope). The
    // host-start stub handle exists ONLY to stand the Flux backend's
    // construction invariant up (it records nothing: the recording feed is
    // the per-turn executor sink below, which carries Lane B's session
    // CostMeter — and the unmetered fallback for test/cron postures).
    let search_meter: Arc<dyn nano_model::metering::UsageSink> =
        Arc::new(nano_model::metering::StubCostMeter::new());
    let search = crate::search_specs::web_search_tool_from_env(Some(search_meter.clone()));
    let search_tool = search.as_ref().map(|resolved| resolved.tool.clone());
    let tools_search = search_tool.clone();
    let tools_meter = search_meter.clone();
    let tools_attachment_home = nano_home.to_path_buf();
    let make_tools = move |workspace: &std::path::Path,
                           mode: PermissionMode,
                           plan_file: &std::path::Path,
                           diff_hook: Option<DiffHook>,
                           search_meter: Option<Arc<dyn nano_model::metering::UsageSink>>,
                           image_approver: Option<
        Arc<dyn nano_tools::image::ImageReadApprover>,
    >|
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
        // P1: web_search with the session meter handle (design §2.5) — the
        // caller's per-turn sink (the session CostMeter dual feed) when it
        // passes one; the host-start handle is the unmetered fallback.
        if let Some(tool) = &tools_search {
            let meter = search_meter.unwrap_or_else(|| tools_meter.clone());
            executor = executor.with_web_search(tool.clone(), meter);
        }
        if let Some(approver) = image_approver {
            match AttachmentStore::open(&tools_attachment_home) {
                Ok(store) => {
                    executor = executor.with_view_image(
                        nano_tools::image::ViewImageTool::new(policy.clone(), workspace, approver),
                        store,
                    );
                }
                Err(error) => {
                    eprintln!("wayland-nano: view_image attachment store unavailable: {error}")
                }
            }
        }
        // P4 §5.5 (F-28 wiring): the session-workspace repo map, built from
        // the SAME policy + cwd as the fs tools. A construction failure
        // leaves the slot empty — calls then fail typed, never silently.
        match nano_tools::repomap::RepoMapTool::new(&policy, workspace) {
            Ok(tool) => executor = executor.with_repo_map(tool),
            Err(error) => eprintln!("wayland-nano: repo_map index unavailable: {error}"),
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
    // P2a §6.3 (LANE-A BOUNDARY): the fail-closed static vision catalog —
    // vendored exact-id entries only; aliases are never blessed in v1. Lane
    // A's `with_tightening_overrides` hook exists for the tightening-only
    // `[model_capabilities]` config — Nano has NO config-file channel at
    // 4ca7700 (env vars only), so the vendored table is the whole runtime
    // picture; the hook is one line once a config channel lands.
    let vision_catalog = match nano_model::vision_catalog::VisionCatalog::vendored() {
        Ok(catalog) => catalog,
        Err(err) => {
            // Fail-closed, same posture as the pricing catalog load: a
            // corrupt vendored table is a typed startup error.
            eprintln!("wayland-nano: vision catalog unavailable: {err}");
            return Ok(2);
        }
    };
    // P2a §5.1: the digest-keyed attachment blob store root (opened lazily
    // at intake/resume; open carries the §5.5 permission audit).
    // P1 §3.1: the pricing catalog — FAIL-CLOSED: a malformed
    // NANO_PRICING_PATH override is a typed startup error naming the path,
    // never a silent fallback to bundled, never a partial parse.
    let pricing = match nano_model::pricing::PricingCatalog::load_default() {
        Ok(catalog) => Some(Arc::new(catalog)),
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return Ok(2);
        }
    };
    // P1 §4.1: the session token cap (unset = no cap, back-compat); a
    // malformed value is a typed startup error, never a silently-uncapped
    // session.
    let budget_cap = match nano_core::budget::session_token_cap_from_env() {
        Ok(cap) => cap,
        Err(err) => {
            eprintln!("wayland-nano: {err}");
            return Ok(2);
        }
    };
    // P2a §5.4 (F-34): the host-startup attachment-store GC sweep. Hygiene
    // only — a store/scan/sweep failure logs and defers to the next start,
    // NEVER blocks the host; the sweep never runs on a partial reference
    // set (scan failure ⇒ skip).
    startup_attachment_sweep(nano_home, &sessions);
    let config = ServeConfig {
        sessions_dir: &sessions,
        default_model: &default_model,
        available_models: &available,
        env_mcp_specs: &env_mcp_specs,
        catalog: &catalog_models,
        window_override,
        limit_override,
        reasoning_effort,
        verbosity,
        sandbox_probe: &sandbox_probe,
        router: &router,
        journal_append_failer: None,
        memory: &memory_config,
        cron_home: Some(nano_home),
        search: search.as_ref(),
        search_meter: Some(&search_meter),
        pricing,
        budget_cap,
        vision_catalog: &vision_catalog,
        attachment_home: nano_home,
    };
    serve(reader, writer, &config, make_driver, make_tools).await
}

/// P2a §5.4 (F-34): the host-startup attachment-store GC sweep. Hygiene
/// only — every failure mode logs and defers to the next startup, NEVER
/// blocks the host. The sweep runs ONLY on a complete reference set: a
/// scan failure (unreadable journal) skips the sweep entirely (a partial
/// set would reap live references).
fn startup_attachment_sweep(nano_home: &std::path::Path, sessions_dir: &std::path::Path) {
    let store = match AttachmentStore::open(nano_home) {
        Ok(store) => store,
        Err(err) => {
            eprintln!("wayland-nano: attachment GC skipped (store unavailable): {err}");
            return;
        }
    };
    let referenced = match nano_session::attachment_store::referenced_blob_digests(sessions_dir) {
        Ok(referenced) => referenced,
        Err(err) => {
            eprintln!("wayland-nano: attachment GC skipped (journal reference scan failed: {err})");
            return;
        }
    };
    match store.sweep(&referenced) {
        Ok(report) if report.lock_skipped => {
            // A writer holds the lease: the typed skip is the §5.4
            // discipline, not an error.
        }
        Ok(report) => {
            if report.removed_blobs > 0 || report.removed_staging > 0 {
                eprintln!(
                    "wayland-nano: attachment GC reclaimed {} bytes ({} blobs, {} staging files)",
                    report.reclaimed_bytes, report.removed_blobs, report.removed_staging
                );
            }
        }
        Err(err) => eprintln!("wayland-nano: attachment GC failed: {err}"),
    }
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

/// NANO_REASONING_EFFORT (C9 §4): low|medium|high, sticky per session. An
/// out-of-vocabulary value is a typed config error naming the setting.
fn parse_env_effort() -> Result<Option<nano_model::types::ReasoningEffort>, String> {
    crate::model_params::effort_from_env()
}

/// NANO_VERBOSITY (C9 §4): low|medium|high, sticky per session.
fn parse_env_verbosity() -> Result<Option<nano_model::types::Verbosity>, String> {
    crate::model_params::verbosity_from_env()
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
    /// C9 §4: the session's sticky reasoning effort / verbosity, applied to
    /// every turn's model request through the Q3 capability ladder. `None`
    /// = the params never leave the config channel.
    pub reasoning_effort: Option<nano_model::types::ReasoningEffort>,
    pub verbosity: Option<nano_model::types::Verbosity>,
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
    pub journal_append_failer: Option<&'a (dyn Fn() -> bool + Send + Sync)>,
    /// C5: cross-session memory. Read/injection is always available over the
    /// user-managed store; the write tools exist only when the operator
    /// opted in (NANO_MEMORY_WRITE).
    pub memory: &'a MemoryHostConfig,
    /// C11: when Some, the session-alive-only cron runner ticks inside this
    /// host (30s, wcore TICK_INTERVAL parity), firing due jobs through the
    /// §5.4 transaction. `None` in tests and headless profiles.
    pub cron_home: Option<&'a std::path::Path>,
    /// P1: the resolved web_search surface and its meter-handle fallback
    /// for UNMETERED sessions (the session CostMeter is the real sink at
    /// every metered site — session setup and the per-turn executor).
    /// `None` = unregistered — the advertised surface and every child match.
    pub search: Option<&'a crate::search_specs::ResolvedSearch>,
    pub search_meter: Option<&'a Arc<dyn nano_model::metering::UsageSink>>,
    /// P1 §3.1: the pricing catalog (loaded at startup, fail-closed). When
    /// present, every session gets a cost meter; `None` = the pre-P1
    /// unmetered posture (tests).
    pub pricing: Option<std::sync::Arc<nano_model::pricing::PricingCatalog>>,
    /// P1 §4.1: the session token cap (`NANO_BUDGET_SESSION_TOKENS`).
    /// `None` = no cap (back-compat).
    pub budget_cap: Option<u64>,
    /// P2a §6.3/§6.4 (LANE-A BOUNDARY): the fail-closed static vision
    /// catalog — exact-id keys, aliases never blessed, tightening-only
    /// overrides already applied. Drives the §6.2 rung-1 per-prompt gate and
    /// the initialize-scoped advisory `promptCapabilities.image`; the §6.2
    /// rung-3 pre-dispatch gate consults the vendored table engine-side.
    pub vision_catalog: &'a nano_model::vision_catalog::VisionCatalog,
    /// P2a §5.1 (LANE-A BOUNDARY): the nano-home root the attachment blob
    /// store opens (store root = `<nano_home>/attachments`, owned by lane
    /// A's `AttachmentStore::open`). Opened lazily at intake/resume — open
    /// carries the §5.5 permission audit, fail-closed.
    pub attachment_home: &'a std::path::Path,
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
    // Send + Sync: the C11 cron tick shares the factories with the prompt
    // path and runs its fires through the async (Send-bound) executor trait.
    FD: Fn(&crate::provider_router::ProviderBinding) -> D + Send + Sync + 'static,
    FT: Fn(
            &std::path::Path,
            PermissionMode,
            &std::path::Path,
            Option<DiffHook>,
            Option<Arc<dyn nano_model::metering::UsageSink>>,
            Option<Arc<dyn nano_tools::image::ImageReadApprover>>,
        ) -> (T, nano_core::permissions::FileSystemSandboxPolicy)
        + Send
        + Sync,
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
    // C9 §3.3: the running turn's steer queue. Replaced per prompt, cleared
    // at turn end; the session/steer handler enqueues through it and
    // resolves IMMEDIATELY with the ack.
    let current_steer: Arc<Mutex<Option<SteerHandle>>> = Arc::new(Mutex::new(None));
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
    // C11: the session-alive-only cron runner ticks here (30s, wcore
    // TICK_INTERVAL parity). A corrupt job store disables the scheduler for
    // the process lifetime (fail-closed, Q6) — the session is unaffected.
    let mut cron_interval = config
        .cron_home
        .map(|_| tokio::time::interval(std::time::Duration::from_secs(30)));
    let mut cron_disabled = false;
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
                            // P2a §6.4/D4: image capability advertises from
                            // the configured STARTUP leaf — initialize-scoped,
                            // advisory only, stale after session/set_model;
                            // the rung-1/3 gates never trust it.
                            write_out(
                                &out,
                                &JsonRpcResponse::ok(
                                    id,
                                    agent_capabilities(config.default_model, config.vision_catalog),
                                ),
                            )?;
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
                            // P3 §3.3: the JournalCoordinator is created here,
                            // beside the session state, and owns EVERY later
                            // append to this journal.
                            let coordinator =
                                match nano_session::JournalCoordinator::open(&journal) {
                                    Ok(coordinator) => Arc::new(coordinator),
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
                            let journaled = coordinator.append(&OpEnvelope::new(
                                format!("{session_id}-begin-1"),
                                "now",
                                Op::SessionBegin {
                                    session_id: session_id.clone(),
                                    cwd: cwd.display().to_string(),
                                },
                            ));
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
                            // P3 §5.2: the elicitation bridge factory is
                            // installed BEFORE any server connection opens —
                            // the capability is advertised only because a
                            // handler exists.
                            let elicitation = Some(elicitation_factory(
                                out.clone(),
                                pending.clone(),
                                permission_ids.clone(),
                                session_id.clone(),
                                cancel.clone(),
                                coordinator.clone(),
                            ));
                            let mcp =
                                session_mcp_registry(&params, config.env_mcp_specs, elicitation);
                            // P3 §3.1: bounded, loud startup warnings.
                            mcp_session_notices(&out, &session_id, &mcp, None)?;
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
                                // P4 §4.4: and its PTY sessions (explicit;
                                // Drop is the backstop).
                                old.pty.terminate_all();
                            }
                            // P1 §3.2/§4.1: the session cost meter (None
                            // when the host carries no pricing catalog).
                            let meter = session_meter(
                                &config.pricing,
                                config.budget_cap,
                                config.default_model,
                            );
                            let image_influenced =
                                Arc::new(std::sync::atomic::AtomicBool::new(false));
                            let mut registry = nano_agent::tasks::TaskRegistry::new(
                                &task_nano_home,
                                &cwd,
                                config.default_model.to_string(),
                                make_task_driver_factory(config.default_model),
                            )
                            .with_image_influence(image_influenced.clone());
                            // P1 (D12): children inherit the session search
                            // chain — advertised exactly like the parent
                            // surface and metered by the SESSION CostMeter
                            // (the configured handle is the unmetered
                            // fallback only).
                            if let Some(resolved) = config.search {
                                let sink = match &meter {
                                    Some(meter) => Some(Arc::new(meter.clone())
                                        as Arc<dyn nano_model::metering::UsageSink>),
                                    None => config.search_meter.cloned(),
                                };
                                if let Some(sink) = sink {
                                    registry = registry
                                        .with_web_search(resolved.tool.clone(), sink);
                                }
                            }
                            // P1 §3.3: metered sessions Arc-share the meter
                            // into every child context AND journal durable
                            // per-child rollups into the parent journal.
                            let tasks = Arc::new(match &meter {
                                Some(meter) => registry.with_meter(
                                    meter.clone(),
                                    child_rollup_sink(coordinator.clone()),
                                    session_id.clone(),
                                ),
                                None => registry,
                            });
                            *current_tasks.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(tasks.clone());
                            // P4 §4.4: the session-owned PTY registry (fresh
                            // per session; never shared across sessions).
                            let pty =
                                Arc::new(nano_tools::pty::PtySessionManager::new(&cwd));
                            session = Some(Session {
                                id: session_id.clone(),
                                workspace: cwd,
                                cancel,
                                journal,
                                coordinator,
                                turn_counter: 0,
                                context,
                                model: config.default_model.to_string(),
                                mode: mode_cell,
                                plan,
                                todos,
                                mode_changes: 0,
                                mcp,
                                tasks,
                                meter,
                                // P2a §9.1: a fresh session starts clean.
                                image_influenced,
                                pty,
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
                            // P3 §3.3: the coordinator owns every append from
                            // here on, including this resume marker.
                            let coordinator =
                                match nano_session::JournalCoordinator::open(&journal) {
                                    Ok(coordinator) => Arc::new(coordinator),
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
                            {
                                let _ = coordinator.append(&OpEnvelope::new(
                                    format!("{session_id}-begin-{}", begin_count + 1),
                                    "now",
                                    Op::SessionBegin {
                                        session_id: session_id.to_string(),
                                        cwd: cwd.display().to_string(),
                                    },
                                ));
                            }
                            // P2a §5.3: rehydrate image manifests from the
                            // blob store (opened, with its fail-closed §5.5
                            // audit, only when the journal references one);
                            // every degradation is a loud placeholder PLUS a
                            // session/update notice (test-asserted).
                            let attachment_store = if journal_has_image_manifests(
                                &report.envelopes,
                            ) {
                                match AttachmentStore::open(config.attachment_home) {
                                    Ok(store) => Some(store),
                                    Err(err) => {
                                        write_out(
                                            &out,
                                            &JsonRpcResponse::err_typed(
                                                id,
                                                NanoErrorKind::AttachmentStoreError,
                                                format!("attachment store unavailable: {err}"),
                                                NanoErrorExtras::default(),
                                            ),
                                        )?;
                                        continue;
                                    }
                                }
                            } else {
                                None
                            };
                            let (context_messages, attachment_issues) =
                                messages_from_envelopes_rehydrating(
                                    &report.envelopes,
                                    attachment_store.as_ref(),
                                );
                            for issue in &attachment_issues {
                                write_out(
                                    &out,
                                    &attachment_missing_notice(session_id, issue.cause.as_str(), &issue.digest_prefix),
                                )?;
                            }
                            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            *current_cancel.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(cancel.clone());
                            let mode_cell = Arc::new(Mutex::new(PermissionMode::default()));
                            *current_mode.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(mode_cell.clone());
                            // P3 §5.2: same bridge-before-connect wiring as
                            // session/new.
                            let elicitation = Some(elicitation_factory(
                                out.clone(),
                                pending.clone(),
                                permission_ids.clone(),
                                session_id.to_string(),
                                cancel.clone(),
                                coordinator.clone(),
                            ));
                            let mcp =
                                session_mcp_registry(&params, config.env_mcp_specs, elicitation);
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
                            // re-injected as a bounded block on rebuild. P1:
                            // ONE fold also feeds the meter reseed below.
                            let folded = SessionState::fold(&report.envelopes);
                            // P3 §3.4: the reconnect digest gate — hydrated
                            // sets re-apply only on a canonical-digest match;
                            // mismatches drop with a typed notice, and the
                            // churn breaker can pin a server Deferred for the
                            // session off the carried window.
                            mcp_session_notices(&out, session_id, &mcp, Some(&folded))?;
                            let todos = Arc::new(Mutex::new(folded.todos.clone()));
                            let mut context = session_context_prefix(&cwd, &todos, &plan);
                            context.extend(context_messages);
                            // C6: replacing a live session tears its children
                            // down first (bounded, then detach).
                            if let Some(old) = session.take() {
                                old.tasks.teardown_all();
                                // P4 §4.4: and its PTY sessions (explicit;
                                // Drop is the backstop).
                                old.pty.terminate_all();
                            }
                            // P1 §3.3/§4.3: reconstruct the exact budget
                            // position — meter totals from TurnEnd.usage +
                            // ChildUsageRollup + the crash-recovery orphan
                            // fold; grants replayed from Op::BudgetGrant.
                            let meter = session_meter(
                                &config.pricing,
                                config.budget_cap,
                                config.default_model,
                            );
                            if let Some(meter) = &meter {
                                let mut usage = folded.session_usage.clone();
                                usage.add_sum(&nano_agent::tasks::fold_orphan_child_usage(
                                    &task_nano_home,
                                    session_id,
                                    &folded.rollup_task_ids,
                                ));
                                meter.reseed(&usage, folded.budget_granted_tokens);
                            }
                            let image_influenced = Arc::new(
                                std::sync::atomic::AtomicBool::new(
                                    image_influenced_from_envelopes(&report.envelopes),
                                ),
                            );
                            let mut registry = nano_agent::tasks::TaskRegistry::new(
                                &task_nano_home,
                                &cwd,
                                config.default_model.to_string(),
                                make_task_driver_factory(config.default_model),
                            )
                            .with_image_influence(image_influenced.clone());
                            // P1 (D12): children inherit the session search
                            // chain — advertised exactly like the parent
                            // surface and metered by the SESSION CostMeter
                            // (the configured handle is the unmetered
                            // fallback only).
                            if let Some(resolved) = config.search {
                                let sink = match &meter {
                                    Some(meter) => Some(Arc::new(meter.clone())
                                        as Arc<dyn nano_model::metering::UsageSink>),
                                    None => config.search_meter.cloned(),
                                };
                                if let Some(sink) = sink {
                                    registry = registry
                                        .with_web_search(resolved.tool.clone(), sink);
                                }
                            }
                            let tasks = Arc::new(match &meter {
                                Some(meter) => registry.with_meter(
                                    meter.clone(),
                                    child_rollup_sink(coordinator.clone()),
                                    session_id.to_string(),
                                ),
                                None => registry,
                            });
                            *current_tasks.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(tasks.clone());
                            // P4 §4.4: the session-owned PTY registry (a
                            // resumed session starts with NO live PTYs —
                            // processes never survive the owning host).
                            let pty =
                                Arc::new(nano_tools::pty::PtySessionManager::new(&cwd));
                            session = Some(Session {
                                id: session_id.to_string(),
                                workspace: cwd,
                                cancel,
                                journal,
                                coordinator,
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
                                meter,
                                // P2a §8 part 2 / §9.1: reconstructed from
                                // the journal — sticky-OR over ALL journaled
                                // CompactionComplete.image_influenced records
                                // plus any UNCOMPACTED image-bearing
                                // manifest. Never the latest record: a
                                // false-negative record cannot reopen the
                                // clamp on resume.
                                image_influenced,
                                pty,
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
                                    &active.coordinator,
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
                            let journaled = active.coordinator.append(&envelope);
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
                                    &active.coordinator,
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
                        // C9 Q1 RULED shape (b): mid-turn steer rides this
                        // extension method; mid-turn session/prompt keeps
                        // its byte-identical -32602 rejection below. The
                        // ack resolves IMMEDIATELY — it is NOT the turn
                        // result. Clients discover the method from the
                        // advertised nanoExtensions capability, never by
                        // probing; older hosts fall back to -32601.
                        "session/steer" => {
                            let Some(active) = session.as_ref() else {
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
                            let text = params
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string();
                            if text.is_empty() {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err(
                                        id,
                                        -32602,
                                        "session/steer requires a non-empty text",
                                    ),
                                )?;
                                continue;
                            }
                            let handle = current_steer
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .clone();
                            // The submitter identity is the wire request id:
                            // a later drop-on-cancel notice addresses it.
                            let submitter = id.to_string();
                            let _ = active; // session presence validated above
                            match handle {
                                Some(handle) => match handle.enqueue(submitter, text) {
                                    EnqueueAck::Queued { position } => {
                                        write_out(
                                            &out,
                                            &JsonRpcResponse::ok(
                                                id,
                                                steer_queued_result(position),
                                            ),
                                        )?;
                                    }
                                    EnqueueAck::RejectedClosed => {
                                        write_out(
                                            &out,
                                            &JsonRpcResponse::err(
                                                id,
                                                -32602,
                                                "steer queue closed",
                                            ),
                                        )?;
                                    }
                                    EnqueueAck::RejectedFull => {
                                        write_out(
                                            &out,
                                            &JsonRpcResponse::err(
                                                id,
                                                -32602,
                                                "steer queue full",
                                            ),
                                        )?;
                                    }
                                },
                                // No turn in flight: the queue is closed.
                                None => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err(
                                            id,
                                            -32602,
                                            "steer queue closed",
                                        ),
                                    )?;
                                }
                            }
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
                            // P2a §2.3: the text-only extractor (which
                            // silently dropped non-text blocks) is REPLACED
                            // by the typed converter — order and multiplicity
                            // preserved exactly; unknown block types
                            // typed-reject naming the tag only (§14
                            // deviation 8); image blocks are validated and
                            // re-encoded by the §4 loader (claimed MIME is a
                            // HINT only); `image_path` extension blocks
                            // (§3.1, the TUI attach path) resolve through
                            // the same host-side loader. The FIRST invalid
                            // block aborts the whole prompt.
                            let prompt_parts = params
                                .get("prompt")
                                .and_then(|p| p.as_array())
                                .cloned()
                                .unwrap_or_default();
                            let turn_input = match acp_blocks_to_content_blocks(
                                &prompt_parts,
                                &active.workspace,
                            )
                            .await
                            {
                                Ok(input) => input,
                                Err(rejection) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            rejection.kind,
                                            rejection.message,
                                            NanoErrorExtras::default(),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            // P2a §6.2 rung 1 (the load-bearing rung): an
                            // image-bearing prompt against a current leaf
                            // that is not vision-proven in the §6.3 catalog
                            // is typed-rejected BEFORE the turn starts and
                            // BEFORE any byte leaves the egress path — zero
                            // network I/O. The TUI may pre-check for UX
                            // immediacy, but THIS host check is
                            // authoritative — the TUI is not a trust
                            // boundary, and Desktop exempts ACP agents from
                            // its own vision gate.
                            if turn_input.has_images()
                                && !config.vision_catalog.image_in(&active.model)
                            {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::ModelLacksVision,
                                        format!(
                                            "model_lacks_vision: {} cannot process images. Switch to a vision-capable model (/model).",
                                            active.model
                                        ),
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            }
                            // P2a §5.1: publish the re-encoded blobs BEFORE
                            // the turn journals their manifest references.
                            // The whole span — staging write → rename →
                            // journal-append fsync — runs under the shared
                            // GC write lease (§5.4): a sweep can never
                            // observe, let alone delete, a published-but-
                            // not-yet-referenced blob. The lease guard rides
                            // the turn sink below and is dropped once the
                            // TurnBegin append is durable.
                            let mut attach_lease = None;
                            if turn_input.has_images() {
                                let store = match AttachmentStore::open(config.attachment_home) {
                                    Ok(store) => store,
                                    Err(err) => {
                                        write_out(
                                            &out,
                                            &JsonRpcResponse::err_typed(
                                                id,
                                                NanoErrorKind::AttachmentStoreError,
                                                format!("attachment store unavailable: {err}"),
                                                NanoErrorExtras::default(),
                                            ),
                                        )?;
                                        continue;
                                    }
                                };
                                let lease = match store.acquire_write_lease() {
                                    Ok(lease) => lease,
                                    Err(err) => {
                                        write_out(
                                            &out,
                                            &JsonRpcResponse::err_typed(
                                                id,
                                                NanoErrorKind::AttachmentStoreError,
                                                format!("attachment store lock failed: {err}"),
                                                NanoErrorExtras::default(),
                                            ),
                                        )?;
                                        continue;
                                    }
                                };
                                let mut publish_failed = None;
                                for block in &turn_input.blocks {
                                    let nano_agent::turn_input::TurnBlock::Image {
                                        reference,
                                        data,
                                    } = block
                                    else {
                                        continue;
                                    };
                                    use base64::Engine;
                                    let bytes = match base64::engine::general_purpose::STANDARD
                                        .decode(data)
                                    {
                                        Ok(bytes) => bytes,
                                        Err(_) => {
                                            publish_failed = Some(
                                                "image block data is not valid base64".to_string(),
                                            );
                                            break;
                                        }
                                    };
                                    // The store content-addresses the bytes;
                                    // the returned digest MUST equal the
                                    // loader-computed one (defense in depth).
                                    match store.put(&lease, &bytes) {
                                        Ok(digest) if digest == reference.digest => {}
                                        Ok(_) => {
                                            publish_failed = Some(
                                                "attachment digest mismatch at publish"
                                                    .to_string(),
                                            );
                                            break;
                                        }
                                        Err(err) => {
                                            publish_failed =
                                                Some(format!("attachment publish failed: {err}"));
                                            break;
                                        }
                                    }
                                }
                                if let Some(message) = publish_failed {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::AttachmentStoreError,
                                            message,
                                            NanoErrorExtras::default(),
                                        ),
                                    )?;
                                    continue;
                                }
                                attach_lease = Some(lease);
                            }
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
                            // P3 §3.3: the session's coordinator (opened
                            // fail-closed at session/new|load) is the turn's
                            // append authority — no per-turn writer.
                            let turn_coordinator = active.coordinator.clone();
                            // C11 §3.1/§6: the SessionGuard is the ONE
                            // exclusion mechanism — interactive turns hold it
                            // for the turn's lifetime so a fork or cron fire
                            // can never interleave on this session (the "turn
                            // in progress" check above is its fast-path
                            // reflection). Busy is a typed error.
                            let turn_guard = match nano_agent::bootstrap::session_guard_registry()
                                .try_acquire(&active.journal)
                            {
                                Ok(guard) => guard,
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::TurnInProgress,
                                            format!("session busy: {err}"),
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
                            // P2a §9.1: the sticky session flag is set
                            // whenever THIS turn's assembled context carries
                            // any image — the current prompt OR the replayed
                            // history (post-rehydration). Sticky for the
                            // rest of the session; compaction never clears
                            // it (§8 part 2 provenance).
                            if turn_input.has_images()
                                || nano_agent::image_influence::history_image_influence(
                                    &prior_context,
                                    &nano_agent::image_influence::ReplayManifestState::from_presence(
                                        active.image_influenced.load(Ordering::SeqCst),
                                    ),
                                )
                            {
                                active.image_influenced.store(true, Ordering::SeqCst);
                            }
                            let image_influenced_cell = active.image_influenced.clone();
                            let image_influenced_before =
                                image_influenced_cell.load(Ordering::SeqCst);
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
                            // P4 §4.4: the session's PTY registry — the
                            // turn's executor routes pty_* calls through it
                            // without taking ownership.
                            let turn_pty = active.pty.clone();
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
                            // The session's task registry (C6): the turn's
                            // executor routes task_* calls through it.
                            let turn_tasks = active.tasks.clone();
                            // P1 §3.2/§4.2: the session meter, rebound per
                            // turn to the resolved binding's pricing
                            // provider (an absent row prices unpriced —
                            // honest, never a wrong price).
                            let turn_meter = active
                                .meter
                                .clone()
                                .map(|meter| meter.with_provider(&binding.provider_id));
                            // C9 §3.2/§3.3: the turn's steer queue, bound to
                            // THIS turn id. The drop closure translates each
                            // still-queued steer at close into exactly one
                            // later session/update notice per submitter
                            // (request id + text digest, never the text).
                            let drop_out = out.clone();
                            let drop_session = active.id.clone();
                            let steer_handle = SteerHandle::new(
                                &turn_id,
                                nano_agent::steer::DEFAULT_CAPACITY,
                                Arc::new(move |item| {
                                    let notice = steer_dropped_notice(
                                        &drop_session,
                                        &item.submitter,
                                        &format!("len:{}", item.text.len()),
                                    );
                                    let mut guard =
                                        drop_out.lock().unwrap_or_else(|p| p.into_inner());
                                    let _ = write_json(&mut *guard, &notice);
                                }),
                            );
                            *current_steer.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(steer_handle.clone());
                            // C9 §4: the session's sticky params, captured
                            // per turn like the model pick.
                            let turn_effort = config.reasoning_effort;
                            let turn_verbosity = config.verbosity;
                            // The turn future must own its handles: clone the
                            // loop-invariant Arcs before the `async move`.
                            let gate_out = out.clone();
                            let gate_pending = pending.clone();
                            let gate_ids = permission_ids.clone();
                            let sink_out = out.clone();
                            let budget_out = out.clone();
                            let observer_out = out.clone();
                            let sandbox_probe = config.sandbox_probe;
                            let memory_dir = config.memory.dir.clone();
                            let memory_write = config.memory.write_enabled;
                            let make_driver = make_driver.clone();
                            let make_tools = &make_tools;
                            // P2a: move the §5.4 GC lease guard (held until
                            // TurnBegin is durably journaled), the §9.1 flag
                            // cell (gate clamp + §8 flow-back), and the
                            // authoritative TurnInput into the turn.
                            let mut attach_lease = attach_lease;
                            let gate_image_influenced = image_influenced_cell.clone();
                            let turn_future = async move {
                                // Held for the turn's lifetime (C11): forks
                                // and cron fires see typed busy.
                                let _guard = turn_guard;
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
                                // P1 §2.5/§3.2: the turn's search sink is
                                // Lane B's session CostMeter in the r3
                                // codex-F1 dual feed — one grounding record
                                // charges the meter AND lands in this turn's
                                // accumulator cell (drained into
                                // TurnEnd.usage before terminal journaling).
                                let extra_usage = turn_meter.as_ref().map(|_| {
                                    Arc::new(Mutex::new(
                                        nano_session::op::TurnUsage::default(),
                                    ))
                                });
                                let search_sink = match (&turn_meter, &extra_usage) {
                                    (Some(meter), Some(cell)) => {
                                        Some(Arc::new(nano_agent::cost::MeteringTurnSink::new(
                                            meter.clone(),
                                            cell.clone(),
                                        ))
                                            as Arc<dyn nano_model::metering::UsageSink>)
                                    }
                                    _ => None,
                                };
                                let vision_backed = binding.wire == WireKind::AnthropicMessages
                                    && nano_model::vision_catalog::VisionCatalog::vendored()
                                        .map(|catalog| catalog.image_in(&turn_model))
                                        .unwrap_or(false);
                                let image_approver = vision_backed.then(|| {
                                    Arc::new(AcpImageReadApprover {
                                        session_id: session_id.clone(),
                                        out: gate_out.clone(),
                                        pending: gate_pending.clone(),
                                        next_id: gate_ids.clone(),
                                        cancel: cancel.clone(),
                                    }) as Arc<dyn nano_tools::image::ImageReadApprover>
                                });
                                let (tools, turn_policy) = make_tools(
                                    &workspace,
                                    turn_mode,
                                    &plan_file,
                                    Some(diff_hook),
                                    search_sink,
                                    image_approver,
                                );
                                // MCP-merged executor: mcp__ names route to the
                                // session registry, everything else to the core
                                // tools; the model sees both tool sets.
                                let mcp_executor =
                                    McpToolExecutor::from_shared(turn_mcp.clone(), &tools);
                                let mut tool_definitions = v1_tool_definitions(
                                    config.search.is_some(),
                                    vision_backed && tools.image_results_backed(),
                                );
                                tool_definitions
                                    .extend(mcp_executor.tool_definitions_from_registry());
                                // P3 §3.2/§4.3: the MCP session tools are
                                // advertised ONLY when the live registry
                                // warrants them — tool_search iff a deferred
                                // inventory exists (its description carries
                                // the bounded §3.5 source listing), the
                                // resource pair iff a server negotiated the
                                // resources capability.
                                {
                                    let registry =
                                        turn_mcp.lock().unwrap_or_else(|p| p.into_inner());
                                    let listing = registry
                                        .has_deferred_tools()
                                        .then(|| registry.deferred_source_listing());
                                    let resources_available =
                                        registry.has_resources_capability();
                                    drop(registry);
                                    tool_definitions.extend(
                                        nano_agent::wiring::mcp_session_tool_definitions(
                                            listing.as_deref(),
                                            resources_available,
                                        ),
                                    );
                                }
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
                                    // P2a §9.1: the image-influenced clamp —
                                    // protected trust mutations ALWAYS take
                                    // the human prompt while set, in every
                                    // mode including full_auto.
                                    image_influenced: gate_image_influenced.clone(),
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
                                    turn_coordinator.clone(),
                                    session_id.clone(),
                                );
                                // P4 §4.3/§4.4: the session-owned PTY tools
                                // route to the session's PtySessionManager
                                // (ownership by construction). The gate's
                                // explicit pty_spawn arm is the
                                // authorization; this layer NEVER sees a
                                // child surface (children are built from
                                // v1_tool_definitions, which never carries
                                // the pty names).
                                let executor = crate::session_tools::PtyToolExecutor::new(
                                    &executor, turn_pty,
                                );
                                tool_definitions
                                    .extend(nano_tools::pty::pty_tool_definitions());
                                // P3 §3.2/§4.3: the MCP session tools
                                // (tool_search / mcp_list_resources /
                                // mcp_read_resource) ride the shared registry
                                // journal-first through the session's
                                // coordinator.
                                let executor =
                                    nano_agent::mcp_session_tools::McpSessionToolExecutor::new(
                                        Some(turn_mcp.clone()),
                                        turn_coordinator.clone(),
                                        session_id.clone(),
                                        &executor,
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
                                // C9 §5/§2.2: the typed observation channel
                                // → session/update notices. Reconnect and
                                // inert-param notices arrive live; the
                                // rate-limit snapshot arrives coalesced
                                // (latest-wins per turn iteration) from the
                                // engine.
                                let obs_out = observer_out;
                                let obs_session = session_id.clone();
                                let observer = move |observation: ModelObservation| {
                                    let notice = match observation {
                                        ModelObservation::Reconnecting {
                                            attempt,
                                            next_delay_ms,
                                            deadline_remaining_ms,
                                        } => reconnect_notice(
                                            &obs_session,
                                            attempt,
                                            next_delay_ms,
                                            deadline_remaining_ms,
                                        ),
                                        ModelObservation::ParamInert {
                                            param,
                                            surface,
                                            detail,
                                        } => param_inert_notice(
                                            &obs_session,
                                            &param,
                                            &surface,
                                            &detail,
                                        ),
                                        ModelObservation::RateLimit(snapshot) => {
                                            rate_limit_notice(
                                                &obs_session,
                                                serde_json::to_value(&snapshot)
                                                    .unwrap_or(serde_json::Value::Null),
                                            )
                                        }
                                        // P1 §4.1/§4.2: typed budget
                                        // notices (C7 vocabulary).
                                        ModelObservation::BudgetWarn {
                                            limit,
                                            observed,
                                            pct_used,
                                        } => budget_warn_notice(
                                            &obs_session,
                                            limit,
                                            observed,
                                            pct_used,
                                        ),
                                        ModelObservation::BudgetClamp {
                                            requested,
                                            granted,
                                        } => budget_clamp_notice(&obs_session, requested, granted),
                                    };
                                    let mut guard =
                                        obs_out.lock().unwrap_or_else(|p| p.into_inner());
                                    let _ = write_json(&mut *guard, &notice);
                                };
                                let engine = TurnEngine {
                                    model: &driver,
                                    tools: &executor,
                                    budget: TurnBudget::default(),
                                    model_name: turn_model,
                                    tool_definitions,
                                    approval: Some(&gate),
                                    compaction: Some(compaction),
                                    robustness: TurnRobustness {
                                        steer: Some(steer_handle),
                                        auth_refresh: None, // static Flux key: 401 → zero retries (Q5)
                                        observer: Some(&observer),
                                        reasoning_effort: turn_effort,
                                        verbosity: turn_verbosity,
                                        output_schema: None,
                                        // P1: the session meter drives the
                                        // atomic reservation/clamp and the
                                        // journaled turn-sum; None = pre-P1.
                                        // extra_usage is the search lane's
                                        // dual-feed cell (r3 codex-F1).
                                        meter: turn_meter.clone(),
                                        extra_usage,
                                        image_influence: Some(gate_image_influenced.clone()),
                                    },
                                };
                                let sink_session = session_id.clone();
                                let sink_coordinator = turn_coordinator.clone();
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
                                    } else if let Op::CompactionComplete {
                                        compaction_id,
                                        summary,
                                        changed_files,
                                        image_influenced,
                                        ..
                                    } = &envelope.op
                                    {
                                        // P3 §3.3 [r4 codex new-1]: the
                                        // compaction critical section — under
                                        // the coordinator's ONE guard, capture
                                        // the watermark (the full snapshot's
                                        // op ids), build the carry as the
                                        // EXACT fold of the replay input at
                                        // the watermark (carry(W) ≡ fold),
                                        // then append the Complete durably.
                                        // No append can interleave between
                                        // capture and publish. A failed append
                                        // inside the section aborts the
                                        // compaction: nothing is published,
                                        // the session continues uncompacted
                                        // (compact_messages turns the false
                                        // into a typed CompactionCancel).
                                        let section = sink_coordinator.compaction().and_then(
                                            |mut guard| {
                                                let snapshot = guard.snapshot()?;
                                                let covers: Vec<String> = snapshot
                                                    .iter()
                                                    .map(|e| e.id.clone())
                                                    .collect();
                                                let carry =
                                                    nano_session::hydration_carry_at(&snapshot)?;
                                                let complete = OpEnvelope::new(
                                                    envelope.id.clone(),
                                                    envelope.ts.clone(),
                                                    Op::CompactionComplete {
                                                        compaction_id: compaction_id.clone(),
                                                        summary: summary.clone(),
                                                        covers_op_ids: covers,
                                                        changed_files: changed_files.clone(),
                                                        image_influenced: *image_influenced,
                                                        mcp_hydration: carry,
                                                    },
                                                );
                                                guard.append_complete(&complete)
                                            },
                                        );
                                        match section {
                                            Ok(_) => true,
                                            Err(err) => {
                                                eprintln!(
                                                    "wayland-nano: compaction critical section failed: {err}"
                                                );
                                                false
                                            }
                                        }
                                    } else {
                                        match sink_coordinator.append(envelope) {
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
                                    // P2a §5.1/§5.4: TurnBegin is the FIRST
                                    // op through this sink — once its append
                                    // is durable the blob-publish → journal-
                                    // reference span is closed and the
                                    // shared GC lease is released.
                                    if matches!(envelope.op, Op::TurnBegin { .. }) {
                                        drop(attach_lease.take());
                                    }
                                    // P2a §8 part 2 flow-back: compaction
                                    // provenance returns to host session
                                    // state over the existing op channel —
                                    // the sticky flag ORs every observed
                                    // CompactionComplete.image_influenced.
                                    if let Op::CompactionComplete {
                                        image_influenced,
                                        ..
                                    } = &envelope.op
                                    {
                                        gate_image_influenced
                                            .fetch_or(*image_influenced, Ordering::SeqCst);
                                    }
                                    let mut guard =
                                        sink_out.lock().unwrap_or_else(|p| p.into_inner());
                                    let _ =
                                        write_op_frame(&mut *guard, &sink_session, envelope);
                                    true
                                };
                                // P2a §5.2.1: the host's sole prompt-
                                // execution call is RE-ROUTED to the blocks
                                // entry — the authoritative TurnInput (the
                                // §2.3 converter's output) plus the session's
                                // sticky provenance flag.
                                let result = engine
                                    .run_turn_streaming_with_context_blocks(
                                        &turn_id,
                                        turn_input,
                                        prior_context,
                                        Some(cancel.as_ref()),
                                        &mut sink,
                                        image_influenced_before,
                                    )
                                    .await;
                                // P1 §5: the meter's status payload rides
                                // every turn end (Σ tokens | $cost|unpriced
                                // + the cap position when configured).
                                if let Some(meter) = &turn_meter {
                                    let usage = meter.session_usage();
                                    let state = meter.budget_state();
                                    let notice = budget_notice(
                                        &session_id,
                                        usage.total_tokens(),
                                        usage.microcents,
                                        usage.priced,
                                        state.map(|s| s.limit),
                                        state.map(|s| s.observed),
                                    );
                                    let mut guard =
                                        budget_out.lock().unwrap_or_else(|p| p.into_inner());
                                    let _ = write_json(&mut *guard, &notice);
                                }
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
                            // never swap the in-memory history. P3 §3.3: the
                            // session's JournalCoordinator (opened fail-closed
                            // at session construction) is the append
                            // authority; the read below is id sequencing only.
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
                            // P3 §3.3: covers_op_ids is a PLACEHOLDER here —
                            // the emit closure recomputes the watermark and
                            // the hydration carry under the coordinator's
                            // guard at CompactionComplete time (the atomic
                            // cut), so no append can interleave between
                            // capture and the durable Complete.
                            let covers_op_ids = Vec::new();
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
                            // P2a §8 part 2: capture the provenance inputs
                            // BEFORE the swap — the session's sticky flag
                            // and whether the compacted context held images.
                            let image_influenced_before =
                                active.image_influenced.load(Ordering::SeqCst);
                            let had_images = context.iter().any(|m| {
                                m.content
                                    .iter()
                                    .any(|b| matches!(b, ContentBlock::Image { .. }))
                            });
                            let mut op_sequence = 0u32;
                            let notice_id = compaction_id.clone();
                            let notice_session = session_id.clone();
                            let notice_out = out.clone();
                            let compact_coordinator = active.coordinator.clone();
                            let mut emit = |op: Op| -> bool {
                                op_sequence += 1;
                                let status = match &op {
                                    Op::CompactionBegin { .. } => Some("begin"),
                                    Op::CompactionComplete { .. } => Some("complete"),
                                    Op::CompactionCancel { .. } => Some("cancel"),
                                    _ => None,
                                };
                                let envelope_id = format!("{notice_id}-op-{op_sequence}");
                                // P3 §3.3 [r4 codex new-1]: the Complete lands
                                // inside the coordinator's critical section —
                                // watermark capture (the full snapshot's op
                                // ids) + carry construction (carry(W) ≡
                                // fold(replay input at W)) + the durable
                                // append, all under the ONE guard. A failure
                                // inside the section aborts the compaction:
                                // nothing is published, the session continues
                                // uncompacted.
                                let journaled = if let Op::CompactionComplete {
                                    compaction_id,
                                    summary,
                                    changed_files,
                                    image_influenced,
                                    ..
                                } = &op
                                {
                                    compact_coordinator
                                        .compaction()
                                        .and_then(|mut guard| {
                                            let snapshot = guard.snapshot()?;
                                            let covers: Vec<String> = snapshot
                                                .iter()
                                                .map(|e| e.id.clone())
                                                .collect();
                                            let carry =
                                                nano_session::hydration_carry_at(&snapshot)?;
                                            guard.append_complete(&OpEnvelope::new(
                                                envelope_id.clone(),
                                                "now",
                                                Op::CompactionComplete {
                                                    compaction_id: compaction_id.clone(),
                                                    summary: summary.clone(),
                                                    covers_op_ids: covers,
                                                    changed_files: changed_files.clone(),
                                                    image_influenced: *image_influenced,
                                                    mcp_hydration: carry,
                                                },
                                            ))
                                        })
                                        .map_err(|err| {
                                            eprintln!(
                                                "wayland-nano: compaction critical section failed: {err}"
                                            );
                                            err
                                        })
                                        .is_ok()
                                } else {
                                    match compact_coordinator.append(&OpEnvelope::new(
                                        envelope_id,
                                        "now",
                                        op,
                                    )) {
                                        Ok(_) => true,
                                        Err(err) => {
                                            eprintln!(
                                                "wayland-nano: session journal append failed: {err}"
                                            );
                                            false
                                        }
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
                                // P2a §8 part 2: the session-side sticky
                                // provenance flag — the host passes its
                                // session value directly (r5).
                                image_influenced_before,
                                &mut emit,
                            )
                            .await;
                            // On failure the context came back untouched
                            // (compact_messages never swaps without a durable
                            // Complete); on success it IS the compacted one.
                            active.context = context;
                            // P2a §8 part 2 flow-back: the journaled
                            // provenance (= before OR any-image-evicted;
                            // eviction happens iff the context held an image)
                            // folds back into the sticky session flag.
                            if outcome.is_ok() && (image_influenced_before || had_images) {
                                active.image_influenced.store(true, Ordering::SeqCst);
                            }
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
                        // P1 §4.1/§4.3: `/budget continue <tokens>` — the
                        // single-shot grant (one grant per stop is the RC2
                        // scope). Journal-first, accepted-only (the C10
                        // TodoSet ordering pattern): the command validates,
                        // Op::BudgetGrant lands DURABLY, and only then does
                        // the effective-limit cell mutate. Append failure =
                        // typed error, limit unchanged. Without this command
                        // a budget-stopped session stays stopped — typed
                        // error on every prompt, never a soft-continue.
                        "session/budget" => {
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
                            let tokens = params
                                .as_ref()
                                .and_then(|p| p.get("tokens"))
                                .and_then(|t| t.as_u64())
                                .filter(|t| *t > 0);
                            let (meter, tokens) = match (active.meter.clone(), tokens) {
                                (Some(meter), Some(tokens)) => (meter, tokens),
                                _ => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::InvalidParams,
                                            "session/budget requires a positive integer `tokens` (and a metered session)",
                                            NanoErrorExtras::default(),
                                        ),
                                    )?;
                                    continue;
                                }
                            };
                            let Some(state) = meter.budget_state() else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "no session token cap configured (set NANO_BUDGET_SESSION_TOKENS)",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            let after_limit = state.limit.saturating_add(tokens);
                            active.mode_changes += 1;
                            let grant_id = format!("{}-grant-{}", active.id, active.mode_changes);
                            let envelope_id =
                                format!("{}-budget-grant-{}", active.id, active.mode_changes);
                            // Journal-first: the op lands durably BEFORE the
                            // limit cell mutates. P3 §3.3: through the
                            // session's coordinator, the one append authority.
                            let journaled = active.coordinator.append(&OpEnvelope::new(
                                envelope_id,
                                "now",
                                Op::BudgetGrant {
                                    grant_id,
                                    tokens,
                                    after_limit,
                                },
                            ));
                            match journaled {
                                Ok(_) => {
                                    let after = meter
                                        .apply_grant(tokens)
                                        .expect("cap presence checked above");
                                    let usage = meter.session_usage();
                                    let state = meter.budget_state();
                                    let notice = budget_notice(
                                        &active.id,
                                        usage.total_tokens(),
                                        usage.microcents,
                                        usage.priced,
                                        state.map(|s| s.limit),
                                        state.map(|s| s.observed),
                                    );
                                    write_out(&out, &notice)?;
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::ok(
                                            id,
                                            serde_json::json!({ "limit": after }),
                                        ),
                                    )?;
                                }
                                Err(err) => {
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::JournalUnavailable,
                                            format!(
                                                "budget grant journal append failed (limit unchanged): {err}"
                                            ),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?;
                                }
                            }
                        }
                        // ── C11 (Q1 RULED): ACP extension methods, thin
                        // adapters over the nano-session/nano-agent library
                        // APIs — no business logic in the ACP layer. ──
                        "_wayland/session/list" => {
                            // P4 session browser: the listing is GLOBAL and
                            // read-only — served with or without an active
                            // session, and never gated on a running turn
                            // (the fork discipline below does not apply: no
                            // journal is written here).
                            match crate::session_browser::handle_list_request(
                                config.sessions_dir,
                            ) {
                                Ok(result) => {
                                    write_out(&out, &JsonRpcResponse::ok(id, result))?
                                }
                                Err(err) => {
                                    eprintln!("wayland-nano: session list failed: {err}");
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::JournalUnavailable,
                                            format!("session listing unavailable: {err}"),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?;
                                }
                            }
                        }
                        "_wayland/session/fork" => {
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
                            let fork_session = params
                                .get("sessionId")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                                .or_else(|| session.as_ref().map(|s| s.id.clone()));
                            let Some(fork_session) = fork_session else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "_wayland/session/fork requires a sessionId",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            let at_turn = params
                                .get("atTurn")
                                .and_then(|t| t.as_str())
                                .map(str::to_string);
                            match crate::session_cmds::session_fork_core(
                                config.sessions_dir,
                                &fork_session,
                                at_turn,
                            ) {
                                Ok(result) => {
                                    write_out(&out, &JsonRpcResponse::ok(id, result))?
                                }
                                Err(err) => {
                                    // The detail (guard-busy / missing parent /
                                    // journal I/O) stays logs-side; the wire
                                    // carries the static presentation.
                                    eprintln!("wayland-nano: session fork failed: {err}");
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::SessionForkFailed,
                                            error_presentation(
                                                NanoErrorKind::SessionForkFailed,
                                            ),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?
                                }
                            }
                        }
                        "_wayland/goal/set" => {
                            let params = params.unwrap_or_default();
                            let Some(goal_session) = params
                                .get("sessionId")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                                .or_else(|| session.as_ref().map(|s| s.id.clone()))
                            else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "_wayland/goal/set requires a sessionId",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            let objective = params
                                .get("objective")
                                .and_then(|o| o.as_str())
                                .unwrap_or("")
                                .to_string();
                            let budget_of = |key: &str| {
                                params
                                    .get("budgets")
                                    .and_then(|b| b.get(key))
                                    .and_then(|v| v.as_u64())
                            };
                            let budgets = nano_session::GoalBudgets {
                                token_budget: budget_of("tokenBudget"),
                                turn_budget: budget_of("turnBudget"),
                                wall_clock_budget_ms: budget_of("wallClockBudgetMs"),
                            };
                            match crate::session_cmds::goal_set_core(
                                config.sessions_dir,
                                &goal_session,
                                &objective,
                                &budgets,
                            ) {
                                Ok(result) => {
                                    write_out(&out, &JsonRpcResponse::ok(id, result))?
                                }
                                Err(err) => {
                                    eprintln!("wayland-nano: goal operation failed: {err}");
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::GoalOpFailed,
                                            error_presentation(NanoErrorKind::GoalOpFailed),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?
                                }
                            }
                        }
                        "_wayland/goal/status" => {
                            let params = params.unwrap_or_default();
                            let goal_session = params
                                .get("sessionId")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                                .or_else(|| session.as_ref().map(|s| s.id.clone()));
                            let Some(goal_session) = goal_session else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        "_wayland/goal/status requires a sessionId",
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            match crate::session_cmds::goal_status_core(
                                config.sessions_dir,
                                &goal_session,
                            ) {
                                Ok(result) => {
                                    write_out(&out, &JsonRpcResponse::ok(id, result))?
                                }
                                Err(err) => {
                                    eprintln!("wayland-nano: goal operation failed: {err}");
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::GoalOpFailed,
                                            error_presentation(NanoErrorKind::GoalOpFailed),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?
                                }
                            }
                        }
                        "_wayland/goal/pause" | "_wayland/goal/resume" | "_wayland/goal/cancel" => {
                            let action = method
                                .rsplit('/')
                                .next()
                                .unwrap_or_default()
                                .to_string();
                            let params = params.unwrap_or_default();
                            let goal_session = params
                                .get("sessionId")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                                .or_else(|| session.as_ref().map(|s| s.id.clone()));
                            let Some(goal_session) = goal_session else {
                                write_out(
                                    &out,
                                    &JsonRpcResponse::err_typed(
                                        id,
                                        NanoErrorKind::InvalidParams,
                                        format!("{method} requires a sessionId"),
                                        NanoErrorExtras::default(),
                                    ),
                                )?;
                                continue;
                            };
                            match crate::session_cmds::goal_transition_core(
                                config.sessions_dir,
                                &goal_session,
                                &action,
                            ) {
                                Ok(result) => {
                                    write_out(&out, &JsonRpcResponse::ok(id, result))?
                                }
                                Err(err) => {
                                    eprintln!("wayland-nano: goal operation failed: {err}");
                                    write_out(
                                        &out,
                                        &JsonRpcResponse::err_typed(
                                            id,
                                            NanoErrorKind::GoalOpFailed,
                                            error_presentation(NanoErrorKind::GoalOpFailed),
                                            NanoErrorExtras::default(),
                                        ),
                                    )?
                                }
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
                // C9: the finished turn's queue is closed (the engine did
                // it, with drop notices); a fresh prompt installs a fresh
                // queue, and a steer between turns rejects closed.
                *current_steer.lock().unwrap_or_else(|p| p.into_inner()) = None;
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
                            // P2a: refresh the sticky §9.1 flag from the
                            // journal fold (sticky-OR over ALL records —
                            // belt-and-braces beside the live sink fold),
                            // and rehydrate image manifests through the SAME
                            // §5.3 site session/load uses. A store failure
                            // here degrades loudly (placeholders + notices),
                            // never aborts the session between turns.
                            let influenced = image_influenced_from_envelopes(&report.envelopes);
                            active
                                .image_influenced
                                .fetch_or(influenced, Ordering::SeqCst);
                            let attachment_store =
                                if journal_has_image_manifests(&report.envelopes) {
                                    match AttachmentStore::open(config.attachment_home) {
                                        Ok(store) => Some(store),
                                        Err(err) => {
                                            // Fail-loud degradation: every
                                            // manifest entry becomes the §5.3
                                            // placeholder below.
                                            eprintln!(
                                                "wayland-nano: attachment store open failed between turns: {err}"
                                            );
                                            None
                                        }
                                    }
                                } else {
                                    None
                                };
                            let (rebuilt, attachment_issues) =
                                messages_from_envelopes_rehydrating(
                                    &report.envelopes,
                                    attachment_store.as_ref(),
                                );
                            context.extend(rebuilt);
                            active.context = context;
                            for issue in &attachment_issues {
                                write_out(
                                    &out,
                                    &attachment_missing_notice(&turn_session, issue.cause.as_str(), &issue.digest_prefix),
                                )?;
                            }
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
            // C11: the session-alive-only cron runner (Q3). One tick computes
            // due jobs and dispatches them through the §5.4 journal-first
            // fire transaction; the SessionGuard excludes interleaving with
            // a running turn (typed Busy → deferred to the next tick, never
            // stacked). A corrupt job store disables the scheduler for the
            // process lifetime (Q6) — the session itself is unaffected.
            _ = async {
                match cron_interval.as_mut() {
                    Some(interval) => interval.tick().await,
                    None => std::future::pending::<tokio::time::Instant>().await,
                }
            }, if cron_interval.is_some() && !cron_disabled => {
                let live_mode = |id: &str| -> Option<&'static str> {
                    session
                        .as_ref()
                        .filter(|active| active.id == id)
                        .map(|active| active.mode.lock().unwrap_or_else(|p| p.into_inner()).id())
                };
                let tick = crate::cron_fire::tick_once(
                    config.cron_home.expect("ticker only when configured"),
                    config.sessions_dir,
                    config.default_model,
                    make_driver.as_ref(),
                    &make_tools,
                    config.router,
                    (config.sandbox_probe)(),
                    &live_mode,
                )
                .await;
                match tick {
                    Ok(outcomes) => {
                        for outcome in outcomes {
                            if !matches!(
                                outcome,
                                nano_agent::cron::JobTickOutcome::Idle { .. }
                            ) {
                                eprintln!("wayland-nano: cron tick: {outcome:?}");
                            }
                        }
                    }
                    Err(err) => {
                        eprintln!("wayland-nano: cron scheduler disabled: {err}");
                        cron_disabled = true;
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
    elicitation: Option<nano_agent::mcp::ElicitationHandlerFactory>,
) -> Arc<Mutex<McpRegistry>> {
    let specs: Vec<McpServerSpec> = env_mcp_specs
        .iter()
        .cloned()
        .chain(crate::mcp_specs::mcp_specs_from_acp_params(params))
        .collect();
    Arc::new(Mutex::new(crate::mcp_specs::register_all_with(
        specs,
        elicitation,
    )))
}

/// P3 §3.1/§3.4: after a session's registry is built, every deferred-
/// inventory startup warning is surfaced as a bounded session/update notice
/// (loud, never silent), and — on load — the journaled hydration state is
/// re-applied through the canonical digest gate (mismatch ⇒ drop-and-notify;
/// churn ⇒ pinned Deferred with the typed warning).
fn mcp_session_notices<W: Write>(
    out: &Arc<Mutex<W>>,
    session_id: &str,
    mcp: &Arc<Mutex<McpRegistry>>,
    folded: Option<&nano_session::SessionState>,
) -> std::io::Result<()> {
    let notices = {
        let mut registry = mcp.lock().unwrap_or_else(|p| p.into_inner());
        let mut notices = registry.startup_warnings.clone();
        if let Some(state) = folded {
            notices.extend(registry.resume_hydration(
                &state.mcp_hydrated,
                &state.mcp_tools_digest,
                &state.mcp_recent_digests,
            ));
        }
        notices
    };
    for notice in &notices {
        write_out(
            out,
            &nano_protocol::acp::mcp_notice(session_id, "mcp", notice),
        )?;
    }
    Ok(())
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
/// P3 §5.2: resolve one elicitation card response by OPAQUE ID — never by
/// label (the bridge's binding is the authority; a duplicate or adversarial
/// label cannot steer the answer). Every non-answer is fail-closed; §5.4
/// pins the mapping: a wire `cancelled` outcome is `Cancel` (journaled/sent
/// as the spec-legal `cancel`), a dismiss or any malformed shape is
/// `Dismiss` (→ decline); an unknown id is NOT a guessed value.
fn resolve_elicitation_response(
    value: &serde_json::Value,
    known: &std::collections::HashSet<String>,
    card_id: u64,
) -> nano_agent::elicitation::ElicitAskOutcome {
    use nano_agent::elicitation::ElicitAskOutcome;
    let Some(outcome) = value.get("result").and_then(|r| r.get("outcome")) else {
        return ElicitAskOutcome::Dismiss;
    };
    let option_id = outcome.get("optionId").and_then(|o| o.as_str());
    match outcome.get("outcome").and_then(|o| o.as_str()) {
        Some("selected") => match option_id {
            Some(nano_protocol::acp::QUESTION_DISMISS_ID) => ElicitAskOutcome::Dismiss,
            Some(id) if known.contains(id) => ElicitAskOutcome::Answered {
                card_id,
                option_id: id.to_string(),
            },
            // An id the binding never minted: fail closed, never a guess.
            Some(_) | None => ElicitAskOutcome::Dismiss,
        },
        // F-P3-7: user-cancel is the pinned `cancel`, not a decline (§5.4).
        Some("cancelled") => ElicitAskOutcome::Cancel,
        _ => ElicitAskOutcome::Dismiss,
    }
}

#[cfg(test)]
mod elicitation_resolve_tests {
    //! P3 §5.4 pinned outcome mapping at the ACP wire boundary (F-P3-7):
    /// user-cancel ⇒ `Cancel` (journaled/sent as the spec-legal `cancel`),
    /// dismiss/unknown/malformed ⇒ fail-closed `Dismiss` (→ decline).
    use super::resolve_elicitation_response;
    use nano_agent::elicitation::ElicitAskOutcome;

    fn known() -> std::collections::HashSet<String> {
        std::collections::HashSet::from(["a".repeat(24)])
    }

    #[test]
    fn wire_cancelled_maps_to_pinned_cancel_not_decline() {
        let outcome = resolve_elicitation_response(
            &serde_json::json!({"result": {"outcome": {"outcome": "cancelled"}}}),
            &known(),
            7,
        );
        assert_eq!(outcome, ElicitAskOutcome::Cancel);
    }

    #[test]
    fn dismiss_and_malformed_stay_fail_closed_dismiss() {
        // Desktop's Dismiss mapping: a selected `reject` id.
        let dismissed = resolve_elicitation_response(
            &serde_json::json!({"result": {"outcome": {"outcome": "selected", "optionId": "reject"}}}),
            &known(),
            7,
        );
        assert_eq!(dismissed, ElicitAskOutcome::Dismiss);
        // A selected id the binding never minted: never a guessed value.
        let forged = resolve_elicitation_response(
            &serde_json::json!({"result": {"outcome": {"outcome": "selected", "optionId": "b".repeat(24)}}}),
            &known(),
            7,
        );
        assert_eq!(forged, ElicitAskOutcome::Dismiss);
        // No outcome at all.
        let malformed =
            resolve_elicitation_response(&serde_json::json!({"result": {}}), &known(), 7);
        assert_eq!(malformed, ElicitAskOutcome::Dismiss);
    }

    #[test]
    fn selected_known_id_answers() {
        let id = "a".repeat(24);
        let outcome = resolve_elicitation_response(
            &serde_json::json!({"result": {"outcome": {"outcome": "selected", "optionId": id}}}),
            &known(),
            7,
        );
        assert_eq!(
            outcome,
            ElicitAskOutcome::Answered {
                card_id: 7,
                option_id: id
            }
        );
    }
}

/// P3 §5.2: the elicitation ask channel — a session-scoped closure the
/// dispatcher's handler thread drives (never the turn loop). It rides the
/// SAME PendingMap question machinery as AcpApproval::ask, but the wire
/// option ids are the bridge's opaque 96-bit ids verbatim and the card is
/// labeled server-originated ("MCP server '<name>' asks:"). Cancel, the
/// bounded 300s timeout (the C10 default), and disconnect all fail closed;
/// the pending entry is removed on EVERY exit, so a late answer lands in the
/// reader's unknown-id arm and is dropped+logged.
fn elicitation_ask_channel<W: Write + Send + 'static>(
    out: Arc<Mutex<W>>,
    pending: PendingMap,
    next_id: Arc<std::sync::atomic::AtomicU64>,
    session_id: String,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Arc<
    dyn Fn(nano_agent::elicitation::ElicitQuestion) -> nano_agent::elicitation::ElicitAskOutcome
        + Send
        + Sync,
> {
    Arc::new(move |question: nano_agent::elicitation::ElicitQuestion| {
        use nano_agent::elicitation::ElicitAskOutcome;
        let card_id = next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(card_id, tx);
        let known: std::collections::HashSet<String> =
            question.options.iter().map(|(id, _)| id.clone()).collect();
        let request = nano_protocol::acp::request_elicitation_request(
            card_id,
            &session_id,
            &format!("mcp-elicit-{card_id}"),
            &question.header,
            &question.message,
            &question.options,
        );
        if write_out(&out, &request).is_err() {
            pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&card_id);
            return ElicitAskOutcome::Unavailable;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let outcome = loop {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(response) => {
                    break resolve_elicitation_response(&response, &known, card_id);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::SeqCst) {
                        break ElicitAskOutcome::Cancel;
                    }
                    if std::time::Instant::now() >= deadline {
                        break ElicitAskOutcome::Timeout;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break ElicitAskOutcome::Unavailable;
                }
            }
        };
        // Removal on EVERY exit (answer, timeout, cancel, disconnect).
        pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&card_id);
        outcome
    })
}

/// P3 §5.2: the per-server bridge factory installed before registration —
/// the elicitation capability is advertised only because this handler exists
/// (the handshake honesty rule). The bridge journals through the session's
/// coordinator and asks through the ACP card channel above.
fn elicitation_factory<W: Write + Send + 'static>(
    out: Arc<Mutex<W>>,
    pending: PendingMap,
    next_id: Arc<std::sync::atomic::AtomicU64>,
    session_id: String,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    coordinator: Arc<nano_session::JournalCoordinator>,
) -> nano_agent::mcp::ElicitationHandlerFactory {
    Arc::new(
        move |server_id: &str, display_name: &str, interrupted_call: Arc<Mutex<Option<String>>>| {
            // §2.7 (F-P3-3): `server_id` is the registry-minted instance id
            // (the journaled key); `display_name` labels the ask card only.
            let bridge = Arc::new(nano_agent::elicitation::ElicitationBridge::new(
                server_id.to_string(),
                display_name.to_string(),
                session_id.clone(),
                coordinator.clone(),
                interrupted_call,
                elicitation_ask_channel(
                    out.clone(),
                    pending.clone(),
                    next_id.clone(),
                    session_id.clone(),
                    cancel.clone(),
                ),
            ));
            nano_agent::mcp::ElicitationHandlerParts {
                handler: bridge.clone(),
                slot_retired_hook: bridge.slot_retired_hook(),
            }
        },
    )
}

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
    /// P2a §9.1/D12: the session's sticky image-influenced cell. While set,
    /// protected TRUST MUTATIONS (rule amendment, credential/secret and
    /// nano-home config writes, statically-unclassifiable shell commands —
    /// the classes reachable through today's tool surface; OAuth consent /
    /// egress-allowlist / capability-override flows have no tool surface at
    /// 4ca7700) ALWAYS route to the interactive human prompt, in every mode
    /// including full_auto — auto-approve paths are bypassed. Read-only and
    /// ordinary workspace-scoped calls keep their normal mode semantics.
    image_influenced: Arc<std::sync::atomic::AtomicBool>,
}

struct AcpImageReadApprover<W: Write> {
    session_id: String,
    out: Arc<Mutex<W>>,
    pending: PendingMap,
    next_id: Arc<std::sync::atomic::AtomicU64>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl<W: Write> std::fmt::Debug for AcpImageReadApprover<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpImageReadApprover")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl<W: Write + Send> nano_tools::image::ImageReadApprover for AcpImageReadApprover<W> {
    fn request(&self, canonical: &std::path::Path) -> nano_tools::image::ImageReadApproval {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        let arguments = serde_json::json!({"path": canonical.to_string_lossy()});
        let request = request_permission_request(
            id,
            &self.session_id,
            "view-image-read",
            "view_image",
            &arguments,
        );
        if write_out(&self.out, &request).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return nano_tools::image::ImageReadApproval::Denied;
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
        match decision {
            ApprovalDecision::Approve => nano_tools::image::ImageReadApproval::Approved,
            ApprovalDecision::Deny => nano_tools::image::ImageReadApproval::Denied,
        }
    }
}

/// P2a §9.1: the closed protected-class classifier. A call is a protected
/// trust mutation when it can persistently alter what Nano trusts:
/// rule files (AGENTS.md), secret/credential subtrees (`.secrets`), or Nano
/// home config (`.nano` — capability/egress/credential settings). `shell`
/// commands are not statically classifiable (a contained shell can write a
/// rule file), so under the clamp they ALWAYS ask — fail-closed. Path
/// denials stay with the tool layer; this classifier only downgrades
/// auto-approval to the human prompt.
pub(crate) fn is_protected_trust_mutation(call: &ToolCall) -> bool {
    nano_agent::turn::is_protected_trust_mutation(call)
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
        // 1c. P3 §4.3 [r2 claude-F1, r3 claude-N7]: the explicit MCP approval
        //     classes — AFTER the two fast paths (these names match NEITHER
        //     by construction, pinned in tests) and BEFORE posture_allows:
        //     plan-posture enforcement must never decide an MCP surface
        //     before its class is consulted. DiscoveryLocal is auto-approved
        //     by deliberate assignment (local index search; mutates only
        //     journaled session state — the todo rationale), never
        //     inherited. The server classes deliberately ride the strict
        //     mode arms: read_only DENIES, default/full_auto PROMPT — a
        //     resource read never gets a read_only pass and never a
        //     full_auto pass.
        if let Some(class) = nano_agent::wiring::mcp_approval_class(&call.name) {
            return match class {
                nano_agent::wiring::McpApprovalClass::DiscoveryLocal => ApprovalDecision::Approve,
                nano_agent::wiring::McpApprovalClass::ServerQuery
                | nano_agent::wiring::McpApprovalClass::ServerDataRead => {
                    match self.effective_mode() {
                        PermissionMode::ReadOnly => ApprovalDecision::Deny,
                        _ => self.prompt_host(call),
                    }
                }
            };
        }
        // 1d. P4 §4.3 (V2's cross-audit ruling, mechanized): `pty_spawn`
        //     ALWAYS prompts outside read_only — no sandbox-available fast
        //     path (an unsandboxed interactive PTY is a sandbox-off
        //     capability by another name; the prompt is the AUTHORIZATION),
        //     no rule-DSL interaction (the rules surface matches only the
        //     literal `shell` name). read_only denies categorically.
        if call.name == "pty_spawn" {
            return match self.effective_mode() {
                PermissionMode::ReadOnly => ApprovalDecision::Deny,
                _ => self.prompt_host(call),
            };
        }
        // 1e. P4 §4.3: the four follow-ups on an EXISTING session are not
        //     re-gated (the spawn was the gated action — codex's model);
        //     ownership is enforced by the session-scoped PtySessionManager
        //     (a foreign id is unknown ⇒ typed PtySessionGone). read_only
        //     still denies categorically — fail-closed: under read_only no
        //     spawn was ever authorized, so a live session cannot
        //     legitimately exist.
        if nano_agent::wiring::PTY_TOOL_NAMES.contains(&call.name.as_str()) {
            return match self.effective_mode() {
                PermissionMode::ReadOnly => ApprovalDecision::Deny,
                _ => ApprovalDecision::Approve,
            };
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
        // 2b. P2a §9.1 (D12): on an image-influenced turn a protected TRUST
        //     MUTATION always takes the explicit human prompt — auto-approve
        //     paths are bypassed in EVERY mode (read_only's categorical deny
        //     is stricter still and unchanged). Placed after the plan
        //     posture so the plan-file exception still works under the
        //     clamp; ordinary workspace writes keep their mode semantics.
        if self.image_influenced.load(Ordering::SeqCst) && is_protected_trust_mutation(call) {
            return match self.effective_mode() {
                PermissionMode::ReadOnly => ApprovalDecision::Deny,
                _ => self.prompt_host(call),
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

    fn can_prompt_image_clamp(&self) -> bool {
        true
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
/// task_cancel, and task_apply change live state and always ask. P4 §5.5:
/// `repo_map` is a read-only lexical query (policy-filtered; denied-read
/// paths are never indexed or returned). The `pty_*` names are deliberately
/// ABSENT — their approval comes from the explicit §4.3 arms, never a
/// prefix fast path.
/// (pub: the C11 cron fire path reuses this exact predicate.)
pub fn is_read_only_tool(name: &str) -> bool {
    name.starts_with("fs_read")
        || name.starts_with("search")
        || name.starts_with("glob")
        || name.starts_with("repo_map")
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
            // C9: drained steers and schema re-asks replay as the user
            // messages they were (undrained steers are never journaled, so
            // replay can never resurrect input the model never saw).
            Op::SteerInput { text, .. } => {
                frames.push(user_message_chunk(session_id, text));
            }
            Op::SchemaReask { feedback, .. } => {
                frames.push(user_message_chunk(session_id, feedback));
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

/// P2a §5.3: one loud resume-degradation event — a journaled image manifest
/// entry whose blob could not be rehydrated. `cause` distinguishes
/// MISSING/TAMPERED/MALFORMED OPERATOR-SIDE (logs, notices); the
/// user-facing typed kind stays `AttachmentMissing` in every case (Q3
/// RULED).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentIssueCause {
    Missing,
    Tampered,
    Malformed,
}

impl AttachmentIssueCause {
    /// The bounded wire word (closed vocabulary — the C7 discipline).
    pub fn as_str(self) -> &'static str {
        match self {
            AttachmentIssueCause::Missing => "missing",
            AttachmentIssueCause::Tampered => "tampered",
            AttachmentIssueCause::Malformed => "malformed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentIssue {
    pub cause: AttachmentIssueCause,
    /// 8-char prefix of a VALIDATED digest, or "<malformed>" — never a raw
    /// journal string.
    pub digest_prefix: String,
}

/// §5.3 steps 2–5: validate the digest (exactly `^[0-9a-f]{64}$` — lane A's
/// `is_valid_digest`) BEFORE any path use, then open the blob through the
/// store (no-follow, reparse-point rejection, and sha256 verification live
/// INSIDE lane A's `read_verified`), then obey the §4.2 wire-payload cap.
/// Any failure is the loud `AttachmentMissing` degradation — resume NEVER
/// aborts (Q3 RULED), and a malformed digest can never become an
/// arbitrary-path read.
fn rehydrate_image_block(
    store: Option<&AttachmentStore>,
    reference: &nano_session::op::ImageRef,
) -> Result<String, AttachmentIssue> {
    let prefix = || {
        if is_valid_digest(&reference.digest) {
            reference.digest.chars().take(12).collect()
        } else {
            "<malformed>".to_string()
        }
    };
    if !is_valid_digest(&reference.digest) {
        return Err(AttachmentIssue {
            cause: AttachmentIssueCause::Malformed,
            digest_prefix: prefix(),
        });
    }
    let Some(store) = store else {
        return Err(AttachmentIssue {
            cause: AttachmentIssueCause::Missing,
            digest_prefix: prefix(),
        });
    };
    let bytes = store
        .read_verified(&reference.digest)
        .map_err(|err| AttachmentIssue {
            cause: match &err {
                BlobReadError::Missing | BlobReadError::Store(_) => AttachmentIssueCause::Missing,
                BlobReadError::Tampered => AttachmentIssueCause::Tampered,
                BlobReadError::MalformedDigest => AttachmentIssueCause::Malformed,
            },
            digest_prefix: prefix(),
        })?;
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    // §5.3 step 5: a digest-valid blob still obeys the §4.2 wire cap
    // (defense-in-depth — intake capped it; a store that wrote more is
    // inconsistent and degrades loudly).
    if data.len() as u64 > nano_tools::image::MAX_IMAGE_PAYLOAD_BYTES {
        eprintln!(
            "wayland-nano: attachment {} OVERSIZE blob exceeds the wire cap",
            prefix()
        );
        return Err(AttachmentIssue {
            cause: AttachmentIssueCause::Missing,
            digest_prefix: prefix(),
        });
    }
    Ok(data)
}

/// P2a §8 part 2 replay-fold rule: the image-influenced session flag is
/// reconstructed as the STICKY-OR over ALL journaled records — ANY
/// `CompactionComplete.image_influenced == true` OR any UNCOMPACTED
/// image-bearing `input_blocks` manifest (a manifest is compacted once some
/// `CompactionComplete.covers_op_ids` covers its TurnBegin envelope id).
/// Never the LATEST record: a false-negative record cannot reopen the §9.1
/// clamp on resume.
pub fn image_influenced_from_envelopes(envelopes: &[OpEnvelope]) -> bool {
    let mut covered: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut any_influenced = false;
    for envelope in envelopes {
        if let Op::CompactionComplete {
            image_influenced,
            covers_op_ids,
            ..
        } = &envelope.op
        {
            any_influenced |= *image_influenced;
            covered.extend(covers_op_ids.iter().map(String::as_str));
        }
    }
    if any_influenced {
        return true;
    }
    envelopes.iter().any(|envelope| {
        let present = match &envelope.op {
            Op::TurnBegin { input_blocks, .. } => input_blocks
                .iter()
                .any(|b| matches!(b, InputBlock::ImageRef(_))),
            Op::ToolResult { image_refs, .. } => !image_refs.is_empty(),
            _ => false,
        };
        present && !covered.contains(envelope.id.as_str())
    })
}

/// Whether any journaled manifest references an attachment — the store is
/// opened (with its §5.5 fail-closed audit) only when one does.
fn journal_has_image_manifests(envelopes: &[OpEnvelope]) -> bool {
    envelopes.iter().any(|envelope| match &envelope.op {
        Op::TurnBegin { input_blocks, .. } => input_blocks
            .iter()
            .any(|b| matches!(b, InputBlock::ImageRef(_))),
        Op::ToolResult { image_refs, .. } => !image_refs.is_empty(),
        _ => false,
    })
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
///
/// P2a §5.3 — THE SOLE image rehydration site: this signature is preserved
/// for journal-only consumers (the C1 replay/fault-injection tests, exec and
/// cron rebuilds). With no store handle, every image manifest entry degrades
/// to the loud §5.3 placeholder (operator-logged MISSING) — never a silent
/// drop. ACP session paths call [`messages_from_envelopes_rehydrating`]
/// with the store and emit the test-asserted session/update notices.
pub fn messages_from_envelopes(envelopes: &[OpEnvelope]) -> Vec<Message> {
    messages_from_envelopes_rehydrating(envelopes, None).0
}

/// P2a §5.3: the rehydrating variant — walks each non-compacted TurnBegin's
/// `input_blocks` manifest IN ORDER: `Text` → `ContentBlock::Text`,
/// `ImageRef` → a digest-verified `ContentBlock::Image` rebuilt at its
/// manifest position. Missing/corrupt/tampered blobs become the loud
/// placeholder at that position and an [`AttachmentIssue`] (the caller emits
/// the C9-style session/update notice — test-asserted). Image bytes and
/// digests NEVER reach `replay_frames` (the two-surface split).
pub fn messages_from_envelopes_rehydrating(
    envelopes: &[OpEnvelope],
    attachments: Option<&AttachmentStore>,
) -> (Vec<Message>, Vec<AttachmentIssue>) {
    let mut notices = Vec::new();
    let mut messages = Vec::new();
    let mut assistant: Vec<ContentBlock> = Vec::new();
    let mut call_names: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
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
            Op::TurnBegin {
                input,
                input_blocks,
                ..
            } => {
                flush_assistant(&mut messages, &mut assistant);
                if input_blocks.is_empty() {
                    // Pre-P2a journal or a text-only turn: the projection IS
                    // the message — byte-identical to the pre-P2a fold.
                    messages.push(Message::user(input.clone()));
                } else {
                    // §5.2: walk the ordered manifest — NO string matching
                    // is ever performed; leading/trailing/adjacent/duplicate
                    // images and user-authored `[Image #…]`-like text are
                    // all unambiguous by construction.
                    let mut blocks: Vec<ContentBlock> = Vec::new();
                    let mut image_ordinal = 0u32;
                    for block in input_blocks {
                        match block {
                            InputBlock::Text { text } => {
                                blocks.push(ContentBlock::Text { text: text.clone() });
                            }
                            InputBlock::ImageRef(reference) => {
                                image_ordinal += 1;
                                match rehydrate_image_block(attachments, reference) {
                                    Ok(data) => blocks.push(ContentBlock::Image {
                                        mime: reference.mime.clone(),
                                        data,
                                    }),
                                    Err(issue) => {
                                        let word = match issue.cause {
                                            AttachmentIssueCause::Missing => "MISSING",
                                            AttachmentIssueCause::Tampered => "TAMPERED",
                                            AttachmentIssueCause::Malformed => "MALFORMED",
                                        };
                                        eprintln!(
                                            "wayland-nano: attachment {word}: {}",
                                            issue.digest_prefix
                                        );
                                        notices.push(issue);
                                        // Lane A's placeholder fn is the ONE
                                        // source of the §5.3 text (12-char
                                        // prefix, never a raw digest).
                                        blocks.push(ContentBlock::Text {
                                            text: attachment_unavailable_placeholder(
                                                image_ordinal as usize,
                                                &reference.digest,
                                            ),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    messages.push(Message::user_blocks(blocks));
                }
            }
            // C9 kill-resume fidelity: drained steers and the schema re-ask
            // fold EXACTLY like the TurnBegin input fold, so a resumed
            // context is byte-identical to the live one. Undrained steers
            // are never journaled and never resurrect here.
            Op::SteerInput { text, .. } => {
                flush_assistant(&mut messages, &mut assistant);
                messages.push(Message::user(text.clone()));
            }
            Op::SchemaReask { feedback, .. } => {
                flush_assistant(&mut messages, &mut assistant);
                messages.push(Message::user(feedback.clone()));
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
                use std::collections::hash_map::Entry;
                match call_names.entry(call_id.clone()) {
                    Entry::Vacant(slot) => {
                        slot.insert(Some(name.clone()));
                    }
                    Entry::Occupied(mut slot) => {
                        slot.insert(None);
                    }
                }
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
                image_refs,
                ..
            } => {
                flush_assistant(&mut messages, &mut assistant);
                // [R1] The ONE synthesized-result encoding, shared with the
                // compaction repair pass and repeat-protection skips.
                // C7/D5: a typed failure resumes as `<presentation> [output
                // elided]` so the model still sees WHY the call failed; the
                // kind is journaled, the text re-derives from the table.
                let mut content = match (ok, error_kind) {
                    (false, Some(kind)) => {
                        format!("{} [output elided]", error_presentation(*kind))
                    }
                    _ => format!(
                        "[tool output elided from journal: ok={ok}, digest={output_digest}]"
                    ),
                };
                if image_refs.is_empty() {
                    messages.push(Message::tool_result(call_id, content, !ok));
                    continue;
                }
                let tool_name = match call_names.get(call_id).and_then(|name| name.as_deref()) {
                    Some(name) => name,
                    None => {
                        eprintln!(
                            "wayland-nano: unpaired or duplicate tool call id during image-result replay"
                        );
                        "<unavailable: unpaired call>"
                    }
                };
                let mut verified = Vec::new();
                for (index, reference) in image_refs.iter().enumerate() {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    // §3.3: the SAME BlobReadError split as the P2a intake
                    // arm — operator logs distinguish TAMPERED/MISSING, never
                    // a collapsed cause.
                    let read = match attachments {
                        Some(store) => store.read_verified(&reference.digest),
                        None => Err(BlobReadError::Missing),
                    };
                    match read {
                        Ok(bytes) => {
                            content.push_str(&nano_model::image_result::image_label(
                                index, tool_name, reference,
                            ));
                            verified.push((reference.clone(), bytes));
                        }
                        Err(err) => {
                            let cause = match &err {
                                BlobReadError::Missing | BlobReadError::Store(_) => {
                                    AttachmentIssueCause::Missing
                                }
                                BlobReadError::Tampered => AttachmentIssueCause::Tampered,
                                BlobReadError::MalformedDigest => AttachmentIssueCause::Malformed,
                            };
                            let prefix = if is_valid_digest(&reference.digest) {
                                reference.digest.chars().take(12).collect::<String>()
                            } else {
                                "malformed-digest".to_string()
                            };
                            content.push_str(&format!(
                                "[Image #{} from tool {tool_name} unavailable: attachment {prefix} missing — do not describe it from memory]",
                                index + 1
                            ));
                            notices.push(AttachmentIssue {
                                cause,
                                digest_prefix: prefix,
                            });
                        }
                    }
                }
                if verified.len() == image_refs.len() {
                    match nano_model::image_result::rehydrate_tool_result_images(verified) {
                        Ok((images, provenance))
                            if matches!(
                                provenance.kind(),
                                nano_model::image_result::ImageProvenanceKind::ReplayVerified { .. }
                            ) =>
                        {
                            drop(provenance);
                            messages.push(Message::tool_result_with_images(
                                call_id, content, !ok, images,
                            ));
                        }
                        _ => messages.push(Message::tool_result(call_id, content, !ok)),
                    }
                } else {
                    messages.push(Message::tool_result(call_id, content, !ok));
                }
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
                // P2a §8: the rehydrated history flows through the SAME
                // canonical builder (images counted at 6,000 bytes, the
                // deterministic placeholder eviction) — live and resumed
                // contexts stay byte-identical by construction.
                messages = nano_agent::compact::build_compacted_history(
                    std::mem::take(&mut messages),
                    summary,
                );
            }
            _ => {}
        }
    }
    flush_assistant(&mut messages, &mut assistant);
    (messages, notices)
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
        // C9: a drained steer / re-ask enters history as a user message;
        // render it as one, live, at the point it entered.
        Op::SteerInput { text, .. } => write_json(writer, &user_message_chunk(session_id, text)),
        Op::SchemaReask { feedback, .. } => {
            write_json(writer, &user_message_chunk(session_id, feedback))
        }
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
        /// P2a §9.1: the image-influenced cell wired into the gate.
        image_influenced: Arc<std::sync::atomic::AtomicBool>,
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
            let image_influenced = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
                image_influenced: image_influenced.clone(),
            };
            Self {
                gate,
                out,
                mode_cell,
                plan,
                stop,
                responder,
                image_influenced,
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

        /// P2a §9.1: simulate an image-influenced session.
        fn set_image_influenced(&self, value: bool) {
            self.image_influenced.store(value, Ordering::SeqCst);
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

    /// P3 §4.3 [r2 claude-F1]: the full per-mode decision table for the MCP
    /// session surfaces, driven through AcpApproval::approve. The round-1
    /// bypass (resource reads sailing through read_only by name-coupling) is
    /// the regression pin.
    #[test]
    fn p3_mcp_approval_class_decision_table() {
        let ws = workspace();
        // tool_search (DiscoveryLocal): approve in EVERY mode, no prompt,
        // no server contact (the transport-seam no-contact assertion lives
        // in nano-agent's mcp tests via the fake-server marker file).
        for mode in PermissionMode::ALL {
            let rig = TestGate::new(mode, &ws.0, false, None);
            assert_eq!(
                rig.gate
                    .approve(&call("tool_search", serde_json::json!({"query": "x"}))),
                ApprovalDecision::Approve,
                "{mode:?} must approve tool_search"
            );
            assert_eq!(rig.prompt_count(), 0, "{mode:?}: tool_search never prompts");
        }
        // The server classes: read_only DENIES without a prompt; default and
        // full_auto PROMPT (never a silent approve — full_auto included).
        for name in ["mcp_list_resources", "mcp_read_resource"] {
            let rig = TestGate::new(PermissionMode::ReadOnly, &ws.0, true, None);
            assert_eq!(
                rig.gate
                    .approve(&call(name, serde_json::json!({"server": "s"}))),
                ApprovalDecision::Deny,
                "read_only must deny {name}"
            );
            assert_eq!(rig.prompt_count(), 0, "read_only never prompts for {name}");
            for mode in [PermissionMode::Default, PermissionMode::FullAuto] {
                let rig = TestGate::new(mode, &ws.0, true, Some("allow"));
                assert_eq!(
                    rig.gate
                        .approve(&call(name, serde_json::json!({"server": "s"}))),
                    ApprovalDecision::Approve,
                    "{mode:?}/{name} resolves through the host prompt"
                );
                assert_eq!(
                    rig.prompt_count(),
                    1,
                    "{mode:?}/{name} must PROMPT the host"
                );
            }
        }
        // The mcp__ catch-all is unchanged: full_auto still PROMPTS (never
        // auto-approves server calls).
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("allow"));
        assert_eq!(
            rig.gate
                .approve(&call("mcp__server__tool", serde_json::json!({}))),
            ApprovalDecision::Approve
        );
        assert_eq!(rig.prompt_count(), 1, "full_auto prompts for mcp__ calls");
    }

    /// P3 §4.3 [r2 claude-F1]: the MCP session names match NEITHER fast path
    /// — a future rename cannot silently re-couple them to auto-approval.
    #[test]
    fn p3_mcp_session_names_match_neither_fast_path() {
        for name in nano_agent::wiring::MCP_SESSION_TOOL_NAMES {
            assert!(
                !is_read_only_tool(name),
                "{name} must not be a read-only fast-path name"
            );
            assert!(
                !nano_agent::wiring::SESSION_TOOL_NAMES.contains(&name),
                "{name} must not join the auto-approve-coupled SESSION_TOOL_NAMES"
            );
            assert!(
                nano_agent::wiring::mcp_approval_class(name).is_some(),
                "{name} must have an explicit approval class"
            );
        }
        // The mcp__ catch-all has no class (it rides the mode arms).
        assert!(nano_agent::wiring::mcp_approval_class("mcp__s__t").is_none());
    }

    /// P3 §4.3 [r3 claude-N7]: the class arm runs BEFORE posture_allows — a
    /// plan-posture session shows mcp_read_resource in plan+default still
    /// PROMPTING (posture_allows is never the decider for MCP names).
    #[test]
    fn p3_class_arm_precedes_plan_posture() {
        let ws = workspace();
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("allow"));
        rig.plan.lock().unwrap_or_else(|p| p.into_inner()).active = true;
        assert_eq!(
            rig.gate.approve(&call(
                "mcp_read_resource",
                serde_json::json!({"server": "s", "uri": "file:///x"})
            )),
            ApprovalDecision::Approve,
            "plan+default: the class arm prompts; posture never preempts"
        );
        assert_eq!(rig.prompt_count(), 1);
        // plan+read_only still denies categorically.
        let rig = TestGate::new(PermissionMode::ReadOnly, &ws.0, true, None);
        rig.plan.lock().unwrap_or_else(|p| p.into_inner()).active = true;
        assert_eq!(
            rig.gate.approve(&call(
                "mcp_read_resource",
                serde_json::json!({"server": "s", "uri": "file:///x"})
            )),
            ApprovalDecision::Deny
        );
        assert_eq!(rig.prompt_count(), 0);
        // The posture's own carve-out is untouched: a plan-file write still
        // approves, an off-plan write still denies.
        let plan_file = rig
            .plan
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .plan_file()
            .to_path_buf();
        assert_eq!(
            rig.gate.approve(&call(
                "fs_write",
                serde_json::json!({"path": plan_file, "content": "x"})
            )),
            ApprovalDecision::Approve
        );
        assert_eq!(
            rig.gate.approve(&contained_write(&ws.0)),
            ApprovalDecision::Deny
        );
    }

    /// P4 §13 spawn-gating matrix (the legs that could not exist
    /// pre-wiring): `pty_spawn` — read_only DENIES without a prompt;
    /// default PROMPTS; full_auto PROMPTS even with a probed sandbox (the
    /// always-prompt arm is the regression pin — no fast path).
    #[test]
    fn p4_pty_spawn_gate_matrix() {
        let ws = workspace();
        let spawn = || call("pty_spawn", serde_json::json!({"command": "cmd"}));
        // read_only: categorical deny, never a prompt.
        let rig = TestGate::new(PermissionMode::ReadOnly, &ws.0, true, None);
        assert_eq!(rig.gate.approve(&spawn()), ApprovalDecision::Deny);
        assert_eq!(
            rig.prompt_count(),
            0,
            "read_only never prompts for pty_spawn"
        );
        // default: ALWAYS the host prompt.
        let rig = TestGate::new(PermissionMode::Default, &ws.0, true, Some("allow"));
        assert_eq!(rig.gate.approve(&spawn()), ApprovalDecision::Approve);
        assert_eq!(rig.prompt_count(), 1, "default prompts for pty_spawn");
        // full_auto: PROMPTS — sandbox availability is NOT a fast path, in
        // either probe state.
        for sandbox in [true, false] {
            let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, sandbox, Some("allow"));
            assert_eq!(
                rig.gate.approve(&spawn()),
                ApprovalDecision::Approve,
                "full_auto (sandbox_available={sandbox}) resolves through the prompt"
            );
            assert_eq!(
                rig.prompt_count(),
                1,
                "full_auto (sandbox_available={sandbox}) must PROMPT for pty_spawn"
            );
        }
        // A denied prompt denies the spawn.
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("reject"));
        assert_eq!(rig.gate.approve(&spawn()), ApprovalDecision::Deny);
        assert_eq!(rig.prompt_count(), 1);
    }

    /// P4 §4.3/§13: the four follow-ups are NOT re-gated outside read_only
    /// (the spawn was the gated action — codex's model); read_only denies
    /// them categorically (no spawn was ever authorized under it). They
    /// match NEITHER fast path by construction (the MCP-names pin's pty
    /// analogue).
    #[test]
    fn p4_pty_followups_gate_matrix() {
        let ws = workspace();
        for name in ["pty_write", "pty_read", "pty_kill", "pty_list"] {
            assert!(
                !is_read_only_tool(name),
                "{name} must not join the read-only fast path"
            );
            assert!(
                !nano_agent::wiring::SESSION_TOOL_NAMES.contains(&name),
                "{name} must not join the auto-approve-coupled SESSION_TOOL_NAMES"
            );
            let rig = TestGate::new(PermissionMode::ReadOnly, &ws.0, true, None);
            assert_eq!(
                rig.gate
                    .approve(&call(name, serde_json::json!({"session_id": "pty_1"}))),
                ApprovalDecision::Deny,
                "read_only must deny {name}"
            );
            assert_eq!(rig.prompt_count(), 0, "read_only never prompts for {name}");
            for mode in [PermissionMode::Default, PermissionMode::FullAuto] {
                let rig = TestGate::new(mode, &ws.0, true, None);
                assert_eq!(
                    rig.gate
                        .approve(&call(name, serde_json::json!({"session_id": "pty_1"}))),
                    ApprovalDecision::Approve,
                    "{mode:?}/{name} is not re-gated"
                );
                assert_eq!(rig.prompt_count(), 0, "{mode:?}/{name} never prompts");
            }
        }
    }

    /// F-28 MEDIUM-2 (P4 §5.5): the three read-only predicates agree on
    /// `repo_map` — the acp fast path (this module) and the exec gate's
    /// decision core (which rides it). The child-gate half (tasks.rs's
    /// private predicate) is pinned in nano-agent's tasks tests; a single
    /// test cannot span the crate boundary, so the agreement is pinned by
    /// this pair landing together.
    #[test]
    fn repo_map_read_only_predicates_agree_acp_and_exec() {
        assert!(is_read_only_tool("repo_map"), "acp fast path");
        for mode in PermissionMode::ALL {
            let decision = crate::exec_mode::exec_gate_decision(
                &call("repo_map", serde_json::json!({"query": "x"})),
                mode,
                &PermissionProfile::workspace_write().file_system_sandbox_policy(),
                std::path::Path::new("."),
                false,
            );
            assert_eq!(
                decision,
                ApprovalDecision::Approve,
                "exec gate must approve repo_map in {mode:?} (read-only)"
            );
        }
        // And the PTY names agree the OTHER way: no predicate may call
        // them read-only (§4.3's arms own their approval).
        for name in nano_agent::wiring::PTY_TOOL_NAMES {
            assert!(
                !is_read_only_tool(name),
                "{name} must never be a read-only fast-path name"
            );
        }
    }

    /// F-36 (P3 §6.3): the OAuth grant producer — checked conversion,
    /// journal-side validation, append through the session coordinator.
    #[test]
    fn oauth_grant_recorder_journals_validated_grants() {
        use nano_mcp::oauth::flow::{GrantEndpoint as FlowEndpoint, GrantRecord};

        let tmp = tempfile::tempdir().unwrap();
        let journal = tmp.path().join("s.jsonl");
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal).unwrap());
        let record = |method: nano_egress::grant::HttpMethod, grant_id: &str| GrantRecord {
            grant_id: grant_id.into(),
            // §2.7 (F-P3-3): the grant keys on the stable instance id.
            server_id: "srv_0123456789abcdef".into(),
            as_origin: "https://as.example".into(),
            issuer: "https://as.example".into(),
            endpoints: vec![FlowEndpoint {
                method,
                path: "/oauth/token".into(),
            }],
        };

        let recorder = oauth_grant_recorder(coordinator.clone());
        // GET and POST journal; the op lands and replays.
        recorder(&record(nano_egress::grant::HttpMethod::Get, "g-1")).unwrap();
        let report = read_journal(&journal).unwrap();
        let grants: Vec<_> = report
            .envelopes
            .iter()
            .filter_map(|e| match &e.op {
                Op::McpOauthGrant {
                    grant_id,
                    endpoints,
                    ..
                } => Some((grant_id.clone(), endpoints.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].0, "g-1");
        assert_eq!(grants[0].1[0].method, nano_session::op::GrantMethod::Get);
        // Idempotence: the same grant id re-appends as already-durable.
        recorder(&record(nano_egress::grant::HttpMethod::Get, "g-1")).unwrap();
        let report = read_journal(&journal).unwrap();
        assert_eq!(report.envelopes.len(), 1, "grant_id is the op id");

        // Checked conversion: a method outside the journal vocabulary
        // (PUT/DELETE/PATCH/HEAD/OPTIONS) is REJECTED — never journaled as
        // an unenforceable grant.
        let err = recorder(&record(nano_egress::grant::HttpMethod::Put, "g-2"))
            .expect_err("PUT must be rejected");
        assert!(matches!(
            err,
            nano_mcp::oauth::OAuthError::Failed {
                reason: nano_mcp::oauth::FailReason::GrantRejected
            }
        ));
        let report = read_journal(&journal).unwrap();
        assert_eq!(report.envelopes.len(), 1, "rejected grant never journals");

        // Journal-bounds violations reject the same way.
        let mut oversized = record(nano_egress::grant::HttpMethod::Post, "g-3");
        oversized.as_origin = "https://x".repeat(200);
        assert!(recorder(&oversized).is_err());

        // §2.7 (F-P3-3): a display-name server_id is NOT an instance id —
        // the grant is refused (GrantRejected) and never journaled.
        let mut named = record(nano_egress::grant::HttpMethod::Post, "g-3b");
        named.server_id = "fs".into();
        let err = recorder(&named).expect_err("display-name server_id refused");
        assert!(matches!(
            err,
            nano_mcp::oauth::OAuthError::Failed {
                reason: nano_mcp::oauth::FailReason::GrantRejected
            }
        ));
        let report = read_journal(&journal).unwrap();
        assert_eq!(report.envelopes.len(), 1, "refused grants never journal");

        // Append failure (journal path replaced by a directory mid-flight)
        // surfaces the typed journal_unavailable reason.
        drop(recorder);
        drop(coordinator);
        let coordinator = Arc::new(nano_session::JournalCoordinator::open(&journal).unwrap());
        let recorder = oauth_grant_recorder(coordinator);
        std::fs::remove_file(&journal).unwrap();
        std::fs::create_dir(&journal).unwrap();
        let err = recorder(&record(nano_egress::grant::HttpMethod::Get, "g-4"))
            .expect_err("append must fail");
        assert!(matches!(
            err,
            nano_mcp::oauth::OAuthError::Failed {
                reason: nano_mcp::oauth::FailReason::JournalUnavailable
            }
        ));
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
            Arc::new(nano_session::JournalCoordinator::open(&journal).unwrap()),
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
            Arc::new(nano_session::JournalCoordinator::open(&journal).unwrap()),
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

    // ── P2a tests ────────────────────────────────────────────────────────

    fn p2a_manifest_envelopes() -> Vec<OpEnvelope> {
        use nano_session::op::{ImageRef, InputBlock};
        let reference = |digest: &str, n: u32| {
            InputBlock::ImageRef(ImageRef {
                digest: digest.into(),
                mime: "image/png".into(),
                bytes: 100,
                width: 8,
                height: 8,
                normalized_from: None,
                placeholder: format!("[Image #{n}: /tmp/{n}.png]"),
            })
        };
        vec![
            OpEnvelope::new(
                "s1-begin-1",
                "2026-08-12T00:00:00Z",
                Op::SessionBegin {
                    session_id: "s1".into(),
                    cwd: "C:\\repo".into(),
                },
            ),
            // Leading image, interleaved text, DUPLICATE image, trailing
            // image — order and multiplicity must reconstruct exactly.
            OpEnvelope::new(
                "s1-turn-1-1",
                "2026-08-12T00:00:01Z",
                Op::TurnBegin {
                    turn_id: "s1-turn-1".into(),
                    input: "[Image #1: /tmp/1.png]\nlook\n[Image #2: /tmp/2.png]\n[Image #3: /tmp/3.png]".into(),
                    input_blocks: vec![
                        reference(&"aa".repeat(32), 1),
                        InputBlock::Text {
                            text: "look".into(),
                        },
                        reference(&"bb".repeat(32), 2),
                        reference(&"aa".repeat(32), 3), // duplicate digest
                    ],
                },
            ),
        ]
    }

    /// §5.3 with NO store handle: every manifest image degrades IN POSITION
    /// to the loud placeholder (never a silent drop, never an abort — Q3
    /// RULED); text blocks survive verbatim; ordinal numbering is per-turn
    /// manifest order.
    #[test]
    fn p2a_rehydration_without_store_degrades_loudly_in_order() {
        let messages = messages_from_envelopes(&p2a_manifest_envelopes());
        assert_eq!(messages.len(), 1);
        let content = &messages[0].content;
        assert_eq!(content.len(), 4);
        assert!(
            matches!(&content[0], ContentBlock::Text { text } if text.contains("[Image #1 unavailable") && text.contains(&"aa".repeat(4)))
        );
        assert!(matches!(&content[1], ContentBlock::Text { text } if text == "look"));
        assert!(
            matches!(&content[2], ContentBlock::Text { text } if text.contains("[Image #2 unavailable"))
        );
        assert!(
            matches!(&content[3], ContentBlock::Text { text } if text.contains("[Image #3 unavailable"))
        );
        // The placeholder instructs the model not to confabulate.
        let text = match &content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!(),
        };
        assert!(text.contains("do not describe it from memory"));
    }

    /// §5.2/§12: user-authored `[Image #…]`-like text stays TEXT through the
    /// fold — no string parsing anywhere. Old journals (no manifest field)
    /// replay byte-identically to the pre-P2a fold.
    #[test]
    fn p2a_placeholder_like_text_stays_text_and_old_journals_unchanged() {
        let envelopes = vec![
            OpEnvelope::new(
                "1",
                "t",
                Op::SessionBegin {
                    session_id: "s".into(),
                    cwd: "c".into(),
                },
            ),
            OpEnvelope::new(
                "2",
                "t",
                Op::TurnBegin {
                    turn_id: "t1".into(),
                    input: "[Image #99: fake]".into(),
                    input_blocks: vec![], // serde-defaulted old journal
                },
            ),
            OpEnvelope::new(
                "3",
                "t",
                Op::TurnBegin {
                    turn_id: "t2".into(),
                    input: "see [Image #1: /tmp/a.png]".into(),
                    input_blocks: vec![nano_session::op::InputBlock::Text {
                        text: "see [Image #1: /tmp/a.png]".into(), // user-authored TEXT
                    }],
                },
            ),
        ];
        let messages = messages_from_envelopes(&envelopes);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].content,
            vec![ContentBlock::Text {
                text: "[Image #99: fake]".into()
            }]
        );
        assert_eq!(
            messages[1].content,
            vec![ContentBlock::Text {
                text: "see [Image #1: /tmp/a.png]".into()
            }]
        );
    }

    /// §8 part 2 replay-fold rule (sticky-OR): ANY journaled
    /// CompactionComplete.image_influenced=true reconstructs the flag; an
    /// UNCOMPACTED image manifest reconstructs it; a compaction-COVERED
    /// manifest with a false record does not (the record was truthful:
    /// post-eviction); and a false record never REOPENS a set flag.
    #[test]
    fn p2a_image_influenced_fold_is_sticky_or() {
        use nano_session::op::{ImageRef, InputBlock};
        let manifest_turn = |id: &str| {
            OpEnvelope::new(
                id,
                "t",
                Op::TurnBegin {
                    turn_id: id.into(),
                    input: "x".into(),
                    input_blocks: vec![InputBlock::ImageRef(ImageRef {
                        digest: "ab".repeat(32),
                        mime: "image/png".into(),
                        bytes: 1,
                        width: 1,
                        height: 1,
                        normalized_from: None,
                        placeholder: "[Image #1: /tmp/x.png]".into(),
                    })],
                },
            )
        };
        let compaction = |id: &str, covers: Vec<String>, influenced: bool| {
            OpEnvelope::new(
                id,
                "t",
                Op::CompactionComplete {
                    compaction_id: id.into(),
                    summary: "s".into(),
                    covers_op_ids: covers,
                    changed_files: vec![],
                    image_influenced: influenced,
                    mcp_hydration: None,
                },
            )
        };
        // No images anywhere → false.
        assert!(!image_influenced_from_envelopes(&[]));
        // Uncompacted image manifest → true.
        assert!(image_influenced_from_envelopes(&[manifest_turn("t1")]));
        // Any true record → true, even with a later false record (sticky-OR,
        // never latest).
        assert!(image_influenced_from_envelopes(&[
            manifest_turn("t1"),
            compaction("k1", vec!["t1".into()], true),
            compaction("k2", vec![], false),
        ]));
        // A COVERED manifest with only false records → false (the covered
        // manifest's pixels were summarized post-eviction).
        assert!(!image_influenced_from_envelopes(&[
            manifest_turn("t1"),
            compaction("k1", vec!["t1".into()], false),
        ]));
    }

    /// §5.3 two-surface split — the replay-frame CANARY: replay_frames reads
    /// ONLY the `input` projection; neither digests nor any base64 payload
    /// reach ACP replay frames.
    #[test]
    fn p2a_replay_frames_never_carry_image_bytes_or_digests() {
        let frames = replay_frames("s1", &p2a_manifest_envelopes());
        let wire = serde_json::to_string(&frames).expect("serialize");
        assert!(wire.contains("[Image #1: /tmp/1.png]"), "projection shows");
        assert!(!wire.contains(&"aa".repeat(32)), "no digest leaks");
        assert!(!wire.contains("base64,"), "no data-URL payloads");
        assert!(!wire.contains("image_ref"), "no manifest structure leaks");
    }

    /// §9.1 (D12): on an image-influenced turn, protected trust mutations
    /// ALWAYS surface the human-approval prompt — every mode, zero
    /// auto-approvals — while ordinary workspace-scoped calls keep their
    /// normal mode semantics. (The §12 screenshot-of-instructions fixture
    /// drives this same gate end-to-end in the proof legs.)
    #[test]
    fn p2a_image_influenced_clamp_forces_human_approval() {
        let ws = workspace();
        for mode in PermissionMode::ALL {
            let rig = TestGate::new(mode, &ws.0, true, Some("allow"));
            rig.set_image_influenced(true);
            // Rule amendment (contained write to AGENTS.md).
            let amend = call(
                "fs_write",
                serde_json::json!({"path": ws.0.join("AGENTS.md"), "content": "x"}),
            );
            // Credential/secret-subtree write.
            let secret = call(
                "fs_write",
                serde_json::json!({"path": ws.0.join(".secrets/flux-test-key"), "content": "x"}),
            );
            // Statically unclassifiable shell command.
            let shell = call("shell", serde_json::json!({"command": "echo hi"}));
            for protected in [amend, secret, shell] {
                let before = rig.prompt_count();
                rig.gate.approve(&protected);
                match mode {
                    PermissionMode::ReadOnly => {
                        assert_eq!(
                            rig.prompt_count(),
                            before,
                            "read_only denies categorically (stricter), no prompt"
                        );
                    }
                    _ => assert_eq!(
                        rig.prompt_count(),
                        before + 1,
                        "{mode:?}: image-influenced {} must prompt the human",
                        protected.name
                    ),
                }
            }
            // Ordinary workspace-scoped calls keep their mode semantics:
            // contained fs_write to a regular file still auto-approves in
            // full_auto under the clamp.
            let ordinary = contained_write(&ws.0);
            let before = rig.prompt_count();
            let decision = rig.gate.approve(&ordinary);
            if mode == PermissionMode::FullAuto {
                assert_eq!(decision, ApprovalDecision::Approve);
                assert_eq!(rig.prompt_count(), before, "ordinary write: no prompt");
            }
        }
        // Control: without the flag, the same AGENTS.md write auto-approves
        // in full_auto (the clamp, not the path, forces the prompt).
        let rig = TestGate::new(PermissionMode::FullAuto, &ws.0, true, Some("allow"));
        let amend = call(
            "fs_write",
            serde_json::json!({"path": ws.0.join("AGENTS.md"), "content": "x"}),
        );
        assert_eq!(rig.gate.approve(&amend), ApprovalDecision::Approve);
        assert_eq!(rig.prompt_count(), 0);
    }

    // ── P2b §3.3/§7: result-image replay/rehydration battery ─────────────

    /// A temp attachment-store home that cleans itself up on drop.
    struct TestStoreHome(std::path::PathBuf);
    impl Drop for TestStoreHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn p2b_store(tag: &str) -> (TestStoreHome, AttachmentStore) {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "nano-p2b-store-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("store home");
        let store = AttachmentStore::open(&dir).expect("attachment store");
        (TestStoreHome(dir), store)
    }

    /// A two-image live artifact set, blobs written to the store, with the
    /// second image journaled as downscaled (the §3.7 normalized-from
    /// geometry). Returns the canonical parts and the journaled envelopes.
    fn p2b_live_parts_and_envelopes(
        store: &AttachmentStore,
    ) -> (
        nano_model::image_result::ImageToolResultParts,
        Vec<OpEnvelope>,
    ) {
        let lease = store.acquire_write_lease().expect("lease");
        let digest_a = store.put(&lease, b"fake-png-one").expect("put a");
        let digest_b = store.put(&lease, b"fake-png-two").expect("put b");
        let ordered =
            |bytes: &[u8], digest: &str, normalized_from| nano_model::image_result::OrderedImage {
                bytes: bytes.to_vec(),
                mime: "image/png".into(),
                digest: digest.into(),
                width: 8,
                height: 8,
                normalized_from,
            };
        let (parts, provenance) = nano_model::image_result::build_image_tool_result(
            "c1",
            "view_image",
            vec![
                ordered(b"fake-png-one", &digest_a, None),
                ordered(b"fake-png-two", &digest_b, Some((3000, 3000))),
            ],
        )
        .expect("canonical builder");
        drop(provenance); // the live acceptance seam consumed it; the replay arm re-mints
        let envelopes = vec![
            OpEnvelope::new(
                "t-1",
                "ts",
                Op::ToolCall {
                    turn_id: "t".into(),
                    call_id: "c1".into(),
                    name: "view_image".into(),
                    args: serde_json::json!({"path": "a.png"}),
                },
            ),
            OpEnvelope::new(
                "t-2",
                "ts",
                Op::ToolResult {
                    call_id: "c1".into(),
                    ok: true,
                    output_digest: parts.output_digest.clone(),
                    changed_files: vec![],
                    error_kind: None,
                    image_refs: parts.image_refs.clone(),
                },
            ),
        ];
        (parts, envelopes)
    }

    /// Kill-resume fidelity: the envelopes make a full JSON round trip (the
    /// journaled byte shape is what a resumed session parses), then the
    /// rehydrating fold restores BOTH images byte-verified (base64 equality
    /// against the live artifacts) at the same pairing position, with the
    /// ReplayVerified provenance accepted (the message carries the images at
    /// all).
    #[test]
    fn p2b_kill_resume_result_image_rehydrates_byte_verified() {
        let (_home, store) = p2b_store("rehydrate");
        let (parts, envelopes) = p2b_live_parts_and_envelopes(&store);
        // Kill-resume honesty: serialize every envelope to its journaled
        // JSON and re-parse — the fold consumes the RESUMED bytes, never the
        // in-memory originals.
        let envelopes: Vec<OpEnvelope> = envelopes
            .iter()
            .map(|envelope| {
                serde_json::from_str(&serde_json::to_string(envelope).expect("journal serialize"))
                    .expect("journal parse")
            })
            .collect();
        let (messages, notices) = messages_from_envelopes_rehydrating(&envelopes, Some(&store));
        assert!(notices.is_empty(), "clean rehydration, zero notices");
        let result = messages
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result message");
        let ContentBlock::ToolResult {
            tool_use_id,
            content,
            images,
            ..
        } = &result.content[0]
        else {
            panic!("tool result block")
        };
        assert_eq!(tool_use_id, "c1", "same pairing position");
        assert_eq!(images, &parts.images, "byte-verified rehydration");
        // §3.3: the replayed content is the standard elision marker PLUS the
        // re-derived label lines — the deterministic parts byte-identical.
        let expected = format!(
            "[tool output elided from journal: ok=true, digest={}]\n{}",
            parts.output_digest, parts.content
        );
        assert_eq!(content, &expected, "elision marker + re-derived labels");
    }

    /// §3.3: the replayed projection is byte-identical to the live builder's
    /// — INCLUDING the §3.7 normalized-from geometry, which round-trips
    /// through the journaled refs.
    #[test]
    fn p2b_live_and_replay_result_labels_are_byte_identical() {
        let (_home, store) = p2b_store("labels");
        let (parts, envelopes) = p2b_live_parts_and_envelopes(&store);
        let (messages, _) = messages_from_envelopes_rehydrating(&envelopes, Some(&store));
        let result = messages
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result message");
        let ContentBlock::ToolResult { content, .. } = &result.content[0] else {
            panic!("tool result block")
        };
        // The first line is the standard elision marker (the tool's own free
        // text stays elided per the digest-only invariant); EVERYTHING after
        // it is the label lines, byte-identical to the live projection.
        let (marker, labels) = content.split_once('\n').expect("marker + labels");
        assert_eq!(
            marker,
            format!(
                "[tool output elided from journal: ok=true, digest={}]",
                parts.output_digest
            )
        );
        assert_eq!(labels, parts.content, "live == replay label lines");
        assert!(
            labels
                .contains("[Image #2 from tool view_image — 8x8 png (normalized from 3000x3000)]"),
            "the normalized-from geometry survives the journal: {labels}"
        );
    }

    /// §3.3/§7 replay index: a result with NO journaled call, or with a
    /// DUPLICATE call id, resolves to the deterministic unavailable name
    /// (never an inferred name) — and the pixels are STILL digest-verified
    /// and rehydrated (the name is the only casualty).
    #[test]
    fn p2b_replay_index_unpaired_and_duplicate_calls_degrade_name_only() {
        let (_home, store) = p2b_store("index");
        let (parts, envelopes) = p2b_live_parts_and_envelopes(&store);
        let result_envelope = envelopes[1].clone();
        let call_envelope = |name: &str, id: &str| {
            OpEnvelope::new(
                id,
                "ts",
                Op::ToolCall {
                    turn_id: "t".into(),
                    call_id: "c1".into(),
                    name: name.into(),
                    args: serde_json::json!({}),
                },
            )
        };
        for (tag, envelopes) in [
            ("unpaired", vec![result_envelope.clone()]),
            (
                "duplicate",
                vec![
                    call_envelope("view_image", "t-d1"),
                    call_envelope("view_image", "t-d2"),
                    result_envelope,
                ],
            ),
        ] {
            let (messages, notices) = messages_from_envelopes_rehydrating(&envelopes, Some(&store));
            assert!(notices.is_empty(), "{tag}: pixels still verify clean");
            let result = messages
                .iter()
                .find(|message| message.role == Role::Tool)
                .unwrap_or_else(|| panic!("{tag}: tool result message"));
            let ContentBlock::ToolResult {
                content, images, ..
            } = &result.content[0]
            else {
                panic!("{tag}: tool result block")
            };
            assert!(
                content.contains("<unavailable: unpaired call>"),
                "{tag}: the deterministic unavailable name, never inferred: {content}"
            );
            assert_eq!(
                images, &parts.images,
                "{tag}: pixels still digest-verified and rehydrated"
            );
        }
    }

    /// §3.3 (M3 regression): a flipped blob byte logs TAMPERED, a deleted
    /// blob logs MISSING — never collapsed — and both degrade to the loud
    /// placeholder line with zero images attached.
    #[test]
    fn p2b_replay_distinguishes_tampered_from_missing() {
        let (_home, store) = p2b_store("causes");
        let (parts, _) = p2b_live_parts_and_envelopes(&store);
        let result_op = |image_refs: Vec<nano_session::op::ImageRef>| {
            vec![
                OpEnvelope::new(
                    "t-1",
                    "ts",
                    Op::ToolCall {
                        turn_id: "t".into(),
                        call_id: "c1".into(),
                        name: "view_image".into(),
                        args: serde_json::json!({}),
                    },
                ),
                OpEnvelope::new(
                    "t-2",
                    "ts",
                    Op::ToolResult {
                        call_id: "c1".into(),
                        ok: true,
                        output_digest: "d".into(),
                        changed_files: vec![],
                        error_kind: None,
                        image_refs,
                    },
                ),
            ]
        };
        // MISSING: a ref whose blob was never written.
        let missing_ref = nano_session::op::ImageRef {
            digest: "ef".repeat(32),
            ..parts.image_refs[0].clone()
        };
        let (messages, notices) =
            messages_from_envelopes_rehydrating(&result_op(vec![missing_ref]), Some(&store));
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].cause, AttachmentIssueCause::Missing);
        let ContentBlock::ToolResult {
            content, images, ..
        } = &messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("result")
            .content[0]
        else {
            panic!("result block")
        };
        assert!(images.is_empty(), "no pixels without verification");
        assert!(
            content.contains(
                "unavailable: attachment efefefefefef missing — do not describe it from memory"
            ),
            "the loud line: {content}"
        );
        // TAMPERED: flip one byte of the FIRST blob on disk.
        let digest = &parts.image_refs[0].digest;
        let blob = store.root().join("blobs").join(&digest[..2]).join(digest);
        let mut bytes = std::fs::read(&blob).expect("blob");
        bytes[0] ^= 0xFF;
        std::fs::write(&blob, bytes).expect("flip");
        let (messages, notices) = messages_from_envelopes_rehydrating(
            &result_op(vec![parts.image_refs[0].clone()]),
            Some(&store),
        );
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].cause,
            AttachmentIssueCause::Tampered,
            "operator logs distinguish TAMPERED from MISSING"
        );
        let ContentBlock::ToolResult { images, .. } = &messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("result")
            .content[0]
        else {
            panic!("result block")
        };
        assert!(images.is_empty(), "tampered pixels never reattach");
    }

    /// §3.3 canary, extended to image_refs (the P2a replay-frame canary's
    /// sibling): replay_frames reads ONLY the digest-only projections —
    /// neither image bytes nor digests nor the manifest structure reach ACP
    /// replay frames.
    #[test]
    fn p2b_replay_frames_never_carry_result_image_bytes_or_digests() {
        let (_home, store) = p2b_store("canary");
        let (parts, envelopes) = p2b_live_parts_and_envelopes(&store);
        let frames = replay_frames("s1", &envelopes);
        let wire = serde_json::to_string(&frames).expect("serialize");
        for reference in &parts.image_refs {
            assert!(!wire.contains(&reference.digest), "no digest leaks");
        }
        assert!(!wire.contains("base64,"), "no data payloads");
        assert!(!wire.contains("image_ref"), "no manifest structure leaks");
        assert!(
            !wire.contains("from tool view_image"),
            "no label lines in replay frames (projection-only surface)"
        );
    }

    /// §3.2 honest compat, pinned: an OLD reader ignores `image_refs`
    /// entirely, so a pre-P2b binary replays the EXACT generic elision
    /// marker — no label lines, no pixels. The degradation is a contract,
    /// not a surprise.
    #[test]
    fn p2b_old_reader_degraded_replay_text_pinned() {
        let (_home, store) = p2b_store("degraded");
        let (parts, envelopes) = p2b_live_parts_and_envelopes(&store);
        // What the old reader sees: the op with image_refs stripped (the
        // field it never knew), parsed by the pre-P2b schema.
        let mut value = serde_json::to_value(&envelopes[1]).expect("serialize");
        value["op"]
            .as_object_mut()
            .expect("op object")
            .remove("image_refs");
        let old_read: OpEnvelope = serde_json::from_value(value).expect("old-reader parse");
        let messages = messages_from_envelopes(&[envelopes[0].clone(), old_read]);
        let result = messages
            .iter()
            .find(|message| message.role == Role::Tool)
            .expect("tool result message");
        let ContentBlock::ToolResult {
            content, images, ..
        } = &result.content[0]
        else {
            panic!("tool result block")
        };
        assert!(images.is_empty(), "old reader: zero pixels");
        assert_eq!(
            content,
            &format!(
                "[tool output elided from journal: ok=true, digest={}]",
                parts.output_digest
            ),
            "the EXACT degraded replay text is pinned"
        );
        assert!(!content.contains("[Image"), "no label lines, no markers");
    }

    /// §3.5/§7: a CompactionComplete folds through the canonical builder on
    /// replay, so a resumed post-compaction context with a result image in
    /// the compacted prefix is byte-identical to the live compaction.
    #[test]
    fn p2b_compacted_replay_with_result_images_byte_identical_to_live() {
        let (_home, store) = p2b_store("compacted");
        let (parts, envelopes) = p2b_live_parts_and_envelopes(&store);
        let summary = "summary text".to_string();
        let mut compacted_envelopes = envelopes.clone();
        compacted_envelopes.push(OpEnvelope::new(
            "t-3",
            "ts",
            Op::CompactionComplete {
                compaction_id: "k1".into(),
                summary: summary.clone(),
                covers_op_ids: vec!["t-1".into(), "t-2".into()],
                changed_files: vec![],
                image_influenced: true,
                mcp_hydration: None,
            },
        ));
        let (replayed, notices) =
            messages_from_envelopes_rehydrating(&compacted_envelopes, Some(&store));
        assert!(notices.is_empty());
        // The live side: the same pre-compaction context through the SAME
        // canonical builder.
        let live = vec![
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "c1".into(),
                    name: "view_image".into(),
                    input: serde_json::json!({"path": "a.png"}),
                }],
            },
            Message::tool_result_with_images(
                "c1",
                parts.content.clone(),
                false,
                parts.images.clone(),
            ),
        ];
        let live_compacted = nano_agent::compact::build_compacted_history(live, &summary);
        assert_eq!(replayed, live_compacted, "live == replay, byte-identical");
    }

    /// §3.6/§8 part 2: the resume fold counts result-image manifests by
    /// PRESENCE — a covered manifest with a false record clears, an
    /// uncompacted result-image manifest sets, sticky-OR over records.
    #[test]
    fn p2b_image_influenced_fold_counts_result_image_manifests() {
        let result_manifest = |id: &str| {
            OpEnvelope::new(
                id,
                "t",
                Op::ToolResult {
                    call_id: "c".into(),
                    ok: true,
                    output_digest: "d".into(),
                    changed_files: vec![],
                    error_kind: None,
                    image_refs: vec![nano_session::op::ImageRef {
                        digest: "ab".repeat(32),
                        mime: "image/png".into(),
                        bytes: 1,
                        width: 1,
                        height: 1,
                        normalized_from: None,
                        placeholder: "[Image #1 from tool view_image — 1x1 png]".into(),
                    }],
                },
            )
        };
        let compaction = |id: &str, covers: Vec<String>, influenced: bool| {
            OpEnvelope::new(
                id,
                "t",
                Op::CompactionComplete {
                    compaction_id: id.into(),
                    summary: "s".into(),
                    covers_op_ids: covers,
                    changed_files: vec![],
                    image_influenced: influenced,
                    mcp_hydration: None,
                },
            )
        };
        // Uncompacted result-image manifest → influential (blob deleted or
        // not — presence is the contract).
        assert!(image_influenced_from_envelopes(&[result_manifest("r1")]));
        // Covered by a false record → the pixels were summarized post-
        // eviction; the flag stays clear.
        assert!(!image_influenced_from_envelopes(&[
            result_manifest("r1"),
            compaction("k1", vec!["r1".into()], false),
        ]));
        // Any true record sticks, even covered and followed by false.
        assert!(image_influenced_from_envelopes(&[
            result_manifest("r1"),
            compaction("k1", vec!["r1".into()], true),
            compaction("k2", vec![], false),
        ]));
    }
}
