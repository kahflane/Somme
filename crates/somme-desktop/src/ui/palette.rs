use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::FutureExt as _;
use futures::StreamExt as _;
use futures::future::{LocalBoxFuture, Shared};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, KeyDownEvent,
    MouseButton, ParentElement as _, PathPromptOptions, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};
use somme_domain::Id;

use crate::editor::tools::Tool;
use crate::host_os;
use crate::search::{self, Find, Hit, Place};
use crate::store::Store;
use crate::terminal::blocks::short_path;
use crate::terminal::disk::Disk;
use crate::ui::common::Lucide;
use crate::ui::filter::{fuzzy_score, host_haystack, parse_quick_connect};
use crate::ui::workspace::Page;

/// Long lists slow typing down and nobody scrolls that far.
const MAX_ROWS: usize = 50;
/// A search starts once typing pauses this long, not on every key.
const SEARCH_DELAY: Duration = Duration::from_millis(150);
const PLACEHOLDER: &str = "Search hosts, commands and snippets, or type user@host";

/// What a palette row does when picked.
#[derive(Clone)]
pub enum PaletteAction {
    /// Saves typed `user@host:port` as a host and connects.
    QuickConnect(String),
    Host(Id),
    /// Runs a snippet in the terminal used last.
    Snippet(String),
    NewLocalTerminal,
    NewEditor,
    Page(Page),
    Lock,
    /// Opens the SSH section, which asks for the master password.
    Unlock,
    /// Runs a text tool in the file editor that had the focus.
    Tool(Tool),
    /// Turns the palette into a search for files. The palette stays open.
    Search(Find),
    /// Opens what a search found. `here` says it was found in [`Here`], so
    /// it may be on a server.
    Open { hit: Hit, here: bool },
}

pub enum PaletteEvent {
    Pick(PaletteAction),
    Dismiss,
}

/// The folder of the terminal or file in front, where a search looks first.
#[derive(Clone)]
pub struct Here {
    pub dir: PathBuf,
    /// Where `dir` is. A server's file access opens the first time it is needed.
    pub disk: Shared<LocalBoxFuture<'static, Option<Disk>>>,
    /// The server's name, when `dir` is on one.
    pub server: Option<SharedString>,
    /// The home folder there, to show paths short.
    pub home: Option<PathBuf>,
}

/// What was in front when the palette opened.
#[derive(Clone, Default)]
pub struct PaletteContext {
    /// A file editor had the focus, so its text tools are offered.
    pub editor: bool,
    pub here: Option<Here>,
}

/// Where a search looks.
#[derive(Clone, PartialEq, Eq)]
enum Scope {
    Here,
    /// All of the server the terminal in front is on.
    Server,
    Home,
    Computer,
    Folder(PathBuf),
}

struct Searching {
    find: Find,
    scope: Scope,
    hits: Vec<Hit>,
    busy: bool,
    error: Option<String>,
    /// Tells the search running now to stop.
    stop: Arc<AtomicBool>,
    _task: Option<Task<()>>,
}

struct Row {
    action: PaletteAction,
    icon: Icon,
    title: SharedString,
    detail: SharedString,
    kind: &'static str,
}

/// Search box over hosts, snippets and commands. Typing an address offers to
/// connect to it. It also finds files and folders, by name or by the text in
/// them.
pub struct CommandPalette {
    input: Entity<InputState>,
    context: PaletteContext,
    /// The text the rows were made for.
    query: String,
    search: Option<Searching>,
    rows: Vec<Row>,
    selected: usize,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Drop for CommandPalette {
    fn drop(&mut self) {
        if let Some(search) = &self.search {
            search.stop.store(true, Ordering::Relaxed);
        }
    }
}

impl CommandPalette {
    pub fn new(context: PaletteContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        let subscriptions = vec![cx.subscribe_in(&input, window, |palette, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => palette.refresh(cx),
            InputEvent::PressEnter { .. } => palette.pick(palette.selected, window, cx),
            _ => {}
        })];
        let mut palette = Self {
            input,
            context,
            query: String::new(),
            search: None,
            rows: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        palette.show_rows(cx);
        palette
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        // Enter sends a change too, which must not reset the selection.
        let query = self.input.read(cx).value().to_string();
        if query == self.query {
            return;
        }
        self.query = query;
        if self.search.is_some() {
            self.start_search(cx);
        } else {
            self.show_rows(cx);
        }
    }

