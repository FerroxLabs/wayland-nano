//! §12 dispatcher battery: scripted fake server (the
//! `wayland-nano-mcp-fake-server` probe bin) against the real
//! `Connection`/`McpClient` — threads, pipes, watchdogs, and all.

use nano_mcp::client::{McpClient, McpError};
use nano_mcp::dispatcher::{
    ConnectionHandle, ConnectionOptions, ServerRequest, ServerRequestHandler,
};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::time::{Duration, Instant};

const FAKE: &str = env!("CARGO_BIN_EXE_wayland-nano-mcp-fake-server");

fn connect(scenario: &str, env: &[(&str, &str)], options: ConnectionOptions) -> McpClient {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    McpClient::connect_with_options(FAKE, &[scenario.to_string()], &env, options)
        .unwrap_or_else(|e| panic!("connect to fake server ({scenario}): {e}"))
}

fn connect_default(scenario: &str) -> McpClient {
    connect(scenario, &[], ConnectionOptions::default())
}

/// A notification sink collecting into a shared vec (the `fake/obs`
/// channel plus everything else).
#[derive(Clone, Default)]
struct Collect {
    seen: Arc<Mutex<Vec<Value>>>,
}

impl Collect {
    fn sink(&self) -> Arc<dyn Fn(&nano_mcp::protocol::JsonRpcNotification) + Send + Sync> {
        let seen = self.seen.clone();
        Arc::new(move |n| {
            seen.lock()
                .unwrap()
                .push(serde_json::json!({"method": n.method, "params": n.params}));
        })
    }

