use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::badge::Badge;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, TitleBar, WindowExt as _, h_flex,
    v_flex,
};
use futures::FutureExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, PathPromptOptions, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, actions, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use somme_application::dto::{HostInput, HostView, SecretUpdate};
use somme_domain::Id;

use crate::changes::ChangesView;
use crate::editor::pane::{FileText, cant_open, read_text};
use crate::editor::scratch::{self, OpenEntry};
use crate::editor::system_open;
use crate::editor::{EditorPane, EditorPaneEvent};
use crate::github::api::Kind;
use crate::github::counts::Counts;
use crate::github::profile;
use crate::github::repo::Repo;
use crate::services::Services;
use crate::settings::Settings;
use crate::search::Hit;
use crate::store::{Store, StoreEvent, mutate};
use crate::terminal::SessionTarget;
use crate::terminal::view::TerminalView;
use crate::terminal::disk::Disk;
use crate::ui::common::{Lucide, avatar, notify_error, parse_color};
use crate::ui::native;
use crate::ui::fields::host_input;
use crate::ui::filter::parse_quick_connect;
use crate::ui::forwards::ForwardsPage;
use crate::ui::hosts::HostsPage;
use crate::ui::keychain::KeychainPage;
use crate::ui::lock::LockScreen;
use crate::ui::known_hosts::KnownHostsPage;
use crate::ui::palette::{CommandPalette, Here, PaletteAction, PaletteContext, PaletteEvent};
use crate::ui::pinned::{self, Pinned};
use crate::ui::settings_page::SettingsPage;
use crate::ui::side_panel::{SidePanel, SidePanelEvent};
use crate::ui::sftp::SftpPage;
use crate::ui::snippets::SnippetsPage;
use crate::ui::terminal_tab::{TerminalTab, TerminalTabEvent};
use crate::ui::update;
use crate::update::Updates;

actions!(
    workspace,
    [
        NewLocalTerminal,
        NewEditor,
        OpenFile,
        CloseTab,
        ClosePane,
        NextTab,
        PreviousTab,
        ShowHome,
        SplitRight,
        SplitLeft,
        SplitDown,
        SplitUp,
        FocusNextPane,
        FocusPreviousPane,
        ToggleBroadcast,
        LockVault,
        OpenSettings,
        TogglePalette,
        ToggleSidePanel,
        Quit,
        RenameTab,
        TogglePinTab,
        ShowHosts,
        ShowKeychain,
        ShowSnippets,
        ShowForwarding,
        ShowKnownHosts,
        ShowLogs,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectTab9,
    ]
);

pub fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    let m = "cmd";
    #[cfg(not(target_os = "macos"))]
    let m = "ctrl-shift";
    cx.bind_keys([
        KeyBinding::new(&format!("{m}-t"), NewLocalTerminal, None),
        KeyBinding::new(&format!("{m}-n"), NewEditor, None),
        KeyBinding::new(&format!("{m}-o"), OpenFile, None),
        KeyBinding::new(&format!("{m}-w"), ClosePane, None),
        KeyBinding::new(&format!("{m}-shift-w"), CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new(&format!("{m}-shift-]"), NextTab, None),
        KeyBinding::new(&format!("{m}-shift-["), PreviousTab, None),
        KeyBinding::new(&format!("{m}-d"), SplitRight, None),
        KeyBinding::new(&format!("{m}-shift-d"), SplitDown, None),
        KeyBinding::new(&format!("{m}-]"), FocusNextPane, None),
        KeyBinding::new(&format!("{m}-["), FocusPreviousPane, None),
        KeyBinding::new(&format!("{m}-alt-i"), ToggleBroadcast, None),
        KeyBinding::new(&format!("{m}-shift-l"), LockVault, None),
        KeyBinding::new(&format!("{m}-shift-h"), ShowHome, None),
        KeyBinding::new(&format!("{m}-,"), OpenSettings, None),
        KeyBinding::new(&format!("{m}-p"), TogglePalette, None),
        KeyBinding::new(&format!("{m}-shift-s"), ToggleSidePanel, None),
        KeyBinding::new(&format!("{m}-q"), Quit, None),
        KeyBinding::new(&format!("{m}-1"), SelectTab1, None),
        KeyBinding::new(&format!("{m}-2"), SelectTab2, None),
        KeyBinding::new(&format!("{m}-3"), SelectTab3, None),
        KeyBinding::new(&format!("{m}-4"), SelectTab4, None),
        KeyBinding::new(&format!("{m}-5"), SelectTab5, None),
        KeyBinding::new(&format!("{m}-6"), SelectTab6, None),
        KeyBinding::new(&format!("{m}-7"), SelectTab7, None),
        KeyBinding::new(&format!("{m}-8"), SelectTab8, None),
        KeyBinding::new(&format!("{m}-9"), SelectTab9, None),
    ]);
}

/// What the rail on the left shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// The terminal tabs. The app starts here.
    Cli,
    /// Hosts and the rest of the vault. Asks for the master password first.
    Ssh,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Hosts,
    Sftp,
    Keychain,
    Forwarding,
    Snippets,
    KnownHosts,
    Settings,
}

impl Page {
    /// The pages in the SSH section, in sidebar order.
    const VAULT: [Page; 6] = [
        Page::Hosts,
        Page::Sftp,
        Page::Keychain,
        Page::Forwarding,
        Page::Snippets,
        Page::KnownHosts,
    ];

    pub(crate) const ALL: [Page; 7] = [
        Page::Hosts,
        Page::Sftp,
        Page::Keychain,
        Page::Forwarding,
        Page::Snippets,
        Page::KnownHosts,
        Page::Settings,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Page::Hosts => "Hosts",
            Page::Sftp => "SFTP",
            Page::Keychain => "Keychain",
            Page::Forwarding => "Port Forwarding",
            Page::Snippets => "Snippets",
            Page::KnownHosts => "Known Hosts",
            Page::Settings => "Settings",
        }
    }

    pub(crate) fn icon(self) -> Icon {
        match self {
            Page::Hosts => Icon::new(Lucide::Server),
            Page::Sftp => Icon::new(Lucide::FolderOpen),
            Page::Keychain => Icon::new(Lucide::KeyRound),
            Page::Forwarding => Icon::new(Lucide::ArrowLeftRight),
            Page::Snippets => Icon::new(Lucide::SquareCode),
            Page::KnownHosts => Icon::new(Lucide::ShieldCheck),
            Page::Settings => Icon::new(Lucide::Settings),
        }
    }
}

enum TabContent {
    Terminal(Entity<TerminalTab>),
    Changes(Entity<ChangesView>),
    /// Files on this computer, or scratch files not saved anywhere yet.
    Editor(Entity<EditorPane>),
}

/// What the command palette's picks act on.
#[derive(Default)]
struct PaletteTarget {
    /// The file editor that had the focus, for text tools.
    editor: Option<WeakEntity<EditorPane>>,
    /// The terminal whose folder a search looked in.
    terminal: Option<(WeakEntity<TerminalTab>, WeakEntity<TerminalView>)>,
}

struct Tab {
    id: usize,
    /// Kept on the left, safe from ⌘W, and opened again at the next start.
    pinned: bool,
    content: TabContent,
    _subscriptions: Vec<Subscription>,
}

/// A tab whose new name is being typed.
struct Renaming {
    tab_id: usize,
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// Which tabs a "Close …" item in a tab's menu closes.
#[derive(Clone, Copy)]
enum CloseTabs {
    Others,
    Left,
    Right,
    All,
}

impl CloseTabs {
    /// Whether the tab at `ix` goes, for the menu of the tab at `clicked`.
    fn picks(self, ix: usize, clicked: usize) -> bool {
        match self {
            CloseTabs::Others => ix != clicked,
            CloseTabs::Left => ix < clicked,
            CloseTabs::Right => ix > clicked,
            CloseTabs::All => true,
        }
    }
}

/// A tab being dragged to a new place. It also draws itself under the pointer.
#[derive(Clone)]
struct DraggedTab {
    id: usize,
    title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .h(px(26.))
            .max_w(px(220.))
            .px_2()
            .flex()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_sm()
            .text_color(theme.foreground)
            .shadow_md()
            .truncate()
            .child(self.title.clone())
    }
}

