//! MCP integration for the agent loop: server registry, tool exposure, and
//! execution routing through the ToolExecutor seam.
//!
//! Rules: MCP tools are namespaced `mcp__<server>__<tool>`; servers get no
//! security bypass (stdio children die with the registry — no orphans);
//! at-least-once retry semantics are surfaced, never hidden.
//!
//! P3 (design note §3/§4): ToolSearch exposure tiers (direct ∪ hydrated only
//! ever reach a model request), journaled hydration with the canonical
//! digest gate and the churn breaker, and resources v1 (capability-gated
//! list/read with the fail-closed advertised-URI check).

use crate::loop_protection::ProgressSignals;
use crate::turn::ToolExecutor;
use crate::turn::ToolOutcome;
use nano_mcp::client::{McpClient, McpError};
use nano_mcp::dispatcher::{ConnectionOptions, ServerRequestHandler, SlotRetired};
use nano_mcp::protocol::{McpResourceDescriptor, McpToolDescriptor};
use nano_model::types::{ToolCall, ToolDefinition};
use nano_session::{HydrationEntry, NanoErrorKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// §3.1 thresholds — one place, pinned by test. Overrides (when they land)
// are TIGHTENING-ONLY: lower defers more, never less.
// ---------------------------------------------------------------------------

/// A server whose sanitized schemas exceed this many bytes is ALL Deferred.
pub const DEFER_SCHEMA_BYTES: usize = 32 * 1024;
/// A server advertising more tools than this is Deferred (many-tiny-schemas).
pub const DEFER_TOOL_COUNT: usize = 20;
/// Global direct-exposure schema budget, divided into per-server fair shares.
pub const GLOBAL_DIRECT_SCHEMA_BYTES: usize = 96 * 1024;
/// Inventory hard cap [r2 codex-F8]: over this, the server registers ZERO
/// tools with a bounded loud startup warning.
pub const MAX_INVENTORY_TOOLS: usize = 500;
/// Inventory hard cap in sanitized schema bytes (2 MiB).
pub const MAX_INVENTORY_SCHEMA_BYTES: usize = 2 * 1024 * 1024;
/// Descriptions are untrusted server-authored content (§3.6): truncated to
/// this many chars and control-stripped before ANY use.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
/// tool_search fixed limit — no limit knob in v1.
pub const TOOL_SEARCH_LIMIT: usize = 10;
/// §3.5: the pre-hydration per-server source listing is bounded to this
/// many bytes total.
pub const SOURCE_LISTING_MAX_BYTES: usize = 4 * 1024;

/// Churn breaker (§3.4): this many digest transitions inside the carried
/// window pins the server Deferred for the session.
const CHURN_TRANSITION_LIMIT: usize = 3;
/// Startup warnings are loud but bounded — a pathological config cannot
/// turn the notice channel into a memory sink.
const MAX_STARTUP_WARNINGS: usize = 64;
/// The wcore-honest status line (§3.2) — kills the re-search death loop.
pub const TOOL_SEARCH_STATUS: &str =
    "LOADED — these tools are now callable by name; searching again returns the same result";

/// §3.1 exposure tiers. `Hidden` is the V2 marketplace seam
/// (installed-but-inactive): NO registration path produces it in P3 — zero
/// logic consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Direct,
    Deferred,
    Hidden,
}

/// Pure per-server classification (§3.1) — registration-order independent
/// given the configured server count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InventoryClass {
    Direct,
    Deferred,
    /// Hard caps exceeded: zero tools registered, loud bounded warning.
    Blocked,
}

/// The per-server direct share of the global budget (§3.1, Q2 RULED).
fn fair_share(configured_server_count: usize) -> usize {
    DEFER_SCHEMA_BYTES.min(GLOBAL_DIRECT_SCHEMA_BYTES / configured_server_count.max(1))
}

fn classify_inventory(
    tool_count: usize,
    schema_bytes: usize,
    configured_server_count: usize,
) -> InventoryClass {
    // Hard caps first [r2 codex-F8]: a pathological inventory never reaches
    // the index or the digest computation.
    if tool_count > MAX_INVENTORY_TOOLS || schema_bytes > MAX_INVENTORY_SCHEMA_BYTES {
        return InventoryClass::Blocked;
    }
    // Primary rule (bytes), secondary rule (count).
    if schema_bytes > DEFER_SCHEMA_BYTES || tool_count > DEFER_TOOL_COUNT {
        return InventoryClass::Deferred;
    }
    // Fair-share budget: every configured server gets the same shot at
    // direct exposure regardless of config order.
    if schema_bytes <= fair_share(configured_server_count) {
        InventoryClass::Direct
    } else {
        InventoryClass::Deferred
    }
}

/// §3.6 sanitization: control characters stripped, then char-safe
/// truncation to MAX_DESCRIPTION_CHARS. Returns (sanitized, truncated).
pub(crate) fn sanitize_description(raw: Option<&str>) -> (Option<String>, bool) {
    let Some(raw) = raw else { return (None, false) };
    let stripped: String = raw.chars().filter(|c| !c.is_control()).collect();
    if stripped.chars().count() > MAX_DESCRIPTION_CHARS {
        (
            Some(stripped.chars().take(MAX_DESCRIPTION_CHARS).collect()),
            true,
        )
    } else {
        (Some(stripped), false)
    }
}

