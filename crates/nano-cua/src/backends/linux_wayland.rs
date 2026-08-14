/// Fail-closed compositor allowlist. Test fixtures are explicit and never
/// consulted outside tests, avoiding ambient-env claims in production.
pub fn compositor_allows_background_input() -> bool {
    #[cfg(test)]
    {
        if std::env::var_os("NANO_CUA_TEST_WAYLAND_RESTRICTED").is_some() {
            return false;
        }
        if std::env::var_os("NANO_CUA_TEST_WAYLAND_PERMISSIVE").is_some() {
            return true;
        }
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    desktop
        .split([':', ';'])
        .any(|part| matches!(part.trim(), "sway" | "river"))
        || (desktop.contains("kde") && std::env::var_os("LIBEI_SOCKET").is_some())
}

#[cfg(target_os = "linux")]
pub struct LinuxWaylandBackend;
#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl crate::ComputerUseBackend for LinuxWaylandBackend {
    fn name(&self) -> &'static str {
        "linux-wayland"
    }
    fn platform(&self) -> crate::Platform {
        crate::Platform::LinuxWayland
    }
    async fn frontmost_app(&self) -> crate::CuaResult<Option<String>> {
        Ok(None)
    }
    async fn dispatch(
        &self,
        _: Option<&str>,
        _: crate::CuaOp,
    ) -> crate::CuaResult<crate::CuaOpResult> {
        Err(crate::CuaError::BackendUnavailable {
            reason: "Wayland helper dispatch must be supplied through nano-platform SpawnSpec",
        })
    }
}
