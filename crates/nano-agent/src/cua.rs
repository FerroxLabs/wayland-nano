//! S9 CUA wiring: the session-side bridge between the turn engine and
//! `nano-cua` (design S9-BROWSER-CUA-DESIGN.md §2–§5).
//!
//! The LOCKED invariants this module enforces or feeds:
//! - **Posture (§3, strictest-wins):** `read_only`/plan ⇒ the tools are not
//!   registered; `default`/`full_auto` ⇒ registered and EVERY op prompts —
//!   CUA is uncontainable by construction (§2.1), so no mode auto-approves
//!   it. Registration is the host's call ([`cua_registration`]); the gate
//!   arm is the enforcement; this module's policy layer sits between them.
//! - **Journal-first (§4.1):** the engine appends `Op::CuaAction` BEFORE
//!   dispatch (a failed append is turn-fatal) and `Op::CuaResult` after —
//!   digests only, never raw coordinates or typed text.
//! - **Kill switch (§2.5):** dispatch races the turn's cancel flag (typed
//!   `Cancelled`); a backend panic is contained into a typed `CuaBackend`
//!   error (spawned task, never a process abort); NO retry of any op.
//! - **Resume (§4.2):** a session resumed with an unpaired `CuaAction`
//!   ([`nano_session::SessionState::interrupted_cua`]) must take a fresh
//!   screenshot before any other CUA op — enforced here, in-memory, dying
//!   with the session (like the §2.2 seen-app set).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use nano_cua::error::CuaError;
use nano_cua::op::{CuaOp, CuaOpResult};
use nano_cua::policy::CuaPolicyOutcome;
use nano_cua::{ComputerUseBackend, CuaPolicy, Region, ScreenshotFormat};
use nano_model::types::{ToolCall, ToolDefinition};
use nano_session::NanoErrorKind;
use nano_session::attachment_store::AttachmentStore;
use nano_session::op::CuaOutcome;
use sha2::Digest as _;

use crate::loop_protection::ProgressSignals;

/// The eight v1 model-surface tools (S9 §1.2) — `mouse_move`, `ax_tree`,
/// and `frontmost_app` are NOT model-callable (the enum keeps the donor's
/// 11-variant wire shape for forward tolerance only).
pub const CUA_TOOL_NAMES: [&str; 8] = [
    "cua_left_click",
    "cua_right_click",
    "cua_double_click",
    "cua_scroll",
    "cua_type",
    "cua_key",
    "cua_screenshot",
    "cua_wait",
];

pub fn is_cua_tool(name: &str) -> bool {
    CUA_TOOL_NAMES.contains(&name)
}

/// Tool name → the `CuaOp` kind tag (the donor `kind_tag` vocabulary).
fn kind_of_tool(name: &str) -> Option<&'static str> {
    Some(match name {
        "cua_left_click" => "left_click",
        "cua_right_click" => "right_click",
        "cua_double_click" => "double_click",
        "cua_scroll" => "scroll",
        "cua_type" => "type",
        "cua_key" => "key",
        "cua_screenshot" => "screenshot",
        "cua_wait" => "wait",
        _ => return None,
    })
}

/// S9 §3 registration decision (strictest-wins, Q3 RULED): registered iff a
/// session backend is wired (no compositor/platform probe pass ⇒ no
/// registration, §5.4/Q5), the plan posture is OFF (plan forbids mutation;
/// CUA is mutation), and the mode registers CUA — `default`/`full_auto`
/// prompt per op; `read_only` never registers.
pub fn cua_registration(mode_id: &str, plan_active: bool, session_wired: bool) -> bool {
    if !session_wired || plan_active {
        return false;
    }
    match nano_cua::posture::CuaMode::from_wire_id(mode_id) {
        Some(mode) => {
            nano_cua::posture::posture_for_mode(mode) == nano_cua::posture::CuaPosture::AlwaysPrompt
        }
        // Unknown mode id: fail closed (the nano-cua wire-id discipline).
        None => false,
    }
}

/// The ops that mutate the desktop (§2.4: pre/post screenshot digests are
/// journaled for these). `screenshot` and `wait` are non-mutating.
fn is_mutating(op: &CuaOp) -> bool {
    matches!(
        op,
        CuaOp::LeftClick { .. }
            | CuaOp::RightClick { .. }
            | CuaOp::DoubleClick { .. }
            | CuaOp::Scroll { .. }
            | CuaOp::Type { .. }
            | CuaOp::Key { .. }
    )
}

