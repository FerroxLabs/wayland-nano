//! Scripted fake MCP server for the §12 dispatcher battery (the
//! `tree_kill_probe` pattern: a probe binary driving one scenario chosen by
//! argv[1], cross-platform, no powershell/sh quoting).
//!
//! Protocol: line-delimited JSON-RPC on stdin/stdout. Always answers
//! `initialize` (protocolVersion 2025-06-18) and `tools/list` (one `echo`
//! tool). `tools/call` behavior is scenario-specific. Observations that the
//! client cannot see directly (e.g. a received `notifications/cancelled`)
//! are reported back as `fake/obs` notifications.
//!
//! Scenarios: echo | threaded | silent | die-on-call | blind | garbage |
//! bad-envelope | big-line | huge-line | mystery-ids | duplicate |
//! notify-mid-call | inject-request | inject-bogus | flood | resources |
//! tree
//!
//! Env knobs: FAKE_CALL_DELAY_MS, FAKE_GARBAGE_N, FAKE_BAD_KIND
//! (version|both|float|method), FAKE_BIG_BYTES, FAKE_MYSTERY_N,
//! FAKE_CANCEL (e.g. "srv-5"), FAKE_NEXT_CURSOR, FAKE_READ_KIND
//! (text|blob|big), FAKE_TREE_PID_FILE (tree scenario: pid-file path).

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Out = Arc<Mutex<std::io::Stdout>>;

fn write_frame(out: &Out, frame: &Value) {
    let mut o = out.lock().unwrap();
    writeln!(o, "{frame}").unwrap();
    o.flush().unwrap();
}

fn write_raw(out: &Out, bytes: &[u8]) {
    let mut o = out.lock().unwrap();
    o.write_all(bytes).unwrap();
    o.flush().unwrap();
}

fn obs(out: &Out, params: Value) {
    write_frame(
        out,
        &json!({"jsonrpc":"2.0","method":"fake/obs","params":params}),
    );
}

fn result_reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Shared collector for the inject-* and flood scenarios: the reply to the
/// server-initiated request(s) arrives on a later line, so the call answer
/// is deferred until the collector completes (or a watchdog fires).
struct Flood {
    call_id: Value,
    /// (server-request id, "result" | "error:<code>")
    replies: Vec<(String, String)>,
    expected: usize,
    done: bool,
}

impl Flood {
    fn finish(&mut self, out: &Out) {
        if self.done {
            return;
        }
        self.done = true;
        let results = self.replies.iter().filter(|(_, k)| k == "result").count();
        let overflow = self
            .replies
            .iter()
            .filter(|(_, k)| k == "error:-32603")
            .count();
        let mut ids: Vec<String> = self
            .replies
            .iter()
            .filter(|(_, k)| k == "result")
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        let answer = json!({
            "results": results,
            "overflow": overflow,
            "answered_ids": ids,
        });
        write_frame(out, &result_reply(&self.call_id.clone(), answer));
    }

    fn record(&mut self, out: &Out, id: &Value, kind: String) {
        if self.done {
            return;
        }
        self.replies.push((id_to_key(id), kind));
        if self.replies.len() >= self.expected {
            self.finish(out);
        }
    }
}

fn id_to_key(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

/// inject-request / inject-bogus state: waiting for the reply to one
/// server request before answering the in-flight call.
struct Inject {
    call_id: Value,
    reply_id: String,
    done: bool,
}

fn answer_call_later(out: &Out, id: Value, delay: Duration, marker: Value) {
    let out = out.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        write_frame(
            &out,
            &result_reply(
                &id,
                json!({
                    "content": [{"type": "text", "text": "pong"}],
                    "isError": false,
                    "marker": marker,
                }),
            ),
        );
    });
}