    fn show_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.query.trim();
        self.rows = rows(query, &self.context, cx);
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        match row.action.clone() {
            PaletteAction::Search(find) => self.start_mode(find, window, cx),
            action => cx.emit(PaletteEvent::Pick(action)),
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    // ---- search ----

    fn start_mode(&mut self, find: Find, window: &mut Window, cx: &mut Context<Self>) {
        let scope = if self.context.here.is_some() { Scope::Here } else { Scope::Home };
        self.search = Some(Searching {
            find,
            scope,
            hits: Vec::new(),
            busy: false,
            error: None,
            stop: Arc::new(AtomicBool::new(false)),
            _task: None,
        });
        let placeholder = match find {
            Find::Names => "Find files and folders by name",
            Find::Text => "Find text in files",
        };
        self.query.clear();
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        self.rows.clear();
        self.selected = 0;
        cx.notify();
    }

    /// Back to the commands, as Backspace in an empty search box does.
    fn leave_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = self.search.take() {
            search.stop.store(true, Ordering::Relaxed);
        }
        self.query.clear();
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(PLACEHOLDER, window, cx);
        });
        self.show_rows(cx);
    }

    fn scopes(&self) -> Vec<Scope> {
        let mut scopes = Vec::new();
        if let Some(here) = &self.context.here {
            scopes.push(Scope::Here);
            if here.server.is_some() {
                scopes.push(Scope::Server);
            }
        }
        scopes.extend([Scope::Home, Scope::Computer]);
        if let Some(Searching { scope: scope @ Scope::Folder(_), .. }) = &self.search {
            scopes.push(scope.clone());
        }
        scopes
    }

    fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        if search.scope != scope {
            search.scope = scope;
            self.start_search(cx);
        }
    }

    fn cycle_scope(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(search) = &self.search else { return };
        let scopes = self.scopes();
        let at = scopes.iter().position(|scope| *scope == search.scope).unwrap_or(0) as isize;
        let next = scopes[(at + delta).rem_euclid(scopes.len() as isize) as usize].clone();
        self.set_scope(next, cx);
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Search Here".into()),
        });
        cx.spawn_in(window, async move |palette, cx| {
            let Ok(Ok(Some(mut dirs))) = picked.await else { return };
            let Some(dir) = dirs.pop() else { return };
            let _ = palette.update_in(cx, |palette, window, cx| {
                palette.set_scope(Scope::Folder(dir), cx);
                palette.focus(window, cx);
            });
        })
        .detach();
    }

    /// Stops the search running now and starts one for the typed text.
    fn start_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        search.stop.store(true, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        search.stop = stop.clone();
        search.hits.clear();
        search.error = None;
        search._task = None;
        self.rows.clear();
        self.selected = 0;
        let query = self.query.trim().to_owned();
        search.busy = !query.is_empty();
        cx.notify();
        if query.is_empty() {
            return;
        }
        let find = search.find;
        let local = || futures::future::ready(Some(Disk::Local)).boxed_local();
        let home = || dirs::home_dir().unwrap_or_default();
        let (disk, place) = match (&search.scope, &self.context.here) {
            (Scope::Here, Some(here)) => (here.disk.clone().boxed_local(), Place::Folder(here.dir.clone())),
            (Scope::Server, Some(here)) => (here.disk.clone().boxed_local(), Place::Folder(PathBuf::from("/"))),
            (Scope::Here | Scope::Server | Scope::Home, _) => (local(), Place::Folder(home())),
            (Scope::Computer, _) => (local(), Place::Computer),
            (Scope::Folder(dir), _) => (local(), Place::Folder(dir.clone())),
        };
        search._task = Some(cx.spawn(async move |palette, cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let Some(disk) = disk.await else {
                let reason = "Can't reach the files on this server.".to_owned();
                let _ = palette.update(cx, |palette, cx| palette.search_done(Err(reason), cx));
                return;
            };
            let (tx, rx) = futures::channel::mpsc::unbounded();
            let work = disk.run(move |disk| {
                let send = move |hit: Hit| {
                    let _ = tx.unbounded_send(hit);
                };
                search::search(disk, &place, find, &query, &stop, &send)
            });
            let mut batches = rx.ready_chunks(64);
            while let Some(batch) = batches.next().await {
                if palette.update(cx, |palette, cx| palette.add_hits(batch, cx)).is_err() {
                    return;
                }
            }
            let result = work.await;
            let _ = palette.update(cx, |palette, cx| palette.search_done(result, cx));
        }));
    }

    fn add_hits(&mut self, batch: Vec<Hit>, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        search.hits.extend(batch);
        let query = self.query.trim().to_lowercase();
        match search.find {
            // Whole names first, then names that start with the query, then short paths.
            Find::Names => search.hits.sort_by_cached_key(|hit| {
                let name = hit.path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
                let rank = if name == query {
                    0
                } else if name.starts_with(&query) {
                    1
                } else {
                    2
                };
                (rank, hit.path.components().count(), hit.path.clone())
            }),
            Find::Text => search.hits.sort_by(|a, b| {
                (&a.path, a.line.as_ref().map(|line| line.0)).cmp(&(&b.path, b.line.as_ref().map(|line| line.0)))
            }),
        }
        search.hits.truncate(search::MAX_HITS);
        // Hits on the terminal's own disk open there.
        let here = matches!(search.scope, Scope::Here | Scope::Server);
        let home = match &self.context.here {
            Some(at) if here && at.server.is_some() => at.home.clone(),
            _ => dirs::home_dir(),
        };
        let base = match (&search.scope, &self.context.here) {
            (Scope::Here, Some(at)) => Some(at.dir.clone()),
            (Scope::Folder(dir), _) => Some(dir.clone()),
            _ => None,
        };
        self.rows = search.hits.iter().map(|hit| hit_row(hit, here, base.as_deref(), home.as_deref())).collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        cx.notify();
    }

    fn search_done(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        search.busy = false;
        search.error = result.err();
        cx.notify();
    }
}

