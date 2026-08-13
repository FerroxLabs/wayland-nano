//! P3 journal/schema battery (P3-mcp-ecosystem-design §3.3/§5.6/§6.3/§12):
//! exact serde shapes, forward tolerance both directions, replay folds, the
//! compaction carry, and the journal-side bounds validators.

use crate::op::*;
use crate::replay::SessionState;

fn env(id: &str, op: Op) -> OpEnvelope {
    OpEnvelope::new(id, "2026-08-13T00:00:00Z", op)
}

fn p3_digest(byte: u8) -> String {
    format!("{byte:064x}")
}

fn p3_hydration_op(id: &str) -> Op {
    Op::McpToolHydration {
        hydration_id: id.into(),
        entries: vec![HydrationEntry {
            server_id: "fs".into(),
            tool_names: vec!["read".into(), "write".into()],
            tools_digest: p3_digest(1),
        }],
    }
}

/// The three P3 ops + the carry field: exact wire shapes pinned, round-trip
/// exact, bounded enums tolerant via `#[serde(other)]`.
#[test]
fn p3_ops_wire_shapes_pinned() {
    let hydration = p3_hydration_op("h1");
    let json = serde_json::to_value(&hydration).expect("serialize");
    assert_eq!(json["type"], "mcp_tool_hydration");
    assert_eq!(json["hydration_id"], "h1");
    assert_eq!(json["entries"][0]["server_id"], "fs");
    assert_eq!(json["entries"][0]["tool_names"][0], "read");
    assert_eq!(
        json["entries"][0]["tools_digest"],
        serde_json::Value::from(p3_digest(1))
    );
    let line = serde_json::to_string(&hydration).expect("serialize");
    let back: Op = serde_json::from_str(&line).expect("round trip");
    assert_eq!(back, hydration);

    let elicitation = Op::McpElicitation {
        elicitation_id: "e1".into(),
        server_id: "fs".into(),
        call_id: "c1".into(),
        request_id: "42".into(),
        card_id: 7,
        action: McpElicitationAction::Accept,
        schema_digest: p3_digest(2),
        answer_digest: p3_digest(3),
    };
    let json = serde_json::to_value(&elicitation).expect("serialize");
    assert_eq!(json["type"], "mcp_elicitation");
    assert_eq!(json["action"], "accept");
    assert_eq!(json["card_id"], 7);
    let back: Op = serde_json::from_str(&serde_json::to_string(&elicitation).expect("ser"))
        .expect("round trip");
    assert_eq!(back, elicitation);
    // A future action string deserializes to Unknown, never fails.
    let future = serde_json::to_string(&elicitation)
        .expect("ser")
        .replace(r#""action":"accept""#, r#""action":"escalate""#);
    let Op::McpElicitation { action, .. } =
        serde_json::from_str(&future).expect("forward tolerance")
    else {
        panic!("wrong op")
    };
    assert_eq!(action, McpElicitationAction::Unknown);

    let grant = Op::McpOauthGrant {
        grant_id: "g1".into(),
        server_id: "fs".into(),
        as_origin: "https://as.example".into(),
        issuer: "https://as.example".into(),
        endpoints: vec![
            GrantEndpoint {
                method: GrantMethod::Post,
                path: "/token".into(),
            },
            GrantEndpoint {
                method: GrantMethod::Get,
                path: "/.well-known/oauth-authorization-server".into(),
            },
        ],
    };
    let json = serde_json::to_value(&grant).expect("serialize");
    assert_eq!(json["type"], "mcp_oauth_grant");
    assert_eq!(json["endpoints"][0]["method"], "POST");
    assert_eq!(json["endpoints"][1]["method"], "GET");
    let back: Op =
        serde_json::from_str(&serde_json::to_string(&grant).expect("ser")).expect("round trip");
    assert_eq!(back, grant);
    // A future method string deserializes to Unknown, never fails.
    let future = serde_json::to_string(&grant)
        .expect("ser")
        .replace(r#""method":"POST""#, r#""method":"DELETE""#);
    let Op::McpOauthGrant { endpoints, .. } =
        serde_json::from_str(&future).expect("forward tolerance")
    else {
        panic!("wrong op")
    };
    assert_eq!(endpoints[0].method, GrantMethod::Unknown);

    // The carry field: serde-defaulted, skipped when None, present when Some.
    let old_complete = r#"{"type":"compaction_complete","compaction_id":"k1","summary":"s","covers_op_ids":[],"changed_files":[]}"#;
    let op: Op = serde_json::from_str(old_complete).expect("pre-P3 journal parses");
    let Op::CompactionComplete { mcp_hydration, .. } = &op else {
        panic!("wrong op")
    };
    assert!(mcp_hydration.is_none());
    assert_eq!(
        serde_json::to_string(&op).expect("reserialize"),
        old_complete,
        "pre-P3 journals stay byte-identical"
    );
    let carried = Op::CompactionComplete {
        compaction_id: "k1".into(),
        summary: "s".into(),
        covers_op_ids: vec!["1".into()],
        changed_files: vec![],
        image_influenced: false,
        mcp_hydration: Some(vec![HydrationCarryEntry {
            server_id: "fs".into(),
            tool_names: vec!["read".into()],
            tools_digest: p3_digest(1),
            recent_digests: vec![p3_digest(1)],
        }]),
    };
    let line = serde_json::to_string(&carried).expect("serialize");
    assert!(line.contains(r#""mcp_hydration"#));
    assert!(line.contains(r#""recent_digests"#));
    let back: Op = serde_json::from_str(&line).expect("round trip");
    assert_eq!(back, carried);
}

/// Forward tolerance: a journal line carrying an op TYPE this build does not
/// know deserializes as `Unknown` and folds to nothing (the P1 pattern,
/// extended to the P3 family — a NEWER journal read by an OLDER binary).
#[test]
fn p3_unknown_future_op_skips_on_replay() {
    let state = SessionState::fold(&[env("u1", Op::Unknown)]);
    assert!(state.mcp_hydrated.is_empty());
    assert!(state.mcp_oauth_grants.is_empty());
    let raw = r#"{"v":1,"id":"u2","ts":"now","op":{"type":"mcp_future_thing","x":1}}"#;
    let envelope: OpEnvelope = serde_json::from_str(raw).expect("forward tolerance");
    assert_eq!(envelope.op, Op::Unknown);
}

/// SCHEMA_VERSION stays 1: the additive P3 variants ride the Unknown
/// forward-tolerance, no envelope change.
#[test]
fn p3_schema_version_stays_one() {
    assert_eq!(SCHEMA_VERSION, 1);
}

/// Digest/ids/bounded-enums-only payload discipline: every string leaf in a
/// serialized P3 op is a bounded id/digest/enum — no content strings (the P1
/// numbers-only assertion, extended).
#[test]
fn p3_payloads_carry_no_content() {
    fn assert_bounded(value: &serde_json::Value) {
        match value {
            serde_json::Value::String(s) => {
                assert!(s.chars().count() <= 512, "unbounded string: {s}");
                assert!(
                    !s.chars().any(|c| c.is_control()),
                    "control characters never journal: {s:?}"
                );
            }
            serde_json::Value::Array(items) => items.iter().for_each(assert_bounded),
            serde_json::Value::Object(map) => map.values().for_each(assert_bounded),
            _ => {}
        }
    }
    for op in [
        p3_hydration_op("h1"),
        Op::McpElicitation {
            elicitation_id: "e1".into(),
            server_id: "fs".into(),
            call_id: "c1".into(),
            request_id: "42".into(),
            card_id: 7,
            action: McpElicitationAction::Decline,
            schema_digest: p3_digest(2),
            answer_digest: String::new(),
        },
        Op::McpOauthGrant {
            grant_id: "g1".into(),
            server_id: "fs".into(),
            as_origin: "https://as.example".into(),
            issuer: "https://as.example".into(),
            endpoints: vec![],
        },
    ] {
        assert_bounded(&serde_json::to_value(&op).expect("serialize"));
    }
}

/// Replay: hydration ops union per server, latest digest wins, and the churn
/// window is capped at 8 with oldest dropped (§3.4).
#[test]
fn p3_replay_hydration_fold_and_window_cap() {
    let mut envelopes = vec![env("h1", p3_hydration_op("h1"))];
    for i in 2..=10u8 {
        envelopes.push(env(
            &format!("h{i}"),
            Op::McpToolHydration {
                hydration_id: format!("h{i}"),
                entries: vec![HydrationEntry {
                    server_id: "fs".into(),
                    tool_names: vec![format!("tool{i}")],
                    tools_digest: p3_digest(i),
                }],
            },
        ));
    }
    let state = SessionState::fold(&envelopes);
    let hydrated = state.mcp_hydrated.get("fs").expect("hydrated");
    assert!(hydrated.contains("read"));
    assert!(hydrated.contains("tool10"));
    assert_eq!(state.mcp_tools_digest.get("fs").unwrap(), &p3_digest(10));
    let window = state.mcp_recent_digests.get("fs").expect("window");
    assert_eq!(window.len(), MAX_RECENT_DIGESTS);
    assert_eq!(window.first().unwrap(), &p3_digest(3));
    assert_eq!(window.last().unwrap(), &p3_digest(10));
    // Envelope-id dedup makes a retried append idempotent under the fold.
    let mut dup = envelopes.clone();
    dup.push(envelopes[0].clone());
    let state2 = SessionState::fold(&dup);
    assert_eq!(
        state2.mcp_recent_digests.get("fs").unwrap().len(),
        MAX_RECENT_DIGESTS
    );
}

/// Replay: the compaction carry installs the exact at-watermark state and
/// later surviving ops fold on top (the §3.3 survival rule).
#[test]
fn p3_replay_carry_installs_then_later_ops_fold() {
    let complete = env(
        "3",
        Op::CompactionComplete {
            compaction_id: "c1".into(),
            summary: "s".into(),
            covers_op_ids: vec!["1".into()],
            changed_files: vec![],
            image_influenced: false,
            mcp_hydration: Some(vec![HydrationCarryEntry {
                server_id: "fs".into(),
                tool_names: vec!["read".into()],
                tools_digest: p3_digest(1),
                recent_digests: vec![p3_digest(1)],
            }]),
        },
    );
    let state = SessionState::fold(&[complete, env("h2", p3_hydration_op("h2"))]);
    let hydrated = state.mcp_hydrated.get("fs").expect("hydrated");
    assert!(hydrated.contains("read"));
    assert!(hydrated.contains("write"));
    assert_eq!(
        state.mcp_recent_digests.get("fs").unwrap(),
        &vec![p3_digest(1), p3_digest(1)]
    );
}

/// Replay: elicitation ops are AUDIT-ONLY — no state fold (§5.6; the durable
/// effect rode the interrupted tool call's own ToolResult).
#[test]
fn p3_elicitation_replay_is_audit_only() {
    let before = SessionState::fold(&[]);
    let after = SessionState::fold(&[env(
        "e1",
        Op::McpElicitation {
            elicitation_id: "e1".into(),
            server_id: "fs".into(),
            call_id: "c1".into(),
            request_id: "42".into(),
            card_id: 7,
            action: McpElicitationAction::Accept,
            schema_digest: p3_digest(2),
            answer_digest: p3_digest(3),
        },
    )]);
    assert_eq!(after.mcp_hydrated, before.mcp_hydrated);
    assert_eq!(after.mcp_oauth_grants, before.mcp_oauth_grants);
    assert_eq!(after.open_turn, before.open_turn);
}

/// Replay: OAuth grants fold keyed by (server, as_origin), latest wins, so a
/// kill-resumed session reconstructs the exact grant set (§6.3 step 3).
#[test]
fn p3_oauth_grant_replay_latest_wins() {
    let grant = |id: &str, issuer: &str| {
        env(
            id,
            Op::McpOauthGrant {
                grant_id: id.into(),
                server_id: "fs".into(),
                as_origin: "https://as.example".into(),
                issuer: issuer.into(),
                endpoints: vec![GrantEndpoint {
                    method: GrantMethod::Post,
                    path: "/token".into(),
                }],
            },
        )
    };
    let state = SessionState::fold(&[
        grant("g1", "https://old.example"),
        grant("g2", "https://as.example"),
    ]);
    let record = state
        .mcp_oauth_grants
        .get(&("fs".to_string(), "https://as.example".to_string()))
        .expect("grant reconstructed");
    assert_eq!(record.issuer, "https://as.example");
    assert_eq!(record.endpoints.len(), 1);
}

/// The §3.3/§5.6/§6.3 bounds validators reject every over-cap shape.
#[test]
fn p3_bounds_validators_reject_over_cap_payloads() {
    let good = HydrationEntry {
        server_id: "fs".into(),
        tool_names: vec!["read".into()],
        tools_digest: p3_digest(1),
    };
    assert!(validate_hydration_entry(&good).is_ok());
    assert!(validate_hydration_batch(std::slice::from_ref(&good)).is_ok());
    assert!(validate_hydration_batch(&[]).is_err());
    assert!(validate_hydration_batch(&vec![good.clone(); 9]).is_err());
    let mut too_many_tools = good.clone();
    too_many_tools.tool_names = vec!["t".into(); 65];
    assert!(validate_hydration_entry(&too_many_tools).is_err());
    let mut long_name = good.clone();
    long_name.tool_names = vec!["t".repeat(129)];
    assert!(validate_hydration_entry(&long_name).is_err());
    let mut bad_digest = good.clone();
    bad_digest.tools_digest = "not-hex".into();
    assert!(validate_hydration_entry(&bad_digest).is_err());
    let mut upper_digest = good.clone();
    upper_digest.tools_digest = p3_digest(0xab).to_uppercase();
    assert!(validate_hydration_entry(&upper_digest).is_err());

    let carry = HydrationCarryEntry {
        server_id: "fs".into(),
        tool_names: vec!["read".into()],
        tools_digest: p3_digest(1),
        recent_digests: vec![p3_digest(1)],
    };
    assert!(validate_hydration_carry_entry(&carry).is_ok());
    let mut wide = carry.clone();
    wide.recent_digests = vec![p3_digest(1); 9];
    assert!(validate_hydration_carry_entry(&wide).is_err());
    let mut bad_window = carry.clone();
    bad_window.recent_digests = vec!["zz".into()];
    assert!(validate_hydration_carry_entry(&bad_window).is_err());

    assert!(validate_oauth_grant("https://as.example", "https://as.example", &[]).is_ok());
    assert!(validate_oauth_grant(&"https://h".repeat(40), "i", &[]).is_err());
    assert!(validate_oauth_grant("https://as.example", &"i".repeat(513), &[]).is_err());
    let endpoints = vec![
        GrantEndpoint {
            method: GrantMethod::Get,
            path: "/m".into(),
        };
        5
    ];
    assert!(validate_oauth_grant("https://as.example", "i", &endpoints).is_err());
    let bad_method = vec![GrantEndpoint {
        method: GrantMethod::Unknown,
        path: "/m".into(),
    }];
    assert!(validate_oauth_grant("https://as.example", "i", &bad_method).is_err());
    let bad_path = vec![GrantEndpoint {
        method: GrantMethod::Get,
        path: "relative".into(),
    }];
    assert!(validate_oauth_grant("https://as.example", "i", &bad_path).is_err());
}
