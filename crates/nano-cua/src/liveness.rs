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

    pub fn unavailable(&self) -> Option<&Unavailable> {
        match self {
            Self::Unavailable(u) => Some(u),
            _ => None,
        }
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
        assert!(!CuaLiveness::Ready { via: "x" }.should_narrow());
        assert!(!CuaLiveness::Indeterminate { platform: "x" }.should_narrow());
    }

    /// The headless-Linux arm is the point of the probe: no display must
    /// narrow the capability; a display must keep it. Both directions are
    /// asserted so the probe cannot be trivially always-true/always-false.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_narrows_without_a_display_and_keeps_it_with_one() {
        let _guard = crate::ENV_LOCK.lock().unwrap();
        let prior_x11 = std::env::var_os("DISPLAY");
        let prior_wl = std::env::var_os("WAYLAND_DISPLAY");
        unsafe {
            std::env::remove_var("DISPLAY");
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let headless = probe();
        assert!(
            headless.should_narrow(),
            "a machine with no display still advertised computer use: {headless:?}"
        );
        assert!(headless.unavailable().is_some());
        unsafe { std::env::set_var("DISPLAY", ":0") };
        assert!(
            !probe().should_narrow(),
            "a machine WITH a display lost the capability"
        );
        unsafe {
            match prior_x11 {
                Some(v) => std::env::set_var("DISPLAY", v),
                None => std::env::remove_var("DISPLAY"),
            }
            if let Some(v) = prior_wl {
                std::env::set_var("WAYLAND_DISPLAY", v);
            }
        }
    }

    /// macOS/Windows must not narrow — there is no honest non-executing
    /// probe for a window-server session; guessing would strip a working
    /// feature (and the capability flag stays FALSE regardless until the
    /// §7.2 live proof exists).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn gui_platforms_do_not_narrow() {
        assert!(!probe().should_narrow(), "{:?}", probe());
    }
}
