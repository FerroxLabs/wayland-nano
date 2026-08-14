//! Op envelope and Op vocabulary.
//!
//! Design notes:
//! - `#[serde(other)] Unknown` gives forward tolerance: a journal written by a
//!   newer Nano (with Ops this build does not know) still loads; unknown Ops
//!   are preserved in the raw file but skipped during replay.
//! - `id` gives idempotence: replay dedupes repeated ids, so a retried append
//!   after a crash-uncertain write cannot double-apply effects.

use serde::Deserialize;
use serde::Serialize;

/// Current journal schema version. Bump only for breaking envelope changes;
/// additive Op variants ride the same version (unknown-skip covers old readers).
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpEnvelope {
    pub v: u32,
    pub id: String,
    pub ts: String,
    pub op: Op,
}

impl OpEnvelope {
    pub fn new(id: impl Into<String>, ts: impl Into<String>, op: Op) -> Self {
        Self {
            v: SCHEMA_VERSION,
            id: id.into(),
            ts: ts.into(),
            op,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed,
    /// Written only by replay/restore, never by a live writer: marks a turn
    /// that was in progress when the journal tail was cut.
    Interrupted,
}

/// Bounded, non-sensitive reasons a compaction attempt is abandoned (C1).
/// Free-text reasons are deliberately impossible: the text that tripped the
/// gate is exactly what must never reach the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionCancelReason {
    /// Journaled by a build that predates the reason field.
    #[default]
    Unspecified,
    /// The summary tripped the pre-persistence secret scan.
    RedactionHit,
    /// The secret scanner itself errored (fail-closed, nothing persisted).
    RedactorError,
    /// The summarization model call failed.
    ModelFailed,
    /// The compaction call overflowed even after pair-preserving escalation.
    OverflowEscalationExhausted,
    /// The journal append/flush failed, so the in-memory swap never happened.
    JournalWriteFailed,
    /// A reason written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Goal lifecycle states (C11, kimi-minimal — Q7 RULED). `complete` is the
/// only success terminal; a budget trip is `blocked` with a machine-readable
/// reason, never a separate status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatusKind {
    Active,
    Paused,
    Blocked,
    Complete,
    /// A status written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Bounded, machine-readable reasons for a goal transition (C11). Same rule
/// as [`CompactionCancelReason`]: never free text — a reason field must never
/// become a secondary persistence channel for content that failed a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalReason {
    /// No reason applies (e.g. activation, completion).
    #[default]
    Unspecified,
    BudgetTokens,
    BudgetTurns,
    BudgetWallclock,
    ModelDeclared,
    Error,
    Cancelled,
    /// A reason written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Terminal goal outcomes journaled by `GoalEnd` (C11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalOutcome {
    Complete,
    Blocked,
    /// An outcome written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// One server's atomically journaled ToolSearch hydration batch.
/// `server_id` is the §2.7 stable instance id (`srv_<16 hex>` — see
/// [`is_mcp_instance_id`]), NEVER the display name (F-P3-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydrationEntry {
    pub server_id: String,
    pub tool_names: Vec<String>,
    pub tools_digest: String,
}

/// Hydration state carried across a covered compaction prefix.
/// `server_id` is the §2.7 instance id, as on [`HydrationEntry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydrationCarryEntry {
    pub server_id: String,
    pub tool_names: Vec<String>,
    pub tools_digest: String,
    pub recent_digests: Vec<String>,
}

/// Audit-safe outcome of a server-initiated elicitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpElicitationAction {
    Accept,
    Decline,
    Cancel,
    #[serde(other)]
    Unknown,
}

/// HTTP methods admitted by a journaled OAuth endpoint grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GrantMethod {
    Get,
    Post,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantEndpoint {
    pub method: GrantMethod,
    pub path: String,
}

// ── P3 journal bounds (P3-mcp-ecosystem-design §3.3/§5.6/§6.3) ───────────
// The host validates BEFORE the journal append; replay stays tolerant (the
// adversarial-journal contract: hostile payloads fold without panic). These
// constants and validators are the single enforcing definitions.

/// A hydration batch covers at most this many servers (the fixed limit-10
/// search result cannot span more).
pub const MAX_HYDRATION_ENTRIES: usize = 8;
/// Per-server journaled tool names per hydration/carry entry.
pub const MAX_HYDRATION_TOOL_NAMES: usize = 64;
/// Per-tool-name character cap.
pub const MAX_HYDRATION_TOOL_NAME_CHARS: usize = 128;
/// The carried churn window per server.
pub const MAX_RECENT_DIGESTS: usize = 8;
/// Canonical digests are sha256 hex: exactly 64 lowercase hex chars.
pub const DIGEST_HEX_CHARS: usize = 64;
/// A journaled OAuth grant names at most this many exact endpoint pairs.
pub const MAX_GRANT_ENDPOINTS: usize = 4;
/// P4 §2.6: a `ShellRuleAmended` op's `prefix` payload caps.
pub const MAX_RULE_AMEND_TOKENS: usize = 8;
pub const MAX_RULE_AMEND_TOKEN_CHARS: usize = 128;
/// `as_origin` is an https origin only, capped.
pub const MAX_AS_ORIGIN_CHARS: usize = 256;
/// The validated issuer string cap.
pub const MAX_ISSUER_CHARS: usize = 512;

/// Lowercase 64-hex sha256 shape, the only digest form the journal accepts.
pub fn is_canonical_digest(value: &str) -> bool {
    value.len() == DIGEST_HEX_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// The §2.7 stable server-instance id shape: `srv_` + the first 16 lowercase
/// hex chars of sha256 over the canonical spec JSON (minted by nano-agent's
/// `mcp::mint_instance_id`). Every P3 journaled server key — hydration,
/// elicitation, OAuth grant — is this id, never a display name.
pub fn is_mcp_instance_id(value: &str) -> bool {
    value.len() == 4 + 16
        && value.starts_with("srv_")
        && value[4..]
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Validates one hydration entry (§3.3 bounds). Returns the violated rule.
pub fn validate_hydration_entry(entry: &HydrationEntry) -> Result<(), &'static str> {
    if entry.server_id.is_empty() || entry.server_id.len() > MAX_HYDRATION_TOOL_NAME_CHARS {
        return Err("server_id empty or over the cap");
    }
    if entry.tool_names.len() > MAX_HYDRATION_TOOL_NAMES {
        return Err("too many tool names");
    }
    if entry
        .tool_names
        .iter()
        .any(|name| name.is_empty() || name.chars().count() > MAX_HYDRATION_TOOL_NAME_CHARS)
    {
        return Err("tool name empty or over the char cap");
    }
    if !is_canonical_digest(&entry.tools_digest) {
        return Err("tools_digest is not 64-hex");
    }
    Ok(())
}

/// Validates the carry form: the hydration bounds plus the bounded churn
/// window (§3.3/§3.4).
pub fn validate_hydration_carry_entry(entry: &HydrationCarryEntry) -> Result<(), &'static str> {
    validate_hydration_entry(&HydrationEntry {
        server_id: entry.server_id.clone(),
        tool_names: entry.tool_names.clone(),
        tools_digest: entry.tools_digest.clone(),
    })?;
    if entry.recent_digests.len() > MAX_RECENT_DIGESTS {
        return Err("recent_digests over the window cap");
    }
    if entry.recent_digests.iter().any(|d| !is_canonical_digest(d)) {
        return Err("recent_digests entry is not 64-hex");
    }
    Ok(())
}