/// The C7 mapping (design §5's table): nano-cua's local error enum folds
/// into the closed `NanoErrorKind` vocabulary; raw OS strings stay
/// logs-side, never journaled.
pub fn kind_of_cua_error(err: &CuaError) -> NanoErrorKind {
    match err {
        CuaError::PolicyDenied | CuaError::InvalidInput => NanoErrorKind::CuaPolicyDenied,
        CuaError::FocusLost => NanoErrorKind::CuaFocusLost,
        CuaError::OsPermissionDenied { .. } => NanoErrorKind::CuaOsPermissionDenied,
        CuaError::BackendUnavailable { .. } => NanoErrorKind::CuaBackendUnavailable,
        CuaError::CoordinateOutOfRange => NanoErrorKind::CuaCoordinateOutOfRange,
        CuaError::Backend => NanoErrorKind::CuaBackend,
        CuaError::Cancelled => NanoErrorKind::UserCancelled,
    }
}

/// Model/journal-facing strings are bounded (defense in depth — a policy
/// reason or app id must never become an unbounded channel).
fn bounded(text: &str) -> String {
    const CAP: usize = 256;
    if text.chars().count() <= CAP {
        text.to_string()
    } else {
        text.chars().take(CAP).collect()
    }
}

/// sha256 of the canonical serialized args (serde_json's map ordering is
/// sorted — no `preserve_order` feature anywhere in the workspace — so this
/// rendering is deterministic). THE digest the journal carries in place of
/// raw coordinates/typed text (§4.1 digest-only invariant).
fn args_digest_of(call: &ToolCall) -> String {
    let bytes = serde_json::to_vec(&call.arguments).unwrap_or_default();
    format!("{:x}", sha2::Sha256::digest(&bytes))
}

/// The journal-facing half of a prepared call: exactly the `Op::CuaAction`
/// payload fields (§4.1). Digests and bounded ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuaActionFrame {
    pub op_kind: String,
    pub args_digest: String,
    pub frontmost_app: Option<String>,
    pub pre_shot: Option<String>,
}

/// A call that survived parse + policy + (for mutating ops) the pre-shot:
/// cleared for the approval gate and then dispatch.
#[derive(Debug)]
pub struct CuaPrepared {
    pub frame: CuaActionFrame,
    pub op: CuaOp,
    pub mutating: bool,
}

/// The prepare step's outcome. EVERY variant except `Ready` is terminal
/// before dispatch and still journals the CuaAction/CuaResult pair (§2.4:
/// every CUA op produces a journaled pair — a denied attempt is audit
/// evidence, not silence).
#[derive(Debug)]
pub enum CuaPrepare {
    Ready(CuaPrepared),
    /// Policy reject / §4.2 resume rule: denied, `CuaPolicyDenied`.
    Denied {
        frame: CuaActionFrame,
        message: String,
    },
    /// Typed pre-dispatch failure (frontmost probe is soft; the pre-shot
    /// capture/store path is hard): failed, carrying the kind.
    Failed {
        frame: CuaActionFrame,
        kind: NanoErrorKind,
        message: String,
    },
    /// Malformed arguments: failed, `MissingArgs`.
    BadArgs {
        frame: CuaActionFrame,
        message: String,
    },
}

impl CuaPrepare {
    pub fn frame(&self) -> &CuaActionFrame {
        match self {
            CuaPrepare::Ready(prepared) => &prepared.frame,
            CuaPrepare::Denied { frame, .. }
            | CuaPrepare::Failed { frame, .. }
            | CuaPrepare::BadArgs { frame, .. } => frame,
        }
    }

    /// The terminal record for non-Ready outcomes: (outcome, kind, message).
    pub fn terminal(self) -> Option<(CuaOutcome, NanoErrorKind, String)> {
        Some(match self {
            CuaPrepare::Ready(_) => return None,
            CuaPrepare::Denied { message, .. } => {
                (CuaOutcome::Denied, NanoErrorKind::CuaPolicyDenied, message)
            }
            CuaPrepare::Failed { kind, message, .. } => (CuaOutcome::Failed, kind, message),
            CuaPrepare::BadArgs { message, .. } => {
                (CuaOutcome::Failed, NanoErrorKind::MissingArgs, message)
            }
        })
    }