/// F-P3-2 (§2.6) proof scenario: spawn one DIRECT descendant with null
/// stdio and record both pids in FAKE_TREE_PID_FILE, so the client-side
/// test can assert via external process inventory that contained teardown
/// (job terminate / kill-on-close) kills BOTH. The descendant is spawned
/// plain: as a child of a job member it joins the job automatically (no
/// BREAKAWAY_OK in the contained job), and the dropped Child handle does
/// NOT reap it — only job teardown can. The zombie-processes allowance is
/// the point of the probe: the descendant must OUTLIVE any child-handle
/// drop so only job containment can kill it.
#[allow(clippy::zombie_processes)]
fn spawn_tree_probe() {
    #[cfg(target_os = "windows")]
    let descendant = std::process::Command::new("ping.exe")
        .args(["-t", "127.0.0.1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("tree scenario descendant spawns");
    #[cfg(unix)]
    let descendant = std::process::Command::new("sh")
        .args(["-c", "while :; do sleep 60; done"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("tree scenario descendant spawns");
    let pid_file = std::env::var("FAKE_TREE_PID_FILE").expect("FAKE_TREE_PID_FILE");
    std::fs::write(
        pid_file,
        format!(
            "server={}\ndescendant={}\n",
            std::process::id(),
            descendant.id()
        ),
    )
    .unwrap();
}

fn main() {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "echo".into());
    if scenario == "tree" {
        spawn_tree_probe();
    }
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));
    let call_delay = Duration::from_millis(env_u64("FAKE_CALL_DELAY_MS", 0));

    let flood: Arc<Mutex<Option<Flood>>> = Arc::new(Mutex::new(None));
    let inject: Arc<Mutex<Option<Inject>>> = Arc::new(Mutex::new(None));

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(line) = lines.next() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                // Only the "resources" scenario advertises the resources
                // capability (§4.2: the capability gate keys off this).
                let capabilities = if scenario == "resources" {
                    json!({"resources": {}})
                } else {
                    json!({})
                };
                write_frame(
                    &out,
                    &result_reply(
                        &id,
                        json!({
                            "protocolVersion": "2025-06-18",
                            "capabilities": capabilities,
                            "serverInfo": {"name": "fake", "version": "0"},
                        }),
                    ),
                );
            }
            "notifications/initialized" => {}
            "tools/list" => {
                write_frame(
                    &out,
                    &result_reply(
                        &id,
                        json!({"tools": [{"name": "echo", "description": "fake echo"}]}),
                    ),
                );
                // §12 (j): "blind" never reads stdin again, STARTING NOW —
                // the client's next (backpressure-probe) frame is never
                // consumed, so the writer parks mid-frame on any host.
                // Parking on the tools/call line instead (the old shape)
                // means READING that line first: a multi-MB probe frame
                // then transfers in full on a fast host, the writer idles,
                // no stall ever occurs, and the leg rides the 30s call
                // timeout instead of the watchdog (F-P3-13).
                if scenario == "blind" {
                    loop {
                        std::thread::sleep(Duration::from_secs(60));
                    }
                }
            }
            "resources/list" | "resources/read" => {
                handle_resource(&scenario, &out, &id, method, &v)
            }
            "notifications/cancelled" => {
                let params = v.get("params").cloned().unwrap_or_default();
                obs(
                    &out,
                    json!({
                        "event": "saw_cancel",
                        "requestId": params.get("requestId").cloned().unwrap_or(Value::Null),
                        "reason": params.get("reason").cloned().unwrap_or(Value::Null),
                    }),
                );
            }
            "tools/call" if scenario == "flood" => handle_flood(&out, &id, &flood, &mut lines),
            "tools/call" => handle_call(&scenario, &out, &id, &v, call_delay, &inject),
            // No method: a response to one of OUR server requests.
            "" => {
                let kind = if v.get("error").is_some() {
                    let code = v["error"]["code"].as_i64().unwrap_or(0);
                    format!("error:{code}")
                } else {
                    "result".to_string()
                };
                let mut f = flood.lock().unwrap();
                if let Some(flood) = f.as_mut() {
                    flood.record(&out, &id, kind);
                    continue;
                }
                drop(f);
                let mut i = inject.lock().unwrap();
                if let Some(inj) = i.as_mut()
                    && !inj.done
                    && id_to_key(&id) == inj.reply_id
                {
                    inj.done = true;
                    let had_result = v.get("result").is_some();
                    let error_code = v
                        .get("error")
                        .and_then(|e| e.get("code"))
                        .and_then(|c| c.as_i64());
                    let call_id = inj.call_id.clone();
                    write_frame(
                        &out,
                        &result_reply(
                            &call_id,
                            json!({"got_reply": true, "had_result": had_result, "error_code": error_code}),
                        ),
                    );
                }
            }
            _ => {}
        }
    }
}