    fn wait_for(&self, pred: impl Fn(&Value) -> bool, deadline: Duration) -> Option<Value> {
        let start = Instant::now();
        while start.elapsed() < deadline {
            {
                let seen = self.seen.lock().unwrap();
                if let Some(v) = seen.iter().find(|v| pred(v)) {
                    return Some(v.clone());
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn count(&self, pred: impl Fn(&Value) -> bool) -> usize {
        self.seen.lock().unwrap().iter().filter(|v| pred(v)).count()
    }
}

fn is_cancel_obs(v: &Value, reason: &str) -> bool {
    v["method"] == "fake/obs"
        && v["params"]["event"] == "saw_cancel"
        && v["params"]["reason"] == reason
}

/// §12 (b2) handler for the flood legs: parks every server request until
/// the test opens the gate, so the 16-cap queue fills deterministically.
/// On the FIRST parked request it reports `probe/flood_go` to the fake
/// server: that handshake is what pins the handler/queue split (handler
/// holds srv-1, queue empty) before the remaining 17 requests are sent,
/// so the overflow count is exact on any scheduler.
struct GatedHandler {
    gate: Arc<(Mutex<bool>, Condvar)>,
    announced: AtomicBool,
}

impl ServerRequestHandler for GatedHandler {
    fn handle(
        &self,
        conn: &ConnectionHandle,
        _request: &ServerRequest,
    ) -> Option<Result<Value, (i64, String)>> {
        if !self.announced.swap(true, Ordering::SeqCst) {
            let go = serde_json::json!({"jsonrpc":"2.0","method":"probe/flood_go"});
            let _ = conn.enqueue_priority(go.to_string());
        }
        let (lock, cvar) = &*self.gate;
        let mut open = lock.lock().unwrap();
        while !*open {
            open = cvar.wait(open).unwrap();
        }
        Some(Ok(serde_json::json!({"ok": true})))
    }
}

fn gated_options() -> (ConnectionOptions, Arc<(Mutex<bool>, Condvar)>) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let options = ConnectionOptions {
        request_handler: Arc::new(GatedHandler {
            gate: gate.clone(),
            announced: AtomicBool::new(false),
        }),
        ..Default::default()
    };
    (options, gate)
}

fn open(gate: &Arc<(Mutex<bool>, Condvar)>) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

// ---------------------------------------------------------------------------

#[test]
fn handshake_records_capabilities_and_caches_tools() {
    let client = connect_default("echo");
    let negotiated = client.negotiated().expect("negotiated capabilities");
    assert_eq!(negotiated.protocol_version, "2025-06-18");
    // The fake advertises no server capabilities and we advertise no
    // elicitation without a handler (honesty rule on the handshake).
    assert!(!negotiated.tools);
    assert!(!negotiated.resources);
    assert!(!negotiated.elicitation);
    assert!(!negotiated.deferred_tools);
    assert_eq!(client.cached_tools().len(), 1);
    assert_eq!(client.cached_tools()[0].name, "echo");
    client.close();
}

/// §12 (a): a server REQUEST mid-call is classified ServerRequest, queued,
/// answered — never parsed as a response (the §2.1.1 regression).
#[test]
fn server_request_mid_call_is_answered_not_misparsed() {
    let client = connect_default("inject-request");
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .expect("call completes");
    assert_eq!(result["got_reply"], true, "ping must be answered {{}}");
    assert_eq!(result["had_result"], true);
    client.close();
}

/// §12 (d): unknown method ⇒ spec-legal -32601 on the wire, call
/// unaffected.
#[test]
fn unknown_server_method_gets_32601() {
    let client = connect_default("inject-bogus");
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .expect("call completes");
    assert_eq!(result["got_reply"], true);
    assert_eq!(result["error_code"], -32601);
    client.close();
}

/// §12 (b)/(b2): two concurrent calls on the SAME server, answered out of
/// order — each waiter gets its own response.
#[test]
fn out_of_order_concurrent_calls_same_server() {
    let client = connect_default("threaded");
    let slow = client.clone();
    let a = std::thread::spawn(move || {
        slow.call_tool("echo", serde_json::json!({"delay_ms": 400, "marker": "a"}))
    });
    std::thread::sleep(Duration::from_millis(50));
    let b = client.call_tool("echo", serde_json::json!({"delay_ms": 0, "marker": "b"}));
    assert_eq!(b.expect("b")["marker"], "b");
    assert_eq!(a.join().unwrap().expect("a")["marker"], "a");
    client.close();
}

/// §12 (c): a notification mid-call is sink-logged; the call is unaffected.
#[test]
fn notification_mid_call_logged_call_unaffected() {
    let collect = Collect::default();
    let options = ConnectionOptions {
        notification_sink: collect.sink(),
        ..Default::default()
    };
    let client = connect("notify-mid-call", &[], options);
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .expect("call");
    assert_eq!(result["ok"], true);
    let seen = collect
        .wait_for(
            |v| v["method"] == "notifications/progress",
            Duration::from_secs(2),
        )
        .expect("progress notification logged");
    assert_eq!(seen["params"]["n"], 1);
    client.close();
}

/// §12 (e): unparseable lines are skipped and counted; the call is
/// unaffected.
#[test]
fn unparseable_lines_skipped_counted_call_unaffected() {
    let client = connect(
        "garbage",
        &[("FAKE_GARBAGE_N", "3")],
        ConnectionOptions::default(),
    );
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .expect("call");
    assert_eq!(result["ok"], true);
    assert_eq!(client.violations(), 3);
    assert!(client.poisoned_reason().is_none());
    client.close();
}

/// §12 (e): the violation budget poisons at 8 cumulative violations.
#[test]
fn eight_violations_poison() {
    let client = connect(
        "garbage",
        &[("FAKE_GARBAGE_N", "8")],
        ConnectionOptions::default(),
    );
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(
        matches!(err, McpError::Transport(_)),
        "poison ⇒ typed transport failure, got {err}"
    );
    let reason = client.poisoned_reason().expect("poisoned");
    assert!(reason.contains("violation budget"), "reason: {reason}");
    client.close();
}

/// §12 (e): an envelope-invalid frame poisons IMMEDIATELY and the pending
/// waiter fails typed — for every bad-envelope shape.
#[test]
fn envelope_invalid_frame_poisons() {
    for kind in ["version", "both", "float", "method", "neither"] {
        let client = connect(
            "bad-envelope",
            &[("FAKE_BAD_KIND", kind)],
            ConnectionOptions::default(),
        );
        let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, McpError::Transport(_)),
            "[{kind}] poison ⇒ typed transport failure, got {err}"
        );
        let reason = client.poisoned_reason().expect("[{kind}] poisoned");
        assert!(
            reason.contains("envelope-invalid"),
            "[{kind}] reason: {reason}"
        );
        client.close();
    }
}

/// §12 (e): an over-length line is drained to its newline (bounded) and
/// counted as a bad line — not an instant poison.
#[test]
fn overlength_line_drained_counted() {
    let client = connect_default("big-line");
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .expect("call");
    assert_eq!(result["ok"], true);
    assert_eq!(client.violations(), 1);
    assert!(client.poisoned_reason().is_none());
    client.close();
}

/// §12 (e): a line beyond the 4× drain cap poisons (stream position
/// unrecoverable).
#[test]
fn drain_cap_breach_poisons() {
    let client = connect_default("huge-line");
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(matches!(err, McpError::Transport(_)), "got {err}");
    let reason = client.poisoned_reason().expect("poisoned");
    assert!(reason.contains("drain cap"), "reason: {reason}");
    client.close();
}

/// §12 (e2): a response for a NEVER-ISSUED id is violation-counted; the
/// in-flight call is unaffected.
#[test]
fn never_issued_id_violation_counted() {
    for env in [
        &[("FAKE_MYSTERY_N", "1")][..],
        &[("FAKE_MYSTERY_N", "1"), ("FAKE_MYSTERY_STRING", "1")][..],
    ] {
        let client = connect("mystery-ids", env, ConnectionOptions::default());
        let result = client
            .call_tool("echo", serde_json::json!({}))
            .expect("call");
        assert_eq!(result["ok"], true);
        assert_eq!(client.violations(), 1);
        assert!(client.poisoned_reason().is_none());
        client.close();
    }
}

/// §12 (e2): an unknown-id flood hits the same budget — poison, no
/// unbounded log growth.
#[test]
fn unknown_id_flood_poisons() {
    let client = connect(
        "mystery-ids",
        &[("FAKE_MYSTERY_N", "8")],
        ConnectionOptions::default(),
    );
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(matches!(err, McpError::Transport(_)), "got {err}");
    let reason = client.poisoned_reason().expect("poisoned");
    assert!(reason.contains("violation budget"), "reason: {reason}");
    client.close();
}

/// §12 (e2): a duplicate response for a just-answered id is
/// violation-counted (the first response won the call).
#[test]
fn duplicate_response_violation_counted() {
    let client = connect_default("duplicate");
    let result = client
        .call_tool("echo", serde_json::json!({}))
        .expect("call");
    assert_eq!(result["ok"], true);
    // The duplicate rides the pipe behind the real answer; give the reader
    // a bounded moment to process it.
    let start = Instant::now();
    while client.violations() == 0 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.violations(), 1);
    assert!(client.poisoned_reason().is_none());
    client.close();
}