    /// A CUA call reached an engine whose host never wired a backend (§5.4
    /// fail-closed: no probe pass ⇒ no registration; a stale or hallucinated
    /// call still lands here and must never dispatch).
    pub fn unwired(call: &ToolCall) -> CuaPrepare {
        CuaPrepare::Failed {
            frame: CuaActionFrame {
                op_kind: kind_of_tool(&call.name).unwrap_or("unknown").to_string(),
                args_digest: args_digest_of(call),
                frontmost_app: None,
                pre_shot: None,
            },
            kind: NanoErrorKind::CuaBackendUnavailable,
            message: "computer use is not available on this host (no backend wired)".to_string(),
        }
    }
}

/// The dispatch step's outcome (post-approval). `output` is the bounded
/// model-facing text; the journaled `Op::CuaResult` carries only the
/// outcome, the post-shot digest, and the kind.
#[derive(Debug)]
pub struct CuaDispatchOutcome {
    pub outcome: CuaOutcome,
    pub post_shot: Option<String>,
    pub error_kind: Option<NanoErrorKind>,
    pub output: String,
    pub progress: ProgressSignals,
}

impl CuaDispatchOutcome {
    fn terminal(outcome: CuaOutcome, kind: NanoErrorKind, output: String) -> Self {
        Self {
            outcome,
            post_shot: None,
            error_kind: Some(kind),
            output,
            progress: ProgressSignals::default(),
        }
    }

    fn cancelled() -> Self {
        Self::terminal(
            CuaOutcome::Cancelled,
            NanoErrorKind::UserCancelled,
            "cancelled by caller".to_string(),
        )
    }
}

/// The engine-facing seam (a `TurnRobustness` slot). Implemented by
/// [`CuaSession`] in production; tests substitute scripted fakes via
/// `nano_cua::mock::MockBackend` underneath the real session.
#[async_trait::async_trait]
pub trait CuaBridge: std::fmt::Debug + Send + Sync {
    /// Parse + frontmost resolve + policy + (mutating ops) the pre-action
    /// screenshot. Runs BEFORE the CuaAction journal append and the gate.
    async fn prepare(&self, call: &ToolCall) -> CuaPrepare;
    /// Post-approval bookkeeping (§2.2's session-scoped first-contact
    /// seen-set), between gate approval and dispatch.
    fn note_approved(&self, prepared: &CuaPrepared);
    /// Dispatch racing the turn cancel flag (§2.5); panics contained.
    async fn dispatch(
        &self,
        prepared: &CuaPrepared,
        cancel: Option<&AtomicBool>,
    ) -> CuaDispatchOutcome;
}

/// The production bridge: one per session (the policy's seen-app set and the
/// §4.2 resume flag are session-scoped, in-memory, and die with it — §2.2
/// and §4.2 rule 3).
pub struct CuaSession {
    backend: Arc<dyn ComputerUseBackend>,
    policy: CuaPolicy,
    store: AttachmentStore,
    /// §4.2: set when the session resumed with an ambiguous tail — the first
    /// CUA op of the resumed turn MUST be a screenshot (screen state
    /// unknown). Cleared by one successful screenshot.
    needs_rescreenshot: AtomicBool,
}

impl std::fmt::Debug for CuaSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CuaSession")
            .field("backend", &self.backend.name())
            .field(
                "needs_rescreenshot",
                &self
                    .needs_rescreenshot
                    .load(std::sync::atomic::Ordering::SeqCst),
            )
            .finish_non_exhaustive()
    }
}

impl CuaSession {
    pub fn new(
        backend: Arc<dyn ComputerUseBackend>,
        policy: CuaPolicy,
        store: AttachmentStore,
        needs_rescreenshot: bool,
    ) -> Self {
        Self {
            backend,
            policy,
            store,
            needs_rescreenshot: AtomicBool::new(needs_rescreenshot),
        }
    }