pub struct Workspace {
    services: Arc<Services>,
    tabs: Vec<Tab>,
    /// The tab in front when the CLI section shows. `None` once every tab is closed.
    active: Option<usize>,
    section: Section,
    /// The vault page the SSH section shows.
    page: Page,
    next_tab_id: usize,
    /// The terminal tab used last, where snippets run from the home screen.
    last_terminal: Option<usize>,
    /// The tab on this computer used last, where the CLI icon goes back to.
    last_local: Option<usize>,
    lock_screen: Entity<LockScreen>,
    hosts: Option<Entity<HostsPage>>,
    keychain: Option<Entity<KeychainPage>>,
    forwards: Option<Entity<ForwardsPage>>,
    snippets: Option<Entity<SnippetsPage>>,
    known_hosts: Option<Entity<KnownHostsPage>>,
    settings: Option<Entity<SettingsPage>>,
    sftp: Option<Entity<SftpPage>>,
    focus_handle: FocusHandle,
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    /// What was in front when the palette opened, for its picks to act on.
    palette_target: PaletteTarget,
    /// Snippets and themes next to the terminal. Made the first time it shows.
    side_panel: Option<(Entity<SidePanel>, Subscription)>,
    /// Last key press, click or scroll. Auto-lock counts idle time from here.
    last_activity: Instant,
    renaming: Option<Renaming>,
    /// The tab strip, which scrolls when the tabs don't fit.
    tab_scroll: ScrollHandle,
    /// The pinned tabs as last saved, so they are written only when they change.
    pinned_saved: Vec<Pinned>,
    /// Pinned servers waiting for the vault to open before they connect.
    waiting_pins: Vec<Id>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let lock_screen = cx.new(|cx| LockScreen::new(window, cx));
        let subscriptions = vec![
            cx.observe_in(&store, window, |workspace, _, window, cx| {
                workspace.open_waiting_pins(window, cx);
                cx.notify();
            }),
            // New numbers for the pull request and issue badges.
            cx.observe_global::<Counts>(|_, cx| cx.notify()),
            // An update found, or how its download is going.
            cx.observe_global::<Updates>(|_, cx| cx.notify()),
            cx.subscribe_in(&store, window, |workspace, _, event, window, cx| match event {
                // Locking only closes the vault. Terminals stay as they are.
                StoreEvent::VaultChanged => {
                    if workspace.section == Section::Ssh {
                        workspace.focus_current(window, cx);
                    }
                }
            }),
        ];
        Self::watch_idle(cx);
        Self::watch_github(cx);
        let mut workspace = Self {
            services,
            tabs: Vec::new(),
            active: None,
            section: Section::Cli,
            page: Page::Hosts,
            next_tab_id: 0,
            last_terminal: None,
            last_local: None,
            lock_screen,
            hosts: None,
            keychain: None,
            forwards: None,
            snippets: None,
            known_hosts: None,
            settings: None,
            sftp: None,
            focus_handle: cx.focus_handle(),
            palette: None,
            palette_target: PaletteTarget::default(),
            side_panel: None,
            last_activity: Instant::now(),
            renaming: None,
            tab_scroll: ScrollHandle::new(),
            pinned_saved: Vec::new(),
            waiting_pins: Vec::new(),
            _subscriptions: subscriptions,
        };
        // The app starts on a local terminal, without waiting for the vault:
        // the pinned ones, or else a new one.
        if !workspace.restore_pinned(window, cx) {
            workspace.open_terminal(SessionTarget::Local, window, cx);
        }
        if Settings::get(cx).reopen_files {
            workspace.reopen_files(window, cx);
        }
        workspace
    }

    /// Keeps the pull request and issue badges up to date for the repository
    /// the focused shell is in. Each check is cheap; GitHub is only asked when
    /// the numbers are missing or old.
    fn watch_github(cx: &mut Context<Self>) {
        cx.spawn(async move |workspace, cx| {
            loop {
                let alive = workspace.update(cx, |workspace, cx| {
                    let Some(repo) = workspace.github_repo(cx) else { return };
                    Counts::refresh(&repo, &workspace.services, false, cx);
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(5)).await;
            }
        })
        .detach();
    }

    /// The GitHub repository of the focused shell, when its buttons show.
    fn github_repo(&self, cx: &App) -> Option<Repo> {
        if !(Store::global(cx).read(cx).account.signed_in || fake_github()) {
            return None;
        }
        self.active_terminal()?.read(cx).repo(cx)
    }

    /// Locks the vault once the user has been idle longer than the auto-lock setting.
    fn watch_idle(cx: &mut Context<Self>) {
        cx.spawn(async move |workspace, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(15)).await;
                let alive = workspace.update(cx, |workspace, cx| {
                    let minutes = Settings::get(cx).auto_lock_minutes;
                    let store = Store::global(cx);
                    if minutes == 0 || !store.read(cx).is_unlocked() {
                        return;
                    }
                    if workspace.last_activity.elapsed() >= Duration::from_secs(u64::from(minutes) * 60) {
                        workspace.last_activity = Instant::now();
                        store.update(cx, |store, cx| store.lock(cx));
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    fn focus_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.section {
            Section::Cli => match self.active.and_then(|ix| self.tabs.get(ix)) {
                Some(Tab {
                    content: TabContent::Terminal(tab),
                    ..
                }) => window.focus(&tab.focus_handle(cx), cx),
                Some(Tab {
                    content: TabContent::Changes(view),
                    ..
                }) => window.focus(&view.focus_handle(cx), cx),
                Some(Tab {
                    content: TabContent::Editor(pane),
                    ..
                }) => window.focus(&pane.focus_handle(cx), cx),
                None => window.focus(&self.focus_handle, cx),
            },
            Section::Ssh if !Store::global(cx).read(cx).is_unlocked() => {
                self.lock_screen.update(cx, |lock, cx| lock.focus_password(window, cx));
            }
            Section::Ssh | Section::Settings => window.focus(&self.focus_handle, cx),
        }
    }

    // ---- tabs ----

    pub fn open_terminal(&mut self, target: SessionTarget, window: &mut Window, cx: &mut Context<Self>) {
        let dir = matches!(target, SessionTarget::Local).then(|| self.previous_local_dir(cx)).flatten();
        self.open_terminal_in(target, dir, window, cx);
    }

    /// Opens a terminal starting in `dir`, a folder on this computer.
    fn open_terminal_in(&mut self, target: SessionTarget, dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.add_terminal(target, dir, window, cx);
        self.activate(Some(self.tabs.len() - 1), window, cx);
    }

    /// Adds a terminal tab at the end without bringing it to the front.
    fn add_terminal(&mut self, target: SessionTarget, dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let tab = cx.new(|cx| TerminalTab::new(target, dir, services, window, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let subscription = cx.subscribe_in(&tab, window, move |workspace, _, event, window, cx| match event {
            TerminalTabEvent::Changed => {
                // A pinned shell that moved reopens in its new folder.
                workspace.save_pinned(cx);
                cx.notify();
            }
            TerminalTabEvent::Empty => workspace.remove_tab(id, window, cx),
            TerminalTabEvent::OpenChanges(dir) => workspace.open_changes(dir.clone(), None, window, cx),
            TerminalTabEvent::OpenCommit(dir, sha) => workspace.open_changes(dir.clone(), Some(sha.clone()), window, cx),
        });
        self.tabs.push(Tab {
            id,
            pinned: false,
            content: TabContent::Terminal(tab),
            _subscriptions: vec![subscription],
        });
    }

    // ---- pinned tabs ----

    /// Opens the tabs pinned when the app last ran. Servers wait for the
    /// vault. Returns whether a local terminal was opened.
    fn restore_pinned(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let saved = pinned::load(&self.services.data_dir);
        for entry in &saved {
            match entry {
                Pinned::Local { dir } => {
                    let dir = dir.clone().filter(|dir| dir.is_dir());
                    self.add_terminal(SessionTarget::Local, dir, window, cx);
                    if let Some(tab) = self.tabs.last_mut() {
                        tab.pinned = true;
                    }
                }
                Pinned::Ssh { host } => self.waiting_pins.push(*host),
            }
        }
        self.pinned_saved = saved;
        let opened = !self.tabs.is_empty();
        if opened {
            self.activate(Some(0), window, cx);
        }
        self.open_waiting_pins(window, cx);
        opened
    }

    /// Connects the pinned servers once the vault is open and its hosts are in.
    fn open_waiting_pins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting_pins.is_empty() {
            return;
        }
        let store = Store::global(cx).read(cx);
        if !store.is_unlocked() || store.hosts.is_empty() {
            return;
        }
        // Hosts deleted since are dropped.
        let hosts: Vec<(Id, String)> = std::mem::take(&mut self.waiting_pins)
            .into_iter()
            .filter_map(|id| store.host(id).map(|host| (id, host.label.clone())))
            .collect();
        for (host_id, label) in hosts {
            self.add_terminal(SessionTarget::Ssh { host_id, label }, None, window, cx);
            if let Some(tab) = self.tabs.last_mut() {
                tab.pinned = true;
            }
        }
        self.sort_pinned();
        self.save_pinned(cx);
        cx.notify();
    }

    fn toggle_pin(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(ix) else { return };
        tab.pinned = !tab.pinned;
        self.sort_pinned();
        self.save_pinned(cx);
        self.save_open_files(cx);
        cx.notify();
    }

    /// Puts the pinned tabs first, each group keeping its order.
    fn sort_pinned(&mut self) {
        let front = self.active.and_then(|ix| self.tabs.get(ix)).map(|tab| tab.id);
        self.tabs.sort_by_key(|tab| !tab.pinned);
        self.active = front.and_then(|id| self.tabs.iter().position(|tab| tab.id == id));
    }

    fn is_pinned(&self, ix: usize) -> bool {
        self.tabs.get(ix).is_some_and(|tab| tab.pinned)
    }

    /// Writes the pinned terminals down for the next start.
    fn save_pinned(&mut self, cx: &App) {
        let mut pins: Vec<Pinned> = self
            .tabs
            .iter()
            .filter(|tab| tab.pinned)
            .filter_map(|tab| {
                let TabContent::Terminal(term) = &tab.content else { return None };
                let term = term.read(cx);
                match &term.target {
                    SessionTarget::Local => {
                        let session = term.active().read(cx).session.read(cx);
                        let dir = session.blocks.as_ref().and_then(|b| b.cwd.clone()).or_else(|| session.current_dir());
                        Some(Pinned::Local { dir })
                    }
                    SessionTarget::Ssh { host_id, .. } => Some(Pinned::Ssh { host: *host_id }),
                    SessionTarget::Replay { .. } => None,
                }
            })
            .collect();
        pins.extend(self.waiting_pins.iter().map(|host| Pinned::Ssh { host: *host }));
        if pins != self.pinned_saved {
            pinned::save(&self.services.data_dir, &pins);
            self.pinned_saved = pins;
        }
    }

    /// Shows the git changes of the repo that holds `dir`, or what `commit`
    /// changed there, in its own tab.
    fn open_changes(&mut self, dir: PathBuf, commit: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.tabs.iter().position(|tab| match &tab.content {
            TabContent::Changes(view) => view.read(cx).shows(&dir) && view.read(cx).shown_commit() == commit.as_deref(),
            TabContent::Terminal(_) | TabContent::Editor(_) => false,
        });
        if let Some(ix) = open {
            if let TabContent::Changes(view) = &self.tabs[ix].content {
                view.update(cx, |view, cx| view.reload(cx));
            }
            self.activate(Some(ix), window, cx);
            return;
        }
        let view = cx.new(|cx| match commit {
            Some(sha) => ChangesView::commit(dir, sha, cx),
            None => ChangesView::new(dir, cx),
        });
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let subscription = cx.observe(&view, |_, _, cx| cx.notify());
        self.tabs.push(Tab {
            id,
            pinned: false,
            content: TabContent::Changes(view),
            _subscriptions: vec![subscription],
        });
        self.activate(Some(self.tabs.len() - 1), window, cx);
    }

    /// Opens an empty scratch file in a new tab. It is kept in the app data
    /// folder until it is saved somewhere.
    fn new_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match scratch::create(&scratch::dir(&self.services.data_dir)) {
            Ok(path) => self.open_editor(path, FileText::Code(String::new()), true, true, window, cx),
            Err(err) => notify_error(window, cx, "Can't make a new file", err),
        }
    }

    /// Opens a file in a tab of its own. `show` brings that tab, or the one the
    /// file is open in already, to the front.
    fn open_editor(
        &mut self,
        path: PathBuf,
        text: FileText,
        scratch: bool,
        show: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.tabs.iter().position(|tab| match &tab.content {
            TabContent::Editor(pane) => pane.read(cx).holds(&path),
            TabContent::Terminal(_) | TabContent::Changes(_) => false,
        });
        if let Some(ix) = open {
            if show {
                self.activate(Some(ix), window, cx);
            }
            return;
        }
        let pane = cx.new(EditorPane::in_tab);
        pane.update(cx, |pane, cx| {
            if scratch {
                pane.add_scratch(path, text, window, cx);
            } else {
                pane.add(path, Disk::Local, text, false, None, window, cx);
            }
        });
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let subscriptions = vec![
            cx.observe(&pane, |_, _, cx| cx.notify()),
            cx.subscribe_in(&pane, window, move |workspace, _, event, window, cx| match event {
                EditorPaneEvent::Empty => workspace.remove_tab(id, window, cx),
                EditorPaneEvent::FilesChanged => workspace.save_open_files(cx),
                EditorPaneEvent::OpenCommit(dir, sha) => workspace.open_changes(dir.clone(), Some(sha.clone()), window, cx),
            }),
        ];
        self.tabs.push(Tab {
            id,
            pinned: false,
            content: TabContent::Editor(pane),
            _subscriptions: subscriptions,
        });
        if show {
            self.activate(Some(self.tabs.len() - 1), window, cx);
        } else {
            // Adding the file focused its editor.
            self.focus_current(window, cx);
            cx.notify();
        }
        self.save_open_files(cx);
    }

    /// Asks for files on this computer and opens each in its own tab.
    fn open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |workspace, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            for path in paths {
                if system_open::opens_outside(&path) {
                    let _ = workspace.update(cx, |_, cx| cx.open_with_system(&path));
                    continue;
                }
                let text = Disk::Local.run({
                    let path = path.clone();
                    move |disk| read_text(disk, &path)
                });
                let text = text.await;
                let _ = workspace.update_in(cx, |workspace, window, cx| match text {
                    Ok(text) => workspace.open_editor(path, text, false, true, window, cx),
                    Err(err) => cant_open(&path, &err, window, cx),
                });
            }
        })
        .detach();
    }

    /// Opens the editor files that were open when the app last quit, behind
    /// the tab in front. They are read off the UI thread so the terminal is not
    /// kept waiting. Files that are gone are skipped.
    fn reopen_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let data_dir = self.services.data_dir.clone();
        let files = Disk::Local.run(move |disk| {
            scratch::load_list(&data_dir)
                .into_iter()
                .filter_map(|entry| read_text(disk, &entry.path).ok().map(|text| (entry, text)))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |workspace, cx| {
            let files = files.await;
            let _ = workspace.update_in(cx, |workspace, window, cx| {
                for (entry, text) in files {
                    workspace.open_editor(entry.path, text, entry.scratch, false, window, cx);
                }
            });
        })
        .detach();
    }

    /// Remembers the open editor files, in tab order, for the next start.
    fn save_open_files(&self, cx: &App) {
        let entries: Vec<OpenEntry> = self
            .tabs
            .iter()
            .filter_map(|tab| match &tab.content {
                TabContent::Editor(pane) => Some(pane),
                TabContent::Terminal(_) | TabContent::Changes(_) => None,
            })
            .flat_map(|pane| {
                pane.read(cx).files().map(|(path, scratch)| OpenEntry { path: path.to_owned(), scratch }).collect::<Vec<_>>()
            })
            .collect();
        scratch::save_list(&self.services.data_dir, &entries);
    }

    /// The folder of the local terminal used last: the focused pane of the open
    /// tab when it is local, or else the newest local tab.
    fn previous_local_dir(&self, cx: &App) -> Option<PathBuf> {
        let open = self.active.and_then(|i| self.tabs.get(i));
        open.into_iter().chain(self.tabs.iter().rev()).find_map(|tab| {
            let TabContent::Terminal(term) = &tab.content else { return None };
            let term = term.read(cx);
            std::iter::once(term.active().clone())
                .chain(term.panes())
                .find_map(|pane| pane.read(cx).session.read(cx).current_dir())
        })
    }

    /// Saves typed text like `user@host:port` as a host, then connects to it.
    /// Returns false when the text is not an address.
    pub fn quick_connect(
        &mut self,
        text: &str,
        group_id: Option<Id>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((user, address, port)) = parse_quick_connect(text) else {
            return false;
        };
        let label = match &user {
            Some(user) => format!("{user}@{address}"),
            None => address.clone(),
        };
        let input = HostInput {
            id: None,
            label,
            address,
            port,
            group_id,
            identity_id: None,
            username: user,
            password: SecretUpdate::Keep,
            key_id: None,
            jump_host_id: None,
            startup_snippet_id: None,
            tags: Vec::new(),
            color: None,
        };
        mutate(
            window,
            cx,
            move |services| async move { services.hosts.save(input).await },
            |workspace, result, window, cx| match result {
                Ok(host) => workspace.open_terminal(
                    SessionTarget::Ssh {
                        host_id: host.id,
                        label: host.label.clone(),
                    },
                    window,
                    cx,
                ),
                Err(err) => notify_error(window, cx, "Could not connect", err),
            },
        );
        true
    }

    // ---- command palette ----

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let (context, target) = self.palette_context(window, cx);
        self.palette_target = target;
        let palette = cx.new(|cx| CommandPalette::new(context, window, cx));
        let subscription = cx.subscribe_in(&palette, window, |workspace, _, event, window, cx| match event {
            PaletteEvent::Dismiss => workspace.close_palette(window, cx),
            PaletteEvent::Pick(action) => {
                workspace.close_palette(window, cx);
                workspace.run_palette_action(action.clone(), window, cx);
            }
        });
        palette.update(cx, |palette, cx| palette.focus(window, cx));
        self.palette = Some((palette, subscription));
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.focus_current(window, cx);
        cx.notify();
    }

    fn run_palette_action(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            // Saving a host needs the vault, so a locked one asks for it first.
            PaletteAction::QuickConnect(_) if !Store::global(cx).read(cx).is_unlocked() => {
                self.show_page(Page::Hosts, window, cx)
            }
            PaletteAction::QuickConnect(text) => {
                self.quick_connect(&text, None, window, cx);
            }
            PaletteAction::Host(id) => self.connect_host(id, window, cx),
            PaletteAction::Snippet(script) => self.send_snippet(&script, true, window, cx),
            PaletteAction::NewLocalTerminal => self.open_terminal(SessionTarget::Local, window, cx),
            PaletteAction::NewEditor => self.new_editor(window, cx),
            PaletteAction::Page(page) => self.show_page(page, window, cx),
            PaletteAction::Lock => Store::global(cx).update(cx, |store, cx| store.lock(cx)),
            PaletteAction::Unlock => self.show_page(Page::Hosts, window, cx),
            PaletteAction::Tool(tool) => {
                let Some(pane) = self.palette_target.editor.take().and_then(|pane| pane.upgrade()) else { return };
                window.focus(&pane.focus_handle(cx), cx);
                pane.update(cx, |pane, cx| pane.run_tool(tool, window, cx));
            }
            PaletteAction::Search(_) => {}
            PaletteAction::Open { hit, here } => self.open_hit(hit, here, window, cx),
        }
    }

    /// The focused file editor, and the folder of the terminal or file in front.
    fn palette_context(&self, window: &Window, cx: &App) -> (PaletteContext, PaletteTarget) {
        let editor = self
            .tabs
            .iter()
            .filter_map(|tab| match &tab.content {
                TabContent::Editor(pane) => Some(pane.clone()),
                TabContent::Terminal(tab) => tab.read(cx).editor().cloned(),
                TabContent::Changes(_) => None,
            })
            .find(|pane| pane.read(cx).has_focus(window, cx));
        let mut target = PaletteTarget {
            editor: editor.as_ref().map(Entity::downgrade),
            terminal: None,
        };
        let front = self.active.and_then(|ix| self.tabs.get(ix)).filter(|_| self.section == Section::Cli);
        let here = match front.map(|tab| &tab.content) {
            Some(TabContent::Terminal(tab)) => {
                let view = tab.read(cx).active().clone();
                let session = view.read(cx).session.read(cx);
                let blocks = session.blocks.as_ref();
                let dir = blocks.and_then(|blocks| blocks.cwd.clone()).or_else(|| session.current_dir());
                dir.filter(|_| !session.is_replay()).map(|dir| {
                    let server = match &session.target {
                        SessionTarget::Ssh { label, .. } => Some(SharedString::from(label.clone())),
                        _ => None,
                    };
                    let home = if server.is_some() { blocks.and_then(|blocks| blocks.home.clone()) } else { dirs::home_dir() };
                    let here = Here {
                        dir,
                        disk: session.disk().boxed_local().shared(),
                        server,
                        home,
                    };
                    target.terminal = Some((tab.downgrade(), view.downgrade()));
                    here
                })
            }
            Some(TabContent::Editor(pane)) => pane.read(cx).folder().map(|(dir, disk)| Here {
                dir,
                disk: futures::future::ready(Some(disk)).boxed_local().shared(),
                server: None,
                home: dirs::home_dir(),
            }),
            _ => None,
        };
        (PaletteContext { editor: editor.is_some(), here }, target)
    }

    /// Opens a file or folder the palette found. One found in the terminal's
    /// folder opens through that terminal, as it may be on a server.
    fn open_hit(&mut self, hit: Hit, here: bool, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.palette_target.terminal.take().filter(|_| here);
        let terminal = terminal.and_then(|(tab, view)| Some((tab.upgrade()?, view.upgrade()?)));
        let line = hit.line.as_ref().map(|(number, _)| *number);
        match terminal {
            Some((tab, view)) if hit.folder && !view.read(cx).session.read(cx).is_local() => {
                let cd = format!("cd {}", shell_quote(&hit.path.to_string_lossy().replace('\\', "/")));
                tab.update(cx, |tab, cx| tab.send_snippet(&cd, true, cx));
            }
            Some((tab, view)) if !hit.folder => {
                tab.update(cx, |tab, cx| tab.open_file(&view, hit.path, line, window, cx));
            }
            _ if hit.folder => self.open_terminal_in(SessionTarget::Local, Some(hit.path), window, cx),
            _ => self.open_local_file(hit.path, line, window, cx),
        }
    }

    /// Opens what Somme was asked to open from outside it: a folder in a new
    /// terminal there, a file in the editor. Files are never handed back to
    /// the system, which may send them here again.
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            if path.is_dir() {
                self.open_terminal_in(SessionTarget::Local, Some(path), window, cx);
            } else {
                self.edit_local_file(path, None, window, cx);
            }
        }
    }

    /// Opens a file on this computer in a tab of its own, with the cursor on
    /// `line`. Images, PDFs and the like open in the system's app.
    fn open_local_file(&mut self, path: PathBuf, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        if system_open::opens_outside(&path) {
            cx.open_with_system(&path);
            return;
        }
        self.edit_local_file(path, line, window, cx);
    }

    /// Opens a file on this computer in the editor, or says why it can't.
    fn edit_local_file(&mut self, path: PathBuf, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_local_file(&path, line, window, cx) {
            return;
        }
        let text = Disk::Local.run({
            let path = path.clone();
            move |disk| read_text(disk, &path)
        });
        cx.spawn_in(window, async move |workspace, cx| {
            let text = text.await;
            let _ = workspace.update_in(cx, |workspace, window, cx| match text {
                Ok(text) => {
                    workspace.open_editor(path.clone(), text, false, true, window, cx);
                    workspace.show_local_file(&path, line, window, cx);
                }
                Err(err) => cant_open(&path, &err, window, cx),
            });
        })
        .detach();
    }

    /// Brings the tab holding `path` to the front, with the cursor on `line`.
    /// False when no tab holds it.
    fn show_local_file(&mut self, path: &Path, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let found = self.tabs.iter().position(|tab| matches!(&tab.content, TabContent::Editor(pane) if pane.read(cx).holds(path)));
        let Some(ix) = found else { return false };
        self.activate(Some(ix), window, cx);
        if let TabContent::Editor(pane) = &self.tabs[ix].content {
            pane.update(cx, |pane, cx| {
                pane.show_open(path, &Disk::Local, window, cx);
                if let Some(line) = line {
                    pane.go_to_line(line, window, cx);
                }
            });
        }
        true
    }

    // ---- side panel ----

    /// The panel only makes sense next to a live terminal, not a replayed log.
    fn side_panel_fits(&self, cx: &App) -> bool {
        self.active_terminal()
            .is_some_and(|tab| !matches!(tab.read(cx).target, SessionTarget::Replay { .. }))
    }

    fn toggle_side_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.side_panel_fits(cx) {
            return;
        }
        Settings::update(cx, |s| s.side_panel = !s.side_panel);
        self.focus_current(window, cx);
    }

    /// Opens the side panel on the session logs of the terminal in front.
    fn show_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.side_panel_fits(cx) {
            return;
        }
        if !Settings::get(cx).side_panel {
            Settings::update(cx, |s| s.side_panel = true);
        }
        let panel = self.side_panel(window, cx);
        panel.update(cx, |panel, cx| panel.show_logs(cx));
    }

    fn side_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<SidePanel> {
        if let Some((panel, _)) = &self.side_panel {
            return panel.clone();
        }
        let panel = cx.new(|cx| SidePanel::new(window, cx));
        let subscription = cx.subscribe_in(&panel, window, |workspace, _, event, window, cx| match event {
            SidePanelEvent::Send { script, run } => workspace.send_snippet(script, *run, window, cx),
            SidePanelEvent::ManageSnippets => workspace.show_page(Page::Snippets, window, cx),
            SidePanelEvent::OpenLog(entry) => {
                let target = SessionTarget::Replay {
                    label: entry.label.clone(),
                    file: entry.file.clone(),
                };
                workspace.open_terminal(target, window, cx);
            }
            SidePanelEvent::FocusTerminal => workspace.focus_current(window, cx),
            SidePanelEvent::Close => workspace.toggle_side_panel(window, cx),
        });
        self.side_panel = Some((panel.clone(), subscription));
        panel
    }

    pub fn connect_host(&mut self, host_id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = Store::global(cx).read(cx).host(host_id).cloned() else { return };
        let has_user = host.username.as_deref().is_some_and(|u| !u.trim().is_empty()) || host.identity_id.is_some();
        if !has_user {
            self.ask_username(host, window, cx);
            return;
        }
        self.open_terminal(
            SessionTarget::Ssh {
                host_id,
                label: host.label.clone(),
            },
            window,
            cx,
        );
    }

    /// The host has no user name yet: ask for one, save it, then connect.
    fn ask_username(&mut self, host: HostView, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("root"));
        let workspace = cx.entity().downgrade();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            let submit = {
                let input = input.clone();
                let workspace = workspace.clone();
                let host = host.clone();
                move |window: &mut Window, cx: &mut App| -> bool {
                    let username = input.read(cx).value().trim().to_owned();
                    if username.is_empty() {
                        return false;
                    }
                    let mut save = host_input(&host);
                    save.username = Some(username);
                    let _ = workspace.update(cx, |_, cx| {
                        mutate(
                            window,
                            cx,
                            move |services| async move { services.hosts.save(save).await },
                            |workspace, result, window, cx| match result {
                                Ok(host) => workspace.open_terminal(
                                    SessionTarget::Ssh {
                                        host_id: host.id,
                                        label: host.label,
                                    },
                                    window,
                                    cx,
                                ),
                                Err(err) => notify_error(window, cx, "Could not save user name", err),
                            },
                        );
                    });
                    true
                }
            };
            let on_ok = submit.clone();
            dialog
                .title(format!("Connect to {}", host.label))
                .w(px(400.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().child("User name"))
                        .child(Input::new(&input)),
                )
                .on_ok(move |_, window, cx| on_ok(window, cx))
                .footer(
                    DialogFooter::new()
                        .child(Button::new("user-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)))
                        .child(Button::new("user-ok").primary().label("Connect").on_click(move |_, window, cx| {
                            if submit(window, cx) {
                                window.close_dialog(cx);
                            }
                        })),
                )
        });
    }

    /// Sends a snippet to the terminal used last and shows it.
    pub fn send_snippet(&mut self, script: &str, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let is_terminal = |ix: &usize| matches!(self.tabs[*ix].content, TabContent::Terminal(_));
        let ix = self
            .active
            .filter(is_terminal)
            .or_else(|| self.tabs.iter().position(|t| Some(t.id) == self.last_terminal))
            .or_else(|| (0..self.tabs.len()).rev().find(is_terminal));
        let Some(ix) = ix else {
            notify_error(window, cx, "No terminal open", "Connect to a host or open a local terminal first.");
            return;
        };
        self.activate(Some(ix), window, cx);
        if let TabContent::Terminal(tab) = &self.tabs[ix].content {
            tab.update(cx, |tab, cx| tab.send_snippet(script, run, cx));
        }
    }

    /// Opens a new tab for each host and runs the snippet once it connects.
    pub fn run_on_hosts(&mut self, hosts: Vec<Id>, script: String, window: &mut Window, cx: &mut Context<Self>) {
        for host_id in hosts {
            let Some(label) = Store::global(cx).read(cx).host(host_id).map(|h| h.label.clone()) else {
                continue;
            };
            self.open_terminal(SessionTarget::Ssh { host_id, label }, window, cx);
            if let Some(Tab {
                content: TabContent::Terminal(tab),
                ..
            }) = self.tabs.last()
            {
                let script = script.clone();
                tab.update(cx, |tab, cx| tab.run_when_connected(script, cx));
            }
        }
    }

    /// Shows the CLI section with the tab at `index` in front.
    fn activate(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index.filter(|ix| *ix < self.tabs.len());
        self.section = Section::Cli;
        if let Some(ix) = self.active {
            self.tab_scroll.scroll_to_item(ix);
            if let TabContent::Terminal(_) = self.tabs[ix].content {
                self.last_terminal = Some(self.tabs[ix].id);
            }
            if !self.is_ssh_tab(ix, cx) {
                self.last_local = Some(self.tabs[ix].id);
            }
        }
        self.focus_current(window, cx);
        cx.notify();
    }

    fn is_ssh_tab(&self, ix: usize, cx: &App) -> bool {
        matches!(
            &self.tabs[ix].content,
            TabContent::Terminal(term) if matches!(term.read(cx).target, SessionTarget::Ssh { .. })
        )
    }

    /// The rail icon that lights up. An SSH tab in front counts as SSH.
    fn rail_section(&self, cx: &App) -> Section {
        match self.section {
            Section::Cli if self.active.is_some_and(|ix| self.is_ssh_tab(ix, cx)) => Section::Ssh,
            section => section,
        }
    }

    /// Like pressing ⌘1 to ⌘8: a number past the last tab does nothing.
    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.activate(Some(ix), window, cx);
        }
    }

    fn show_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if section == Section::Cli {
            // The CLI icon goes to a tab on this computer, never an SSH one.
            let local = |ix: &usize| !self.is_ssh_tab(*ix, cx);
            let ix = self
                .active
                .filter(local)
                .or_else(|| self.tabs.iter().position(|t| Some(t.id) == self.last_local))
                .or_else(|| (0..self.tabs.len()).rev().find(local));
            match ix {
                Some(ix) => self.activate(Some(ix), window, cx),
                None => self.open_terminal(SessionTarget::Local, window, cx),
            }
            return;
        }
        self.section = section;
        if section == Section::Ssh {
            Store::global(cx).update(cx, |store, cx| store.open_vault(cx));
            self.reload_page(cx);
        }
        self.focus_current(window, cx);
        cx.notify();
    }

    fn remove_tab(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t.id == id) else { return };
        let tab = self.tabs.remove(ix);
        if tab.pinned {
            self.save_pinned(cx);
        }
        if let TabContent::Editor(pane) = tab.content {
            pane.update(cx, |pane, _| pane.discard());
            self.save_open_files(cx);
        }
        let next = match self.active {
            Some(active) if active == ix => {
                if self.tabs.is_empty() {
                    None
                } else {
                    Some(ix.min(self.tabs.len() - 1))
                }
            }
            Some(active) if active > ix => Some(active - 1),
            other => other,
        };
        // A tab that ends behind the vault or settings leaves them in front.
        if self.section == Section::Cli {
            self.activate(next, window, cx);
        } else {
            self.active = next;
            cx.notify();
        }
    }

    /// Moves the tab with `id` to position `to`, keeping the same tab in front.
    fn move_tab(&mut self, id: usize, to: usize, cx: &mut Context<Self>) {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else { return };
        // Pinned tabs stay ahead of the others.
        let pinned = self.tabs.iter().filter(|t| t.pinned).count();
        let to = if self.tabs[from].pinned { to.min(pinned.saturating_sub(1)) } else { to.max(pinned) };
        let to = to.min(self.tabs.len() - 1);
        if from == to {
            return;
        }
        let active_id = self.active.map(|ix| self.tabs[ix].id);
        let tab = self.tabs.remove(from);
        let editor = matches!(tab.content, TabContent::Editor(_));
        self.tabs.insert(to, tab);
        self.active = active_id.and_then(|id| self.tabs.iter().position(|t| t.id == id));
        if editor {
            self.save_open_files(cx);
        }
        cx.notify();
    }

    /// Puts a name box in place of the tab's title.
    fn start_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let TabContent::Terminal(term) = &tab.content else { return };
        let (tab_id, title) = (tab.id, term.read(cx).title(cx));
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, |workspace, _, event, window, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => workspace.finish_rename(true, window, cx),
            _ => {}
        });
        self.renaming = Some(Renaming {
            tab_id,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Saves or drops the typed name. A blank name gives the tab its automatic name back.
    fn finish_rename(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else { return };
        if save
            && let Some(tab) = self.tabs.iter().find(|t| t.id == renaming.tab_id)
            && let TabContent::Terminal(term) = &tab.content
        {
            let name = renaming.input.read(cx).value();
            term.update(cx, |term, cx| term.rename(&name, cx));
        }
        self.focus_current(window, cx);
        cx.notify();
    }

    fn tab_is_running(&self, ix: usize, cx: &App) -> bool {
        match &self.tabs[ix].content {
            TabContent::Terminal(tab) => tab.read(cx).is_running(cx),
            TabContent::Changes(_) | TabContent::Editor(_) => false,
        }
    }

    fn has_unsaved(tab: &Tab, cx: &App) -> bool {
        match &tab.content {
            TabContent::Terminal(term) => term.read(cx).has_unsaved(cx),
            TabContent::Editor(pane) => pane.read(cx).has_changes(),
            TabContent::Changes(_) => false,
        }
    }

    /// Whether any tab has files with unsaved changes.
    fn any_unsaved(&self, cx: &App) -> bool {
        self.tabs.iter().any(|tab| Self::has_unsaved(tab, cx))
    }

    /// Closes the tabs `which` picks around the tab at `ix`, without asking.
    /// Pinned tabs stay, and so do tabs with unsaved files, so no typing is lost.
    fn close_tabs(&mut self, ix: usize, which: CloseTabs, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clicked) = self.tabs.get(ix).map(|tab| tab.id) else { return };
        let front = self.active.and_then(|active| self.tabs.get(active)).map(|tab| tab.id);
        let ids: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(other, tab)| which.picks(*other, ix) && !tab.pinned && !Self::has_unsaved(tab, cx))
            .map(|(_, tab)| tab.id)
            .collect();
        for id in ids {
            self.remove_tab(id, window, cx);
        }
        // The tab in front stays there. When it closed, the clicked one comes forward.
        let next = [front, Some(clicked)]
            .into_iter()
            .flatten()
            .find_map(|id| self.tabs.iter().position(|tab| tab.id == id));
        if next.is_some() && self.section == Section::Cli {
            self.activate(next, window, cx);
        }
    }

    /// Closes a tab, asking first when sessions in it are still connected.
    fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let unsaved = Self::has_unsaved(tab, cx);
        let id = tab.id;
        let running = Settings::get(cx).confirm_close_tab && self.tab_is_running(ix, cx);
        if !unsaved && !running {
            self.remove_tab(id, window, cx);
            return;
        }
        let detail = if unsaved {
            "Files open in this tab have unsaved changes. They will be lost."
        } else {
            "The sessions in this tab are still connected. They will be disconnected."
        };
        let workspace = cx.entity().downgrade();
        native::confirm(
            "Close this tab?",
            detail,
            "Close",
            move |window, cx| {
                let _ = workspace.update(cx, |workspace, cx| workspace.remove_tab(id, window, cx));
            },
            window,
            cx,
        );
    }

    /// The terminal tab in front, when the CLI section shows.
    fn active_terminal(&self) -> Option<Entity<TerminalTab>> {
        if self.section != Section::Cli {
            return None;
        }
        match &self.tabs.get(self.active?)?.content {
            TabContent::Terminal(tab) => Some(tab.clone()),
            TabContent::Changes(_) | TabContent::Editor(_) => None,
        }
    }

    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if count == 0 {
            return;
        }
        // From the vault or settings, the first step lands on the first or last tab.
        let next = match self.active.filter(|_| self.section == Section::Cli) {
            Some(ix) if forward => (ix + 1) % count,
            Some(ix) => (ix + count - 1) % count,
            None if forward => 0,
            None => count - 1,
        };
        self.activate(Some(next), window, cx);
    }

    /// Shows a vault page in the SSH section, or the settings.
    pub fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if page == Page::Settings {
            self.show_section(Section::Settings, window, cx);
            return;
        }
        self.page = page;
        self.show_section(Section::Ssh, window, cx);
    }

    /// Opens the SFTP screen with `host_id` on the right side.
    pub fn open_sftp(&mut self, host_id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.sftp
            .get_or_insert_with(|| cx.new(|cx| SftpPage::new(window, cx)))
            .update(cx, |page, cx| page.open_host(host_id, window, cx));
        self.show_page(Page::Sftp, window, cx);
    }

    /// The account picture shows the user's GitHub profile, or the account
    /// page when nobody is signed in.
    fn show_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = Store::global(cx).read(cx).account.login.clone().or_else(fake_login) else {
            return self.show_account(window, cx);
        };
        let workspace = cx.entity().downgrade();
        let on_account = move |window: &mut Window, cx: &mut App| {
            let _ = workspace.update(cx, |workspace, cx| workspace.show_account(window, cx));
        };
        profile::show(login, &self.services, on_account, window, cx);
    }

    fn show_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings
            .get_or_insert_with(|| cx.new(SettingsPage::new))
            .update(cx, |page, cx| page.show_account(cx));
        self.show_page(Page::Settings, window, cx);
    }

    /// Some pages show data that changes outside the store while a terminal is in front.
    fn reload_page(&mut self, cx: &mut Context<Self>) {
        match self.page {
            // Host keys are saved by the backend during connect, without a store refresh.
            Page::KnownHosts => Store::global(cx).update(cx, |store, cx| store.refresh(cx)),
            _ => {}
        }
    }

    // ---- actions ----

    fn on_new_local(&mut self, _: &NewLocalTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.open_terminal(SessionTarget::Local, window, cx);
    }

    fn on_close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_terminal() {
            Some(tab) if tab.read(cx).panes().len() > 1 => tab.update(cx, |tab, cx| tab.close_active(window, cx)),
            _ => self.on_close_tab(&CloseTab, window, cx),
        }
    }

    /// Closes the tab in front, or the window when no tab is.
    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        match self.active.filter(|_| self.section == Section::Cli) {
            // A pinned tab closes only from its menu.
            Some(ix) if self.is_pinned(ix) => {}
            Some(ix) => self.request_close_tab(ix, window, cx),
            None => window.remove_window(),
        }
    }

    /// Splits the focused pane. `before` puts the new pane on the left or on top.
    fn on_split(&mut self, axis: Axis, before: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_terminal() {
            tab.update(cx, |tab, cx| tab.split(axis, before, window, cx));
        }
    }

    fn on_lock(&mut self, _: &LockVault, _: &mut Window, cx: &mut Context<Self>) {
        Store::global(cx).update(cx, |store, cx| store.lock(cx));
    }

    // ---- rendering ----

    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // Tabs sit on a light wash of the text colour, so they stand out on any theme.
        let (tab_bg, tab_hover, tab_active) = (
            theme.foreground.opacity(0.05),
            theme.foreground.opacity(0.09),
            theme.foreground.opacity(0.14),
        );
        let workspace = cx.entity().downgrade();
        // Which tabs the close items in a tab's menu would reach.
        let closable: Vec<bool> = self.tabs.iter().map(|tab| !tab.pinned).collect();
        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let (left, right) = (closable[..ix].contains(&true), closable[ix + 1..].contains(&true));
            let selected = self.section == Section::Cli && self.active == Some(ix);
            // Only terminal tabs can be renamed.
            let changes = !matches!(tab.content, TabContent::Terminal(_));
            let icon = match &tab.content {
                TabContent::Changes(_) => Some(Lucide::FileDiff),
                TabContent::Editor(_) => Some(Lucide::FileText),
                TabContent::Terminal(_) => None,
            };
            let unsaved = matches!(&tab.content, TabContent::Editor(pane) if pane.read(cx).has_changes());
            let (title, color, broadcast, running, renamed) = match &tab.content {
                TabContent::Terminal(term) => {
                    let term = term.read(cx);
                    let color = match &term.target {
                        SessionTarget::Ssh { host_id, .. } => Store::global(cx)
                            .read(cx)
                            .host(*host_id)
                            .and_then(|h| parse_color(h.color.as_deref())),
                        SessionTarget::Local | SessionTarget::Replay { .. } => None,
                    };
                    (term.title(cx), color, term.broadcast, term.is_running(cx), term.custom_title.is_some())
                }
                TabContent::Changes(view) => (view.read(cx).title(), None, false, false, false),
                TabContent::Editor(pane) => (pane.read(cx).title(), None, false, false, false),
            };
            let dot = color.unwrap_or(if running { theme.success } else { theme.muted_foreground });
            let editing = self.renaming.as_ref().filter(|r| r.tab_id == tab.id).map(|r| r.input.clone());
            let label = match editing {
                Some(input) => div()
                    .flex_1()
                    .min_w_0()
                    .capture_action(cx.listener(|workspace, _: &Escape, window, cx| {
                        workspace.finish_rename(false, window, cx);
                    }))
                    .child(Input::new(&input).xsmall().appearance(false))
                    .into_any_element(),
                None => div().flex_1().min_w_0().truncate().child(title.clone()).into_any_element(),
            };
            let dragged = DraggedTab { id: tab.id, title: title.clone() };
            let tab_id = tab.id;
            let pinned = tab.pinned;
            let menu_workspace = workspace.clone();
            div()
                .id(("tab", tab.id))
                .group("tab")
                .h(px(26.))
                .min_w(px(if pinned { 0. } else { 110. }))
                .max_w(px(if pinned { 160. } else { 220. }))
                // Short already; when tabs run out of room, the others give way.
                .when(pinned, |el| el.flex_none())
                .pl_2()
                .pr(px(if pinned { 8. } else { 4. }))
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .border_1()
                .text_sm()
                .cursor_pointer()
                .when(selected, |el| {
                    el.bg(tab_active).border_color(theme.border).text_color(theme.foreground)
                })
                .when(!selected, |el| {
                    el.bg(tab_bg)
                        .border_color(theme.transparent)
                        .text_color(theme.tab_foreground)
                        .hover(|el| el.bg(tab_hover))
                })
                .when_some(icon, |el, icon| el.child(Icon::new(icon).size(px(13.))))
                .when(icon.is_none(), |el| el.child(div().flex_none().size(px(7.)).rounded_full().bg(dot)))
                .when(broadcast, |el| {
                    el.child(Icon::new(Lucide::RadioTower).size(px(12.)).text_color(theme.warning))
                })
                .child(label)
                .when(unsaved, |el| {
                    el.child(div().flex_none().size(px(7.)).rounded_full().bg(theme.foreground.opacity(0.7)))
                })
                .when(pinned, |el| {
                    el.child(Icon::new(Lucide::Pin).size(px(11.)).text_color(theme.muted_foreground))
                })
                .when(!pinned, |el| {
                    el.child(
                        Button::new(("close-tab", tab.id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tab_stop(false)
                            .on_click(cx.listener(move |workspace, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                workspace.request_close_tab(ix, window, cx);
                            })),
                    )
                })
                .on_click(cx.listener(move |workspace, event: &ClickEvent, window, cx| {
                    if event.click_count() == 2 && !changes {
                        workspace.start_rename(ix, window, cx);
                    } else if workspace.renaming.as_ref().is_none_or(|r| r.tab_id != tab_id) {
                        workspace.activate(Some(ix), window, cx);
                    }
                }))
                // Keeps a press here from moving the window, as the title bar behind would.
                .block_mouse_except_scroll()
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |workspace, _, window, cx| {
                        if !workspace.is_pinned(ix) {
                            workspace.request_close_tab(ix, window, cx);
                        }
                    }),
                )
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                .drag_over::<DraggedTab>(|style, _, _, cx| style.border_color(cx.theme().primary))
                .on_drop(cx.listener(move |workspace, dragged: &DraggedTab, _, cx| {
                    workspace.move_tab(dragged.id, ix, cx);
                }))
                .context_menu(move |menu, _, _| {
                    let (rename, reset, close) = (menu_workspace.clone(), menu_workspace.clone(), menu_workspace.clone());
                    let pin = menu_workspace.clone();
                    let close_item = |label: &'static str, which: CloseTabs, enabled: bool| {
                        let workspace = menu_workspace.clone();
                        PopupMenuItem::new(label).disabled(!enabled).on_click(move |_, window, cx| {
                            let _ = workspace.update(cx, |workspace, cx| workspace.close_tabs(ix, which, window, cx));
                        })
                    };
                    menu.when(!changes, |menu| {
                        menu.item(PopupMenuItem::new("Rename…").icon(Icon::new(Lucide::Pencil)).on_click(
                            move |_, window, cx| {
                                let _ = rename.update(cx, |workspace, cx| workspace.start_rename(ix, window, cx));
                            },
                        ))
                    })
                    .when(renamed, |menu| {
                        menu.item(PopupMenuItem::new("Use Automatic Name").icon(Icon::new(Lucide::RotateCcw)).on_click(
                            move |_, _, cx| {
                                let _ = reset.update(cx, |workspace, cx| {
                                    if let Some(TabContent::Terminal(term)) = workspace.tabs.get(ix).map(|t| &t.content) {
                                        term.update(cx, |term, cx| term.rename("", cx));
                                    }
                                });
                            },
                        ))
                    })
                    .when(!changes, |menu| menu.separator())
                    .item(
                        PopupMenuItem::new(if pinned { "Unpin Tab" } else { "Pin Tab" })
                            .icon(Icon::new(if pinned { Lucide::PinOff } else { Lucide::Pin }))
                            .on_click(move |_, _, cx| {
                                let _ = pin.update(cx, |workspace, cx| workspace.toggle_pin(ix, cx));
                            }),
                    )
                    .separator()
                    .item(PopupMenuItem::new("Close Tab").icon(IconName::Close).on_click(move |_, window, cx| {
                        let _ = close.update(cx, |workspace, cx| workspace.request_close_tab(ix, window, cx));
                    }))
                    .item(close_item("Close Other Tabs", CloseTabs::Others, left || right))
                    .item(close_item("Close Tabs to the Left", CloseTabs::Left, left))
                    .item(close_item("Close Tabs to the Right", CloseTabs::Right, right))
                    .item(close_item("Close All Tabs", CloseTabs::All, left || right || !pinned))
                })
        });

        let new_tab = Button::new("new-tab")
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .tooltip("New tab")
            .dropdown_menu(|menu, _, _| {
                menu.menu_with_icon("Local Terminal", Icon::new(Lucide::SquareTerminal), Box::new(NewLocalTerminal))
                    .menu_with_icon("Text Editor", Icon::new(Lucide::FilePen), Box::new(NewEditor))
                    .menu_with_icon("Open File…", Icon::new(Lucide::FolderOpen), Box::new(OpenFile))
                    .separator()
                    .menu_with_icon("Split Right", Icon::new(Lucide::Columns2), Box::new(SplitRight))
                    .menu_with_icon("Split Left", Icon::new(Lucide::Columns2), Box::new(SplitLeft))
                    .menu_with_icon("Split Down", Icon::new(Lucide::Rows2), Box::new(SplitDown))
                    .menu_with_icon("Split Up", Icon::new(Lucide::Rows2), Box::new(SplitUp))
                    .separator()
                    .menu_with_icon("Broadcast Input", Icon::new(Lucide::RadioTower), Box::new(ToggleBroadcast))
            });

        // The tabs scroll; the new tab button stays in sight after them.
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .id("tab-strip")
                    .min_w_0()
                    .gap_1()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs),
            )
            .child(new_tab)
    }

    fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unlocked = Store::global(cx).read(cx).is_unlocked();
        // Pull requests and issues show for a signed-in user whose shell is in a GitHub repository.
        let github = self.github_repo(cx).zip(self.active_terminal()).map(|(repo, tab)| {
            let shown = tab.read(cx).github_kind(cx);
            (tab, Counts::summary(&repo, cx), repo, shown)
        });
        let badge = cx.theme().blue;
        // The commit graph shows for a local shell in any git repository.
        let graph = self
            .active_terminal()
            .filter(|tab| tab.read(cx).git_dir(cx).is_some())
            .map(|tab| (tab.read(cx).graph_shown(), tab));
        TitleBar::new()
            // The kit keeps room for the window buttons, which full screen hides.
            .when(window.is_fullscreen(), |bar| bar.pl_0())
            .child(
                // The kit's bar grows to fit what is in it, so many tabs would
                // push the buttons out of the window. Laid over the room the
                // bar has instead, the tabs scroll and the buttons stay.
                div().flex_1().h_full().relative().child(
                h_flex()
                    .absolute()
                    .inset_0()
                    .pr_2()
                    .gap_2()
                    .child(self.render_tab_strip(cx))
                    .when_some(github, |bar, (tab, summary, repo, shown)| {
                        let issues_tab = tab.clone();
                        let name = summary.as_ref().map_or_else(|| repo.full_name(), |summary| summary.repo.full_name());
                        let (pulls, issues) = summary.map_or((None, None), |summary| (Some(summary.pulls), Some(summary.issues)));
                        let tip = |count: Option<u64>, what: &str| match count {
                            Some(count) => format!("{count} open {what} in {name}"),
                            None => format!("Open {what} in {name}"),
                        };
                        bar.child(
                            Badge::new().count(pulls.unwrap_or_default() as usize).color(badge).child(
                                Button::new("github-pulls")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(Lucide::GitPullRequest))
                                    .selected(shown == Some(Kind::Pulls))
                                    .tooltip(tip(pulls, "pull requests"))
                                    .on_click(move |_, window, cx| {
                                        tab.update(cx, |tab, cx| tab.toggle_github(Kind::Pulls, window, cx));
                                    }),
                            ),
                        )
                        .child(
                            Badge::new().count(issues.unwrap_or_default() as usize).color(badge).child(
                                Button::new("github-issues")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(Lucide::CircleDot))
                                    .selected(shown == Some(Kind::Issues))
                                    .tooltip(tip(issues, "issues"))
                                    .on_click(move |_, window, cx| {
                                        issues_tab.update(cx, |tab, cx| tab.toggle_github(Kind::Issues, window, cx));
                                    }),
                            ),
                        )
                    })
                    .when_some(graph, |bar, (shown, tab)| {
                        bar.child(
                            Button::new("git-graph")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(Lucide::GitGraph))
                                .selected(shown)
                                .tooltip("Commit graph")
                                .on_click(move |_, window, cx| tab.update(cx, |tab, cx| tab.toggle_graph(window, cx))),
                        )
                    })
                    .when(self.side_panel_fits(cx), |bar| {
                        bar.child(
                            Button::new("side-panel")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(Lucide::PanelRight))
                                .selected(Settings::get(cx).side_panel)
                                .tooltip("Snippets and themes (⌘⇧S)")
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.toggle_side_panel(window, cx))),
                        )
                    })
                    .child(
                        Button::new("palette")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Search)
                            .tooltip("Search and connect (⌘P)")
                            .on_click(cx.listener(|workspace, _, window, cx| workspace.toggle_palette(window, cx))),
                    )
                    // Locking is part of the vault, so the button only shows there.
                    .when(unlocked && self.section == Section::Ssh, |bar| {
                        bar.child(
                            Button::new("lock")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(Lucide::Lock))
                                .tooltip("Lock saved servers and keys (⌘⇧L)")
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.on_lock(&LockVault, window, cx))),
                        )
                    }),
                ),
            )
    }

    /// The thin strip on the left: CLI, SSH and settings, with the account at the bottom.
    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let lit = self.rail_section(cx);
        let item = |id: &'static str, icon: Lucide, section: Section, name: &'static str, action: Box<dyn gpui_kit::Action>| {
            let current = lit == section;
            div()
                .id(id)
                .relative()
                .size(px(34.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .cursor_pointer()
                .when(current, |el| el.bg(theme.foreground.opacity(0.1)))
                .when(!current, |el| el.opacity(0.7).hover(|el| el.opacity(1.).bg(theme.foreground.opacity(0.05))))
                // A bar on the outer edge marks where you are.
                .when(current, |el| {
                    el.child(div().absolute().left(px(-5.)).top(px(8.)).w(px(3.)).h(px(18.)).rounded_r(px(2.)).bg(theme.primary))
                })
                .child(Icon::new(icon).size(px(18.)).text_color(theme.foreground))
                .tooltip(move |window, cx| Tooltip::new(name).action(action.as_ref(), None).build(window, cx))
                .on_click(cx.listener(move |workspace, _, window, cx| workspace.show_section(section, window, cx)))
        };

        let store = Store::global(cx).read(cx);
        let handle: SharedString = match &store.account.login {
            Some(login) => format!("@{login}").into(),
            None => "Sign in with GitHub".into(),
        };
        let account = div()
            .id("rail-account")
            .size(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_lg()
            .cursor_pointer()
            .hover(|el| el.bg(theme.foreground.opacity(0.05)))
            .child(avatar(store.avatar.clone(), px(22.), cx))
            .tooltip(move |window, cx| Tooltip::new(handle.clone()).build(window, cx))
            .on_click(cx.listener(|workspace, _, window, cx| workspace.show_profile(window, cx)));
        let workspace = cx.entity().downgrade();
        let update_button = update::rail_button(
            move |cx| workspace.upgrade().is_some_and(|workspace| workspace.read(cx).any_unsaved(cx)),
            cx,
        );

        v_flex()
            .flex_none()
            .w(px(44.))
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(item("rail-cli", Lucide::SquareTerminal, Section::Cli, "Terminal", Box::new(NewLocalTerminal)))
            .child(item("rail-ssh", Lucide::Server, Section::Ssh, "SSH", Box::new(ShowHome)))
            .child(div().flex_1())
            .child(account)
            .children(update_button)
            .child(item("rail-settings", Lucide::Settings, Section::Settings, "Settings", Box::new(OpenSettings)))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = Page::VAULT.into_iter().map(|page| {
            SidebarMenuItem::new(page.label())
                .icon(page.icon())
                .active(self.page == page)
                .on_click(cx.listener(move |workspace, _, window, cx| workspace.show_page(page, window, cx)))
        });
        Sidebar::new("nav")
            .collapsible(false)
            .child(SidebarGroup::new("SSH").child(SidebarMenu::new().children(items)))
    }

    /// Shown in the CLI section once every tab is closed.
    fn render_no_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(Icon::new(Lucide::SquareTerminal).size(px(40.)).text_color(cx.theme().muted_foreground))
            .child(
                Button::new("no-tabs-new")
                    .primary()
                    .label("New Terminal")
                    .tooltip("⌘T")
                    .on_click(cx.listener(|workspace, _, window, cx| {
                        workspace.open_terminal(SessionTarget::Local, window, cx)
                    })),
            )
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.section == Section::Settings {
            return self.settings.get_or_insert_with(|| cx.new(SettingsPage::new)).clone().into_any_element();
        }
        match self.page {
            Page::Hosts => {
                let workspace = cx.entity().downgrade();
                let page = self
                    .hosts
                    .get_or_insert_with(|| cx.new(|cx| HostsPage::new(workspace, window, cx)))
                    .clone();
                page.into_any_element()
            }
            Page::Keychain => self
                .keychain
                .get_or_insert_with(|| cx.new(|cx| KeychainPage::new(window, cx)))
                .clone()
                .into_any_element(),
            Page::Forwarding => self
                .forwards
                .get_or_insert_with(|| cx.new(ForwardsPage::new))
                .clone()
                .into_any_element(),
            Page::Snippets => {
                let workspace = cx.entity().downgrade();
                self.snippets
                    .get_or_insert_with(|| cx.new(|cx| SnippetsPage::new(workspace, window, cx)))
                    .clone()
                    .into_any_element()
            }
            Page::KnownHosts => self
                .known_hosts
                .get_or_insert_with(|| cx.new(|cx| KnownHostsPage::new(window, cx)))
                .clone()
                .into_any_element(),
            Page::Sftp => self
                .sftp
                .get_or_insert_with(|| cx.new(|cx| SftpPage::new(window, cx)))
                .clone()
                .into_any_element(),
            Page::Settings => self
                .settings
                .get_or_insert_with(|| cx.new(SettingsPage::new))
                .clone()
                .into_any_element(),
        }
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unlocked = Store::global(cx).read(cx).is_unlocked();
        let theme = cx.theme().clone();
        let tab = self.active.and_then(|ix| self.tabs.get(ix)).filter(|_| self.section == Section::Cli);
        let body: AnyElement = if self.section == Section::Ssh && !unlocked {
            self.lock_screen.clone().into_any_element()
        } else if self.section == Section::Settings {
            self.render_page(window, cx)
        } else if self.section == Section::Cli && tab.is_none() {
            self.render_no_tabs(cx).into_any_element()
        } else if let Some(Tab {
            content: TabContent::Changes(view),
            ..
        }) = tab
        {
            view.clone().into_any_element()
        } else if let Some(Tab {
            content: TabContent::Editor(pane),
            ..
        }) = tab
        {
            pane.clone().into_any_element()
        } else if let Some(Tab {
            content: TabContent::Terminal(term),
            ..
        }) = tab
        {
            let term = term.clone();
            if Settings::get(cx).side_panel && self.side_panel_fits(cx) {
                let log_host = match &term.read(cx).target {
                    SessionTarget::Ssh { host_id, .. } => Some(host_id.to_string()),
                    SessionTarget::Local | SessionTarget::Replay { .. } => None,
                };
                let panel = self.side_panel(window, cx);
                panel.update(cx, |panel, cx| panel.set_log_host(log_host, cx));
                h_flex()
                    .size_full()
                    .child(div().flex_1().min_w_0().h_full().child(term))
                    .child(panel)
                    .into_any_element()
            } else {
                term.into_any_element()
            }
        } else {
            h_flex()
                .size_full()
                .child(self.render_sidebar(cx))
                .child(v_flex().flex_1().min_w_0().h_full().child(self.render_page(window, cx)))
                .into_any_element()
        };

        v_flex()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .capture_key_down(cx.listener(|w, _, _, _| w.touch()))
            .capture_any_mouse_down(cx.listener(|w, _, _, _| w.touch()))
            .on_scroll_wheel(cx.listener(|w, _, _, _| w.touch()))
            .on_action(cx.listener(Self::on_new_local))
            .on_action(cx.listener(|w, _: &NewEditor, window, cx| w.new_editor(window, cx)))
            .on_action(cx.listener(|w, _: &OpenFile, window, cx| w.open_files(window, cx)))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_lock))
            .on_action(cx.listener(|w, _: &NextTab, window, cx| w.cycle_tab(true, window, cx)))
            .on_action(cx.listener(|w, _: &PreviousTab, window, cx| w.cycle_tab(false, window, cx)))
            .on_action(cx.listener(|w, _: &ShowHome, window, cx| w.show_section(Section::Ssh, window, cx)))
            .on_action(cx.listener(|w, _: &SplitRight, window, cx| w.on_split(Axis::Horizontal, false, window, cx)))
            .on_action(cx.listener(|w, _: &SplitLeft, window, cx| w.on_split(Axis::Horizontal, true, window, cx)))
            .on_action(cx.listener(|w, _: &SplitDown, window, cx| w.on_split(Axis::Vertical, false, window, cx)))
            .on_action(cx.listener(|w, _: &SplitUp, window, cx| w.on_split(Axis::Vertical, true, window, cx)))
            .on_action(cx.listener(|w, _: &FocusNextPane, window, cx| {
                if let Some(tab) = w.active_terminal() {
                    tab.update(cx, |tab, cx| tab.focus_next(true, window, cx));
                }
            }))
            .on_action(cx.listener(|w, _: &FocusPreviousPane, window, cx| {
                if let Some(tab) = w.active_terminal() {
                    tab.update(cx, |tab, cx| tab.focus_next(false, window, cx));
                }
            }))
            .on_action(cx.listener(|w, _: &ToggleBroadcast, _, cx| {
                if let Some(tab) = w.active_terminal() {
                    tab.update(cx, |tab, cx| tab.toggle_broadcast(cx));
                }
            }))
            .on_action(cx.listener(|w, _: &OpenSettings, window, cx| w.show_page(Page::Settings, window, cx)))
            .on_action(cx.listener(|w, _: &TogglePalette, window, cx| w.toggle_palette(window, cx)))
            .on_action(cx.listener(|w, _: &ToggleSidePanel, window, cx| w.toggle_side_panel(window, cx)))
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|w, _: &RenameTab, window, cx| {
                if let Some(ix) = w.active {
                    w.start_rename(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &TogglePinTab, _, cx| {
                if let Some(ix) = w.active.filter(|_| w.section == Section::Cli) {
                    w.toggle_pin(ix, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &ShowHosts, window, cx| w.show_page(Page::Hosts, window, cx)))
            .on_action(cx.listener(|w, _: &ShowKeychain, window, cx| w.show_page(Page::Keychain, window, cx)))
            .on_action(cx.listener(|w, _: &ShowSnippets, window, cx| w.show_page(Page::Snippets, window, cx)))
            .on_action(cx.listener(|w, _: &ShowForwarding, window, cx| w.show_page(Page::Forwarding, window, cx)))
            .on_action(cx.listener(|w, _: &ShowKnownHosts, window, cx| w.show_page(Page::KnownHosts, window, cx)))
            .on_action(cx.listener(|w, _: &ShowLogs, window, cx| w.show_logs(window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab1, window, cx| w.select_tab(0, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab2, window, cx| w.select_tab(1, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab3, window, cx| w.select_tab(2, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab4, window, cx| w.select_tab(3, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab5, window, cx| w.select_tab(4, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab6, window, cx| w.select_tab(5, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab7, window, cx| w.select_tab(6, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab8, window, cx| w.select_tab(7, window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab9, window, cx| {
                let last = w.tabs.len().checked_sub(1);
                w.activate(last, window, cx)
            }))
            .child(self.render_title_bar(window, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_rail(cx))
                    .child(div().flex_1().min_w_0().h_full().child(body)),
            )
            .when_some(self.palette.as_ref(), |root, (palette, _)| {
                root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(72.))
                        .bg(gpui_kit::black().opacity(0.35))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|workspace, _, window, cx| workspace.close_palette(window, cx)),
                        )
                        .child(palette.clone()),
                )
            })
    }
}

/// `text` in single quotes, for a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// UI check builds pointed at a fake GitHub show its buttons without signing in.
fn fake_github() -> bool {
    #[cfg(feature = "snapshot")]
    return crate::github::api::fake_api().is_some();
    #[cfg(not(feature = "snapshot"))]
    false
}

/// UI check builds show this user's profile without signing in.
fn fake_login() -> Option<String> {
    #[cfg(feature = "snapshot")]
    return std::env::var("SOMME_GITHUB_LOGIN").ok().filter(|_| fake_github());
    #[cfg(not(feature = "snapshot"))]
    None
}

#[cfg(test)]
mod tests {
    use super::CloseTabs;

    #[test]
    fn close_items_pick_the_right_tabs() {
        let picked = |which: CloseTabs| (0..5).filter(|ix| which.picks(*ix, 2)).collect::<Vec<_>>();
        assert_eq!(picked(CloseTabs::Others), [0, 1, 3, 4]);
        assert_eq!(picked(CloseTabs::Left), [0, 1]);
        assert_eq!(picked(CloseTabs::Right), [3, 4]);
        assert_eq!(picked(CloseTabs::All), [0, 1, 2, 3, 4]);
    }
}
