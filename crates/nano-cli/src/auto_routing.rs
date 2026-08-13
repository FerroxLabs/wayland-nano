//! P5 — Flux Auto routing (panel-certified design note
//! `shared/reviews/panel-tui/P5-auto-routing-design.md`, REVISED round 3).
//!
//! Contract anchors:
//! - §1 precedence: explicit session/CLI pin > configured default pin >
//!   explicit Auto opt-in (`NANO_ROUTING_AUTO` / `--auto`, resolved reference
//!   `flux-auto`) > implicit `flux-auto` passthrough. A pin is TERMINAL — it
//!   never falls through to `flux-auto`, the fallback, or the ladder.
//! - §1/§2.2: the no-config default is fail-closed alias passthrough to Flux
//!   ONLY. Client-side routing exists ONLY behind the explicit opt-in and
//!   only for `routing_mode = auto_client_side`.
//! - §3: candidate construction is deterministic and filtered BEFORE any
//!   network attempt (advertised + credentialed + live-proven + capability);
//!   the tool-capability catalog is a hard prerequisite — until it exists,
//!   tool-bearing Auto turns fail closed with the typed capability error.
//! - §4: at most three physical provider attempts per Auto turn, at most one
//!   attempt per candidate; cascade ONLY on 408/429/5xx, typed
//!   rate-limit/overload, and pre-commit transport failures; the first
//!   emitted semantic delta or tool request commits the attempt
//!   (post-commit failure is terminal); classifier conflicts resolve
//!   CONSERVATIVE-wins; unclassifiable is terminal.
//! - §4.1: snapshot + budget are durable before dispatch; attempt start/end
//!   journaled per rung; resume replays the journaled snapshot and NEVER
//!   rediscovers; an in-flight attempt at kill time is consumed (charged
//!   the §3.5 conservative estimate, `reported: false`, never free).
//! - §5: image-bearing turns require exact-leaf `image_in`; tool-bearing
//!   turns require proven tool-use on the exact provider/surface/leaf.
//!   Unknown equals false. The capability-empty refusal is a DISTINCT typed
//!   error from the §1 step-5 no-credential failure.
//! - §6: meter every physical attempt; the actual leaf comes ONLY from the
//!   successful terminal completion frame; absent/alias-valued/mismatched
//!   identity meters `unknown`/unpriced; alias-candidate leaves are
//!   provenance-only unless the documented evidence path holds.
//! - §7: snake_case journal enums; ids/numbers/bounded enums only — never
//!   credentials, headers, bodies, or raw provider errors.
//! - §8: the deterministic test seam injects the candidate snapshot plus
//!   per-candidate transports; it is a hidden surface that cannot alter
//!   production dispatch and never weakens egress or redirects the vendored
//!   catalog (the sole endpoint authority).

use std::sync::{Arc, Mutex};

use nano_agent::turn::ModelDriver;
use nano_model::pricing::PricingCatalog;
use nano_model::types::{
    CallHooks, ContentBlock, Message, ModelError, ModelRequest, ModelResponse, ToolDefinition,
    TransportPhase, Usage,
};
use nano_model::vision_catalog::VisionCatalog;
use nano_session::op::{
    CandidateKind, CandidateRejection, LeafProvenance, Op, OpEnvelope, RoutingCandidate,
    RoutingExhaustion, RoutingFailureClass, RoutingMode, RoutingOutcome, RoutingUsage,
};

use crate::provider_router::ProviderRouter;

/// §4: the global physical-attempt budget for one Auto turn.
pub const ATTEMPT_BUDGET: u32 = 3;

/// §1: the explicit Auto opt-in env var (absent means false — fail-closed).
pub const AUTO_ROUTING_ENV: &str = "NANO_ROUTING_AUTO";
/// §1 step 2: the configured-default channel. Env is the only config
/// channel that exists today; the value is a model reference (bare Flux id
/// or `provider:model`) parsed and validated with typed config errors.
pub const DEFAULT_MODEL_ENV: &str = "NANO_DEFAULT_MODEL";

/// The one reference the Auto opt-in resolves to (§1).
pub const FLUX_AUTO: &str = "flux-auto";

/// §2.1: the four Flux router aliases (never leaf identity on the wire).
pub fn is_flux_alias(id: &str) -> bool {
    matches!(
        id,
        "flux-auto" | "flux-standard" | "flux-fast" | "flux-reasoning"
    )
}

// ═══════════════════════ §1 opt-in control surface ═══════════════════════

/// The parsed process-level routing controls.
#[derive(Debug, Clone, Default)]
pub struct RoutingConfig {
    /// `NANO_ROUTING_AUTO` (absent/empty = false).
    pub auto_opt_in: bool,
    /// `NANO_DEFAULT_MODEL` (absent/empty = None) — the configured default
    /// pin. Validation against the advertised set happens at startup (a
    /// misconfigured default fails loudly, never silently reroutes).
    pub configured_default: Option<String>,
}

/// Parses `NANO_ROUTING_AUTO`: absent/empty → false; `1|true` → true;
/// `0|false` → false; anything else is a typed config error, never a
/// silent default (the `parse_env_u64` discipline).
pub fn parse_auto_opt_in(raw: Option<String>) -> Result<bool, String> {
    match raw {
        None => Ok(false),
        Some(raw) if raw.trim().is_empty() => Ok(false),
        Some(raw) => match raw.trim() {
            "1" | "true" => Ok(true),
            "0" | "false" => Ok(false),
            other => Err(format!(
                "{AUTO_ROUTING_ENV} must be 1|true|0|false, got {other:?}"
            )),
        },
    }
}

/// Parses `NANO_DEFAULT_MODEL`: absent/empty → None; a present value must
/// parse as a model reference (the namespace parser's shape rules) — a
/// malformed value is a typed config error. Advertisement/proven/credential
/// validation happens at startup/binding (terminal pin failures).
pub fn parse_configured_default(raw: Option<String>) -> Result<Option<String>, String> {
    match raw {
        None => Ok(None),
        Some(raw) if raw.trim().is_empty() => Ok(None),
        Some(raw) => {
            let value = raw.trim().to_string();
            crate::provider_router::ProviderRouter::parse_model_id(&value).map_err(|_| {
                format!(
                    "{DEFAULT_MODEL_ENV} must be a model id (bare or provider:model), got {value:?}"
                )
            })?;
            Ok(Some(value))
        }
    }
}

/// How the session's current model reference came to be (§1 precedence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    /// An explicit session/CLI pin (`--model` / `session/set_model`).
    ExplicitPin,
    /// The configured default (`NANO_DEFAULT_MODEL`).
    ConfiguredDefault,
    /// The implicit default (Flux `flux-auto` when credentialed, else the
    /// deterministic proven fallback).
    ImplicitDefault,
}

/// The per-turn routing decision: the journaled `routing_mode` plus the
/// resolved reference the turn is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecision {
    pub mode: RoutingMode,
    pub reference: String,
}

/// §1 precedence, whole rule in one place: explicit pin > configured
/// default pin > explicit Auto opt-in (with the resolved reference
/// `flux-auto`) > implicit alias passthrough. Only `auto_client_side`
/// admits the client-side ladder.
pub fn resolve_routing(source: ModelSource, reference: &str, auto_opt_in: bool) -> RoutingDecision {
    let mode = match source {
        ModelSource::ExplicitPin => RoutingMode::ExplicitAliasPin,
        ModelSource::ConfiguredDefault => RoutingMode::ConfiguredDefaultAlias,
        ModelSource::ImplicitDefault => {
            if auto_opt_in && reference == FLUX_AUTO {
                RoutingMode::AutoClientSide
            } else {
                RoutingMode::ImplicitAliasPassthrough
            }
        }
    };
    RoutingDecision {
        mode,
        reference: reference.to_string(),
    }
}

// ═══════════════════════ §5 capability requirements ═══════════════════════

/// The turn's required capabilities, computed BEFORE candidate selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnRequirements {
    /// Any image in the current prompt OR the assembled context (including
    /// tool-result images).
    pub images: bool,
    /// Any tool advertised (built-in or MCP) OR any tool interaction in the
    /// context (tool continuation turns).
    pub tools: bool,
}

fn blocks_have_image(blocks: &[ContentBlock]) -> bool {
    blocks.iter().any(|block| {
        matches!(block, ContentBlock::Image { .. })
            || matches!(block, ContentBlock::ToolResult { images, .. } if !images.is_empty())
    })
}

fn blocks_have_tool_interaction(blocks: &[ContentBlock]) -> bool {
    blocks.iter().any(|block| {
        matches!(
            block,
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
        )
    })
}

/// §5/§3: requirement detection. `tool_definitions` is the EXACT advertised
/// surface; the context is inspected for tool continuations and carried
/// images separately, so an empty tool list with a tool-result history is
/// still tool-bearing.
pub fn requirements_of(
    input_blocks: &[ContentBlock],
    context: &[Message],
    tool_definitions: &[ToolDefinition],
) -> TurnRequirements {
    TurnRequirements {
        images: blocks_have_image(input_blocks)
            || context
                .iter()
                .any(|m| blocks_have_image(m.content.as_slice())),
        tools: !tool_definitions.is_empty()
            || blocks_have_tool_interaction(input_blocks)
            || context
                .iter()
                .any(|m| blocks_have_tool_interaction(m.content.as_slice())),
    }
}

/// §5: the exact leaf-and-wire tool-capability catalog interface. UNKNOWN
/// EQUALS FALSE — the production v1 catalog is empty (the real catalog is a
/// hard prerequisite tracked by the design's §3/§9), so every tool-bearing
/// Auto turn fails closed until it lands. The §8 seam injects proof maps.
pub trait ToolCapabilityCatalog {
    /// Proven tool-use support on the exact provider/leaf. Absent ⇒ false.
    fn tool_use_proven(&self, provider: &str, leaf: &str) -> bool;
}

/// The production v1 posture: nothing is proven.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyToolCapabilityCatalog;