    /// Capture + store a screenshot (the pre/post-shot evidence path, §2.4 —
    /// redaction always ON before bytes reach the blob store, §2.6).
    /// Returns the attachment digest.
    async fn capture_shot(&self) -> Result<String, (NanoErrorKind, String)> {
        let shot = self
            .backend
            .dispatch(
                None,
                CuaOp::Screenshot {
                    region: Region::Full,
                    format: ScreenshotFormat::Png,
                    redact: true,
                },
            )
            .await
            .map_err(|err| (kind_of_cua_error(&err), bounded(&err.to_string())))?;
        let CuaOpResult::Screenshot { data_b64, .. } = shot else {
            return Err((
                NanoErrorKind::CuaBackend,
                "screenshot dispatch returned a non-screenshot result".to_string(),
            ));
        };
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&data_b64)
            .map_err(|_| {
                (
                    NanoErrorKind::CuaBackend,
                    "screenshot payload was not valid base64".to_string(),
                )
            })?;
        let lease = self
            .store
            .acquire_write_lease()
            .map_err(|err| (err.kind(), bounded(&err.to_string())))?;
        self.store
            .put(&lease, &bytes)
            .map_err(|err| (err.kind(), bounded(&err.to_string())))
    }
}

#[async_trait::async_trait]
impl CuaBridge for CuaSession {
    async fn prepare(&self, call: &ToolCall) -> CuaPrepare {
        let op_kind = kind_of_tool(&call.name).unwrap_or("unknown").to_string();
        let frame = CuaActionFrame {
            op_kind: op_kind.clone(),
            args_digest: args_digest_of(call),
            frontmost_app: None,
            pre_shot: None,
        };
        // 1. Parse: inject the kind tag into the arguments so the internally
        //    tagged CuaOp enum is the ONE parse path; a model-smuggled
        //    `kind` key is removed first (the tool name is the authority).
        let op = {
            let mut args = call.arguments.clone();
            let parsed = match args.as_object_mut() {
                Some(object) => {
                    object.remove("kind");
                    object.insert("kind".into(), op_kind.clone().into());
                    serde_json::from_value::<CuaOp>(args.clone())
                        .map_err(|err| bounded(&err.to_string()))
                }
                None => Err("arguments must be an object".to_string()),
            };
            match parsed {
                Ok(op) if op.is_v1_model_surface() && op.kind_tag() == op_kind.as_str() => op,
                Ok(_) => {
                    return CuaPrepare::BadArgs {
                        frame,
                        message: format!("{}: parsed op does not match the tool name", call.name),
                    };
                }
                Err(message) => {
                    return CuaPrepare::BadArgs {
                        frame,
                        message: format!("bad {} arguments: {message}", call.name),
                    };
                }
            }
        };
        // 2. Frontmost resolve (§5.1: the value the approval prompt is
        //    issued against; the backend re-resolves at dispatch). A probe
        //    failure is UNRESOLVED (None), not fatal — §2.3's fail-closed
        //    rule routes unresolved targets to the mandatory prompt when
        //    app-scoped rules exist.
        let frontmost_app = self
            .backend
            .frontmost_app()
            .await
            .ok()
            .flatten()
            .map(|app| bounded(&app));
        let mut frame = CuaActionFrame {
            frontmost_app: frontmost_app.clone(),
            ..frame
        };
        // 3. §4.2 resume rule: a resumed turn's first CUA op must be a
        //    screenshot (screen state unknown after an ambiguous tail).
        if self
            .needs_rescreenshot
            .load(std::sync::atomic::Ordering::SeqCst)
            && op_kind != "screenshot"
        {
            return CuaPrepare::Denied {
                frame,
                message: "session resumed with an interrupted computer-use action: screen state is unknown — call cua_screenshot before any other computer-use op".to_string(),
            };
        }
        // 4. Policy (§2.3, fail-closed): hard rejects deny pre-gate; Prompt
        //    outcomes still prompt (every op prompts, §2.2 — the gate is the
        //    mechanism); Allow proceeds.
        match self
            .policy
            .check_op(&op, frontmost_app.as_deref().unwrap_or(""))
        {
            CuaPolicyOutcome::Reject { reason } => {
                return CuaPrepare::Denied {
                    frame,
                    message: bounded(&format!(
                        "computer-use policy denied the operation: {reason}"
                    )),
                };
            }
            CuaPolicyOutcome::Prompt { .. } | CuaPolicyOutcome::Allow => {}
        }
        // 5. Pre-shot for mutating ops (§2.4): capture evidence BEFORE the
        //    action; failure is typed and pre-dispatch (the action never
        //    happens without its evidence trail).
        let mutating = is_mutating(&op);
        if mutating {
            match self.capture_shot().await {
                Ok(digest) => frame.pre_shot = Some(digest),
                Err((kind, message)) => {
                    return CuaPrepare::Failed {
                        frame,
                        kind,
                        message,
                    };
                }
            }
        }
        CuaPrepare::Ready(CuaPrepared {
            frame,
            op,
            mutating,
        })
    }

