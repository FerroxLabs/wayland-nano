//! Elicitation bridge (P3 design §5 — LOCKED): the `elicitation/create`
//! server-request handler installed on the dispatcher's [`ServerRequestHandler`]
//! seam.
//!
//! Method note (§5.1): codex's observed legacy custom-method spelling and the
//! 2025-06-18 spec method are the SAME wire string, `elicitation/create` — one
//! handler covers both. Everything else returns `None` and falls through to
//! the dispatcher's spec-legal `-32601`.
//!
//! Transport scoping (§5.5): the bridge is stdio-only BY CONSTRUCTION — the
//! elicitation client capability is advertised only on stdio connections
//! (`advertises_elicitation` gates `initialize_params_with_elicitation`, and
//! the stdio facade is the only dispatcher consumer), and nano-cli refuses
//! HTTP + `requires:["elicitation"]` at config time. There is deliberately
//! nothing to enforce at runtime here.
//!
//! Invariants honored (§2.4/§5.2/§5.4/§5.6):
//! - an elicitation is answered only against the DESIGNATED foreground call
//!   (`accept_elicitation`); never a card opened against a guessed parent;
//! - per-designated-call cap 8, per-connection budget 64 (dispatcher-owned);
//! - answers resolve ONLY through the one-shot [`ElicitationBinding`]'s opaque
//!   96-bit option ids — never by label text, never a guessed value;
//! - §2.7 (F-P3-3): the journaled `server_id` is the server's stable
//!   INSTANCE ID (`srv_<16 hex>`), minted by the registry at registration —
//!   never the display name. The display name rides separately and is used
//!   ONLY as the ask-card label ("MCP server '<name>' asks:");
//! - JOURNAL-FIRST at answer time: the `Op::McpElicitation` decision (digests
//!   only — answer CONTENT is never journaled) lands via the session's
//!   `JournalCoordinator` BEFORE the wire reply is computed; an append failure
//!   is fail-closed `JournalUnavailable` ⇒ the wire gets `{"action":"cancel"}`
//!   and an unjournaled answer never leaves the process;
//! - no lock is held across the ask wait beyond this bridge's own state mutex
//!   for short mutations; the registry mutex is never touched here.

use nano_mcp::dispatcher::{ConnectionHandle, ServerRequest, ServerRequestHandler, SlotRetired};
use nano_session::{JournalCoordinator, McpElicitationAction, Op, OpEnvelope};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The one wire string for both the spec method and codex's legacy spelling
/// (§5.1).
pub const ELICITATION_CREATE_METHOD: &str = "elicitation/create";

/// `-32602` invalid params (protocol.rs exports only the dispatcher-emitted
/// codes; the bridge emits this one itself for malformed elicitation params).
const INVALID_PARAMS: i64 = -32602;

/// §3.6-style bound on server-authored text crossing into the UI: 1024 chars,
/// control characters stripped.
pub const MAX_MESSAGE_CHARS: usize = 1024;

/// Per-designated-call elicitation cap (§5.2 [r2 codex-F3]).
pub const PER_CALL_ELICITATION_CAP: usize = 8;

/// The host-side ask ceiling (§5.4 — the C10 300s default); a trip maps to
/// `Timeout` ⇒ wire `cancel`. Shortened in tests.
pub const DEFAULT_ASK_TIMEOUT: Duration = Duration::from_secs(300);

/// Ask-wait poll slice: the slot-retirement cancel flag is observed within
/// one slice (≤100ms, §2.4 rule 4 / §12 retirement leg).
const ASK_POLL: Duration = Duration::from_millis(50);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Strips control characters and truncates to [`MAX_MESSAGE_CHARS`] chars.
/// Server-authored text never reaches the ask card raw.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

// ---------------------------------------------------------------------------
// §5.1 params parsing
// ---------------------------------------------------------------------------

/// Parsed `elicitation/create` params. `requested_schema == None` is the
/// url-mode shape (§5.3: typed-declined, never opened anywhere).
#[derive(Debug, Clone, PartialEq)]
pub struct ElicitRequestParams {
    pub message: String,
    pub requested_schema: Option<Value>,
}

/// Malformed params are a spec-legal `-32602` error reply, never a hang.
fn parse_params(request: &ServerRequest) -> Result<ElicitRequestParams, (i64, String)> {
    let params = request.params.as_ref().ok_or_else(|| {
        (
            INVALID_PARAMS,
            "elicitation/create requires params".to_string(),
        )
    })?;
    let obj = params.as_object().ok_or_else(|| {
        (
            INVALID_PARAMS,
            "elicitation/create params must be an object".to_string(),
        )
    })?;
    let message = obj.get("message").and_then(|m| m.as_str()).ok_or_else(|| {
        (
            INVALID_PARAMS,
            "elicitation/create params.message must be a string".to_string(),
        )
    })?;
    let requested_schema = match obj.get("requestedSchema") {
        None => None,
        Some(v) if v.is_object() => Some(v.clone()),
        Some(_) => {
            return Err((
                INVALID_PARAMS,
                "elicitation/create params.requestedSchema must be an object".to_string(),
            ));
        }
    };
    Ok(ElicitRequestParams {
        message: sanitize(message),
        requested_schema,
    })
}

/// Method routing: ONLY `elicitation/create` is served (§5.1/§2.5).
fn routes(method: &str) -> bool {
    method == ELICITATION_CREATE_METHOD
}

// ---------------------------------------------------------------------------
// §5.3 schema → options reduction (enum 2..=4 / boolean only)
// ---------------------------------------------------------------------------

/// One reducible schema field: an enum of 2..=4 string options, or a boolean
/// rendered as two options.
#[derive(Debug, Clone, PartialEq)]
pub struct ElicitFieldSpec {
    pub name: String,
    /// Sanitized title (schema `title`, else the field name).
    pub title: String,
    /// Sanitized enum labels / boolean true-false labels (2..=4 entries).
    pub options: Vec<String>,
    pub is_boolean: bool,
}

/// §5.3: the whole elicitation reduces to askable fields, or the WHOLE
/// elicitation declines — never a partial answer.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaReduction {
    Fields(Vec<ElicitFieldSpec>),
    Decline,
}

fn sanitized_label(raw: Option<&Value>) -> Option<String> {
    let label = sanitize(raw?.as_str()?);
    (!label.is_empty()).then_some(label)
}

fn reduce_field(name: &str, prop: &Value) -> Option<ElicitFieldSpec> {
    let title = sanitized_label(prop.get("title")).unwrap_or_else(|| sanitize(name));
    if let Some(variants) = prop.get("enum").and_then(|e| e.as_array()) {
        // 2..=4 string options, matching validate_question_args's bound.
        if !(2..=4).contains(&variants.len()) {
            return None;
        }
        let mut options = Vec::with_capacity(variants.len());
        for variant in variants {
            options.push(sanitize(variant.as_str()?));
        }
        return Some(ElicitFieldSpec {
            name: name.to_string(),
            title,
            options,
            is_boolean: false,
        });
    }
    if prop.get("type").and_then(|t| t.as_str()) == Some("boolean") {
        // §5.3: the schema's title/description as the true/false label text.
        let yes = sanitized_label(prop.get("title")).unwrap_or_else(|| "True".to_string());
        let no = sanitized_label(prop.get("description")).unwrap_or_else(|| "False".to_string());
        return Some(ElicitFieldSpec {
            name: name.to_string(),
            title,
            options: vec![yes, no],
            is_boolean: true,
        });
    }
    // Freeform string/number, nested object, anything else: unsupported.
    None
}

/// Reduces `requestedSchema` to askable fields. Multi-field schemas are
/// accepted ONLY when every field is enum/boolean; any unsupported field
/// declines the whole elicitation. Field order is serde_json's canonical
/// (sorted) key order — the same order content is assembled in.
pub fn reduce_schema(schema: &Value) -> SchemaReduction {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return SchemaReduction::Decline;
    };
    if props.is_empty() {
        return SchemaReduction::Decline;
    }
    let mut fields = Vec::with_capacity(props.len());
    for (name, prop) in props {
        let Some(field) = reduce_field(name, prop) else {
            return SchemaReduction::Decline;
        };
        fields.push(field);
    }
    SchemaReduction::Fields(fields)
}