/// §12 (f): a silent server fails the call with typed McpTimeout on the
/// caller's clock (the §2.1.4 unbounded hang is dead).
#[test]
fn silent_server_typed_timeout() {
    let client = connect_default("silent").with_timeout(Duration::from_millis(200));
    let start = Instant::now();
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(matches!(err, McpError::Timeout(200)), "got {err}");
    assert!(start.elapsed() < Duration::from_secs(5));
    client.close();
}

/// §12 (g), timeout half: on caller timeout the id is retired,
/// `notifications/cancelled {reason:"timeout"}` goes out, and the late
/// response is dropped via the retired arm — NOT violation-counted.
#[test]
fn timeout_retires_id_and_late_response_drops() {
    let collect = Collect::default();
    let options = ConnectionOptions {
        notification_sink: collect.sink(),
        ..Default::default()
    };
    let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "600")], options)
        .with_timeout(Duration::from_millis(150));
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(matches!(err, McpError::Timeout(150)), "got {err}");
    collect
        .wait_for(|v| is_cancel_obs(v, "timeout"), Duration::from_secs(5))
        .expect("server saw notifications/cancelled with reason=timeout");
    // The late answer lands at ~600ms; bounded wait, then assert it was a
    // retired drop+log, never a violation.
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(client.violations(), 0);
    assert!(client.poisoned_reason().is_none());
    client.close();
}

/// §12 (g), cancel half: turn cancellation is terminal — typed Cancelled,
/// `notifications/cancelled {reason:"cancelled"}` on the wire, late
/// response dropped.
#[test]
fn cancel_mid_call_is_terminal() {
    let collect = Collect::default();
    let options = ConnectionOptions {
        notification_sink: collect.sink(),
        ..Default::default()
    };
    let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "600")], options);
    let cancel = AtomicBool::new(false);
    let outcome = std::thread::scope(|scope| {
        let call =
            scope.spawn(|| client.call_tool_cancellable("echo", serde_json::json!({}), &cancel));
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::SeqCst);
        call.join().unwrap()
    });
    let err = outcome.unwrap_err();
    assert!(matches!(err, McpError::Cancelled), "got {err}");
    collect
        .wait_for(|v| is_cancel_obs(v, "cancelled"), Duration::from_secs(5))
        .expect("server saw notifications/cancelled with reason=cancelled");
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(client.violations(), 0);
    client.close();
}

