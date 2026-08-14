pub struct MacOsBackend;
#[async_trait::async_trait]
impl crate::ComputerUseBackend for MacOsBackend {
    fn name(&self) -> &'static str {
        "macos"
    }
    fn platform(&self) -> crate::Platform {
        crate::Platform::MacOs
    }
    async fn frontmost_app(&self) -> crate::CuaResult<Option<String>> {
        Ok(None)
    }
    async fn dispatch(
        &self,
        _: Option<&str>,
        _: crate::CuaOp,
    ) -> crate::CuaResult<crate::CuaOpResult> {
        Err(crate::CuaError::OsPermissionDenied {
            remedy: "grant Terminal/Nano Accessibility and Screen Recording in System Settings > Privacy",
        })
    }
}
