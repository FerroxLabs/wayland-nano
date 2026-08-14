//! macOS backend — `CGEvent` synthesized input + `CGDisplay` capture.
//! Events post at `CGEventTapLocation::HID`, which inserts at the HID
//! layer WITHOUT activating the target app; this module never calls
//! `activateWithOptions_:` or any other focus-stealing API. OS refusals
//! (TCC Accessibility for input, Screen Recording for capture) map to
//! typed `OsPermissionDenied` with a bounded remedy string (design §5.3).

use std::time::Duration;

use async_trait::async_trait;
use core_graphics::{
    display::CGDisplay,
    event::{
        CGEvent, CGEventTapLocation, CGEventType, CGKeyCode, CGMouseButton, EventField, KeyCode,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::{CGPoint, CGRect, CGSize},
};

use crate::{
    ComputerUseBackend, CuaError, CuaOp, CuaOpResult, CuaResult, KeyMods, MouseButton, Platform,
    Region, ScreenshotFormat,
};
use base64::Engine;

const ACCESSIBILITY_REMEDY: &str =
    "grant Terminal/Nano Accessibility in System Settings > Privacy & Security";
const SCREEN_RECORDING_REMEDY: &str =
    "grant Terminal/Nano Screen Recording in System Settings > Privacy & Security";

pub struct MacOsBackend;

/// Frontmost resolution via System Events. A probe failure yields `None`
/// (unresolved) — policy fail-closes to a mandatory prompt when any
/// app-scoped rule is configured (design §2.3).
async fn osascript_frontmost() -> Option<String> {
    let res = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(
            "tell application \"System Events\" to get name of first application process whose frontmost is true",
        )
        .output();
    match tokio::time::timeout(Duration::from_millis(500), res).await {
        Ok(Ok(out)) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        _ => None,
    }
}

/// A `CGEventSource` cannot be created without the Accessibility TCC
/// grant — this is the typed-denial boundary, not a generic failure.
fn make_source() -> CuaResult<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| {
        CuaError::OsPermissionDenied {
            remedy: ACCESSIBILITY_REMEDY,
        }
    })
}

fn cg_event(result: Result<CGEvent, ()>) -> CuaResult<CGEvent> {
    result.map_err(|_| CuaError::OsPermissionDenied {
        remedy: ACCESSIBILITY_REMEDY,
    })
}

fn map_mouse_button(button: MouseButton) -> (CGMouseButton, CGEventType, CGEventType) {
    match button {
        MouseButton::Left => (
            CGMouseButton::Left,
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
        ),
        MouseButton::Right => (
            CGMouseButton::Right,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
        ),
        MouseButton::Middle => (
            CGMouseButton::Center,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
        ),
    }
}

fn press_mods(source: &CGEventSource, mods: KeyMods) -> CuaResult<Vec<CGKeyCode>> {
    let mut held = Vec::new();
    for (active, code) in [
        (mods.shift, KeyCode::SHIFT),
        (mods.ctrl, KeyCode::CONTROL),
        (mods.alt, KeyCode::OPTION),
        (mods.meta, KeyCode::COMMAND),
    ] {
        if active {
            let ev = cg_event(CGEvent::new_keyboard_event(source.clone(), code, true))?;
            ev.post(CGEventTapLocation::HID);
            held.push(code);
        }
    }
    Ok(held)
}

fn release_mods(source: &CGEventSource, held: Vec<CGKeyCode>) {
    for code in held.into_iter().rev() {
        if let Ok(ev) = CGEvent::new_keyboard_event(source.clone(), code, false) {
            ev.post(CGEventTapLocation::HID);
        }
    }
}

fn mouse_click(
    x: i32,
    y: i32,
    button: MouseButton,
    mods: KeyMods,
    double: bool,
) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    let point = CGPoint::new(f64::from(x), f64::from(y));
    let (cg_button, down_ty, up_ty) = map_mouse_button(button);
    let held = press_mods(&source, mods)?;
    let down = cg_event(CGEvent::new_mouse_event(
        source.clone(),
        down_ty,
        point,
        cg_button,
    ))?;
    let up = cg_event(CGEvent::new_mouse_event(
        source.clone(),
        up_ty,
        point,
        cg_button,
    ))?;
    if double {
        // Click-state 2 triggers the OS double-click handler.
        down.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 2);
        up.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 2);
    }
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    release_mods(&source, held);
    Ok(CuaOpResult::Ok)
}