impl ToolCapabilityCatalog for EmptyToolCapabilityCatalog {
    fn tool_use_proven(&self, _provider: &str, _leaf: &str) -> bool {
        false
    }
}

/// Closure-friendly seam form (tests inject proof maps through this).
impl<F> ToolCapabilityCatalog for F
where
    F: Fn(&str, &str) -> bool,
{
    fn tool_use_proven(&self, provider: &str, leaf: &str) -> bool {
        self(provider, leaf)
    }
}

// ═══════════════════════ §3 candidate construction ═══════════════════════

/// One admitted candidate in dispatch order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedCandidate {
    pub ordinal: u32,
    /// The catalog provider id (Flux alias/leaf rungs: `flux-router`).
    pub provider_id: String,
    /// The bare id sent on the wire (alias or leaf).
    pub candidate: String,
    /// The configured reference form `resolve_binding` consumes (bare for
    /// Flux, `provider:model` for direct providers).
    pub reference: String,
    pub kind: CandidateKind,
}

/// The construction result: the journaled candidate list (ALL candidates,
/// with admission verdicts) plus the admitted subset in dispatch order.
#[derive(Debug, Clone, Default)]
pub struct CandidatePlan {
    pub candidates: Vec<RoutingCandidate>,
    pub admitted: Vec<AdmittedCandidate>,
}

/// The inputs candidate construction filters against (§3: "an immutable
/// ordered candidate snapshot from the vendored provider catalog, live
/// model advertisements, currently resolved credentials, capability proof
/// catalogs, and the proven set"). Filtering precedes network attempts.
pub struct CandidateInputs<'a> {
    pub router: &'a ProviderRouter,
    pub get_env: &'a dyn Fn(&str) -> Option<String>,
    pub now_unix_secs: u64,
    /// Flux credential resolves right now.
    pub flux_credentialed: bool,
    /// Bare Flux ids the live catalog/fixture advertised this process.
    pub flux_advertised: &'a [String],
    pub vision: &'a VisionCatalog,
    pub tools: &'a dyn ToolCapabilityCatalog,
    /// The panel-approved leaf manifest (configured order): bare Flux leaf
    /// ids (rung 2) and namespaced `provider:model` ids (rung 3). V1
    /// production passes an EMPTY list — panel question Q3 (manifest
    /// authority) is unresolved, so no leaf rungs exist outside the §8
    /// seam. The alias rung is NOT part of this manifest.
    pub approved_leaves: &'a [String],
    pub requirements: TurnRequirements,
}

impl CandidateInputs<'_> {
    /// §5 admission for one candidate: the turn's required capabilities must
    /// be proven for the exact provider/leaf. Unknown equals false.
    fn capability_rejection(
        &self,
        provider: &str,
        leaf: &str,
        vision_key: &str,
    ) -> Option<CandidateRejection> {
        if self.requirements.images && !self.vision.image_in(vision_key) {
            return Some(CandidateRejection::CapabilityUnproven);
        }
        if self.requirements.tools && !self.tools.tool_use_proven(provider, leaf) {
            return Some(CandidateRejection::CapabilityUnproven);
        }
        None
    }
}

/// §3 construction: deterministic order — (1) the `flux-auto` alias rung,
/// (2) panel-approved pinned Flux leaves in configured order, (3)
/// panel-approved direct-provider leaves in provider-catalog order, then
/// advertised-model order. Rejected candidates stay in the journaled list
/// with their bounded rejection reason (receipt completeness).
pub fn construct_candidates(inputs: &CandidateInputs<'_>) -> CandidatePlan {
    let mut plan = CandidatePlan::default();
    let push = |plan: &mut CandidatePlan,
                provider: &str,
                candidate: &str,
                reference: String,
                kind: CandidateKind,
                rejection: Option<CandidateRejection>| {
        let admitted = rejection.is_none();
        plan.candidates.push(RoutingCandidate {
            provider: provider.to_string(),
            candidate: candidate.to_string(),
            kind,
            admitted,
            rejection,
        });
        if admitted {
            let ordinal = plan.admitted.len() as u32;
            plan.admitted.push(AdmittedCandidate {
                ordinal,
                provider_id: provider.to_string(),
                candidate: candidate.to_string(),
                reference,
                kind,
            });
        }
    };

    // ── Rung 1: the Flux router alias passthrough. Admitted when Flux is
    // credentialed AND the turn's required capabilities can be proven for
    // alias routing (§3) — tool/image capability on an alias is unprovable
    // until the capability catalogs bless it.
    let alias_rejection = if !inputs.flux_credentialed {
        Some(CandidateRejection::ProviderUncredentialed)
    } else {
        let flux = nano_model::provider_catalog::flux_router();
        inputs.capability_rejection(flux.id, FLUX_AUTO, FLUX_AUTO)
    };
    push(
        &mut plan,
        nano_model::provider_catalog::flux_router().id,
        FLUX_AUTO,
        FLUX_AUTO.to_string(),
        CandidateKind::Alias,
        alias_rejection,
    );

    // ── Rung 2: panel-approved pinned Flux leaves in configured order.
    for leaf in inputs.approved_leaves.iter().filter(|id| !id.contains(':')) {
        let rejection = if !inputs.flux_advertised.iter().any(|m| m == leaf) {
            Some(CandidateRejection::NotAdvertised)
        } else if !inputs.flux_credentialed {
            Some(CandidateRejection::ProviderUncredentialed)
        } else {
            let flux = nano_model::provider_catalog::flux_router();
            inputs.capability_rejection(flux.id, leaf, leaf)
        };
        push(
            &mut plan,
            nano_model::provider_catalog::flux_router().id,
            leaf,
            leaf.clone(),
            CandidateKind::Leaf,
            rejection,
        );
    }

    // ── Rung 3: panel-approved direct-provider leaves in provider-catalog
    // order, then advertised-model order (NOT manifest order — the
    // deterministic tie-break, §3).
    let mut namespaced: Vec<&String> = inputs
        .approved_leaves
        .iter()
        .filter(|id| id.contains(':'))
        .collect();
    namespaced.sort_by_key(|id| {
        let (provider, model) = id.split_once(':').expect("namespaced");
        let provider_pos = nano_model::provider_catalog::PROVIDERS
            .iter()
            .position(|spec| spec.id == provider)
            .unwrap_or(usize::MAX);
        let model_pos = inputs
            .router
            .providers()
            .iter()
            .find(|p| p.spec.id == provider)
            .and_then(|p| p.models.iter().position(|m| m == model))
            .unwrap_or(usize::MAX);
        (provider_pos, model_pos, (*id).clone())
    });
    for id in namespaced {
        let (provider, model) = id.split_once(':').expect("namespaced");
        let rejection = match nano_model::provider_catalog::provider_by_id(provider) {
            None => Some(CandidateRejection::NotAdvertised),
            Some(_) if !inputs.router.is_advertised(provider, model) => {
                Some(CandidateRejection::NotAdvertised)
            }
            Some(spec) if !inputs.router.is_provider_proven(spec.id) => {
                Some(CandidateRejection::ProviderUnproven)
            }
            Some(spec) => {
                match crate::provider_key::resolve_credential(
                    spec,
                    inputs.get_env,
                    inputs.now_unix_secs,
                ) {
                    crate::provider_key::CredentialResolution::Resolved(_) => {
                        inputs.capability_rejection(provider, model, id)
                    }
                    crate::provider_key::CredentialResolution::ExpiredBearer
                    | crate::provider_key::CredentialResolution::Absent => {
                        Some(CandidateRejection::ProviderUncredentialed)
                    }
                }
            }
        };
        push(
            &mut plan,
            provider,
            model,
            id.clone(),
            CandidateKind::Leaf,
            rejection,
        );
    }
    plan
}

/// §7: the snapshot's catalog/proof digest — a sha256 over the exact
/// evaluation inputs (catalog provider rows incl. proven flags, the
/// validated payload advertisement, credential PRESENCE (booleans, never
/// secrets), the approved manifest, the requirements, and the per-candidate
/// capability decisions). Replay/audit can detect input drift; no secrets
/// or remote payloads are stored.
pub fn snapshot_digest(inputs: &CandidateInputs<'_>, plan: &CandidatePlan) -> String {
    use sha2::Digest;
    let mut canonical = String::new();
    for spec in nano_model::provider_catalog::PROVIDERS {
        canonical.push_str(&format!(
            "catalog|{}|{}|{}|{}\n",
            spec.id, spec.proven, spec.base_url, spec.api_path
        ));
    }
    for provider in inputs.router.providers() {
        canonical.push_str(&format!(
            "payload|{}|{}\n",
            provider.spec.id,
            provider.models.join(",")
        ));
    }
    canonical.push_str(&format!("flux_credentialed|{}\n", inputs.flux_credentialed));
    canonical.push_str(&format!(
        "flux_advertised|{}\n",
        inputs.flux_advertised.join(",")
    ));
    canonical.push_str(&format!("approved|{}\n", inputs.approved_leaves.join(",")));
    canonical.push_str(&format!(
        "requirements|images={}|tools={}\n",
        inputs.requirements.images, inputs.requirements.tools
    ));
    for candidate in &plan.candidates {
        let vision_key = match candidate.kind {
            CandidateKind::Leaf if candidate.provider != "flux-router" => {
                format!("{}:{}", candidate.provider, candidate.candidate)
            }
            _ => candidate.candidate.clone(),
        };
        canonical.push_str(&format!(
            "candidate|{}|{}|vision={}:{}|tools={}\n",
            candidate.provider,
            candidate.candidate,
            vision_key,
            inputs.vision.image_in(&vision_key),
            inputs
                .tools
                .tool_use_proven(&candidate.provider, &candidate.candidate),
        ));
    }
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ═══════════════════════ §4 classifier ═══════════════════════

/// The independent signals available for one failed attempt (§4 classifier
/// precedence): each is classified on its own, then the CONSERVATIVE
/// (terminal) classification wins — body/SDK evidence may narrow a
/// cascading status to terminal, never promote terminal to cascading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailureSignals {
    /// The HTTP status, when the failure carried one.
    pub status: Option<u16>,
    /// The typed SDK error class.
    pub sdk: Option<RoutingFailureClass>,
    /// Response-body evidence (e.g. `error.type == "auth_error"`).
    pub body: Option<RoutingFailureClass>,
}