    fn note_approved(&self, prepared: &CuaPrepared) {
        // §2.2 first-contact: the seen-set advances ONLY on an approved op,
        // and dies with the session (no cross-session grant store).
        if let Some(app) = &prepared.frame.frontmost_app {
            self.policy.mark_app_seen(app);
        }
    }

    async fn dispatch(
        &self,
        prepared: &CuaPrepared,
        cancel: Option<&AtomicBool>,
    ) -> CuaDispatchOutcome {
        use std::sync::atomic::Ordering;
        if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            return CuaDispatchOutcome::cancelled();
        }
        // §2.5: the backend call runs on a SPAWNED task — a backend panic
        // surfaces as a JoinError and becomes a typed CuaBackend error,
        // never a process abort (the pty_host external-oracle precedent),
        // and the cancel race can abort the in-flight dispatch.
        let backend = self.backend.clone();
        let expected = prepared.frame.frontmost_app.clone();
        let op = prepared.op.clone();
        let mut task = tokio::spawn(async move { backend.dispatch(expected.as_deref(), op).await });
        let result = match cancel {
            Some(flag) => {
                let watcher = async {
                    while !flag.load(Ordering::SeqCst) {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                    }
                };
                tokio::select! {
                    joined = &mut task => Some(joined),
                    _ = watcher => None,
                }
            }
            None => Some((&mut task).await),
        };
        let joined = match result {
            None => {
                // Cancel raced dispatch (the donor tool.rs select! pattern):
                // abort the in-flight call and journal Cancelled.
                task.abort();
                return CuaDispatchOutcome::cancelled();
            }
            Some(joined) => joined,
        };
        let result = match joined {
            Err(join_error) if join_error.is_panic() => {
                return CuaDispatchOutcome::terminal(
                    CuaOutcome::Failed,
                    NanoErrorKind::CuaBackend,
                    "computer-use backend panicked (contained; typed error, no retry)".to_string(),
                );
            }
            Err(_) => return CuaDispatchOutcome::cancelled(), // aborted task
            Ok(result) => result,
        };
        let result = match result {
            Err(err @ CuaError::Cancelled) => {
                let _ = err;
                return CuaDispatchOutcome::cancelled();
            }
            Err(err) => {
                let kind = kind_of_cua_error(&err);
                return CuaDispatchOutcome::terminal(
                    CuaOutcome::Failed,
                    kind,
                    bounded(&format!("{err}")),
                );
            }
            Ok(result) => result,
        };
        // ── success paths ────────────────────────────────────────────────
        match result {
            CuaOpResult::Screenshot {
                format,
                data_b64,
                width,
                height,
                redacted,
            } => {
                use base64::Engine as _;
                let (post_shot, note) = match base64::engine::general_purpose::STANDARD
                    .decode(&data_b64)
                {
                    Ok(bytes) => match self.store.acquire_write_lease() {
                        Ok(lease) => match self.store.put(&lease, &bytes) {
                            Ok(digest) => (Some(digest.clone()), format!("attachment {digest}")),
                            Err(err) => (None, format!("attachment store failed: {err}")),
                        },
                        Err(err) => (None, format!("attachment store failed: {err}")),
                    },
                    Err(_) => (None, "screenshot payload was not valid base64".to_string()),
                };
                // §4.2: one successful screenshot re-arms the resumed turn.
                self.needs_rescreenshot.store(false, Ordering::SeqCst);
                CuaDispatchOutcome {
                    outcome: CuaOutcome::Completed,
                    post_shot,
                    error_kind: None,
                    output: bounded(&format!(
                        "screenshot captured: {width}x{height} {} (redacted={redacted}), {note}",
                        match format {
                            ScreenshotFormat::Png => "png",
                        }
                    )),
                    progress: ProgressSignals {
                        new_information: true,
                        ..Default::default()
                    },
                }
            }
            _ => {
                // Mutating ops take the post-shot (§2.4). A post-shot failure
                // cannot un-dispatch the landed action: the pair journals
                // `completed` with `post_shot` ABSENT and the model-facing
                // text names the evidence gap — never a silent omission.
                let (post_shot, note) = if prepared.mutating {
                    match self.capture_shot().await {
                        Ok(digest) => (Some(digest), String::new()),
                        Err((_, message)) => (
                            None,
                            format!("; post-action screenshot capture failed: {message}"),
                        ),
                    }
                } else {
                    (None, String::new())
                };
                CuaDispatchOutcome {
                    outcome: CuaOutcome::Completed,
                    post_shot,
                    error_kind: None,
                    output: bounded(&format!("{} completed{note}", prepared.frame.op_kind)),
                    progress: ProgressSignals {
                        process_outcome_changed: prepared.mutating,
                        ..Default::default()
                    },
                }
            }
        }
    }
}

