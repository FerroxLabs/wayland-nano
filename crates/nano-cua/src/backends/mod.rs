use crate::{ComputerUseBackend, Platform};
use std::sync::Arc;

pub mod linux_wayland;
#[cfg(target_os = "linux")]
pub mod linux_x11;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub fn for_platform(platform: Platform) -> Option<Arc<dyn ComputerUseBackend>> {
    match platform {
        #[cfg(target_os = "windows")]
        Platform::Windows => Some(Arc::new(windows::WindowsBackend)),
        #[cfg(target_os = "macos")]
        Platform::MacOs => Some(Arc::new(macos::MacOsBackend)),
        #[cfg(target_os = "linux")]
        Platform::LinuxX11 => Some(Arc::new(linux_x11::LinuxX11Backend)),
        #[cfg(target_os = "linux")]
        Platform::LinuxWayland if linux_wayland::compositor_allows_background_input() => {
            Some(Arc::new(linux_wayland::LinuxWaylandBackend))
        }
        _ => None,
    }
}