// ---------------------------------------------------------------------------
// Canonical digests (§5.6): sha256 over canonical JSON (object keys sorted
// recursively, no insignificant whitespace), lowercase 64-hex.
// ---------------------------------------------------------------------------

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).expect("a string always serializes"));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("a string always serializes"));
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn sha256_hex(data: &str) -> String {
    let digest = Sha256::digest(data.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Canonical digest of the `requestedSchema` (§5.6 `schema_digest`).
pub fn canonical_schema_digest(schema: &Value) -> String {
    sha256_hex(&canonical_json(schema))
}

/// Canonical digest of the assembled answer content (§5.6 `answer_digest`).
/// Anyone holding the answer can prove it is what was answered.
pub fn canonical_answer_digest(content: &Value) -> String {
    sha256_hex(&canonical_json(content))
}

// ---------------------------------------------------------------------------
// Opaque option ids [r2 codex-F9]: 96 bits of random entropy rendered as 24
// lowercase hex chars — NOT sequential `opt_{i}`, never label-adjacent.
// ---------------------------------------------------------------------------

fn mint_option_id() -> String {
    let bytes: [u8; 12] = rand::random();
    let mut id = String::with_capacity(24);
    for byte in bytes {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

// ---------------------------------------------------------------------------
// §5.2 one-shot binding record
// ---------------------------------------------------------------------------

/// The record that ties server instance + request id + interrupted call +
/// ask card + schema digest to the returned answer. Consumed EXACTLY ONCE:
/// answers resolve only through `id_map`'s opaque ids, never by echoing the
/// label, so duplicate or adversarial labels cannot produce an ambiguous or
/// server-injected answer.
#[derive(Debug)]
pub struct ElicitationBinding {
    pub server_id: String,
    /// The server's JSON-RPC request id, stringified.
    pub jsonrpc_request_id: String,
    /// The interrupted ToolCall op's id from the shared cell ("" when absent).
    pub interrupted_call_id: String,
    /// Set when the host's ask card opens (the `Answered` outcome's card id).
    pub card_id: u64,
    pub schema_digest: String,
    /// opaque option id → (field name, schema value).
    id_map: HashMap<String, (String, Value)>,
    consumed: bool,
}

impl ElicitationBinding {
    /// Resolves an opaque option id to its (field, schema value). `None` when
    /// the id is not in the map (typed internal decline — never a guessed
    /// value) or the binding is already consumed.
    pub fn resolve(&self, option_id: &str) -> Option<&(String, Value)> {
        if self.consumed {
            return None;
        }
        self.id_map.get(option_id)
    }

    /// One-shot consumption: after this, no resolution ever succeeds again.
    pub fn consume(&mut self) {
        self.consumed = true;
    }

    pub fn is_consumed(&self) -> bool {
        self.consumed
    }
}

// ---------------------------------------------------------------------------
// Host ask seam (the nano-cli lane owns the card; the bridge is UI-agnostic)
// ---------------------------------------------------------------------------

/// One question handed to the host, which turns it into an ask card carrying
/// the opaque option ids VERBATIM.
#[derive(Debug, Clone, PartialEq)]
pub struct ElicitQuestion {
    /// Server-originated marker: "MCP server '<display name>' asks:" (the
    /// display name is a label only — the journaled key is the §2.7
    /// instance id).
    pub header: String,
    pub message: String,
    /// (opaque id, sanitized label) pairs.
    pub options: Vec<(String, String)>,
}

/// The host maps its UI outcomes onto this (§5.4, pinned).
#[derive(Debug, Clone, PartialEq)]
pub enum ElicitAskOutcome {
    Answered { card_id: u64, option_id: String },
    Dismiss,
    Cancel,
    Timeout,
    Unavailable,
}

/// §5.4 outcome → wire/journal action mapping (pinned in tests):
/// Answered ⇒ accept, Dismiss ⇒ decline, Cancel ⇒ cancel, Timeout ⇒ cancel,
/// Unavailable ⇒ decline.
fn outcome_action(outcome: &ElicitAskOutcome) -> McpElicitationAction {
    match outcome {
        ElicitAskOutcome::Answered { .. } => McpElicitationAction::Accept,
        ElicitAskOutcome::Dismiss | ElicitAskOutcome::Unavailable => McpElicitationAction::Decline,
        ElicitAskOutcome::Cancel | ElicitAskOutcome::Timeout => McpElicitationAction::Cancel,
    }
}

/// The spec-legal `ElicitResult` wire shape: `{"action": ...}` plus
/// `content` on accept only.
fn action_reply(action: McpElicitationAction, content: Option<Value>) -> Value {
    let action_str = match action {
        McpElicitationAction::Accept => "accept",
        McpElicitationAction::Decline => "decline",
        _ => "cancel",
    };
    match (action, content) {
        (McpElicitationAction::Accept, Some(content)) => {
            serde_json::json!({"action": action_str, "content": content})
        }
        // Declines/cancels never carry content, even if a caller slips one in.
        _ => serde_json::json!({"action": action_str}),
    }
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// An ask currently waiting on the host: the retirement hook sets `cancel`
/// (§2.4 rule 4) and the ask wait observes it within one poll slice.
struct OpenAsk {
    call_id: u64,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct BridgeState {
    /// Per-designated-call elicitation count (cap 8); entry removed when the
    /// slot retires (§2.4 rule 4 / §5.2).
    per_call_counts: HashMap<u64, usize>,
    /// The last designated call that has since RETIRED: an elicitation
    /// finding no designated call after this was queued at retirement and
    /// gets exactly ONE spec-legal decline (§2.4 rule 4(a)).
    retired_designated: Option<u64>,
    /// At most one open ask per connection: the dispatcher's handler thread
    /// IS the serialization (§5.2).
    open_ask: Option<OpenAsk>,
}

/// The §5.2 elicitation bridge: one per server connection, installed as the
/// dispatcher's [`ServerRequestHandler`].
pub struct ElicitationBridge {
    /// The §2.7 instance id — the journaled key (Op::McpElicitation and the
    /// binding record), NEVER the display name.
    server_id: String,
    /// Display name: the ask-card label only; keys nothing.
    display_name: String,
    session_id: String,
    journal: Arc<JournalCoordinator>,
    /// The interrupted ToolCall op's id, written by the host lane when a
    /// tools/call starts ("" journaled — with a log — when absent).
    interrupted_call: Arc<Mutex<Option<String>>>,
    ask: Arc<dyn Fn(ElicitQuestion) -> ElicitAskOutcome + Send + Sync>,
    ask_timeout: Duration,
    /// Monotonic per-session counter; the journal idempotence key is
    /// "{session_id}-elicit-{counter}". Restored from the durable journal at
    /// construction (F-P3-4): the bridge is rebuilt fresh on `session/load`,
    /// and a counter restarting at 0 re-mints already-durable ids — the
    /// idempotent append then no-ops and the answer would leave unjournaled.
    counter: AtomicU64,
    state: Mutex<BridgeState>,
}

impl std::fmt::Debug for ElicitationBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElicitationBridge")
            .field("server_id", &self.server_id)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// F-P3-4: restore the elicitation counter from the durable journal at
/// bridge construction. The bridge is rebuilt fresh on every
/// `session/load`; the idempotence key "{session_id}-elicit-{counter}" is
/// session-scoped, so the counter must resume at the max minted suffix —
/// otherwise the first post-resume decisions re-mint already-durable ids,
/// the append becomes an idempotent no-op (`Ok(false)`), and the answer
/// would reach the wire UNJOURNALED (§5.6 journal-first defeated; the
/// leg5b live proof). Audit-only ops survive compaction in the append-only
/// file, so the raw envelope stream is the exact restore source.
///
/// A restore failure is NOT fatal here: `decide` re-mints on collision and
/// fails closed (never wiring an unjournaled answer) if the restored value
/// undercounts.
fn restored_counter(journal: &JournalCoordinator, session_id: &str) -> u64 {
    let prefix = format!("{session_id}-elicit-");
    match nano_session::read_journal(journal.path()) {
        Ok(report) => report
            .envelopes
            .iter()
            .filter_map(|env| match &env.op {
                Op::McpElicitation { elicitation_id, .. } => elicitation_id
                    .strip_prefix(&prefix)
                    .and_then(|suffix| suffix.parse::<u64>().ok()),
                _ => None,
            })
            .max()
            .unwrap_or(0),
        Err(err) => {
            eprintln!(
                "wayland-nano mcp elicitation: counter restore failed ({err}); id collisions re-mint/fail closed at append time"
            );
            0
        }
    }
}

impl ElicitationBridge {
    pub fn new(
        server_id: String,
        display_name: String,
        session_id: String,
        journal: Arc<JournalCoordinator>,
        interrupted_call: Arc<Mutex<Option<String>>>,
        ask: Arc<dyn Fn(ElicitQuestion) -> ElicitAskOutcome + Send + Sync>,
    ) -> Self {
        let counter = restored_counter(&journal, &session_id);
        Self {
            server_id,
            display_name,
            session_id,
            journal,
            interrupted_call,
            ask,
            ask_timeout: DEFAULT_ASK_TIMEOUT,
            counter: AtomicU64::new(counter),
            state: Mutex::new(BridgeState::default()),
        }
    }

    /// Test-only override of the 300s ask ceiling (§12's shortened clock).
    #[doc(hidden)]
    pub fn with_ask_timeout(mut self, timeout: Duration) -> Self {
        self.ask_timeout = timeout;
        self
    }

    /// §2.4 rule 4 hook: retirement of the designated call. On
    /// `was_answering` the open ask wait is signaled to end NOW so `handle`
    /// maps it to exactly ONE wire `{"action":"cancel"}` with the binding
    /// consumed once — no post-retirement extension, no second reply. The
    /// per-call cap entry is cleared either way. Retirement racing an
    /// acceptance serializes on the bridge state mutex (accept-then-retire or
    /// retire-then-reject, never both).
    pub fn slot_retired_hook(self: &Arc<Self>) -> Arc<dyn Fn(SlotRetired) + Send + Sync> {
        let bridge = self.clone();
        Arc::new(move |retired: SlotRetired| {
            let mut state = lock(&bridge.state);
            state.per_call_counts.remove(&retired.call_id);
            state.retired_designated = Some(retired.call_id);
            if retired.was_answering
                && let Some(open) = &state.open_ask
                && open.call_id == retired.call_id
            {
                open.cancel.store(true, Ordering::SeqCst);
            }
        })
    }

    fn interrupted_call_id(&self) -> String {
        let id = lock(&self.interrupted_call).clone().unwrap_or_default();
        if id.is_empty() {
            eprintln!(
                "wayland-nano mcp elicitation: no interrupted tool call id recorded; journaling \"\""
            );
        }
        id
    }

    /// JOURNAL-FIRST (§5.6): the bound decision (digests only — answer
    /// content is never journaled) must land BEFORE the reply leaves
    /// `handle`. Append failure is the typed `JournalUnavailable` condition:
    /// fail closed with a spec-legal `{"action":"cancel"}` — an unjournaled
    /// answer must never reach the wire. An idempotent no-op append
    /// (`Ok(false)` — the minted id is already durable) is NOT success for
    /// THIS decision (F-P3-4): the id is re-minted a bounded number of
    /// times, and exhaustion is the same fail-closed cancel.
    fn decide(
        &self,
        request: &ServerRequest,
        action: McpElicitationAction,
        card_id: u64,
        schema_digest: &str,
        answer_digest: &str,
        content: Option<Value>,
    ) -> Result<Value, (i64, String)> {
        const MAX_REMINTS: u32 = 8;
        let mut remints = 0u32;
        let journaled = loop {
            let elicitation_id = format!(
                "{}-elicit-{}",
                self.session_id,
                self.counter.fetch_add(1, Ordering::SeqCst) + 1
            );
            let envelope = OpEnvelope::new(
                elicitation_id.clone(),
                "now",
                Op::McpElicitation {
                    elicitation_id,
                    server_id: self.server_id.clone(),
                    call_id: self.interrupted_call_id(),
                    request_id: request.id.to_string(),
                    card_id,
                    action,
                    schema_digest: schema_digest.to_string(),
                    answer_digest: answer_digest.to_string(),
                },
            );
            let Op::McpElicitation {
                elicitation_id: ref op_id,
                ref server_id,
                ref call_id,
                ref request_id,
                ..
            } = envelope.op
            else {
                unreachable!("decide builds only McpElicitation")
            };
            // LOW-9/§5.6: an out-of-bounds payload (e.g. a hostile oversized
            // server request id) is the same fail-closed cancel as an append
            // failure — never journal it, never answer unjournaled.
            let valid = nano_session::validate_elicitation(
                op_id,
                server_id,
                call_id,
                request_id,
                schema_digest,
                answer_digest,
            );
            if let Err(rule) = valid {
                eprintln!(
                    "wayland-nano mcp elicitation: decision payload out of bounds ({rule}); fail-closed cancel (decision NOT journaled)"
                );
                return Ok(action_reply(McpElicitationAction::Cancel, None));
            }
            match self.journal.append(&envelope) {
                Ok(true) => break true,
                Ok(false) => {
                    // F-P3-4: the id is already durable — the restored
                    // counter undercounted (or a concurrent bridge minted
                    // it). This decision was NOT journaled; re-mint rather
                    // than wiring an unjournaled answer.
                    remints += 1;
                    if remints > MAX_REMINTS {
                        eprintln!(
                            "wayland-nano mcp elicitation: elicitation id keeps colliding after {MAX_REMINTS} re-mints; fail-closed cancel (decision NOT journaled)"
                        );
                        break false;
                    }
                    eprintln!(
                        "wayland-nano mcp elicitation: elicitation id {op_id} already durable; re-minting (collision {remints}/{MAX_REMINTS})"
                    );
                }
                Err(err) => {
                    eprintln!(
                        "wayland-nano mcp elicitation: JournalUnavailable: {err}; fail-closed cancel (decision NOT journaled)"
                    );
                    break false;
                }
            }
        };
        if journaled {
            Ok(action_reply(action, content))
        } else {
            Ok(action_reply(McpElicitationAction::Cancel, None))
        }
    }

    /// Builds the §5.2 binding plus, per field, the (opaque id, label) option
    /// pairs the ask card carries verbatim. Enum values are their sanitized
    /// labels; boolean options map to JSON `true`/`false`.
    fn build_binding(
        &self,
        request: &ServerRequest,
        schema_digest: String,
        fields: &[ElicitFieldSpec],
    ) -> (ElicitationBinding, Vec<Vec<(String, String)>>) {
        let mut id_map = HashMap::new();
        let mut question_options = Vec::with_capacity(fields.len());
        for field in fields {
            let mut pairs = Vec::with_capacity(field.options.len());
            for (index, label) in field.options.iter().enumerate() {
                let value = if field.is_boolean {
                    Value::Bool(index == 0)
                } else {
                    Value::String(label.clone())
                };
                let id = mint_option_id();
                id_map.insert(id.clone(), (field.name.clone(), value));
                pairs.push((id, label.clone()));
            }
            question_options.push(pairs);
        }
        let binding = ElicitationBinding {
            server_id: self.server_id.clone(),
            jsonrpc_request_id: request.id.to_string(),
            interrupted_call_id: self.interrupted_call_id(),
            card_id: 0,
            schema_digest,
            id_map,
            consumed: false,
        };
        (binding, question_options)
    }

    /// Drives ONE ask through the host callback on a worker thread, polling
    /// the retirement cancel flag and the ask deadline in ≤100ms slices. No
    /// lock is held across this wait. A callback still blocked after a
    /// retirement signal leaks one worker thread per raced ask — the host's
    /// own card teardown is what unblocks it; the wire side has already been
    /// answered exactly once by then.
    fn run_ask(&self, question: ElicitQuestion, cancel: &Arc<AtomicBool>) -> ElicitAskOutcome {
        let (tx, rx) = mpsc::channel();
        let ask = self.ask.clone();
        std::thread::spawn(move || {
            let _ = tx.send(ask(question));
        });
        let deadline = Instant::now() + self.ask_timeout;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return ElicitAskOutcome::Cancel;
            }
            let now = Instant::now();
            if now >= deadline {
                return ElicitAskOutcome::Timeout;
            }
            match rx.recv_timeout(ASK_POLL.min(deadline - now)) {
                Ok(outcome) => return outcome,
                Err(RecvTimeoutError::Timeout) => continue,
                // The host callback is gone: fail closed as Unavailable.
                Err(RecvTimeoutError::Disconnected) => return ElicitAskOutcome::Unavailable,
            }
        }
    }

    fn handle_create(
        &self,
        conn: &ConnectionHandle,
        request: &ServerRequest,
    ) -> Result<Value, (i64, String)> {
        let params = parse_params(request)?;
        let schema_digest = params
            .requested_schema
            .as_ref()
            .map(canonical_schema_digest)
            .unwrap_or_default();

        // (a) §2.4 rule 2 attribution: only the DESIGNATED foreground call
        // may parent an elicitation. None with a retired designation means
        // this request was queued at retirement ⇒ exactly ONE spec-legal
        // decline; None with no designation ever ⇒ the typed -32601-style
        // rejection — never a card against a guessed parent.
        let call_id = match conn.accept_elicitation() {
            Some(id) => {
                lock(&self.state).retired_designated = None;
                id
            }
            None => {
                let retired = lock(&self.state).retired_designated.is_some();
                if retired {
                    return self.decide(
                        request,
                        McpElicitationAction::Decline,
                        0,
                        &schema_digest,
                        "",
                        None,
                    );
                }
                return Err((
                    nano_mcp::protocol::METHOD_NOT_FOUND,
                    "elicitation requires an in-flight tool call".to_string(),
                ));
            }
        };

        // (b) §2.4 rule 3: per-connection lifetime budget (64); overflow is
        // an auto-decline on the error lane.
        if !conn.take_elicitation_budget() {
            return Err((
                nano_mcp::protocol::INTERNAL_ERROR,
                "elicitation budget exhausted".to_string(),
            ));
        }

        // (c) §5.2 per-designated-call cap 8; the overflow decline IS a
        // journaled decision.
        {
            let mut state = lock(&self.state);
            let count = state.per_call_counts.entry(call_id).or_insert(0);
            if *count >= PER_CALL_ELICITATION_CAP {
                drop(state);
                return self.decide(
                    request,
                    McpElicitationAction::Decline,
                    0,
                    &schema_digest,
                    "",
                    None,
                );
            }
            *count += 1;
        }

        // (d) §5.3 reduction. Url-mode (no requestedSchema), freeform, >4
        // options, nested objects, or ANY unsupported field in a multi-field
        // schema ⇒ the WHOLE elicitation declines, journaled.
        let fields = match &params.requested_schema {
            None => None,
            Some(schema) => match reduce_schema(schema) {
                SchemaReduction::Fields(fields) => Some(fields),
                SchemaReduction::Decline => None,
            },
        };
        let Some(fields) = fields else {
            return self.decide(
                request,
                McpElicitationAction::Decline,
                0,
                &schema_digest,
                "",
                None,
            );
        };

        // (e) Binding + the one-time deadline extension (§2.4 rule 1), then
        // the ask wait(s) — one question per field, sequentially. The cancel
        // flag is armed under the state mutex if the slot already retired
        // between accept and registration (retire-then-reject).
        let (mut binding, question_options) =
            self.build_binding(request, schema_digest.clone(), &fields);
        conn.note_ask_entered(call_id);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut state = lock(&self.state);
            if state.retired_designated == Some(call_id) {
                cancel.store(true, Ordering::SeqCst);
            }
            state.open_ask = Some(OpenAsk {
                call_id,
                cancel: cancel.clone(),
            });
        }

        let mut content = serde_json::Map::new();
        let mut action: Option<McpElicitationAction> = None;
        let multi = fields.len() > 1;
        for (field, options) in fields.iter().zip(question_options) {
            let message = if multi {
                sanitize(&format!("{} — {}", params.message, field.title))
            } else {
                params.message.clone()
            };
            let question = ElicitQuestion {
                header: format!("MCP server '{}' asks:", self.display_name),
                message,
                options,
            };
            match self.run_ask(question, &cancel) {
                ElicitAskOutcome::Answered { card_id, option_id } => {
                    let resolved = binding
                        .resolve(&option_id)
                        .map(|(field_name, value)| (field_name.clone(), value.clone()));
                    match resolved {
                        Some((field_name, value)) => {
                            // §5.2: the answer is accepted only through the
                            // binding; the card id is recorded for the journal.
                            binding.card_id = card_id;
                            content.insert(field_name, value);
                        }
                        // An id not in the binding ⇒ typed internal decline,
                        // never a guessed value [r2 codex-F9].
                        None => {
                            action = Some(McpElicitationAction::Decline);
                            break;
                        }
                    }
                }
                outcome => {
                    action = Some(outcome_action(&outcome));
                    break;
                }
            }
        }

        // The ask wait is over: release the open-ask slot (ours only).
        {
            let mut state = lock(&self.state);
            if state
                .open_ask
                .as_ref()
                .is_some_and(|open| open.call_id == call_id)
            {
                state.open_ask = None;
            }
        }

        // (f/g) Journal-first, then exactly-once consumption of the binding.
        let reply = match action {
            None => {
                let content = Value::Object(content);
                self.decide(
                    request,
                    McpElicitationAction::Accept,
                    binding.card_id,
                    &binding.schema_digest,
                    &canonical_answer_digest(&content),
                    Some(content),
                )
            }
            Some(action) => self.decide(request, action, 0, &binding.schema_digest, "", None),
        };
        binding.consume();
        reply
    }
}

impl ServerRequestHandler for ElicitationBridge {
    fn handle(
        &self,
        conn: &ConnectionHandle,
        request: &ServerRequest,
    ) -> Option<Result<Value, (i64, String)>> {
        if !routes(&request.method) {
            return None;
        }
        Some(self.handle_create(conn, request))
    }

    /// §2.7 honesty rule: installing this handler means the client
    /// capability IS advertised (stdio only — see the module docs).
    fn advertises_elicitation(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(method: &str, params: Option<Value>) -> ServerRequest {
        ServerRequest {
            id: json!(7),
            method: method.to_string(),
            params,
        }
    }

    fn elicit_params(schema: Value) -> Option<Value> {
        Some(json!({"message": "pick one", "requestedSchema": schema}))
    }

    // --- params parsing + sanitization (§5.1) -------------------------------

    #[test]
    fn parse_params_ok_sanitizes_message() {
        let parsed = parse_params(&request(
            ELICITATION_CREATE_METHOD,
            Some(json!({
                "message": "pi\u{0007}ck one",
                "requestedSchema": {"type": "object", "properties": {}}
            })),
        ))
        .expect("valid params");
        assert_eq!(parsed.message, "pick one");
        assert!(parsed.requested_schema.is_some());
    }

    #[test]
    fn parse_params_truncates_at_1024_chars() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 500);
        let parsed = parse_params(&request(
            ELICITATION_CREATE_METHOD,
            Some(json!({"message": long})),
        ))
        .expect("valid params");
        assert_eq!(parsed.message.chars().count(), MAX_MESSAGE_CHARS);
    }

    #[test]
    fn parse_params_malformed_is_32602_never_a_hang() {
        for params in [
            None,
            Some(json!("not-an-object")),
            Some(json!({"requestedSchema": {}})), // message missing
            Some(json!({"message": 42})),         // message not a string
            Some(json!({"message": "m", "requestedSchema": "nope"})), // schema not object
        ] {
            let err = parse_params(&request(ELICITATION_CREATE_METHOD, params))
                .expect_err("malformed params must fail");
            assert_eq!(err.0, -32602);
            assert!(err.1.len() <= 128, "error message stays bounded");
        }
    }

    #[test]
    fn parse_params_url_mode_has_no_schema() {
        let parsed = parse_params(&request(
            ELICITATION_CREATE_METHOD,
            Some(json!({"message": "open a url", "url": "https://example.test"})),
        ))
        .expect("url-mode params parse");
        assert!(parsed.requested_schema.is_none());
    }

    #[test]
    fn routes_only_elicitation_create() {
        assert!(routes(ELICITATION_CREATE_METHOD));
        for other in [
            "ping",
            "sampling/createMessage",
            "roots/list",
            "elicitation/create ",
        ] {
            assert!(!routes(other), "{other} must fall through to -32601");
        }
    }

    // --- schema reduction battery (§5.3) ------------------------------------

    fn enum_schema(count: usize) -> Value {
        let options: Vec<Value> = (0..count).map(|i| json!(format!("opt{i}"))).collect();
        json!({"type": "object", "properties": {"choice": {"type": "string", "enum": options}}})
    }

    #[test]
    fn reduce_enum_two_to_four_options_accepted() {
        for count in 2..=4 {
            let SchemaReduction::Fields(fields) = reduce_schema(&enum_schema(count)) else {
                panic!("enum of {count} must reduce");
            };
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].name, "choice");
            assert_eq!(fields[0].options.len(), count);
            assert!(!fields[0].is_boolean);
        }
    }

    #[test]
    fn reduce_enum_out_of_bounds_or_non_string_declines() {
        for schema in [
            enum_schema(1),
            enum_schema(5),
            json!({"properties": {"c": {"enum": ["a", 2]}}}),
            json!({"properties": {"c": {"enum": "not-an-array"}}}),
        ] {
            assert_eq!(reduce_schema(&schema), SchemaReduction::Decline);
        }
    }

    #[test]
    fn reduce_boolean_yields_two_options_from_title_description() {
        let schema = json!({"properties": {"confirm": {
            "type": "boolean",
            "title": "Yes, do it",
            "description": "No, skip",
        }}});
        let SchemaReduction::Fields(fields) = reduce_schema(&schema) else {
            panic!("boolean must reduce");
        };
        assert!(fields[0].is_boolean);
        assert_eq!(fields[0].options, vec!["Yes, do it", "No, skip"]);

        // Without title/description the labels default to True/False.
        let bare = json!({"properties": {"confirm": {"type": "boolean"}}});
        let SchemaReduction::Fields(fields) = reduce_schema(&bare) else {
            panic!("bare boolean must reduce");
        };
        assert_eq!(fields[0].options, vec!["True", "False"]);
    }

    #[test]
    fn reduce_freeform_number_and_nested_decline() {
        for prop in [
            json!({"type": "string"}),
            json!({"type": "number"}),
            json!({"type": "object", "properties": {"nested": {"enum": ["a", "b"]}}}),
            json!({"type": "array", "items": {"type": "string"}}),
        ] {
            let schema = json!({"type": "object", "properties": {"field": prop}});
            assert_eq!(reduce_schema(&schema), SchemaReduction::Decline);
        }
    }

    #[test]
    fn reduce_multi_field_all_enum_or_boolean_accepted_in_canonical_order() {
        let schema = json!({"type": "object", "properties": {
            "zulu": {"type": "boolean"},
            "alpha": {"type": "string", "enum": ["a", "b"]},
        }});
        let SchemaReduction::Fields(fields) = reduce_schema(&schema) else {
            panic!("all-supported multi-field must reduce");
        };
        // serde_json's canonical (sorted) key order: alpha before zulu.
        assert_eq!(fields[0].name, "alpha");
        assert!(!fields[0].is_boolean);
        assert_eq!(fields[1].name, "zulu");
        assert!(fields[1].is_boolean);
    }

    #[test]
    fn reduce_multi_field_with_any_unsupported_field_declines_wholesale() {
        let schema = json!({"type": "object", "properties": {
            "good": {"type": "string", "enum": ["a", "b"]},
            "bad": {"type": "string"},
        }});
        assert_eq!(reduce_schema(&schema), SchemaReduction::Decline);
    }

    #[test]
    fn reduce_missing_or_empty_properties_declines() {
        for schema in [
            json!({}),
            json!({"type": "object"}),
            json!({"type": "object", "properties": {}}),
            json!("not-an-object"),
        ] {
            assert_eq!(reduce_schema(&schema), SchemaReduction::Decline);
        }
    }

    // --- canonical digests (§5.6) ---------------------------------------------

    #[test]
    fn digests_are_canonical_and_deterministic() {
        let a = json!({"b": [true, "x"], "a": 1});
        let b = json!({ "a" : 1, "b" : [ true, "x" ] }); // key order + whitespace
        assert_eq!(canonical_schema_digest(&a), canonical_schema_digest(&b));
        assert_eq!(canonical_answer_digest(&a), canonical_answer_digest(&b));
        // Fixed vector: sha256 of the canonical rendering {"a":1,"b":[true,"x"]}.
        assert_eq!(
            canonical_schema_digest(&a),
            "63e8063d9dc6f0fd5a24b4706818a165fd57c3531b74466cf5dea62bff09b0b6"
        );
        let digest = canonical_answer_digest(&json!({"color": "blue"}));
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(
            canonical_schema_digest(&a),
            canonical_schema_digest(&json!({"a": 2}))
        );
    }

    // --- opaque option ids [r2 codex-F9] ---------------------------------------

    #[test]
    fn minted_option_ids_are_24_hex_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let id = mint_option_id();
            assert_eq!(id.len(), 24);
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            );
            assert!(seen.insert(id), "ids must be unique across 1000 mints");
        }
    }

    // --- binding (§5.2 one-shot) ------------------------------------------------

    fn test_binding() -> ElicitationBinding {
        let mut id_map = HashMap::new();
        id_map.insert("a".repeat(24), ("color".to_string(), json!("blue")));
        ElicitationBinding {
            server_id: "fake".into(),
            jsonrpc_request_id: "7".into(),
            interrupted_call_id: "call-1".into(),
            card_id: 0,
            schema_digest: "d".repeat(64),
            id_map,
            consumed: false,
        }
    }

    #[test]
    fn binding_resolves_once_then_never_again() {
        let mut binding = test_binding();
        let known = "a".repeat(24);
        let (field, value) = binding.resolve(&known).expect("known id resolves");
        assert_eq!(field, "color");
        assert_eq!(value, &json!("blue"));
        // Unknown id: typed internal decline path — never a guessed value.
        assert!(binding.resolve(&"b".repeat(24)).is_none());
        assert!(binding.resolve("blue").is_none(), "never resolves by label");
        assert!(!binding.is_consumed());
        binding.consume();
        assert!(binding.is_consumed());
        assert!(
            binding.resolve(&known).is_none(),
            "consumed binding is dead"
        );
    }

    #[test]
    fn build_binding_mints_opaque_ids_with_schema_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal =
            Arc::new(JournalCoordinator::open(dir.path().join("j.jsonl")).expect("journal"));
        let bridge = ElicitationBridge::new(
            "fake".into(),
            "fake".into(),
            "sess".into(),
            journal,
            Arc::new(Mutex::new(Some("call-1".to_string()))),
            Arc::new(|_| ElicitAskOutcome::Unavailable),
        );
        let fields = vec![
            ElicitFieldSpec {
                name: "color".into(),
                title: "Color".into(),
                options: vec!["red".into(), "blue".into()],
                is_boolean: false,
            },
            ElicitFieldSpec {
                name: "confirm".into(),
                title: "Confirm".into(),
                options: vec!["Yes".into(), "No".into()],
                is_boolean: true,
            },
        ];
        let req = request(ELICITATION_CREATE_METHOD, elicit_params(json!({})));
        let (binding, question_options) = bridge.build_binding(&req, "s".repeat(64), &fields);
        assert_eq!(binding.server_id, "fake");
        assert_eq!(binding.jsonrpc_request_id, "7");
        assert_eq!(binding.interrupted_call_id, "call-1");
        assert_eq!(question_options.len(), 2);
        // Enum values are the labels; boolean options map to true/false.
        let (enum_field, enum_value) = binding.resolve(&question_options[0][1].0).expect("blue");
        assert_eq!((enum_field.as_str(), enum_value), ("color", &json!("blue")));
        let (bool_field, bool_value) = binding.resolve(&question_options[1][1].0).expect("no");
        assert_eq!(
            (bool_field.as_str(), bool_value),
            ("confirm", &json!(false))
        );
        let (bool_field, bool_value) = binding.resolve(&question_options[1][0].0).expect("yes");
        assert_eq!((bool_field.as_str(), bool_value), ("confirm", &json!(true)));
        // All four ids distinct 24-hex.
        let ids: std::collections::HashSet<&String> = question_options
            .iter()
            .flat_map(|pairs| pairs.iter().map(|(id, _)| id))
            .collect();
        assert_eq!(ids.len(), 4);
        assert!(ids.iter().all(|id| id.len() == 24));
    }

    // --- outcome → wire mapping (§5.4, pinned) ----------------------------------

    #[test]
    fn outcome_mapping_table_is_pinned() {
        assert_eq!(
            outcome_action(&ElicitAskOutcome::Answered {
                card_id: 1,
                option_id: "x".into()
            }),
            McpElicitationAction::Accept
        );
        assert_eq!(
            outcome_action(&ElicitAskOutcome::Dismiss),
            McpElicitationAction::Decline
        );
        assert_eq!(
            outcome_action(&ElicitAskOutcome::Cancel),
            McpElicitationAction::Cancel
        );
        assert_eq!(
            outcome_action(&ElicitAskOutcome::Timeout),
            McpElicitationAction::Cancel
        );
        assert_eq!(
            outcome_action(&ElicitAskOutcome::Unavailable),
            McpElicitationAction::Decline
        );
    }

    #[test]
    fn action_reply_wire_shapes() {
        assert_eq!(
            action_reply(McpElicitationAction::Accept, Some(json!({"color": "blue"}))),
            json!({"action": "accept", "content": {"color": "blue"}})
        );
        assert_eq!(
            action_reply(McpElicitationAction::Decline, None),
            json!({"action": "decline"})
        );
        assert_eq!(
            action_reply(McpElicitationAction::Cancel, None),
            json!({"action": "cancel"})
        );
        // Declines/cancels never carry content even if asked to.
        assert_eq!(
            action_reply(McpElicitationAction::Decline, Some(json!({"x": 1}))),
            json!({"action": "decline"})
        );
    }

    // --- ask wait (timeout / cancel / dead host) --------------------------------

    fn test_bridge(
        dir: &tempfile::TempDir,
        ask: Arc<dyn Fn(ElicitQuestion) -> ElicitAskOutcome + Send + Sync>,
    ) -> ElicitationBridge {
        ElicitationBridge::new(
            "fake".into(),
            "fake".into(),
            "sess".into(),
            Arc::new(JournalCoordinator::open(dir.path().join("j.jsonl")).expect("journal")),
            Arc::new(Mutex::new(Some("call-1".to_string()))),
            ask,
        )
    }

    #[test]
    fn run_ask_maps_deadline_to_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = test_bridge(
            &dir,
            Arc::new(|_| {
                std::thread::sleep(Duration::from_secs(5));
                ElicitAskOutcome::Dismiss
            }),
        )
        .with_ask_timeout(Duration::from_millis(150));
        let cancel = Arc::new(AtomicBool::new(false));
        let question = ElicitQuestion {
            header: "h".into(),
            message: "m".into(),
            options: vec![],
        };
        let started = Instant::now();
        assert_eq!(bridge.run_ask(question, &cancel), ElicitAskOutcome::Timeout);
        assert!(started.elapsed() < Duration::from_secs(2), "bounded wait");
    }

    #[test]
    fn run_ask_observes_retirement_cancel_within_a_poll_slice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = test_bridge(
            &dir,
            Arc::new(|_| {
                std::thread::sleep(Duration::from_secs(5));
                ElicitAskOutcome::Dismiss
            }),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        cancel.store(true, Ordering::SeqCst);
        let question = ElicitQuestion {
            header: "h".into(),
            message: "m".into(),
            options: vec![],
        };
        let started = Instant::now();
        assert_eq!(bridge.run_ask(question, &cancel), ElicitAskOutcome::Cancel);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn run_ask_dead_host_is_unavailable_not_a_hang() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bridge = test_bridge(&dir, Arc::new(|_| panic!("host died")));
        let cancel = Arc::new(AtomicBool::new(false));
        let question = ElicitQuestion {
            header: "h".into(),
            message: "m".into(),
            options: vec![],
        };
        assert_eq!(
            bridge.run_ask(question, &cancel),
            ElicitAskOutcome::Unavailable
        );
    }

    // --- journal-first (§5.6) ---------------------------------------------------

    fn journaled_elicitations(
        path: &std::path::Path,
    ) -> Vec<(String, McpElicitationAction, String, String, u64)> {
        let report = nano_session::read_journal(path).expect("readable journal");
        report
            .envelopes
            .iter()
            .filter_map(|env| match &env.op {
                Op::McpElicitation {
                    elicitation_id,
                    action,
                    schema_digest,
                    answer_digest,
                    card_id,
                    ..
                } => Some((
                    elicitation_id.clone(),
                    *action,
                    schema_digest.clone(),
                    answer_digest.clone(),
                    *card_id,
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn decide_journals_every_action_with_monotonic_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("j.jsonl");
        let bridge = test_bridge(&dir, Arc::new(|_| ElicitAskOutcome::Unavailable));
        let req = request(ELICITATION_CREATE_METHOD, elicit_params(json!({})));
        let content = json!({"color": "blue"});
        let answer_digest = canonical_answer_digest(&content);

        let schema_digest = "a".repeat(64);
        let accept = bridge
            .decide(
                &req,
                McpElicitationAction::Accept,
                42,
                &schema_digest,
                &answer_digest,
                Some(content.clone()),
            )
            .expect("decision");
        assert_eq!(
            accept,
            json!({"action": "accept", "content": {"color": "blue"}})
        );
        let decline = bridge
            .decide(
                &req,
                McpElicitationAction::Decline,
                0,
                &schema_digest,
                "",
                None,
            )
            .expect("decision");
        assert_eq!(decline, json!({"action": "decline"}));
        let cancel = bridge
            .decide(
                &req,
                McpElicitationAction::Cancel,
                0,
                &schema_digest,
                "",
                None,
            )
            .expect("decision");
        assert_eq!(cancel, json!({"action": "cancel"}));

        let ops = journaled_elicitations(&path);
        assert_eq!(ops.len(), 3, "every processed decision is journaled");
        assert_eq!(ops[0].0, "sess-elicit-1");
        assert_eq!(ops[1].0, "sess-elicit-2");
        assert_eq!(ops[2].0, "sess-elicit-3");
        assert_eq!(ops[0].1, McpElicitationAction::Accept);
        assert_eq!(
            ops[0].3, answer_digest,
            "accept carries the canonical answer digest"
        );
        assert_eq!(ops[0].4, 42, "the card the answer came through");
        assert_eq!(ops[1].1, McpElicitationAction::Decline);
        assert_eq!(ops[1].3, "", "decline journals no answer digest");
        assert_eq!(ops[2].1, McpElicitationAction::Cancel);
        assert_eq!(ops[2].3, "", "cancel journals no answer digest");
        // Answer CONTENT is never journaled: the raw line holds digests only.
        let raw = std::fs::read_to_string(&path).expect("journal file");
        assert!(!raw.contains("blue"), "no answer content in the journal");
    }

    #[test]
    fn journal_failure_is_fail_closed_cancel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("j.jsonl");
        let coordinator = Arc::new(JournalCoordinator::open(&path).expect("journal"));
        // Poison the coordinator's mutex: a panic while the compaction guard
        // is held makes every later append return Err (deterministic on every
        // platform, unlike chmod/read-only tricks).
        let c = coordinator.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = c.compaction().expect("guard");
            panic!("deliberately poison the coordinator mutex");
        }));
        let bridge = ElicitationBridge::new(
            "fake".into(),
            "fake".into(),
            "sess".into(),
            coordinator,
            Arc::new(Mutex::new(Some("call-1".to_string()))),
            Arc::new(|_| ElicitAskOutcome::Unavailable),
        );
        let req = request(ELICITATION_CREATE_METHOD, elicit_params(json!({})));
        let reply = bridge
            .decide(
                &req,
                McpElicitationAction::Accept,
                42,
                "s",
                "a",
                Some(json!({"color": "blue"})),
            )
            .expect("a reply is always produced");
        assert_eq!(
            reply,
            json!({"action": "cancel"}),
            "no accept with a failed append"
        );
        assert!(
            journaled_elicitations(&path).is_empty(),
            "nothing was journaled"
        );
    }

    /// F-P3-4 (the leg5b live proof): pre-kill the session journaled one
    /// elicitation decision; after `session/load` a FRESH bridge must resume
    /// the counter from the journal — the second post-resume answer is
    /// journaled under a fresh id AND answered on the wire. Before the fix
    /// the counter restarted at 0, the re-minted id collided, the append
    /// no-oped (`Ok(false)`), and the wire got an unjournaled accept.
    #[test]
    fn resume_restores_counter_so_post_resume_decisions_journal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("j.jsonl");
        let req = request(ELICITATION_CREATE_METHOD, elicit_params(json!({})));
        let schema_digest = "a".repeat(64);
        let content = json!({"color": "blue"});
        let answer_digest = canonical_answer_digest(&content);
        {
            // Pre-kill session: one decision journaled as "sess-elicit-1".
            let bridge = ElicitationBridge::new(
                "fake".into(),
                "fake".into(),
                "sess".into(),
                Arc::new(JournalCoordinator::open(&path).expect("journal")),
                Arc::new(Mutex::new(Some("call-1".to_string()))),
                Arc::new(|_| ElicitAskOutcome::Unavailable),
            );
            let reply = bridge
                .decide(
                    &req,
                    McpElicitationAction::Accept,
                    1,
                    &schema_digest,
                    &answer_digest,
                    Some(content.clone()),
                )
                .expect("decision");
            assert_eq!(reply["action"], "accept");
        }
        // Host kill + session/load: every in-memory bridge is gone; the
        // journal is the only survivor.
        let bridge = ElicitationBridge::new(
            "fake".into(),
            "fake".into(),
            "sess".into(),
            Arc::new(JournalCoordinator::open(&path).expect("journal")),
            Arc::new(Mutex::new(Some("call-1".to_string()))),
            Arc::new(|_| ElicitAskOutcome::Unavailable),
        );
        let reply = bridge
            .decide(
                &req,
                McpElicitationAction::Accept,
                2,
                &schema_digest,
                &answer_digest,
                Some(content),
            )
            .expect("decision");
        assert_eq!(
            reply["action"], "accept",
            "the answer reaches the wire BECAUSE it journaled"
        );
        let ops = journaled_elicitations(&path);
        assert_eq!(ops.len(), 2, "both decisions durable");
        assert_eq!(ops[0].0, "sess-elicit-1");
        assert_eq!(ops[1].0, "sess-elicit-2", "no id re-mint after resume");
    }

    /// F-P3-4 backstop: an id that became durable AFTER the bridge's restore
    /// (an externally-appended op the restore could not see) collides at
    /// append time — the idempotent `Ok(false)` must never pass as success.
    /// The bridge re-mints and journals the decision under a fresh id.
    #[test]
    fn id_collision_remints_and_never_wires_an_unjournaled_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("j.jsonl");
        let coordinator = Arc::new(JournalCoordinator::open(&path).expect("journal"));
        let bridge = ElicitationBridge::new(
            "fake".into(),
            "fake".into(),
            "sess".into(),
            coordinator.clone(),
            Arc::new(Mutex::new(Some("call-1".to_string()))),
            Arc::new(|_| ElicitAskOutcome::Unavailable),
        );
        // Land "sess-elicit-1" AFTER construction: the restore (empty
        // journal) could not know about it.
        coordinator
            .append(&OpEnvelope::new(
                "sess-elicit-1".to_string(),
                "now",
                Op::McpElicitation {
                    elicitation_id: "sess-elicit-1".to_string(),
                    server_id: "fake".to_string(),
                    call_id: "call-x".to_string(),
                    request_id: "7".to_string(),
                    card_id: 0,
                    action: McpElicitationAction::Decline,
                    schema_digest: "s".repeat(64),
                    answer_digest: String::new(),
                },
            ))
            .expect("external append");
        let req = request(ELICITATION_CREATE_METHOD, elicit_params(json!({})));
        let content = json!({"color": "blue"});
        let answer_digest = canonical_answer_digest(&content);
        let reply = bridge
            .decide(
                &req,
                McpElicitationAction::Accept,
                3,
                &"a".repeat(64),
                &answer_digest,
                Some(content),
            )
            .expect("decision");
        assert_eq!(
            reply["action"], "accept",
            "the re-minted decision journaled, so the accept may wire"
        );
        let ops = journaled_elicitations(&path);
        assert_eq!(ops.len(), 2, "the collision did not swallow the decision");
        assert_eq!(ops[0].0, "sess-elicit-1");
        assert_eq!(ops[1].0, "sess-elicit-2");
        assert_eq!(ops[1].1, McpElicitationAction::Accept);
    }
}

