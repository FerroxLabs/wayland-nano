//! Permission modes (C2): `read_only` / `default` / `full_auto`.
//!
//! This enum is the SINGLE source of truth for the mode vocabulary: wire
//! ids, display labels, ACP advertisement metadata, the privilege ordering,
//! and parsing all live here, so the wire advertisement, TUI labels, and
//! the approval gate's behavior cannot drift apart.
//!
//! Mode semantics (design §2):
//! - `read_only`: read tools auto-approve; every mutation/execution is
//!   DENIED at the gate (no prompt — prompting for categorically forbidden
//!   actions would re-widen the session one click at a time) and the
//!   tool-layer policy itself refuses writes (defense in depth).
//! - `default`: read tools auto-approve; mutations/shell/MCP prompt the
//!   host (the historical behavior).
//! - `full_auto`: read tools auto-approve; CONTAINED fs writes
//!   auto-approve; shell auto-approves iff a platform sandbox backend is
//!   available; uncontained/unknown/MCP still prompt. The mode NEVER widens
//!   the sandbox or the tool-layer policy.

use serde::Deserialize;
use serde::Serialize;

/// The three permission modes, in privilege order: the derived `Ord` IS the
/// privilege ordering (`ReadOnly < Default < FullAuto`) the gate's
/// asymmetric min(captured, current) rule relies on. Do not reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    Default,
    FullAuto,
}

impl PermissionMode {
    /// Every mode, in privilege (advertisement) order.
    pub const ALL: [PermissionMode; 3] = [
        PermissionMode::ReadOnly,
        PermissionMode::Default,
        PermissionMode::FullAuto,
    ];

    /// The ACP wire id (`availableModes[].id`, `session/set_mode.modeId`).
    pub fn id(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "read_only",
            PermissionMode::Default => "default",
            PermissionMode::FullAuto => "full_auto",
        }
    }

    /// The display label (`availableModes[].name`; UIs title-case from here).
    pub fn label(self) -> &'static str {
        match self {
            PermissionMode::ReadOnly => "Read Only",
            PermissionMode::Default => "Default",
            PermissionMode::FullAuto => "Full Auto",
        }
    }

    /// Parse an inbound wire id. Unknown ids (including ids from a NEWER
    /// agent this build does not know) are `None` — the set_mode handler
    /// turns that into a typed error and changes nothing (fail-closed).
    pub fn parse(id: &str) -> Option<PermissionMode> {
        match id {
            "read_only" => Some(PermissionMode::ReadOnly),
            "default" => Some(PermissionMode::Default),
            "full_auto" => Some(PermissionMode::FullAuto),
            _ => None,
        }
    }
}

/// Every session — new or resumed — starts in `default`. The mode is never
/// restored from the journal: elevated autonomy always requires a fresh,
/// explicit grant.
impl Default for PermissionMode {
    fn default() -> Self {
        PermissionMode::Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_ids_are_snake_case_and_round_trip() {
        for mode in PermissionMode::ALL {
            assert_eq!(PermissionMode::parse(mode.id()), Some(mode));
            // serde uses the same vocabulary as the hand-rolled parser.
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.id()));
            assert_eq!(serde_json::from_str::<PermissionMode>(&json).unwrap(), mode);
        }
    }

    #[test]
    fn unknown_ids_do_not_parse() {
        for garbage in [
            "",
            "yolo",
            "force",
            "FULL_AUTO",
            "full-auto",
            "read-only",
            "dangerously_skip_permissions",
        ] {
            assert_eq!(PermissionMode::parse(garbage), None, "{garbage:?}");
        }
        assert!(serde_json::from_str::<PermissionMode>("\"yolo\"").is_err());
    }

    #[test]
    fn privilege_ordering_is_read_only_default_full_auto() {
        assert!(PermissionMode::ReadOnly < PermissionMode::Default);
        assert!(PermissionMode::Default < PermissionMode::FullAuto);
        // The asymmetric-application primitive: min(captured, current).
        assert_eq!(
            PermissionMode::FullAuto.min(PermissionMode::ReadOnly),
            PermissionMode::ReadOnly
        );
        assert_eq!(
            PermissionMode::Default.min(PermissionMode::FullAuto),
            PermissionMode::Default
        );
    }

    #[test]
    fn labels_are_title_case() {
        assert_eq!(PermissionMode::ReadOnly.label(), "Read Only");
        assert_eq!(PermissionMode::Default.label(), "Default");
        assert_eq!(PermissionMode::FullAuto.label(), "Full Auto");
    }
}