/// A row for a hit. Paths show from `base`, the folder searched, when they
/// are inside it, or else from `home` as `~`.
fn hit_row(hit: &Hit, here: bool, base: Option<&Path>, home: Option<&Path>) -> Row {
    let name = hit.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let action = PaletteAction::Open { hit: hit.clone(), here };
    let show = |path: &Path| match base.and_then(|base| path.strip_prefix(base).ok()) {
        Some(inside) if inside.as_os_str().is_empty() => ".".to_owned(),
        Some(inside) => inside.display().to_string(),
        None => short_path(path, home),
    };
    match &hit.line {
        Some((number, text)) => Row {
            action,
            icon: Icon::new(Lucide::FileText),
            title: if text.is_empty() { name.into() } else { text.clone().into() },
            detail: format!("{} · line {number}", show(&hit.path)).into(),
            kind: "Text",
        },
        None => Row {
            action,
            icon: Icon::new(if hit.folder { Lucide::Folder } else { Lucide::File }),
            title: name.into(),
            detail: hit.path.parent().map(show).unwrap_or_default().into(),
            kind: if hit.folder { "Folder" } else { "File" },
        },
    }
}

fn tool_icon(tool: Tool) -> Lucide {
    match tool {
        Tool::FormatDocument => Lucide::WandSparkles,
        Tool::FormatJson => Lucide::Braces,
        Tool::MinifyJson => Lucide::Minimize2,
        Tool::Base64Encode | Tool::Base64Decode => Lucide::Binary,
        Tool::UrlEncode | Tool::UrlDecode => Lucide::Link,
    }
}

/// Every command, each with the text a query is matched against.
fn command_rows(context: &PaletteContext, cx: &App) -> Vec<(String, Row)> {
    let command = |title: &str, action: PaletteAction, icon: Icon, kind: &'static str| {
        let row = Row {
            action,
            icon,
            title: title.to_owned().into(),
            detail: "".into(),
            kind,
        };
        (title.to_owned(), row)
    };
    let mut rows = Vec::new();
    if context.editor {
        for tool in Tool::ALL {
            rows.push(command(tool.label(), PaletteAction::Tool(tool), Icon::new(tool_icon(tool)), "Editor"));
        }
    }
    rows.push(command(
        "Search Files and Folders…",
        PaletteAction::Search(Find::Names),
        Icon::new(Lucide::FolderSearch),
        "Search",
    ));
    rows.push(command("Search Text in Files…", PaletteAction::Search(Find::Text), Icon::new(Lucide::TextSearch), "Search"));
    rows.push(command("New Local Terminal", PaletteAction::NewLocalTerminal, Icon::new(Lucide::SquareTerminal), "Command"));
    rows.push(command("New Text Editor", PaletteAction::NewEditor, Icon::new(Lucide::FilePen), "Command"));
    for page in Page::ALL {
        rows.push(command(&format!("Go to {}", page.label()), PaletteAction::Page(page), page.icon(), "Command"));
    }
    let (title, action, icon) = if Store::global(cx).read(cx).is_unlocked() {
        ("Lock Saved Servers and Keys", PaletteAction::Lock, Lucide::Lock)
    } else {
        ("Unlock Saved Servers and Keys", PaletteAction::Unlock, Lucide::LockOpen)
    };
    rows.push(command(title, action, Icon::new(icon), "Command"));
    rows
}

