//! Windows backend re-expressed on the repository-pinned windows-sys 0.52.
//! Input never activates a window; focus is resolved and checked just before dispatch.

use crate::{
    ComputerUseBackend, CuaError, CuaOp, CuaOpResult, CuaResult, MouseButton, Platform, Region,
    ScreenshotFormat,
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
            INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
            MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
            MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_CXSCREEN,
            SM_CYSCREEN,
        },
    },
};

pub struct WindowsBackend;

pub fn physical_to_normalized(x: i32, y: i32, width: i32, height: i32) -> CuaResult<(i32, i32)> {
    if width <= 0 || height <= 0 || x < 0 || y < 0 || x >= width || y >= height {
        return Err(CuaError::CoordinateOutOfRange);
    }
    Ok((
        ((i64::from(x) * 65_535) / i64::from(width - 1).max(1)) as i32,
        ((i64::from(y) * 65_535) / i64::from(height - 1).max(1)) as i32,
    ))
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

fn click(x: i32, y: i32, button: MouseButton, double: bool) -> CuaResult<CuaOpResult> {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let (nx, ny) = physical_to_normalized(x, y, width, height)?;
    let (down, up) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let mouse = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut inputs = vec![
        mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE),
        mouse(down),
        mouse(up),
    ];
    if double {
        inputs.extend([mouse(down), mouse(up)]);
    }
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        return Err(CuaError::OsPermissionDenied {
            remedy: "run Nano at the same integrity level as the focused application",
        });
    }
    Ok(CuaOpResult::Ok)
}

fn screenshot(region: Region, format: ScreenshotFormat, redact: bool) -> CuaResult<CuaOpResult> {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let full_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let full_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
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
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
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
        if frontmost()?.as_deref() != expected {
            return Err(CuaError::FocusLost);
        }
        match op {
            CuaOp::LeftClick { x, y, button, .. } => click(x, y, button, false),
            CuaOp::RightClick { x, y, .. } => click(x, y, MouseButton::Right, false),
            CuaOp::DoubleClick { x, y, button } => click(x, y, button, true),
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
            _ => Err(CuaError::BackendUnavailable {
                reason: "operation backend is pending integrator wiring",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coordinates_reject_instead_of_clamp() {
        assert!(matches!(
            physical_to_normalized(-1, 0, 1920, 1080),
            Err(CuaError::CoordinateOutOfRange)
        ));
        assert_eq!(physical_to_normalized(0, 0, 1920, 1080).unwrap(), (0, 0));
    }
}
