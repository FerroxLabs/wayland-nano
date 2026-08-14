//! Linux X11 backend — XTest `fake_input` via `x11rb` + `get_image`
//! screenshot. XTest posts at the server's as-if-from-the-real-device
//! layer and does NOT activate the target window (no `set_input_focus`,
//! no `XSendEvent`). Feature-gated (`x11`, default on); without the
//! feature or without `DISPLAY` every op fails fast with a typed error —
//! never a silent no-op (design §5.4).

use std::time::Duration;

use async_trait::async_trait;

use crate::{
    ComputerUseBackend, CuaError, CuaOp, CuaOpResult, CuaResult, KeyMods, MouseButton, Platform,
    Region, ScreenshotFormat,
};

pub struct LinuxX11Backend;

/// Frontmost window class via `xdotool`, fixed argv (no model data, no
/// shell). Probe failure yields `None` (unresolved) — policy fail-closes
/// to a mandatory prompt when app-scoped rules are configured (§2.3).
async fn xdotool_frontmost() -> Option<String> {
    let res = tokio::process::Command::new("xdotool")
        .args(["getactivewindow", "getwindowclassname"])
        .output();
    match tokio::time::timeout(Duration::from_millis(500), res).await {
        Ok(Ok(out)) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    }
}

#[async_trait]
impl ComputerUseBackend for LinuxX11Backend {
    fn name(&self) -> &'static str {
        "linux-x11"
    }
    fn platform(&self) -> Platform {
        Platform::LinuxX11
    }
    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        Ok(xdotool_frontmost().await)
    }
    async fn dispatch(&self, expected: Option<&str>, op: CuaOp) -> CuaResult<CuaOpResult> {
        // Approve-then-recheck (design §5.1).
        if xdotool_frontmost().await.as_deref() != expected {
            return Err(CuaError::FocusLost);
        }
        match op {
            CuaOp::LeftClick { x, y, button, mods } => x11::mouse_click(x, y, button, mods, false),
            CuaOp::RightClick { x, y, mods } => {
                x11::mouse_click(x, y, MouseButton::Right, mods, false)
            }
            CuaOp::DoubleClick { x, y, button } => {
                x11::mouse_click(x, y, button, KeyMods::default(), true)
            }
            CuaOp::Scroll { x, y, dx, dy } => x11::scroll(x, y, dx, dy),
            CuaOp::Type { text } => x11::type_text(&text),
            CuaOp::Key { keys, .. } => x11::key_combo(&keys),
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => x11::screenshot(region, format, redact),
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp {
                app_id: xdotool_frontmost().await,
            }),
            // Not on the v1 model surface (`op.rs` exposure guard).
            _ => Err(CuaError::BackendUnavailable {
                reason: "operation is not part of the v1 model surface",
            }),
        }
    }
}

