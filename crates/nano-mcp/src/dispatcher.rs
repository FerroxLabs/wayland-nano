//! Full-duplex MCP dispatcher (P3 design note §2 — LOCKED).
//!
//! One [`Connection`] per connected server:
//!
//! ```text
//!  child stdout → reader thread (owns BufReader<ChildStdout>):
//!     read_line (bounded 8 MiB) → parse → validate_frame → classify_frame → route:
//!       Response      → pending map → waiter oneshot
//!       ServerRequest → bounded queue (16) → handler thread → priority lane
//!       Notification  → sink (log + server-side cancel tracking)
//!       bad LINE      → bounded skip + violation counter (poison at 8)
//!       bad ENVELOPE  → poison
//!  child stdin  ← writer thread draining a two-lane bounded queue
//!     (priority 16: responses/cancels; normal 64: client requests),
//!     priority checked before EVERY frame, 4-burst then one interleaved
//!     normal frame; write-progress published for the supervisor watchdog.
//!  supervisor   — the ONLY thread that kills the child and joins threads.
//! ```
//!
//! Invariants (§2.2/§2.3, test-asserted):
//! - `poison()` is state-only and idempotent; it NEVER kills the child and
//!   NEVER joins a thread (a reader can never self-join).
//! - Lock ordering: no lock is ever held across an enqueue or a wait; the
//!   pending lock is never held while touching any queue.
//! - EOF / child-exit / reader/handler/writer panic / queue disconnect all
//!   route through the supervisor's single event channel.

use crate::client::McpError;
use crate::protocol::{
    INTERNAL_ERROR, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, METHOD_NOT_FOUND,
    NegotiatedCapabilities, error_response, result_response,
};
use crate::stdio::TransportParts;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Child;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Reader frame cap (§2.2): generous over MAX_OUTPUT_BYTES since frames
/// carry base64-free JSON.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// An over-length line is drained to its newline, bounded at 4× the frame
/// cap; a line exceeding the drain cap ⇒ poison (§2.2).
pub const DRAIN_CAP_BYTES: usize = 4 * MAX_FRAME_BYTES;
/// Cumulative bad-line / impossible-id violations per connection before
/// poison (§2.2).
pub const VIOLATION_LIMIT: usize = 8;
/// Bounded ring of recently retired (timed-out / cancelled) ids (§2.4).
pub const RETIRED_RING_CAP: usize = 256;
pub const SERVER_REQUEST_QUEUE_CAP: usize = 16;
pub const PRIORITY_LANE_CAP: usize = 16;
pub const NORMAL_LANE_CAP: usize = 64;
/// Bounded priority burst: after this many consecutive priority frames the
/// writer interleaves exactly one waiting normal frame (§2.2/D2).
pub const PRIORITY_BURST: usize = 4;
/// Server-side `notifications/cancelled` tracking bound (§2.5).
pub const CANCELLED_TRACKING_CAP: usize = 256;
/// Per-connection elicitation budget (§2.4 rule 3).
pub const ELICITATION_BUDGET: u64 = 64;
/// The one-time foreground-call extension: ask_entered_at + 300s + 30s
/// grace (§2.4 rule 1).
pub const ASK_EXTENSION: Duration = Duration::from_secs(330);

/// Write-progress watchdog: a frame write incomplete after this is a
/// writer-stall event routed through the supervisor (§2.2/D2).
pub const WRITE_PROGRESS_DEADLINE: Duration = Duration::from_secs(10);
/// Supervisor watch-loop tick (§2.2).
pub const SUPERVISOR_TICK: Duration = Duration::from_secs(1);
/// Graceful shutdown: bounded wait for the child to exit on stdin EOF
/// before the supervisor terminates it (§2.6).
pub const GRACEFUL_SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

const WRITER_IDLE_TICK: Duration = Duration::from_millis(25);
const HANDLER_TICK: Duration = Duration::from_millis(50);
const GRACEFUL_POLL: Duration = Duration::from_millis(25);
/// Handler-reply drain retry: 500 × 1ms = 500ms bounded wait for a healthy
/// writer to drain a transiently full priority lane (then poison, §2.2).
const HANDLER_DRAIN_RETRIES: usize = 500;
const HANDLER_DRAIN_STEP: Duration = Duration::from_millis(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Frame validation and classification (§2.2, BEFORE any routing)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// method + id — a server-initiated request we must answer.
    ServerRequest,
    /// method only.
    Notification,
    /// id only — a response to one of our requests.
    Response,
}

/// JSON-RPC envelope validation (§2.2): `jsonrpc == "2.0"`; `method`, when
/// present, a string; `id`, when present, a string or an integer; a response
/// (no `method`) carries exactly one of `result`/`error`, `error` an object
/// with integer `code` and string `message`. Any failure is protocol
/// corruption ⇒ the caller poisons; it is NEVER a skip.
pub fn validate_frame(value: &Value) -> Result<(), String> {
    let obj = value.as_object().ok_or("frame is not a JSON object")?;
    match obj.get("jsonrpc") {
        Some(v) if v.as_str() == Some(crate::protocol::JSONRPC_VERSION) => {}
        _ => return Err("\"jsonrpc\" missing or not \"2.0\"".into()),
    }
    if let Some(method) = obj.get("method") {
        if !method.is_string() {
            return Err("\"method\" is not a string".into());
        }
    }
    if let Some(id) = obj.get("id") {
        let legal = id.is_string() || id.as_i64().is_some() || id.as_u64().is_some();
        if !legal {
            return Err("\"id\" is not a string or integer".into());
        }
    }
    let has_method = obj.contains_key("method");
    let has_id = obj.contains_key("id");
    if !has_method && !has_id {
        return Err("frame carries neither method nor id".into());
    }
    if !has_method {
        let has_result = obj.contains_key("result");
        let has_error = obj.contains_key("error");
        if has_result == has_error {
            return Err("response carries both/neither result and error".into());
        }
        if has_error {
            let ok = obj["error"].as_object().is_some_and(|e| {
                e.get("code").is_some_and(|c| c.as_i64().is_some())
                    && e.get("message").is_some_and(|m| m.is_string())
            });
            if !ok {
                return Err("error is not an object with integer code + string message".into());
            }
        }
    }
    Ok(())
}

/// Routes a validated frame on shape (§2.2). Call ONLY after
/// [`validate_frame`] has accepted the frame.
pub fn classify_frame(value: &Value) -> FrameKind {
    match (value.get("method").is_some(), value.get("id").is_some()) {
        (true, true) => FrameKind::ServerRequest,
        (true, false) => FrameKind::Notification,
        (false, true) => FrameKind::Response,
        (false, false) => unreachable!("validate_frame rejects method-less, id-less frames"),
    }
}

// ---------------------------------------------------------------------------
// Cross-lane seams (§5.2's handler installs here; §2.4's slot is enforced here)
// ---------------------------------------------------------------------------

/// A server-initiated JSON-RPC request (method + id), queued for the one
/// handler thread.
#[derive(Debug, Clone)]
pub struct ServerRequest {
    pub id: Value,
    pub method: String,
    pub params: Option<Value>,
}

/// What the §2.4 slot-retirement hook needs to emit the terminal declines:
/// the designated call id and whether an ask was open (ANSWERING).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRetired {
    pub call_id: u64,
    pub was_answering: bool,
}

/// Server-request handler seam (§2.5/§5.2). The elicitation lane installs
/// its bridge here. Return `Some(Ok(result))` for a result reply,
/// `Some(Err((code, message)))` for an error reply, `None` to fall through
/// to the spec-legal `-32601` method-not-found.
pub trait ServerRequestHandler: Send + Sync {
    fn handle(
        &self,
        conn: &ConnectionHandle,
        request: &ServerRequest,
    ) -> Option<Result<Value, (i64, String)>>;