/// The status-signal classification (§4 lists): 408/429/5xx cascade;
/// 400/422, 401/403, 402, 404, 413(context) are terminal; anything else is
/// unclassifiable → terminal Unknown.
pub fn classify_status_signal(status: u16) -> RoutingFailureClass {
    match status {
        408 => RoutingFailureClass::RequestTimeout,
        429 => RoutingFailureClass::RateLimited,
        401 | 403 => RoutingFailureClass::Auth,
        402 => RoutingFailureClass::Billing,
        404 => RoutingFailureClass::ModelNotFound,
        400 | 422 => RoutingFailureClass::FormatRejected,
        413 => RoutingFailureClass::ContextOverflow,
        s if s >= 500 => RoutingFailureClass::ServerError,
        _ => RoutingFailureClass::Unknown,
    }
}

/// §4 classifier precedence: classify each signal independently; terminal
/// beats cascading (conservative wins); ties resolve body > sdk > status
/// for terminals (narrower evidence first) and status > sdk > body for
/// cascades (the wire fact first). No signals at all is Unknown (terminal).
pub fn classify_attempt(signals: &FailureSignals) -> RoutingFailureClass {
    let status_class = signals.status.map(classify_status_signal);
    let by_priority = [signals.body, signals.sdk, status_class];
    if let Some(terminal) = by_priority
        .iter()
        .flatten()
        .find(|class| !class.cascades() && **class != RoutingFailureClass::Unknown)
    {
        return *terminal;
    }
    // No terminal class: cascade classes in wire-fact order, else Unknown.
    for class in [status_class, signals.sdk, signals.body]
        .into_iter()
        .flatten()
    {
        if class.cascades() {
            return class;
        }
    }
    RoutingFailureClass::Unknown
}

/// Production signal extraction: the typed `ModelError` carries the
/// typed-SDK class and (where present) the HTTP status; body evidence is
/// already folded into the variant by the adapters' `classify_status`
/// (e.g. 500 + `auth_error` body ⇒ `ModelError::Auth`).
pub fn signals_of_model_error(err: &ModelError) -> FailureSignals {
    match err {
        ModelError::Auth { status, .. } => FailureSignals {
            status: *status,
            sdk: Some(RoutingFailureClass::Auth),
            body: None,
        },
        ModelError::RateLimited { .. } => FailureSignals {
            status: Some(429),
            sdk: Some(RoutingFailureClass::RateLimited),
            body: None,
        },
        ModelError::ContextOverflow(_) => FailureSignals {
            sdk: Some(RoutingFailureClass::ContextOverflow),
            ..FailureSignals::default()
        },
        ModelError::Entitlement(_) => FailureSignals {
            status: Some(402),
            sdk: Some(RoutingFailureClass::Billing),
            body: None,
        },
        ModelError::Server { status, .. } => FailureSignals {
            status: Some(*status),
            sdk: None,
            body: None,
        },
        ModelError::Transport { phase, .. } => FailureSignals {
            sdk: Some(match phase {
                TransportPhase::Connect | TransportPhase::Tls | TransportPhase::BeforeFirstByte => {
                    RoutingFailureClass::TransportPreCommit
                }
                // §4 commit boundary: failure after response bytes started
                // (partial SSE frames) is POST-COMMIT — terminal.
                TransportPhase::MidStream => RoutingFailureClass::PostCommit,
            }),
            ..FailureSignals::default()
        },
        ModelError::Protocol(_) | ModelError::OutputSchema(_) => FailureSignals {
            sdk: Some(RoutingFailureClass::Protocol),
            ..FailureSignals::default()
        },
        ModelError::UnsupportedParam { .. } => FailureSignals {
            sdk: Some(RoutingFailureClass::CapabilityRejected),
            ..FailureSignals::default()
        },
        ModelError::Cancelled => FailureSignals {
            sdk: Some(RoutingFailureClass::Cancelled),
            ..FailureSignals::default()
        },
        ModelError::Egress(_) => FailureSignals {
            sdk: Some(RoutingFailureClass::PolicyDenied),
            ..FailureSignals::default()
        },
    }
}

/// The HTTP status a classified `ModelError` carried (journaled where
/// nonsecret, §4 receipt fields).
pub fn status_of_model_error(err: &ModelError) -> Option<u16> {
    match err {
        ModelError::Auth { status, .. } => *status,
        ModelError::RateLimited { .. } => Some(429),
        ModelError::Entitlement(_) => Some(402),
        ModelError::Server { status, .. } => Some(*status),
        _ => None,
    }
}

/// Reconstruct a representative `ModelError` from a journaled failure class
/// (kill-resume §4.1: "resume reports the journaled exhaustion outcome" —
/// the surfaced typed error derives from the journaled class, never from
/// fresh network state). Messages are static and secret-free.
pub fn model_error_of_failure_class(class: RoutingFailureClass, status: Option<u16>) -> ModelError {
    match class {
        RoutingFailureClass::RateLimited => ModelError::RateLimited {
            retry_after_ms: None,
        },
        RoutingFailureClass::Overloaded => ModelError::Server {
            status: status.unwrap_or(503),
            message: "overloaded (journaled)".to_string(),
        },
        RoutingFailureClass::ServerError => ModelError::Server {
            status: status.unwrap_or(500),
            message: "server error (journaled)".to_string(),
        },
        RoutingFailureClass::RequestTimeout => ModelError::Server {
            status: 408,
            message: "request timeout (journaled)".to_string(),
        },
        RoutingFailureClass::TransportPreCommit => ModelError::Transport {
            phase: TransportPhase::BeforeFirstByte,
            message: "transport failure (journaled)".to_string(),
        },
        RoutingFailureClass::Auth => ModelError::Auth {
            message: "auth failure (journaled)".to_string(),
            status,
        },
        RoutingFailureClass::Billing => ModelError::Entitlement("billing (journaled)".to_string()),
        RoutingFailureClass::ModelNotFound => ModelError::Server {
            status: 404,
            message: "model not found (journaled)".to_string(),
        },
        RoutingFailureClass::ContextOverflow => {
            ModelError::ContextOverflow("context overflow (journaled)".to_string())
        }
        RoutingFailureClass::FormatRejected => ModelError::Server {
            status: status.unwrap_or(400),
            message: "format rejected (journaled)".to_string(),
        },
        RoutingFailureClass::PostCommit => ModelError::Transport {
            phase: TransportPhase::MidStream,
            message: "post-commit failure (journaled)".to_string(),
        },
        RoutingFailureClass::Protocol => ModelError::Protocol("protocol (journaled)".to_string()),
        RoutingFailureClass::PolicyDenied => {
            ModelError::Protocol("policy/egress denial during routing (journaled)".to_string())
        }
        RoutingFailureClass::CapabilityRejected => {
            ModelError::Protocol("capability rejection during routing (journaled)".to_string())
        }
        RoutingFailureClass::Cancelled => ModelError::Cancelled,
        RoutingFailureClass::Unknown => {
            ModelError::Protocol("unclassified routing failure (journaled)".to_string())
        }
    }
}

// ═══════════════════════ §6 metering ═══════════════════════

/// The per-attempt metering decision (§6): leaf-identity provenance plus
/// the priced/unpriced usage record for the receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptMetering {
    pub provenance: LeafProvenance,
    /// The provider-reported response model, journaled as evidence when the
    /// wire carried one (including alias-valued and mismatched values).
    pub response_model: Option<String>,
    pub usage: RoutingUsage,
}

/// §6 normalization: strip ONLY the responding provider's own namespace
/// prefix; anything else bearing a colon is a mismatch (never fuzzy
/// matching).
pub fn normalize_response_model<'a>(provider: &str, response_model: &'a str) -> Option<&'a str> {
    match response_model.split_once(':') {
        None => Some(response_model),
        Some((prefix, leaf)) if prefix == provider && !leaf.is_empty() => Some(leaf),
        Some(_) => None,
    }
}

/// §6 leaf-identity trust for a SUCCESSFUL attempt. `admitted_leaves` are
/// the journaled snapshot's admitted leaf ids (bare). `alias_identity_trusted`
/// is the documented evidence path (§8.2 leg-1 live proof) — false in v1.
///
/// - absent or alias-valued response model ⇒ `Absent`, unpriced;
/// - LEAF candidate: a normalized leaf matching no admitted candidate ⇒
///   `Mismatch`, unpriced; a match ⇒ `ProviderReported`, priced against the
///   ATTEMPT's provider + reported leaf when a pricing row exists;
/// - ALIAS candidate: a concrete reported leaf cannot match the snapshot by
///   construction ⇒ `ProviderReported` provenance-ONLY, unpriced, UNLESS
///   `alias_identity_trusted` AND the pricing catalog carries the exact row.
pub fn meter_success(
    kind: CandidateKind,
    provider: &str,
    admitted_leaves: &[String],
    response_model: Option<&str>,
    usage: &Usage,
    alias_identity_trusted: bool,
    pricing: Option<&PricingCatalog>,
) -> AttemptMetering {
    let reported = response_model
        .and_then(|m| normalize_response_model(provider, m))
        .filter(|m| !is_flux_alias(m));
    let unpriced = |provenance: LeafProvenance, response_model: Option<String>| AttemptMetering {
        provenance,
        response_model,
        usage: RoutingUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            microcents: 0,
            priced: false,
            reported: true,
        },
    };
    let Some(leaf) = reported else {
        return unpriced(LeafProvenance::Absent, response_model.map(str::to_string));
    };
    let price_key = match kind {
        CandidateKind::Leaf => {
            if admitted_leaves.iter().any(|admitted| admitted == leaf) {
                Some(leaf.to_string())
            } else {
                return unpriced(LeafProvenance::Mismatch, Some(leaf.to_string()));
            }
        }
        CandidateKind::Alias => {
            // Provenance-only unless the §6 evidence path holds AND the
            // exact leaf has a pricing row.
            let row_exists = pricing
                .and_then(|catalog| catalog.get(provider, leaf).ok())
                .is_some();
            if alias_identity_trusted && row_exists {
                Some(leaf.to_string())
            } else {
                None
            }
        }
        CandidateKind::Unknown => None,
    };
    let Some(price_leaf) = price_key else {
        return unpriced(LeafProvenance::ProviderReported, Some(leaf.to_string()));
    };
    let (microcents, priced) = pricing
        .and_then(|catalog| {
            catalog
                .estimate_cost_with_cache_status(
                    provider,
                    &price_leaf,
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cached_input_tokens.unwrap_or(0),
                    0,
                )
                .ok()
        })
        .map(|status| (status.microcents, status.priced))
        .unwrap_or((0, false));
    AttemptMetering {
        provenance: LeafProvenance::ProviderReported,
        response_model: Some(leaf.to_string()),
        usage: RoutingUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            microcents,
            priced,
            reported: true,
        },
    }
}

