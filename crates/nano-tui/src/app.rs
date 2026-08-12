//! The app: a single `tokio::select!` loop over three channels (design doc
//! §4, codex `app.rs:1198-1246` pattern):
//!
//! - `bus_rx` — the internal AppEvent bus (keyboard/paste/resize, redraws,
//!   doctor results, quit)
//! - `conn.next_event()` — ACP frames from the acp-host subprocess
//! - `redraw_rx` — coalesced frame-scheduler requests
//!
//! Fail-closed rules honored here (design §5/§8):
//! - every approval is an explicit human decision keyed by the wire request
//!   id; Esc = deny; malformed/duplicate permission requests auto-deny;
//! - the engine's read-only fast-path is DESIGNED behavior (fs_read/search/
//!   glob never emit permission requests) — the TUI must not prompt for
//!   those, and nobody should later "fix" it as a gap (C5);
//! - all rendered text is sanitized (transcript/composer own that).

use std::collections::HashMap;
use std::collections::HashSet;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::Terminal;
use ratatui::backend::Backend;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::acp_client::{self, ConnEvent, Connection, Inbound, SessionUpdate, WireError};
use crate::composer::{Composer, ComposerAction};
use crate::doctor;
use crate::event::{AppEvent, AppEventSender};
use crate::frame_requester;
use crate::modal::{ApprovalRequest, ListItem, ListSelectionView, ModalOutcome};
use crate::notify::{Notifier, NotifyKind};
use crate::slash_commands::{self, SlashCommand};
use crate::status::{Status, WireState};
use crate::transcript::Transcript;

/// What an outstanding client request is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Initialize,
    SessionNew,
    SessionLoad,
    Prompt,
    SetModel,
    SetMode,
    Compact,
    /// C9: a mid-turn session/steer enqueue; the ack carries the position.
    Steer,
    Goal,
}

/// The open modal, if any.
enum Modal {
    Approval {
        request: ApprovalRequest,
        view: ListSelectionView,
    },
    ModelPicker(ListSelectionView),
    ModePicker(ListSelectionView),
}

pub struct App {
    pub transcript: Transcript,
    pub composer: Composer,
    pub status: Status,
    modal: Option<Modal>,
    pending: HashMap<u64, Pending>,
    ids: acp_client::RequestIds,
    /// The approval currently on screen (its wire request id).
    open_permission: Option<u64>,
    /// Every permission id ever shown or auto-denied — a replayed id is a
    /// spoof attempt and is denied without a prompt (design §8).
    seen_permission_ids: HashSet<u64>,
    session_id: Option<String>,
    resume_session: Option<String>,
    /// The mode id a session/set_mode request is carrying (C2): applied on
    /// a successful ack. C10: the ack also carries the modes block (and,
    /// for plan entry, the plan file path) — preferred when present.
    requested_mode: Option<String>,
    /// OSC 9 desktop notifications (C10 §7): two-entry allowlist, config
    /// off-switch, fire-and-forget.
    notifier: Notifier,
    /// The latest todo list seen on the wire (C10 §2): tracked from `todo`
    /// tool_call frames (no new wire affordance in v1) for /todo and the
    /// status-line count.
    todos: Vec<(String, String, String)>,
    cwd: String,
    nano_home: std::path::PathBuf,
    turn_active: bool,
    /// C9: the host advertised session/steer in its nanoExtensions block.
    /// Discovered, never probed; without it a mid-turn submit keeps the old
    /// "a turn is already running" behavior.
    steer_supported: bool,
    ready: bool,
    should_quit: bool,
    sender: AppEventSender,
}

impl App {
    pub fn new(
        sender: AppEventSender,
        cwd: String,
        nano_home: std::path::PathBuf,
        resume_session: Option<String>,
    ) -> Self {
        Self {
            transcript: Transcript::new(),
            composer: Composer::new(),
            status: Status::default(),
            modal: None,
            pending: HashMap::new(),
            ids: acp_client::RequestIds::default(),
            open_permission: None,
            seen_permission_ids: HashSet::new(),
            session_id: None,
            resume_session,
            requested_mode: None,
            notifier: Notifier::from_env(),
            todos: Vec::new(),
            cwd,
            nano_home,
            turn_active: false,
            steer_supported: false,
            ready: false,
            should_quit: false,
            sender,
        }
    }