    /// Whether installing this handler means the client should advertise
    /// the `elicitation` capability at initialize (§2.7 — advertised only
    /// when a handler actually exists; the honesty rule on the handshake).
    fn advertises_elicitation(&self) -> bool {
        false
    }
}

/// Default handler: nothing served, everything falls through to `-32601`
/// (ping is answered by the dispatcher core itself, §2.5).
struct NoServerRequests;

impl ServerRequestHandler for NoServerRequests {
    fn handle(
        &self,
        _conn: &ConnectionHandle,
        _request: &ServerRequest,
    ) -> Option<Result<Value, (i64, String)>> {
        None
    }
}

/// Per-connection options: sinks, handler seam, and the watchdog clocks
/// (shortened in tests).
pub struct ConnectionOptions {
    /// Notification sink (§2.5): v1 is structured log only; no notification
    /// mutates engine state.
    pub notification_sink: Arc<dyn Fn(&JsonRpcNotification) + Send + Sync>,
    pub request_handler: Arc<dyn ServerRequestHandler>,
    /// Invoked exactly once when the foreground slot retires (§2.4 rule 4);
    /// the elicitation lane uses it for the terminal per-queued declines.
    pub slot_retired_hook: Arc<dyn Fn(SlotRetired) + Send + Sync>,
    pub write_progress_deadline: Duration,
    pub supervisor_tick: Duration,
    pub graceful_shutdown_wait: Duration,
    /// Test-only fault injection: the reader panics after processing N
    /// frames, to prove the supervisor path (§12 (i)).
    #[doc(hidden)]
    pub reader_panic_after_frames: Option<u64>,
}

impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            notification_sink: Arc::new(|n: &JsonRpcNotification| {
                // Bounded by construction: the method name only, never the
                // (server-authored, unbounded) params.
                eprintln!("wayland-nano mcp: server notification: {}", n.method);
            }),
            request_handler: Arc::new(NoServerRequests),
            slot_retired_hook: Arc::new(|_| {}),
            write_progress_deadline: WRITE_PROGRESS_DEADLINE,
            supervisor_tick: SUPERVISOR_TICK,
            graceful_shutdown_wait: GRACEFUL_SHUTDOWN_WAIT,
            reader_panic_after_frames: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Pending map (§2.2/§2.4) — every mutation goes through the enforcing fns
// ---------------------------------------------------------------------------

type WaiterTx = SyncSender<Result<Value, McpError>>;

/// A waiter's absolute deadline, set once at issue and extendable at most
/// once (§2.4 rule 1 — the one-time elicitation-open extension).
pub(crate) struct DeadlineCell {
    inner: Mutex<(Instant, bool)>,
}

impl DeadlineCell {
    pub(crate) fn new(deadline: Instant) -> Self {
        Self {
            inner: Mutex::new((deadline, false)),
        }
    }

    pub(crate) fn deadline(&self) -> Instant {
        lock(&self.inner).0
    }

    /// At most ONE extension is ever granted (§2.4 rule 1). Returns false
    /// if the extension was already consumed.
    fn try_extend_once(&self, new_deadline: Instant) -> bool {
        let mut inner = lock(&self.inner);
        if inner.1 {
            return false;
        }
        inner.0 = new_deadline;
        inner.1 = true;
        true
    }
}

pub(crate) struct PendingSlot {
    pub(crate) tx: WaiterTx,
    pub(crate) deadline: Arc<DeadlineCell>,
}

/// Foreground-slot designation state machine (§2.4 rule 4):
/// FREE → DESIGNATED → ANSWERING → RETIRED (terminal; returns to FREE only
/// when the server-request queue is drained).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ForegroundSlot {
    #[default]
    Free,
    Designated(u64),
    Answering(u64),
    Retired,
}

#[derive(Default)]
struct PendingState {
    map: HashMap<u64, PendingSlot>,
    /// Recently retired ids (timed-out / cancelled) — bounded ring (§2.4).
    retired: VecDeque<u64>,
    slot: ForegroundSlot,
}

impl PendingState {
    fn retire_slot_locked(&mut self, id: u64) -> Option<SlotRetired> {
        let report = match self.slot {
            ForegroundSlot::Designated(call_id) if call_id == id => Some(SlotRetired {
                call_id,
                was_answering: false,
            }),
            ForegroundSlot::Answering(call_id) if call_id == id => Some(SlotRetired {
                call_id,
                was_answering: true,
            }),
            _ => None,
        };
        if report.is_some() {
            self.slot = ForegroundSlot::Retired;
        }
        report
    }

    /// DESIGNATED before dispatch (§2.4 rule 2): at most one designated
    /// call per connection; fails while the slot is held.
    fn designate(&mut self, id: u64) -> bool {
        if self.slot != ForegroundSlot::Free {
            return false;
        }
        self.slot = ForegroundSlot::Designated(id);
        true
    }

    /// Accept decision for an arriving elicitation: Some(call id) when a
    /// designated call holds the slot (DESIGNATED → ANSWERING), None when
    /// FREE or RETIRED (§2.4 rule 2 — never a guessed parent).
    fn accept(&mut self) -> Option<u64> {
        match self.slot {
            ForegroundSlot::Designated(id) => {
                self.slot = ForegroundSlot::Answering(id);
                Some(id)
            }
            ForegroundSlot::Answering(id) => Some(id),
            ForegroundSlot::Free | ForegroundSlot::Retired => None,
        }
    }

    /// RETIRED → FREE only when the server-request queue is drained (§2.4
    /// rule 4); the handler calls this when it observes an empty queue.
    fn free_if_retired(&mut self) {
        if self.slot == ForegroundSlot::Retired {
            self.slot = ForegroundSlot::Free;
        }
    }

    /// Caller timeout / cancellation / shutdown: remove the waiter, join
    /// the bounded retired ring, retire the slot (§2.4). Returns the taken
    /// slot so the shutdown sweep can still fail the waiter typed.
    fn retire(&mut self, id: u64) -> (Option<PendingSlot>, Option<SlotRetired>) {
        let Some(slot) = self.map.remove(&id) else {
            return (None, None);
        };
        self.retired.push_back(id);
        while self.retired.len() > RETIRED_RING_CAP {
            self.retired.pop_front();
        }
        let report = self.retire_slot_locked(id);
        (Some(slot), report)
    }
}

/// Where a response id lands (§2.4 late/retired/impossible-id discipline).
enum Disposition {
    Pending(PendingSlot),
    /// Recently retired (timed-out / cancelled): bounded drop + log.
    Retired,
    /// Never-issued, future, or duplicate-of-answered: protocol violation.
    Unknown,
}

// ---------------------------------------------------------------------------
// Supervisor events (§2.3) — every teardown cause routes through here
// ---------------------------------------------------------------------------

enum SupEvent {
    /// `poison()` was called (state already set); tear down.
    Poisoned,
    /// Graceful close (facade drop / close()).
    Shutdown,
    /// Child stdout EOF — a closed pipe is a dead server.
    StdoutEof,
    ReaderPanicked,
    HandlerPanicked,
    WriterFailed(String),
    /// Both writer lanes disconnected outside a shutdown.
    WriterDone,
    HandlerQueueDisconnected,
}

