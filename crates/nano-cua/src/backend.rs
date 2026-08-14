use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{CuaOp, CuaOpResult, CuaResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    MacOs,
    LinuxX11,
    LinuxWayland,
    Windows,
    Unsupported,
}

impl Platform {
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                Self::LinuxWayland
            } else {
                Self::LinuxX11
            }
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            Self::Unsupported
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Region {
    #[default]
    Full,
    Rect {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyMods {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub meta: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    #[default]
    Png,
}

#[async_trait]
pub trait ComputerUseBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn platform(&self) -> Platform;
    async fn dispatch(
        &self,
        expected_frontmost_app: Option<&str>,
        op: CuaOp,
    ) -> CuaResult<CuaOpResult>;
    async fn frontmost_app(&self) -> CuaResult<Option<String>>;
}