/// Validates a whole `McpToolHydration` batch (one atomic append, ≤ 8
/// servers).
pub fn validate_hydration_batch(entries: &[HydrationEntry]) -> Result<(), &'static str> {
    if entries.is_empty() || entries.len() > MAX_HYDRATION_ENTRIES {
        return Err("batch empty or over the server cap");
    }
    for entry in entries {
        validate_hydration_entry(entry)?;
    }
    Ok(())
}

/// The server-influenced elicitation request-id cap (§5.6): the stringified
/// JSON-RPC id is bounded before it may reach the journal.
pub const MAX_ELICITATION_REQUEST_ID_CHARS: usize = 64;

/// Validates a `McpElicitation` payload (§5.6 bounds): ids bounded, digests
/// canonical (`answer_digest` may be empty for decline/cancel).
pub fn validate_elicitation(
    elicitation_id: &str,
    server_id: &str,
    call_id: &str,
    request_id: &str,
    schema_digest: &str,
    answer_digest: &str,
) -> Result<(), &'static str> {
    if elicitation_id.is_empty() || elicitation_id.chars().count() > MAX_AS_ORIGIN_CHARS {
        return Err("elicitation_id empty or over the cap");
    }
    if server_id.is_empty() || server_id.chars().count() > MAX_HYDRATION_TOOL_NAME_CHARS {
        return Err("server_id empty or over the cap");
    }
    // call_id may be empty (attribution unavailable at a race edge); bounded.
    if call_id.chars().count() > MAX_AS_ORIGIN_CHARS {
        return Err("call_id over the cap");
    }
    if request_id.chars().count() > MAX_ELICITATION_REQUEST_ID_CHARS {
        return Err("request_id over the cap");
    }
    if !is_canonical_digest(schema_digest) {
        return Err("schema_digest is not 64-hex");
    }
    if !answer_digest.is_empty() && !is_canonical_digest(answer_digest) {
        return Err("answer_digest is not empty-or-64-hex");
    }
    Ok(())
}

/// Validates a `McpOauthGrant` payload (§6.3 bounds + the §2.7 identity
/// rule). Origin-shape rules (https-only, host normalization) are enforced
/// by the egress layer's `host_of`; here the journal-side caps apply, plus
/// the fail-closed server-id check: the grant keys CREDENTIAL storage
/// (keyring account = instance id, §6), so a display-name key is refused at
/// the journal boundary rather than mis-keying stored tokens (F-P3-3).
pub fn validate_oauth_grant(
    server_id: &str,
    as_origin: &str,
    issuer: &str,
    endpoints: &[GrantEndpoint],
) -> Result<(), &'static str> {
    if !is_mcp_instance_id(server_id) {
        return Err("server_id is not a §2.7 instance id (srv_<16 hex>)");
    }
    if as_origin.is_empty() || as_origin.chars().count() > MAX_AS_ORIGIN_CHARS {
        return Err("as_origin empty or over the cap");
    }
    if issuer.is_empty() || issuer.chars().count() > MAX_ISSUER_CHARS {
        return Err("issuer empty or over the cap");
    }
    if endpoints.len() > MAX_GRANT_ENDPOINTS {
        return Err("too many grant endpoints");
    }
    for endpoint in endpoints {
        if matches!(endpoint.method, GrantMethod::Unknown) {
            return Err("grant endpoint method unknown");
        }
        if endpoint.path.is_empty()
            || !endpoint.path.starts_with('/')
            || endpoint.path.chars().count() > MAX_ISSUER_CHARS
        {
            return Err("grant endpoint path invalid");
        }
    }
    Ok(())
}

/// Validates a `ShellRuleAmended` payload (P4 §2.6 bounds). The journal-side
/// caps are checked BEFORE the op is appended (the wiring validates at mint
/// time, so an over-bounds amendment never reaches the append); the digest
/// must be the canonical sha256-hex form.
pub fn validate_shell_rule_amended(
    amendment_id: &str,
    prefix: &[String],
    rule_digest: &str,
) -> Result<(), &'static str> {
    if amendment_id.is_empty() || amendment_id.chars().count() > MAX_ISSUER_CHARS {
        return Err("amendment_id empty or over the cap");
    }
    if prefix.is_empty() || prefix.len() > MAX_RULE_AMEND_TOKENS {
        return Err("amended prefix empty or over the token cap");
    }
    if prefix
        .iter()
        .any(|token| token.is_empty() || token.chars().count() > MAX_RULE_AMEND_TOKEN_CHARS)
    {
        return Err("amended token empty or over the char cap");
    }
    if !is_canonical_digest(rule_digest) {
        return Err("rule_digest is not canonical sha256 hex");
    }
    Ok(())
}

// ── P1 economy (panel-certified design: P1-search-economy-design.md) ─────
// JOURNAL-MIGRATION REVIEW FLAG (RC2 coordinated review): the THREE P1
// journal additions — (a) `usage` on `Op::TurnEnd`, (b) `Op::BudgetGrant`,
// (c) `Op::ChildUsageRollup` — ride the coordinated RC2 journal-migration
// review together (design §3.3/§6): no piecemeal schema erosion. All three
// are numbers/bounded-enums-only payloads (digest-only invariant untouched),
// all serde-defaulted so pre-P1 journals replay unchanged, and old readers
// skip them via the `Unknown`-op forward tolerance.
//
// The SAME flag covers the P3 additions (P3-mcp-ecosystem-design sections
// 3.3/5.6/6.3, "one flag"): (d) `Op::McpToolHydration`, (e)
// `Op::McpElicitation`, (f) `Op::McpOauthGrant`, and (g) the serde-defaulted
// `CompactionComplete.mcp_hydration` carry field. All four carry ids,
// bounded strings, bounded enums, and digests only — no content payloads —
// and ride the identical forward-tolerance discipline.

/// Where a usage figure came from (P1 §3.5 provenance): provider-reported
/// wire usage vs the fail-closed conservative estimate charged when the wire
/// reports nothing. Bounded enum, `#[serde(other)]`-tolerant like
/// [`GoalReason`] — never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The provider reported these tokens on the wire.
    #[default]
    ProviderReported,
    /// The wire reported nothing; the meter charged the §3.5 conservative
    /// estimate (never zero — an under-reporting provider must not become a
    /// budget bypass).
    Estimated,
    /// A source written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Stable id of the §3.5 conservative-estimation formula (Q4 RULED): bump
/// when the formula changes so old journals stay re-derivable under the
/// rules that produced them.
pub const ESTIMATION_METHOD_VERSION: u32 = 1;