    pub fn modal_open(&self) -> bool {
        self.modal.is_some()
    }

    /// The model-picker / mode-picker / approval modal's view, for the
    /// renderer.
    pub fn modal_view(&self) -> Option<(&str, &ListSelectionView, Option<&str>)> {
        match &self.modal {
            Some(Modal::Approval { request, view }) => {
                Some(("approval", view, Some(request.raw_input.as_str())))
            }
            Some(Modal::ModelPicker(view)) => Some(("model", view, None)),
            Some(Modal::ModePicker(view)) => Some(("mode", view, None)),
            None => None,
        }
    }

    fn send_request<C: Connection>(
        &mut self,
        conn: &mut C,
        method: &str,
        params: Value,
        pending: Pending,
    ) {
        let id = self.ids.alloc();
        let frame = acp_client::request(id, method, params);
        if let Err(err) = conn.send(&frame) {
            self.transcript
                .push_note(&format!("wire send failed: {err}"));
            self.status.wire = WireState::Disconnected;
            return;
        }
        self.pending.insert(id, pending);
    }

    fn send_frame<C: Connection>(&mut self, conn: &mut C, frame: &Value) {
        if let Err(err) = conn.send(frame) {
            self.transcript
                .push_note(&format!("wire send failed: {err}"));
            self.status.wire = WireState::Disconnected;
        }
    }

    /// The select! loop. Drives the handshake (initialize → session/new |
    /// session/load), then events until quit or host death.
    pub async fn run<C: Connection, B: Backend<Error = std::io::Error>>(
        &mut self,
        conn: &mut C,
        terminal: &mut Terminal<B>,
        bus_rx: &mut mpsc::UnboundedReceiver<AppEvent>,
        redraw_rx: &mut mpsc::UnboundedReceiver<()>,
    ) -> std::io::Result<()> {
        self.transcript
            .push_note("nano-tui — connecting to wayland-nano acp-host");
        self.send_request(
            conn,
            "initialize",
            acp_client::initialize_params(),
            Pending::Initialize,
        );
        self.draw(terminal)?;

        while !self.should_quit {
            tokio::select! {
                event = bus_rx.recv() => {
                    let Some(event) = event else { break };
                    self.handle_bus_event(conn, event);
                }
                conn_event = conn.next_event() => {
                    match conn_event {
                        Some(event) => self.handle_conn_event(conn, event),
                        None => break,
                    }
                }
                redraw = redraw_rx.recv() => {
                    if redraw.is_none() {
                        // Scheduler closed; keep serving the other channels.
                        continue;
                    }
                }
            }
            self.draw(terminal)?;
            // A burst of redraw requests collapses into the draw above.
            frame_requester::drain_pending(redraw_rx);
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn draw<B: Backend<Error = std::io::Error>>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> std::io::Result<()> {
        // Zero-size terminal (resize storm): defer, never panic (design §8).
        let size = terminal.size()?;
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        terminal.draw(|frame| crate::render::render(frame, self))?;
        Ok(())
    }

    // ── bus events ──────────────────────────────────────────────────────

    /// Test hook (L2): the startup note + initialize request, without the
    /// async loop. Production flow goes through [`App::run`].
    #[doc(hidden)]
    pub fn begin_for_tests<C: Connection>(&mut self, conn: &mut C) {
        self.transcript
            .push_note("nano-tui — connecting to wayland-nano acp-host");
        self.send_request(
            conn,
            "initialize",
            acp_client::initialize_params(),
            Pending::Initialize,
        );
    }

    #[doc(hidden)]
    pub fn handle_bus_event<C: Connection>(&mut self, conn: &mut C, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(conn, key),
            AppEvent::Paste(text) => {
                if self.modal.is_none() {
                    self.composer.insert_paste(&text);
                }
            }
            AppEvent::Resize(_, _) | AppEvent::Redraw => {}
            AppEvent::DoctorDone { output, exit_code } => {
                self.status.doctor_summary = doctor::summary_line(&output);
                if self.status.doctor_summary.is_none() && exit_code != 0 {
                    self.status.doctor_summary = Some(format!("doctor exited {exit_code}"));
                }
                self.transcript.push_note(&format!(
                    "wayland-nano doctor (exit {exit_code}):\n{output}"
                ));
            }
            AppEvent::Quit => self.begin_quit(conn),
        }
    }

    fn handle_key<C: Connection>(&mut self, conn: &mut C, key: KeyEvent) {
        // Modal owns the keyboard while open.
        if let Some(modal) = self.modal.take() {
            self.handle_modal_key(conn, modal, key);
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => self.begin_quit(conn),
            KeyCode::Esc => {
                if self.turn_active
                    && let Some(session_id) = self.session_id.clone()
                {
                    self.send_frame(conn, &acp_client::cancel_notification(&session_id));
                    self.transcript.push_note("cancel requested");
                }
            }
            KeyCode::PageUp => self.transcript.scroll_by(10),
            KeyCode::PageDown => self.transcript.scroll_by(-10),
            _ => {
                if let ComposerAction::Submit = self.composer.handle_key(key) {
                    self.submit(conn);
                }
            }
        }
    }

    fn handle_modal_key<C: Connection>(&mut self, conn: &mut C, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Approval { request, view } => {
                let mut view = view;
                match view.handle_key(key) {
                    ModalOutcome::Open => {
                        self.modal = Some(Modal::Approval { request, view });
                    }
                    ModalOutcome::Selected(option_id) => {
                        self.resolve_permission(conn, &request, &option_id);
                    }
                    // Esc = deny (explicit decision, keyed by request id).
                    ModalOutcome::Cancelled => {
                        let deny = deny_option_id(&request);
                        self.resolve_permission(conn, &request, &deny);
                    }
                }
            }
            Modal::ModelPicker(view) => {
                let mut view = view;
                match view.handle_key(key) {
                    ModalOutcome::Open => {
                        self.modal = Some(Modal::ModelPicker(view));
                    }
                    ModalOutcome::Selected(model_id) => {
                        if let Some(session_id) = self.session_id.clone() {
                            self.send_request(
                                conn,
                                "session/set_model",
                                acp_client::set_model_params(&session_id, &model_id),
                                Pending::SetModel,
                            );
                        }
                    }
                    ModalOutcome::Cancelled => {}
                }
            }
            Modal::ModePicker(view) => {
                let mut view = view;
                match view.handle_key(key) {
                    ModalOutcome::Open => {
                        self.modal = Some(Modal::ModePicker(view));
                    }
                    ModalOutcome::Selected(mode_id) => {
                        if let Some(session_id) = self.session_id.clone() {
                            self.requested_mode = Some(mode_id.clone());
                            self.send_request(
                                conn,
                                "session/set_mode",
                                acp_client::set_mode_params(&session_id, &mode_id),
                                Pending::SetMode,
                            );
                        }
                    }
                    ModalOutcome::Cancelled => {}
                }
            }
        }
    }

