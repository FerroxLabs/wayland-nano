//! Scripted fake acp-host (design doc §6, L2/L3): plays RECORDED real
//! acp-host transcripts back at the TUI (corpus binding C2 — the fake host
//! may not encode the TUI author's beliefs about the wire; its frames come
//! from `crates/nano-cli/tests/acp_record.rs` recordings of the real host
//! loop, and hand-authored adversarial fixtures pass the structural lint in
//! tests/support).
//!
//! Fixture format: NDJSON, one `{"dir": ">"|"<", "frame": {...}}` object
//! per line. `>` = a frame the CLIENT (TUI) is expected to send; `<` = a
//! frame the HOST sends in reply. A `<` response to a request gets its `id`
//! rewritten to the live request id; a `<` host→client request (permission)
//! gets a fresh live id and the client response is checked against it.

use std::collections::HashMap;
use std::collections::VecDeque;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    Request(String),
    Notification(String),
    /// A client response to a host→client request (permission answer).
    Response,
}

#[derive(Debug)]
pub struct Step {
    pub expect: Expect,
    pub send: Vec<Value>,
}

/// One recorded client decision the fake host observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedDecision {
    pub live_id: u64,
    pub option_id: String,
}

#[derive(Debug, Default)]
pub struct FakeHost {
    steps: VecDeque<Step>,
    /// recorded permission id → live id we substituted.
    perm_ids: HashMap<u64, u64>,
    next_live_perm_id: u64,
    /// Every permission answer the client sent (newest last).
    pub decisions: Vec<ObservedDecision>,
}

const FIRST_LIVE_PERMISSION_ID: u64 = 9000;

impl FakeHost {
    pub fn from_script(text: &str) -> Result<Self, String> {
        let mut steps: Vec<Step> = Vec::new();
        for (lineno, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: Value = serde_json::from_str(line)
                .map_err(|e| format!("fixture line {}: invalid json: {e}", lineno + 1))?;
            let dir = entry
                .get("dir")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("fixture line {}: missing dir", lineno + 1))?;
            let frame = entry
                .get("frame")
                .cloned()
                .ok_or_else(|| format!("fixture line {}: missing frame", lineno + 1))?;
            match dir {
                ">" => steps.push(Step {
                    expect: expectation(&frame)
                        .map_err(|e| format!("fixture line {}: {e}", lineno + 1))?,
                    send: Vec::new(),
                }),
                "<" => {
                    let step = steps.last_mut().ok_or_else(|| {
                        format!("fixture line {}: '<' before any '>'", lineno + 1)
                    })?;
                    step.send.push(frame);
                }
                other => return Err(format!("fixture line {}: bad dir {other:?}", lineno + 1)),
            }
        }
        Ok(Self {
            steps: steps.into(),
            perm_ids: HashMap::new(),
            next_live_perm_id: FIRST_LIVE_PERMISSION_ID,
            decisions: Vec::new(),
        })
    }

    pub fn is_exhausted(&self) -> bool {
        self.steps.is_empty()
    }

    /// Unfulfilled expectations, for loud failure at teardown.
    pub fn remaining(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|s| format!("{:?}", s.expect))
            .collect()
    }

    /// Feed one client frame; returns the host frames to emit in reply.
    /// The script step is consumed ONLY when the frame validates — a
    /// spoofed or wrong frame never advances the script.
    pub fn feed(&mut self, frame: &Value) -> Result<Vec<Value>, String> {
        let Some(step) = self.steps.front() else {
            return Err(format!(
                "unexpected client frame (script exhausted): {frame}"
            ));
        };
        match &step.expect {
            Expect::Request(method) => {
                let got = frame.get("method").and_then(Value::as_str).unwrap_or("");
                if got != *method {
                    return Err(format!("expected request {method}, got {got}: {frame}"));
                }
            }
            Expect::Notification(method) => {
                let got = frame.get("method").and_then(Value::as_str).unwrap_or("");
                if got != *method {
                    return Err(format!(
                        "expected notification {method}, got {got}: {frame}"
                    ));
                }
            }
            Expect::Response => {
                let id = frame
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("expected a response frame, got: {frame}"))?;
                if !self.perm_ids.values().any(|live| *live == id) {
                    return Err(format!("response id {id} matches no open permission"));
                }
            }
        }
        let step = self.steps.pop_front().expect("checked front above");
        match &step.expect {
            Expect::Request(_) => {
                let id = frame.get("id").cloned().unwrap_or(Value::Null);
                Ok(step.send.iter().map(|f| self.remap_reply(f, &id)).collect())
            }
            Expect::Notification(_) => Ok(step.send.clone()),
            Expect::Response => {
                let id = frame.get("id").and_then(Value::as_u64).unwrap_or(0);
                let option_id = frame
                    .get("result")
                    .and_then(|r| r.get("outcome"))
                    .and_then(|o| o.get("optionId"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.decisions.push(ObservedDecision {
                    live_id: id,
                    option_id,
                });
                Ok(step
                    .send
                    .iter()
                    .map(|f| self.remap_reply(f, &Value::Null))
                    .collect())
            }
        }
    }

    /// Rewrite recorded ids to live ones: responses adopt the client request
    /// id; host→client requests (permission) get a fresh live id with the
    /// mapping remembered for the answer check.
    fn remap_reply(&mut self, frame: &Value, request_id: &Value) -> Value {
        let mut frame = frame.clone();
        let is_request = frame.get("method").is_some() && frame.get("id").is_some();
        let is_response = frame.get("method").is_none() && frame.get("id").is_some();
        if is_response {
            frame["id"] = request_id.clone();
        } else if is_request {
            let recorded = frame.get("id").and_then(Value::as_u64).unwrap_or(0);
            let live = if let Some(live) = self.perm_ids.get(&recorded) {
                *live
            } else {
                self.next_live_perm_id += 1;
                let live = self.next_live_perm_id;
                self.perm_ids.insert(recorded, live);
                live
            };
            frame["id"] = Value::from(live);
        }
        frame
    }
}

