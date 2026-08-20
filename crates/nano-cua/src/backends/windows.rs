//! Windows backend re-expressed on the repository-pinned windows-sys 0.52.
//! Input never activates a window (no `SetForegroundWindow`); focus is
//! resolved and re-checked just before dispatch (design §5.1). Coordinates
//! are physical pixels of the primary display (design Q6, `crate::coords`).

use crate::{
    ComputerUseBackend, CuaError, CuaOp, CuaOpResult, CuaResult, KeyMods, MouseButton, Platform,
    Region, ScreenshotFormat, coords,
};
use async_trait::async_trait;
use base64::Engine;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND},
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, RGBQUAD, ReleaseDC, SRCCOPY,
        SelectObject,
    },
    System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::{
        HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
            KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
            MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
            MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
            MOUSEINPUT, MapVirtualKeyW, SendInput,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_CXSCREEN,
            SM_CYSCREEN,
        },
    },
};

pub struct WindowsBackend;

fn physical_size() -> (i32, i32) {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    (unsafe { GetSystemMetrics(SM_CXSCREEN) }, unsafe {
        GetSystemMetrics(SM_CYSCREEN)
    })
}

fn frontmost() -> CuaResult<Option<String>> {
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd == 0 {
        return Ok(None);
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return Ok(None);
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process == 0 {
        return Ok(None);
    }
    let mut buf = vec![0u16; 32_768];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    unsafe { CloseHandle(process) };
    if ok == 0 {
        return Ok(None);
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Ok(path.rsplit(['\\', '/']).next().map(str::to_ascii_lowercase))
}

/// A short `SendInput` result means the call was blocked (UIPI/integrity
/// level) — a typed OS-permission denial, never a silent partial.
fn send(inputs: &[INPUT]) -> CuaResult<()> {
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        return Err(CuaError::OsPermissionDenied {
            remedy: "run Nano at the same integrity level as the focused application",
        });
    }
    Ok(())
}

fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse_input(nx: i32, ny: i32, data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mod_vks(mods: KeyMods) -> Vec<u16> {
    let mut held = Vec::new();
    for (active, vk) in [
        (mods.shift, 0x10), // VK_SHIFT
        (mods.ctrl, 0x11),  // VK_CONTROL
        (mods.alt, 0x12),   // VK_MENU (Alt)
        (mods.meta, 0x5B),  // VK_LWIN
    ] {
        if active {
            held.push(vk);
        }
    }
    held
}

fn press_mods(mods: KeyMods) -> CuaResult<Vec<u16>> {
    let held = mod_vks(mods);
    if !held.is_empty() {
        send(
            &held
                .iter()
                .map(|vk| key_input(*vk, 0, 0))
                .collect::<Vec<_>>(),
        )?;
    }
    Ok(held)
}

fn release_mods(held: Vec<u16>) {
    if !held.is_empty() {
        let _ = send(
            &held
                .into_iter()
                .rev()
                .map(|vk| key_input(vk, 0, KEYEVENTF_KEYUP))
                .collect::<Vec<_>>(),
        );
    }
}

fn click(
    x: i32,
    y: i32,
    button: MouseButton,
    mods: KeyMods,
    double: bool,
) -> CuaResult<CuaOpResult> {
    let (width, height) = physical_size();
    let (nx, ny) = coords::physical_to_normalized(x, y, width, height)?;
    let (down, up) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let held = press_mods(mods)?;
    let mut inputs = vec![mouse_input(
        nx,
        ny,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
    )];
    for _ in 0..if double { 2 } else { 1 } {
        inputs.push(mouse_input(nx, ny, 0, down));
        inputs.push(mouse_input(nx, ny, 0, up));
    }
    let result = send(&inputs);
    release_mods(held);
    result?;
    Ok(CuaOpResult::Ok)
}

fn scroll(x: i32, y: i32, dx: i32, dy: i32) -> CuaResult<CuaOpResult> {
    let (width, height) = physical_size();
    // Wheel events land on the window under the cursor, so honor the
    // scroll-AT-a-coordinate contract by moving first (also validates
    // the coordinate against the display bounds).
    let (nx, ny) = coords::physical_to_normalized(x, y, width, height)?;
    // Win32 wheel convention: 120 = one notch; positive WHEEL delta
    // scrolls up, so `dy` (positive = down) is negated.
    let mut inputs = vec![mouse_input(
        nx,
        ny,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
    )];
    if dy != 0 {
        inputs.push(mouse_input(
            0,
            0,
            (-i64::from(dy) * 120) as i32 as u32,
            MOUSEEVENTF_WHEEL,
        ));
    }
    if dx != 0 {
        inputs.push(mouse_input(
            0,
            0,
            (i64::from(dx) * 120) as i32 as u32,
            MOUSEEVENTF_HWHEEL,
        ));
    }
    send(&inputs)?;
    Ok(CuaOpResult::Ok)
}

fn type_text(text: &str) -> CuaResult<CuaOpResult> {
    // `KEYEVENTF_UNICODE` injects arbitrary code points without keymap
    // lookup; each UTF-16 unit emits a down+up pair. Chunked so a long
    // payload never exceeds a single `SendInput` call's practical limit.
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(64) {
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for unit in chunk {
            inputs.push(key_input(0, *unit, KEYEVENTF_UNICODE));
            inputs.push(key_input(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }
        send(&inputs)?;
    }
    Ok(CuaOpResult::Ok)
}

fn key_combo(keys: &str) -> CuaResult<CuaOpResult> {
    let (mods, vk) = parse_combo_win(keys).ok_or(CuaError::InvalidInput)?;
    let held = press_mods(mods)?;
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
    let result = send(&[key_input(vk, scan, 0), key_input(vk, scan, KEYEVENTF_KEYUP)]);
    release_mods(held);
    result?;
    Ok(CuaOpResult::Ok)
}

/// `ctrl+shift+t` / `command-q` / `alt f4` → (mods, virtual-key code).
fn parse_combo_win(combo: &str) -> Option<(KeyMods, u16)> {
    let mut mods = KeyMods::default();
    let mut vk: Option<u16> = None;
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
            "return" | "enter" => vk = Some(0x0D),
            "tab" => vk = Some(0x09),
            "escape" | "esc" => vk = Some(0x1B),
            "backspace" => vk = Some(0x08),
            "delete" => vk = Some(0x2E),
            "space" => vk = Some(0x20),
            "left" => vk = Some(0x25),
            "up" => vk = Some(0x26),
            "right" => vk = Some(0x27),
            "down" => vk = Some(0x28),
            t if t.len() == 1 => {
                let c = t.chars().next()?;
                if c.is_ascii_alphanumeric() {
                    vk = Some(c.to_ascii_uppercase() as u16);
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }
    vk.map(|v| (mods, v))
}

fn screenshot(region: Region, format: ScreenshotFormat, redact: bool) -> CuaResult<CuaOpResult> {
    let (full_w, full_h) = physical_size();
    let (x, y, w, h) = match region {
        Region::Full => (0, 0, full_w, full_h),
        Region::Rect {
            x,
            y,
            width,
            height,
        } => {
            let w = i32::try_from(width).map_err(|_| CuaError::CoordinateOutOfRange)?;
            let h = i32::try_from(height).map_err(|_| CuaError::CoordinateOutOfRange)?;
            if x < 0 || y < 0 || w <= 0 || h <= 0 || x + w > full_w || y + h > full_h {
                return Err(CuaError::CoordinateOutOfRange);
            }
            (x, y, w, h)
        }
    };
    unsafe {
        let screen = GetDC(0);
        if screen == 0 {
            return Err(CuaError::Backend);
        }
        let mem = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        if mem == 0 || bitmap == 0 {
            return Err(CuaError::Backend);
        }
        let old = SelectObject(mem, bitmap);
        if BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY) == 0 {
            return Err(CuaError::Backend);
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        let rows = GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            bgra.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(0, screen);
        if rows == 0 {
            return Err(CuaError::Backend);
        }
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255
        }
        let image =
            image::RgbaImage::from_raw(w as u32, h as u32, bgra).ok_or(CuaError::Backend)?;
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
            width: w as u32,
            height: h as u32,
            redacted,
        })
    }
}

#[async_trait]
impl ComputerUseBackend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }
    fn platform(&self) -> Platform {
        Platform::Windows
    }
    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        frontmost()
    }
    async fn dispatch(&self, expected: Option<&str>, op: CuaOp) -> CuaResult<CuaOpResult> {
        // Focus check (§5.1): mismatch ⇒ FocusLost, not dispatched. Exception:
        // a screenshot with NO recorded expectation is the §2.4 evidence path —
        // it captures, never injects, and failing it closed would brick every
        // pre/post-shot on a live desktop (something is ALWAYS frontmost there).
        // Input ops keep the strict rule: expected=None vs a live frontmost app
        // IS a mismatch and fails closed (first live run, F-49).
        let is_bare_screenshot = matches!(op, CuaOp::Screenshot { .. }) && expected.is_none();
        if !is_bare_screenshot && frontmost()?.as_deref() != expected {
            return Err(CuaError::FocusLost);
        }
        match op {
            CuaOp::LeftClick { x, y, button, mods } => click(x, y, button, mods, false),
            CuaOp::RightClick { x, y, mods } => click(x, y, MouseButton::Right, mods, false),
            CuaOp::DoubleClick { x, y, button } => click(x, y, button, KeyMods::default(), true),
            CuaOp::Scroll { x, y, dx, dy } => scroll(x, y, dx, dy),
            CuaOp::Type { text } => type_text(&text),
            CuaOp::Key { keys, .. } => key_combo(&keys),
            CuaOp::Screenshot {
                region,
                format,
                redact,
            } => screenshot(region, format, redact),
            CuaOp::Wait { duration_ms } => {
                tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
                Ok(CuaOpResult::Ok)
            }
            CuaOp::FrontmostApp {} => Ok(CuaOpResult::FrontmostApp {
                app_id: frontmost()?,
            }),
            // Not on the v1 model surface (`op.rs` exposure guard):
            // mouse_move (no standalone use without drag) and ax_tree
            // (donor's own Windows backend never implemented it).
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
        let (mods, vk) = parse_combo_win("ctrl+shift+t").unwrap();
        assert!(mods.ctrl && mods.shift && !mods.alt && !mods.meta);
        assert_eq!(vk, u16::from(b'T'));
        let (mods, vk) = parse_combo_win("shift delete").unwrap();
        assert!(mods.shift);
        assert_eq!(vk, 0x2E);
        let (_, enter) = parse_combo_win("enter").unwrap();
        assert_eq!(enter, 0x0D);
        assert!(parse_combo_win("nonsense+key").is_none());
        assert!(parse_combo_win("ctrl+").is_none());
    }
}
