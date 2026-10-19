use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, SharedString, Task};
use parking_lot::Mutex;
use somme_application::ports::{OutputSink, ShellOptions, TermSize};
use somme_domain::Id;
use tokio::sync::mpsc as io_mpsc;

use crate::github::repo::{self, Repo};
use crate::runtime;
use crate::services::Services;
use crate::settings::Settings;
use crate::terminal::blocks::{self, Block, BlockState, GitInfo};
use crate::terminal::colors;
use crate::terminal::disk::Disk;
use crate::terminal::log::{self, SessionLog};
use crate::terminal::marks::{Mark, MarkScanner, Piece};

/// How long a new terminal waits for the shell hooks before showing a classic screen.
pub const HOOKS_WAIT: Duration = Duration::from_secs(3);

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
    /// A command started, or the shell is back at its prompt.
    PhaseChanged,
    /// The shell moved in or out of a GitHub repository.
    RepoChanged,
}

/// Command blocks for a shell that runs Somme's hooks.
#[derive(Default)]
pub struct Blocks {
    pub list: Vec<Block>,
    /// Set once the shell's first prompt mark arrives.
    pub ready: bool,
    /// The shell's folder at its last prompt. On a server it is the server's folder.
    pub cwd: Option<PathBuf>,
    pub git: Option<GitInfo>,
    /// The GitHub repository `origin` points at, for the shell's folder.
    pub repo: Option<Repo>,
    /// The shell's PATH, for completing command names.
    pub path: Option<String>,
    /// The user's home folder where the shell runs.
    pub home: Option<PathBuf>,
    /// The command sent from the editor, until the shell starts it.
    pending: Option<String>,
    /// Since when the view waits for the hooks' first mark. Unset when the
    /// shell runs without them.
    waiting_since: Option<Instant>,
}

impl Blocks {
    pub fn live(&self) -> Option<&Block> {
        self.list.last().filter(|block| block.is_live())
    }

    fn live_mut(&mut self) -> Option<&mut Block> {
        self.list.last_mut().filter(|block| block.is_live())
    }
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
    /// Where a local shell starts. Unset means the home folder.
    start_dir: Option<PathBuf>,
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
    scanner: MarkScanner,
    /// Set when command blocks are on; used once the shell's hooks report in.
    pub blocks: Option<Blocks>,
    /// Keeps the branch and changes up to date while files change.
    git_watch: Option<GitWatch>,
}

/// Watches a repo on this computer, so the shell's branch and changes
/// follow edits made anywhere, not only at the next prompt.
struct GitWatch {
    root: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
    _task: Task<()>,
}

/// How long changes settle before the branch and changes are read again.
const GIT_SETTLE: Duration = Duration::from_millis(600);
/// Where watching a whole repo costs too much, it is read this often.
const GIT_POLL: Duration = Duration::from_secs(4);

/// Whether a change at `path` can change what `git diff HEAD` says.
fn moves_git(root: &Path, path: &Path) -> bool {
    let Ok(inside) = path.strip_prefix(root) else { return false };
    let mut parts = inside.components().map(|part| part.as_os_str().to_string_lossy());
    match parts.next().as_deref() {
        // A commit or a branch switch; the rest of .git is git's own work.
        Some(".git") => matches!(parts.next().as_deref(), Some("HEAD" | "refs" | "packed-refs")),
        Some(_) => !inside.components().any(|part| matches!(part.as_os_str().to_str(), Some("target" | "node_modules"))),
        None => false,
    }
}

impl EventEmitter<SessionEvent> for TerminalSession {}