/// The turn-scoped usage record (P1 §3.4–3.5): the SUM across EVERY
/// `record_usage` call in the turn — explicitly NOT the last response's
/// usage — plus cost and estimation provenance. NUMBERS and bounded enums
/// only, no content: the digest-only journal invariant is untouched. All
/// fields serde-defaulted; the whole record is optional on `TurnEnd` and
/// omitted when a turn recorded no usage at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    /// Meter-computed cost in integer microcents (the budget authority;
    /// provider-reported `cost_usd` is observability only and is NEVER
    /// journaled here). Meaningful only when `priced` is true.
    #[serde(default)]
    pub microcents: u64,
    /// Cost-truth honesty flag (P1 §3.1): true = every contributing record
    /// had a real metered (or known-free) price; false = unpriceable, NEVER
    /// render as $0. Defaults true for an empty sum (nothing unpriced yet).
    #[serde(default = "default_priced")]
    pub priced: bool,
    /// Provenance (P1 §3.5): `estimated` when ANY contributing record took
    /// the conservative missing-usage charge.
    #[serde(default)]
    pub usage_source: UsageSource,
    /// The formula id that produced estimated charges ([`ESTIMATION_METHOD_VERSION`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimation_method_version: Option<u32>,
    /// Sum of the request-side input estimates used by estimated charges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_estimate_input: Option<u64>,
    /// Sum of the request-side output estimates (the reserved allowance)
    /// used by estimated charges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_estimate_output: Option<u64>,
    /// Sum of the conservative charges actually applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_estimate: Option<u64>,
}

fn default_priced() -> bool {
    true
}

impl Default for TurnUsage {
    /// An empty sum is "nothing unpriced yet" (`priced: true`); the first
    /// unpriceable record flips it false permanently.
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            cached_input_tokens: 0,
            reasoning_tokens: 0,
            microcents: 0,
            priced: true,
            usage_source: UsageSource::default(),
            estimation_method_version: None,
            request_estimate_input: None,
            request_estimate_output: None,
            applied_estimate: None,
        }
    }
}

impl TurnUsage {
    /// Total tokens counted against the session cap (input + output, the C11
    /// goal-budget accounting rule; cached input is part of the provider's
    /// reported input total, never double-counted).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    /// True when nothing was recorded (the `TurnEnd.usage` field stays
    /// omitted — new journals stay byte-minimal).
    pub fn is_zero(&self) -> bool {
        self.total_tokens() == 0
            && self.cached_input_tokens == 0
            && self.reasoning_tokens == 0
            && self.applied_estimate.unwrap_or(0) == 0
    }

    /// Fold one provider-reported response's usage into the sum.
    #[allow(clippy::too_many_arguments)]
    pub fn add_provider_reported(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        reasoning_tokens: u64,
        microcents: u64,
        priced: bool,
    ) {
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.cached_input_tokens = self.cached_input_tokens.saturating_add(cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(reasoning_tokens);
        self.microcents = self.microcents.saturating_add(microcents);
        self.priced &= priced;
    }

    /// Fold one §3.5 conservative (missing-usage) charge into the sum, WITH
    /// auditable provenance.
    #[allow(clippy::too_many_arguments)]
    pub fn add_estimated(
        &mut self,
        input_tokens: u64,
        output_tokens: u64,
        microcents: u64,
        priced: bool,
        method_version: u32,
        applied_estimate: u64,
    ) {
        self.input_tokens = self.input_tokens.saturating_add(input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
        self.microcents = self.microcents.saturating_add(microcents);
        self.priced &= priced;
        self.usage_source = UsageSource::Estimated;
        self.estimation_method_version = Some(method_version);
        self.request_estimate_input = Some(
            self.request_estimate_input
                .unwrap_or(0)
                .saturating_add(input_tokens),
        );
        self.request_estimate_output = Some(
            self.request_estimate_output
                .unwrap_or(0)
                .saturating_add(output_tokens),
        );
        self.applied_estimate = Some(
            self.applied_estimate
                .unwrap_or(0)
                .saturating_add(applied_estimate),
        );
    }

    /// Fold another recorded sum into this one (replay reconstruction,
    /// orphan-child rollups). Provenance merges conservatively: any
    /// estimated contribution marks the result estimated.
    pub fn add_sum(&mut self, other: &TurnUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.microcents = self.microcents.saturating_add(other.microcents);
        self.priced &= other.priced;
        if other.usage_source == UsageSource::Estimated {
            self.usage_source = UsageSource::Estimated;
        }
        if other.estimation_method_version.is_some() {
            self.estimation_method_version = other.estimation_method_version;
        }
        let merge = |a: &mut Option<u64>, b: Option<u64>| {
            if let Some(b) = b {
                *a = Some(a.unwrap_or(0).saturating_add(b));
            }
        };
        merge(
            &mut self.request_estimate_input,
            other.request_estimate_input,
        );
        merge(
            &mut self.request_estimate_output,
            other.request_estimate_output,
        );
        merge(&mut self.applied_estimate, other.applied_estimate);
    }
}

/// Goal budget limits (C11, kimi `GoalBudgetLimits` shape): all optional,
/// any combination. Enforcement lives in the goal driver (nano-agent).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBudgets {
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub turn_budget: Option<u64>,
    #[serde(default)]
    pub wall_clock_budget_ms: Option<u64>,
}

/// Cap on a goal objective (kimi `MAX_GOAL_OBJECTIVE_LENGTH`).
pub const MAX_GOAL_OBJECTIVE_LEN: usize = 4000;
/// Cap on a `goal_complete` summary, schema-validated like any other argument.
pub const MAX_GOAL_SUMMARY_LEN: usize = 2000;

// ── P2a vision intake (panel-certified design: P2a-vision-intake-design.md) ─
// JOURNAL-MIGRATION REVIEW FLAG (RC2 coordinated review, same review as the
// P1 trio above): the TWO P2a journal additions — (a) `input_blocks` on
// `Op::TurnBegin`, (b) `image_influenced` on `Op::CompactionComplete` — are
// both additive-optional (serde-defaulted, skipped when empty/false), so
// pre-P2a journals replay unchanged and old readers tolerate the fields
// (unknown-field tolerance on `Op` is load-bearing — CI-pinned in tests). `SCHEMA_VERSION`
// stays 1. The manifest carries metadata + sha256 DIGESTS only — image
// bytes NEVER reach the journal (the digest-only invariant).

/// The durable half of an attached image (P2a §5.2/§5.2.1): everything the
/// journal needs to reference and rehydrate a blob-store attachment —
/// digests only, NEVER bytes. `digest` is validated (`^[0-9a-f]{64}$`)
/// before ANY path use on journal read (§5.3).
pub use nano_model::image_result::ImageRef;

/// One entry of the ordered input-block manifest journaled on
/// `Op::TurnBegin` (P2a §5.2): the machine contract for reconstructing
/// arbitrary ordered ACP content — text/image interleaving, duplicates, and
/// user-authored `[Image #…]`-like text are all unambiguous because no
/// string matching is ever performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputBlock {
    Text { text: String },
    ImageRef(ImageRef),
}

/// `skip_serializing_if` helper for the `CompactionComplete.image_influenced`
/// flag (P2a §8 part 2): keeps false-valued journals byte-minimal. Not std;
/// the existing precedents use `Option::is_none`.
fn is_false(b: &bool) -> bool {
    !b
}