/// §4.1/§3.5: the conservative estimate charged for a consumed in-flight
/// attempt with NO journaled usage frame — `reported: false`, NEVER zero,
/// unpriced (no trustworthy leaf identity exists for a killed attempt).
/// Input is estimated from the journaled turn input (chars/4, floored at
/// 1); the output charge matches the input estimate (a response in flight
/// had unknown length; symmetric is the conservative documented floor when
/// no session median is available at reconciliation time).
pub fn consumed_inflight_estimate(turn_input: &str) -> RoutingUsage {
    let input = (turn_input.chars().count() as u64 / 4).max(1);
    RoutingUsage {
        input_tokens: input,
        output_tokens: input,
        microcents: 0,
        priced: false,
        reported: false,
    }
}

// ═══════════════════════ journal seam (§4/§7) ═══════════════════════

/// The ladder's journal seam: the host appends through the session's ONE
/// coordinator; tests collect. `false` = the durable append failed — the
/// ladder fails CLOSED (an attempt never dispatches behind an unjournaled
/// begin; a response never becomes live behind an unjournaled receipt).
pub trait RoutingSink: Send + Sync + std::fmt::Debug {
    fn append(&self, envelope: &OpEnvelope) -> bool;
}

/// Production sink: the session's JournalCoordinator (P3 §3.3 append
/// authority).
#[derive(Debug, Clone)]
pub struct CoordinatorRoutingSink(pub Arc<nano_session::JournalCoordinator>);

impl RoutingSink for CoordinatorRoutingSink {
    fn append(&self, envelope: &OpEnvelope) -> bool {
        match self.0.append(envelope) {
            Ok(_) => true,
            Err(err) => {
                eprintln!("wayland-nano: routing journal append failed: {err}");
                false
            }
        }
    }
}

fn routing_envelope(turn_id: &str, suffix: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(format!("{turn_id}-routing-{suffix}"), "now", op)
}

/// The journaled snapshot op for one turn (§3/§4.1: durable BEFORE the
/// first dispatch). Returns false on append failure (fail-closed).
pub fn journal_snapshot(
    sink: &dyn RoutingSink,
    turn_id: &str,
    mode: RoutingMode,
    configured_reference: &str,
    candidates: Vec<RoutingCandidate>,
    catalog_digest: String,
) -> bool {
    sink.append(&routing_envelope(
        turn_id,
        "snapshot",
        Op::RoutingSnapshot {
            turn_id: turn_id.to_string(),
            routing_mode: mode,
            configured_reference: configured_reference.to_string(),
            attempt_budget: ATTEMPT_BUDGET,
            candidates,
            catalog_digest,
        },
    ))
}

/// The singleton snapshot for pin/implicit turns (§8.1 routing-mode
/// separation: every mode journals its `routing_mode`). The single admitted
/// candidate is the resolved reference itself; budget 1 (a pin gets exactly
/// one dispatch — failure is terminal).
pub fn pin_snapshot_candidates(reference: &str) -> Vec<RoutingCandidate> {
    let (provider, candidate, kind) = match reference.split_once(':') {
        Some((provider, model)) => (provider.to_string(), model.to_string(), CandidateKind::Leaf),
        None => (
            nano_model::provider_catalog::flux_router().id.to_string(),
            reference.to_string(),
            if is_flux_alias(reference) {
                CandidateKind::Alias
            } else {
                CandidateKind::Leaf
            },
        ),
    };
    vec![RoutingCandidate {
        provider,
        candidate,
        kind,
        admitted: true,
        rejection: None,
    }]
}

/// §7: the digest for a pin/implicit turn's singleton snapshot — a sha256
/// over the catalog provider rows (incl. proven flags), the validated
/// payload advertisement, and the resolved reference. Same discipline as
/// [`snapshot_digest`]: identifiers and digests, never secrets.
pub fn pin_snapshot_digest(router: &ProviderRouter, reference: &str) -> String {
    use sha2::Digest;
    let mut canonical = String::new();
    for spec in nano_model::provider_catalog::PROVIDERS {
        canonical.push_str(&format!(
            "catalog|{}|{}|{}|{}\n",
            spec.id, spec.proven, spec.base_url, spec.api_path
        ));
    }
    for provider in router.providers() {
        canonical.push_str(&format!(
            "payload|{}|{}\n",
            provider.spec.id,
            provider.models.join(",")
        ));
    }
    canonical.push_str(&format!("reference|{reference}\n"));
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ═══════════════════════ the ladder (§4) ═══════════════════════

/// One ROUTING attempt's outcome (§8 seam): the result plus the PHYSICAL
/// attempt count it consumed (>1 only when same-candidate transport retry
/// is retained — the §4 budget accounting stays exact) and any
/// provider-reported usage on a FAILED attempt (§6: failover never makes a
/// consumed attempt free).
#[derive(Debug)]
pub struct AttemptOutcome {
    pub result: Result<ModelResponse, ModelError>,
    pub attempts_consumed: u32,
    pub failed_usage: Option<Usage>,
}

/// The deterministic test seam (§8 build prerequisite): the per-candidate
/// transport the ladder drives. Production candidates adapt a `ModelDriver`
/// (single-attempt retry posture, so every physical attempt is visible to
/// the budget); seam tests inject scripted or loopback-mock transports.
/// This trait is a hidden test/ladder surface — it cannot alter production
/// dispatch (the host builds production candidates only from validated
/// catalog bindings) and never weakens egress.
pub trait CandidateTransport: Send + Sync + std::fmt::Debug {
    fn attempt<'a>(
        &'a self,
        request: &'a ModelRequest,
        hooks: &'a CallHooks<'a>,
    ) -> impl std::future::Future<Output = AttemptOutcome> + Send + 'a;
}

/// Production adapter: one routing attempt = exactly one physical attempt
/// (the candidate's driver was built with `RetryConfig::single_attempt`).
#[derive(Debug)]
pub struct DriverTransport<D>(pub D);

impl<D: ModelDriver> CandidateTransport for DriverTransport<D> {
    async fn attempt(&self, request: &ModelRequest, hooks: &CallHooks<'_>) -> AttemptOutcome {
        AttemptOutcome {
            result: self.0.complete_observed(request, hooks).await,
            attempts_consumed: 1,
            failed_usage: None,
        }
    }
}

/// One dispatch-ready ladder candidate: the admitted plan entry plus its
/// transport.
#[derive(Debug)]
pub struct LadderCandidate<T> {
    pub plan: AdmittedCandidate,
    pub transport: T,
}

/// The §4 failover ladder: ordered admitted candidates, the global
/// three-attempt budget, the commit boundary, conservative classification,
/// and journaled begin/receipt per rung. Once a candidate emits (a
/// successful response), the ladder LATCHES — the rest of the turn rides
/// the selected leaf (a later failure is post-commit: terminal for routing,
/// never a cascade).
pub struct Ladder<T: CandidateTransport> {
    turn_id: String,
    routing_mode: RoutingMode,
    configured_reference: String,
    candidates: Vec<LadderCandidate<T>>,
    sink: Arc<dyn RoutingSink>,
    pricing: Option<Arc<PricingCatalog>>,
    /// §6 evidence path flag (§8.2 leg-1): false in v1 — alias-passthrough
    /// turns carry no pricing attribution.
    alias_identity_trusted: bool,
    budget: Mutex<u32>,
    cursor: Mutex<usize>,
    latched: Mutex<Option<usize>>,
}

impl<T: CandidateTransport> std::fmt::Debug for Ladder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ladder")
            .field("turn_id", &self.turn_id)
            .field("routing_mode", &self.routing_mode)
            .field("candidates", &self.candidates.len())
            .finish()
    }
}

/// A journal-append failure behind the ladder (fail-closed). Mapped to
/// ModelError::Protocol — typed, terminal, and never cascaded; the C7
/// presentation reads as a protocol-class failure. (The engine's own
/// journal-unavailable path lives above the driver boundary.)
fn journal_error() -> ModelError {
    ModelError::Protocol("routing journal append failed (fail-closed)".to_string())
}