fn scroll(x: i32, y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    // Quartz binds wheel events to the cursor position; move first so the
    // scroll lands on the requested coordinate (still HID — no focus steal).
    let point = CGPoint::new(f64::from(x), f64::from(y));
    let move_ev = cg_event(CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::MouseMoved,
        point,
        CGMouseButton::Left,
    ))?;
    move_ev.post(CGEventTapLocation::HID);
    // Quartz wheel1 positive = scroll UP; negate to match the op contract
    // (positive dy = down, positive dx = right).
    let scroll_ev = cg_event(CGEvent::new_scroll_event(
        source,
        ScrollEventUnit::LINE,
        2,
        -dy,
        -dx,
        0,
    ))?;
    scroll_ev.post(CGEventTapLocation::HID);
    Ok(CuaOpResult::Ok)
}

fn type_text(text: &str) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    // UTF-16 string injection works for IME-friendly text without holding
    // modifier keys; chunked at 20 UTF-16 units under the CG buffer limit.
    for chunk in text.chars().collect::<Vec<_>>().chunks(20) {
        let utf16: Vec<u16> = chunk.iter().collect::<String>().encode_utf16().collect();
        let down = cg_event(CGEvent::new_keyboard_event(source.clone(), 0, true))?;
        down.set_string_from_utf16_unchecked(&utf16);
        down.post(CGEventTapLocation::HID);
        let up = cg_event(CGEvent::new_keyboard_event(source.clone(), 0, false))?;
        up.set_string_from_utf16_unchecked(&utf16);
        up.post(CGEventTapLocation::HID);
    }
    Ok(CuaOpResult::Ok)
}

fn key_combo(keys: &str) -> CuaResult<CuaOpResult> {
    let source = make_source()?;
    let (mods, code) = parse_combo_macos(keys).ok_or(CuaError::InvalidInput)?;
    let held = press_mods(&source, mods)?;
    let down = cg_event(CGEvent::new_keyboard_event(source.clone(), code, true))?;
    down.post(CGEventTapLocation::HID);
    let up = cg_event(CGEvent::new_keyboard_event(source.clone(), code, false))?;
    up.post(CGEventTapLocation::HID);
    release_mods(&source, held);
    Ok(CuaOpResult::Ok)
}

/// Apple HID virtual-key codes (kVK_ANSI_*) per `<HIToolbox/Events.h>`;
/// `core-graphics` only exposes special keys via `KeyCode`.
fn ansi_keycode(token: &str) -> Option<CGKeyCode> {
    Some(match token {
        "a" => 0x00,
        "b" => 0x0B,
        "c" => 0x08,
        "d" => 0x02,
        "e" => 0x0E,
        "f" => 0x03,
        "g" => 0x05,
        "h" => 0x04,
        "i" => 0x22,
        "j" => 0x26,
        "k" => 0x28,
        "l" => 0x25,
        "m" => 0x2E,
        "n" => 0x2D,
        "o" => 0x1F,
        "p" => 0x23,
        "q" => 0x0C,
        "r" => 0x0F,
        "s" => 0x01,
        "t" => 0x11,
        "u" => 0x20,
        "v" => 0x09,
        "w" => 0x0D,
        "x" => 0x07,
        "y" => 0x10,
        "z" => 0x06,
        "0" => 0x1D,
        "1" => 0x12,
        "2" => 0x13,
        "3" => 0x14,
        "4" => 0x15,
        "5" => 0x17,
        "6" => 0x16,
        "7" => 0x1A,
        "8" => 0x1C,
        "9" => 0x19,
        _ => return None,
    })
}