fn rows(query: &str, context: &PaletteContext, cx: &App) -> Vec<Row> {
    let store = Store::global(cx).read(cx);
    // Rows keep their section order (hosts, commands, snippets) until a query
    // ranks them. In a file editor its text tools come first.
    let mut rows: Vec<(i32, Row)> = Vec::new();
    let mut add = |text: &str, row: Row| {
        if let Some(score) = fuzzy_score(query, text) {
            rows.push((score, row));
        }
    };
    let mut commands = Some(command_rows(context, cx));
    if context.editor {
        for (text, row) in commands.take().into_iter().flatten() {
            add(&text, row);
        }
    }

    let mut hosts: Vec<_> = store.hosts.iter().collect();
    hosts.sort_by_key(|h| h.label.to_lowercase());
    for host in hosts {
        let mut detail = match host.username.as_deref().filter(|u| !u.is_empty()) {
            Some(user) => format!("{user}@{}", host.address),
            None => host.address.clone(),
        };
        if host.port != 22 {
            detail.push_str(&format!(":{}", host.port));
        }
        let icon = host_os::icon(host, cx);
        add(
            &host_haystack(host),
            Row {
                action: PaletteAction::Host(host.id),
                icon,
                title: host.label.clone().into(),
                detail: detail.into(),
                kind: "Host",
            },
        );
    }

    for (text, row) in commands.into_iter().flatten() {
        add(&text, row);
    }

    for snippet in &store.snippets {
        let first_line = snippet.script.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        add(
            &format!("{} {}", snippet.label, snippet.script),
            Row {
                action: PaletteAction::Snippet(snippet.script.clone()),
                icon: Icon::new(Lucide::SquareCode),
                title: snippet.label.clone().into(),
                detail: first_line.trim().to_owned().into(),
                kind: "Snippet",
            },
        );
    }

    if !query.is_empty() {
        // Stable sort, so equal scores keep their section order.
        rows.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    }
    let mut rows: Vec<Row> = rows.into_iter().map(|(_, row)| row).take(MAX_ROWS).collect();

    // Text that looks like an address is offered as a new connection, unless a saved host has it.
    let looks_like_address =
        query.contains('@') || query.contains('.') || query.contains(':') || query.starts_with("ssh ");
    if let Some((user, address, port)) = parse_quick_connect(query)
        && (looks_like_address || rows.is_empty())
    {
        let saved = store.hosts.iter().any(|h| {
            h.address.eq_ignore_ascii_case(&address)
                && h.port == port
                && (user.is_none() || h.username == user)
        });
        if !saved {
            rows.insert(
                0,
                Row {
                    action: PaletteAction::QuickConnect(query.to_owned()),
                    icon: Icon::new(Lucide::Zap),
                    title: format!("Connect to {query}").into(),
                    detail: "Saves it as a new host".into(),
                    kind: "SSH",
                },
            );
        }
    }
    rows
}

