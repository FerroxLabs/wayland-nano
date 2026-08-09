//! Nano capability profile: the honest advertisement the host sees, in the
//! corpus capabilities shape.
//!
//! v1 scope truth (constitution): files/shell/streaming/thinking/approvals
//! true; orchestration surfaces false — never omitted, never fudged.

use crate::messages::NanoCapabilities;
use std::collections::BTreeMap;

pub fn v1_capabilities() -> NanoCapabilities {
    // mcp/skills are TRUE because they are proven end-to-end through the
    // live vertical slice (mcp tool call routed + skill instruction visible
    // to the model) — the capability honesty rule: flags flip only after
    // slice proof, never on intent.
    NanoCapabilities {
        cost_attribution: true,
        mcp: true,
        memory_enabled: false,
        plugins: false,
        streaming_tools: true,
        structured_traces: true,
        sub_agent_traces: false,
        thinking: true,
        tool_approval: true,
        browser_suite: false,
        computer_use: false,
        modes: vec!["default".into()],
        current_mode: "default".into(),
        extensions: BTreeMap::from([("skills".to_string(), serde_json::json!(true))]),
    }
}

/// Skills capability (separate flag in the capabilities object — the corpus
/// shape has no dedicated skills key, so it rides the extensions map).
pub fn skills_capability() -> serde_json::Value {
    serde_json::json!(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_profile_is_honest_in_corpus_shape() {
        let caps = v1_capabilities();
        let json: serde_json::Value = serde_json::from_str(&serde_json::to_string(&caps).unwrap()).unwrap();
        assert_eq!(json["thinking"], true);
        assert_eq!(json["tool_approval"], true);
        assert_eq!(json["mcp"], true);
        assert_eq!(json["browser_suite"], false);
        assert_eq!(json["computer_use"], false);
        assert_eq!(json["plugins"], false);
        assert_eq!(json["skills"], true);
        assert_eq!(json["current_mode"], "default");
    }
}
