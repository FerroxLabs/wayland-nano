use crate::Platform;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: &'static str,
    pub remedy: &'static str,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuaLiveness {
    Ready { via: &'static str },
    Unavailable(Unavailable),
    Indeterminate { platform: &'static str },
}
impl CuaLiveness {
    pub fn should_narrow(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

pub fn probe() -> CuaLiveness {
    match Platform::current() {
        Platform::LinuxWayland
            if crate::backends::linux_wayland::compositor_allows_background_input() =>
        {
            CuaLiveness::Ready {
                via: "linux-wayland",
            }
        }
        Platform::LinuxWayland => CuaLiveness::Unavailable(Unavailable {
            reason: "active Wayland compositor does not prove input delivery",
            remedy: "use a supported sway, river, or Plasma/libei session",
        }),
        Platform::LinuxX11 if std::env::var_os("DISPLAY").is_some() => {
            CuaLiveness::Ready { via: "linux-x11" }
        }
        Platform::LinuxX11 => CuaLiveness::Unavailable(Unavailable {
            reason: "DISPLAY is unset",
            remedy: "run in a graphical X11 session",
        }),
        Platform::MacOs => CuaLiveness::Indeterminate { platform: "macos" },
        Platform::Windows => CuaLiveness::Indeterminate {
            platform: "windows",
        },
        Platform::Unsupported => CuaLiveness::Unavailable(Unavailable {
            reason: "no backend exists for this platform",
            remedy: "use Windows, macOS, X11, or a proven Wayland compositor",
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_positive_unavailability_narrows() {
        assert!(
            CuaLiveness::Unavailable(Unavailable {
                reason: "r",
                remedy: "m"
            })
            .should_narrow()
        );
        assert!(!CuaLiveness::Indeterminate { platform: "x" }.should_narrow());
    }
}