/// §12 (h): a server-request flood over the 16-cap queue ⇒ exactly one
/// `-32603` overflow reply, the reader NEVER blocks, every queued request
/// is still answered.
#[test]
fn flood_overflow_32603_reader_never_blocked() {
    let (options, gate) = gated_options();
    let client = connect("flood", &[], options);
    let call_client = client.clone();
    let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
    // Deterministic by construction: the fake server sends srv-1 alone and
    // waits for the handler's probe/flood_go (handler parked, queue empty)
    // before sending srv-2..=srv-18 — so 16 fill the queue and srv-18
    // overflows, exactly one -32603, on ANY scheduler.
    let start = Instant::now();
    while client.overflow_replies() == 0 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.overflow_replies(), 1, "exactly one overflow reply");
    open(&gate);
    let result = call.join().unwrap().expect("call completes");
    assert_eq!(result["results"], 17, "every queued request answered");
    assert_eq!(result["overflow"], 1);
    client.close();
}

/// §2.5: the server cancelling ITS OWN queued request drops it — no reply
/// for the cancelled id, everything else still answered. The fake server
/// sends the cancel AFTER the flood (wire order), so the reader records it
/// strictly after all enqueue/overflow decisions: the overflow count stays
/// exactly one (same pinned ordering as the flood leg above) and srv-5 is
/// dropped at handler-dequeue time.
#[test]
fn server_cancel_drops_queued_request() {
    let (options, gate) = gated_options();
    let client = connect("flood", &[("FAKE_CANCEL", "srv-5")], options);
    let call_client = client.clone();
    let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
    let start = Instant::now();
    while client.overflow_replies() == 0 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(client.overflow_replies(), 1);
    open(&gate);
    let result = call.join().unwrap().expect("call completes");
    assert_eq!(result["results"], 16);
    assert_eq!(result["overflow"], 1);
    let answered: Vec<String> = serde_json::from_value(result["answered_ids"].clone()).unwrap();
    assert!(!answered.iter().any(|id| id == "srv-5"));
    client.close();
}

/// §12 (i): a reader-thread panic is caught at the boundary, routed through
/// the supervisor (poison → kill → join — the reader NEVER self-joins), and
/// the pending waiter fails typed without a hang.
#[test]
fn reader_panic_poisons_via_supervisor_no_hang() {
    let options = ConnectionOptions {
        // Frames 1/2 are initialize + tools/list; the call's response is 3.
        reader_panic_after_frames: Some(3),
        supervisor_tick: Duration::from_millis(50),
        ..Default::default()
    };
    let client = connect("echo", &[], options);
    let start = Instant::now();
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(matches!(err, McpError::Transport(_)), "got {err}");
    assert!(start.elapsed() < Duration::from_secs(10));
    let reason = client.poisoned_reason().expect("poisoned");
    assert!(reason.contains("reader"), "reason: {reason}");
    // Every subsequent call returns the SAME typed error without touching
    // the child (a poisoned connection never half-lives, §2.3).
    let again = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert_eq!(again.to_string(), err.to_string());
    assert!(start.elapsed() < Duration::from_secs(10));
    client.close();
}

/// §12 (k): child crash mid-call ⇒ supervisor teardown, all waiters fail
/// typed, and the connection stays dead (same typed error, no child touch).
#[test]
fn child_crash_mid_call_fails_waiters_typed() {
    let client = connect_default("die-on-call");
    let start = Instant::now();
    let err = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(matches!(err, McpError::Transport(_)), "got {err}");
    assert!(start.elapsed() < Duration::from_secs(10));
    let reason = client.poisoned_reason().expect("poisoned");
    assert!(
        reason.contains("EOF") || reason.contains("exited"),
        "reason: {reason}"
    );
    let again = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert_eq!(again.to_string(), err.to_string());
    client.close();
}