// ---------------------------------------------------------------------------
// §3.4 canonical digest: sha256 over the canonical JSON of the server's
// sorted stable tool identities ({name, input_schema} with recursively
// sorted keys and no whitespace). `description` is EXCLUDED — it is
// sanitized display text, not authority.
// ---------------------------------------------------------------------------

fn canonical_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).expect("string serializes"),
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", body.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).expect("key serializes"),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The §3.4 canonical tools digest: a name or input-schema change
/// invalidates hydration; a description-only change never does; key order
/// and insignificant whitespace are irrelevant.
pub fn canonical_tools_digest(tools: &[McpToolDescriptor]) -> String {
    let mut entries: Vec<(&str, String)> = tools
        .iter()
        .map(|tool| {
            let schema = tool.input_schema.clone().unwrap_or(serde_json::Value::Null);
            // Object keys sorted: "input_schema" < "name".
            let entry = format!(
                "{{\"input_schema\":{},\"name\":{}}}",
                canonical_json(&schema),
                serde_json::to_string(&tool.name).expect("name serializes")
            );
            (tool.name.as_str(), entry)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let body: Vec<&str> = entries.iter().map(|(_, e)| e.as_str()).collect();
    sha256_hex(format!("[{}]", body.join(",")).as_bytes())
}

/// Digest transitions over a churn window: consecutive entries that differ.
fn count_transitions(window: &[String]) -> usize {
    window.windows(2).filter(|w| w[0] != w[1]).count()
}

// ---------------------------------------------------------------------------
// tool_search outcome types (§3.2/§3.3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSearchHit {
    pub server_id: String,
    pub tool: String,
    pub namespaced: String,
}

/// One search result: matched names, the honest truncation count, the ONE
/// hydration batch (journal-first — the host appends, THEN calls
/// `apply_hydration`), bounded notices, and the fixed status line.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSearchOutcome {
    pub hits: Vec<ToolSearchHit>,
    /// Matches beyond the fixed limit-10 ("N more matches — refine your
    /// query"); honest and bounded, no ranking claimed.
    pub more: usize,
    pub hydration: Vec<HydrationEntry>,
    pub notices: Vec<String>,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Resources v1 types (§4.3)
// ---------------------------------------------------------------------------

/// A typed resource-lane failure. `kind` is exactly one of
/// McpResourceUnsupported / McpResourceDenied / McpContentUnsupported for
/// the three gates; other client failures map via
/// `crate::error_map::kind_of_mcp`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceError {
    pub kind: NanoErrorKind,
    pub message: String,
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ResourceError {}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceListing {
    pub server: String,
    pub resources: Vec<McpResourceDescriptor>,
    /// A `nextCursor` was returned: the page is served truncated and the
    /// cursor is NEVER followed in v1 (§4.1).
    pub truncated: bool,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceText {
    pub server: String,
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: String,
}

/// Per-connection advertised-URI cache (§4.3): refreshed ONLY by an
/// explicit list call — no background re-list (§2.5).
#[derive(Debug, Clone, Default)]
struct ResourceCache {
    uris: BTreeSet<String>,
    truncated: bool,
}

pub(crate) fn resource_error_of_mcp(err: McpError) -> ResourceError {
    let kind = match &err {
        McpError::ResourceUnsupported => NanoErrorKind::McpResourceUnsupported,
        McpError::ContentUnsupported => NanoErrorKind::McpContentUnsupported,
        other => crate::error_map::kind_of_mcp(other),
    };
    ResourceError {
        kind,
        message: err.to_string(),
    }
}

/// §4.3 fail-closed URI check (enforcing function): only a URI the
/// server's most recent resources/list advertised may be read; anything
/// else is a typed denial with NO wire call — never a fetch of a
/// server-invented URI.
fn validate_resource_uri(
    cache: &BTreeMap<String, ResourceCache>,
    server: &str,
    uri: &str,
) -> Result<(), ResourceError> {
    let advertised = cache
        .get(server)
        .is_some_and(|entry| entry.uris.contains(uri));
    if advertised {
        Ok(())
    } else {
        Err(ResourceError {
            kind: NanoErrorKind::McpResourceDenied,
            message: format!(
                "URI '{uri}' was not advertised by MCP server '{server}' — call mcp_list_resources first"
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Elicitation plumbing seam (§5; the bridge itself is another lane)
// ---------------------------------------------------------------------------

/// The two halves the elicitation bridge installs per connection.
pub struct ElicitationHandlerParts {
    pub handler: Arc<dyn ServerRequestHandler>,
    pub slot_retired_hook: Arc<dyn Fn(SlotRetired) + Send + Sync>,
}

/// Factory the elicitation lane installs before registration. Receives the
/// server name and the registry-owned interrupted-call cell (the executor
/// sets it to `Some(call.id)` around each dispatched call for
/// interrupted-call attribution in the elicitation journal record).
pub type ElicitationHandlerFactory =
    Arc<dyn Fn(&str, Arc<Mutex<Option<String>>>) -> ElicitationHandlerParts + Send + Sync>;

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

struct ServerEntry {
    spec: McpServerSpec,
    client: McpClient,
    /// Sanitized descriptors (§3.6) — the ONLY form ever served (index,
    /// definitions, listing). Empty when the inventory was blocked.
    tools: Vec<McpToolDescriptor>,
    /// As advertised at registration (pre-cap), for warning/notice text.
    tool_count: usize,
    schema_bytes: usize,
    exposure: ToolExposure,
    blocked: bool,
    /// Hydrated tool names (exposure only, §3.3 — never authority).
    hydrated: BTreeSet<String>,
    /// §3.4 churn window: recent hydration digests, cap 8, drop oldest.
    churn_window: Vec<String>,
    /// Churn breaker: pinned Deferred for the session.
    pinned: bool,
    /// Elicitation interrupted-call attribution cell (None when the
    /// elicitation lane is not installed).
    interrupted_call: Arc<Mutex<Option<String>>>,
}

pub struct McpRegistry {
    servers: Vec<ServerEntry>,
    /// Bounded, loud startup/session warnings (inventory blocks, churn
    /// pins). The host surfaces these; they are never silent.
    pub startup_warnings: Vec<String>,
    description_truncations: usize,
    configured_server_count: Option<usize>,
    resource_cache: BTreeMap<String, ResourceCache>,
    elicitation_factory: Option<ElicitationHandlerFactory>,
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRegistry")
            .field("servers", &self.servers.len())
            .finish()
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            servers: vec![],
            startup_warnings: vec![],
            description_truncations: 0,
            configured_server_count: None,
            resource_cache: BTreeMap::new(),
            elicitation_factory: None,
        }
    }

    /// The fair-share divisor (§3.1). Set to the number of configured
    /// servers BEFORE registration so admission is registration-order
    /// independent; unset falls back to servers-registered-so-far + 1.
    pub fn set_configured_server_count(&mut self, n: usize) {
        self.configured_server_count = Some(n);
    }

    /// Installs the elicitation bridge factory (§5 seam). Subsequent
    /// `register` calls connect with the factory-produced request handler
    /// and slot-retired hook; servers registered before this call are
    /// unaffected (no handler, no advertised capability — honest).
    pub fn set_elicitation_handler_factory(&mut self, factory: Option<ElicitationHandlerFactory>) {
        self.elicitation_factory = factory;
    }

    fn warn(&mut self, message: String) {
        eprintln!("wayland-nano mcp: {message}");
        if self.startup_warnings.len() < MAX_STARTUP_WARNINGS {
            self.startup_warnings.push(message);
        }
    }

    /// Connects to a server and registers its tools under the §3.1 exposure
    /// rules. A failed handshake fails the registration, never the runtime.
    /// Returns the number of tools registered (0 when the inventory hit the
    /// hard caps).
    pub fn register(&mut self, spec: McpServerSpec) -> Result<usize, McpError> {
        let interrupted_call: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let client = match &self.elicitation_factory {
            Some(factory) => {
                let parts = factory(&spec.name, interrupted_call.clone());
                McpClient::connect_with_options(
                    &spec.command,
                    &spec.args,
                    &spec.env,
                    ConnectionOptions {
                        request_handler: parts.handler,
                        slot_retired_hook: parts.slot_retired_hook,
                        ..ConnectionOptions::default()
                    },
                )?
            }
            None => McpClient::connect(&spec.command, &spec.args, &spec.env)?,
        };
        // §2.7 (deviation 3): consume the connect-time tools/list cache —
        // no duplicate wire call.
        let mut truncated_count = 0usize;
        let tools: Vec<McpToolDescriptor> = client
            .cached_tools()
            .iter()
            .map(|tool| {
                let (description, truncated) = sanitize_description(tool.description.as_deref());
                truncated_count += usize::from(truncated);
                McpToolDescriptor {
                    name: tool.name.clone(),
                    description,
                    input_schema: tool.input_schema.clone(),
                }
            })
            .collect();
        if truncated_count > 0 {
            self.description_truncations += truncated_count;
            eprintln!(
                "wayland-nano mcp: server '{}': {} tool description(s) truncated to {} chars",
                spec.name, truncated_count, MAX_DESCRIPTION_CHARS
            );
        }
        let tool_count = tools.len();
        let schema_bytes = tools
            .iter()
            .map(|tool| serde_json::to_vec(tool).map(|v| v.len()).unwrap_or(0))
            .sum::<usize>();
        let configured = self
            .configured_server_count
            .unwrap_or(self.servers.len() + 1);
        let class = classify_inventory(tool_count, schema_bytes, configured);
        let (exposure, blocked, tools) = match class {
            InventoryClass::Blocked => {
                self.warn(format!(
                    "MCP server '{}': inventory blocked ({} tools / {} schema bytes exceed the hard caps of {} tools / {} bytes); zero tools registered",
                    spec.name, tool_count, schema_bytes, MAX_INVENTORY_TOOLS, MAX_INVENTORY_SCHEMA_BYTES
                ));
                (ToolExposure::Deferred, true, Vec::new())
            }
            InventoryClass::Deferred => (ToolExposure::Deferred, false, tools),
            InventoryClass::Direct => (ToolExposure::Direct, false, tools),
        };
        let kept = tools.len();
        self.servers.push(ServerEntry {
            spec,
            client,
            tools,
            tool_count,
            schema_bytes,
            exposure,
            blocked,
            hydrated: BTreeSet::new(),
            churn_window: Vec::new(),
            pinned: false,
            interrupted_call,
        });
        Ok(kept)
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// The MCP tool surface as agent-facing tool definitions, namespaced.
    /// §3.1: direct ∪ hydrated ONLY — deferred tools appear in NO model
    /// request.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut out = Vec::new();
        for entry in &self.servers {
            for tool in &entry.tools {
                let exposed = entry.exposure == ToolExposure::Direct
                    || (!entry.pinned && entry.hydrated.contains(&tool.name));
                if exposed {
                    out.push(namespaced_definition(&entry.spec.name, tool));
                }
            }
        }
        out
    }

    /// §3.4 dispatch-time membership check: is this namespaced tool in the
    /// live direct ∪ hydrated set? A tool never hydrated, dropped at
    /// resume, or on a pinned server is NOT exposed.
    pub fn is_exposed(&self, namespaced: &str) -> bool {
        let Some((server, tool)) = namespaced
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        else {
            return false;
        };
        self.servers.iter().any(|entry| {
            entry.spec.name == server
                && entry.tools.iter().any(|t| t.name == tool)
                && (entry.exposure == ToolExposure::Direct
                    || (!entry.pinned && entry.hydrated.contains(tool)))
        })
    }

    /// Whether any server carries searchable deferred tools (the §3.2
    /// advertisement double-guard).
    pub fn has_deferred_tools(&self) -> bool {
        self.servers
            .iter()
            .any(|entry| entry.exposure == ToolExposure::Deferred && !entry.tools.is_empty())
    }

    /// §3.5 pre-hydration source listing: per-server display name +
    /// deferred count + first 8 tool names, deterministic (registration)
    /// order, bounded to SOURCE_LISTING_MAX_BYTES total.
    pub fn deferred_source_listing(&self) -> String {
        let rows: Vec<(String, usize, Vec<String>)> = self
            .servers
            .iter()
            .filter(|entry| entry.exposure == ToolExposure::Deferred && !entry.tools.is_empty())
            .map(|entry| {
                (
                    entry.spec.name.clone(),
                    entry.tools.len(),
                    entry.tools.iter().take(8).map(|t| t.name.clone()).collect(),
                )
            })
            .collect();
        render_source_listing(&rows)
    }

    /// §3.2: token-AND search over DEFERRED tools only. Every
    /// whitespace-separated token must appear (case-insensitive) in the
    /// namespaced name or the sanitized description. Cancel-checked every
    /// 100 scanned items. Hydration state does NOT mutate here — the host
    /// journals the batch first, then calls `apply_hydration`.
    pub fn tool_search(
        &self,
        query: &str,
        cancel: Option<&AtomicBool>,
    ) -> Result<ToolSearchOutcome, NanoErrorKind> {
        let tokens: Vec<String> = query
            .split_whitespace()
            .map(|token| token.to_lowercase())
            .collect();
        if tokens.is_empty() {
            // Never an empty-match flood.
            return Err(NanoErrorKind::InvalidParams);
        }
        let mut notices: Vec<String> = Vec::new();
        let mut matched: Vec<ToolSearchHit> = Vec::new();
        let mut over_cap_noticed: BTreeSet<String> = BTreeSet::new();
        let mut scanned = 0usize;
        for entry in &self.servers {
            if entry.blocked {
                notices.push(format!(
                    "MCP server '{}': inventory exceeds the hard caps ({} tools / {} bytes); not searchable this session",
                    entry.spec.name, entry.tool_count, entry.schema_bytes
                ));
                continue;
            }
            if entry.pinned {
                notices.push(format!(
                    "MCP server '{}': pinned Deferred after digest churn; hydration refused this session",
                    entry.spec.name
                ));
                continue;
            }
            if entry.exposure != ToolExposure::Deferred {
                continue;
            }
            for tool in &entry.tools {
                scanned += 1;
                if scanned % 100 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
                    return Err(NanoErrorKind::UserCancelled);
                }
                if tool.name.chars().count() > nano_session::MAX_HYDRATION_TOOL_NAME_CHARS {
                    // The journal batch would reject this name — it can
                    // never be hydrated, so it is never a hit (fail-closed,
                    // bounded notice).
                    if over_cap_noticed.insert(entry.spec.name.clone()) {
                        notices.push(format!(
                            "MCP server '{}': tool name(s) over the {}-char journal cap are not searchable",
                            entry.spec.name,
                            nano_session::MAX_HYDRATION_TOOL_NAME_CHARS
                        ));
                    }
                    continue;
                }
                let namespaced = format!("mcp__{}__{}", entry.spec.name, tool.name);
                let haystack = format!(
                    "{} {}",
                    namespaced.to_lowercase(),
                    tool.description.clone().unwrap_or_default().to_lowercase()
                );
                if tokens.iter().all(|token| haystack.contains(token)) {
                    matched.push(ToolSearchHit {
                        server_id: entry.spec.name.clone(),
                        tool: tool.name.clone(),
                        namespaced,
                    });
                }
            }
        }
        let total = matched.len();
        let hits: Vec<ToolSearchHit> = matched.into_iter().take(TOOL_SEARCH_LIMIT).collect();
        let more = total - hits.len();
        if more > 0 {
            notices.push(format!("{more} more matches — refine your query"));
        }
        // ONE hydration batch, grouped by server in hit order, capped at
        // MAX_HYDRATION_ENTRIES (8) servers.
        let (mut hydration, capped) = build_hydration_batch(&hits, |server| {
            self.servers
                .iter()
                .find(|e| e.spec.name == server)
                .map(|e| canonical_tools_digest(&e.tools))
        });
        if capped {
            notices.push(format!(
                "hydration capped at {} servers — refine your query to load the rest",
                nano_session::MAX_HYDRATION_ENTRIES
            ));
        }
        if !hydration.is_empty()
            && let Err(rule) = nano_session::validate_hydration_batch(&hydration)
        {
            // Fail-closed: an unjournalable batch is never offered.
            notices.push(format!(
                "hydration batch rejected by bounds validation ({rule}); no tools offered"
            ));
            hydration.clear();
        }
        Ok(ToolSearchOutcome {
            hits,
            more,
            hydration,
            notices,
            status: TOOL_SEARCH_STATUS.to_string(),
        })
    }

    /// §3.3: unions journaled tool names into the per-server hydrated set,
    /// pushes each server's digest onto its churn window (cap 8, drop
    /// oldest), then re-evaluates the churn breaker. Call only AFTER the
    /// journal append succeeded (journal-first ordering).
    pub fn apply_hydration(&mut self, entries: &[HydrationEntry]) {
        for entry in entries {
            let Some(index) = self
                .servers
                .iter()
                .position(|e| e.spec.name == entry.server_id)
            else {
                continue;
            };
            if self.servers[index].pinned {
                let name = self.servers[index].spec.name.clone();
                self.warn(format!(
                    "MCP server '{name}': hydration offer refused (pinned Deferred after digest churn)"
                ));
                continue;
            }
            let server = &mut self.servers[index];
            for name in &entry.tool_names {
                server.hydrated.insert(name.clone());
            }
            server.churn_window.push(entry.tools_digest.clone());
            if server.churn_window.len() > nano_session::MAX_RECENT_DIGESTS {
                server.churn_window.remove(0);
            }
            if count_transitions(&server.churn_window) >= CHURN_TRANSITION_LIMIT {
                server.pinned = true;
                server.hydrated.clear();
                let name = server.spec.name.clone();
                self.warn(format!(
                    "MCP server '{name}': tools digest churned (>= {CHURN_TRANSITION_LIMIT} transitions in the last {} hydrations); pinned Deferred for the session",
                    nano_session::MAX_RECENT_DIGESTS
                ));
            }
        }
    }

    /// §3.4 reconnect gate. For each journaled server present in the
    /// registry: digest match re-applies the hydrated set + churn window;
    /// mismatch drops that server's entries with a loud notice. Journaled
    /// servers absent from the registry are ignored silently. Returns the
    /// notices for the host to surface as session/update lines.
    pub fn resume_hydration(
        &mut self,
        hydrated: &BTreeMap<String, BTreeSet<String>>,
        digests: &BTreeMap<String, String>,
        windows: &BTreeMap<String, Vec<String>>,
    ) -> Vec<String> {
        let mut notices = Vec::new();
        for (server_id, journaled_digest) in digests {
            let Some(index) = self.servers.iter().position(|e| e.spec.name == *server_id) else {
                // Server not configured this session — ignored silently.
                continue;
            };
            let fresh = canonical_tools_digest(&self.servers[index].tools);
            if fresh != *journaled_digest {
                // Security-relevant inventory change: drop-and-notify.
                self.servers[index].hydrated.clear();
                self.servers[index].churn_window.clear();
                let dropped = hydrated.get(server_id).map(|s| s.len()).unwrap_or(0);
                notices.push(format!(
                    "MCP server '{server_id}': {dropped} hydrated tools dropped (inventory changed); use tool_search to re-hydrate"
                ));
                continue;
            }
            if let Some(set) = hydrated.get(server_id) {
                self.servers[index].hydrated = set.clone();
            }
            let mut window = windows.get(server_id).cloned().unwrap_or_default();
            if window.len() > nano_session::MAX_RECENT_DIGESTS {
                window.drain(..window.len() - nano_session::MAX_RECENT_DIGESTS);
            }
            if count_transitions(&window) >= CHURN_TRANSITION_LIMIT {
                self.servers[index].pinned = true;
                self.servers[index].hydrated.clear();
                notices.push(format!(
                    "MCP server '{server_id}': tools digest churned (>= {CHURN_TRANSITION_LIMIT} transitions in the restored window); pinned Deferred for the session"
                ));
            }
            self.servers[index].churn_window = window;
        }
        notices
    }

    // -------------------------------------------------------------------
    // Resources v1 (§4)
    // -------------------------------------------------------------------

    /// §4.2/§4.3 gate + handle resolution (§2.1 defect 5 / §5.2: "lock to
    /// clone, never to wait"). Takes `&self`, resolves the server, runs the
    /// capability gate BEFORE any wire activity, and returns the CLONED
    /// client — the caller drops the registry lock before the round trip.
    pub fn resource_client_for(&self, server: &str) -> Result<McpClient, ResourceError> {
        let Some(entry) = self.servers.iter().find(|e| e.spec.name == server) else {
            return Err(ResourceError {
                kind: NanoErrorKind::UnknownTool,
                message: format!("unknown MCP server: {server}"),
            });
        };
        if !entry.client.negotiated().is_some_and(|n| n.resources) {
            // Zero wire activity without the negotiated capability.
            return Err(ResourceError {
                kind: NanoErrorKind::McpResourceUnsupported,
                message: format!("MCP server '{server}' does not support resources"),
            });
        }
        Ok(entry.client.clone())
    }

    /// The fail-closed advertised-URI check (§4.3), `&self`: only a URI the
    /// server's most recent resources/list advertised may be read; anything
    /// else is a typed denial with NO wire call.
    pub fn validate_advertised_uri(&self, server: &str, uri: &str) -> Result<(), ResourceError> {
        validate_resource_uri(&self.resource_cache, server, uri)
    }

    /// The cache refresh (§4.3: ONLY an explicit list call refreshes; no
    /// background re-list, §2.5). Called AFTER the wire round trip completes,
    /// under a fresh short lock. Returns the truncation notices.
    pub fn cache_resource_listing(
        &mut self,
        server: &str,
        result: &nano_mcp::protocol::McpResourceListResult,
    ) -> Vec<String> {
        let truncated = result.truncated();
        let cache = self.resource_cache.entry(server.to_string()).or_default();
        cache.uris = result.resources.iter().map(|r| r.uri.clone()).collect();
        cache.truncated = truncated;
        if truncated {
            vec![format!(
                "MCP server '{server}': resources/list page truncated (nextCursor present; not followed in v1)"
            )]
        } else {
            Vec::new()
        }
    }

    /// §4.2/§4.3: list one server's resources. Composes the three split
    /// phases — gate+clone, wire round trip, cache refresh. Callers holding
    /// the registry mutex must use the split phases directly so the lock is
    /// never held across the round trip (mcp_session_tools does exactly
    /// that; the stall test in mcp_tests proves it).
    pub fn list_resources(&mut self, server: &str) -> Result<ResourceListing, ResourceError> {
        let client = self.resource_client_for(server)?;
        let result = client.list_resources().map_err(resource_error_of_mcp)?;
        let notices = self.cache_resource_listing(server, &result);
        Ok(ResourceListing {
            server: server.to_string(),
            truncated: result.truncated(),
            resources: result.resources,
            notices,
        })
    }

    /// §4.3: read a resource by URI. The fail-closed advertised-URI check
    /// runs BEFORE any wire call; blob/non-text content is typed-refused by
    /// the client and mapped through. Same lock discipline as
    /// [`Self::list_resources`]: no registry lock is needed across the round
    /// trip — gate and URI check are `&self`, the cache is untouched.
    pub fn read_resource(
        &mut self,
        server: &str,
        uri: &str,
    ) -> Result<ResourceText, ResourceError> {
        let client = self.resource_client_for(server)?;
        self.validate_advertised_uri(server, uri)?;
        let result = client.read_resource(uri).map_err(resource_error_of_mcp)?;
        let mut text = String::new();
        let mut mime_type = None;
        for content in &result.contents {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&content.text);
            if mime_type.is_none() {
                mime_type = content.mime_type.clone();
            }
        }
        Ok(ResourceText {
            server: server.to_string(),
            uri: uri.to_string(),
            mime_type,
            text,
        })
    }

    /// The §8 advertisement guard: true when ANY registered server
    /// negotiated the resources capability.
    pub fn has_resources_capability(&self) -> bool {
        self.servers
            .iter()
            .any(|entry| entry.client.negotiated().is_some_and(|n| n.resources))
    }

    #[cfg(test)]
    fn resolve(&self, namespaced: &str) -> Option<(String, String)> {
        let (server, tool) = namespaced.strip_prefix("mcp__")?.split_once("__")?;
        self.servers
            .iter()
            .find(|entry| entry.spec.name == server)
            .map(|_| (server.to_string(), tool.to_string()))
    }
}

/// §3.3 batch construction: group the served hits by server in hit order,
/// capped at MAX_HYDRATION_ENTRIES servers (returns capped=true when a
/// match spanned more). Pure — digest lookup injected by the caller.
fn build_hydration_batch(
    hits: &[ToolSearchHit],
    digest_of: impl Fn(&str) -> Option<String>,
) -> (Vec<HydrationEntry>, bool) {
    let mut server_order: Vec<String> = Vec::new();
    let mut names_by_server: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for hit in hits {
        names_by_server
            .entry(hit.server_id.clone())
            .or_insert_with(|| {
                server_order.push(hit.server_id.clone());
                Vec::new()
            })
            .push(hit.tool.clone());
    }
    let mut hydration: Vec<HydrationEntry> = Vec::new();
    let mut capped = false;
    for server_id in &server_order {
        if hydration.len() >= nano_session::MAX_HYDRATION_ENTRIES {
            capped = true;
            break;
        }
        let Some(digest) = digest_of(server_id) else {
            continue;
        };
        hydration.push(HydrationEntry {
            server_id: server_id.clone(),
            tool_names: names_by_server.get(server_id).cloned().unwrap_or_default(),
            tools_digest: digest,
        });
    }
    (hydration, capped)
}

/// §3.5 listing renderer: one line per server, bounded to
/// SOURCE_LISTING_MAX_BYTES total including the truncation trailer.
fn render_source_listing(rows: &[(String, usize, Vec<String>)]) -> String {
    const TRAILER: &str = "…(source listing truncated)\n";
    let budget = SOURCE_LISTING_MAX_BYTES - TRAILER.len();
    let mut out = String::new();
    let mut truncated = false;
    for (name, count, names) in rows {
        let line = format!("{name}: {count} deferred tools ({})\n", names.join(", "));
        if out.len() + line.len() > budget {
            truncated = true;
            break;
        }
        out.push_str(&line);
    }
    if truncated {
        out.push_str(TRAILER);
    }
    out
}

fn namespaced_definition(server: &str, tool: &McpToolDescriptor) -> ToolDefinition {
    ToolDefinition {
        name: format!("mcp__{server}__{}", tool.name),
        description: format!(
            "[MCP {server}] {}",
            tool.description.clone().unwrap_or_default()
        ),
        input_schema: tool
            .input_schema
            .clone()
            .unwrap_or_else(|| serde_json::json!({"type": "object"})),
    }
}

/// ToolExecutor that routes `mcp__` calls to the registry and defers
/// everything else to the wrapped executor.
#[derive(Debug)]
pub struct McpToolExecutor<'a> {
    registry: std::sync::Arc<std::sync::Mutex<McpRegistry>>,
    inner: &'a dyn ToolExecutor,
}

impl<'a> McpToolExecutor<'a> {
    pub fn new(registry: McpRegistry, inner: &'a dyn ToolExecutor) -> Self {
        Self {
            registry: std::sync::Arc::new(std::sync::Mutex::new(registry)),
            inner,
        }
    }

    /// Shares an existing registry (e.g. the per-session registry the ACP
    /// host builds at session/new) instead of taking ownership of a fresh
    /// one, so every turn of the session routes through the same servers.
    pub fn from_shared(
        registry: std::sync::Arc<std::sync::Mutex<McpRegistry>>,
        inner: &'a dyn ToolExecutor,
    ) -> Self {
        Self { registry, inner }
    }

    /// The namespaced MCP tool definitions from the registry.
    pub fn tool_definitions_from_registry(&self) -> Vec<ToolDefinition> {
        self.registry.lock().unwrap().tool_definitions()
    }
}

#[async_trait::async_trait]
impl ToolExecutor for McpToolExecutor<'_> {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        if !call.name.starts_with("mcp__") {
            return self.inner.execute(call).await;
        }
        // Lock to clone, never to wait (§2.1.5/§9.2): the registry lock is
        // held only to resolve the server handle and clone the client; the
        // wire wait happens AFTER the lock is dropped.
        let (client, tool, interrupted_call) = {
            let registry = self.registry.lock().unwrap();
            let Some(entry) = registry
                .servers
                .iter()
                .find(|e| call.name.starts_with(&format!("mcp__{}__", e.spec.name)))
            else {
                return ToolOutcome {
                    ok: false,
                    output: format!("unknown MCP tool: {}", call.name),
                    progress: ProgressSignals::default(),
                    error_kind: Some(NanoErrorKind::UnknownTool),
                };
            };
            // §3.4 stale-call invalidation, FIRST arm after name
            // resolution: a tool not in direct ∪ hydrated (never hydrated,
            // dropped at resume, or on a pinned server) is not callable.
            if !registry.is_exposed(&call.name) {
                return ToolOutcome {
                    ok: false,
                    output: format!(
                        "MCP tool '{}' is not currently exposed (deferred, hydration dropped, or pinned); call tool_search first to load it",
                        call.name
                    ),
                    progress: ProgressSignals::default(),
                    error_kind: Some(NanoErrorKind::UnknownTool),
                };
            }
            let tool = call
                .name
                .strip_prefix(&format!("mcp__{}__", entry.spec.name))
                .unwrap_or(&call.name)
                .to_string();
            (entry.client.clone(), tool, entry.interrupted_call.clone())
        };
        // Elicitation interrupted-call attribution (§5.6): the cell names
        // the in-flight call for the journal record; cleared after.
        *interrupted_call.lock().unwrap() = Some(call.id.clone());
        let result = client.call_tool(&tool, call.arguments.clone());
        *interrupted_call.lock().unwrap() = None;
        match result {
            Ok(result) => ToolOutcome {
                ok: !result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                output: serde_json::to_string_pretty(&result).unwrap_or_default(),
                progress: ProgressSignals {
                    new_information: true,
                    process_outcome_changed: true,
                    ..Default::default()
                },
                // An isError payload is the server reporting a tool-level
                // failure — typed as mcp_server (design §3).
                error_kind: result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    .then_some(NanoErrorKind::McpServer),
            },
            Err(err) => ToolOutcome {
                ok: false,
                output: err.to_string(),
                progress: ProgressSignals::default(),
                error_kind: Some(crate::error_map::kind_of_mcp(&err)),
            },
        }
    }

    /// P1: thread the turn's cancel flag through to the inner executor
    /// (web_search's in-flight cancellation); the mcp__ arm dispatches
    /// through the full-duplex client (call-level cancel lives in
    /// `McpClient::call_tool_cancellable`, wired by the turn loop lane).
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> ToolOutcome {
        if call.name.starts_with("mcp__") {
            self.execute(call).await
        } else {
            self.inner.execute_cancellable(call, cancel).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespacing_shapes() {
        let descriptor = McpToolDescriptor {
            name: "echo".into(),
            description: Some("echo back".into()),
            input_schema: None,
        };
        let def = namespaced_definition("fs", &descriptor);
        assert_eq!(def.name, "mcp__fs__echo");
        assert!(def.description.contains("[MCP fs]"));
    }

    #[test]
    fn resolve_rejects_unknown_namespaces() {
        let registry = McpRegistry::new();
        assert!(registry.resolve("mcp__missing__tool").is_none());
        assert!(registry.resolve("not-mcp").is_none());
        assert!(registry.resolve("mcp__no_tool_sep").is_none());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::loop_protection::ProgressSignals;
    use crate::turn::ToolExecutor;

    #[tokio::test]
    async fn executor_routes_namespaced_call_to_server() {
        // Full round-trip through a real stdio fake server (powershell on
        // Windows, sh on unix — same JSON-RPC line protocol both ways).
        #[cfg(windows)]
        let (command, args) = {
            let script = r#"
$reader = [System.Console]::In
while ($true) {
    $line = $reader.ReadLine()
    if ($null -eq $line) { break }
    $obj = $line | ConvertFrom-Json
    if ($obj.method -eq "initialize") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"protocolVersion`":`"2025-03-26`",`"capabilities`":{},`"serverInfo`":{`"name`":`"fake`",`"version`":`"0`"}}}")
    } elseif ($obj.method -eq "tools/list") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"tools`":[{`"name`":`"echo`",`"description`":`"echoes`"}]}}")
    } elseif ($obj.method -eq "tools/call") {
        Write-Output ("{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{`"content`":`"pong`",`"isError`":false}}")
    }
}
"#;
            (
                "powershell.exe".to_string(),
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    script.to_string(),
                ],
            )
        };
        #[cfg(unix)]
        let (command, args) = {
            let script = r#"
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *'"initialize"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake","version":"0"}}}\n' "$id" ;;
        *'"tools/list"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echoes"}]}}\n' "$id" ;;
        *'"tools/call"'*)
            printf '{"jsonrpc":"2.0","id":%s,"result":{"content":"pong","isError":false}}\n' "$id" ;;
    esac
done
"#;
            ("sh".to_string(), vec!["-c".to_string(), script.to_string()])
        };
        let mut registry = McpRegistry::new();
        let registered = registry.register(McpServerSpec {
            name: "fake".into(),
            command,
            args,
            env: vec![],
        });
        assert_eq!(registered.expect("register"), 1);
        assert_eq!(registry.tool_definitions().len(), 1);
        assert_eq!(registry.tool_definitions()[0].name, "mcp__fake__echo");

        #[derive(Debug)]
        struct Noop;
        #[async_trait::async_trait]
        impl ToolExecutor for Noop {
            async fn execute(&self, _call: &ToolCall) -> crate::turn::ToolOutcome {
                crate::turn::ToolOutcome {
                    ok: false,
                    output: "should not route here".into(),
                    progress: ProgressSignals::default(),
                    error_kind: None,
                }
            }
        }
        let executor = McpToolExecutor::new(registry, &Noop);
        let outcome = executor
            .execute(&ToolCall {
                id: "c1".into(),
                name: "mcp__fake__echo".into(),
                arguments: serde_json::json!({"text": "ping"}),
            })
            .await;
        assert!(outcome.ok, "mcp call must succeed: {}", outcome.output);
        assert!(outcome.output.contains("pong"));
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod mcp_tests;