// ---------------------------------------------------------------------------
// Live dispatcher round-trips: a real stdio fake server (powershell on
// Windows, sh on unix — the same JSON-RPC line protocol both ways, following
// the mcp.rs integration-test pattern) driving `elicitation/create`
// mid-`tools/call` through a real Connection, real handler thread, real slot.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod live_tests {
    use super::*;
    use nano_mcp::client::McpClient;
    use nano_mcp::dispatcher::ConnectionOptions;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    const ENUM_SCHEMA: &str = r#"{"type":"object","properties":{"color":{"type":"string","enum":["red","blue","green"]}}}"#;

    /// Fake server modes: "once" = one elicitation per tools/call; "storm" =
    /// ten per call; "report-unsolicited" = one elicitation sent BEFORE any
    /// tools/call (never designated); "after-retire" = an elicitation sent
    /// right after the first call's response (slot already RETIRED). The
    /// elicitation replies the server observed come back verbatim (joined by
    /// " | ") as the tools/call result text.
    #[cfg(windows)]
    fn fake_server(mode: &str) -> (String, Vec<String>) {
        let script = format!(
            r#"
$mode = "{mode}"
$reader = [System.Console]::In
$out = [System.Console]::Out
$stored = ""
$elicit = '{{"jsonrpc":"2.0","id":__EID__,"method":"elicitation/create","params":{{"message":"pick a color","requestedSchema":{ENUM_SCHEMA}}}}}'
while ($true) {{
    $line = $reader.ReadLine()
    if ($null -eq $line) {{ break }}
    $obj = $line | ConvertFrom-Json
    if ($obj.method -eq "initialize") {{
        $out.WriteLine("{{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{{`"protocolVersion`":`"2025-06-18`",`"capabilities`":{{`"tools`":{{}}}},`"serverInfo`":{{`"name`":`"fake`",`"version`":`"0`"}}}}}}")
        $out.Flush()
    }} elseif ($obj.method -eq "tools/list") {{
        $out.WriteLine("{{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{{`"tools`":[{{`"name`":`"drive`",`"description`":`"drives elicitation`"}}]}}}}")
        $out.Flush()
        if ($mode -eq "report-unsolicited") {{
            $out.WriteLine(($elicit -replace '__EID__', '100'))
            $out.Flush()
            $stored = $reader.ReadLine()
        }}
    }} elseif ($obj.method -eq "tools/call") {{
        if ($mode -eq "after-retire" -and "$($obj.params.arguments.phase)" -eq "first") {{
            $out.WriteLine("{{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{{`"content`":[{{`"type`":`"text`",`"text`":`"first-done`"}}],`"isError`":false}}}}")
            $out.Flush()
            $out.WriteLine(($elicit -replace '__EID__', '300'))
            $out.Flush()
            # The next client frame (a racing second tools/call) can arrive
            # BEFORE the elicitation reply on the same pipe: stash it and
            # answer it once the reply lands.
            $stashed = ""
            while ($true) {{
                $l = $reader.ReadLine()
                if ($null -eq $l) {{ break }}
                if ($l.Contains('"id":300')) {{ $stored = $l; break }}
                $stashed = $l
            }}
            if ($stashed -ne "") {{
                $obj2 = $stashed | ConvertFrom-Json
                $joined2 = $stored.Replace("\", "\\").Replace('"', '\"')
                $out.WriteLine("{{`"jsonrpc`":`"2.0`",`"id`":$($obj2.id),`"result`":{{`"content`":[{{`"type`":`"text`",`"text`":`"$joined2`"}}],`"isError`":false}}}}")
                $out.Flush()
            }}
            continue
        }}
        $replies = @()
        $n = 0
        if ($mode -eq "once") {{ $n = 1 }}
        if ($mode -eq "storm") {{ $n = 10 }}
        for ($i = 0; $i -lt $n; $i++) {{
            $out.WriteLine(($elicit -replace '__EID__', [string](200 + $i)))
            $out.Flush()
            $replies += $reader.ReadLine()
        }}
        if ($mode -eq "report-unsolicited" -or $mode -eq "after-retire") {{ $replies += $stored }}
        $joined = ($replies -join " | ").Replace("\", "\\").Replace('"', '\"')
        $out.WriteLine("{{`"jsonrpc`":`"2.0`",`"id`":$($obj.id),`"result`":{{`"content`":[{{`"type`":`"text`",`"text`":`"$joined`"}}],`"isError`":false}}}}")
        $out.Flush()
    }}
}}
"#
        );
        (
            "powershell.exe".to_string(),
            vec!["-NoProfile".to_string(), "-Command".to_string(), script],
        )
    }

    #[cfg(unix)]
    fn fake_server(mode: &str) -> (String, Vec<String>) {
        let script = format!(
            r#"
mode="{mode}"
stored=""
elicit() {{
    printf '{{"jsonrpc":"2.0","id":%s,"method":"elicitation/create","params":{{"message":"pick a color","requestedSchema":{ENUM_SCHEMA}}}}}\n' "$1"
}}
esc() {{ printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }}
while IFS= read -r line; do
    id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *'"initialize"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-06-18","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"fake","version":"0"}}}}}}\n' "$id" ;;
        *'"tools/list"'*)
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"drive","description":"drives elicitation"}}]}}}}\n' "$id"
            if [ "$mode" = "report-unsolicited" ]; then
                elicit 100
                IFS= read -r stored
            fi ;;
        *'"tools/call"'*)
            if [ "$mode" = "after-retire" ]; then
                case "$line" in
                    *'"first"'*)
                        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"first-done"}}],"isError":false}}}}\n' "$id"
                        elicit 300
                        # A racing second tools/call can arrive BEFORE the
                        # elicitation reply on the same pipe: stash it and
                        # answer it once the reply lands.
                        stashed=""
                        while IFS= read -r l; do
                            case "$l" in
                                *'"id":300'*) stored="$l"; break ;;
                                *) stashed="$l" ;;
                            esac
                        done
                        if [ -n "$stashed" ]; then
                            id2=$(printf '%s' "$stashed" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
                            e2=$(esc "$stored")
                            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"%s"}}],"isError":false}}}}\n' "$id2" "$e2"
                        fi
                        continue ;;
                esac
            fi
            n=0
            [ "$mode" = "once" ] && n=1
            [ "$mode" = "storm" ] && n=10
            replies=""
            i=0
            while [ "$i" -lt "$n" ]; do
                elicit $((200 + i))
                IFS= read -r rep
                e=$(esc "$rep")
                if [ -z "$replies" ]; then replies="$e"; else replies="$replies | $e"; fi
                i=$((i + 1))
            done
            if [ "$mode" = "report-unsolicited" ] || [ "$mode" = "after-retire" ]; then
                replies=$(esc "$stored")
            fi
            printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"%s"}}],"isError":false}}}}\n' "$id" "$replies" ;;
    esac