/// §12 (j): writer backpressure — a server that never reads its stdin
/// parks the writer mid-frame; the supervisor's write-progress watchdog
/// fires at WRITE_PROGRESS_DEADLINE and every waiter resolves typed within
/// the wall-clock bound. A call enqueued onto the full normal lane fails
/// typed immediately.
#[test]
fn writer_backpressure_wall_clock_bound() {
    let options = ConnectionOptions {
        write_progress_deadline: Duration::from_millis(700),
        supervisor_tick: Duration::from_millis(100),
        ..Default::default()
    };
    let client = connect("blind", &[], options);
    // Park the writer: one frame far larger than any OS pipe buffer.
    let big = client.clone();
    let parked = std::thread::spawn(move || {
        big.call_tool(
            "echo",
            serde_json::json!({"pad": "x".repeat(16 * 1024 * 1024)}),
        )
    });
    std::thread::sleep(Duration::from_millis(200));
    // Fill the normal lane behind the parked write.
    let mut waiters = Vec::new();
    for _ in 0..64 {
        let c = client.clone();
        waiters.push(std::thread::spawn(move || {
            c.call_tool("echo", serde_json::json!({}))
        }));
    }
    // The 65th overflows the lane: typed failure for the enqueueing caller.
    let overflow = client.call_tool("echo", serde_json::json!({})).unwrap_err();
    assert!(
        matches!(&overflow, McpError::Transport(r) if r.contains("writer queue full") || r.contains("stalled") || r.contains("EOF") || r.contains("exited")),
        "got {overflow}"
    );
    // The watchdog must resolve every waiter well inside the 30s
    // recv_timeout backstop (§2.2: max(10s deadline, 30s) + ε — here the
    // deadline is shortened to 700ms).
    let start = Instant::now();
    let parked_result = parked.join().unwrap();
    assert!(matches!(parked_result, Err(McpError::Transport(_))));
    for waiter in waiters {
        assert!(matches!(
            waiter.join().unwrap(),
            Err(McpError::Transport(_))
        ));
    }
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "all waiters resolved within the wall-clock bound, took {:?}",
        start.elapsed()
    );
    let reason = client.poisoned_reason().expect("poisoned by watchdog");
    assert!(reason.contains("stalled"), "reason: {reason}");
    client.close();
}

/// §2.6 graceful: close() cancels pending ids on the wire
/// (`reason:"shutdown"`), fails their waiters typed, and does not hang.
#[test]
fn graceful_shutdown_cancels_pending_and_returns() {
    let collect = Collect::default();
    let options = ConnectionOptions {
        notification_sink: collect.sink(),
        graceful_shutdown_wait: Duration::from_millis(500),
        supervisor_tick: Duration::from_millis(50),
        ..Default::default()
    };
    let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "5000")], options);
    let call_client = client.clone();
    let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
    std::thread::sleep(Duration::from_millis(100));
    let start = Instant::now();
    client.close();
    assert!(start.elapsed() < Duration::from_secs(5));
    let err = call.join().unwrap().unwrap_err();
    assert!(
        matches!(&err, McpError::Transport(r) if r.contains("shut down")),
        "got {err}"
    );
    collect
        .wait_for(|v| is_cancel_obs(v, "shutdown"), Duration::from_secs(2))
        .expect("server saw notifications/cancelled with reason=shutdown");
}

/// F-P3-6 regression battery: the close sweep runs CONCURRENT with the
/// writer's closing-drain. Pre-fix, closing was set before the cancels were
/// enqueued and a fast writer drain-exited on one 25ms quiet tick, killing
/// the cancel in the lane. Every round has a call pending at close time, so
/// the sweep ALWAYS owes a cancel and the wire observation is asserted every
/// round (12 rounds: the pre-fix race surfaces under scheduler load).
#[test]
fn cancel_race_at_close_never_loses_the_cancel() {
    for round in 0..12 {
        let collect = Collect::default();
        let options = ConnectionOptions {
            notification_sink: collect.sink(),
            graceful_shutdown_wait: Duration::from_millis(500),
            supervisor_tick: Duration::from_millis(50),
            ..Default::default()
        };
        let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "5000")], options);
        let call_client = client.clone();
        let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
        std::thread::sleep(Duration::from_millis(100));
        client.close();
        let err = call.join().unwrap().expect_err("call fails typed at close");
        assert!(
            matches!(&err, McpError::Transport(r) if r.contains("shut down")),
            "round {round}: the pending waiter fails typed, got {err}"
        );
        collect
            .wait_for(|v| is_cancel_obs(v, "shutdown"), Duration::from_secs(2))
            .unwrap_or_else(|| {
                panic!("round {round}: pending id swept but the cancel never reached the wire")
            });
    }
}

struct AdvertiseElicitation;

