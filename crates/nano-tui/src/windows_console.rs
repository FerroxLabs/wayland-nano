//! Windows console VT input mode handling — port of the donor pattern
//! (codex `tui/src/tui/windows_console.rs`, see UPSTREAM.md).
//!
//! On Windows, crossterm raw mode does not manage
//! ENABLE_VIRTUAL_TERMINAL_INPUT the way the TUI needs: the input mode must
//! be switched to INPUT_RECORD mode while the TUI runs and restored exactly
//! on exit (including the original VT-input bit). The pure mode math is
//! platform-independent and unit-tested here; the Win32 calls are
//! `cfg(windows)` only (windows-sys 0.52, the workspace pin).

const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VirtualTerminalInput {
    Enabled,
    Disabled,
}

pub(crate) fn input_record_mode(mode: u32) -> u32 {
    mode & !ENABLE_VIRTUAL_TERMINAL_INPUT
}

pub(crate) fn restored_input_mode(mode: u32, original: VirtualTerminalInput) -> u32 {
    match original {
        VirtualTerminalInput::Enabled => mode | ENABLE_VIRTUAL_TERMINAL_INPUT,
        VirtualTerminalInput::Disabled => input_record_mode(mode),
    }
}

#[cfg(windows)]
static ORIGINAL_VT_INPUT: std::sync::Mutex<Vec<VirtualTerminalInput>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(windows)]
fn current_input_mode() -> Option<(windows_sys::Win32::Foundation::HANDLE, u32)> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::GetConsoleMode;
    use windows_sys::Win32::System::Console::GetStdHandle;
    use windows_sys::Win32::System::Console::STD_INPUT_HANDLE;

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle == INVALID_HANDLE_VALUE || handle == 0 {
        return None;
    }

    let mut mode = 0;
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return None;
    }

    Some((handle, mode))
}

/// Switch the console input to INPUT_RECORD mode, remembering the original
/// VT-input bit for [`restore_input_mode`]. No-op when there is no console
/// (e.g. redirected stdin under CI).
#[cfg(windows)]
pub fn set_input_record_mode() -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::SetConsoleMode;

    let Some((handle, mode)) = current_input_mode() else {
        return Ok(());
    };
    let requested_mode = input_record_mode(mode);
    if requested_mode != mode && unsafe { SetConsoleMode(handle, requested_mode) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let original = if mode & ENABLE_VIRTUAL_TERMINAL_INPUT != 0 {
        VirtualTerminalInput::Enabled
    } else {
        VirtualTerminalInput::Disabled
    };
    ORIGINAL_VT_INPUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(original);
    Ok(())
}

/// Re-assert INPUT_RECORD mode (defense against a child process or shell
/// resetting the console mode while the TUI is alive).
#[cfg(windows)]
pub fn ensure_input_record_mode() -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::SetConsoleMode;

    let Some((handle, mode)) = current_input_mode() else {
        return Ok(());
    };
    let requested_mode = input_record_mode(mode);
    if requested_mode != mode && unsafe { SetConsoleMode(handle, requested_mode) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(())
}

/// Restore the input mode captured by [`set_input_record_mode`], preserving
/// the original VT-input bit exactly (LIFO with nested enter/exit).
#[cfg(windows)]
pub fn restore_input_mode() -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::SetConsoleMode;

    let mut original_modes = ORIGINAL_VT_INPUT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(original) = original_modes.last().copied() else {
        return Ok(());
    };
    let Some((handle, mode)) = current_input_mode() else {
        original_modes.pop();
        return Ok(());
    };
    let requested_mode = restored_input_mode(mode, original);
    if requested_mode != mode && unsafe { SetConsoleMode(handle, requested_mode) } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    original_modes.pop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENABLE_WINDOW_INPUT: u32 = 0x0008;
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;

    #[test]
    fn input_record_mode_clears_vt_input_bit() {
        let mode = ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_WINDOW_INPUT | ENABLE_PROCESSED_INPUT;
        assert_eq!(
            input_record_mode(mode),
            ENABLE_WINDOW_INPUT | ENABLE_PROCESSED_INPUT
        );
    }

    #[test]
    fn restored_mode_preserves_original_vt_bit() {
        let base = ENABLE_WINDOW_INPUT | ENABLE_PROCESSED_INPUT;
        assert_eq!(
            restored_input_mode(base, VirtualTerminalInput::Enabled),
            base | ENABLE_VIRTUAL_TERMINAL_INPUT
        );
        assert_eq!(
            restored_input_mode(base, VirtualTerminalInput::Disabled),
            base
        );
        // Restore from a mode where something else re-set the VT bit: an
        // originally-disabled console must end disabled.
        assert_eq!(
            restored_input_mode(
                base | ENABLE_VIRTUAL_TERMINAL_INPUT,
                VirtualTerminalInput::Disabled
            ),
            base
        );
    }
}
