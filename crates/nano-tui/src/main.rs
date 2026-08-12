//! nano-tui entry point: spawn `wayland-nano acp-host`, wire the channels,
//! run the app on the alternate screen (design doc §3/§4).
//!
//! The TUI never reads credentials: the acp-host subprocess owns key
//! resolution (FLUX_API_KEY → FLUX_TEST_KEY → FLUX_API_KEY_FILE); if the
//! host refuses to start, its stderr is shown verbatim.

use std::io::Write as _;

use crossterm::event::Event;
use crossterm::event::KeyEventKind;
use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use nano_tui::acp_client::SubprocessConnection;
use nano_tui::app::App;
use nano_tui::event::AppEvent;
use nano_tui::event::AppEventSender;

struct Args {
    resume_session: Option<String>,
    cwd: std::path::PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut resume = None;
    let mut cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--session" => {
                resume = Some(
                    it.next()
                        .ok_or_else(|| "--session requires a session id".to_string())?,
                );
            }
            "--cwd" => {
                cwd = std::path::PathBuf::from(
                    it.next()
                        .ok_or_else(|| "--cwd requires a path".to_string())?,
                );
            }
            "--help" | "-h" => {
                return Err(
                    "usage: nano-tui [--session <id>] [--cwd <dir>]\n  /model /status /doctor /quit"
                        .to_string(),
                );
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        resume_session: resume,
        cwd,
    })
}

fn nano_home() -> std::path::PathBuf {
    if let Some(home) = std::env::var_os("NANO_HOME") {
        return std::path::PathBuf::from(home);
    }
    let base = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match base {
        Some(base) => std::path::PathBuf::from(base).join(".nano"),
        None => std::path::PathBuf::from("."),
    }
}

/// Restore the terminal exactly on exit (raw mode, alternate screen,
/// bracketed paste, Windows VT input mode — the windows_console.rs port).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        #[cfg(windows)]
        nano_tui::windows_console::set_input_record_mode()?;
        let mut stdout = std::io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            crossterm::event::EnableBracketedPaste
        )?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let _ = execute!(
            stdout,
            crossterm::event::DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
        #[cfg(windows)]
        let _ = nano_tui::windows_console::restore_input_mode();
    }
}

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("nano-tui: {err}");
            2
        }
    };
    std::process::exit(code);
}

fn run() -> std::io::Result<i32> {
    let args =
        parse_args().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    // The host program: NANO_TUI_ACP_HOST override (test hook used by the
    // L3 PTY smoke), else the wayland-nano sibling binary / PATH.
    let (program, mut host_args) = match std::env::var_os("NANO_TUI_ACP_HOST") {
        Some(program) => (std::path::PathBuf::from(program), Vec::new()),
        None => (
            nano_tui::doctor::wayland_nano_program(),
            vec!["acp-host".to_string()],
        ),
    };
    if let Some(extra) = std::env::var_os("NANO_TUI_ACP_HOST_ARGS") {
        host_args.extend(
            extra
                .to_string_lossy()
                .split_whitespace()
                .map(str::to_string),
        );
    }

    let mut conn = match SubprocessConnection::spawn(&program, &host_args, &args.cwd) {
        Ok(conn) => conn,
        Err(err) => {
            return Err(std::io::Error::new(
                err.kind(),
                format!("failed to spawn {}: {err}", program.display()),
            ));
        }
    };

    let _guard = TerminalGuard::enter()?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let (bus_tx, mut bus_rx) = tokio::sync::mpsc::unbounded_channel();
        let sender = AppEventSender::new(bus_tx);
        let (redraw_tx, mut redraw_rx) = tokio::sync::mpsc::unbounded_channel();
        let _frame_requester = nano_tui::frame_requester::FrameRequester::new(redraw_tx);

        // The terminal event pump: a dedicated blocking thread (the
        // acp-host reader-thread pattern), posting onto the bus.
        let key_sender = sender.clone();
        std::thread::spawn(move || {
            loop {
                match crossterm::event::read() {
                    Ok(Event::Key(key)) => {
                        // Unix terminals emit Release/Repeat; Windows only Press.
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        if !key_sender.send(AppEvent::Key(key)) {
                            break;
                        }
                    }
                    Ok(Event::Paste(text)) => {
                        if !key_sender.send(AppEvent::Paste(text)) {
                            break;
                        }
                    }
                    Ok(Event::Resize(cols, rows)) => {
                        if !key_sender.send(AppEvent::Resize(cols, rows)) {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });

        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        let mut terminal = ratatui::Terminal::new(backend)?;
        let mut app = App::new(
            sender,
            args.cwd.display().to_string(),
            nano_home(),
            args.resume_session,
        );
        app.run(&mut conn, &mut terminal, &mut bus_rx, &mut redraw_rx)
            .await?;
        std::io::Result::Ok(())
    })?;
    let _ = std::io::stdout().flush();
    Ok(0)
}
