//! Permission posture (design §3, strictest-wins): CUA is uncontainable
//! by construction, so no mode auto-approves it. `read_only` and plan
//! mode do not register the tool at all (plan mode is enforced by the
//! integrator — it forbids mutation and CUA is mutation); `default` and
//! `full_auto` register it but route EVERY op through the approval gate.
//!
//! The mode vocabulary mirrors `nano_protocol::permission_mode` by wire
//! id; nano-cua deliberately does NOT depend on nano-protocol (that crate
//! pulls the provider/network tree, and nano-cua links no HTTP client by
//! construction — design §2.6). The integrator maps
//! `PermissionMode::id()` into [`CuaMode::from_wire_id`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuaMode {
    ReadOnly,
    Default,
    FullAuto,
}

impl CuaMode {
    /// Same wire vocabulary as `PermissionMode::parse`; unknown ids are
    /// `None` (fail-closed — the integrator changes nothing on them).
    pub fn from_wire_id(id: &str) -> Option<CuaMode> {
        match id {
            "read_only" => Some(CuaMode::ReadOnly),
            "default" => Some(CuaMode::Default),
            "full_auto" => Some(CuaMode::FullAuto),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CuaPosture {
    /// Tool absent from the registry; the model sees no CUA schema.
    NotRegistered,
    /// Registered; every op prompts, including under `full_auto`.
    AlwaysPrompt,
}

pub fn posture_for_mode(mode: CuaMode) -> CuaPosture {
    match mode {
        CuaMode::ReadOnly => CuaPosture::NotRegistered,
        CuaMode::Default | CuaMode::FullAuto => CuaPosture::AlwaysPrompt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Design §7.1 gate matrix: read_only ⇒ absent; default/full_auto ⇒
    /// prompt. Plan mode is an agent-level flag the integrator maps to
    /// `NotRegistered`; the crate-local matrix covers the mode vocabulary.
    #[test]
    fn gate_matrix_is_strictest_wins() {
        assert_eq!(
            posture_for_mode(CuaMode::ReadOnly),
            CuaPosture::NotRegistered
        );
        for mode in [CuaMode::Default, CuaMode::FullAuto] {
            assert_eq!(
                posture_for_mode(mode),
                CuaPosture::AlwaysPrompt,
                "{mode:?} must still prompt — full_auto never auto-approves CUA"
            );
        }
    }

    #[test]
    fn wire_ids_match_the_protocol_vocabulary() {
        // Same ids as nano_protocol::permission_mode::PermissionMode.
        assert_eq!(CuaMode::from_wire_id("read_only"), Some(CuaMode::ReadOnly));
        assert_eq!(CuaMode::from_wire_id("default"), Some(CuaMode::Default));
        assert_eq!(CuaMode::from_wire_id("full_auto"), Some(CuaMode::FullAuto));
        for garbage in ["", "yolo", "FULL_AUTO", "full-auto", "read-only"] {
            assert_eq!(CuaMode::from_wire_id(garbage), None, "{garbage:?}");
        }
    }
}
