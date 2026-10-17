use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, SharedString};
use parking_lot::Mutex;
use somme_application::ports::{OutputSink, TermSize};
use somme_domain::Id;
use tokio::sync::mpsc as io_mpsc;

use crate::runtime;
use crate::services::Services;
use crate::settings::Settings;
use crate::terminal::colors;
use crate::terminal::log::{self, SessionLog};

#[derive(Clone, Copy, Debug)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for GridSize {
    fn default() -> Self {
        Self {
            columns: 80,
            lines: 24,
            cell_width: 8.,
            cell_height: 17.,
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Collects events the emulator raises while parsing, handled right after.
#[derive(Clone, Default)]
pub struct Listener(Arc<Mutex<Vec<TermEvent>>>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        self.0.lock().push(event);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionTarget {
    Local,
    Ssh { host_id: Id, label: String },
    /// A recorded session played back from its log file. Nothing is sent anywhere.
    Replay { label: String, file: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    Closed(Option<String>),
    Failed(String),
}

pub enum SessionEvent {
    TitleChanged,
    Bell,
    StatusChanged,
}

enum Output {
    Data(Vec<u8>),
    Closed(Option<String>),
}

enum Io {
    Write(Vec<u8>),
    Resize(TermSize),
}

struct UiSink(mpsc::UnboundedSender<Output>);

impl OutputSink for UiSink {
    fn data(&self, bytes: &[u8]) {
        let _ = self.0.unbounded_send(Output::Data(bytes.to_vec()));
    }

    fn closed(&self, reason: Option<String>) {
        let _ = self.0.unbounded_send(Output::Closed(reason));
    }
}

/// One terminal: the emulator state plus the live connection behind it.
pub struct TerminalSession {
    pub target: SessionTarget,
    pub status: Status,
    pub started_at: Instant,
    title: Option<String>,
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
    size: GridSize,
    io: Option<io_mpsc::UnboundedSender<Io>>,
    session_id: Option<Id>,
    services: Arc<Services>,
    log: Option<SessionLog>,
    /// Bytes typed since the last newline, used to record shell history.
    line_buffer: String,
    pub history: Vec<String>,
}

impl EventEmitter<SessionEvent> for TerminalSession {}

impl TerminalSession {
    pub fn new(target: SessionTarget, services: Arc<Services>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let settings = Settings::get(cx);
            let replay = matches!(target, SessionTarget::Replay { .. });
            let config = TermConfig {
                // Keep the whole recording, up to the emulator's limit.
                scrolling_history: if replay { 100_000 } else { settings.scrollback_lines },
                ..TermConfig::default()
            };
            let log = (settings.log_sessions && !replay)
                .then(|| SessionLog::create(&services.data_dir, &target))
                .flatten();
            let listener = Listener::default();
            let size = GridSize::default();
            let mut session = Self {
                target,
                status: Status::Connecting,
                started_at: Instant::now(),
                title: None,
                term: Term::new(config, &size, listener.clone()),
                parser: Processor::new(),
                listener,
                size,
                io: None,
                session_id: None,
                services,
                log,
                line_buffer: String::new(),
                history: Vec::new(),
            };
            session.connect(cx);
            session
        })
    }

    pub fn title(&self) -> SharedString {
        if let SessionTarget::Replay { label, .. } = &self.target {
            return format!("Log: {label}").into();
        }
        if let Some(title) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return title.clone().into();
        }
        self.host_label()
    }

    pub fn host_label(&self) -> SharedString {
        match &self.target {
            SessionTarget::Local => "Local Terminal".into(),
            SessionTarget::Ssh { label, .. } | SessionTarget::Replay { label, .. } => label.clone().into(),
        }
    }

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        &mut self.term
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, Status::Connecting | Status::Connected)
    }

    pub fn is_replay(&self) -> bool {
        matches!(self.target, SessionTarget::Replay { .. })
    }

    /// Opens (or reopens) the connection. Screen contents are kept.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        if let SessionTarget::Replay { file, .. } = &self.target {
            self.load_replay(file.clone(), cx);
            return;
        }
        let (output_tx, output_rx) = mpsc::unbounded();
        let (io_tx, io_rx) = io_mpsc::unbounded_channel();
        self.io = Some(io_tx);
        self.session_id = None;
        self.status = Status::Connecting;
        self.started_at = Instant::now();
        cx.emit(SessionEvent::StatusChanged);

        cx.spawn(async move |this, cx| {
            let mut batches = output_rx.ready_chunks(256);
            while let Some(batch) = batches.next().await {
                let alive = this.update(cx, |session, cx| {
                    for output in batch {
                        session.handle_output(output, cx);
                    }
                    // So the Logs page can open a session that is still running.
                    if let Some(log) = session.log.as_mut() {
                        log.flush();
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let services = self.services.clone();
        let target = self.target.clone();
        let size = TermSize {
            cols: self.size.columns as u16,
            rows: self.size.lines as u16,
        };
        cx.spawn(async move |this, cx| {
            let sink: Arc<dyn OutputSink> = Arc::new(UiSink(output_tx));
            let sessions = services.sessions.clone();
            let opened = runtime::io(async move {
                match target {
                    SessionTarget::Local => sessions.open_local(size, move |_| sink),
                    SessionTarget::Ssh { host_id, .. } => sessions.open_ssh(host_id, size, move |_| sink).await,
                    SessionTarget::Replay { .. } => unreachable!("replays never connect"),
                }
            })
            .await;
            let _ = this.update(cx, |session, cx| {
                match opened {
                    Ok(id) => {
                        session.session_id = Some(id);
                        if session.status == Status::Connecting {
                            session.status = Status::Connected;
                        }
                        runtime::handle().spawn(write_loop(services, id, io_rx));
                    }
                    Err(err) => {
                        session.io = None;
                        session.status = Status::Failed(err.to_string());
                        if let Some(log) = session.log.as_mut() {
                            log.note(&format!("Connection failed: {err}"));
                        }
                    }
                }
                cx.emit(SessionEvent::StatusChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn load_replay(&mut self, file: String, cx: &mut Context<Self>) {
        self.status = Status::Closed(None);
        let data_dir = self.services.data_dir.clone();
        cx.spawn(async move |this, cx| {
            let bytes = runtime::io(async move { log::read(&data_dir, &file) }).await;
            let _ = this.update(cx, |session, cx| {
                match bytes {
                    Ok(bytes) => session.replay(&bytes),
                    Err(err) => session.status = Status::Failed(format!("Could not read the log: {err}")),
                }
                cx.emit(SessionEvent::StatusChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Plays a log back at the sizes it was recorded at, then fits it to this view.
    fn replay(&mut self, bytes: &[u8]) {
        let view_size = self.size;
        self.term.resize(GridSize::default());
        for chunk in log::chunks(bytes) {
            match chunk {
                log::Chunk::Output(bytes) => self.parser.advance(&mut self.term, bytes),
                log::Chunk::Size { columns, lines } => self.term.resize(GridSize {
                    columns,
                    lines,
                    ..view_size
                }),
            }
        }
        self.term.resize(view_size);
        // Replies the recording asks for have nowhere to go.
        self.listener.0.lock().clear();
    }

    fn handle_output(&mut self, output: Output, cx: &mut Context<Self>) {
        match output {
            Output::Data(bytes) => {
                if self.status == Status::Connecting {
                    self.status = Status::Connected;
                    cx.emit(SessionEvent::StatusChanged);
                }
                if let Some(log) = self.log.as_mut() {
                    log.write(&bytes);
                }
                self.parser.advance(&mut self.term, &bytes);
                self.drain_term_events(cx);
            }
            Output::Closed(reason) => {
                self.io = None;
                self.session_id = None;
                if let Some(log) = self.log.as_mut() {
                    log.note(reason.as_deref().unwrap_or("Session closed"));
                }
                self.status = Status::Closed(reason);
                cx.emit(SessionEvent::StatusChanged);
            }
        }
    }

    fn drain_term_events(&mut self, cx: &mut Context<Self>) {
        let events = std::mem::take(&mut *self.listener.0.lock());
        for event in events {
            match event {
                TermEvent::PtyWrite(text) => self.send(text.into_bytes()),
                TermEvent::Title(title) => {
                    self.title = Some(title);
                    cx.emit(SessionEvent::TitleChanged);
                }
                TermEvent::ResetTitle => {
                    self.title = None;
                    cx.emit(SessionEvent::TitleChanged);
                }
                TermEvent::Bell => cx.emit(SessionEvent::Bell),
                TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                // A server may copy into the clipboard but never read it: it could hold a password.
                TermEvent::ClipboardLoad(..) => {}
                TermEvent::ColorRequest(index, format) => {
                    let theme = colors::theme_named(&Settings::get(cx).terminal_theme);
                    let color = self.term.colors()[index].unwrap_or_else(|| match index {
                        256 => theme.foreground,
                        257 => theme.background,
                        258 => theme.cursor,
                        i if i < 16 => theme.ansi[i],
                        i => colors::resolve(
                            alacritty_terminal::vte::ansi::Color::Indexed(i as u8),
                            self.term.colors(),
                            theme,
                        ),
                    });
                    self.send(format(color).into_bytes());
                }
                TermEvent::TextAreaSizeRequest(format) => {
                    let size = WindowSize {
                        num_lines: self.size.lines as u16,
                        num_cols: self.size.columns as u16,
                        cell_width: self.size.cell_width as u16,
                        cell_height: self.size.cell_height as u16,
                    };
                    self.send(format(size).into_bytes());
                }
                _ => {}
            }
        }
    }

    /// Sends raw bytes to the remote side.
    pub fn send(&mut self, bytes: Vec<u8>) {
        if let Some(io) = &self.io {
            let _ = io.send(Io::Write(bytes));
        }
    }

    /// Sends what the user typed: scrolls to the bottom and records shell history.
    pub fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !self.is_running() {
            return;
        }
        if Settings::get(cx).keep_history {
            self.track_history(&bytes);
        }
        if self.term.grid().display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
        }
        self.term.selection = None;
        self.send(bytes);
        cx.notify();
    }

    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            let clean = text.replace('\x1b', "");
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            text.into_bytes()
        };
        self.input(bytes, cx);
    }

    fn track_history(&mut self, bytes: &[u8]) {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        for &byte in bytes {
            match byte {
                b'\r' | b'\n' => {
                    let line = std::mem::take(&mut self.line_buffer);
                    let line = line.trim();
                    if !line.is_empty() && self.history.last().map(String::as_str) != Some(line) {
                        self.history.push(line.to_owned());
                    }
                }
                0x7f | 0x08 => {
                    self.line_buffer.pop();
                }
                0x03 | 0x15 => self.line_buffer.clear(),
                0x1b => self.line_buffer.clear(),
                byte if byte >= 0x20 => {
                    if let Ok(text) = std::str::from_utf8(&[byte]) {
                        self.line_buffer.push_str(text);
                    } else {
                        self.line_buffer.push(char::from(byte));
                    }
                }
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, size: GridSize) {
        let changed = size.columns != self.size.columns || size.lines != self.size.lines;
        self.size = size;
        if !changed || size.columns == 0 || size.lines == 0 {
            return;
        }
        self.term.resize(size);
        if let Some(log) = self.log.as_mut() {
            log.resize(size.columns, size.lines);
        }
        if let Some(io) = &self.io {
            let _ = io.send(Io::Resize(TermSize {
                cols: size.columns as u16,
                rows: size.lines as u16,
            }));
        }
    }

    pub fn scroll(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.term.scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    pub fn scroll_to(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.term.scroll_display(scroll);
        cx.notify();
    }

    pub fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        self.term.grid_mut().clear_history();
        // Ask the shell to redraw its prompt on a clean screen.
        self.send(b"\x0c".to_vec());
        cx.notify();
    }

    pub fn start_selection(&mut self, kind: SelectionType, point: GridPoint, side: alacritty_terminal::index::Side) {
        self.term.selection = Some(Selection::new(kind, point, side));
    }

    pub fn update_selection(&mut self, point: GridPoint, side: alacritty_terminal::index::Side) {
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let start = GridPoint::new(self.term.topmost_line(), Column(0));
        let end = GridPoint::new(self.term.bottommost_line(), self.term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, alacritty_terminal::index::Side::Left);
        selection.update(end, alacritty_terminal::index::Side::Right);
        self.term.selection = Some(selection);
        cx.notify();
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    /// The visible text, used for search and tests.
    pub fn visible_text(&self) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        for line in 0..self.size.lines as i32 {
            let row = &grid[Line(line - grid.display_offset() as i32)];
            let text: String = (0..self.size.columns.min(row.len()))
                .map(|col| row[Column(col)].c)
                .collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Dropping the sender ends the write loop, which then closes the session.
        self.io = None;
    }
}

async fn write_loop(services: Arc<Services>, id: Id, mut rx: io_mpsc::UnboundedReceiver<Io>) {
    while let Some(message) = rx.recv().await {
        let result = match message {
            Io::Write(bytes) => services.sessions.write(id, &bytes).await,
            Io::Resize(size) => services.sessions.resize(id, size).await,
        };
        if let Err(err) = result {
            tracing::debug!(%err, "terminal write failed");
        }
    }
    services.sessions.close(id).await;
}