#[cfg(feature = "x11")]
mod x11 {
    use super::*;
    use base64::Engine;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat as XImageFormat};
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    /// Raw X11 error strings stay logs-side (closed-vocabulary error
    /// discipline): every backend failure collapses to `CuaError::Backend`.
    ///
    /// x11rb requests return lazy `VoidCookie`s — a protocol error only
    /// surfaces on the next round-trip, so every op ends in `sync_x11`,
    /// which is where a rejected injection becomes a typed error.
    fn connect() -> CuaResult<(RustConnection, usize)> {
        if std::env::var_os("DISPLAY").is_none() {
            return Err(CuaError::BackendUnavailable {
                reason: "DISPLAY is unset",
            });
        }
        RustConnection::connect(None).map_err(|_| CuaError::Backend)
    }

    /// x11rb has no `.sync()`; flush + a cheap round-trip getter is the
    /// documented `XSync` equivalent.
    fn sync_x11(conn: &RustConnection) -> CuaResult<()> {
        conn.flush().map_err(|_| CuaError::Backend)?;
        let _ = conn
            .get_input_focus()
            .map_err(|_| CuaError::Backend)?
            .reply()
            .map_err(|_| CuaError::Backend)?;
        Ok(())
    }

    /// Post-scaling coordinates must land inside the primary screen —
    /// reject, never clamp (design Q6).
    fn check_bounds(x: i32, y: i32, w: u16, h: u16) -> CuaResult<()> {
        if x < 0 || y < 0 || x >= i32::from(w) || y >= i32::from(h) {
            return Err(CuaError::CoordinateOutOfRange);
        }
        Ok(())
    }

    /// X button numbering: 1=Left, 2=Middle, 3=Right, 4/5=scroll up/down,
    /// 6/7=scroll left/right.
    fn button_code(b: MouseButton) -> u8 {
        match b {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
        }
    }

    pub fn mouse_click(
        x: i32,
        y: i32,
        button: MouseButton,
        mods: KeyMods,
        double: bool,
    ) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let screen = &conn.setup().roots[screen_idx];
        check_bounds(x, y, screen.width_in_pixels, screen.height_in_pixels)?;
        let root = screen.root;
        let held = press_mods(&conn, mods);
        // XTest button events use the CURRENT pointer position; move first.
        let _ = conn.xtest_fake_input(/*MotionNotify*/ 6, 0, 0, root, x as i16, y as i16, 0);
        let btn = button_code(button);
        for _ in 0..if double { 2 } else { 1 } {
            let _ = conn.xtest_fake_input(/*ButtonPress*/ 4, btn, 0, root, 0, 0, 0);
            let _ = conn.xtest_fake_input(/*ButtonRelease*/ 5, btn, 0, root, 0, 0, 0);
        }
        let result = sync_x11(&conn);
        release_mods(&conn, held);
        result?;
        Ok(CuaOpResult::Ok)
    }

    pub fn scroll(x: i32, y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let screen = &conn.setup().roots[screen_idx];
        check_bounds(x, y, screen.width_in_pixels, screen.height_in_pixels)?;
        let root = screen.root;
        // Button scrolls land on the window under the cursor; move first
        // to honor the scroll-AT-a-coordinate contract.
        let _ = conn.xtest_fake_input(6, 0, 0, root, x as i16, y as i16, 0);
        // Positive dy scrolls down (button 5); positive dx right (button 7).
        for (btn, ticks) in [
            (if dy < 0 { 4u8 } else { 5u8 }, dy.unsigned_abs()),
            (if dx < 0 { 6u8 } else { 7u8 }, dx.unsigned_abs()),
        ] {
            for _ in 0..ticks.min(120) {
                let _ = conn.xtest_fake_input(4, btn, 0, root, 0, 0, 0);
                let _ = conn.xtest_fake_input(5, btn, 0, root, 0, 0, 0);
            }
        }
        sync_x11(&conn)?;
        Ok(CuaOpResult::Ok)
    }

    pub fn type_text(text: &str) -> CuaResult<CuaOpResult> {
        let (conn, _) = connect()?;
        // The xdotool trick: map each char's keysym onto the scratch
        // keycode, press+release, repeat. Faithful for ASCII + Latin-1;
        // multi-byte chars use the Unicode keysym block.
        let scratch = conn.setup().max_keycode;
        for ch in text.chars() {
            let keysym = char_to_keysym(ch);
            let _ = conn.change_keyboard_mapping(1, scratch, 1, &[keysym, 0]);
            sync_x11(&conn)?;
            let _ = conn.xtest_fake_input(/*KeyPress*/ 2, scratch, 0, x11rb::NONE, 0, 0, 0);
            let _ = conn.xtest_fake_input(/*KeyRelease*/ 3, scratch, 0, x11rb::NONE, 0, 0, 0);
        }
        sync_x11(&conn)?;
        Ok(CuaOpResult::Ok)
    }

    pub fn key_combo(keys: &str) -> CuaResult<CuaOpResult> {
        let (conn, _) = connect()?;
        let (mods, keysym) = parse_combo_x11(keys).ok_or(CuaError::InvalidInput)?;
        let held = press_mods(&conn, mods);
        let scratch = conn.setup().max_keycode;
        let _ = conn.change_keyboard_mapping(1, scratch, 1, &[keysym, 0]);
        let result = sync_x11(&conn);
        let _ = conn.xtest_fake_input(2, scratch, 0, x11rb::NONE, 0, 0, 0);
        let _ = conn.xtest_fake_input(3, scratch, 0, x11rb::NONE, 0, 0, 0);
        let result = result.and_then(|()| sync_x11(&conn));
        release_mods(&conn, held);
        result?;
        Ok(CuaOpResult::Ok)
    }

    fn press_mods(conn: &RustConnection, mods: KeyMods) -> Vec<u8> {
        let scratch_base = conn.setup().max_keycode;
        let mut held = Vec::new();
        for (active, keysym, slot) in [
            (mods.shift, 0xffe1, scratch_base.saturating_sub(1)), // Shift_L
            (mods.ctrl, 0xffe3, scratch_base.saturating_sub(2)),  // Control_L
            (mods.alt, 0xffe9, scratch_base.saturating_sub(3)),   // Alt_L
            (mods.meta, 0xffeb, scratch_base.saturating_sub(4)),  // Super_L
        ] {
            if !active {
                continue;
            }
            let _ = conn.change_keyboard_mapping(1, slot, 1, &[keysym, 0]);
            let _ = sync_x11(conn);
            let _ = conn.xtest_fake_input(2, slot, 0, x11rb::NONE, 0, 0, 0);
            held.push(slot);
        }
        held
    }

    fn release_mods(conn: &RustConnection, held: Vec<u8>) {
        for slot in held.into_iter().rev() {
            let _ = conn.xtest_fake_input(3, slot, 0, x11rb::NONE, 0, 0, 0);
        }
        let _ = sync_x11(conn);
    }

    /// ASCII → X11 keysym; multi-byte chars use the Unicode keysym block
    /// (0x01000000 | codepoint).
    fn char_to_keysym(ch: char) -> u32 {
        let cp = ch as u32;
        if cp <= 0x7F { cp } else { 0x0100_0000 | cp }
    }

    fn parse_combo_x11(combo: &str) -> Option<(KeyMods, u32)> {
        let mut mods = KeyMods::default();
        let mut keysym: Option<u32> = None;
        for raw in combo.split(['+', '-', ' ']) {
            let tok = raw.trim().to_ascii_lowercase();
            if tok.is_empty() {
                continue;
            }
            match tok.as_str() {
                "cmd" | "command" | "meta" | "win" | "super" => mods.meta = true,
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "option" | "opt" => mods.alt = true,
                "shift" => mods.shift = true,
                "return" | "enter" => keysym = Some(0xff0d),
                "tab" => keysym = Some(0xff09),
                "escape" | "esc" => keysym = Some(0xff1b),
                "backspace" => keysym = Some(0xff08),
                "delete" => keysym = Some(0xffff),
                "space" => keysym = Some(0x0020),
                "left" => keysym = Some(0xff51),
                "up" => keysym = Some(0xff52),
                "right" => keysym = Some(0xff53),
                "down" => keysym = Some(0xff54),
                t if t.chars().count() == 1 => {
                    keysym = Some(char_to_keysym(t.chars().next()?));
                }
                _ => return None,
            }
        }
        keysym.map(|k| (mods, k))
    }

    pub fn screenshot(
        region: Region,
        format: ScreenshotFormat,
        redact: bool,
    ) -> CuaResult<CuaOpResult> {
        let (conn, screen_idx) = connect()?;
        let screen = &conn.setup().roots[screen_idx];
        let (sx, sy, sw, sh) = match region {
            Region::Full => (0i16, 0i16, screen.width_in_pixels, screen.height_in_pixels),
            Region::Rect {
                x,
                y,
                width,
                height,
            } => {
                check_bounds(x, y, screen.width_in_pixels, screen.height_in_pixels)?;
                let sw = u16::try_from(width).map_err(|_| CuaError::CoordinateOutOfRange)?;
                let sh = u16::try_from(height).map_err(|_| CuaError::CoordinateOutOfRange)?;
                if sw == 0 || sh == 0 {
                    return Err(CuaError::CoordinateOutOfRange);
                }
                (x as i16, y as i16, sw, sh)
            }
        };
        let reply = conn
            .get_image(
                XImageFormat::Z_PIXMAP,
                screen.root,
                sx,
                sy,
                sw,
                sh,
                u32::MAX,
            )
            .map_err(|_| CuaError::Backend)?
            .reply()
            .map_err(|_| CuaError::Backend)?;
        // 32-bpp little-endian BGRA on modern X servers; repack to RGBA.
        let mut rgba = Vec::with_capacity(usize::from(sw) * usize::from(sh) * 4);
        for chunk in reply.data.chunks_exact(4) {
            rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 0xff]);
        }
        let image = image::RgbaImage::from_raw(u32::from(sw), u32::from(sh), rgba)
            .ok_or(CuaError::Backend)?;
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .map_err(|_| CuaError::Backend)?;
        let (bytes, redacted) = if redact {
            crate::redact::redact_png_best_effort(bytes)
        } else {
            (bytes, false)
        };
        Ok(CuaOpResult::Screenshot {
            format,
            data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
            width: u32::from(sw),
            height: u32::from(sh),
            redacted,
        })
    }
}