// ── P5 Flux Auto routing (panel-certified design: P5-auto-routing-design.md) ─
// JOURNAL-MIGRATION REVIEW FLAG (RC2 coordinated review, the SAME review the
// P1/P2a/P3 additions above ride): the THREE P5 journal additions —
// (a) `Op::RoutingSnapshot`, (b) `Op::RoutingAttemptBegin`, (c)
// `Op::RoutingReceipt` — are additive ops carrying ids, numbers, bounded
// strings, and bounded enums ONLY (the digest-only journal invariant is
// untouched: never credentials, headers, bodies, or raw provider errors).
// Pre-P5 journals replay unchanged; old readers skip the new ops via the
// `Unknown`-op forward tolerance. `SCHEMA_VERSION` stays 1.

/// How a turn's model reference was resolved (P5 §1): the journaled
/// consent/mode record. Only `AutoClientSide` admits the client-side
/// candidate ladder; the other three are pins or provider-side passthrough
/// and NEVER route client-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// No explicit/configured model: the resolved default (`flux-auto` when
    /// a Flux credential resolves, else the deterministic proven fallback) —
    /// alias passthrough to the provider ONLY, never client-side routing.
    ImplicitAliasPassthrough,
    /// An explicit session/CLI pin (`--model` / `session/set_model`).
    ExplicitAliasPin,
    /// The configured default model pin (`NANO_DEFAULT_MODEL`).
    ConfiguredDefaultAlias,
    /// The explicit Auto opt-in (`NANO_ROUTING_AUTO` / `--auto`) with the
    /// resolved reference `flux-auto` — the ONLY mode with a ladder.
    AutoClientSide,
    /// A mode written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

/// Whether a candidate is a provider-routed alias (the leaf is chosen
/// provider-side) or a concrete pinned leaf (P5 §2/§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// A Flux router alias (`flux-auto` &co): provider-side leaf selection.
    Alias,
    /// A concrete leaf: a pinned Flux leaf or a namespaced `provider:model`.
    Leaf,
    #[serde(other)]
    Unknown,
}

/// Bounded admission-rejection reasons for a filtered candidate (P5 §3/§5).
/// Never free text — the reason must never carry payload detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRejection {
    /// Not in the validated advertisement set.
    NotAdvertised,
    /// Advertised and credentialed but not live-proven (`is_proven` gate at
    /// selection time, not binding time).
    ProviderUnproven,
    /// No usable credential resolved at construction (or lost by resume).
    ProviderUncredentialed,
    /// The turn's required capability (image_in / tool-use) is not proven
    /// for this exact provider/surface/leaf. Unknown equals false.
    CapabilityUnproven,
    #[serde(other)]
    Unknown,
}

/// One candidate entry of the journaled snapshot (P5 §3/§4): the receipt for
/// FILTERED candidates lives here (`admitted: false` + `rejection`); admitted
/// candidates produce `RoutingAttemptBegin`/`RoutingReceipt` ops when run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingCandidate {
    pub provider: String,
    /// The bare candidate id (leaf or alias) — never namespaced on the wire.
    pub candidate: String,
    pub kind: CandidateKind,
    pub admitted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<CandidateRejection>,
}

/// The P5 §4 classifier output, journaled per failed attempt. Bounded enum,
/// never free text. `cascades()` is the ONE cascade authority: exactly the
/// §4 cascade classes (408/429/5xx, typed rate-limit/overload, pre-commit
/// transport) return true; every other class — and every unknown — is
/// terminal (fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingFailureClass {
    // ── Cascading classes (§4 "Cascade to the next admitted candidate only
    // on") ──
    /// HTTP 408 request timeout.
    RequestTimeout,
    /// HTTP 429 or a typed SDK rate-limit.
    RateLimited,
    /// A typed SDK overload signal.
    Overloaded,
    /// HTTP 5xx server error.
    ServerError,
    /// Connection/DNS/TLS/timeout/response-stream transport failure BEFORE
    /// the commit boundary (no response byte observed).
    TransportPreCommit,
    // ── Terminal classes (§4 "Never cascade on") ──
    /// HTTP 400/422 format or parameter rejection.
    FormatRejected,
    /// HTTP 401/403 authentication/authorization (incl. a 5xx/429 whose
    /// body evidence narrows it to auth — conservative precedence).
    Auth,
    /// HTTP 402 billing/entitlement.
    Billing,
    /// HTTP 404 model-not-found: the advertised snapshot is stale; fail
    /// closed, never cascade.
    ModelNotFound,
    /// Context overflow.
    ContextOverflow,
    /// Any failure AFTER the commit boundary: partial SSE frames, partial
    /// tool-call arguments, failure after tool dispatch.
    PostCommit,
    /// Parse/protocol error, malformed or truncated response (including
    /// malformed success bodies).
    Protocol,
    /// Policy/egress denial.
    PolicyDenied,
    /// Pre-dispatch capability rejection (unverified/unsupported param).
    CapabilityRejected,
    /// Cancellation, user interruption, deadline expiry.
    Cancelled,
    /// Unclassifiable — terminal (ambiguous wire states fail closed).
    #[serde(other)]
    Unknown,
}

impl RoutingFailureClass {
    /// The §4 cascade rule. Conservative by construction: only the five
    /// listed classes cascade.
    pub fn cascades(self) -> bool {
        matches!(
            self,
            RoutingFailureClass::RequestTimeout
                | RoutingFailureClass::RateLimited
                | RoutingFailureClass::Overloaded
                | RoutingFailureClass::ServerError
                | RoutingFailureClass::TransportPreCommit
        )
    }
}

/// The per-candidate disposition journaled on a receipt (P5 §4/§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingOutcome {
    /// The attempt emitted (commit boundary crossed) and succeeded; this
    /// candidate is the turn's selected leaf.
    Committed,
    /// The attempt failed with a cascading class; the ladder moved on.
    CascadeFailure,
    /// The attempt failed with a terminal class; the ladder closed.
    TerminalFailure,
    /// Kill-resume reconciliation (§4.1): the attempt was in flight at kill
    /// time — indeterminate, consumed against the budget, NEVER auto-
    /// replayed, and charged the §3.5 conservative estimate (never free).
    ConsumedInflight,
    /// Never dispatched: filtered at construction (see the snapshot) or
    /// rejected at resume (credential lost) — `rejection` carries the reason.
    Rejected,
    #[serde(other)]
    Unknown,
}

/// Provenance of a response-reported actual-leaf identity (P5 §6): receipts
/// record WHERE leaf identity came from so pricing decisions are auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeafProvenance {
    /// The wire carried no usable leaf identity (absent or alias-valued).
    Absent,
    /// A concrete leaf was reported by the successful terminal completion
    /// frame of the bound endpoint (for ALIAS candidates this is
    /// provenance-only evidence — never a mismatch, never priced without
    /// the §6 evidence path).
    ProviderReported,
    /// LEAF candidate only: the reported leaf matched no admitted candidate
    /// in the journaled snapshot — journaled as a mismatch, metered
    /// unknown/unpriced.
    Mismatch,
    #[serde(other)]
    Unknown,
}