fn expectation(frame: &Value) -> Result<Expect, String> {
    let method = frame.get("method").and_then(Value::as_str);
    let has_id = frame.get("id").is_some_and(|i| !i.is_null());
    match (method, has_id) {
        (Some(m), true) => Ok(Expect::Request(m.to_string())),
        (Some(m), false) => Ok(Expect::Notification(m.to_string())),
        (None, true) => Ok(Expect::Response),
        (None, false) => Err("frame is neither request, notification, nor response".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCRIPT: &str = r#"
{"dir":">","frame":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s1"}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{}}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}}
"#;

    #[test]
    fn plays_back_and_remaps_response_ids() {
        let mut host = FakeHost::from_script(SCRIPT).unwrap();
        let replies = host
            .feed(&json!({"id": 10, "method": "initialize"}))
            .unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0]["id"], 10, "response adopts the live request id");

        let replies = host
            .feed(&json!({"id": 11, "method": "session/new"}))
            .unwrap();
        assert_eq!(replies[0]["result"]["sessionId"], "s1");

        let replies = host
            .feed(&json!({"id": 12, "method": "session/prompt"}))
            .unwrap();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[1]["id"], 12);
        assert!(host.is_exhausted());
    }

    #[test]
    fn rejects_wrong_method_loudly() {
        let mut host = FakeHost::from_script(SCRIPT).unwrap();
        let err = host.feed(&json!({"id": 1, "method": "bogus"})).unwrap_err();
        assert!(err.contains("expected request initialize"));
    }

    #[test]
    fn permission_ids_remap_and_answers_are_checked() {
        let script = r#"
{"dir":">","frame":{"id":1,"method":"session/prompt"}}
{"dir":"<","frame":{"jsonrpc":"2.0","id":1,"method":"session/request_permission","params":{}}}
{"dir":">","frame":{"jsonrpc":"2.0","id":1,"result":{"outcome":{"outcome":"selected","optionId":"allow"}}}}
{"dir":"<","frame":{"jsonrpc":"2.0","method":"session/update","params":{}}}
"#;
        let mut host = FakeHost::from_script(script).unwrap();
        let replies = host
            .feed(&json!({"id": 5, "method": "session/prompt"}))
            .unwrap();
        let live_id = replies[0]["id"].as_u64().unwrap();
        assert_ne!(live_id, 1, "permission request gets a fresh live id");
        // An answer with an unknown id is rejected (spoof check).
        let script_err = host
            .feed(&json!({"id": 12345, "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}}))
            .unwrap_err();
        assert!(script_err.contains("matches no open permission"));
        // The right id is accepted and recorded.
        host.feed(&json!({"id": live_id, "result": {"outcome": {"outcome": "selected", "optionId": "deny"}}}))
            .unwrap();
        assert_eq!(
            host.decisions,
            vec![ObservedDecision {
                live_id,
                option_id: "deny".into()
            }]
        );
    }
}