impl CommandPalette {
    /// Where the search looks, as chips to click. Tab moves between them.
    fn render_scopes(&self, search: &Searching, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let chip = |id: &'static str, icon: Lucide, label: SharedString, active: bool| {
            h_flex()
                .id(id)
                .flex_none()
                .gap_1()
                .items_center()
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .border_1()
                .border_color(if active { theme.primary } else { theme.border })
                .when(active, |chip| chip.bg(theme.accent))
                .when(!active, |chip| chip.text_color(theme.muted_foreground).hover(|chip| chip.bg(theme.accent)))
                .child(Icon::new(icon).size(px(12.)))
                .child(div().max_w(px(220.)).truncate().child(label))
        };
        let mut chips = h_flex()
            .px_3()
            .py_1p5()
            .gap_1p5()
            .items_center()
            .flex_wrap()
            .border_b_1()
            .border_color(theme.border)
            .child(div().text_xs().text_color(theme.muted_foreground).child("Look in"));
        let on_server = self.context.here.as_ref().is_some_and(|here| here.server.is_some());
        for scope in self.scopes() {
            let active = search.scope == scope;
            let (id, icon, label): (&'static str, Lucide, SharedString) = match &scope {
                Scope::Here => {
                    let here = self.context.here.as_ref().expect("scope needs a folder");
                    let dir = short_path(&here.dir, here.home.as_deref());
                    match &here.server {
                        Some(server) => ("scope-here", Lucide::Server, format!("{dir} on {server}").into()),
                        None => ("scope-here", Lucide::Folder, dir.into()),
                    }
                }
                Scope::Server => {
                    let server = self.context.here.as_ref().and_then(|here| here.server.clone()).unwrap_or_default();
                    ("scope-server", Lucide::HardDrive, format!("All of {server}").into())
                }
                Scope::Home if on_server => ("scope-home", Lucide::House, "Home on this computer".into()),
                Scope::Home => ("scope-home", Lucide::House, "Home".into()),
                Scope::Computer => ("scope-computer", Lucide::Monitor, "Whole computer".into()),
                Scope::Folder(dir) => {
                    let name = dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned());
                    ("scope-folder", Lucide::FolderOpen, name.into())
                }
            };
            chips = chips.child(
                chip(id, icon, label, active)
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |palette, _, _, cx| palette.set_scope(scope.clone(), cx))),
            );
        }
        chips
            .child(
                chip("scope-choose", Lucide::FolderSearch, "Choose…".into(), false)
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(|palette, _, window, cx| palette.choose_folder(window, cx))),
            )
            .into_any_element()
    }

    fn render_status(&self, search: &Searching, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (text, warn) = if let Some(error) = &search.error {
            (error.clone(), true)
        } else if search.busy {
            ("Searching…".to_owned(), false)
        } else if search.hits.len() >= search::MAX_HITS {
            (format!("Showing the first {}. Type more to narrow it down.", search::MAX_HITS), false)
        } else if !self.query.trim().is_empty() {
            let found = search.hits.len();
            (format!("{found} found"), false)
        } else {
            ("Tab changes where to look · Backspace goes back".to_owned(), false)
        };
        h_flex()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(if warn { theme.warning } else { theme.muted_foreground })
            .child(div().truncate().child(text))
            .into_any_element()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected;
        let empty = match &self.search {
            None => "No matches",
            Some(search) if self.query.trim().is_empty() => match search.find {
                Find::Names => "Type part of a file or folder name",
                Find::Text => "Type the text to look for",
            },
            Some(search) if search.busy => "Searching…",
            Some(_) => "Nothing found",
        };
        let list = if self.rows.is_empty() {
            div().p_4().text_sm().text_color(theme.muted_foreground).child(empty).into_any_element()
        } else {
            v_flex()
                .id("palette-rows")
                .flex_1()
                .min_h_0()
                .p_1()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children(self.rows.iter().enumerate().map(|(ix, row)| {
                    h_flex()
                        .id(ix)
                        .px_2()
                        .py_1p5()
                        .gap_3()
                        .rounded_md()
                        .cursor_pointer()
                        .when(ix == selected, |el| el.bg(theme.accent))
                        .on_mouse_move(cx.listener(move |palette, _, _, cx| {
                            if palette.selected != ix {
                                palette.selected = ix;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |palette, _, window, cx| palette.pick(ix, window, cx)))
                        .child(row.icon.clone().size(px(16.)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().truncate().child(row.title.clone()))
                                .when(!row.detail.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .truncate()
                                            .child(row.detail.clone()),
                                    )
                                }),
                        )
                        .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(row.kind))
                }))
                .into_any_element()
        };
        let prefix = match self.search.as_ref().map(|search| search.find) {
            None => Icon::new(IconName::Search),
            Some(Find::Names) => Icon::new(Lucide::FolderSearch),
            Some(Find::Text) => Icon::new(Lucide::TextSearch),
        };
        let scopes = self.search.as_ref().map(|search| self.render_scopes(search, cx));
        let status = self.search.as_ref().map(|search| self.render_status(search, cx));

        v_flex()
            .id("palette")
            .w(px(600.))
            .max_h(px(480.))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            // Arrow keys move through the list instead of the text cursor.
            .capture_action(cx.listener(|palette, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                palette.move_selection(-1, cx);
            }))
            .capture_action(cx.listener(|palette, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                palette.move_selection(1, cx);
            }))
            .capture_action(cx.listener(|_, _: &Escape, _, cx| {
                cx.stop_propagation();
                cx.emit(PaletteEvent::Dismiss);
            }))
            .capture_key_down(cx.listener(|palette, event: &KeyDownEvent, window, cx| {
                if palette.search.is_none() {
                    return;
                }
                let keystroke = &event.keystroke;
                let modifiers = keystroke.modifiers;
                let plain = !(modifiers.control || modifiers.platform || modifiers.alt);
                if keystroke.key == "tab" && plain {
                    cx.stop_propagation();
                    palette.cycle_scope(if modifiers.shift { -1 } else { 1 }, cx);
                } else if keystroke.key == "backspace" && plain && palette.query.is_empty() {
                    cx.stop_propagation();
                    palette.leave_search(window, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div().px_2().py_1().border_b_1().border_color(theme.border).child(
                    Input::new(&self.input).appearance(false).prefix(prefix.size(px(16.))),
                ),
            )
            .children(scopes)
            .child(list)
            .children(status)
    }
}