/// Feature-off fallback: a typed blocker, never a silent no-op.
#[cfg(not(feature = "x11"))]
mod x11 {
    use super::*;

    const REASON: &str = "X11 backend requires the `x11` cargo feature (x11rb XTest support)";

    pub fn mouse_click(
        _: i32,
        _: i32,
        _: MouseButton,
        _: KeyMods,
        _: bool,
    ) -> CuaResult<CuaOpResult> {
        Err(CuaError::BackendUnavailable { reason: REASON })
    }
    pub fn scroll(_: i32, _: i32, _: i32, _: i32) -> CuaResult<CuaOpResult> {
        Err(CuaError::BackendUnavailable { reason: REASON })
    }
    pub fn type_text(_: &str) -> CuaResult<CuaOpResult> {
        Err(CuaError::BackendUnavailable { reason: REASON })
    }
    pub fn key_combo(_: &str) -> CuaResult<CuaOpResult> {
        Err(CuaError::BackendUnavailable { reason: REASON })
    }
    pub fn screenshot(_: Region, _: ScreenshotFormat, _: bool) -> CuaResult<CuaOpResult> {
        Err(CuaError::BackendUnavailable { reason: REASON })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[tokio::test]
    // DISPLAY must stay unset across the async dispatch; the std guard
    // serializing competing env-mutating tests is the intent.
    #[allow(clippy::await_holding_lock)]
    async fn unset_display_refuses_with_typed_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prior = std::env::var_os("DISPLAY");
        unsafe { std::env::remove_var("DISPLAY") };
        // Wait needs no display and must still work headless.
        let r = LinuxX11Backend
            .dispatch(None, CuaOp::Wait { duration_ms: 0 })
            .await;
        assert!(matches!(r, Ok(CuaOpResult::Ok)));
        // An input op without DISPLAY is a typed refusal, never a no-op.
        let r = LinuxX11Backend
            .dispatch(
                None,
                CuaOp::LeftClick {
                    x: 0,
                    y: 0,
                    button: MouseButton::Left,
                    mods: KeyMods::default(),
                },
            )
            .await;
        assert!(matches!(r, Err(CuaError::BackendUnavailable { .. })));
        unsafe {
            if let Some(v) = prior {
                std::env::set_var("DISPLAY", v);
            }
        }
    }
}
