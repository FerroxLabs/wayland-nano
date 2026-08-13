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
//! notify-mid-call | inject-request | inject-bogus | flood
//!
//! Env knobs: FAKE_CALL_DELAY_MS, FAKE_GARBAGE_N, FAKE_BAD_KIND
//! (version|both|float|method), FAKE_BIG_BYTES, FAKE_MYSTERY_N,
//! FAKE_CANCEL (e.g. "srv-5").

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

fn main() {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "echo".into());
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));
    let call_delay = Duration::from_millis(env_u64("FAKE_CALL_DELAY_MS", 0));

    let flood: Arc<Mutex<Option<Flood>>> = Arc::new(Mutex::new(None));
    let inject: Arc<Mutex<Option<Inject>>> = Arc::new(Mutex::new(None));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => write_frame(
                &out,
                &result_reply(
                    &id,
                    json!({
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "serverInfo": {"name": "fake", "version": "0"},
                    }),
                ),
            ),
            "notifications/initialized" => {}
            "tools/list" => write_frame(
                &out,
                &result_reply(
                    &id,
                    json!({"tools": [{"name": "echo", "description": "fake echo"}]}),
                ),
            ),
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
            "tools/call" => handle_call(&scenario, &out, &id, &v, call_delay, &flood, &inject),
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

fn handle_call(
    scenario: &str,
    out: &Out,
    id: &Value,
    request: &Value,
    call_delay: Duration,
    flood: &Arc<Mutex<Option<Flood>>>,
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
        "blind" => {
            // Never read stdin again: the client's write side fills the OS
            // pipe and parks — the §12 (j) backpressure leg.
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
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
        "flood" => {
            // 18 server requests on a NON-builtin method (the test installs
            // a gated handler that parks): with the handler occupied, the
            // 16-cap queue fills and the LAST request overflows to a -32603
            // reply (§12 h).
            const FLOOD_N: u64 = 18;
            for n in 1..=FLOOD_N {
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
                call_id: id.clone(),
                replies: Vec::new(),
                expected,
                done: false,
            });
            // Watchdog: answer with partial counts after 15s so a broken
            // client fails the test fast instead of at the 30s call timeout.
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
        other => {
            write_frame(
                out,
                &json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("unknown scenario {other}")}}),
            );
        }
    }
}