/// The eight v1 tool definitions (S9 §1.2 surface). Coordinates are PHYSICAL
/// pixels of the primary display (§5.2/Q6 — one coordinate authority, the
/// same space screenshots capture); out-of-range is an error, never a clamp.
/// Every description states the always-prompt posture honestly.
pub fn cua_tool_definitions() -> Vec<ToolDefinition> {
    let mods = serde_json::json!({
        "type": "object",
        "properties": {
            "shift": {"type": "boolean"},
            "ctrl": {"type": "boolean"},
            "alt": {"type": "boolean"},
            "meta": {"type": "boolean"}
        }
    });
    vec![
        ToolDefinition {
            name: "cua_left_click".into(),
            description: "Click the focused window at physical-pixel coordinates of the primary display (the same space cua_screenshot captures). Every call prompts the user; a focus change between approval and dispatch fails typed. Args: x, y, optional button (left/right/middle), optional mods {shift,ctrl,alt,meta}.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]},
                    "mods": mods
                },
                "required": ["x", "y"]
            }),
        },
        ToolDefinition {
            name: "cua_right_click".into(),
            description: "Right-click the focused window (physical pixels, primary display). Every call prompts the user. Args: x, y, optional mods {shift,ctrl,alt,meta}.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "mods": mods
                },
                "required": ["x", "y"]
            }),
        },
        ToolDefinition {
            name: "cua_double_click".into(),
            description: "Double-click the focused window (physical pixels, primary display). Every call prompts the user. Args: x, y, optional button.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]}
                },
                "required": ["x", "y"]
            }),
        },
        ToolDefinition {
            name: "cua_scroll".into(),
            description: "Scroll the focused window at a point (physical pixels, primary display); dx/dy are wheel deltas. Every call prompts the user. Args: x, y, dx, dy.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "dx": {"type": "integer"},
                    "dy": {"type": "integer"}
                },
                "required": ["x", "y", "dx", "dy"]
            }),
        },
        ToolDefinition {
            name: "cua_type".into(),
            description: "Type text into the focused window as synthesized keystrokes. Every call prompts the user; the prompt shows the exact text. Control characters and forbidden key combos are policy-denied. Args: text.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string"}
                },
                "required": ["text"]
            }),
        },
        ToolDefinition {
            name: "cua_key".into(),
            description: "Press a key combo in the focused window (e.g. \"ctrl+t\"). Every call prompts the user; forbidden combos (secure-attention, lock-screen) are policy-denied. Args: keys, optional mods {shift,ctrl,alt,meta}.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "keys": {"type": "string"},
                    "mods": mods
                },
                "required": ["keys"]
            }),
        },
        ToolDefinition {
            name: "cua_screenshot".into(),
            description: "Capture the focused screen as PNG (heuristic password-field redaction always on). Prompts the user like every computer-use op — screen content is exfiltratable context. The capture is stored as a digest-addressed attachment. Args: optional region {x,y,width,height}, optional redact (default true).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "region": {
                        "type": "object",
                        "properties": {
                            "x": {"type": "integer"},
                            "y": {"type": "integer"},
                            "width": {"type": "integer"},
                            "height": {"type": "integer"}
                        },
                        "required": ["x", "y", "width", "height"]
                    },
                    "redact": {"type": "boolean"}
                }
            }),
        },
        ToolDefinition {
            name: "cua_wait".into(),
            description: "Wait without injecting input (bounded; cancellable). Prompts the user like every computer-use op. Args: duration_ms.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "duration_ms": {"type": "integer"}
                },
                "required": ["duration_ms"]
            }),
        },
    ]
}
