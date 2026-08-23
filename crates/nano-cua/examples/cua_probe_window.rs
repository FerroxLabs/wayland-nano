//! CUA proof helper window (WP-0.1): a minimal Win32 overlapped window that
//! records every `WM_LBUTTONDOWN` client coordinate, then self-closes after an
//! idle timeout and prints ONE result JSON line for the caller to parse:
//! `{"probe":"cua_window","clicks":[{"x":..,"y":..}],"ticks":<ms>}`.
//! Exit codes: 0 = ran clean (>=0 clicks recorded), 2 = window-creation or
//! message-loop failure. External process state, never self-report.

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer,
        MSG, PostQuitMessage, RegisterClassW, SW_SHOW, SetTimer, ShowWindow, TranslateMessage,
        WM_DESTROY, WM_LBUTTONDOWN, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };

    pub const CLASS_NAME: &str = "NanoCuaProbeWindow\0";
    const IDLE_TIMER_ID: usize = 1;
    const IDLE_TIMEOUT_MS: u32 = 1500;
    const WIDTH: i32 = 400;
    const HEIGHT: i32 = 300;

    // Recorded clicks as packed (x << 32) | y client coordinates.
    static CLICKS: [AtomicI32; 64] = [const { AtomicI32::new(-1) }; 64];
    static CLICK_COUNT: AtomicU64 = AtomicU64::new(0);
    static LAST_TICK_MS: AtomicU64 = AtomicU64::new(0);
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    fn now_ms() -> u64 {
        START
            .get()
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_LBUTTONDOWN => {
                let x = (lparam & 0xffff) as i16 as i32;
                let y = ((lparam >> 16) & 0xffff) as i16 as i32;
                let index = CLICK_COUNT.fetch_add(1, Ordering::SeqCst);
                if index < 64 {
                    CLICKS[index as usize].store(x, Ordering::SeqCst);
                    CLICKS_Y[index as usize].store(y, Ordering::SeqCst);
                }
                LAST_TICK_MS.store(now_ms(), Ordering::SeqCst);
                0
            }
            WM_TIMER => {
                let last = LAST_TICK_MS.load(Ordering::SeqCst);
                let now = now_ms();
                if last != 0 && now - last > IDLE_TIMEOUT_MS as u64 {
                    unsafe { KillTimer(hwnd, IDLE_TIMER_ID) };
                    unsafe { DestroyWindow(hwnd) };
                }
                0
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    static CLICKS_Y: [AtomicI32; 64] = [const { AtomicI32::new(-1) }; 64];

    pub fn run() -> i32 {
        let start = std::time::Instant::now();
        let _ = START.set(start);
        let class: Vec<u16> = CLASS_NAME.encode_utf16().collect();
        let wnd = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: 0,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        if unsafe { RegisterClassW(&wnd) } == 0 {
            return 2;
        }
        let title: Vec<u16> = "nano-cua probe\0".encode_utf16().collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                200,
                200,
                WIDTH,
                HEIGHT,
                0,
                0,
                0,
                std::ptr::null(),
            )
        };
        if hwnd == 0 {
            return 2;
        }
        unsafe { ShowWindow(hwnd, SW_SHOW) };
        LAST_TICK_MS.store(now_ms(), Ordering::SeqCst); // idle clock starts at visibility
        unsafe { SetTimer(hwnd, IDLE_TIMER_ID, 200, None) };

        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let got = unsafe { GetMessageW(&mut msg, 0, 0, 0) };
            if got <= 0 {
                break; // WM_QUIT or error
            }
            unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }
        let _ = unsafe { KillTimer(hwnd, IDLE_TIMER_ID) };

        let count = CLICK_COUNT.load(Ordering::SeqCst).min(64);
        let mut out = String::from("{\"probe\":\"cua_window\",\"clicks\":[");
        for i in 0..count {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"x\":{},\"y\":{}}}",
                CLICKS[i as usize].load(Ordering::SeqCst),
                CLICKS_Y[i as usize].load(Ordering::SeqCst)
            ));
        }
        out.push_str(&format!("],\"ticks\":{}}}", start.elapsed().as_millis()));
        println!("{out}");
        0
    }
}

#[cfg(target_os = "windows")]
fn main() {
    std::process::exit(imp::run());
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("cua_probe_window is a Windows-only CUA proof helper");
    std::process::exit(2);
}
