//! Nano capability profile: the honest advertisement the host sees, in the
//! corpus capabilities shape.
//!
//! v1 scope truth (constitution): files/shell/streaming/thinking/approvals
//! true; orchestration surfaces false — never omitted, never fudged.

use crate::messages::NanoCapabilities;
use std::collections::BTreeMap;

pub fn v1_capabilities() -> NanoCapabilities {
    NanoCapabilities {
        cost_attribution: true,
        mcp: false,
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
        extensions: BTreeMap::new(),
    }
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
        assert_eq!(json["mcp"], false);
        assert_eq!(json["browser_suite"], false);
        assert_eq!(json["computer_use"], false);
        assert_eq!(json["plugins"], false);
        assert_eq!(json["current_mode"], "default");
    }
}