impl TerminalSession {
    pub fn new(target: SessionTarget, start_dir: Option<PathBuf>, services: Arc<Services>, cx: &mut App) -> Entity<Self> {
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
            let blocks = (!replay && settings.command_blocks).then(Blocks::default);
            let mut session = Self {
                target,
                start_dir,
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
                scanner: MarkScanner::default(),
                blocks,
                git_watch: None,
            };
            session.connect(cx);
            session
        })
    }

    pub fn title(&self) -> SharedString {
        if let SessionTarget::Replay { label, .. } = &self.target {
            return format!("Log: {label}").into();
        }
        // A local shell's folder says more than the title it sets.
        if let SessionTarget::Local = self.target
            && let Some(blocks) = self.blocks.as_ref()
            && let Some(cwd) = blocks.cwd.as_deref()
        {
            return blocks::short_path(cwd, blocks.home.as_deref()).into();
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

    /// The screen output goes to now: the running block's, or the main one.
    pub fn term(&self) -> &Term<Listener> {
        match self.blocks.as_ref().and_then(Blocks::live) {
            Some(block) => &block.term,
            None => &self.term,
        }
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        match self.blocks.as_mut().and_then(Blocks::live_mut) {
            Some(block) => &mut block.term,
            None => &mut self.term,
        }
    }

    /// Whether the view shows command blocks instead of one screen. Full-screen
    /// programs like vim still get the whole screen.
    pub fn shows_blocks(&self) -> bool {
        match &self.blocks {
            None => false,
            Some(blocks) if blocks.ready => !self.term().mode().contains(TermMode::ALT_SCREEN),
            Some(blocks) => self.is_running() && blocks.waiting_since.is_some_and(|since| since.elapsed() < HOOKS_WAIT),
        }
    }

    /// The shell waits at its prompt, so the editor can take a command.
    pub fn at_prompt(&self) -> bool {
        self.is_running() && self.blocks.as_ref().is_some_and(|b| b.ready && b.live().is_none())
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn mode(&self) -> TermMode {
        *self.term().mode()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, Status::Connecting | Status::Connected)
    }

    /// The folder a live local shell is in now.
    pub fn current_dir(&self) -> Option<PathBuf> {
        match (&self.target, self.session_id) {
            (SessionTarget::Local, Some(id)) if self.is_running() => self.services.sessions.cwd(id),
            _ => None,
        }
    }

    pub fn is_replay(&self) -> bool {
        matches!(self.target, SessionTarget::Replay { .. })
    }

    /// Whether the shell runs on this computer, so its folders are local ones.
    pub fn is_local(&self) -> bool {
        matches!(self.target, SessionTarget::Local)
    }

    /// Where this shell's files are. On a server, this opens file access over
    /// the session's connection; None when the server offers none.
    pub fn disk(&self) -> impl Future<Output = Option<Disk>> + 'static {
        let local = self.is_local();
        let id = self.session_id.filter(|_| self.is_running());
        let home = self.blocks.as_ref().and_then(|blocks| blocks.home.clone());
        let services = self.services.clone();
        async move {
            if local {
                return Some(Disk::Local);
            }
            let id = id?;
            let files = runtime::io(async move { services.sessions.files(id).await }).await?;
            Some(Disk::Remote { files, home })
        }
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
        // A new secret for each shell, so output from programs cannot fake marks.
        let token = self.blocks.is_some().then(|| uuid::Uuid::new_v4().simple().to_string());
        self.scanner = token.as_deref().map(MarkScanner::new).unwrap_or_default();
        let size = self.size;
        if let Some(blocks) = self.blocks.as_mut() {
            if let Some(block) = blocks.live_mut() {
                block.finish(-1, size);
            }
            blocks.ready = false;
            blocks.waiting_since = Some(Instant::now());
        }
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
        let start_dir = self.start_dir.clone();
        let size = TermSize {
            cols: self.size.columns as u16,
            rows: self.size.lines as u16,
        };
        cx.spawn(async move |this, cx| {
            let sink: Arc<dyn OutputSink> = Arc::new(UiSink(output_tx));
            let sessions = services.sessions.clone();
            let opened = runtime::io(async move {
                match target {
                    SessionTarget::Local => {
                        let options = ShellOptions {
                            dir: start_dir.as_deref(),
                            marks: token.as_deref(),
                        };
                        sessions.open_local(size, options, move |_| sink)
                    }
                    SessionTarget::Ssh { host_id, .. } => {
                        sessions.open_ssh(host_id, size, token.as_deref(), move |_| sink).await
                    }
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
                        if let SessionTarget::Ssh { host_id, .. } = &session.target {
                            crate::host_os::check(*host_id, session.disk(), cx);
                        }
                        session.wait_for_hooks(services.sessions.has_hooks(id), cx);
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

    /// Waits a little for the hooks' first mark, or shows the classic screen
    /// right away when the shell runs without them.
    fn wait_for_hooks(&mut self, hooks: bool, cx: &mut Context<Self>) {
        let Some(blocks) = self.blocks.as_mut().filter(|blocks| !blocks.ready) else { return };
        blocks.waiting_since = hooks.then(Instant::now);
        if hooks {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOOKS_WAIT).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
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
                if self.blocks.is_some() {
                    let mut scanner = std::mem::take(&mut self.scanner);
                    scanner.scan(&bytes, |piece| match piece {
                        Piece::Output(bytes) => self.feed(bytes),
                        Piece::Mark(mark) => self.on_mark(mark, cx),
                    });
                    self.scanner = scanner;
                } else {
                    self.parser.advance(&mut self.term, &bytes);
                }
                self.drain_term_events(cx);
            }
            Output::Closed(reason) => {
                self.io = None;
                self.session_id = None;
                let size = self.size;
                if let Some(block) = self.blocks.as_mut().and_then(Blocks::live_mut) {
                    block.finish(0, size);
                }
                if let Some(log) = self.log.as_mut() {
                    log.note(reason.as_deref().unwrap_or("Session closed"));
                }
                self.status = Status::Closed(reason);
                cx.emit(SessionEvent::StatusChanged);
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let term = match self.blocks.as_mut().and_then(Blocks::live_mut) {
            Some(block) => &mut block.term,
            None => &mut self.term,
        };
        self.parser.advance(term, bytes);
    }

    fn new_block_term(&self, cx: &App) -> Term<Listener> {
        let config = TermConfig {
            scrolling_history: Settings::get(cx).scrollback_lines,
            ..TermConfig::default()
        };
        Term::new(config, &self.size, self.listener.clone())
    }

    fn on_mark(&mut self, mark: Mark, cx: &mut Context<Self>) {
        let size = self.size;
        let fresh = self.new_block_term(cx);
        let Some(blocks) = self.blocks.as_mut() else { return };
        match mark {
            Mark::Prompt { exit, cwd } => {
                if !blocks.ready {
                    blocks.ready = true;
                    // What the shell printed so far, like a server's welcome
                    // message, becomes the first block. The main screen then only
                    // holds hidden prompts, so it needs no history.
                    let config = TermConfig {
                        scrolling_history: 0,
                        ..TermConfig::default()
                    };
                    let startup = std::mem::replace(&mut self.term, Term::new(config, &size, self.listener.clone()));
                    blocks.list.extend(Block::startup(startup, size));
                }
                if let Some(block) = blocks.live_mut() {
                    block.finish(exit, size);
                    if blocks::clears_screen(&block.command) {
                        blocks.list.clear();
                    }
                }
                blocks.pending = None;
                let moved = blocks.cwd != cwd;
                blocks.cwd = cwd;
                if moved {
                    blocks.git = None;
                    // The tab names the folder, so it changes too.
                    cx.emit(SessionEvent::TitleChanged);
                    self.refresh_repo(cx);
                }
                self.refresh_git(cx);
            }
            Mark::Accepted => {
                if blocks.live().is_some() {
                    // The shell asked for more of the line and got it.
                    return;
                }
                let command = blocks.pending.take().unwrap_or_default();
                let branch = blocks.git.as_ref().map(|git| git.branch.clone());
                blocks.list.push(Block::new(command, blocks.cwd.clone(), branch, BlockState::Accepted, fresh));
            }
            Mark::Path(path) => {
                blocks.path = Some(path);
                return;
            }
            Mark::Home(home) => {
                blocks.home = Some(home);
                return;
            }
            Mark::Command(command) => {
                match blocks.live_mut() {
                    Some(block) if block.state == BlockState::Accepted => {
                        // Drop the shell's redraw of the line; the block shows the command itself.
                        block.term = fresh;
                        block.state = BlockState::Running;
                        block.started = Instant::now();
                        if !command.is_empty() {
                            block.command = command.clone();
                        }
                    }
                    Some(_) => {}
                    None => {
                        let branch = blocks.git.as_ref().map(|git| git.branch.clone());
                        let block = Block::new(command.clone(), blocks.cwd.clone(), branch, BlockState::Running, fresh);
                        blocks.list.push(block);
                    }
                }
                if Settings::get(cx).keep_history
                    && !command.trim().is_empty()
                    && self.history.last() != Some(&command)
                {
                    self.history.push(command);
                }
            }
        }
        cx.emit(SessionEvent::PhaseChanged);
    }

    /// Looks up the branch and changes for the shell's folder in the background.
    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        if !self.is_local() {
            return;
        }
        let Some(dir) = self.blocks.as_ref().and_then(|b| b.cwd.clone()) else { return };
        cx.spawn(async move |this, cx| {
            let lookup = dir.clone();
            let git = cx.background_spawn(async move { blocks::git_info(&lookup) }).await;
            let _ = this.update(cx, |session, cx| {
                if let Some(blocks) = session.blocks.as_mut()
                    && blocks.cwd.as_ref() == Some(&dir)
                {
                    let root = git.as_ref().map(|git| git.root.clone());
                    blocks.git = git;
                    session.watch_repo(root, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reads the branch and changes again whenever files in `root` change.
    fn watch_repo(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.git_watch.as_ref().map(|watch| &watch.root) == root.as_ref() {
            return;
        }
        let Some(root) = root else {
            self.git_watch = None;
            return;
        };
        let (changes, mut changed) = mpsc::unbounded::<()>();
        // Linux watches each folder on its own, too many in a big repo.
        let watcher = if cfg!(target_os = "linux") {
            None
        } else {
            let inside = root.clone();
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event
                    && event.paths.iter().any(|path| moves_git(&inside, path))
                {
                    let _ = changes.unbounded_send(());
                }
            })
            .and_then(|mut watcher| {
                use notify::Watcher as _;
                watcher.watch(&root, notify::RecursiveMode::Recursive).map(|()| watcher)
            })
            .map_err(|err| tracing::warn!(%err, "the shell's git changes can't follow edits"))
            .ok()
        };
        let polls = watcher.is_none();
        let task = cx.spawn(async move |this, cx| {
            loop {
                if polls {
                    cx.background_executor().timer(GIT_POLL).await;
                } else {
                    if changed.next().await.is_none() {
                        break;
                    }
                    cx.background_executor().timer(GIT_SETTLE).await;
                    while changed.try_recv().is_ok() {}
                }
                if this.update(cx, |session, cx| session.refresh_git(cx)).is_err() {
                    break;
                }
            }
        });
        self.git_watch = Some(GitWatch { root, _watcher: watcher, _task: task });
    }

    /// Looks up the GitHub repository of the shell's folder, here or on the server.
    fn refresh_repo(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.blocks.as_ref().and_then(|b| b.cwd.clone()) else { return };
        let disk = self.disk();
        cx.spawn(async move |this, cx| {
            let lookup = dir.clone();
            let found = match disk.await {
                Some(disk) => disk.run(move |disk| repo::find(disk, &lookup)).await,
                None => None,
            };
            let _ = this.update(cx, |session, cx| {
                if let Some(blocks) = session.blocks.as_mut()
                    && blocks.cwd.as_ref() == Some(&dir)
                    && blocks.repo != found
                {
                    blocks.repo = found;
                    cx.emit(SessionEvent::RepoChanged);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Runs a command typed in the editor. Returns the bytes sent.
    pub fn run_command(&mut self, command: &str, cx: &mut Context<Self>) -> Vec<u8> {
        if let Some(blocks) = self.blocks.as_mut() {
            blocks.pending = Some(command.to_owned());
        }
        // Ctrl-U first drops anything stray in the shell's own line.
        let bytes = if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x15\x1b[200~{}\x1b[201~\r", command.replace('\x1b', ""))
        } else {
            format!("\x15{}\r", command.replace("\r\n", "\r").replace('\n', "\r"))
        };
        let bytes = bytes.into_bytes();
        self.send(bytes.clone());
        cx.notify();
        bytes
    }

    /// Runs a line: through the editor path at a blocks prompt, typed otherwise.
    pub fn run(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.at_prompt() {
            self.run_command(text.trim_end_matches(['\r', '\n']), cx);
            return;
        }
        let mut bytes = text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
        if bytes.last() != Some(&b'\r') {
            bytes.push(b'\r');
        }
        self.input(bytes, cx);
    }

    /// Removes finished blocks.
    pub fn clear_blocks(&mut self, cx: &mut Context<Self>) {
        if let Some(blocks) = self.blocks.as_mut() {
            blocks.list.retain(Block::is_live);
            cx.notify();
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
                // Old logs never touch the clipboard; live servers only when allowed.
                TermEvent::ClipboardStore(_, text) => {
                    if !self.is_replay() && Settings::get(cx).remote_clipboard_write {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
                // A server may copy into the clipboard but never read it: it could hold a password.
                TermEvent::ClipboardLoad(..) => {}
                TermEvent::ColorRequest(index, format) => {
                    let theme = colors::active_theme(cx);
                    let color = self.term().colors()[index].unwrap_or_else(|| match index {
                        256 => theme.foreground,
                        257 => theme.background,
                        258 => theme.cursor,
                        i if i < 16 => theme.ansi[i],
                        i => colors::resolve(
                            alacritty_terminal::vte::ansi::Color::Indexed(i as u8),
                            self.term().colors(),
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
        // With blocks, the shell reports each command itself.
        if Settings::get(cx).keep_history && self.blocks.is_none() {
            self.track_history(&bytes);
        }
        let term = self.term_mut();
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
        term.selection = None;
        self.send(bytes);
        cx.notify();
    }

    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let bytes = if self.term().mode().contains(TermMode::BRACKETED_PASTE) {
            let clean = text.replace('\x1b', "");
            format!("\x1b[200~{clean}\x1b[201~").into_bytes()
        } else {
            text.into_bytes()
        };
        self.input(bytes, cx);
    }

    fn track_history(&mut self, bytes: &[u8]) {
        if self.term().mode().contains(TermMode::ALT_SCREEN) {
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
        if let Some(blocks) = self.blocks.as_mut() {
            for block in &mut blocks.list {
                block.resize(size);
            }
        }
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
        self.term_mut().scroll_display(Scroll::Delta(lines));
        cx.notify();
    }

    pub fn scroll_to(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        self.term_mut().scroll_display(scroll);
        cx.notify();
    }

    pub fn clear_scrollback(&mut self, cx: &mut Context<Self>) {
        if self.shows_blocks() {
            self.clear_blocks(cx);
            return;
        }
        self.term_mut().grid_mut().clear_history();
        // Ask the shell to redraw its prompt on a clean screen.
        self.send(b"\x0c".to_vec());
        cx.notify();
    }

    pub fn start_selection(&mut self, kind: SelectionType, point: GridPoint, side: alacritty_terminal::index::Side) {
        self.term_mut().selection = Some(Selection::new(kind, point, side));
    }

    pub fn update_selection(&mut self, point: GridPoint, side: alacritty_terminal::index::Side) {
        if let Some(selection) = self.term_mut().selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let term = self.term_mut();
        let start = GridPoint::new(term.topmost_line(), Column(0));
        let end = GridPoint::new(term.bottommost_line(), term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, alacritty_terminal::index::Side::Left);
        selection.update(end, alacritty_terminal::index::Side::Right);
        term.selection = Some(selection);
        cx.notify();
    }

    pub fn clear_selection(&mut self) {
        self.term_mut().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term().selection_to_string().filter(|text| !text.is_empty())
    }

    /// The visible text, used for search and tests.
    pub fn visible_text(&self) -> String {
        let grid = self.term().grid();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changes_git_can_see_wake_the_watch() {
        let root = Path::new("/work/app");
        assert!(moves_git(root, Path::new("/work/app/src/main.rs")));
        assert!(moves_git(root, Path::new("/work/app/.git/HEAD")));
        assert!(moves_git(root, Path::new("/work/app/.git/refs/heads/main")));
        assert!(!moves_git(root, Path::new("/work/app/.git/index.lock")));
        assert!(!moves_git(root, Path::new("/work/app/target/debug/app")));
        assert!(!moves_git(root, Path::new("/elsewhere/file")));
    }
}