impl SupEvent {
    fn poison_reason(&self) -> String {
        match self {
            SupEvent::Poisoned => "connection poisoned".into(),
            SupEvent::Shutdown => "connection shut down".into(),
            SupEvent::StdoutEof => "child stdout EOF (dead server)".into(),
            SupEvent::ReaderPanicked => "reader thread panicked".into(),
            SupEvent::HandlerPanicked => "handler thread panicked".into(),
            SupEvent::WriterFailed(e) => format!("writer failed: {e}"),
            SupEvent::WriterDone => "writer queue disconnected".into(),
            SupEvent::HandlerQueueDisconnected => "handler queue disconnected".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct ConnState {
    /// First-writer-wins poison reason (§2.3).
    poison: Option<String>,
    /// Graceful close in progress: refuse new requests, drain, exit.
    closing: bool,
}

struct Shared {
    state: Mutex<ConnState>,
    pending: Mutex<PendingState>,
    /// Server-cancelled server-request ids (§2.5), Value::to_string form.
    cancelled_requests: Mutex<HashSet<String>>,
    next_id: AtomicU64,
    violations: AtomicUsize,
    overflow_replies: AtomicU64,
    elicitations_handled: AtomicU64,
    /// Millis since `epoch` when the current frame write started; 0 = idle.
    write_started_ms: AtomicU64,
    epoch: Instant,
    prio_tx: SyncSender<String>,
    normal_tx: SyncSender<String>,
    server_req_tx: SyncSender<ServerRequest>,
    events_tx: Sender<SupEvent>,
    sink: Arc<dyn Fn(&JsonRpcNotification) + Send + Sync>,
    handler: Arc<dyn ServerRequestHandler>,
    slot_retired_hook: Arc<dyn Fn(SlotRetired) + Send + Sync>,
    reader_panic_after_frames: Option<u64>,
}

impl Shared {
    // --- poison (§2.3): state-only, idempotent, first-writer-wins --------

    fn poison(&self, reason: &str) {
        {
            let mut state = lock(&self.state);
            if state.poison.is_some() {
                return;
            }
            state.poison = Some(reason.to_string());
        }
        self.pending_fail_all(reason);
        // The server-request queue is "closed" by the poison flag itself:
        // the handler drains remaining items with a best-effort -32603.
        let _ = self.events_tx.send(SupEvent::Poisoned);
    }

    fn poisoned_reason(&self) -> Option<String> {
        lock(&self.state).poison.clone()
    }

    fn poisoned(&self) -> bool {
        lock(&self.state).poison.is_some()
    }

    fn closing(&self) -> bool {
        lock(&self.state).closing
    }

    fn set_closing(&self) {
        lock(&self.state).closing = true;
    }

    // --- pending map enforcing functions (§2.2) --------------------------

    fn pending_insert(&self, id: u64, slot: PendingSlot) {
        lock(&self.pending).map.insert(id, slot);
    }

    /// Terminal path for a DELIVERED response. Slot retirement rides the
    /// same path (§2.4 rule 4); the hook fires after the lock is dropped.
    fn pending_take(&self, id: u64) -> Option<PendingSlot> {
        let mut retired = None;
        let taken = {
            let mut pending = lock(&self.pending);
            let taken = pending.map.remove(&id);
            if taken.is_some() {
                retired = pending.retire_slot_locked(id);
            }
            taken
        };
        if let Some(report) = retired {
            (self.slot_retired_hook)(report);
        }
        taken
    }

    /// Terminal path for caller timeout / cancellation: the id joins the
    /// bounded retired ring so the late response is a drop+log, never a
    /// cross-talk (§2.4). Returns the taken slot (the shutdown sweep fails
    /// its waiter typed; the timeout/cancel paths hold their own receiver).
    fn pending_retire(&self, id: u64) -> Option<PendingSlot> {
        let (slot, retired_hook) = {
            let mut pending = lock(&self.pending);
            pending.retire(id)
        };
        if let Some(report) = retired_hook {
            (self.slot_retired_hook)(report);
        }
        slot
    }

    /// Poison path: every waiter fails typed. Sends happen AFTER the map
    /// lock is dropped (no lock held across a wait-channel touch).
    fn pending_fail_all(&self, reason: &str) {
        let senders: Vec<WaiterTx> = {
            let mut pending = lock(&self.pending);
            pending.map.drain().map(|(_, slot)| slot.tx).collect()
        };
        for tx in senders {
            let _ = tx.try_send(Err(McpError::Transport(reason.to_string())));
        }
    }

    /// Where a response id lands (§2.4): delivered via `pending_take` (the
    /// map-mutation enforcing function), retired ring ⇒ drop+log, else the
    /// impossible-id violation class.
    fn response_disposition(&self, id: u64) -> Disposition {
        if let Some(slot) = self.pending_take(id) {
            return Disposition::Pending(slot);
        }
        if lock(&self.pending).retired.contains(&id) {
            Disposition::Retired
        } else {
            Disposition::Unknown
        }
    }

    // --- foreground slot (§2.4 rules 2/4) --------------------------------

    /// DESIGNATED before dispatch (§2.4 rule 2): at most one designated
    /// call per connection; fails when the slot is held (the call then runs
    /// as an ordinary non-interactive call on its absolute clock).
    fn designate_slot(&self, id: u64) -> bool {
        lock(&self.pending).designate(id)
    }

    /// The handler's accept decision for an arriving elicitation (§2.4
    /// rule 2).
    fn accept_elicitation(&self) -> Option<u64> {
        lock(&self.pending).accept()
    }

    /// RETIRED → FREE only when the server-request queue is drained (§2.4
    /// rule 4); the handler calls this when it observes an empty queue.
    fn slot_free_if_retired(&self) {
        lock(&self.pending).free_if_retired();
    }

    /// The one-time absolute extension for the designated call entering an
    /// ask wait: ask_entered_at + 300s + 30s grace (§2.4 rule 1).
    fn note_ask_entered(&self, call_id: u64) {
        let cell = {
            let pending = lock(&self.pending);
            pending.map.get(&call_id).map(|slot| slot.deadline.clone())
        };
        if let Some(cell) = cell {
            cell.try_extend_once(Instant::now() + ASK_EXTENSION);
        }
    }

    /// Per-connection elicitation budget (§2.4 rule 3).
    fn take_elicitation_budget(&self) -> bool {
        self.elicitations_handled.fetch_add(1, Ordering::SeqCst) < ELICITATION_BUDGET
    }

    // --- violations (§2.2 two-class framing) ------------------------------

    /// Bad-line / impossible-id accounting: bounded, poison at the limit.
    fn note_violation(&self, what: &str) {
        eprintln!("wayland-nano mcp: protocol violation: {what}");
        if self.violations.fetch_add(1, Ordering::SeqCst) + 1 >= VIOLATION_LIMIT {
            self.poison(&format!(
                "violation budget exhausted ({VIOLATION_LIMIT} bad lines/impossible ids)"
            ));
        }
    }

    // --- writer lanes (§2.2/D2) -------------------------------------------

    /// Normal lane (client requests). A full lane is a typed failure for
    /// the enqueueing caller (§2.2) — the caller pending_retires its id.
    fn enqueue_normal(&self, frame: String) -> Result<(), McpError> {
        if let Some(reason) = self.poisoned_reason() {
            return Err(McpError::Transport(reason));
        }
        if self.closing() {
            return Err(McpError::Transport("connection closing".into()));
        }
        match self.normal_tx.try_send(frame) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(McpError::Transport("writer queue full".into())),
            Err(TrySendError::Disconnected(_)) => {
                Err(McpError::Transport("writer thread gone".into()))
            }
        }
    }

    /// Priority lane (server-request responses, `notifications/cancelled`).
    /// A full priority lane ⇒ poison (§2.2).
    fn enqueue_priority(&self, frame: String) -> Result<(), McpError> {
        if let Some(reason) = self.poisoned_reason() {
            return Err(McpError::Transport(reason));
        }
        match self.prio_tx.try_send(frame) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.poison("priority lane full");
                Err(McpError::Transport("priority lane full".into()))
            }
            Err(TrySendError::Disconnected(_)) => {
                Err(McpError::Transport("writer thread gone".into()))
            }
        }
    }

