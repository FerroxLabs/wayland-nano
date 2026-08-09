//! Nano capability profile: the honest advertisement the host sees.
//!
//! v1 scope truth (constitution): files/shell/streaming/thinking/approvals
//! true; mcp/skills false (land later); subagents 0 (bounded helpers are a
//! runtime concern, not advertised v1); orchestration surfaces explicitly
//! unavailable. Never advertise what is not implemented.

use crate::messages::Capabilities;
use std::collections::BTreeMap;

pub fn v1_capabilities() -> Capabilities {
    Capabilities {
        files: true,
        shell: true,
        streaming: true,
        thinking: true,
        approvals: true,
        mcp: false,
        skills: false,
        subagents: 0,
        unavailable: vec![
            "mcp".into(),
            "skills".into(),
            "anvil".into(),
            "crucible".into(),
            "evolution".into(),
            "workflows".into(),
            "browser".into(),
            "computer_use".into(),
        ],
        extensions: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_profile_is_honest() {
        let caps = v1_capabilities();
        assert!(caps.files && caps.shell && caps.streaming && caps.approvals);
        assert!(!caps.mcp && !caps.skills);
        assert_eq!(caps.subagents, 0);
        assert!(caps.unavailable.contains(&"evolution".to_string()));
        assert!(caps.unavailable.contains(&"workflows".to_string()));
    }
}