    /// Every prompt the TUI shows gets an explicit human decision; the
    /// response is keyed by exactly the wire request id (design §4/§5).
    fn resolve_permission<C: Connection>(
        &mut self,
        conn: &mut C,
        request: &ApprovalRequest,
        option_id: &str,
    ) {
        self.send_frame(
            conn,
            &acp_client::permission_response(request.request_id, option_id),
        );
        self.transcript.push_note(&format!(
            "approval decision: {option_id} — {}",
            request.title
        ));
        self.open_permission = None;
        if self.turn_active {
            self.status.wire = WireState::TurnRunning;
        } else {
            self.status.wire = WireState::Ready;
        }
    }

    fn submit<C: Connection>(&mut self, conn: &mut C) {
        let text = self.composer.take_submission();
        if text.trim().is_empty() {
            return;
        }
        match slash_commands::parse(&text) {
            None => self.submit_prompt(conn, &text),
            Some(SlashCommand::Model) => self.open_model_picker(),
            Some(SlashCommand::Mode) => self.open_mode_picker(),
            Some(SlashCommand::Plan) => self.submit_plan(conn),
            Some(SlashCommand::Todo) => self.show_todos(),
            Some(SlashCommand::Compact) => self.submit_compact(conn),
            Some(SlashCommand::Goal(action)) => self.submit_goal(conn, &action),
            Some(SlashCommand::Status) => {
                self.transcript.push_note(&self.status.report());
                // C1: /status doctor data comes from the short-lived
                // doctor subprocess, never an engine link.
                doctor::run_doctor_async(self.sender.clone(), &self.nano_home);
            }
            Some(SlashCommand::Doctor) => {
                self.transcript.push_note("running wayland-nano doctor…");
                doctor::run_doctor_async(self.sender.clone(), &self.nano_home);
            }
            Some(SlashCommand::Quit) => self.begin_quit(conn),
            Some(SlashCommand::Unknown(command)) => {
                self.transcript.push_note(&format!(
                    "unknown command: {command} (have: /model /mode /plan /todo /status /doctor /compact /quit)"
                ));
            }
        }
    }