/// Per-attempt usage summary journaled on a receipt (P5 §6): numbers and
/// flags only. `reported: false` marks a non-provider-reported charge: the
/// §3.5 conservative estimate (the killed-attempt charge — never zero) or,
/// for a failed attempt whose wire reported nothing (F-P5-3), the honest
/// zero-token/unpriced record (explicit zeros, never fabricated numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Catalog-priced cost in integer microcents against the ACTUAL leaf —
    /// meaningful only when `priced` (unpriced is never a fake $0).
    #[serde(default)]
    pub microcents: u64,
    /// False when no trustworthy actual-leaf price exists (absent/alias/
    /// mismatched identity, missing pricing row, or the alias evidence path
    /// not established) — the P1 honesty flag, AND-accumulated on rollup.
    #[serde(default = "default_priced")]
    pub priced: bool,
    /// True = provider-reported on the wire; false = no wire usage — either
    /// the §3.5 estimate (never zero) or the F-P5-3 honest zero record for a
    /// failed attempt the wire metered nothing for.
    #[serde(default)]
    pub reported: bool,
}

impl RoutingUsage {
    /// Fold into the turn/session usage sum (P5 §6 rollup): provider-reported
    /// usage keeps its provenance; the §3.5 estimate charge is marked
    /// `estimated` with the pinned formula version, never zero. An
    /// UNREPORTED all-zero record (F-P5-3: a failed attempt whose wire
    /// carried no usage) folds to NOTHING — it is an explicit journal
    /// record, not a usage measurement; folding it would stamp the sum
    /// `estimated` (and AND its unpriced flag in) over zero tokens.
    pub fn to_turn_usage(&self) -> TurnUsage {
        let mut sum = TurnUsage::default();
        if !self.reported
            && self.input_tokens == 0
            && self.output_tokens == 0
            && self.microcents == 0
        {
            return sum;
        }
        if self.reported {
            sum.add_provider_reported(
                self.input_tokens,
                self.output_tokens,
                0,
                0,
                self.microcents,
                self.priced,
            );
        } else {
            sum.add_estimated(
                self.input_tokens,
                self.output_tokens,
                self.microcents,
                self.priced,
                ESTIMATION_METHOD_VERSION,
                self.input_tokens.saturating_add(self.output_tokens),
            );
        }
        sum
    }
}

/// Why a routed turn stopped cascading (P5 §4): journaled on the FINAL
/// receipt of an exhausted ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingExhaustion {
    /// The global three-attempt budget ran out with candidates remaining.
    BudgetExhausted,
    /// Every admitted candidate was attempted (or the ladder had none).
    CandidatesExhausted,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    Stop,
    SessionStart,
    SessionEnd,
    PreCompact,
    PostCompact,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOutcome {
    Pass,
    Blocked,
    Failed,
    Timeout,
    BoundedOutput,
    #[serde(other)]
    Unknown,
}

/// Audit-safe terminal state of a checkpoint restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointRestoreOutcome {
    Applied,
    RolledBack,
    Failed,
    #[serde(other)]
    Unknown,
}

// ── S9 CUA (panel-certified design: S9-BROWSER-CUA-DESIGN.md §4) ──────────
// JOURNAL-MIGRATION REVIEW FLAG (rides the SAME coordinated RC2
// journal-migration review as the P1/P2a/P3/P5 additions above): the TWO S9
// additions — `Op::CuaAction` and `Op::CuaResult` — are additive variants;
// `SCHEMA_VERSION` stays 1, pre-S9 journals replay byte-identical, and old
// readers skip both via the `Unknown`-op forward tolerance. The payloads are
// ids, bounded strings, digests, and bounded enums ONLY: coordinates and
// typed text are payload and NEVER reach the journal (the digest-only
// invariant — the `ToolResult.output_digest` precedent, stricter: unlike
// `ToolCall.args`, CUA arguments are digest-only because typed text is
// keystroke payload). Replay folds them context-neutrally except for the
// §4.2 ambiguous-tail rule (an unpaired CuaAction marks the tail
// interrupted — see replay.rs).

/// Terminal outcome of one journaled CUA op (S9 §4.1): reuses
/// [`TurnOutcome`]'s serde discipline (snake_case, closed set, forward
/// tolerance). `denied` covers gate denials, policy rejects, and hook blocks
/// — `error_kind` on the result op disambiguates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuaOutcome {
    Completed,
    Denied,
    Cancelled,
    Failed,
    /// An outcome written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

pub const MAX_CHECKPOINT_ID_CHARS: usize = 128;
pub const WORKSPACE_KEY_HEX_CHARS: usize = 16;

pub fn is_workspace_key(value: &str) -> bool {
    value.len() == WORKSPACE_KEY_HEX_CHARS
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

pub fn validate_checkpoint_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty() || value.chars().count() > MAX_CHECKPOINT_ID_CHARS {
        Err("checkpoint id empty or over the cap")
    } else {
        Ok(())
    }
}

pub fn validate_checkpoint_created(
    checkpoint_id: &str,
    workspace_key: &str,
    parent: Option<&str>,
    tree_digest: &str,
) -> Result<(), &'static str> {
    validate_checkpoint_id(checkpoint_id)?;
    if !is_workspace_key(workspace_key) {
        return Err("workspace key is not canonical 16-hex");
    }
    if let Some(parent) = parent {
        validate_checkpoint_id(parent)?;
    }
    if !is_canonical_digest(tree_digest) {
        return Err("tree digest is not canonical sha256 hex");
    }
    Ok(())
}

pub fn validate_checkpoint_restore_begin(
    checkpoint_id: &str,
    safety_checkpoint_id: &str,
    tree_digest: &str,
) -> Result<(), &'static str> {
    validate_checkpoint_id(checkpoint_id)?;
    validate_checkpoint_id(safety_checkpoint_id)?;
    if !is_canonical_digest(tree_digest) {
        return Err("tree digest is not canonical sha256 hex");
    }
    Ok(())
}