done
"#
        );
        ("sh".to_string(), vec!["-c".to_string(), script])
    }

    struct Live {
        client: McpClient,
        journal_path: std::path::PathBuf,
        asks: Arc<Mutex<Vec<ElicitQuestion>>>,
        _dir: tempfile::TempDir,
    }

    fn connect_live(
        mode: &str,
        poison_journal: bool,
        ask: impl Fn(&ElicitQuestion) -> ElicitAskOutcome + Send + Sync + 'static,
    ) -> Live {
        let dir = tempfile::tempdir().expect("tempdir");
        let journal_path = dir.path().join("journal.jsonl");
        let coordinator = Arc::new(JournalCoordinator::open(&journal_path).expect("journal"));
        if poison_journal {
            // A panic while the compaction guard is held poisons the
            // coordinator mutex; every later append fails deterministically.
            let c = coordinator.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _guard = c.compaction().expect("guard");
                panic!("deliberately poison the coordinator mutex");
            }));
        }
        let asks: Arc<Mutex<Vec<ElicitQuestion>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = asks.clone();
        let bridge = Arc::new(ElicitationBridge::new(
            "fake".into(),
            "fake".into(),
            "sess".into(),
            coordinator,
            Arc::new(Mutex::new(Some("call-1".to_string()))),
            Arc::new(move |question| {
                seen.lock().unwrap().push(question.clone());
                ask(&question)
            }),
        ));
        let options = ConnectionOptions {
            request_handler: bridge.clone(),
            slot_retired_hook: bridge.slot_retired_hook(),
            ..ConnectionOptions::default()
        };
        let (command, args) = fake_server(mode);
        let client = McpClient::connect_with_options(&command, &args, &[], options)
            .expect("connect to fake server");
        Live {
            client,
            journal_path,
            asks,
            _dir: dir,
        }
    }

    /// The replies the fake server observed, extracted from the tools/call
    /// result text (verbatim frames joined by " | ").
    fn observed_replies(result: &Value) -> Vec<Value> {
        let text = result["content"][0]["text"]
            .as_str()
            .expect("tool result text");
        text.split(" | ")
            .map(|frame| serde_json::from_str(frame).expect("reply frame parses"))
            .collect()
    }

    fn journaled(path: &std::path::Path) -> Vec<Value> {
        nano_session::read_journal(path)
            .expect("readable journal")
            .envelopes
            .iter()
            .filter(|env| matches!(env.op, Op::McpElicitation { .. }))
            .map(|env| serde_json::to_value(env).expect("envelope serializes"))
            .collect()
    }

    fn answer_blue(question: &ElicitQuestion) -> ElicitAskOutcome {
        let (id, _) = question
            .options
            .iter()
            .find(|(_, label)| label == "blue")
            .expect("blue option offered");
        ElicitAskOutcome::Answered {
            card_id: 42,
            option_id: id.clone(),
        }
    }

    #[test]
    fn live_accept_roundtrip_writes_content_and_completes_call() {
        let live = connect_live("once", false, answer_blue);
        let result = live
            .client
            .call_tool("drive", json!({}))
            .expect("tool call completes");
        let replies = observed_replies(&result);
        assert_eq!(replies.len(), 1);
        assert_eq!(
            replies[0],
            json!({"jsonrpc": "2.0", "id": 200, "result": {"action": "accept", "content": {"color": "blue"}}})
        );
        let asks = live.asks.lock().unwrap();
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].header, "MCP server 'fake' asks:");
        assert_eq!(asks[0].message, "pick a color");
        assert_eq!(asks[0].options.len(), 3);
        assert!(
            asks[0]
                .options
                .iter()
                .all(|(id, _)| id.len() == 24 && id.chars().all(|c| c.is_ascii_hexdigit()))
        );
        drop(asks);
        // Journal-first record: the digest recomputes over the wire content.
        let ops = journaled(&live.journal_path);
        assert_eq!(ops.len(), 1);
        let op = &ops[0]["op"];
        assert_eq!(op["elicitation_id"], "sess-elicit-1");
        assert_eq!(op["server_id"], "fake");
        assert_eq!(op["call_id"], "call-1");
        assert_eq!(op["request_id"], "200");
        assert_eq!(op["card_id"], 42);
        assert_eq!(op["action"], "accept");
        assert_eq!(
            op["schema_digest"].as_str().expect("digest"),
            canonical_schema_digest(&serde_json::from_str::<Value>(ENUM_SCHEMA).expect("schema"))
        );
        assert_eq!(
            op["answer_digest"].as_str().expect("digest"),
            canonical_answer_digest(&json!({"color": "blue"}))
        );
        live.client.close();
    }

    #[test]
    fn live_dismiss_maps_to_decline() {
        let live = connect_live("once", false, |_| ElicitAskOutcome::Dismiss);
        let result = live.client.call_tool("drive", json!({})).expect("call");
        let replies = observed_replies(&result);
        assert_eq!(replies[0]["result"], json!({"action": "decline"}));
        let ops = journaled(&live.journal_path);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["op"]["action"], "decline");
        assert_eq!(ops[0]["op"]["answer_digest"], "");
        live.client.close();
    }

    #[test]
    fn live_cancel_maps_to_cancel() {
        let live = connect_live("once", false, |_| ElicitAskOutcome::Cancel);
        let result = live.client.call_tool("drive", json!({})).expect("call");
        let replies = observed_replies(&result);
        assert_eq!(replies[0]["result"], json!({"action": "cancel"}));
        let ops = journaled(&live.journal_path);
        assert_eq!(ops[0]["op"]["action"], "cancel");
        live.client.close();
    }

    #[test]
    fn live_unknown_option_id_is_internal_decline_never_a_guess() {
        let live = connect_live("once", false, |_| ElicitAskOutcome::Answered {
            card_id: 42,
            option_id: "f".repeat(24), // a foreign/bogus opaque id
        });
        let result = live.client.call_tool("drive", json!({})).expect("call");
        let replies = observed_replies(&result);
        assert_eq!(
            replies[0]["result"],
            json!({"action": "decline"}),
            "an id outside the binding declines; no guessed value"
        );
        let ops = journaled(&live.journal_path);
        assert_eq!(ops[0]["op"]["action"], "decline");
        live.client.close();
    }

    #[test]
    fn live_no_designated_call_gets_32601_and_no_card() {
        let live = connect_live("report-unsolicited", false, |_| {
            panic!("no card may open without a designated call")
        });
        // Let the handler thread answer the unsolicited elicitation BEFORE
        // any tools/call designates the slot. The server holds its stored
        // reply until the first tools/call, so ordering after the sleep is
        // deterministic up to a stalled handler (generous margin).
        std::thread::sleep(Duration::from_millis(1000));
        let result = live.client.call_tool("drive", json!({})).expect("call");
        let replies = observed_replies(&result);
        assert_eq!(replies.len(), 1);
        assert_eq!(
            replies[0]["error"]["code"],
            json!(nano_mcp::protocol::METHOD_NOT_FOUND)
        );
        assert!(
            journaled(&live.journal_path).is_empty(),
            "a protocol-level rejection is not a journaled decision"
        );
        live.client.close();
    }

    #[test]
    fn live_elicitation_queued_at_retirement_gets_one_decline() {
        let live = connect_live("after-retire", false, |_| {
            panic!("no card may open for a retired parent")
        });
        let first = live
            .client
            .call_tool("drive", json!({"phase": "first"}))
            .expect("first call completes");
        assert_eq!(first["content"][0]["text"], "first-done");
        // The reader processed the call's response — retiring the slot and
        // firing the hook — BEFORE it queued the elicitation (line order), so
        // the decline below is deterministic.
        let second = live
            .client
            .call_tool("drive", json!({"phase": "second"}))
            .expect("second call completes");
        let replies = observed_replies(&second);
        assert_eq!(
            replies.len(),
            1,
            "exactly ONE reply to the queued elicitation"
        );
        assert_eq!(replies[0]["result"], json!({"action": "decline"}));
        let ops = journaled(&live.journal_path);
        assert_eq!(
            ops.len(),
            1,
            "the retirement decline is a journaled decision"
        );
        assert_eq!(ops[0]["op"]["action"], "decline");
        live.client.close();
    }

    #[test]
    fn live_storm_drains_at_the_per_call_cap() {
        let ask_calls = Arc::new(AtomicUsize::new(0));
        let counted = ask_calls.clone();
        let live = connect_live("storm", false, move |q| {
            counted.fetch_add(1, Ordering::SeqCst);
            answer_blue(q)
        });
        let result = live.client.call_tool("drive", json!({})).expect("call");
        let replies = observed_replies(&result);
        assert_eq!(replies.len(), 10);
        let accepts = replies
            .iter()
            .filter(|r| r["result"]["action"] == "accept")
            .count();
        let declines = replies
            .iter()
            .filter(|r| r["result"]["action"] == "decline")
            .count();
        assert_eq!(accepts, 8, "exactly the per-call cap is answered");
        assert_eq!(declines, 2, "the overflow is auto-declined");
        assert_eq!(
            ask_calls.load(Ordering::SeqCst),
            8,
            "no card opens past the cap"
        );
        let ops = journaled(&live.journal_path);
        assert_eq!(ops.len(), 10, "all ten decisions journaled");
        assert!(
            ops.iter()
                .all(|op| op["op"]["elicitation_id"].as_str() == op["id"].as_str())
        );
        live.client.close();
    }

    #[test]
    fn live_journal_failure_yields_wire_cancel_never_accept() {
        let live = connect_live("once", true, answer_blue);
        let result = live.client.call_tool("drive", json!({})).expect("call");
        let replies = observed_replies(&result);
        assert_eq!(
            replies[0]["result"],
            json!({"action": "cancel"}),
            "JournalUnavailable is fail-closed: the answered elicitation cancels"
        );
        assert!(
            journaled(&live.journal_path).is_empty(),
            "the append failed, so nothing was journaled"
        );
        live.client.close();
    }
}
