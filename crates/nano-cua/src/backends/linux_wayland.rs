//! Linux Wayland backend — input via `wlrctl`, capture via `grim`.
//!
//! nano-platform is still a stub (5-line lib.rs, no SpawnSpec), so the
//! helper invocations follow the S5 precedent (`nano-mcp/src/stdio.rs`):
//! direct argv-mode spawn — each argument is a separate argv entry, no
//! shell interpreter, model-supplied data (typed text, combos) can never
//! be metacharacter-expanded. Fixed programs (`wlrctl`, `grim`); the only
//! variable argv elements are coordinates, deltas, and the text payload
//! itself. When nano-platform lands SpawnSpec these calls route through
//! it (docs/FOLLOWUPS.md).
//!
//! Restricted compositors (GNOME mutter default, Hyprland default) drop
//! unprivileged injection SILENTLY — worse than a hard error — so the
//! probe below refuses registration, and dispatch re-checks before every
//! input op (a compositor can become restricted mid-session).

#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use async_trait::async_trait;

#[cfg(target_os = "linux")]
use crate::{
    ComputerUseBackend, CuaError, CuaOp, CuaOpResult, CuaResult, MouseButton, Platform, Region,
    ScreenshotFormat,
};

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
fn op_is_input(op: &CuaOp) -> bool {
    matches!(
        op,
        CuaOp::LeftClick { .. }
            | CuaOp::RightClick { .. }
            | CuaOp::DoubleClick { .. }
            | CuaOp::MouseMove { .. }
            | CuaOp::Scroll { .. }
            | CuaOp::Type { .. }
            | CuaOp::Key { .. }
    )
}

/// Argv-mode spawn, the S5 stdio.rs precedent: no shell, fixed program,
/// non-zero exit is a typed backend failure. stderr text is discarded —
/// raw OS/helper strings stay logs-side (closed-vocabulary discipline).
#[cfg(target_os = "linux")]
async fn run_argv(program: &str, args: &[String]) -> CuaResult<Vec<u8>> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|_| CuaError::BackendUnavailable {
            reason: "required Wayland helper binary is missing from PATH",
        })?;
    if !out.status.success() {
        return Err(CuaError::Backend);
    }
    Ok(out.stdout)
}

#[cfg(target_os = "linux")]
pub struct LinuxWaylandBackend;