    /// `/plan` (C10 §3): the TUI is an ACP CLIENT, so this sends
    /// session/set_mode {modeId:"plan"} over the wire — exactly as /model
    /// sends session/set_model. No parallel local channel. The host's ack
    /// carries the plan file path, printed for discoverability (Q5).
    fn submit_plan<C: Connection>(&mut self, conn: &mut C) {
        if !self.ready {
            self.transcript
                .push_note("not connected yet — wait for the session");
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        self.requested_mode = Some("plan".to_string());
        self.send_request(
            conn,
            "session/set_mode",
            acp_client::set_mode_params(&session_id, "plan"),
            Pending::SetMode,
        );
    }

    /// `/todo` (C10 §2): print the latest list tracked from the wire.
    fn show_todos(&mut self) {
        if self.todos.is_empty() {
            self.transcript
                .push_note("no todo list seen this session (the model sets it via the todo tool)");
            return;
        }
        let mut lines = vec![format!("todo list ({} item(s)):", self.todos.len())];
        for (id, content, status) in &self.todos {
            lines.push(format!("- [{status}] {id}: {content}"));
        }
        self.transcript.push_note(&lines.join("\n"));
    }

    /// `/compact` (C1 §7): engine-side manual compaction via session/compact.
    /// The host rejects it while a turn runs; the TUI blocks it early for a
    /// clearer note. Begin/complete surface as compaction session/update
    /// notices from the host.
    /// `/goal` (C11): thin mirror over the `_wayland/goal/*` extension
    /// methods — the lifecycle state machine lives engine-side.
    fn submit_goal<C: Connection>(&mut self, conn: &mut C, action: &str) {
        if !self.ready {
            self.transcript
                .push_note("not connected yet — wait for the session");
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let (method, params) = match action {
            "status" | "pause" | "resume" | "cancel" => (
                acp_client::goal_method(action),
                acp_client::goal_params(&session_id, None),
            ),
            other => {
                self.transcript.push_note(&format!(
                    "unknown /goal action {other:?} (status|pause|resume|cancel)"
                ));
                return;
            }
        };
        self.send_request(conn, &method, params, Pending::Goal);
    }

    fn submit_compact<C: Connection>(&mut self, conn: &mut C) {
        if !self.ready {
            self.transcript
                .push_note("not connected yet — wait for the session");
            return;
        }
        if self.turn_active {
            self.transcript
                .push_note("cannot compact while a turn is running");
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        self.transcript.push_note("compacting context…");
        self.send_request(
            conn,
            "session/compact",
            acp_client::compact_params(&session_id),
            Pending::Compact,
        );
    }

    fn submit_prompt<C: Connection>(&mut self, conn: &mut C, text: &str) {
        if !self.ready {
            self.transcript
                .push_note("not connected yet — wait for the session");
            return;
        }
        if self.turn_active {
            // C9 §3.4: a mid-turn submit rides session/steer when the host
            // advertised it; the ack arrives as a normal response. Without
            // the capability the old behavior stands.
            if !self.steer_supported {
                self.transcript
                    .push_note("a turn is already running (Esc cancels)");
                return;
            }
            let Some(session_id) = self.session_id.clone() else {
                self.transcript.push_note("no session");
                return;
            };
            // No echo at submit: the pending indicator + ack note narrate
            // the queue, and the DRAIN renders the user cell at the point
            // the steer enters history (never a doubled render).
            self.status.pending_steers += 1;
            self.send_request(
                conn,
                "session/steer",
                acp_client::steer_params(&session_id, text),
                Pending::Steer,
            );
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            self.transcript.push_note("no session");
            return;
        };
        self.transcript.push_user(text);
        self.send_request(
            conn,
            "session/prompt",
            acp_client::prompt_params(&session_id, text),
            Pending::Prompt,
        );
        self.turn_active = true;
        self.status.wire = WireState::TurnRunning;
    }

    fn open_model_picker(&mut self) {
        if self.status.models.is_empty() {
            self.transcript
                .push_note("no model catalog advertised by the session");
            return;
        }
        let items = self
            .status
            .models
            .iter()
            .map(|(id, name)| ListItem {
                id: id.clone(),
                name: if name.is_empty() {
                    id.clone()
                } else {
                    name.clone()
                },
                description: None,
                is_current: *id == self.status.model,
            })
            .collect();
        self.modal = Some(Modal::ModelPicker(ListSelectionView::new(
            "Switch model (session/set_model)",
            items,
        )));
    }

    /// `/mode` (C2): a picker over the advertised availableModes, sending
    /// session/set_mode. The advertised ids render as-is — a newer agent's
    /// unknown mode is the host's to accept or reject, never the TUI's to
    /// pre-filter.
    fn open_mode_picker(&mut self) {
        if self.status.modes.is_empty() {
            self.transcript
                .push_note("no permission modes advertised by the session");
            return;
        }
        let items = self
            .status
            .modes
            .iter()
            .map(|(id, name)| ListItem {
                id: id.clone(),
                name: if name.is_empty() {
                    id.clone()
                } else {
                    name.clone()
                },
                description: None,
                is_current: *id == self.status.mode,
            })
            .collect();
        self.modal = Some(Modal::ModePicker(ListSelectionView::new(
            "Switch permission mode (session/set_mode)",
            items,
        )));
    }

    fn begin_quit<C: Connection>(&mut self, conn: &mut C) {
        if self.turn_active
            && let Some(session_id) = self.session_id.clone()
        {
            self.send_frame(conn, &acp_client::cancel_notification(&session_id));
        }
        self.should_quit = true;
    }

    // ── ACP frames ──────────────────────────────────────────────────────

    #[doc(hidden)]
    pub fn handle_conn_event<C: Connection>(&mut self, conn: &mut C, event: ConnEvent) {
        match event {
            ConnEvent::Frame(frame) => match acp_client::classify(&frame) {
                Inbound::Response { id, result, error } => {
                    self.handle_response(conn, id, result, error);
                }
                Inbound::Update(update) => self.handle_update(update),
                Inbound::Permission(request) => self.handle_permission(conn, request),
                Inbound::MalformedPermission { id, reason } => {
                    // Fail-closed mirror of the engine's gate (design §4):
                    // malformed/absent response = deny.
                    self.transcript.push_error(
                        "Malformed permission request — auto-denied",
                        &reason,
                        "permission",
                        None,
                        false,
                    );
                    if let Some(id) = id {
                        self.seen_permission_ids.insert(id);
                        self.send_frame(conn, &acp_client::permission_response(id, "deny"));
                    }
                }
                Inbound::UnknownRequest { id, method } => {
                    self.send_frame(conn, &acp_client::method_not_found_response(&id, &method));
                }
                Inbound::UnknownNotification { .. } => {}
                Inbound::MalformedFrame(reason) => {
                    self.transcript.push_error(
                        "Malformed wire frame",
                        &reason,
                        "frame",
                        None,
                        false,
                    );
                }
            },
            ConnEvent::ParseError(err) => {
                self.transcript
                    .push_error("Malformed wire frame", &err, "frame", None, false);
            }
            ConnEvent::Closed(stderr_tail) => {
                self.status.wire = WireState::Disconnected;
                // C7: the host-exit note becomes a host_exited error cell;
                // the stderr tail stays truncated to its existing 4 KiB.
                self.transcript.push_error(
                    "acp-host exited",
                    stderr_tail.trim(),
                    "host_exited",
                    None,
                    false,
                );
            }
        }
    }

    fn handle_response<C: Connection>(
        &mut self,
        conn: &mut C,
        id: u64,
        result: Option<Value>,
        error: Option<WireError>,
    ) {
        let Some(pending) = self.pending.remove(&id) else {
            // A response to nothing we asked — spoof surface; note and drop.
            self.transcript
                .push_note("ignoring response with unknown id");
            return;
        };
        if let Some(error) = error {
            self.push_wire_error(&error);
            match pending {
                Pending::Initialize | Pending::SessionNew | Pending::SessionLoad => {
                    self.status.wire = WireState::Disconnected;
                }
                Pending::Prompt => {
                    // C7/D1 mid-stream semantics: the already-committed
                    // partial assistant cell STAYS; the error cell is
                    // appended after it (push order + commit-on-arrival).
                    self.transcript.commit_active();
                    self.turn_active = false;
                    self.status.wire = WireState::Ready;
                }
                Pending::Steer => {
                    // The steer never queued: the user text stays in the
                    // transcript (they wrote it) and the count settles.
                    self.status.pending_steers = self.status.pending_steers.saturating_sub(1);
                }
                Pending::SetModel => {}
                Pending::SetMode => {
                    // The mode visibly did not change.
                    self.requested_mode = None;
                }
                Pending::Compact => {}
                Pending::Goal => {}
            }
            return;
        }
        let result = result.unwrap_or(Value::Null);
        match pending {
            Pending::Initialize => {
                // C9: steer support is DISCOVERED from the advertised
                // nanoExtensions block, never probed.
                self.steer_supported = acp_client::parse_steer_capability(&result);
                let cwd = self.cwd.clone();
                if let Some(resume) = self.resume_session.clone() {
                    // session/load: journal-backed resume (design §2). Replay
                    // notifications arrive BEFORE the load response.
                    self.send_request(
                        conn,
                        "session/load",
                        acp_client::session_load_params(&resume, &cwd),
                        Pending::SessionLoad,
                    );
                } else {
                    self.send_request(
                        conn,
                        "session/new",
                        acp_client::session_new_params(&cwd),
                        Pending::SessionNew,
                    );
                }
            }
            Pending::SessionNew => {
                let session_id = result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if session_id.is_empty() {
                    self.transcript
                        .push_note("session/new returned no sessionId");
                    self.status.wire = WireState::Disconnected;
                    return;
                }
                self.session_id = Some(session_id.clone());
                self.status.session_id = Some(session_id.clone());
                if let Some((current, models)) = acp_client::parse_models(&result) {
                    self.status.model = current;
                    self.status.models = models;
                }
                if let Some((current, modes)) = acp_client::parse_modes(&result) {
                    self.status.mode = current;
                    self.status.modes = modes;
                }
                self.ready = true;
                self.status.wire = WireState::Ready;
                self.transcript
                    .push_note(&format!("session ready: {session_id}"));
            }
            Pending::SessionLoad => {
                if let Some((current, models)) = acp_client::parse_models(&result) {
                    self.status.model = current;
                    self.status.models = models;
                }
                // C2 (panel Q5): a resumed session always comes back in
                // `default` — the load response's modes block says so, and
                // the status line must show it (never a resurrected mode).
                if let Some((current, modes)) = acp_client::parse_modes(&result) {
                    self.status.mode = current;
                    self.status.modes = modes;
                }
                self.ready = true;
                self.status.wire = WireState::Ready;
                let resumed = self.resume_session.clone().unwrap_or_default();
                self.session_id = Some(resumed.clone());
                self.status.session_id = Some(resumed.clone());
                self.transcript
                    .push_note(&format!("session resumed: {resumed}"));
            }
            Pending::Prompt => {
                self.transcript.commit_active();
                self.turn_active = false;
                self.status.wire = WireState::Ready;
                // The turn ended: any reconnect banner clears, and every
                // still-pending steer was either drained (rendered as a
                // user chunk) or dropped (a steer_dropped notice).
                self.status.reconnect = None;
                self.status.pending_steers = 0;
                let stop = result
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                // C7: stopReason is only ever end_turn or a genuine cancel
                // now — turn-fatal failures arrive as typed error
                // responses. Anything else (an older/third-party host)
                // renders as a generic terminal cell, never a raw string.
                if stop == "cancelled" {
                    self.transcript.push_note("turn ended: cancelled");
                } else if !stop.is_empty() && stop != "end_turn" {
                    self.transcript
                        .push_error("Turn failed", "", stop, None, false);
                }
                // C10 §7: turn complete (agent idle, the user-away case) —
                // one of the two allowlisted notifications.
                self.notifier
                    .notify_terminal(NotifyKind::TurnComplete, "the agent is idle");
            }
            Pending::Steer => {
                if result.get("queued").and_then(Value::as_bool) == Some(true) {
                    // Queued: stays counted until it drains (user chunk) or
                    // drops (steer_dropped notice).
                    let position = result.get("position").and_then(Value::as_u64).unwrap_or(0);
                    self.transcript
                        .push_note(&format!("steer queued (position {position})"));
                } else {
                    self.status.pending_steers = self.status.pending_steers.saturating_sub(1);
                    self.transcript.push_note("steer rejected by the host");
                }
            }
            Pending::SetModel => {
                // The engine echoes the models state; it is the source of
                // truth for what the session now runs.
                if let Some((current, models)) = acp_client::parse_models(&result) {
                    self.status.model = current.clone();
                    self.status.models = models;
                    self.transcript
                        .push_note(&format!("model switched to {current}"));
                } else {
                    self.transcript
                        .push_note("set_model response carried no models block");
                }
            }
            Pending::SetMode => {
                // C10: the ack carries the modes block (currentModeId is
                // the source of truth — "plan" while the posture is
                // active) and, on plan entry, the plan file path (Q5
                // discoverability). Fall back to the requested id when an
                // older host acks with an empty object.
                if let Some((current, modes)) = acp_client::parse_modes(&result) {
                    self.status.mode = current.clone();
                    self.status.modes = modes;
                    self.transcript
                        .push_note(&format!("mode switched to {current}"));
                } else {
                    match self.requested_mode.take() {
                        Some(mode) => {
                            self.status.mode = mode.clone();
                            self.transcript
                                .push_note(&format!("mode switched to {mode}"));
                        }
                        None => {
                            self.transcript
                                .push_note("set_mode acked with no requested mode");
                        }
                    }
                }
                self.requested_mode = None;
                if let Some(plan_file) = result.get("planFile").and_then(Value::as_str) {
                    self.transcript
                        .push_note(&format!("plan file: {plan_file}"));
                }
            }
            Pending::Compact => {
                // The begin/complete notices already narrate the transcript;
                // the response only confirms the wire call succeeded.
                if result.get("compacted").and_then(Value::as_bool) != Some(true) {
                    self.transcript
                        .push_note("session/compact returned an unexpected result");
                }
            }
            Pending::Goal => {
                // Render the engine's answer compactly (status JSON or the
                // transition ack).
                self.transcript.push_note(&format!(
                    "goal: {}",
                    serde_json::to_string(&result).unwrap_or_default()
                ));
            }
        }
    }

    /// C7: a wire error becomes a typed error cell. Known kinds render the
    /// table's title/hint with the wire's retryable flag; unknown or
    /// kindless errors render the generic TERMINAL cell with a static
    /// title — NEVER the raw wire message (design §4, §7).
    fn push_wire_error(&mut self, error: &WireError) {
        let code_label = error.code.to_string();
        match error.nano {
            Some(payload) if payload.kind != nano_session::NanoErrorKind::Unknown => {
                let spec = nano_session::error_codes::spec(payload.kind);
                // The wire flag is honored only where the table agrees —
                // a known kind's retryability can never widen past the
                // table, and unknown kinds took the generic arm above.
                self.transcript.push_error(
                    spec.title,
                    spec.hint,
                    &code_label,
                    Some(payload.kind),
                    payload.retryable && spec.retryable,
                );
            }
            _ => {
                self.transcript
                    .push_error("Request failed", "", &code_label, None, false);
            }
        }
    }

    fn handle_update(&mut self, update: SessionUpdate) {
        match update {
            SessionUpdate::AgentChunk(text) => self.transcript.push_agent_chunk(&text),
            SessionUpdate::UserChunk(text) => {
                // A live user chunk is a drained steer (C9): the host emits
                // user chunks live only from the steer/re-ask drain path.
                self.status.pending_steers = self.status.pending_steers.saturating_sub(1);
                self.transcript.push_user(&text);
            }
            SessionUpdate::SteerDropped {
                request_id,
                text_digest,
            } => {
                // Exactly one typed note per dropped steer (C9 §3.3).
                self.status.pending_steers = self.status.pending_steers.saturating_sub(1);
                self.transcript.push_note(&format!(
                    "steer dropped (request {request_id}, {text_digest})"
                ));
            }
            SessionUpdate::Reconnecting {
                attempt,
                next_delay_ms,
                deadline_remaining_ms,
            } => {
                self.status.reconnect = Some((attempt, next_delay_ms, deadline_remaining_ms));
            }
            SessionUpdate::ParamInert {
                param,
                surface,
                detail,
            } => {
                self.transcript
                    .push_note(&format!("param inert: {param} on {surface} — {detail}"));
            }
            SessionUpdate::RateLimit {
                requests_remaining,
                requests_limit,
                tokens_remaining,
                tokens_limit,
            } => {
                self.status.rate_limit = Some(crate::status::RateLimitView {
                    requests_remaining,
                    requests_limit,
                    tokens_remaining,
                    tokens_limit,
                });
            }
            SessionUpdate::ToolCall {
                call_id,
                title,
                status,
                raw_input,
            } => {
                // C10 §2: track the todo list from `todo` write frames for
                // /todo and the status-line count (no new wire affordance).
                if title == "todo"
                    && let Some(items) = raw_input.get("todos").and_then(Value::as_array)
                {
                    self.todos = items
                        .iter()
                        .filter_map(|item| {
                            Some((
                                item.get("id")?.as_str()?.to_string(),
                                item.get("content")?.as_str()?.to_string(),
                                item.get("status")?.as_str()?.to_string(),
                            ))
                        })
                        .collect();
                    self.status.todo_open = Some(
                        self.todos
                            .iter()
                            .filter(|(_, _, status)| status == "pending" || status == "in_progress")
                            .count(),
                    );
                }
                let detail = one_line(&raw_input);
                self.transcript
                    .push_tool_call(&call_id, &title, &status, &detail);
            }
            SessionUpdate::ToolCallUpdate {
                call_id,
                status,
                raw_output,
                diff,
                nano_error,
            } => {
                self.transcript
                    .push_tool_result(&call_id, &status, &raw_output, nano_error);
                // C10 §6: the human-facing diff block (the TUI is the v1
                // renderer).
                if let Some(diff) = diff {
                    self.transcript.push_tool_diff(
                        &diff.path,
                        diff.old_text.as_deref(),
                        &diff.new_text,
                    );
                }
            }
            SessionUpdate::Unknown(_) => {}
            SessionUpdate::Compaction { status } => {
                // Model-generated summaries never reach this string — it is a
                // bounded status word — and push_note sanitizes regardless.
                self.transcript
                    .push_note(&format!("context compaction: {status}"));
            }
        }
    }

    fn handle_permission<C: Connection>(
        &mut self,
        conn: &mut C,
        request: acp_client::PermissionRequest,
    ) {
        if self.open_permission.is_some() || self.seen_permission_ids.contains(&request.id) {
            // Approval spoofing (design §8): duplicate/replayed request ids
            // are denied without a prompt — every approval requires one
            // explicit human decision, never two decisions for one id and
            // never a decision for an id already answered.
            self.transcript.push_note(&format!(
                "duplicate permission request #{} auto-denied",
                request.id
            ));
            self.send_frame(conn, &acp_client::permission_response(request.id, "deny"));
            return;
        }
        self.seen_permission_ids.insert(request.id);
        self.open_permission = Some(request.id);
        self.status.wire = WireState::AwaitingApproval;
        // C10 §7: a permission/question prompt is waiting on the user.
        self.notifier
            .notify_terminal(NotifyKind::PermissionPending, &request.title);
        let detail = one_line(&request.raw_input);
        let approval = ApprovalRequest {
            request_id: request.id,
            title: request.title,
            raw_input: detail,
            options: request
                .options
                .iter()
                .map(|o| ListItem {
                    id: o.option_id.clone(),
                    name: o.name.clone(),
                    description: Some(o.kind.clone()).filter(|k| !k.is_empty()),
                    is_current: false,
                })
                .collect(),
        };
        let view = approval.view();
        self.modal = Some(Modal::Approval {
            request: approval,
            view,
        });
    }
}

fn deny_option_id(request: &ApprovalRequest) -> String {
    request
        .options
        .iter()
        .find(|o| o.id.starts_with("deny") || o.id.starts_with("reject"))
        .map(|o| o.id.clone())
        .unwrap_or_else(|| "deny".to_string())
}

/// Compact one-line rendering of a JSON value for tool cards/approval
/// details (sanitized downstream).
fn one_line(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unprintable>".to_string())
}
