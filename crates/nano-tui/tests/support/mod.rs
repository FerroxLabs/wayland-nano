//! L2 support: VT100Backend (donor port — codex `tui/src/test_backend.rs`,
//! see UPSTREAM.md), a scripted FakeConnection over the recorded fixtures,
//! and a synchronous step world that drives the real app logic against the
//! virtual terminal.
//!
//! Shared by several integration tests; each compiles its own copy, so
//! helpers unused by one consumer are expected.
#![allow(dead_code)]

use std::fmt;
use std::io;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;

use nano_tui::acp_client::ConnEvent;
use nano_tui::acp_client::Connection;
use nano_tui::app::App;
use nano_tui::event::AppEvent;
use nano_tui::event::AppEventSender;
use nano_tui::fake_host::FakeHost;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::backend::WindowSize;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Size;
use ratatui::prelude::CrosstermBackend;

// ── VT100Backend (donor port) ───────────────────────────────────────────

/// CrosstermBackend over a vt100::Parser: a "real" terminal for tests.
///
/// Deviation from the donor (recorded in UPSTREAM.md): codex reaches the
/// parser through CrosstermBackend's `writer()`/`writer_mut()` accessors,
/// which ratatui 0.30 gates behind the UNSTABLE `backend-writer` feature —
/// banned by the design (§3: no unstable features). Here the parser lives
/// behind a shared handle the test owns directly; the backend merely
/// writes through it.
#[derive(Clone)]
struct ParserHandle {
    parser: Arc<Mutex<vt100::Parser>>,
}

impl Write for ParserHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.parser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.parser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .flush()
    }
}

pub struct VT100Backend {
    crossterm_backend: CrosstermBackend<ParserHandle>,
    parser: Arc<Mutex<vt100::Parser>>,
}

impl VT100Backend {
    pub fn new(width: u16, height: u16) -> Self {
        crossterm::style::force_color_output(true);
        let parser = Arc::new(Mutex::new(vt100::Parser::new(height, width, 0)));
        Self {
            crossterm_backend: CrosstermBackend::new(ParserHandle {
                parser: Arc::clone(&parser),
            }),
            parser,
        }
    }

    fn parser(&self) -> std::sync::MutexGuard<'_, vt100::Parser> {
        self.parser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl fmt::Display for VT100Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.parser().screen().contents())
    }
}

impl Backend for VT100Backend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.crossterm_backend.draw(content)?;
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.crossterm_backend.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.crossterm_backend.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.parser().screen().cursor_position().into())
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.crossterm_backend.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.crossterm_backend.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.crossterm_backend.clear_region(clear_type)
    }

    fn append_lines(&mut self, line_count: u16) -> io::Result<()> {
        self.crossterm_backend.append_lines(line_count)
    }

    fn size(&self) -> io::Result<Size> {
        let (rows, cols) = self.parser().screen().size();
        Ok(Size::new(cols, rows))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.parser().screen().size().into(),
            pixels: Size {
                width: 640,
                height: 480,
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.crossterm_backend)
    }
}

// ── scripted connection ─────────────────────────────────────────────────

/// A Connection whose host side is a FakeHost playing a recorded fixture.
/// Host frames materialize synchronously inside `send` (the fake host is
/// purely reactive), so the step world stays deterministic.
pub struct FakeConnection {
    pub host: FakeHost,
    queue: std::collections::VecDeque<ConnEvent>,
    /// Script violations, for loud teardown assertions.
    pub errors: Arc<Mutex<Vec<String>>>,
}