fn parse_combo_macos(combo: &str) -> Option<(KeyMods, CGKeyCode)> {
    let mut mods = KeyMods::default();
    let mut keycode: Option<CGKeyCode> = None;
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
            "space" | "spacebar" => keycode = Some(KeyCode::SPACE),
            "return" | "enter" => keycode = Some(KeyCode::RETURN),
            "tab" => keycode = Some(KeyCode::TAB),
            "escape" | "esc" => keycode = Some(KeyCode::ESCAPE),
            "backspace" | "delete" => keycode = Some(KeyCode::DELETE),
            "left" => keycode = Some(KeyCode::LEFT_ARROW),
            "right" => keycode = Some(KeyCode::RIGHT_ARROW),
            "up" => keycode = Some(KeyCode::UP_ARROW),
            "down" => keycode = Some(KeyCode::DOWN_ARROW),
            t => keycode = Some(ansi_keycode(t)?),
        }
    }
    keycode.map(|c| (mods, c))
}

fn screenshot(region: Region, format: ScreenshotFormat, redact: bool) -> CuaResult<CuaOpResult> {
    let display = CGDisplay::main();
    let bounds = display.bounds();
    let crop = match region {
        Region::Full => bounds,
        Region::Rect {
            x,
            y,
            width,
            height,
        } => CGRect::new(
            &CGPoint::new(f64::from(x), f64::from(y)),
            &CGSize::new(f64::from(width), f64::from(height)),
        ),
    };
    // A null image here is the Screen Recording TCC denial — typed, with
    // remedy, never a silent no-op.
    let image = display
        .image_for_rect(crop)
        .ok_or(CuaError::OsPermissionDenied {
            remedy: SCREEN_RECORDING_REMEDY,
        })?;
    let width = image.width() as u32;
    let height = image.height() as u32;
    let bytes_per_row = image.bytes_per_row();
    let data = image.data();
    let raw: &[u8] = data.bytes();
    // CGImage pixels are BGRA — repack to RGBA for the PNG encoder.
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in raw.chunks(bytes_per_row).take(height as usize) {
        for px in row[..width as usize * 4].chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    let image = image::RgbaImage::from_raw(width, height, rgba).ok_or(CuaError::Backend)?;
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
        width,
        height,
        redacted,
    })
}

#[async_trait]
impl ComputerUseBackend for MacOsBackend {
    fn name(&self) -> &'static str {
        "macos"
    }
    fn platform(&self) -> Platform {
        Platform::MacOs
    }
    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        Ok(osascript_frontmost().await)
    }
    async fn dispatch(&self, expected: Option<&str>, op: CuaOp) -> CuaResult<CuaOpResult> {
        // Approve-then-recheck (design §5.1): the frontmost app is
        // re-resolved immediately before dispatch and compared against
        // the value the approval prompt showed.
        if osascript_frontmost().await.as_deref() != expected {
            return Err(CuaError::FocusLost);
        }
        match op {
            CuaOp::LeftClick { x, y, button, mods } => mouse_click(x, y, button, mods, false),
            CuaOp::RightClick { x, y, mods } => mouse_click(x, y, MouseButton::Right, mods, false),
            CuaOp::DoubleClick { x, y, button } => {
                mouse_click(x, y, button, KeyMods::default(), true)
            }
            CuaOp::Scroll { x, y, dx, dy } => scroll(x, y, dx, dy),
            CuaOp::Type { text } => type_text(&text),
            CuaOp::Key { keys, .. } => key_combo(&keys),
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => screenshot(region, format, redact),
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp {
                app_id: osascript_frontmost().await,
            }),
            // Not on the v1 model surface (`op.rs` exposure guard).
            _ => Err(CuaError::BackendUnavailable {
                reason: "operation is not part of the v1 model surface",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_parser_maps_common_shortcuts() {
        let (mods, code) = parse_combo_macos("cmd+shift+a").unwrap();
        assert!(mods.meta && mods.shift);
        assert_eq!(code, 0x00 /* kVK_ANSI_A */);
        let (mods, code) = parse_combo_macos("ctrl-q").unwrap();
        assert!(mods.ctrl);
        assert_eq!(code, 0x0C /* kVK_ANSI_Q */);
        let (mods, _) = parse_combo_macos("option escape").unwrap();
        assert!(mods.alt);
        assert!(parse_combo_macos("nonsense+key").is_none());
    }
}