impl ServerRequestHandler for AdvertiseElicitation {
    fn handle(
        &self,
        _conn: &ConnectionHandle,
        _request: &ServerRequest,
    ) -> Option<Result<Value, (i64, String)>> {
        None
    }

    fn advertises_elicitation(&self) -> bool {
        true
    }
}

/// Concurrent facade clones must not each run a partial shutdown sweep.
/// One closer owns the sweep; the other waits until every owed cancel is
/// queued before either can allow the writer to finish its closing drain.
#[test]
fn concurrent_close_has_one_complete_cancel_sweep() {
    for round in 0..12 {
        let collect = Collect::default();
        let options = ConnectionOptions {
            notification_sink: collect.sink(),
            graceful_shutdown_wait: Duration::from_millis(500),
            supervisor_tick: Duration::from_millis(50),
            ..Default::default()
        };
        let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "5000")], options);
        let call_client = client.clone();
        let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
        std::thread::sleep(Duration::from_millis(100));

        let barrier = Arc::new(Barrier::new(3));
        let close_a = client.clone();
        let barrier_a = barrier.clone();
        let a = std::thread::spawn(move || {
            barrier_a.wait();
            close_a.close();
        });
        let barrier_b = barrier.clone();
        let b = std::thread::spawn(move || {
            barrier_b.wait();
            client.close();
        });
        barrier.wait();
        a.join().expect("first concurrent close returns");
        b.join().expect("second concurrent close returns");

        let err = call.join().unwrap().expect_err("call fails typed at close");
        assert!(
            matches!(&err, McpError::Transport(r) if r.contains("shut down")),
            "round {round}: pending waiter fails with shutdown, got {err}"
        );
        collect
            .wait_for(|v| is_cancel_obs(v, "shutdown"), Duration::from_secs(2))
            .unwrap_or_else(|| panic!("round {round}: concurrent close lost the owed cancel"));
        assert_eq!(
            collect.count(|v| is_cancel_obs(v, "shutdown")),
            1,
            "round {round}: exactly one owner sweep emits exactly one owed cancel"
        );
    }
}

#[test]
fn panicking_retirement_hook_cannot_strand_concurrent_close() {
    let hook_ran = Arc::new(AtomicBool::new(false));
    let hook_flag = hook_ran.clone();
    let options = ConnectionOptions {
        request_handler: Arc::new(AdvertiseElicitation),
        slot_retired_hook: Arc::new(move |_| {
            hook_flag.store(true, Ordering::SeqCst);
            panic!("injected retirement hook panic");
        }),
        graceful_shutdown_wait: Duration::from_millis(500),
        supervisor_tick: Duration::from_millis(50),
        ..Default::default()
    };
    let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "5000")], options);
    let call_client = client.clone();
    let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
    std::thread::sleep(Duration::from_millis(100));
    let other = client.clone();
    let close = std::thread::spawn(move || other.close());
    client.close();
    close.join().expect("concurrent close survives hook panic");
    assert!(hook_ran.load(Ordering::SeqCst), "retirement hook ran");
    assert!(matches!(
        call.join().unwrap(),
        Err(McpError::Transport(ref reason)) if reason.contains("shut down")
    ));
}

#[test]
fn retirement_hook_can_reenter_close_after_sweep_completion() {
    let reentrant = Arc::new(Mutex::new(None::<McpClient>));
    let hook_client = reentrant.clone();
    let hook_ran = Arc::new(AtomicBool::new(false));
    let hook_flag = hook_ran.clone();
    let options = ConnectionOptions {
        request_handler: Arc::new(AdvertiseElicitation),
        slot_retired_hook: Arc::new(move |_| {
            hook_flag.store(true, Ordering::SeqCst);
            if let Some(client) = hook_client.lock().unwrap().take() {
                client.close();
            }
        }),
        graceful_shutdown_wait: Duration::from_millis(500),
        supervisor_tick: Duration::from_millis(50),
        ..Default::default()
    };
    let client = connect("echo", &[("FAKE_CALL_DELAY_MS", "5000")], options);
    *reentrant.lock().unwrap() = Some(client.clone());
    let call_client = client.clone();
    let call = std::thread::spawn(move || call_client.call_tool("echo", serde_json::json!({})));
    std::thread::sleep(Duration::from_millis(100));
    client.close();
    assert!(hook_ran.load(Ordering::SeqCst), "retirement hook ran");
    assert!(matches!(
        call.join().unwrap(),
        Err(McpError::Transport(ref reason)) if reason.contains("shut down")
    ));
}