impl<T: CandidateTransport> Ladder<T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        turn_id: &str,
        routing_mode: RoutingMode,
        configured_reference: &str,
        candidates: Vec<LadderCandidate<T>>,
        sink: Arc<dyn RoutingSink>,
        pricing: Option<Arc<PricingCatalog>>,
        alias_identity_trusted: bool,
        budget: u32,
        cursor: usize,
    ) -> Self {
        Self {
            turn_id: turn_id.to_string(),
            routing_mode,
            configured_reference: configured_reference.to_string(),
            candidates,
            sink,
            pricing,
            alias_identity_trusted,
            budget: Mutex::new(budget),
            cursor: Mutex::new(cursor),
            latched: Mutex::new(None),
        }
    }

    /// The admitted leaf ids of this ladder (the §6 snapshot-match set).
    fn admitted_leaves(&self) -> Vec<String> {
        self.candidates
            .iter()
            .filter(|c| c.plan.kind == CandidateKind::Leaf)
            .map(|c| c.plan.candidate.clone())
            .collect()
    }

    fn journal(&self, suffix: &str, op: Op) -> Result<(), ModelError> {
        if self
            .sink
            .append(&routing_envelope(&self.turn_id, suffix, op))
        {
            Ok(())
        } else {
            Err(journal_error())
        }
    }

    /// The latched candidate index (the committed selection), if any.
    pub fn latched(&self) -> Option<usize> {
        *self.latched.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The latched candidate's transport (post-commit calls ride it).
    pub fn latched_transport(&self) -> Option<&T> {
        self.latched()
            .map(|index| &self.candidates[index].transport)
    }

    /// §4: run the ladder for one model call. Pre-commit: cascade on the
    /// listed classes while budget and candidates remain. The first
    /// success commits and latches. Terminal classes (incl. post-commit
    /// transport/protocol failures, auth, billing, 404, cancel, unknown)
    /// close the ladder immediately — zero calls to later candidates.
    pub async fn complete_observed(
        &self,
        request: &ModelRequest,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        if let Some(index) = self.latched() {
            return self.candidates[index]
                .transport
                .attempt(request, hooks)
                .await
                .result;
        }
        loop {
            if hooks.is_cancelled() {
                return Err(ModelError::Cancelled);
            }
            let index = *self.cursor.lock().unwrap_or_else(|p| p.into_inner());
            let remaining = *self.budget.lock().unwrap_or_else(|p| p.into_inner());
            if index >= self.candidates.len() || remaining == 0 {
                // Unreachable from the host paths (they refuse empty
                // candidate sets and exhausted resumes up front) — defensive
                // fail-closed.
                return Err(ModelError::Protocol(
                    "auto routing ladder exhausted before dispatch".to_string(),
                ));
            }
            let candidate = &self.candidates[index];
            let ordinal = candidate.plan.ordinal;
            // §4.1: the attempt start is durable BEFORE the dispatch.
            self.journal(
                &format!("begin-{ordinal}"),
                Op::RoutingAttemptBegin {
                    turn_id: self.turn_id.clone(),
                    ordinal,
                    routing_mode: self.routing_mode,
                    provider: candidate.plan.provider_id.clone(),
                    candidate: candidate.plan.candidate.clone(),
                },
            )?;
            let mut routed = request.clone();
            // Only the selected provider wire model id is serialized (§7):
            // the candidate's bare leaf/alias replaces the configured alias.
            routed.model = candidate.plan.candidate.clone();
            let outcome = candidate.transport.attempt(&routed, hooks).await;
            let consumed = outcome.attempts_consumed.max(1);
            {
                let mut budget = self.budget.lock().unwrap_or_else(|p| p.into_inner());
                *budget = budget.saturating_sub(consumed);
            }
            match outcome.result {
                Ok(response) => {
                    let metering = meter_success(
                        candidate.plan.kind,
                        &candidate.plan.provider_id,
                        &self.admitted_leaves(),
                        response.model.as_deref(),
                        &response.usage,
                        self.alias_identity_trusted,
                        self.pricing.as_deref(),
                    );
                    self.journal(
                        &format!("receipt-{ordinal}"),
                        Op::RoutingReceipt {
                            turn_id: self.turn_id.clone(),
                            ordinal,
                            routing_mode: self.routing_mode,
                            provider: candidate.plan.provider_id.clone(),
                            configured_reference: self.configured_reference.clone(),
                            candidate: candidate.plan.candidate.clone(),
                            outcome: RoutingOutcome::Committed,
                            failure: None,
                            status: None,
                            attempts_consumed: consumed,
                            selected: true,
                            response_model: metering.response_model,
                            leaf_identity: metering.provenance,
                            usage: Some(metering.usage),
                            exhaustion: None,
                            rejection: None,
                        },
                    )?;
                    *self.latched.lock().unwrap_or_else(|p| p.into_inner()) = Some(index);
                    return Ok(response);
                }
                Err(err) => {
                    let class = classify_attempt(&signals_of_model_error(&err));
                    let status = status_of_model_error(&err);
                    let budget_left = *self.budget.lock().unwrap_or_else(|p| p.into_inner());
                    let more_candidates = index + 1 < self.candidates.len();
                    let cascade = class.cascades() && budget_left > 0 && more_candidates;
                    let exhaustion = if cascade {
                        None
                    } else if class.cascades() {
                        // CandidatesExhausted when the last candidate was
                        // attempted; BudgetExhausted only when candidates
                        // remain but the budget is gone.
                        Some(if more_candidates {
                            RoutingExhaustion::BudgetExhausted
                        } else {
                            RoutingExhaustion::CandidatesExhausted
                        })
                    } else {
                        None
                    };
                    // §6: a failed attempt's provider-reported usage is
                    // retained and charged — failover never makes a consumed
                    // attempt free.
                    let failed_usage = outcome.failed_usage.as_ref().map(|usage| RoutingUsage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                        microcents: 0,
                        priced: false,
                        reported: true,
                    });
                    self.journal(
                        &format!("receipt-{ordinal}"),
                        Op::RoutingReceipt {
                            turn_id: self.turn_id.clone(),
                            ordinal,
                            routing_mode: self.routing_mode,
                            provider: candidate.plan.provider_id.clone(),
                            configured_reference: self.configured_reference.clone(),
                            candidate: candidate.plan.candidate.clone(),
                            outcome: if class.cascades() {
                                RoutingOutcome::CascadeFailure
                            } else {
                                RoutingOutcome::TerminalFailure
                            },
                            failure: Some(class),
                            status,
                            attempts_consumed: consumed,
                            selected: false,
                            response_model: None,
                            leaf_identity: LeafProvenance::Absent,
                            usage: failed_usage,
                            exhaustion,
                            rejection: None,
                        },
                    )?;
                    if cascade {
                        *self.cursor.lock().unwrap_or_else(|p| p.into_inner()) = index + 1;
                        continue;
                    }
                    return Err(err);
                }
            }
        }
    }
}

/// The production ModelDriver face of the ladder (the acp/exec turn path):
/// pre-commit calls run the ladder; post-commit calls ride the latched
/// candidate.
#[derive(Debug)]
pub struct AutoDriver<D: ModelDriver> {
    ladder: Ladder<DriverTransport<D>>,
}

impl<D: ModelDriver> AutoDriver<D> {
    pub fn new(ladder: Ladder<DriverTransport<D>>) -> Self {
        Self { ladder }
    }
}

#[async_trait::async_trait]
impl<D: ModelDriver> ModelDriver for AutoDriver<D> {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        self.complete_observed(request, &CallHooks::none()).await
    }

    async fn complete_observed(
        &self,
        request: &ModelRequest,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        self.ladder.complete_observed(request, hooks).await
    }
}

/// The per-turn driver union (§1): pins and implicit passthrough dispatch
/// exactly like pre-P5; only `auto_client_side` turns run the ladder.
#[derive(Debug)]
pub enum PromptDriver<D: ModelDriver> {
    Pinned(D),
    Auto(AutoDriver<D>),
}

#[async_trait::async_trait]
impl<D: ModelDriver> ModelDriver for PromptDriver<D> {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        match self {
            PromptDriver::Pinned(driver) => driver.complete(request).await,
            PromptDriver::Auto(driver) => driver.complete(request).await,
        }
    }

    async fn complete_observed(
        &self,
        request: &ModelRequest,
        hooks: &CallHooks<'_>,
    ) -> Result<ModelResponse, ModelError> {
        match self {
            PromptDriver::Pinned(driver) => driver.complete_observed(request, hooks).await,
            PromptDriver::Auto(driver) => driver.complete_observed(request, hooks).await,
        }
    }
}

// ═══════════════════════ §4.1 kill-resume ═══════════════════════

/// The resume plan replayed from the JOURNAL (never rediscovered): the
/// journaled snapshot's admitted candidates, the remaining budget, the
/// cursor (the next admitted candidate after the last begun attempt), and
/// the stranded in-flight ordinals (already reconciled by
/// [`reconcile_interrupted`]).
#[derive(Debug, Clone)]
pub struct ResumedLadder {
    pub configured_reference: String,
    /// The journaled snapshot's candidates (admitted AND rejected), for
    /// audit/re-journaling on the resumed turn.
    pub snapshot_candidates: Vec<RoutingCandidate>,
    /// The journaled catalog/proof digest (replayed, never recomputed).
    pub catalog_digest: String,
    /// Admitted candidates the resume may still attempt, in journaled order.
    pub remaining: Vec<AdmittedCandidate>,
    pub budget: u32,
    /// The journaled exhaustion outcome when the budget/candidates were
    /// already consumed at kill time — resume reports it WITHOUT dispatch.
    pub exhaustion: Option<(RoutingExhaustion, Option<RoutingFailureClass>, Option<u16>)>,
}

