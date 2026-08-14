//! Crate-local journal shapes for the S9 `Op::CuaAction`/`Op::CuaResult`
//! variants (design §4.1). The integrator adds the real `Op` arms in
//! `nano-session`; these records pin the field names, serde discipline,
//! and digest-only invariant the merge must match. Digests only — typed
//! text and coordinates are payload and never appear here.

use serde::{Deserialize, Serialize};

/// Appended BEFORE dispatch (the `Op::ToolCall`-before-approval
/// precedent): a failed append is turn-fatal, never a dropped record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuaActionRecord {
    pub turn_id: String,
    pub call_id: String,
    /// Snake-case kind tag (`CuaOp::kind_tag`).
    pub op_kind: String,
    /// sha256 of the canonical serialized args.
    pub args_digest: String,
    /// Frontmost app id shown to the approval prompt, if resolvable.
    #[serde(default)]
    pub frontmost_app: Option<String>,
    /// Pre-action screenshot attachment digest (mutating ops).
    #[serde(default)]
    pub pre_shot: Option<String>,
}

/// Appended after dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuaResultRecord {
    pub call_id: String,
    pub outcome: CuaOutcome,
    /// Post-action screenshot attachment digest (mutating ops).
    #[serde(default)]
    pub post_shot: Option<String>,
    /// Closed-vocabulary error kind on denial/failure.
    #[serde(default)]
    pub error_kind: Option<String>,
}

/// Reuses `TurnOutcome`'s serde discipline: snake_case, closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CuaOutcome {
    Completed,
    Denied,
    Cancelled,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_and_result_roundtrip() {
        let action = CuaActionRecord {
            turn_id: "t1".into(),
            call_id: "c1".into(),
            op_kind: "left_click".into(),
            args_digest: "a".repeat(64),
            frontmost_app: Some("notepad.exe".into()),
            pre_shot: Some("b".repeat(64)),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(
            serde_json::from_str::<CuaActionRecord>(&json).unwrap(),
            action
        );

        for outcome in [
            CuaOutcome::Completed,
            CuaOutcome::Denied,
            CuaOutcome::Cancelled,
            CuaOutcome::Failed,
        ] {
            let result = CuaResultRecord {
                call_id: "c1".into(),
                outcome,
                post_shot: Some("c".repeat(64)),
                error_kind: Some("cua_focus_lost".into()),
            };
            let json = serde_json::to_string(&result).unwrap();
            assert_eq!(
                serde_json::from_str::<CuaResultRecord>(&json).unwrap(),
                result
            );
        }
    }

    #[test]
    fn outcomes_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&CuaOutcome::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }

    #[test]
    fn frames_carry_digests_only() {
        // Digest-only invariant scan: no field of either record is a
        // coordinate or raw typed text — the shape itself enforces it.
        let action = CuaActionRecord {
            turn_id: "t".into(),
            call_id: "c".into(),
            op_kind: "type".into(),
            args_digest: "d".repeat(64),
            frontmost_app: None,
            pre_shot: None,
        };
        let value = serde_json::to_value(&action).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["text", "x", "y", "keys", "args"] {
            assert!(
                !keys.contains(&forbidden),
                "journal frame must not carry raw payload field {forbidden:?}"
            );
        }
    }

    #[test]
    fn optional_fields_default_for_forward_tolerance() {
        // Pre-S9-shaped frames (missing optional fields) still parse.
        let result: CuaResultRecord =
            serde_json::from_str(r#"{"call_id":"c","outcome":"completed"}"#).unwrap();
        assert_eq!(result.post_shot, None);
        assert_eq!(result.error_kind, None);
    }
}