pub fn validate_checkpoint_restore_end(checkpoint_id: &str) -> Result<(), &'static str> {
    validate_checkpoint_id(checkpoint_id)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    SessionBegin {
        session_id: String,
        cwd: String,
    },
    /// Workspace checkpoint metadata only; no paths or file content.
    CheckpointCreated {
        checkpoint_id: String,
        workspace_key: String,
        parent: Option<String>,
        file_count: u32,
        total_bytes: u64,
        tree_digest: String,
        evicted: u32,
    },
    /// Durable phase-one marker written before restore staging/apply.
    CheckpointRestoreBegin {
        checkpoint_id: String,
        safety_checkpoint_id: String,
        file_count: u32,
        tree_digest: String,
    },
    /// Durable restore terminal marker. Recovery writes this with recovered=true.
    CheckpointRestoreEnd {
        checkpoint_id: String,
        outcome: CheckpointRestoreOutcome,
        recovered: bool,
    },
    TurnBegin {
        turn_id: String,
        /// The plain-text PROJECTION of `input_blocks` (placeholders
        /// inline), produced by the §5.2.1 projection function from the
        /// manifest at turn start — one function, one call site, so the two
        /// cannot diverge. Serves display, `replay_frames`, and old readers.
        input: String,
        /// P2a §5.2 — JOURNAL-MIGRATION REVIEW FLAG (addition (a)). The
        /// ORDERED BLOCK MANIFEST: reconstruction walks it in order —
        /// `Text` → `ContentBlock::Text`, `ImageRef` → rehydrated
        /// `ContentBlock::Image` (§5.3). Digests + metadata only, NEVER
        /// bytes. Serde-defaulted: pre-P2a journals replay unchanged
        /// (text-only, byte-identical); omitted when empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        input_blocks: Vec<InputBlock>,
    },
    ToolCall {
        turn_id: String,
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        /// Digest of the output, not the output itself — journals never carry
        /// secret payloads by default.
        output_digest: String,
        changed_files: Vec<String>,
        /// Typed error classification (C7): the KIND only, never raw error
        /// text — the digest-only journal invariant holds, and the
        /// presentation string is re-derivable from the error-code table on
        /// replay. Serde-defaulted: journals written before the field
        /// existed replay unchanged; omitted from serialization when absent
        /// so new journals stay byte-minimal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_kind: Option<crate::error_kind::NanoErrorKind>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_refs: Vec<ImageRef>,
    },
    /// Digest-safe lifecycle-hook audit record. Commands and hook output are
    /// deliberately absent: either may contain operator secrets.
    HookDecision {
        turn_id: String,
        event: HookEvent,
        handler_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        matcher_input: Option<String>,
        outcome: HookOutcome,
        duration_ms: u64,
    },
    /// Assistant-visible reply text for a model step. Unlike tool output this
    /// is content the agent itself produced for the user (it is streamed to
    /// the client live anyway), so journaling it carries no payload the user
    /// has not already seen; it is what lets a restored session rebuild the
    /// assistant side of the conversation.
    AssistantText {
        turn_id: String,
        text: String,
    },
    TurnEnd {
        turn_id: String,
        outcome: TurnOutcome,
        /// P1 §3.4 — JOURNAL-MIGRATION REVIEW FLAG (addition (a) of the RC2
        /// coordinated review trio). The turn-scoped usage SUM across every
        /// `record_usage` call in the turn (explicitly NOT the last
        /// response's usage), with §3.5 estimation provenance — numbers and
        /// bounded enums only, so the digest-only invariant is untouched.
        /// Journaled for EVERY terminal outcome (Completed/Cancelled/Failed):
        /// a cancelled stream that consumed tokens journals those tokens.
        /// Serde-defaulted so pre-P1 journals replay unchanged (the
        /// `ToolResult.error_kind` pattern); omitted when the turn recorded
        /// no usage, keeping new journals byte-minimal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TurnUsage>,
    },
    CompactionBegin {
        compaction_id: String,
    },
    CompactionComplete {
        compaction_id: String,
        summary: String,
        /// Op ids this summary replaces for replay purposes.
        covers_op_ids: Vec<String>,
        /// Durable-effect inventory at compaction time. The summary replaces
        /// the *transcript*; effects must survive or replay diverges.
        changed_files: Vec<String>,
        /// P2a §8 part 2 — JOURNAL-MIGRATION REVIEW FLAG (addition (b)).
        /// Sticky-transitive image provenance: journaled as
        /// `image_influenced_before OR any-image-evicted-this-compaction`.
        /// A summary produced from an image-influenced context is itself
        /// image-influenced (image-derived content can persist in summary
        /// TEXT after pixels are gone), so the §9.1 untrusted-turn clamp can
        /// never be reopened by compaction alone; replay folds the STICKY-OR
        /// over ALL records, never the latest. Serde-defaulted so pre-P2a
        /// journals replay unchanged; omitted when false.
        #[serde(default, skip_serializing_if = "is_false")]
        image_influenced: bool,
        /// P3 §3.3: exact hydration state at the covered watermark, plus
        /// the bounded digest window that keeps churn detection continuous.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mcp_hydration: Option<Vec<HydrationCarryEntry>>,
    },
    CompactionCancel {
        compaction_id: String,
        /// Why the compaction was abandoned. Bounded enum, never free text:
        /// a cancel reason must never become a secondary persistence channel
        /// for the sensitive content that just failed the redaction gate.
        /// Defaults for journals written before the field existed.
        #[serde(default)]
        reason: CompactionCancelReason,
    },
    /// An ACCEPTED todo-list replacement (C10 §2), journaled by the `todo`
    /// tool under journal-first, accepted-only ordering: the item set
    /// validates first, this op lands durably, and only then does the
    /// session's todo cell mutate. CONTENT, not posture: replay folds it
    /// into session state (last-write-wins) so a resumed session restores
    /// the list. Payload policy is the AssistantText class — model-authored,
    /// user-visible, journaled verbatim (a model that writes a secret into a
    /// todo item persists it in plaintext, the same risk class as an
    /// assistant message; documented, user-visible, not a new channel).
    TodoSet {
        items: Vec<TodoItem>,
    },
    /// An ACCEPTED plan-posture transition (C10 §3), journaled by the single
    /// `set_plan_posture` transition every entry/exit path converges on.
    /// Audit history ONLY — the pack-wide rule is "content replays, postures
    /// don't" (C2 Q5 precedent): replay IGNORES this op for activation and
    /// session/load NEVER restores the posture; a resumed session starts
    /// with plan mode off. Older builds read this as `Unknown` and skip.
    PlanSet {
        active: bool,
    },
    /// An ACCEPTED permission-mode change (C2), journaled by the
    /// session/set_mode handler under journal-first, accepted-only ordering:
    /// the id validates first, this op lands durably, and only then does the
    /// session's mode cell mutate. Audit history ONLY — replay treats it as
    /// context-neutral and session/load NEVER restores the mode: every
    /// session starts in `default` and elevated autonomy always requires a
    /// fresh, explicit grant. Older builds read this as `Unknown` and skip.
    ModeSet {
        /// The PermissionMode wire id ("read_only" | "default" | "full_auto").
        mode: String,
    },
    /// A drained mid-turn steer (C9): journaled DURABLY at drain time,
    /// BEFORE the in-memory history mutation, so the journal records what
    /// the model actually saw, in order. Enqueued-but-undrained steers are
    /// never journaled. User text is journaled verbatim (same rule as
    /// `TurnBegin.input`); replay folds it as a user message exactly like
    /// the TurnBegin input fold, so kill-resume reconstructs steer-adjusted
    /// context byte-identically.
    SteerInput {
        turn_id: String,
        text: String,
    },
    /// The one allowed structured-output re-ask (C9 §4.3): the LITERAL
    /// feedback text the model saw, journaled at the moment the feedback
    /// message enters history (journal-first, fail-closed on append
    /// failure). Replay folds it as a user message, so kill-resume
    /// reconstructs the re-asked context byte-identically regardless of
    /// template wording changes across versions. A re-ask is a new
    /// journaled sampling step with its own budget accounting, NOT a retry.
    SchemaReask {
        turn_id: String,
        feedback: String,
    },
    /// Fork lineage marker (C11): the second envelope of every forked child
    /// journal, right after the child genesis `SessionBegin`. It opens the
    /// imported region — the next `imported_ops` envelopes are the parent's
    /// journal through the fork point, copied byte-verbatim. Replay folds the
    /// imported region BY CLASS: context/turn-structure ops fold normally,
    /// control ops (goal/cron) are suppressed from live state into an
    /// audit-only namespace. A declared `imported_ops` count that overruns
    /// the actual stream is a typed replay error (fail-closed).
    ForkedFrom {
        parent_session_id: String,
        /// Id of the last imported parent envelope (the fork point).
        parent_op_id: String,
        /// The turn the fork was taken at (`None` = fork at end).
        at_turn: Option<String>,
        /// SHA-256 (hex) of the parent journal before and after the copy —
        /// the byte-identical-parent proof, asserted equal by the fork.
        parent_digest_before: String,
        parent_digest_after: String,
        /// Number of envelopes in the imported region that follows.
        imported_ops: u64,
    },
    /// Goal activation (C11). One current goal per session; the writer side
    /// rejects a second non-terminal goal and objectives over
    /// [`MAX_GOAL_OBJECTIVE_LEN`].
    GoalBegin {
        goal_id: String,
        objective: String,
        budgets: GoalBudgets,
    },
    /// Every goal transition (C11). Reasons are the bounded [`GoalReason`]
    /// enum, never free text.
    GoalStatus {
        goal_id: String,
        status: GoalStatusKind,
        #[serde(default)]
        reason: GoalReason,
    },
    /// Goal terminal record (C11): `complete`, or terminal `blocked`.
    GoalEnd {
        goal_id: String,
        outcome: GoalOutcome,
    },
    /// Cron fire reservation (C11 §5.4): journaled and flushed BEFORE any
    /// prompt injection or model call — the durable act of firing. Keyed by
    /// `occurrence_id` (`{job_id}:{scheduled_fire_time}` RFC3339 UTC, minute
    /// resolution), stable across restarts/jitter/coalescing, so a durable
    /// reservation can never double-fire. `mode_at_fire` records the capped
    /// `min(session_mode, default)` derivation for after-the-fact audit.
    CronFired {
        job_id: String,
        session_id: String,
        turn_id: String,
        occurrence_id: String,
        /// Permission-mode wire id the fired turn ran under.
        mode_at_fire: String,
        /// Number of missed occurrences this fire coalesces (0 = on time).
        #[serde(default)]
        coalesced: u32,
    },
    /// Cron job creation (C11 §5.5, F-6 closure): the DURABLE ACT of
    /// `cronjob create` — appended and synced BEFORE the `jobs.json` cache
    /// persist (journal-first; the scheduler rebuilds the cache from this
    /// op when the two disagree). Additive; old readers skip via `Unknown`.
    /// The prompt is journal-consistent content (`TurnBegin.input` and
    /// `ToolCall.args` already journal full text verbatim); the payload is
    /// prompt + session + schedule only — nothing that could carry
    /// privilege.
    CronCreated {
        job_id: String,
        session_id: String,
        /// Canonical 5-field crontab (validated BEFORE journaling).
        schedule: String,
        prompt: String,
        /// RFC3339 UTC minute: the coalescing anchor for a never-fired job.
        created_at: String,
    },
    /// Cron job deletion (C11 §5.5, F-6 closure): journaled BEFORE the
    /// cache removal; replay tombstones the job id so a cache that still
    /// carries it (a kill between this append and the cache persist) is
    /// repaired by the scheduler — a deleted job NEVER re-fires.
    CronDeleted {
        job_id: String,
        session_id: String,
    },
    /// An ACCEPTED `/budget continue` grant (P1 §4.3) — JOURNAL-MIGRATION
    /// REVIEW FLAG (addition (b) of the RC2 coordinated review trio).
    /// Journaled DURABLY at acceptance under journal-first, accepted-only
    /// ordering (the C10 `TodoSet` pattern): the command validates, this op
    /// lands, and only then does the session's effective-limit cell mutate;
    /// an append failure leaves the limit unchanged. Numbers and ids only.
    /// Replay folds it into budget state, so a kill-resumed session
    /// reconstructs the exact effective limit (replay-deterministic, never
    /// session-volatile).
    BudgetGrant {
        grant_id: String,
        tokens: u64,
        after_limit: u64,
    },
    /// A C6 child task's usage folded into the PARENT journal at the child's
    /// terminal state (P1 §3.3) — JOURNAL-MIGRATION REVIEW FLAG (addition
    /// (c) of the RC2 coordinated review trio). Journal-first at the
    /// reconciliation boundary: this op lands DURABLY BEFORE the child's
    /// terminal result becomes visible to the parent session
    /// (`task_result`/`task_apply`); append failure fails closed (the
    /// completion is held, never reported while its usage is unjournaled).
    /// Numbers and stable ids only. Replay folds it into meter totals keyed
    /// by `task_id`; envelope-id dedup makes a retried append idempotent,
    /// and the op's presence is the orphan-fold dedup marker at resume.
    ChildUsageRollup {
        task_id: String,
        /// The child's single turn id (`{task_id}-turn-1`, tasks.rs).
        child_turn_id: String,
        outcome: TurnOutcome,
        /// The child's turn-scoped usage sum (§3.4) with §3.5 provenance.
        usage: TurnUsage,
    },
    /// P3 §3.3: one atomic, bounded multi-server hydration decision.
    McpToolHydration {
        hydration_id: String,
        entries: Vec<HydrationEntry>,
    },
    /// P3 §5.6: audit-only bound elicitation decision. Answer content is
    /// deliberately absent; only its canonical digest is durable.
    /// `server_id` is the §2.7 instance id, never the display name (F-P3-3).
    McpElicitation {
        elicitation_id: String,
        server_id: String,
        call_id: String,
        request_id: String,
        card_id: u64,
        action: McpElicitationAction,
        schema_digest: String,
        answer_digest: String,
    },
    /// P3 §6.3: exact, replayable OAuth endpoint authority. `server_id` is
    /// the §2.7 instance id (the credential-storage key), never the display
    /// name (F-P3-3).
    McpOauthGrant {
        grant_id: String,
        server_id: String,
        as_origin: String,
        issuer: String,
        endpoints: Vec<GrantEndpoint>,
    },
    /// P5 §3/§4.1 — JOURNAL-MIGRATION REVIEW FLAG (addition (a) above): the
    /// immutable candidate snapshot for one turn, journaled BEFORE the first
    /// dispatch so replay and kill-resume explain (and replay) the choice
    /// without re-running discovery. Every turn journals exactly one —
    /// pins/implicit passthrough carry a single admitted candidate;
    /// `auto_client_side` carries the ordered ladder. `catalog_digest` is
    /// the sha256 of the catalog/proof inputs the snapshot derived from
    /// (§7: identifiers/digests, never secrets or remote payloads).
    RoutingSnapshot {
        turn_id: String,
        routing_mode: RoutingMode,
        /// The model reference as configured/resolved (e.g. `flux-auto`,
        /// `openai:gpt-5`) — distinct from any response-reported leaf.
        configured_reference: String,
        /// The global physical-attempt budget for the ladder (§4: 3).
        attempt_budget: u32,
        candidates: Vec<RoutingCandidate>,
        catalog_digest: String,
    },
    /// P5 §4.1 — JOURNAL-MIGRATION REVIEW FLAG (addition (b)): the durable
    /// attempt-start marker, journaled per rung BEFORE the dispatch. A begin
    /// without a matching receipt after a kill is the indeterminate
    /// in-flight attempt: consumed against the budget, never auto-replayed.
    RoutingAttemptBegin {
        turn_id: String,
        ordinal: u32,
        routing_mode: RoutingMode,
        provider: String,
        candidate: String,
    },
    /// P5 §4/§6 — JOURNAL-MIGRATION REVIEW FLAG (addition (c)): the
    /// per-candidate receipt at attempt end (or at kill-resume
    /// reconciliation for a consumed in-flight attempt, or at resume-time
    /// credential loss). Ids, numbers, bounded enums, and the
    /// provider-reported leaf id only — never credentials, headers, bodies,
    /// or raw provider errors.
    RoutingReceipt {
        turn_id: String,
        ordinal: u32,
        routing_mode: RoutingMode,
        provider: String,
        configured_reference: String,
        candidate: String,
        outcome: RoutingOutcome,
        /// The §4 classified failure for failure outcomes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<RoutingFailureClass>,
        /// HTTP status where nonsecret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        /// Physical attempts this candidate consumed from the global budget
        /// (>1 only when same-candidate transport retry is retained, §4).
        attempts_consumed: u32,
        /// True on the receipt of the turn's selected (committed) candidate.
        selected: bool,
        /// The provider-reported response model when supplied by the wire —
        /// provenance per `leaf_identity` (§6), never a client prediction.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_model: Option<String>,
        leaf_identity: LeafProvenance,
        /// Per-attempt usage when the provider reported it (or the §3.5
        /// estimate for a consumed in-flight attempt).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<RoutingUsage>,
        /// Set on the final receipt of an exhausted ladder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exhaustion: Option<RoutingExhaustion>,
        /// The bounded rejection reason for `Rejected` outcomes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rejection: Option<CandidateRejection>,
    },
    /// A shell-rules amendment minted by the approval card's
    /// `allow_always_*` selection (P4 §2.6) — JOURNAL-MIGRATION REVIEW FLAG
    /// (rides the SAME coordinated RC2 journal-migration review as the
    /// P1/P2a/P3/P5 additions above): additive variant, `SCHEMA_VERSION`
    /// stays 1, old readers skip it via `Unknown`. AUDIT-ONLY on replay
    /// (the `ModeSet` discipline): the rules themselves are config re-read
    /// from `rules.toml` at session start, never folded from the journal;
    /// replay never trusts this op over the file. The op exists so an
    /// audited session can prove WHEN its prompt surface narrowed and to
    /// exactly what file state. Ordering pinned (§11): file append+rename,
    /// THEN this op, then the in-memory cell swap — a kill between leaves a
    /// rule without its audit op (the SAFE direction).
    ShellRuleAmended {
        /// Idempotence key (`{session_id}-rule-{nanos}-{segment}`).
        amendment_id: String,
        /// The amended argv tokens (≤ 8 tokens, ≤ 128 chars each —
        /// `validate_shell_rule_amended`). Command tokens are
        /// journal-consistent content (`ToolCall.args` already journals the
        /// full command verbatim).
        prefix: Vec<String>,
        /// The end-of-argv anchor state of the minted rule (exact vs
        /// prefix widening).
        exact: bool,
        /// 64-hex sha256 of `rules.toml` AFTER the append.
        rule_digest: String,
    },
    /// S9 §4.1: a computer-use action, journaled BEFORE dispatch (the
    /// `Op::ToolCall`-before-approval precedent: a failed append is
    /// turn-fatal `JournalUnavailable`, never a dropped record). DIGESTS
    /// ONLY: `args_digest` is sha256 of the canonical serialized args —
    /// coordinates and typed text are payload and never journal;
    /// `pre_shot` is the pre-action screenshot's attachment-store digest
    /// (mutating ops; absent otherwise and on pre-dispatch denials).
    /// `op_kind` is the snake_case kind tag; `frontmost_app` is the bounded
    /// app id the approval prompt was issued against (None = unresolved).
    /// Replay is context-neutral except the §4.2 ambiguous-tail rule: an
    /// action without its paired result marks the tail interrupted.
    CuaAction {
        turn_id: String,
        call_id: String,
        op_kind: String,
        args_digest: String,
        /// Serde-defaulted: omitted (byte-minimal) when unresolved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frontmost_app: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_shot: Option<String>,
    },
    /// S9 §4.1: the dispatch-side record, appended after the dispatch
    /// settles (or after a pre-dispatch denial). `error_kind` carries the
    /// closed-vocabulary kind on denial/failure (the `ToolResult.error_kind`
    /// pattern — never raw error text); `post_shot` is the post-action
    /// screenshot's attachment-store digest (mutating ops and the screenshot
    /// op itself).
    CuaResult {
        call_id: String,
        outcome: CuaOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_shot: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_kind: Option<crate::error_kind::NanoErrorKind>,
    },
    /// Forward tolerance: any Op type this build does not know. Skipped on
    /// replay; the raw line stays in the journal for future readers.
    #[serde(other)]
    Unknown,
}