/// resources/list + resources/read (§4.1). Only the "resources" scenario
/// serves them; any other scenario REPORTS the request as a fake/obs
/// notification — the transport-seam probe for the §4.2 capability gate
/// test (a capability-absent call must produce ZERO wire activity).
fn handle_resource(scenario: &str, out: &Out, id: &Value, method: &str, request: &Value) {
    if scenario != "resources" {
        obs(
            out,
            json!({"event": "saw_resources_request", "method": method}),
        );
        return;
    }
    match method {
        "resources/list" => {
            let mut result = json!({
                "resources": [
                    {"uri": "mem://alpha", "name": "alpha", "description": "first", "mimeType": "text/plain"},
                    {"uri": "mem://beta", "name": "beta"},
                ],
            });
            if std::env::var("FAKE_NEXT_CURSOR").is_ok() {
                result["nextCursor"] = json!("page-2");
            }
            write_frame(out, &result_reply(id, result));
        }
        "resources/read" => {
            let uri = request
                .get("params")
                .and_then(|p| p.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let kind = std::env::var("FAKE_READ_KIND").unwrap_or_else(|_| "text".into());
            let result = match kind.as_str() {
                // Non-text content: the client must refuse typed (§4.3).
                "blob" => json!({
                    "contents": [{"uri": uri, "mimeType": "application/octet-stream", "blob": "aGVsbG8="}],
                }),
                // Over MAX_OUTPUT_BYTES once encoded: the client must bound
                // the read like a tool call (§4.1).
                "big" => json!({
                    "contents": [{"uri": uri, "mimeType": "text/plain", "text": "x".repeat(600 * 1024)}],
                }),
                _ => json!({
                    "contents": [{"uri": uri, "mimeType": "text/plain", "text": "resource body"}],
                }),
            };
            write_frame(out, &result_reply(id, result));
        }
        _ => {}
    }
}

fn handle_call(
    scenario: &str,
    out: &Out,
    id: &Value,
    request: &Value,
    call_delay: Duration,
    inject: &Arc<Mutex<Option<Inject>>>,
) {
    let marker = request
        .get("params")
        .and_then(|p| p.get("arguments"))
        .and_then(|a| a.get("marker"))
        .cloned()
        .unwrap_or(Value::Null);
    match scenario {
        "echo" | "threaded" => {
            let delay = request
                .get("params")
                .and_then(|p| p.get("arguments"))
                .and_then(|a| a.get("delay_ms"))
                .and_then(|d| d.as_u64())
                .map(Duration::from_millis)
                .unwrap_or(call_delay);
            answer_call_later(out, id.clone(), delay, marker);
        }
        "silent" => {}
        "die-on-call" => std::process::exit(2),
        "garbage" => {
            let n = env_u64("FAKE_GARBAGE_N", 1);
            for _ in 0..n {
                write_raw(out, b"this is not json at all {\n");
            }
            write_frame(out, &result_reply(id, json!({"ok": true})));
        }
        "bad-envelope" => {
            let kind = std::env::var("FAKE_BAD_KIND").unwrap_or_else(|_| "version".into());
            let bad = match kind.as_str() {
                "both" => {
                    json!({"jsonrpc":"2.0","id":id,"result":{},"error":{"code":-1,"message":"x"}})
                }
                "float" => json!({"jsonrpc":"2.0","id":1.5,"result":{}}),
                "method" => json!({"jsonrpc":"2.0","id":id,"method":42,"result":{}}),
                "neither" => json!({"jsonrpc":"2.0","params":{}}),
                _ => json!({"jsonrpc":"1.0","id":id,"result":{}}),
            };
            write_frame(out, &bad);
            // The client must poison on the bad frame; this proper answer
            // should never be routed.
            write_frame(out, &result_reply(id, json!({"ok": true})));
        }
        "big-line" => {
            let size = env_u64("FAKE_BIG_BYTES", 8 * 1024 * 1024 + 100) as usize;
            let mut line = "x".repeat(size);
            line.push('\n');
            write_raw(out, line.as_bytes());
            write_frame(out, &result_reply(id, json!({"ok": true})));
        }
        "huge-line" => {
            // Over the 4x drain cap with NO newline: stream position is
            // unrecoverable, the client must poison.
            let line = "x".repeat(33 * 1024 * 1024);
            write_raw(out, line.as_bytes());
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        "mystery-ids" => {
            let n = env_u64("FAKE_MYSTERY_N", 1);
            let string_ids = std::env::var("FAKE_MYSTERY_STRING").is_ok();
            for i in 0..n {
                let mid = if string_ids {
                    json!(format!("never-issued-{i}"))
                } else {
                    json!(9000 + i)
                };
                write_frame(out, &json!({"jsonrpc":"2.0","id":mid,"result":{}}));
            }
            write_frame(out, &result_reply(id, json!({"ok": true})));
        }
        "duplicate" => {
            write_frame(out, &result_reply(id, json!({"ok": true})));
            write_frame(out, &result_reply(id, json!({"ok": true})));
        }
        "notify-mid-call" => {
            write_frame(
                out,
                &json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"n":1}}),
            );
            write_frame(out, &result_reply(id, json!({"ok": true})));
        }
        "inject-request" | "inject-bogus" => {
            let (srv_method, srv_id) = if scenario == "inject-request" {
                ("ping", "srv-1")
            } else {
                ("bogus/method", "srv-9")
            };
            write_frame(
                out,
                &json!({"jsonrpc":"2.0","method":srv_method,"id":srv_id,"params":{}}),
            );
            *inject.lock().unwrap() = Some(Inject {
                call_id: id.clone(),
                reply_id: srv_id.to_string(),
                done: false,
            });
            // Watchdog: if no reply arrives within 5s, answer with
            // got_reply:false (the test then fails fast, not at 30s).
            let out2 = out.clone();
            let inject2 = inject.clone();
            let call_id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(5));
                let mut i = inject2.lock().unwrap();
                if let Some(inj) = i.as_mut()
                    && !inj.done
                    && inj.call_id == call_id
                {
                    inj.done = true;
                    write_frame(&out2, &result_reply(&call_id, json!({"got_reply": false})));
                }
            });
        }
        other => {
            write_frame(
                out,
                &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unknown scenario {other}")}}),
            );
        }
    }
}