/// Replay the journaled ladder state for an interrupted turn (§4.1):
/// `turn_routing` is the fold of the killed turn's routing ops. Returns
/// None when the turn was not a routed auto turn (no snapshot / not
/// auto_client_side) or nothing remains to resume.
pub fn plan_resume(turn_routing: &nano_session::TurnRouting) -> Option<ResumedLadder> {
    let (mode, configured_reference, budget, candidates, catalog_digest) =
        match &turn_routing.snapshot {
            Some(Op::RoutingSnapshot {
                routing_mode,
                configured_reference,
                attempt_budget,
                candidates,
                catalog_digest,
                ..
            }) => (
                *routing_mode,
                configured_reference.clone(),
                *attempt_budget,
                candidates.clone(),
                catalog_digest.clone(),
            ),
            _ => return None,
        };
    if mode != RoutingMode::AutoClientSide {
        return None;
    }
    let consumed = turn_routing.attempts_consumed();
    let remaining_budget = budget.saturating_sub(consumed);
    // The ladder continues at the next admitted candidate AFTER the last
    // begun ordinal (in-flight attempts are consumed, never replayed).
    let last_begun = turn_routing.begins.keys().next_back().copied();
    let remaining: Vec<AdmittedCandidate> = candidates
        .iter()
        .filter(|c| c.admitted)
        .enumerate()
        .filter(|(position, _)| last_begun.is_none_or(|last| *position as u32 > last))
        .map(|(position, c)| AdmittedCandidate {
            ordinal: position as u32,
            provider_id: c.provider.clone(),
            candidate: c.candidate.clone(),
            reference: match c.kind {
                CandidateKind::Leaf if c.provider != "flux-router" => {
                    format!("{}:{}", c.provider, c.candidate)
                }
                _ => c.candidate.clone(),
            },
            kind: c.kind,
        })
        .collect();
    // The journaled exhaustion outcome, when the ladder had already spent
    // itself at kill time (the final receipt carries it).
    let exhaustion = turn_routing
        .receipts
        .values()
        .filter_map(|op| match op {
            Op::RoutingReceipt {
                exhaustion: Some(exhaustion),
                failure,
                status,
                ..
            } => Some((*exhaustion, *failure, *status)),
            _ => None,
        })
        .next_back();
    if remaining_budget == 0 && exhaustion.is_none() {
        return Some(ResumedLadder {
            configured_reference,
            snapshot_candidates: candidates,
            catalog_digest: catalog_digest.clone(),
            remaining,
            budget: remaining_budget,
            exhaustion: Some((RoutingExhaustion::BudgetExhausted, None, None)),
        });
    }
    Some(ResumedLadder {
        configured_reference,
        snapshot_candidates: candidates,
        catalog_digest,
        remaining,
        budget: remaining_budget,
        exhaustion,
    })
}

/// §4.1 reconciliation at session/load (and exec bootstrap): every attempt
/// BEGUN without a receipt on an interrupted turn is indeterminate — it is
/// counted against the budget and charged the §3.5 conservative estimate
/// (journaled as a `ConsumedInflight` receipt with `reported: false`,
/// never zero, never free). Idempotent: the receipt's envelope id dedups a
/// retried reconciliation.
///
/// Returns the number of receipts journaled. `turn_input` is the
/// interrupted turn's journaled input (the estimate's input basis).
pub fn reconcile_interrupted(
    sink: &dyn RoutingSink,
    turn_id: &str,
    turn_routing: &nano_session::TurnRouting,
    turn_input: &str,
) -> std::io::Result<u32> {
    let mut journaled = 0u32;
    for ordinal in turn_routing.stranded_ordinals() {
        let (mode, provider, candidate, configured_reference) =
            match turn_routing.begins.get(&ordinal) {
                Some(Op::RoutingAttemptBegin {
                    routing_mode,
                    provider,
                    candidate,
                    ..
                }) => {
                    let reference = match &turn_routing.snapshot {
                        Some(Op::RoutingSnapshot {
                            configured_reference,
                            ..
                        }) => configured_reference.clone(),
                        _ => String::new(),
                    };
                    (
                        *routing_mode,
                        provider.clone(),
                        candidate.clone(),
                        reference,
                    )
                }
                _ => continue,
            };
        let estimate = consumed_inflight_estimate(turn_input);
        if !sink.append(&routing_envelope(
            turn_id,
            &format!("receipt-{ordinal}"),
            Op::RoutingReceipt {
                turn_id: turn_id.to_string(),
                ordinal,
                routing_mode: mode,
                provider,
                configured_reference,
                candidate,
                outcome: RoutingOutcome::ConsumedInflight,
                failure: None,
                status: None,
                attempts_consumed: 1,
                selected: false,
                response_model: None,
                leaf_identity: LeafProvenance::Absent,
                usage: Some(estimate),
                exhaustion: None,
                rejection: None,
            },
        )) {
            return Err(std::io::Error::other(
                "routing reconciliation append failed (fail-closed)",
            ));
        }
        journaled += 1;
    }
    Ok(journaled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nano_session::op::RoutingMode;

    // ── §1 precedence matrix ────────────────────────────────────────────

    #[test]
    fn precedence_matrix_explicit_pin_beats_everything() {
        for (source, reference, opt_in, expected) in [
            (
                ModelSource::ExplicitPin,
                "flux-auto",
                false,
                RoutingMode::ExplicitAliasPin,
            ),
            // An explicit pin of flux-auto stays a PIN even with the opt-in
            // set (precedence: pin > opt-in) — never client-side routing.
            (
                ModelSource::ExplicitPin,
                "flux-auto",
                true,
                RoutingMode::ExplicitAliasPin,
            ),
            (
                ModelSource::ExplicitPin,
                "openai:gpt-5",
                true,
                RoutingMode::ExplicitAliasPin,
            ),
            (
                ModelSource::ConfiguredDefault,
                "flux-auto",
                false,
                RoutingMode::ConfiguredDefaultAlias,
            ),
            // A configured default pin beats the opt-in.
            (
                ModelSource::ConfiguredDefault,
                "flux-auto",
                true,
                RoutingMode::ConfiguredDefaultAlias,
            ),
            (
                ModelSource::ImplicitDefault,
                "flux-auto",
                false,
                RoutingMode::ImplicitAliasPassthrough,
            ),
            (
                ModelSource::ImplicitDefault,
                "flux-auto",
                true,
                RoutingMode::AutoClientSide,
            ),
            // The opt-in with any other resolved reference is NOT auto.
            (
                ModelSource::ImplicitDefault,
                "flux-fast",
                true,
                RoutingMode::ImplicitAliasPassthrough,
            ),
            (
                ModelSource::ImplicitDefault,
                "openai:gpt-5",
                true,
                RoutingMode::ImplicitAliasPassthrough,
            ),
        ] {
            let decision = resolve_routing(source, reference, opt_in);
            assert_eq!(
                decision.mode, expected,
                "source={source:?} reference={reference} opt_in={opt_in}"
            );
            // The resolved reference is NEVER rewritten (no silent reroute).
            assert_eq!(decision.reference, reference);
        }
    }

    #[test]
    fn auto_opt_in_parsing_is_fail_closed_and_typed() {
        assert_eq!(parse_auto_opt_in(None), Ok(false));
        assert_eq!(parse_auto_opt_in(Some(String::new())), Ok(false));
        assert_eq!(parse_auto_opt_in(Some("  ".into())), Ok(false));
        assert_eq!(parse_auto_opt_in(Some("1".into())), Ok(true));
        assert_eq!(parse_auto_opt_in(Some("true".into())), Ok(true));
        assert_eq!(parse_auto_opt_in(Some("0".into())), Ok(false));
        assert_eq!(parse_auto_opt_in(Some("false".into())), Ok(false));
        let err = parse_auto_opt_in(Some("yes".into())).expect_err("typed config error");
        assert!(err.contains(AUTO_ROUTING_ENV), "{err}");
    }

    #[test]
    fn configured_default_parsing_is_typed() {
        assert_eq!(parse_configured_default(None), Ok(None));
        assert_eq!(
            parse_configured_default(Some("openai:gpt-5".into())),
            Ok(Some("openai:gpt-5".into()))
        );
        assert!(parse_configured_default(Some("a:b:c".into())).is_err());
        assert!(parse_configured_default(Some(":".into())).is_err());
    }

    // ── §5 requirement detection ────────────────────────────────────────

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::Value::Null,
        }
    }

    #[test]
    fn requirements_detection_covers_each_surface_separately() {
        let empty: &[ContentBlock] = &[];
        let no_context: &[Message] = &[];
        // Text-only turn, empty tool list: nothing required.
        let reqs = requirements_of(empty, no_context, &[]);
        assert_eq!(
            reqs,
            TurnRequirements {
                images: false,
                tools: false
            }
        );
        // Built-in tools advertised.
        let reqs = requirements_of(empty, no_context, &[tool_def("fs_read")]);
        assert!(reqs.tools && !reqs.images);
        // MCP tools advertised.
        let reqs = requirements_of(empty, no_context, &[tool_def("mcp__server__tool")]);
        assert!(reqs.tools);
        // Tool continuation turn: empty advertised list but tool blocks in
        // the assembled context.
        let context = vec![Message::tool_result("call-1", "ok", false)];
        let reqs = requirements_of(empty, &context, &[]);
        assert!(reqs.tools, "tool continuation turn is tool-bearing");
        // Image in the current prompt.
        let image = [ContentBlock::Image {
            mime: "image/png".into(),
            data: "aA".into(),
        }];
        let reqs = requirements_of(&image, no_context, &[]);
        assert!(reqs.images && !reqs.tools);
        // Image carried by earlier context.
        let context = vec![Message::user_blocks(vec![ContentBlock::Image {
            mime: "image/png".into(),
            data: "aA".into(),
        }])];
        let reqs = requirements_of(empty, &context, &[]);
        assert!(reqs.images);
        // Tool-result images count as image-bearing AND tool-bearing.
        let context = vec![Message::tool_result_with_images(
            "call-1",
            "ok",
            false,
            vec![nano_model::types::ImageData {
                mime: "image/png".into(),
                data: "aA".into(),
            }],
        )];
        let reqs = requirements_of(empty, &context, &[]);
        assert!(reqs.images && reqs.tools);
    }

    // ── §4 classifier matrix ────────────────────────────────────────────

    #[test]
    fn classifier_matrix_cascading_and_terminal_classes() {
        let at = |status| FailureSignals {
            status: Some(status),
            ..FailureSignals::default()
        };
        // Cascading: 408, 429, 5xx.
        assert_eq!(
            classify_attempt(&at(408)),
            RoutingFailureClass::RequestTimeout
        );
        assert_eq!(classify_attempt(&at(429)), RoutingFailureClass::RateLimited);
        for status in [500, 502, 503, 529] {
            assert_eq!(
                classify_attempt(&at(status)),
                RoutingFailureClass::ServerError
            );
        }
        // Terminal: 400/422, 401/403, 402, 404, 413(context).
        assert_eq!(
            classify_attempt(&at(400)),
            RoutingFailureClass::FormatRejected
        );
        assert_eq!(
            classify_attempt(&at(422)),
            RoutingFailureClass::FormatRejected
        );
        assert_eq!(classify_attempt(&at(401)), RoutingFailureClass::Auth);
        assert_eq!(classify_attempt(&at(403)), RoutingFailureClass::Auth);
        assert_eq!(classify_attempt(&at(402)), RoutingFailureClass::Billing);
        assert_eq!(
            classify_attempt(&at(404)),
            RoutingFailureClass::ModelNotFound
        );
        assert_eq!(
            classify_attempt(&at(413)),
            RoutingFailureClass::ContextOverflow
        );
        // Unclassifiable / unknown → terminal Unknown.
        assert_eq!(classify_attempt(&at(418)), RoutingFailureClass::Unknown);
        assert_eq!(
            classify_attempt(&FailureSignals::default()),
            RoutingFailureClass::Unknown
        );
        assert!(!RoutingFailureClass::Unknown.cascades());
    }

    #[test]
    fn classifier_conflicts_resolve_conservative_wins() {
        // 429-with-auth-body: the cascading status narrows to terminal Auth.
        let signals = FailureSignals {
            status: Some(429),
            sdk: Some(RoutingFailureClass::RateLimited),
            body: Some(RoutingFailureClass::Auth),
        };
        assert_eq!(classify_attempt(&signals), RoutingFailureClass::Auth);
        // 500-with-format-body: terminal FormatRejected.
        let signals = FailureSignals {
            status: Some(500),
            sdk: Some(RoutingFailureClass::ServerError),
            body: Some(RoutingFailureClass::FormatRejected),
        };
        assert_eq!(
            classify_attempt(&signals),
            RoutingFailureClass::FormatRejected
        );
        // No conflict: pure cascade stays cascading.
        let signals = FailureSignals {
            status: Some(503),
            sdk: Some(RoutingFailureClass::Overloaded),
            body: None,
        };
        let class = classify_attempt(&signals);
        assert!(class.cascades(), "{class:?}");
        // SDK terminal evidence alone narrows a cascading status.
        let signals = FailureSignals {
            status: Some(500),
            sdk: Some(RoutingFailureClass::Auth),
            body: None,
        };
        assert_eq!(classify_attempt(&signals), RoutingFailureClass::Auth);
    }

    #[test]
    fn model_error_signals_cover_the_commit_boundary() {
        // Pre-commit transport classes cascade.
        for phase in [
            TransportPhase::Connect,
            TransportPhase::Tls,
            TransportPhase::BeforeFirstByte,
        ] {
            let err = ModelError::Transport {
                phase,
                message: String::new(),
            };
            let class = classify_attempt(&signals_of_model_error(&err));
            assert_eq!(class, RoutingFailureClass::TransportPreCommit);
            assert!(class.cascades());
        }
        // Post-commit: MidStream (partial SSE) is terminal.
        let err = ModelError::Transport {
            phase: TransportPhase::MidStream,
            message: String::new(),
        };
        assert_eq!(
            classify_attempt(&signals_of_model_error(&err)),
            RoutingFailureClass::PostCommit
        );
        // Malformed success bodies / truncated streams → Protocol, terminal.
        let err = ModelError::Protocol("bad json".into());
        assert_eq!(
            classify_attempt(&signals_of_model_error(&err)),
            RoutingFailureClass::Protocol
        );
        // The live-wire auth fold: 500 + auth_error body arrives as Auth —
        // terminal despite the 5xx status signal (conservative wins).
        let err = ModelError::Auth {
            message: String::new(),
            status: Some(500),
        };
        assert_eq!(
            classify_attempt(&signals_of_model_error(&err)),
            RoutingFailureClass::Auth
        );
        // Cancellation is terminal.
        assert_eq!(
            classify_attempt(&signals_of_model_error(&ModelError::Cancelled)),
            RoutingFailureClass::Cancelled
        );
        // RateLimited cascades; 404 is terminal.
        let err = ModelError::RateLimited {
            retry_after_ms: None,
        };
        assert_eq!(
            classify_attempt(&signals_of_model_error(&err)),
            RoutingFailureClass::RateLimited
        );
        let err = ModelError::Server {
            status: 404,
            message: String::new(),
        };
        assert_eq!(
            classify_attempt(&signals_of_model_error(&err)),
            RoutingFailureClass::ModelNotFound
        );
    }
}