#[cfg(target_os = "linux")]
impl LinuxWaylandBackend {
    async fn dispatch_op(op: CuaOp) -> CuaResult<CuaOpResult> {
        match op {
            CuaOp::LeftClick { x, y, button, .. } => wlr_click(x, y, button, false).await,
            CuaOp::RightClick { x, y, .. } => wlr_click(x, y, MouseButton::Right, false).await,
            CuaOp::DoubleClick { x, y, button } => wlr_click(x, y, button, true).await,
            CuaOp::Scroll { x, y, dx, dy } => {
                wlr_move(x, y).await?;
                run_argv(
                    "wlrctl",
                    &[
                        "pointer".into(),
                        "scroll".into(),
                        dy.to_string(),
                        dx.to_string(),
                    ],
                )
                .await?;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Type { text } => {
                // `text` is a separate argv entry — never shell-interpreted.
                run_argv("wlrctl", &["keyboard".into(), "type".into(), text]).await?;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Key { keys, .. } => {
                run_argv("wlrctl", &["keyboard".into(), "press".into(), keys]).await?;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => grim_screenshot(region, format, redact).await,
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp { app_id: None }),
            // Not on the v1 model surface (`op.rs` exposure guard).
            _ => Err(CuaError::BackendUnavailable {
                reason: "operation is not part of the v1 model surface",
            }),
        }
    }
}

#[cfg(target_os = "linux")]
async fn wlr_move(x: i32, y: i32) -> CuaResult<()> {
    run_argv(
        "wlrctl",
        &[
            "pointer".into(),
            "move-to".into(),
            x.to_string(),
            y.to_string(),
        ],
    )
    .await?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn wlr_click(x: i32, y: i32, button: MouseButton, double: bool) -> CuaResult<CuaOpResult> {
    // Move first so the click lands at the intended target.
    wlr_move(x, y).await?;
    let btn = match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    };
    for _ in 0..if double { 2 } else { 1 } {
        run_argv("wlrctl", &["pointer".into(), "click".into(), btn.into()]).await?;
    }
    Ok(CuaOpResult::Ok)
}

#[cfg(target_os = "linux")]
async fn grim_screenshot(
    region: Region,
    format: ScreenshotFormat,
    redact: bool,
) -> CuaResult<CuaOpResult> {
    // `grim -` writes the PNG to stdout — no temp file.
    let args = match region {
        Region::Full => vec!["-".to_string()],
        Region::Rect {
            x,
            y,
            width,
            height,
        } => {
            if width == 0 || height == 0 {
                return Err(CuaError::CoordinateOutOfRange);
            }
            vec!["-g".into(), format!("{x},{y} {width}x{height}"), "-".into()]
        }
    };
    let bytes = run_argv("grim", &args).await?;
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|_| CuaError::Backend)?;
    let (bytes, redacted) = if redact {
        crate::redact::redact_png_best_effort(bytes)
    } else {
        (bytes, false)
    };
    use base64::Engine;
    Ok(CuaOpResult::Screenshot {
        format,
        data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        width: decoded.width(),
        height: decoded.height(),
        redacted,
    })
}

#[cfg(target_os = "linux")]
#[async_trait]
impl ComputerUseBackend for LinuxWaylandBackend {
    fn name(&self) -> &'static str {
        "linux-wayland"
    }
    fn platform(&self) -> Platform {
        Platform::LinuxWayland
    }
    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        // No reliable unprivileged frontmost probe on Wayland (donor's
        // was a test-only cache). Unresolved — policy fail-closes to a
        // mandatory prompt when app-scoped rules are configured (§2.3).
        Ok(None)
    }
    async fn dispatch(&self, _expected: Option<&str>, op: CuaOp) -> CuaResult<CuaOpResult> {
        // Defence-in-depth: re-check the compositor before every input op;
        // a mid-session restriction is a typed error, never silent clicks.
        if op_is_input(&op) && !compositor_allows_background_input() {
            return Err(CuaError::BackendUnavailable {
                reason: "compositor refused input injection mid-session",
            });
        }
        Self::dispatch_op(op).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    fn clear_fixtures() {
        unsafe {
            std::env::remove_var("NANO_CUA_TEST_WAYLAND_PERMISSIVE");
            std::env::remove_var("NANO_CUA_TEST_WAYLAND_RESTRICTED");
        }
    }

    #[test]
    fn permissive_fixture_yields_true_restricted_yields_false() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_fixtures();
        unsafe { std::env::set_var("NANO_CUA_TEST_WAYLAND_PERMISSIVE", "1") };
        assert!(compositor_allows_background_input());
        clear_fixtures();
        unsafe { std::env::set_var("NANO_CUA_TEST_WAYLAND_RESTRICTED", "1") };
        assert!(!compositor_allows_background_input());
        clear_fixtures();
    }

    #[test]
    fn production_allowlist_matches_only_proven_compositors() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_fixtures();
        let prior = std::env::var_os("XDG_CURRENT_DESKTOP");
        unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "sway") };
        assert!(compositor_allows_background_input());
        unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "GNOME") };
        assert!(
            !compositor_allows_background_input(),
            "mutter default is restricted — no probe pass, no registration"
        );
        unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "Hyprland") };
        assert!(!compositor_allows_background_input());
        unsafe { std::env::set_var("XDG_CURRENT_DESKTOP", "sway:wlroots") };
        assert!(compositor_allows_background_input());
        unsafe {
            match prior {
                Some(v) => std::env::set_var("XDG_CURRENT_DESKTOP", v),
                None => std::env::remove_var("XDG_CURRENT_DESKTOP"),
            }
        }
    }

    /// Restricted compositor ⇒ input ops fail typed before any helper runs;
    /// permissive fixture ⇒ the op reaches the helper, and a missing
    /// `wlrctl` is a typed BackendUnavailable, never a silent Ok.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    // The env fixture must stay set across the async dispatch; a std
    // guard serializes the competing tests, which is the intent.
    #[allow(clippy::await_holding_lock)]
    async fn dispatch_posture_follows_probe() {
        use crate::{KeyMods, MouseButton};
        let _guard = ENV_LOCK.lock().unwrap();
        let click = CuaOp::LeftClick {
            x: 0,
            y: 0,
            button: MouseButton::Left,
            mods: KeyMods::default(),
        };
        clear_fixtures();
        unsafe { std::env::set_var("NANO_CUA_TEST_WAYLAND_RESTRICTED", "1") };
        let r = LinuxWaylandBackend.dispatch(None, click.clone()).await;
        assert!(
            matches!(
                r,
                Err(CuaError::BackendUnavailable {
                    reason: "compositor refused input injection mid-session"
                })
            ),
            "restricted must refuse input: {r:?}"
        );
        clear_fixtures();
        unsafe { std::env::set_var("NANO_CUA_TEST_WAYLAND_PERMISSIVE", "1") };
        let r = LinuxWaylandBackend.dispatch(None, click).await;
        match r {
            Ok(CuaOpResult::Ok) => {} // wlrctl present and delivered
            Err(CuaError::BackendUnavailable { .. }) | Err(CuaError::Backend) => {}
            other => panic!("permissive path must be real or typed: {other:?}"),
        }
        clear_fixtures();
    }
}