impl FakeConnection {
    pub fn from_script(script: &str) -> Self {
        Self {
            host: FakeHost::from_script(script).expect("fixture script parses"),
            queue: std::collections::VecDeque::new(),
            errors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Drain every host frame queued since the last drain.
    pub fn drain_events(&mut self) -> Vec<ConnEvent> {
        self.queue.drain(..).collect()
    }

    /// Pop exactly one queued host frame.
    pub fn pop_one(&mut self) -> Option<ConnEvent> {
        self.queue.pop_front()
    }
}

impl Connection for FakeConnection {
    fn send(&mut self, frame: &serde_json::Value) -> Result<(), String> {
        match self.host.feed(frame) {
            Ok(replies) => {
                for reply in replies {
                    self.queue.push_back(ConnEvent::Frame(reply));
                }
                Ok(())
            }
            Err(err) => {
                self.errors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(err.clone());
                // Surface to the app as a closed wire — a broken script is
                // a broken host.
                self.queue.push_back(ConnEvent::ParseError(err.clone()));
                Err(err)
            }
        }
    }

    async fn next_event(&mut self) -> Option<ConnEvent> {
        self.queue.pop_front()
    }
}

// ── the step world ──────────────────────────────────────────────────────

/// Synchronous L2 world: the real App, real render, real (recorded) wire
/// frames, virtual terminal. Steps are explicit, so there are no sleeps and
/// no races.
pub struct World {
    pub app: App,
    pub conn: FakeConnection,
    pub terminal: Terminal<VT100Backend>,
    _bus: (
        tokio::sync::mpsc::UnboundedSender<AppEvent>,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ),
}

impl World {
    pub fn new(script: &str, width: u16, height: u16, resume_session: Option<String>) -> Self {
        let (bus_tx, bus_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            AppEventSender::new(bus_tx.clone()),
            "/recorded-workspace".to_string(),
            std::path::PathBuf::from("/recorded-nano-home"),
            resume_session,
        );
        let mut conn = FakeConnection::from_script(script);
        app.begin_for_tests(&mut conn);
        let terminal = Terminal::new(VT100Backend::new(width, height)).expect("terminal");
        let mut world = Self {
            app,
            conn,
            terminal,
            _bus: (bus_tx, bus_rx),
        };
        world.pump();
        world
    }

    /// Drain host frames into the app until the wire goes quiet, drawing
    /// after each frame (the real loop draws after every select! pass).
    pub fn pump(&mut self) {
        for _ in 0..64 {
            let events = self.conn.drain_events();
            if events.is_empty() {
                break;
            }
            for event in events {
                self.app.handle_conn_event(&mut self.conn, event);
                self.app.draw(&mut self.terminal).expect("draw");
            }
        }
        self.app.draw(&mut self.terminal).expect("draw");
    }

    /// Post a bus event WITHOUT pumping — for step-by-step assertions.
    pub fn event_no_pump(&mut self, event: AppEvent) {
        self.app.handle_bus_event(&mut self.conn, event);
    }

    /// Handle exactly one queued host frame and draw (incremental-rendering
    /// assertions). Returns false when nothing was queued.
    pub fn pump_one(&mut self) -> bool {
        let Some(event) = self.conn.pop_one() else {
            return false;
        };
        self.app.handle_conn_event(&mut self.conn, event);
        self.app.draw(&mut self.terminal).expect("draw");
        true
    }

    /// Post a bus event (key, paste, resize, ...) and pump to quiescence.
    pub fn event(&mut self, event: AppEvent) {
        self.app.handle_bus_event(&mut self.conn, event);
        self.pump();
    }

    pub fn key(&mut self, code: crossterm::event::KeyCode) {
        self.event(AppEvent::Key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        )));
    }

    /// Type text and submit with Enter (the compose → send flow).
    pub fn type_and_submit(&mut self, text: &str) {
        for c in text.chars() {
            self.key(crossterm::event::KeyCode::Char(c));
        }
        self.key(crossterm::event::KeyCode::Enter);
    }

    /// The virtual screen's text (trailing whitespace trimmed per line).
    pub fn screen(&self) -> String {
        self.terminal
            .backend()
            .to_string()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    }

    /// Loud teardown: the script must be exhausted and violation-free.
    pub fn finish(self) {
        self.check_violations();
        assert!(
            self.conn.host.is_exhausted(),
            "unplayed fixture expectations: {:?}",
            self.conn.host.remaining()
        );
    }

    /// Partial teardown (multi-world journeys): violations still fail, and
    /// the caller asserts on the remaining expectations explicitly.
    pub fn finish_partial(self) -> Vec<String> {
        self.check_violations();
        self.conn.host.remaining()
    }

    fn check_violations(&self) {
        let errors = self
            .conn
            .errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(errors.is_empty(), "script violations: {errors:?}");
    }
}

// ── hand-authored fixture lint (C2(a)) ──────────────────────────────────

/// Structural lint over hand-authored adversarial fixtures: every line must
/// be a well-formed fixture entry whose frame is valid JSON-RPC, and the
/// sessionUpdate kinds it uses must have appeared in a RECORDED fixture
/// (the lint that keeps authored frames from drifting into fantasy) — or be
/// explicitly declared in `allowed_extra_kinds` (the tolerance-probe kinds
/// that exist precisely to prove the TUI tolerates the unknown).
pub fn lint_fixture(
    text: &str,
    recorded_texts: &[&str],
    allowed_extra_kinds: &[&str],
) -> Result<(), String> {
    let mut recorded_kinds = std::collections::HashSet::new();
    for recorded in recorded_texts {
        for line in recorded.lines().filter(|l| !l.trim().is_empty()) {
            let entry: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| format!("recorded fixture itself is invalid: {e}"))?;
            if let Some(kind) = entry
                .get("frame")
                .and_then(|f| f.get("params"))
                .and_then(|p| p.get("update"))
                .and_then(|u| u.get("sessionUpdate"))
                .and_then(serde_json::Value::as_str)
            {
                recorded_kinds.insert(kind.to_string());
            }
        }
    }
    for (lineno, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("line {}: invalid json: {e}", lineno + 1))?;
        let frame = entry
            .get("frame")
            .ok_or_else(|| format!("line {}: missing frame", lineno + 1))?;
        let has_method = frame.get("method").is_some();
        let has_id = frame.get("id").is_some_and(|i| !i.is_null());
        if !has_method && !has_id {
            return Err(format!("line {}: frame is not JSON-RPC shaped", lineno + 1));
        }
        if let Some(kind) = frame
            .get("params")
            .and_then(|p| p.get("update"))
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(serde_json::Value::as_str)
            && !recorded_kinds.contains(kind)
            && !allowed_extra_kinds.contains(&kind)
        {
            return Err(format!(
                "line {}: sessionUpdate kind {kind:?} never appears in the recorded fixtures",
                lineno + 1
            ));
        }
    }
    Ok(())
}