// ---------------------------------------------------------------------------
// §4 resources v1: list/read over the dispatcher, capability gate, bounds
// ---------------------------------------------------------------------------

/// §4.1: resources/list + resources/read round-trip through the real
/// pending-map dispatcher path, typed results.
#[test]
fn resources_list_read_round_trip() {
    let client = connect_default("resources");
    assert!(client.negotiated().expect("negotiated").resources);
    let list = client.list_resources().expect("list");
    assert!(!list.truncated());
    assert_eq!(list.next_cursor, None);
    assert_eq!(list.resources.len(), 2);
    let alpha = &list.resources[0];
    assert_eq!(alpha.uri, "mem://alpha");
    assert_eq!(alpha.name, "alpha");
    assert_eq!(alpha.description.as_deref(), Some("first"));
    assert_eq!(alpha.mime_type.as_deref(), Some("text/plain"));
    assert_eq!(list.resources[1].uri, "mem://beta");
    assert_eq!(list.resources[1].description, None);
    assert_eq!(list.resources[1].mime_type, None);
    let read = client.read_resource("mem://alpha").expect("read");
    assert_eq!(read.contents.len(), 1);
    assert_eq!(read.contents[0].uri, "mem://alpha");
    assert_eq!(read.contents[0].mime_type.as_deref(), Some("text/plain"));
    assert_eq!(read.contents[0].text, "resource body");
    client.close();
}

/// §4.2: absent `resources` capability ⇒ typed ResourceUnsupported BEFORE
/// any wire write. Asserted at the transport seam: the fake server reports
/// every resources/* request it sees as a fake/obs notification — none may
/// arrive. The connection itself is untouched by the refusal.
#[test]
fn resources_capability_absent_refused_before_wire() {
    let collect = Collect::default();
    let options = ConnectionOptions {
        notification_sink: collect.sink(),
        ..Default::default()
    };
    let client = connect("echo", &[], options);
    assert!(!client.negotiated().expect("negotiated").resources);
    let err = client.list_resources().unwrap_err();
    assert!(matches!(err, McpError::ResourceUnsupported), "got {err}");
    let err = client.read_resource("mem://alpha").unwrap_err();
    assert!(matches!(err, McpError::ResourceUnsupported), "got {err}");
    assert!(
        collect
            .wait_for(
                |v| v["params"]["event"] == "saw_resources_request",
                Duration::from_millis(500),
            )
            .is_none(),
        "no resources request may reach the wire"
    );
    let result = client
        .call_tool("echo", serde_json::json!({"marker": "still-ok"}))
        .expect("connection still healthy");
    assert_eq!(result["marker"], "still-ok");
    assert!(client.poisoned_reason().is_none());
    client.close();
}

/// §4.1: NO pagination in v1 — a `nextCursor` marks the first (bounded)
/// page truncated; the page is still served and the cursor retained.
#[test]
fn resources_list_next_cursor_reports_truncation() {
    let client = connect(
        "resources",
        &[("FAKE_NEXT_CURSOR", "1")],
        ConnectionOptions::default(),
    );
    let list = client.list_resources().expect("list");
    assert_eq!(list.next_cursor.as_deref(), Some("page-2"));
    assert!(list.truncated());
    assert_eq!(list.resources.len(), 2, "first page still served");
    client.close();
}

/// §4.3: blob / non-text resource content is a typed ContentUnsupported
/// refusal — nothing crosses into the agent path; the connection survives.
#[test]
fn resources_read_blob_content_refused_typed() {
    let client = connect(
        "resources",
        &[("FAKE_READ_KIND", "blob")],
        ConnectionOptions::default(),
    );
    let err = client.read_resource("mem://alpha").unwrap_err();
    assert!(matches!(err, McpError::ContentUnsupported), "got {err}");
    assert!(client.poisoned_reason().is_none());
    client.close();
}

/// §4.1: a resource read is bounded at MAX_OUTPUT_BYTES exactly like a
/// tool call.
#[test]
fn resources_read_bounded_at_max_output() {
    let client = connect(
        "resources",
        &[("FAKE_READ_KIND", "big")],
        ConnectionOptions::default(),
    );
    let err = client.read_resource("mem://alpha").unwrap_err();
    assert!(matches!(err, McpError::OutputBounded(_)), "got {err}");
    client.close();
}