#[cfg(test)]
mod construction_tests {
    use super::*;
    use nano_session::op::{CandidateKind, RoutingMode};

    fn vision_catalog_with(pairs: &[(&str, bool)]) -> VisionCatalog {
        let entries = pairs
            .iter()
            .map(|(id, image_in)| {
                if *image_in {
                    format!(
                        r#""{id}": {{ "image_in": true, "proven": "shared/fixtures/flux/vision/probe.json" }}"#
                    )
                } else {
                    format!(r#""{id}": {{ "image_in": false, "proven": null }}"#)
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        VisionCatalog::from_json_str(&format!(r#"{{"version": 1, "entries": {{{entries}}}}}"#))
            .expect("test catalog parses")
    }

    fn router_with_payload() -> ProviderRouter {
        ProviderRouter::from_payload(Some(
            r#"[
                {"provider":"nvidia","models":["nv-alpha"],"hasKey":true},
                {"provider":"openai","models":["gpt-a","gpt-b"],"hasKey":true}
            ]"#,
        ))
        .expect("payload validates")
    }

    fn env_with(keys: &[&str]) -> impl Fn(&str) -> Option<String> {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        move |name: &str| {
            if keys.iter().any(|k| k == name) {
                Some("sk-test-not-a-real-key".to_string())
            } else {
                None
            }
        }
    }

    fn text_only() -> TurnRequirements {
        TurnRequirements::default()
    }

    // ── §3 candidate construction ───────────────────────────────────────

    #[test]
    fn unproven_first_proven_second_regression_at_construction() {
        let router = router_with_payload();
        let get_env = env_with(&["NVIDIA_API_KEY", "OPENAI_API_KEY"]);
        let vision = vision_catalog_with(&[]);
        let inputs = CandidateInputs {
            router: &router,
            get_env: &get_env,
            now_unix_secs: 0,
            flux_credentialed: true,
            flux_advertised: &[],
            vision: &vision,
            tools: &EmptyToolCapabilityCatalog,
            approved_leaves: &["nvidia:nv-alpha".to_string(), "openai:gpt-a".to_string()],
            requirements: text_only(),
        };
        let plan = construct_candidates(&inputs);
        // nvidia is advertised+credentialed but UNPROVEN: excluded at
        // construction, never after selection (§3).
        let nvidia = plan
            .candidates
            .iter()
            .find(|c| c.provider == "nvidia")
            .expect("rejected candidates stay journaled");
        assert!(!nvidia.admitted);
        assert_eq!(nvidia.rejection, Some(CandidateRejection::ProviderUnproven));
        let openai = plan
            .candidates
            .iter()
            .find(|c| c.provider == "openai")
            .expect("openai candidate");
        assert!(openai.admitted);
        // The alias rung leads, then the proven direct leaf.
        assert_eq!(plan.admitted[0].candidate, FLUX_AUTO);
        assert_eq!(plan.admitted[1].reference, "openai:gpt-a");
    }

    #[test]
    fn corrected_fallback_skips_unproven() {
        // The startup fallback applies the same selection-time proven gate.
        let router = router_with_payload();
        let get_env = env_with(&["NVIDIA_API_KEY", "OPENAI_API_KEY"]);
        assert_eq!(
            router.initial_non_flux_model(&get_env, 0).as_deref(),
            Some("openai:gpt-a")
        );
        let unproven_only = ProviderRouter::from_payload(Some(
            r#"[{"provider":"nvidia","models":["nv-alpha"],"hasKey":true}]"#,
        ))
        .expect("payload");
        let get_env = env_with(&["NVIDIA_API_KEY"]);
        assert_eq!(unproven_only.initial_non_flux_model(&get_env, 0), None);
    }

    #[test]
    fn candidate_ordering_is_stable_independent_of_input_order() {
        let router = router_with_payload();
        let get_env = env_with(&["NVIDIA_API_KEY", "OPENAI_API_KEY"]);
        let vision = vision_catalog_with(&[]);
        let forward = [
            "openai:gpt-b".to_string(),
            "openai:gpt-a".to_string(),
            "nvidia:nv-alpha".to_string(),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();
        let build = |approved: &[String]| {
            construct_candidates(&CandidateInputs {
                router: &router,
                get_env: &get_env,
                now_unix_secs: 0,
                flux_credentialed: true,
                flux_advertised: &["flux-pinned-gpt-5".to_string()],
                vision: &vision,
                tools: &EmptyToolCapabilityCatalog,
                approved_leaves: approved,
                requirements: text_only(),
            })
        };
        let a = build(&forward);
        let b = build(&reversed);
        assert_eq!(a.candidates, b.candidates, "deterministic candidate list");
        // Rung-3 order: catalog provider order, then ADVERTISED (payload)
        // model order — not manifest order.
        let order: Vec<&str> = b.admitted.iter().map(|c| c.reference.as_str()).collect();
        assert_eq!(order, ["flux-auto", "openai:gpt-a", "openai:gpt-b"]);
    }

    #[test]
    fn alias_rung_capability_gating() {
        let router = ProviderRouter::default();
        let get_env = env_with(&[]);
        let vision = vision_catalog_with(&[]);
        let build = |requirements: TurnRequirements, credentialed: bool| {
            construct_candidates(&CandidateInputs {
                router: &router,
                get_env: &get_env,
                now_unix_secs: 0,
                flux_credentialed: credentialed,
                flux_advertised: &[],
                vision: &vision,
                tools: &EmptyToolCapabilityCatalog,
                approved_leaves: &[],
                requirements,
            })
        };
        // Text-only + credentialed: the alias rung is admitted.
        let plan = build(text_only(), true);
        assert_eq!(plan.admitted.len(), 1);
        assert_eq!(plan.admitted[0].kind, CandidateKind::Alias);
        // Tool-bearing: the empty tool catalog fails closed (§3 hard
        // prerequisite) — CapabilityUnproven, no dispatch.
        let plan = build(
            TurnRequirements {
                images: false,
                tools: true,
            },
            true,
        );
        assert!(plan.admitted.is_empty());
        assert_eq!(
            plan.candidates[0].rejection,
            Some(CandidateRejection::CapabilityUnproven)
        );
        // Image-bearing: aliases are never vision-blessed (P2a) → rejected.
        let plan = build(
            TurnRequirements {
                images: true,
                tools: false,
            },
            true,
        );
        assert!(plan.admitted.is_empty());
        // Uncredentialed Flux: rejected ProviderUncredentialed.
        let plan = build(text_only(), false);
        assert!(plan.admitted.is_empty());
        assert_eq!(
            plan.candidates[0].rejection,
            Some(CandidateRejection::ProviderUncredentialed)
        );
    }

    #[test]
    fn tool_capability_injection_admits_exact_leaves_only() {
        // The §8 seam: an injected proof map admits exactly the proven
        // (provider, leaf) pairs — unknown equals false.
        let router = router_with_payload();
        let get_env = env_with(&["OPENAI_API_KEY"]);
        let vision = vision_catalog_with(&[]);
        let tools = |provider: &str, leaf: &str| provider == "openai" && leaf == "gpt-a";
        let inputs = CandidateInputs {
            router: &router,
            get_env: &get_env,
            now_unix_secs: 0,
            flux_credentialed: true,
            flux_advertised: &[],
            vision: &vision,
            tools: &tools,
            approved_leaves: &["openai:gpt-a".to_string(), "openai:gpt-b".to_string()],
            requirements: TurnRequirements {
                images: false,
                tools: true,
            },
        };
        let plan = construct_candidates(&inputs);
        let admitted: Vec<&str> = plan.admitted.iter().map(|c| c.reference.as_str()).collect();
        assert_eq!(
            admitted,
            ["openai:gpt-a"],
            "the alias rung is unproven for tools; only the exact proven leaf is admitted"
        );
        let gpt_b = plan
            .candidates
            .iter()
            .find(|c| c.candidate == "gpt-b")
            .expect("journaled");
        assert_eq!(
            gpt_b.rejection,
            Some(CandidateRejection::CapabilityUnproven)
        );
    }

    // ── §6 metering / leaf-identity trust ───────────────────────────────

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        }
    }

    fn pricing_with_flux_leaf() -> PricingCatalog {
        PricingCatalog::from_toml_str(
            "[flux-router.flux-pinned-gpt-5]\ninput_per_mtok_usd = 1.0\noutput_per_mtok_usd = 2.0\n",
        )
        .expect("test pricing parses")
    }

    #[test]
    fn response_leaf_trust_matrix() {
        let pricing = pricing_with_flux_leaf();
        let leaves = vec!["flux-pinned-gpt-5".to_string()];
        // Absent identity: unknown/unpriced.
        let m = meter_success(
            CandidateKind::Alias,
            "flux-router",
            &leaves,
            None,
            &usage(10, 5),
            false,
            Some(&pricing),
        );
        assert_eq!(m.provenance, LeafProvenance::Absent);
        assert!(!m.usage.priced && m.usage.microcents == 0);
        // Alias-valued response model is NOT leaf identity.
        let m = meter_success(
            CandidateKind::Leaf,
            "flux-router",
            &leaves,
            Some("flux-fast"),
            &usage(10, 5),
            false,
            Some(&pricing),
        );
        assert_eq!(m.provenance, LeafProvenance::Absent);
        assert!(!m.usage.priced);
        // Leaf candidate, mismatched response leaf: journaled mismatch,
        // unpriced, never attributed to the configured reference.
        let m = meter_success(
            CandidateKind::Leaf,
            "flux-router",
            &leaves,
            Some("flux-pinned-other"),
            &usage(10, 5),
            false,
            Some(&pricing),
        );
        assert_eq!(m.provenance, LeafProvenance::Mismatch);
        assert_eq!(m.response_model.as_deref(), Some("flux-pinned-other"));
        assert!(!m.usage.priced);
        // Leaf candidate, matching leaf: priced against the actual leaf.
        let m = meter_success(
            CandidateKind::Leaf,
            "flux-router",
            &leaves,
            Some("flux-pinned-gpt-5"),
            &usage(1_000_000, 1_000_000),
            false,
            Some(&pricing),
        );
        assert_eq!(m.provenance, LeafProvenance::ProviderReported);
        assert!(m.usage.priced && m.usage.microcents > 0);
        // Alias candidate, concrete reported leaf: provenance-only, NEVER
        // priced without the evidence path — even with a pricing row.
        let m = meter_success(
            CandidateKind::Alias,
            "flux-router",
            &[],
            Some("flux-pinned-gpt-5"),
            &usage(1_000_000, 1_000_000),
            false,
            Some(&pricing),
        );
        assert_eq!(m.provenance, LeafProvenance::ProviderReported);
        assert!(!m.usage.priced, "provenance-only is unpriced");
        // The evidence path (leg-1 proof + pricing row) lifts it.
        let m = meter_success(
            CandidateKind::Alias,
            "flux-router",
            &[],
            Some("flux-pinned-gpt-5"),
            &usage(1_000_000, 1_000_000),
            true,
            Some(&pricing),
        );
        assert!(m.usage.priced && m.usage.microcents > 0);
        // Evidence path WITHOUT a pricing row stays unpriced.
        let m = meter_success(
            CandidateKind::Alias,
            "flux-router",
            &[],
            Some("flux-pinned-unknown"),
            &usage(10, 5),
            true,
            Some(&pricing),
        );
        assert!(!m.usage.priced);
    }

    #[test]
    fn normalization_strips_only_the_own_prefix() {
        assert_eq!(
            normalize_response_model("openai", "openai:gpt-5"),
            Some("gpt-5")
        );
        assert_eq!(normalize_response_model("openai", "gpt-5"), Some("gpt-5"));
        // Another provider's prefix is a mismatch, never stripped.
        assert_eq!(normalize_response_model("openai", "groq:gpt-5"), None);
        assert_eq!(normalize_response_model("openai", "a:b:c"), None);
        assert_eq!(normalize_response_model("openai", "openai:"), None);
    }

    #[test]
    fn killed_attempt_estimate_is_never_zero_and_unpriced() {
        let estimate = consumed_inflight_estimate("hello");
        assert!(!estimate.reported);
        assert!(!estimate.priced);
        assert!(estimate.input_tokens >= 1 && estimate.output_tokens >= 1);
        let estimate = consumed_inflight_estimate("");
        assert!(
            estimate.input_tokens >= 1,
            "never zero even for empty input"
        );
        // The fold into TurnUsage carries estimated provenance.
        let sum = estimate.to_turn_usage();
        assert_eq!(sum.usage_source, nano_session::UsageSource::Estimated);
        assert!(sum.total_tokens() > 0);
    }

    #[test]
    fn pin_snapshot_marks_alias_vs_leaf() {
        let candidates = pin_snapshot_candidates("flux-auto");
        assert_eq!(candidates[0].kind, CandidateKind::Alias);
        let candidates = pin_snapshot_candidates("openai:gpt-5");
        assert_eq!(candidates[0].kind, CandidateKind::Leaf);
        assert_eq!(candidates[0].provider, "openai");
        assert_eq!(candidates[0].candidate, "gpt-5");
        let candidates = pin_snapshot_candidates("flux-pinned-gpt-5");
        assert_eq!(candidates[0].kind, CandidateKind::Leaf);
        assert_eq!(candidates[0].provider, "flux-router");
    }

    #[test]
    fn snapshot_ops_round_trip_snake_case() {
        // §7: the journal enums are stable snake_case.
        let op = Op::RoutingSnapshot {
            turn_id: "t".into(),
            routing_mode: RoutingMode::AutoClientSide,
            configured_reference: FLUX_AUTO.into(),
            attempt_budget: ATTEMPT_BUDGET,
            candidates: pin_snapshot_candidates(FLUX_AUTO),
            catalog_digest: "ab".repeat(32),
        };
        let json = serde_json::to_value(&op).expect("serialize");
        assert_eq!(json["type"], "routing_snapshot");
        assert_eq!(json["routing_mode"], "auto_client_side");
        assert_eq!(json["candidates"][0]["kind"], "alias");
        let back: Op = serde_json::from_value(json).expect("deserialize");
        assert!(matches!(back, Op::RoutingSnapshot { .. }));
        let receipt = Op::RoutingReceipt {
            turn_id: "t".into(),
            ordinal: 0,
            routing_mode: RoutingMode::ImplicitAliasPassthrough,
            provider: "flux-router".into(),
            configured_reference: FLUX_AUTO.into(),
            candidate: FLUX_AUTO.into(),
            outcome: RoutingOutcome::CascadeFailure,
            failure: Some(RoutingFailureClass::RateLimited),
            status: Some(429),
            attempts_consumed: 1,
            selected: false,
            response_model: None,
            leaf_identity: LeafProvenance::Absent,
            usage: None,
            exhaustion: Some(RoutingExhaustion::BudgetExhausted),
            rejection: None,
        };
        let json = serde_json::to_value(&receipt).expect("serialize");
        assert_eq!(json["outcome"], "cascade_failure");
        assert_eq!(json["failure"], "rate_limited");
        assert_eq!(json["exhaustion"], "budget_exhausted");
        assert_eq!(json["leaf_identity"], "absent");
        assert!(serde_json::from_value::<Op>(json).is_ok());
        // Forward tolerance: unknown modes fold to Unknown, never fail.
        let mut value = serde_json::to_value(&receipt).expect("serialize");
        value["routing_mode"] = serde_json::json!("future_mode");
        let op: Op = serde_json::from_value(value).expect("tolerant");
        match op {
            Op::RoutingReceipt { routing_mode, .. } => {
                assert_eq!(routing_mode, RoutingMode::Unknown)
            }
            other => panic!("expected receipt, got {other:?}"),
        }
    }
}
