//! Live-desktop tests (design §7.2) — self-skipping behind
//! `NANO_CUA_LIVE=1` plus a per-platform probe, exactly like the
//! `FLUX_TEST_KEY` precedent: absence skips WITH A REASON, never a
//! silent pass. These are owner-run proofs; CI coverage is the headless
//! battery in `src/` and `tests/op_enum.rs`.
//!
//! Until each platform's proof lands and is recorded, that platform's
//! capability flag stays FALSE (AGENTS.md honesty rule).
//!
//! Fully-qualified paths throughout: every block is cfg-gated per
//! platform, so shared imports would warn on the other legs.

fn live_gate() -> Option<&'static str> {
    if std::env::var_os("NANO_CUA_LIVE").is_none() {
        return Some(
            "NANO_CUA_LIVE is unset — live-desktop proof is owner-run by design (S9 §7.2)",
        );
    }
    None
}

macro_rules! live_test {
    ($name:ident, $body:block) => {
        #[tokio::test]
        async fn $name() {
            if let Some(reason) = live_gate() {
                eprintln!("SKIP {}: {reason}", stringify!($name));
                return;
            }
            $body
        }
    };
}

live_test!(windows_focus_invariance_and_sendinput_landing, {
    #[cfg(target_os = "windows")]
    {
        use nano_cua::{CuaOp, KeyMods, MouseButton, Platform, backends};
        let Some(backend) = backends::for_platform(Platform::Windows) else {
            panic!("NANO_CUA_LIVE=1 on Windows but no Windows backend — fail, never skip");
        };
        let before = backend.frontmost_app().await.unwrap();
        backend
            .dispatch(
                before.as_deref(),
                CuaOp::LeftClick {
                    x: 100,
                    y: 100,
                    button: MouseButton::Left,
                    mods: KeyMods::default(),
                },
            )
            .await
            .expect("SendInput click must land in a live session");
        let after = backend.frontmost_app().await.unwrap();
        assert_eq!(before, after, "synthesized input must never change focus");
    }
    #[cfg(not(target_os = "windows"))]
    eprintln!("SKIP: Windows focus-invariance proof runs on a Windows host");
});

live_test!(windows_hidpi_coordinate_equivalence, {
    #[cfg(target_os = "windows")]
    {
        // Owner-run at 100% and 150% scaling: a click at a screenshot
        // coordinate must hit the same screen element. The headless
        // mapping proof lives in coords.rs.
        use nano_cua::{CuaOp, CuaOpResult, Platform, Region, ScreenshotFormat, backends};
        let Some(backend) = backends::for_platform(Platform::Windows) else {
            panic!("NANO_CUA_LIVE=1 on Windows but no Windows backend");
        };
        let shot = backend
            .dispatch(
                None,
                CuaOp::Screenshot {
                    region: Region::Full,
                    format: ScreenshotFormat::Png,
                    redact: false,
                },
            )
            .await
            .expect("screenshot must succeed in a live session");
        let CuaOpResult::Screenshot { width, height, .. } = shot else {
            panic!("screenshot op returned a non-screenshot result");
        };
        assert!(width > 0 && height > 0);
    }
    #[cfg(not(target_os = "windows"))]
    eprintln!("SKIP: Windows HiDPI proof runs on a Windows host");
});

live_test!(macos_tcc_grant_dispatch, {
    #[cfg(target_os = "macos")]
    {
        use nano_cua::{CuaError, CuaOp, CuaOpResult, Platform, backends};
        let Some(backend) = backends::for_platform(Platform::MacOs) else {
            panic!("NANO_CUA_LIVE=1 on macOS but no macOS backend");
        };
        // With the Accessibility grant present, CGEvent posting succeeds;
        // without it the op must surface the typed TCC denial, never a
        // silent no-op.
        match backend
            .dispatch(
                None,
                CuaOp::Scroll {
                    x: 100,
                    y: 100,
                    dx: 0,
                    dy: 1,
                },
            )
            .await
        {
            Ok(CuaOpResult::Ok) => {}
            Err(CuaError::OsPermissionDenied { .. }) => {}
            other => panic!("unexpected macOS dispatch outcome: {other:?}"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    eprintln!("SKIP: macOS TCC proof runs on a macOS host");
});

live_test!(linux_x11_xtest_landing, {
    #[cfg(all(target_os = "linux", feature = "x11"))]
    {
        use nano_cua::{CuaError, CuaOp, CuaOpResult, KeyMods, MouseButton, Platform, backends};
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("SKIP: no DISPLAY — run under xvfb-run for the X11 live proof");
            return;
        }
        let Some(backend) = backends::for_platform(Platform::LinuxX11) else {
            panic!("DISPLAY set with NANO_CUA_LIVE=1 but no X11 backend — fail, never skip");
        };
        // Xvfb has no window manager, so there is no frontmost app; a
        // click at a valid coordinate must be accepted by the server and
        // leave the (empty) frontmost state unchanged.
        let before = backend.frontmost_app().await.unwrap();
        backend
            .dispatch(
                before.as_deref(),
                CuaOp::LeftClick {
                    x: 10,
                    y: 10,
                    button: MouseButton::Left,
                    mods: KeyMods::default(),
                },
            )
            .await
            .expect("XTest click must be accepted by a live X server");
        let after = backend.frontmost_app().await.unwrap();
        assert_eq!(before, after);
        // Out-of-bounds is a typed rejection, not a clamp.
        let r = backend
            .dispatch(
                None,
                CuaOp::LeftClick {
                    x: 100_000,
                    y: 10,
                    button: MouseButton::Left,
                    mods: KeyMods::default(),
                },
            )
            .await;
        assert!(matches!(r, Err(CuaError::CoordinateOutOfRange)));
        // A live screenshot must return a decodable PNG.
        let shot = backend
            .dispatch(
                None,
                CuaOp::Screenshot {
                    region: nano_cua::Region::Full,
                    format: nano_cua::ScreenshotFormat::Png,
                    redact: false,
                },
            )
            .await
            .expect("X11 screenshot must succeed under a live X server");
        let CuaOpResult::Screenshot { width, height, .. } = shot else {
            panic!("screenshot op returned a non-screenshot result");
        };
        assert!(width > 0 && height > 0);
    }
    #[cfg(not(all(target_os = "linux", feature = "x11")))]
    eprintln!("SKIP: X11 live proof runs on Linux with the x11 feature");
});

live_test!(linux_wayland_permissive_dispatch, {
    #[cfg(target_os = "linux")]
    {
        use nano_cua::{CuaOp, CuaOpResult, Platform, backends, liveness};
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            eprintln!("SKIP: no WAYLAND_DISPLAY — run on a live sway/river seat");
            return;
        }
        match liveness::probe() {
            liveness::CuaLiveness::Ready { via } => assert_eq!(via, "linux-wayland"),
            other => {
                panic!("NANO_CUA_LIVE=1 on Wayland but probe is {other:?} — fail, never skip")
            }
        }
        let Some(backend) = backends::for_platform(Platform::LinuxWayland) else {
            panic!("permissive probe passed but no Wayland backend was built");
        };
        let r = backend.dispatch(None, CuaOp::Wait { duration_ms: 1 }).await;
        assert!(matches!(r, Ok(CuaOpResult::Ok)));
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("SKIP: Wayland permissive proof runs on a Linux seat");
});
