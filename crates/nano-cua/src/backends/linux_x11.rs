pub struct LinuxX11Backend;
#[async_trait::async_trait]
impl crate::ComputerUseBackend for LinuxX11Backend {
    fn name(&self) -> &'static str {
        "linux-x11"
    }
    fn platform(&self) -> crate::Platform {
        crate::Platform::LinuxX11
    }
    async fn frontmost_app(&self) -> crate::CuaResult<Option<String>> {
        Ok(None)
    }
    async fn dispatch(
        &self,
        _: Option<&str>,
        _: crate::CuaOp,
    ) -> crate::CuaResult<crate::CuaOpResult> {
        if std::env::var_os("DISPLAY").is_none() {
            Err(crate::CuaError::BackendUnavailable {
                reason: "DISPLAY is unset",
            })
        } else {
            Err(crate::CuaError::BackendUnavailable {
                reason: "XTest dispatch awaits a live-gated host",
            })
        }
    }
}