    /// Handler-reply variant: the HANDLER thread (never the reader, §2.2/D3)
    /// may wait out a transiently full lane — a healthy writer drains 16
    /// frames far inside this bound, and §12 (h) requires a full-queue drain
    /// (16 queued replies + the in-handler one) to flow with a 16-cap lane.
    /// Still full after the bound ⇒ the writer is stalled ⇒ poison (§2.2).
    fn enqueue_priority_handler(&self, mut frame: String) {
        if self.poisoned() {
            return;
        }
        for _ in 0..HANDLER_DRAIN_RETRIES {
            match self.prio_tx.try_send(frame) {
                Ok(()) => return,
                Err(TrySendError::Full(returned)) => {
                    frame = returned;
                    std::thread::sleep(HANDLER_DRAIN_STEP);
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        self.poison("priority lane full (handler replies)");
    }
}

// ---------------------------------------------------------------------------
// ConnectionHandle — what the handler seam may touch (§5.2's bridge)
// ---------------------------------------------------------------------------

/// The handler-side handle to the connection: enqueue replies on the
/// priority lane, drive the §2.4 foreground slot, consume the elicitation
/// budget. Deliberately NO shutdown/child access.
#[derive(Clone)]
pub struct ConnectionHandle {
    shared: Arc<Shared>,
}

impl ConnectionHandle {
    /// §2.4 rule 2 accept decision; see [`Shared::accept_elicitation`].
    pub fn accept_elicitation(&self) -> Option<u64> {
        self.shared.accept_elicitation()
    }

    /// The one-time deadline extension when the handler enters an ask wait
    /// (§2.4 rule 1).
    pub fn note_ask_entered(&self, call_id: u64) {
        self.shared.note_ask_entered(call_id);
    }

    /// §2.4 rule 3 per-connection budget (64); false ⇒ auto-decline the
    /// overflow with `-32603`.
    pub fn take_elicitation_budget(&self) -> bool {
        self.shared.take_elicitation_budget()
    }

    /// Enqueue a reply frame on the priority lane; a full lane poisons
    /// (§2.2).
    pub fn enqueue_priority(&self, frame: String) -> Result<(), McpError> {
        self.shared.enqueue_priority(frame)
    }

    pub fn poisoned_reason(&self) -> Option<String> {
        self.shared.poisoned_reason()
    }
}

// ---------------------------------------------------------------------------
// The two-lane writer scheduler (§2.2/D2): priority checked before EVERY
// frame, bounded 4-burst then one interleaved normal frame.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct LaneScheduler {
    burst: usize,
    prio_open: bool,
    normal_open: bool,
}

impl LaneScheduler {
    fn new() -> Self {
        Self {
            burst: 0,
            prio_open: true,
            normal_open: true,
        }
    }

    /// Non-blocking pick honoring the drain discipline. None ⇒ nothing
    /// available right now on either lane.
    fn try_frame(&mut self, prio: &Receiver<String>, normal: &Receiver<String>) -> Option<String> {
        // Bounded burst: after PRIORITY_BURST consecutive priority frames,
        // interleave exactly ONE waiting normal frame before returning to
        // the priority lane.
        if self.burst >= PRIORITY_BURST && self.normal_open {
            match normal.try_recv() {
                Ok(frame) => {
                    self.burst = 0;
                    return Some(frame);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.normal_open = false,
            }
        }
        // Priority checked before EVERY frame.
        if self.prio_open {
            match prio.try_recv() {
                Ok(frame) => {
                    self.burst += 1;
                    return Some(frame);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.prio_open = false,
            }
        }
        if self.normal_open {
            match normal.try_recv() {
                Ok(frame) => {
                    self.burst = 0;
                    return Some(frame);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.normal_open = false,
            }
        }
        None
    }

    /// Blocking pick: parks on the NORMAL lane when idle (client requests
    /// are the common case; priority traffic tolerates one idle tick).
    /// `stop` is the poison flag — checked every tick so a poisoned
    /// connection never parks the writer. None ⇒ both lanes disconnected
    /// and drained, or stop requested.
    fn next_frame(
        &mut self,
        prio: &Receiver<String>,
        normal: &Receiver<String>,
        stop: &dyn Fn() -> bool,
    ) -> Option<String> {
        loop {
            if stop() {
                return None;
            }
            if let Some(frame) = self.try_frame(prio, normal) {
                return Some(frame);
            }
            if !self.prio_open && !self.normal_open {
                return None;
            }
            // Both lanes empty, at least one open: park briefly.
            if self.normal_open {
                match normal.recv_timeout(WRITER_IDLE_TICK) {
                    Ok(frame) => {
                        self.burst = 0;
                        return Some(frame);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => self.normal_open = false,
                }
            } else {
                match prio.recv_timeout(WRITER_IDLE_TICK) {
                    Ok(frame) => {
                        self.burst += 1;
                        return Some(frame);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => self.prio_open = false,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reader thread (§2.2) — owns BufReader<ChildStdout>
// ---------------------------------------------------------------------------

enum LineRead {
    Line(String),
    Eof,
    /// Over-length line drained to its newline within the drain cap:
    /// bad-line class (bounded skip + counter).
    OverlengthDrained,
    /// Over-length line exceeded the drain cap: stream position is
    /// unrecoverable ⇒ poison.
    DrainBreach,
}

/// Bounded read of one line (§2.2): `take(MAX+1)` + `read_line`; overflow
/// drains bounded at 4× the frame cap.
fn read_bounded_line(reader: &mut impl BufRead) -> std::io::Result<LineRead> {
    let mut buf = String::new();
    let over_length = {
        let mut take = (&mut *reader).take(MAX_FRAME_BYTES as u64 + 1);
        let n = take.read_line(&mut buf)?;
        if n == 0 {
            return Ok(LineRead::Eof);
        }
        take.limit() == 0 && !buf.ends_with('\n')
    };
    if !over_length {
        return Ok(LineRead::Line(buf));
    }
    // Bounded drain to the newline (§2.2).
    let mut drained = buf.len();
    loop {
        let mut chunk = String::new();
        let n = {
            let mut take = (&mut *reader).take(8192);
            take.read_line(&mut chunk)?
        };
        if n == 0 {
            // Server died mid-line: the EOF arm handles teardown.
            return Ok(LineRead::Eof);
        }
        drained += n;
        if chunk.ends_with('\n') {
            return Ok(LineRead::OverlengthDrained);
        }
        if drained > DRAIN_CAP_BYTES {
            return Ok(LineRead::DrainBreach);
        }
    }
}

fn reader_main(shared: &Shared, mut stdout: impl BufRead) {
    let mut frames_seen: u64 = 0;
    loop {
        if shared.poisoned() {
            return;
        }
        let line = match read_bounded_line(&mut stdout) {
            Ok(line) => line,
            Err(err) => {
                eprintln!("wayland-nano mcp: reader io error: {err}");
                let _ = shared.events_tx.send(SupEvent::StdoutEof);
                return;
            }
        };
        match line {
            LineRead::Eof => {
                let _ = shared.events_tx.send(SupEvent::StdoutEof);
                return;
            }
            LineRead::DrainBreach => {
                shared.poison("over-length line exceeded the drain cap");
                return;
            }
            LineRead::OverlengthDrained => {
                shared.note_violation("over-length line drained to newline");
                continue;
            }
            LineRead::Line(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                frames_seen += 1;
                if shared.reader_panic_after_frames == Some(frames_seen) {
                    panic!("injected reader fault after {frames_seen} frames");
                }
                // Bad LINES are a different class from bad FRAMES (§2.2):
                // unparseable (or non-object) JSON is skipped + counted.
                let value: Value = match serde_json::from_str(trimmed) {
                    Ok(value) => value,
                    Err(_) => {
                        shared.note_violation("unparseable line skipped");
                        continue;
                    }
                };
                if !value.is_object() {
                    shared.note_violation("non-object json line skipped");
                    continue;
                }
                // Envelope validation BEFORE routing; failure ⇒ poison.
                if let Err(reason) = validate_frame(&value) {
                    shared.poison(&format!("envelope-invalid frame: {reason}"));
                    return;
                }
                match classify_frame(&value) {
                    FrameKind::Response => route_response(shared, value),
                    FrameKind::Notification => route_notification(shared, value),
                    FrameKind::ServerRequest => route_server_request(shared, value),
                }
            }
        }
    }
}

fn route_response(shared: &Shared, value: Value) {
    // Our ids are integers; a response carrying anything else can never be
    // ours (§2.4 impossible-id class).
    let Some(id) = value.get("id").and_then(|i| i.as_u64()) else {
        shared.note_violation("response with a non-integer id");
        return;
    };
    match shared.response_disposition(id) {
        Disposition::Pending(slot) => {
            // Typed-deserialize of an envelope-valid frame; failure ⇒ poison.
            match serde_json::from_value::<JsonRpcResponse>(value) {
                Ok(response) => {
                    // A closed receiver is a timed-out caller: the
                    // late-response arm (§2.4) — drop + bounded log.
                    if slot.tx.try_send(response.into_result()).is_err() {
                        eprintln!("wayland-nano mcp: late response for id {id} dropped");
                    }
                }
                Err(err) => {
                    let reason =
                        format!("typed-deserialize failed for an envelope-valid response: {err}");
                    // Poison fails every waiter still in the map; THIS
                    // waiter's entry was already taken, so fail it here —
                    // no caller is left parked behind a poison (§2.3).
                    shared.poison(&reason);
                    let _ = slot.tx.try_send(Err(McpError::Transport(reason)));
                }
            }
        }
        Disposition::Retired => {
            // Recently retired id (timed-out / cancelled): bounded drop+log.
            eprintln!("wayland-nano mcp: response for retired id {id} dropped");
        }
        Disposition::Unknown => {
            // Never-issued, future, or duplicate-of-answered id.
            shared.note_violation("response for a never-issued/duplicate id");
        }
    }
}

fn route_notification(shared: &Shared, value: Value) {
    let notification: JsonRpcNotification = match serde_json::from_value(value) {
        Ok(notification) => notification,
        Err(err) => {
            shared.poison(&format!(
                "typed-deserialize failed for an envelope-valid notification: {err}"
            ));
            return;
        }
    };
    if notification.method == "notifications/cancelled" {
        // The server cancelling ITS OWN earlier request: the queued item is
        // dropped by the handler (§2.5).
        if let Some(request_id) = notification
            .params
            .as_ref()
            .and_then(|p| p.get("requestId"))
        {
            let mut cancelled = lock(&shared.cancelled_requests);
            if cancelled.len() < CANCELLED_TRACKING_CAP {
                cancelled.insert(request_id.to_string());
            }
        }
    }
    // tools/list_changed / resources/list_changed and everything else:
    // logged, never acted on in v1 (§2.5).
    (shared.sink)(&notification);
}

fn route_server_request(shared: &Shared, value: Value) {
    let request: JsonRpcRequest = match serde_json::from_value(value) {
        Ok(request) => request,
        Err(err) => {
            shared.poison(&format!(
                "typed-deserialize failed for an envelope-valid server request: {err}"
            ));
            return;
        }
    };
    let server_request = ServerRequest {
        id: request.id,
        method: request.method,
        params: request.params,
    };
    match shared.server_req_tx.try_send(server_request) {
        Ok(()) => {}
        Err(TrySendError::Full(server_request)) => {
            // The reader NEVER blocks on a full queue (§2.2/D3): a blocked
            // reader deadlocks the pipe. Overflow ⇒ -32603 via the priority
            // lane + metric.
            shared.overflow_replies.fetch_add(1, Ordering::SeqCst);
            let reply = error_response(
                server_request.id,
                INTERNAL_ERROR,
                "server request queue full",
            );
            let _ = shared
                .enqueue_priority(serde_json::to_string(&reply).expect("error reply serializes"));
        }
        Err(TrySendError::Disconnected(_)) => {
            shared.poison("handler queue disconnected");
        }
    }
}

// ---------------------------------------------------------------------------
// Handler thread (§2.2/D3) — ONE thread, FIFO, answers on the priority lane
// ---------------------------------------------------------------------------

/// The enforcing function for §2.5: known v1 methods are served, everything
/// else gets the spec-legal `-32601` (fail-closed but polite — a silent drop
/// can deadlock a server awaiting reply).
fn dispatch_server_request(handle: &ConnectionHandle, request: &ServerRequest) -> Value {
    if request.method == "ping" {
        return result_response(request.id.clone(), serde_json::json!({}));
    }
    match handle.shared.handler.handle(handle, request) {
        Some(Ok(result)) => result_response(request.id.clone(), result),
        Some(Err((code, message))) => error_response(request.id.clone(), code, &message),
        None => error_response(request.id.clone(), METHOD_NOT_FOUND, "method not found"),
    }
}

fn handler_main(shared: &Arc<Shared>, rx: &Receiver<ServerRequest>) {
    let handle = ConnectionHandle {
        shared: shared.clone(),
    };
    loop {
        match rx.recv_timeout(HANDLER_TICK) {
            Ok(request) => {
                // The server cancelled its own earlier request: drop the
                // queued item, no reply (§2.5).
                let cancelled =
                    { lock(&shared.cancelled_requests).remove(&request.id.to_string()) };
                if cancelled {
                    continue;
                }
                if shared.poisoned() || shared.closing() {
                    // Drained items get a best-effort -32603 (§2.3).
                    let reply = error_response(request.id, INTERNAL_ERROR, "connection closing");
                    let _ = shared
                        .enqueue_priority(serde_json::to_string(&reply).expect("reply serializes"));
                    continue;
                }
                let reply = dispatch_server_request(&handle, &request);
                // Replies ride the priority lane (§2.2); the handler waits
                // out a transiently full lane within a bounded retry, then
                // poisons if the writer is genuinely stalled.
                shared.enqueue_priority_handler(
                    serde_json::to_string(&reply).expect("reply serializes"),
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                // The queue is empty at this instant: a RETIRED foreground
                // slot returns to FREE only when the queue is drained
                // (§2.4 rule 4).
                shared.slot_free_if_retired();
                if shared.poisoned() || shared.closing() {
                    // Final non-blocking drain, then exit.
                    while let Ok(request) = rx.try_recv() {
                        let reply =
                            error_response(request.id, INTERNAL_ERROR, "connection closing");
                        let _ = shared.enqueue_priority(
                            serde_json::to_string(&reply).expect("reply serializes"),
                        );
                    }
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = shared.events_tx.send(SupEvent::HandlerQueueDisconnected);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Writer thread (§2.2/D2) — owns ChildStdin exclusively; no mutex on the pipe
// ---------------------------------------------------------------------------

fn write_frame(shared: &Shared, stdin: &mut impl Write, frame: &str) -> Result<(), String> {
    // Publish write-progress BEFORE the blocking write; the supervisor's
    // watchdog (§2.2) treats a write incomplete after
    // WRITE_PROGRESS_DEADLINE as a writer-stall event.
    shared.write_started_ms.store(
        shared.epoch.elapsed().as_millis().min(u64::MAX as u128) as u64,
        Ordering::SeqCst,
    );
    let result = writeln!(stdin, "{frame}").and_then(|()| stdin.flush());
    shared.write_started_ms.store(0, Ordering::SeqCst);
    result.map_err(|e| format!("write: {e}"))
}

fn writer_main(
    shared: &Arc<Shared>,
    prio_rx: &Receiver<String>,
    normal_rx: &Receiver<String>,
    mut stdin: impl Write,
) {
    let mut scheduler = LaneScheduler::new();
    let mut quiet_observed = false;
    loop {
        if shared.poisoned() {
            return;
        }
        let frame = if shared.closing() {
            // Graceful close: drain both lanes non-blockingly; exit after a
            // quiet period (one idle tick with nothing to write) so cancels
            // enqueued by the shutdown sweep land on the wire first.
            match scheduler.try_frame(prio_rx, normal_rx) {
                Some(frame) => {
                    quiet_observed = false;
                    frame
                }
                None if quiet_observed => break,
                None => {
                    quiet_observed = true;
                    std::thread::sleep(WRITER_IDLE_TICK);
                    continue;
                }
            }
        } else {
            // `stop` must cover BOTH poison and closing: a frame arriving
            // is what moves the writer from next_frame back to the outer
            // loop, and on a graceful close of an idle connection no frame
            // ever arrives — without this the park below is a deadlock.
            let stop = || shared.poisoned() || shared.closing();
            match scheduler.next_frame(prio_rx, normal_rx, &stop) {
                Some(frame) => frame,
                None => {
                    if shared.closing() && !shared.poisoned() {
                        continue; // fall into the closing drain branch
                    }
                    break; // both lanes disconnected and drained
                }
            }
        };
        if let Err(err) = write_frame(shared, &mut stdin, &frame) {
            let _ = shared.events_tx.send(SupEvent::WriterFailed(err));
            return;
        }
    }
    // `stdin` drops here, closing the child's stdin (a well-behaved server
    // exits on EOF, §2.6). Outside a shutdown this is the queue-disconnect
    // event (§2.3).
    let _ = shared.events_tx.send(SupEvent::WriterDone);
}

// ---------------------------------------------------------------------------
// Supervisor (§2.3) — SOLE owner of child-kill and thread joins
// ---------------------------------------------------------------------------

struct SupervisorConfig {
    tick: Duration,
    write_progress_deadline: Duration,
    graceful_shutdown_wait: Duration,
}

fn kill_child(child: &mut Option<Child>) {
    // §2.6: the contained spawn lane swaps this bare kill for the job-object
    // terminate; the supervisor remains the only caller.
    if let Some(mut child) = child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn join_all(handles: &mut Option<Vec<JoinHandle<()>>>) {
    if let Some(handles) = handles.take() {
        for handle in handles {
            let _ = handle.join();
        }
    }
}

fn supervisor_main(
    shared: &Arc<Shared>,
    events_rx: &Receiver<SupEvent>,
    mut child: Option<Child>,
    mut handles: Option<Vec<JoinHandle<()>>>,
    config: &SupervisorConfig,
) {
    loop {
        match events_rx.recv_timeout(config.tick) {
            Ok(SupEvent::Shutdown) => {
                // Graceful (§2.6): stdin is already closed by the writer's
                // drain-exit; wait bounded for the child to exit, then
                // terminate, then join. No poison — this is a clean close.
                let deadline = Instant::now() + config.graceful_shutdown_wait;
                loop {
                    let exited = child
                        .as_mut()
                        .and_then(|c| c.try_wait().ok().flatten())
                        .is_some();
                    if exited || Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(GRACEFUL_POLL);
                }
                kill_child(&mut child);
                join_all(&mut handles);
                return;
            }
            Ok(event) => {
                shared.poison(&event.poison_reason());
                kill_child(&mut child);
                join_all(&mut handles);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                // Write-progress watchdog (§2.2/D2): a frame write
                // incomplete after WRITE_PROGRESS_DEADLINE is a stall
                // event; terminating the child closes the pipe's read end
                // and unblocks the parked syscall with an error.
                let started = shared.write_started_ms.load(Ordering::SeqCst);
                if started != 0 {
                    let now = shared.epoch.elapsed().as_millis().min(u64::MAX as u128) as u64;
                    if now.saturating_sub(started)
                        > config.write_progress_deadline.as_millis() as u64
                    {
                        shared.poison("writer stalled past WRITE_PROGRESS_DEADLINE");
                        kill_child(&mut child);
                        join_all(&mut handles);
                        return;
                    }
                }
                // Child-exit reaping (§2.3): the supervisor owns the child
                // handle, so exit is observed here on the watch tick.
                let exited = child
                    .as_mut()
                    .and_then(|c| c.try_wait().ok().flatten())
                    .is_some();
                if exited {
                    shared.poison("child process exited");
                    kill_child(&mut child);
                    join_all(&mut handles);
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                shared.poison("supervisor event channel disconnected");
                kill_child(&mut child);
                join_all(&mut handles);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Connection — the facade's handle (§2.2)
// ---------------------------------------------------------------------------

/// One full-duplex connection to an MCP server. Clone-cheap state lives in
/// `Arc<Shared>`; this handle owns the supervisor's join handle and is the
/// graceful-shutdown entry point. Dropping the LAST handle shuts the
/// connection down (§2.6).
pub struct Connection {
    shared: Arc<Shared>,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    negotiated: OnceLock<NegotiatedCapabilities>,
}

impl Connection {
    pub fn spawn(parts: TransportParts, options: ConnectionOptions) -> Arc<Connection> {
        let (prio_tx, prio_rx) = mpsc::sync_channel::<String>(PRIORITY_LANE_CAP);
        let (normal_tx, normal_rx) = mpsc::sync_channel::<String>(NORMAL_LANE_CAP);
        let (server_req_tx, server_req_rx) =
            mpsc::sync_channel::<ServerRequest>(SERVER_REQUEST_QUEUE_CAP);
        let (events_tx, events_rx) = mpsc::channel::<SupEvent>();

        let shared = Arc::new(Shared {
            state: Mutex::new(ConnState {
                poison: None,
                closing: false,
            }),
            pending: Mutex::new(PendingState::default()),
            cancelled_requests: Mutex::new(HashSet::new()),
            next_id: AtomicU64::new(1),
            violations: AtomicUsize::new(0),
            overflow_replies: AtomicU64::new(0),
            elicitations_handled: AtomicU64::new(0),
            write_started_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            prio_tx,
            normal_tx,
            server_req_tx,
            events_tx,
            sink: options.notification_sink,
            handler: options.request_handler,
            slot_retired_hook: options.slot_retired_hook,
            reader_panic_after_frames: options.reader_panic_after_frames,
        });

        // Reader: owns stdout. Panics are caught at the thread boundary and
        // routed to the supervisor — the reader NEVER joins itself (§2.3).
        let reader_shared = shared.clone();
        let mut stdout = parts.stdout;
        let reader = std::thread::Builder::new()
            .name("nano-mcp-reader".into())
            .spawn(move || {
                let caught = catch_unwind(AssertUnwindSafe(|| {
                    reader_main(&reader_shared, &mut stdout)
                }));
                if caught.is_err() {
                    let _ = reader_shared.events_tx.send(SupEvent::ReaderPanicked);
                }
            })
            .expect("spawn reader thread");

        // Handler: drains the bounded server-request queue FIFO.
        let handler_shared = shared.clone();
        let handler = std::thread::Builder::new()
            .name("nano-mcp-handler".into())
            .spawn(move || {
                let caught = catch_unwind(AssertUnwindSafe(|| {
                    handler_main(&handler_shared, &server_req_rx)
                }));
                if caught.is_err() {
                    let _ = handler_shared.events_tx.send(SupEvent::HandlerPanicked);
                }
            })
            .expect("spawn handler thread");

        // Writer: owns stdin, drains the two-lane bounded queue.
        let writer_shared = shared.clone();
        let mut stdin = parts.stdin;
        let writer = std::thread::Builder::new()
            .name("nano-mcp-writer".into())
            .spawn(move || {
                let caught = catch_unwind(AssertUnwindSafe(|| {
                    writer_main(&writer_shared, &prio_rx, &normal_rx, &mut stdin)
                }));
                if caught.is_err() {
                    let _ = writer_shared
                        .events_tx
                        .send(SupEvent::WriterFailed("writer thread panicked".into()));
                }
            })
            .expect("spawn writer thread");

        // Supervisor: sole owner of the child handle and all joins.
        let supervisor_shared = shared.clone();
        let supervisor_config = SupervisorConfig {
            tick: options.supervisor_tick,
            write_progress_deadline: options.write_progress_deadline,
            graceful_shutdown_wait: options.graceful_shutdown_wait,
        };
        let supervisor = std::thread::Builder::new()
            .name("nano-mcp-supervisor".into())
            .spawn(move || {
                supervisor_main(
                    &supervisor_shared,
                    &events_rx,
                    Some(parts.child),
                    Some(vec![reader, handler, writer]),
                    &supervisor_config,
                );
            })
            .expect("spawn supervisor thread");

        Arc::new(Connection {
            shared,
            supervisor: Mutex::new(Some(supervisor)),
            negotiated: OnceLock::new(),
        })
    }

    /// Monotonic per-connection request id (§2.2).
    pub(crate) fn mint_id(&self) -> u64 {
        self.shared.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn pending_insert(&self, id: u64, slot: PendingSlot) {
        self.shared.pending_insert(id, slot);
    }

    pub(crate) fn pending_retire(&self, id: u64) {
        self.shared.pending_retire(id);
    }

    pub(crate) fn designate_slot(&self, id: u64) -> bool {
        self.shared.designate_slot(id)
    }

    pub(crate) fn enqueue_normal(&self, frame: String) -> Result<(), McpError> {
        self.shared.enqueue_normal(frame)
    }

    pub fn enqueue_priority(&self, frame: String) -> Result<(), McpError> {
        self.shared.enqueue_priority(frame)
    }

    /// The same typed error every subsequent facade call returns after
    /// poison, without touching the child (§2.3).
    pub fn poisoned_reason(&self) -> Option<String> {
        self.shared.poisoned_reason()
    }

    /// Bad-line / impossible-id violations so far (§2.2 counter; the
    /// metric the note asks to count).
    pub fn violations(&self) -> usize {
        self.shared.violations.load(Ordering::SeqCst)
    }

    /// `-32603` overflow replies emitted for a flooded server-request queue.
    pub fn overflow_replies(&self) -> u64 {
        self.shared.overflow_replies.load(Ordering::SeqCst)
    }

    pub(crate) fn set_negotiated(&self, negotiated: NegotiatedCapabilities) {
        let _ = self.negotiated.set(negotiated);
    }

    /// The recorded initialize-time capabilities (§2.7) — the enforcing
    /// record for the §4.2/§5.5 gates.
    pub fn negotiated(&self) -> Option<&NegotiatedCapabilities> {
        self.negotiated.get()
    }

    pub(crate) fn advertises_elicitation(&self) -> bool {
        self.shared.handler.advertises_elicitation()
    }

    /// Graceful shutdown (§2.6): refuse new requests, send
    /// `notifications/cancelled` for each pending id (best-effort), close
    /// stdin via the writer's drain-exit, bounded wait, then the supervisor
    /// terminates the child and joins the threads. Idempotent.
    pub fn shutdown(&self) {
        self.shared.set_closing();
        // Retire each pending id (so the late response is a retired
        // drop+log), fail its waiter typed FIRST so no caller parks behind
        // the graceful wait, then cancel it on the wire (§2.6).
        let ids: Vec<u64> = lock(&self.shared.pending).map.keys().copied().collect();
        for id in ids {
            if let Some(slot) = self.shared.pending_retire(id) {
                let _ = slot
                    .tx
                    .try_send(Err(McpError::Transport("connection shut down".into())));
            }
            let notification =
                crate::protocol::cancelled_notification(serde_json::json!(id), "shutdown");
            let _ = self.shared.enqueue_priority(
                serde_json::to_string(&notification).expect("notification serializes"),
            );
        }
        let _ = self.shared.events_tx.send(SupEvent::Shutdown);
        if let Some(handle) = lock(&self.supervisor).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Unit tests (pure halves of the §12 battery; the live-process legs are in
// tests/dispatcher_battery.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    // --- validate_frame (§2.2 envelope rules) ------------------------------

    #[test]
    fn validate_accepts_the_three_legal_shapes() {
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":1,"result":{}})).is_ok());
        assert!(
            validate_frame(&json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no"}}))
                .is_ok()
        );
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":"srv-1","method":"ping"})).is_ok());
        assert!(
            validate_frame(&json!({"jsonrpc":"2.0","method":"notifications/progress","params":{}}))
                .is_ok()
        );
        // String and integer ids are the legal set.
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":"abc","result":{}})).is_ok());
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":-3,"result":{}})).is_ok());
    }

    #[test]
    fn validate_rejects_envelope_violations() {
        // Missing / wrong jsonrpc.
        assert!(validate_frame(&json!({"id":1,"result":{}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":"1.0","id":1,"result":{}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":2.0,"id":1,"result":{}})).is_err());
        // Non-string method.
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":1,"method":42})).is_err());
        // Illegal id types: float, boolean, null, object, array.
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":1.5,"result":{}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":true,"result":{}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":null,"result":{}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":{},"result":{}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":[],"result":{}})).is_err());
        // Result/error exclusivity.
        assert!(
            validate_frame(
                &json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":-1,"message":"x"}})
            )
            .is_err()
        );
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":1})).is_err());
        // Error shape: integer code + string message.
        assert!(
            validate_frame(&json!({"jsonrpc":"2.0","id":1,"error":{"code":"x","message":"x"}}))
                .is_err()
        );
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":1,"error":{"code":-1}})).is_err());
        assert!(validate_frame(&json!({"jsonrpc":"2.0","id":1,"error":"nope"})).is_err());
        // Neither method nor id.
        assert!(validate_frame(&json!({"jsonrpc":"2.0","params":{}})).is_err());
    }

    // --- classify_frame (§2.2 shape routing) --------------------------------

    #[test]
    fn classify_routes_on_shape() {
        assert_eq!(
            classify_frame(&json!({"jsonrpc":"2.0","id":1,"method":"elicitation/create"})),
            FrameKind::ServerRequest
        );
        assert_eq!(
            classify_frame(&json!({"jsonrpc":"2.0","method":"notifications/progress"})),
            FrameKind::Notification
        );
        assert_eq!(
            classify_frame(&json!({"jsonrpc":"2.0","id":1,"result":{}})),
            FrameKind::Response
        );
    }

    // --- deny_unknown_fields (§2.2/D1, the §2.1.1 regression pin) -----------

    #[test]
    fn response_type_rejects_unknown_fields_and_server_requests() {
        // An extra field on an envelope-valid response: typed-deserialize
        // failure ⇒ the reader poisons (asserted end-to-end in the battery).
        let extra = json!({"jsonrpc":"2.0","id":1,"result":{},"surprise":true});
        assert!(serde_json::from_value::<JsonRpcResponse>(extra).is_err());
        // The §2.1.1 defect: a server REQUEST must NOT deserialize as a
        // valid response with result/error both None.
        let server_request =
            json!({"jsonrpc":"2.0","id":1,"method":"elicitation/create","params":{}});
        assert!(serde_json::from_value::<JsonRpcResponse>(server_request).is_err());
    }

    // --- LaneScheduler drain discipline (§2.2/D2) ---------------------------

    fn lanes() -> (
        SyncSender<String>,
        Receiver<String>,
        SyncSender<String>,
        Receiver<String>,
    ) {
        let (pt, pr) = mpsc::sync_channel(PRIORITY_LANE_CAP);
        let (nt, nr) = mpsc::sync_channel(NORMAL_LANE_CAP);
        (pt, pr, nt, nr)
    }

    #[test]
    fn priority_checked_before_every_frame() {
        let (pt, pr, nt, nr) = lanes();
        nt.try_send("n1".into()).unwrap();
        pt.try_send("p1".into()).unwrap();
        let mut s = LaneScheduler::new();
        let never = || false;
        assert_eq!(s.next_frame(&pr, &nr, &never).as_deref(), Some("p1"));
        assert_eq!(s.next_frame(&pr, &nr, &never).as_deref(), Some("n1"));
    }

    #[test]
    fn priority_burst_of_4_interleaves_one_normal_frame() {
        let (pt, pr, nt, nr) = lanes();
        for i in 1..=6 {
            pt.try_send(format!("p{i}")).unwrap();
        }
        for i in 1..=2 {
            nt.try_send(format!("n{i}")).unwrap();
        }
        let mut s = LaneScheduler::new();
        let never = || false;
        let order: Vec<String> = (0..8)
            .map(|_| s.next_frame(&pr, &nr, &never).unwrap())
            .collect();
        // 4 priority, ONE normal interleaved, then priority again; the
        // second normal drains when the priority lane empties.
        assert_eq!(order, vec!["p1", "p2", "p3", "p4", "n1", "p5", "p6", "n2"]);
    }

    #[test]
    fn normal_lane_not_starved_by_priority_flood() {
        let (pt, pr, nt, nr) = lanes();
        nt.try_send("n1".into()).unwrap();
        let mut s = LaneScheduler::new();
        let never = || false;
        // A fresh priority frame before every pick: the normal frame must
        // still land after the first 4-burst.
        let mut saw_normal_at = None;
        for pick in 1..=10 {
            pt.try_send(format!("p{pick}")).unwrap();
            let frame = s.next_frame(&pr, &nr, &never).unwrap();
            if frame == "n1" {
                saw_normal_at = Some(pick);
                break;
            }
        }
        assert_eq!(saw_normal_at, Some(5));
    }

    #[test]
    fn scheduler_returns_none_when_both_lanes_closed_and_drained() {
        let (pt, pr, nt, nr) = lanes();
        pt.try_send("p1".into()).unwrap();
        let mut s = LaneScheduler::new();
        let never = || false;
        drop(pt);
        drop(nt);
        assert_eq!(s.next_frame(&pr, &nr, &never).as_deref(), Some("p1"));
        assert!(s.next_frame(&pr, &nr, &never).is_none());
    }

    // --- DeadlineCell (§2.4 rule 1: set once, extended at most once) --------

    #[test]
    fn deadline_extends_at_most_once() {
        let cell = DeadlineCell::new(Instant::now() + Duration::from_secs(30));
        let extended = Instant::now() + Duration::from_secs(330);
        assert!(cell.try_extend_once(extended));
        assert_eq!(cell.deadline(), extended);
        assert!(!cell.try_extend_once(extended + Duration::from_secs(330)));
        assert_eq!(cell.deadline(), extended);
    }

    // --- PendingState: retired ring + foreground slot (§2.4) -----------------

    fn slot_for(_id: u64) -> PendingSlot {
        let (tx, _rx) = mpsc::sync_channel(1);
        PendingSlot {
            tx,
            deadline: Arc::new(DeadlineCell::new(Instant::now() + Duration::from_secs(30))),
        }
    }

    #[test]
    fn retired_ring_is_bounded_and_drops_oldest() {
        let mut pending = PendingState::default();
        for id in 1..=300u64 {
            pending.map.insert(id, slot_for(id));
            pending.retire(id);
        }
        assert_eq!(pending.retired.len(), RETIRED_RING_CAP);
        assert!(!pending.retired.contains(&1));
        assert!(pending.retired.contains(&300));
    }

    #[test]
    fn foreground_slot_state_machine() {
        let mut pending = PendingState::default();
        // FREE: unattributable elicitation rejected.
        assert_eq!(pending.accept(), None);
        // Designate one call; a second designation fails while held.
        pending.map.insert(7, slot_for(7));
        assert!(pending.designate(7));
        assert!(!pending.designate(8));
        // DESIGNATED → ANSWERING on accept; re-accept keeps the same call.
        assert_eq!(pending.accept(), Some(7));
        assert_eq!(pending.accept(), Some(7));
        // Terminal event on the designated call ⇒ RETIRED (terminal).
        let (_, report) = pending.retire(7);
        assert_eq!(
            report,
            Some(SlotRetired {
                call_id: 7,
                was_answering: true
            })
        );
        // After retirement: no acceptance, no re-designation, until the
        // queue-drained observation frees the slot.
        assert_eq!(pending.accept(), None);
        assert!(!pending.designate(9));
        pending.free_if_retired();
        pending.map.insert(9, slot_for(9));
        assert!(pending.designate(9));
    }

    #[test]
    fn retire_reports_designated_not_answering() {
        let mut pending = PendingState::default();
        pending.map.insert(3, slot_for(3));
        assert!(pending.designate(3));
        let (_, report) = pending.retire(3);
        assert_eq!(
            report,
            Some(SlotRetired {
                call_id: 3,
                was_answering: false
            })
        );
    }

    #[test]
    fn retire_of_non_designated_call_leaves_slot_held() {
        let mut pending = PendingState::default();
        pending.map.insert(1, slot_for(1));
        pending.map.insert(2, slot_for(2));
        assert!(pending.designate(1));
        // A concurrent ordinary call resolving does NOT retire the slot.
        assert_eq!(pending.retire(2).1, None);
        assert_eq!(pending.accept(), Some(1));
    }

    // --- read_bounded_line (§2.2 line bound + bounded drain) -----------------

    fn read_all(reader: &mut Cursor<Vec<u8>>) -> Vec<LineRead> {
        let mut out = Vec::new();
        loop {
            match read_bounded_line(reader).unwrap() {
                LineRead::Eof => break,
                other => out.push(other),
            }
        }
        out
    }

    #[test]
    fn normal_lines_and_eof() {
        let mut cursor = Cursor::new(b"{\"a\":1}\n{\"b\":2}\n".to_vec());
        let reads = read_all(&mut cursor);
        assert!(matches!(
            reads.as_slice(),
            [LineRead::Line(_), LineRead::Line(_)]
        ));
    }

    #[test]
    fn line_at_exactly_the_cap_is_accepted() {
        let mut line = "x".repeat(MAX_FRAME_BYTES);
        line.push('\n');
        let mut cursor = Cursor::new(line.into_bytes());
        assert!(matches!(
            read_bounded_line(&mut cursor).unwrap(),
            LineRead::Line(_)
        ));
    }

    #[test]
    fn over_length_line_drains_within_cap() {
        let mut line = "x".repeat(MAX_FRAME_BYTES + 100);
        line.push('\n');
        let mut cursor = Cursor::new(line.into_bytes());
        assert!(matches!(
            read_bounded_line(&mut cursor).unwrap(),
            LineRead::OverlengthDrained
        ));
    }

    #[test]
    fn line_beyond_drain_cap_breaches() {
        let line = "x".repeat(MAX_FRAME_BYTES + DRAIN_CAP_BYTES + 1);
        let mut cursor = Cursor::new(line.into_bytes());
        assert!(matches!(
            read_bounded_line(&mut cursor).unwrap(),
            LineRead::DrainBreach
        ));
    }

    #[test]
    fn partial_line_at_eof_is_a_line_then_eof() {
        let mut cursor = Cursor::new(b"partial".to_vec());
        assert!(matches!(
            read_bounded_line(&mut cursor).unwrap(),
            LineRead::Line(_)
        ));
        assert!(matches!(
            read_bounded_line(&mut cursor).unwrap(),
            LineRead::Eof
        ));
    }
}