/// One todo-list entry (C10 §2). The status vocabulary adopts the
/// wcore/codex set (`pending`/`in_progress`/`completed`/`cancelled`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
    /// A status written by a newer build this one does not know.
    #[serde(other)]
    Unknown,
}

impl TodoStatus {
    /// The wire/model-facing id.
    pub fn id(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
            TodoStatus::Unknown => "unknown",
        }
    }

    /// Parse a model-supplied status string. Unknown strings are `None` —
    /// the todo tool turns that into a typed validation error (fail-closed),
    /// never a silent coercion.
    pub fn parse(id: &str) -> Option<TodoStatus> {
        match id {
            "pending" => Some(TodoStatus::Pending),
            "in_progress" => Some(TodoStatus::InProgress),
            "completed" => Some(TodoStatus::Completed),
            "cancelled" => Some(TodoStatus::Cancelled),
            _ => None,
        }
    }

    /// Counts toward the open work the status line reports.
    pub fn is_open(self) -> bool {
        matches!(self, TodoStatus::Pending | TodoStatus::InProgress)
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    #[test]
    fn checkpoint_payload_bounds_are_pinned() {
        let digest = "a".repeat(64);
        assert!(validate_checkpoint_created("id", "0123456789abcdef", None, &digest).is_ok());
        assert!(validate_checkpoint_created("id", "0123456789abcdeG", None, &digest).is_err());
        assert!(validate_checkpoint_created("id", "0123456789abcdef", None, "bad").is_err());
        assert!(validate_checkpoint_restore_begin("id", "safety", &digest).is_ok());
        assert!(validate_checkpoint_restore_end("").is_err());
    }

    #[test]
    fn checkpoint_ops_serialize_without_path_or_content_fields() {
        let value = serde_json::to_value(Op::CheckpointCreated {
            checkpoint_id: "id".into(),
            workspace_key: "0123456789abcdef".into(),
            parent: None,
            file_count: 1,
            total_bytes: 2,
            tree_digest: "a".repeat(64),
            evicted: 0,
        })
        .unwrap();
        let object = value.as_object().unwrap();
        for forbidden in ["path", "paths", "content", "manifest", "bytes"] {
            assert!(!object.contains_key(forbidden));
        }
    }
}