/// flood (§12 h / §2.5): 18 server requests on a NON-builtin method against
/// the client's 16-cap server-request queue, with a gated handler parked on
/// the first one. TWO-PHASE, because the overflow count is only
/// deterministic when the handler/thread split is fixed BEFORE the flood:
///
///   Phase 1 — send `srv-1` only. The client's handler thread dequeues it
///   and parks on the test gate, then signals `probe/flood_go`. Waiting for
///   that signal is what makes the arithmetic exact: the handler holds
///   srv-1 and the queue is EMPTY when phase 2 starts. (Without the
///   handshake a slow scheduler can let the reader saturate the queue
///   before the handler's first recv, overflowing TWO requests instead of
///   one — the macos-15-intel CI flake.)
///
///   Phase 2 — send `srv-2..=srv-18`: 16 fill the queue, srv-18 overflows
///   to exactly one -32603 reply. The optional FAKE_CANCEL
///   `notifications/cancelled` goes LAST on the wire, so the reader records
///   it strictly after all enqueue/overflow decisions; it drops the queued
///   request at handler-dequeue time and can never shift the overflow
///   count.
fn handle_flood(
    out: &Out,
    call_id: &Value,
    flood: &Arc<Mutex<Option<Flood>>>,
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
) {
    const FLOOD_N: u64 = 18;
    write_frame(
        out,
        &json!({"jsonrpc":"2.0","method":"probe/knock","id":"srv-1","params":{}}),
    );
    // Block until the client's handler reports parked-on-gate. Anything
    // else arriving first is ignored; EOF means the client is gone.
    for line in lines.by_ref() {
        let Ok(line) = line else { return };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("method").and_then(|m| m.as_str()) == Some("probe/flood_go") {
            break;
        }
    }
    for n in 2..=FLOOD_N {
        write_frame(
            out,
            &json!({"jsonrpc":"2.0","method":"probe/knock","id":format!("srv-{n}"),"params":{}}),
        );
    }
    let cancelled = std::env::var("FAKE_CANCEL").unwrap_or_default();
    if !cancelled.is_empty() {
        write_frame(
            out,
            &json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":cancelled}}),
        );
    }
    let expected = if cancelled.is_empty() {
        FLOOD_N as usize
    } else {
        FLOOD_N as usize - 1
    };
    *flood.lock().unwrap() = Some(Flood {
        call_id: call_id.clone(),
        replies: Vec::new(),
        expected,
        done: false,
    });
    // Watchdog: answer with partial counts after 15s so a broken client
    // fails the test fast instead of at the 30s call timeout.
    let out2 = out.clone();
    let flood2 = flood.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(15));
        let mut f = flood2.lock().unwrap();
        if let Some(flood) = f.as_mut() {
            flood.finish(&out2);
        }
    });
}
