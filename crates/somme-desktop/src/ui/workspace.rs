use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, TitleBar, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, actions, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use somme_application::dto::{HostInput, HostView, SecretUpdate};
use somme_domain::Id;

use crate::changes::ChangesView;
use crate::services::Services;
use crate::settings::Settings;
use crate::store::{Store, StoreEvent, mutate};
use crate::terminal::SessionTarget;
use crate::ui::common::{APP_NAME, Lucide, logo, notify_error, parse_color};
use crate::ui::native;
use crate::ui::fields::host_input;
use crate::ui::filter::parse_quick_connect;
use crate::ui::forwards::ForwardsPage;
use crate::ui::hosts::HostsPage;
use crate::ui::keychain::KeychainPage;
use crate::ui::lock::LockScreen;
use crate::ui::known_hosts::KnownHostsPage;
use crate::ui::logs::LogsPage;
use crate::ui::palette::{CommandPalette, PaletteAction, PaletteEvent};
use crate::ui::settings_page::SettingsPage;
use crate::ui::side_panel::{SidePanel, SidePanelEvent};
use crate::ui::sftp::SftpPage;
use crate::ui::snippets::SnippetsPage;
use crate::ui::terminal_tab::{TerminalTab, TerminalTabEvent};

actions!(
    workspace,
    [
        NewLocalTerminal,
        CloseTab,
        ClosePane,
        NextTab,
        PreviousTab,
        ShowHome,
        SplitRight,
        SplitDown,
        FocusNextPane,
        FocusPreviousPane,
        ToggleBroadcast,
        LockVault,
        OpenSettings,
        TogglePalette,
        ToggleSidePanel,
        Quit,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Hosts,
    Sftp,
    Keychain,
    Forwarding,
    Snippets,
    KnownHosts,
    Logs,
    Settings,
}

impl Page {
    pub(crate) const ALL: [Page; 8] = [
        Page::Hosts,
        Page::Sftp,
        Page::Keychain,
        Page::Forwarding,
        Page::Snippets,
        Page::KnownHosts,
        Page::Logs,
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
            Page::Logs => "Logs",
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
            Page::Logs => Icon::new(Lucide::Clock),
            Page::Settings => Icon::new(Lucide::Settings),
        }
    }
}

enum TabContent {
    Terminal(Entity<TerminalTab>),
    Changes(Entity<ChangesView>),
}

struct Tab {
    id: usize,
    content: TabContent,
    _subscription: Subscription,
}

/// A tab whose new name is being typed.
struct Renaming {
    tab_id: usize,
    input: Entity<InputState>,
    _subscription: Subscription,
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
    /// `None` shows the home screen (hosts, keychain, ...).
    active: Option<usize>,
    page: Page,
    next_tab_id: usize,
    /// The terminal tab used last, where snippets run from the home screen.
    last_terminal: Option<usize>,
    lock_screen: Entity<LockScreen>,
    hosts: Option<Entity<HostsPage>>,
    keychain: Option<Entity<KeychainPage>>,
    forwards: Option<Entity<ForwardsPage>>,
    snippets: Option<Entity<SnippetsPage>>,
    known_hosts: Option<Entity<KnownHostsPage>>,
    logs: Option<Entity<LogsPage>>,
    settings: Option<Entity<SettingsPage>>,
    sftp: Option<Entity<SftpPage>>,
    focus_handle: FocusHandle,
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    /// Snippets and themes next to the terminal. Made the first time it shows.
    side_panel: Option<(Entity<SidePanel>, Subscription)>,
    /// Last key press, click or scroll. Auto-lock counts idle time from here.
    last_activity: Instant,
    /// Set after the first unlock, so the start-up terminal opens only once.
    started: bool,
    renaming: Option<Renaming>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let lock_screen = cx.new(|cx| LockScreen::new(window, cx));
        let subscriptions = vec![
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.subscribe_in(&store, window, |workspace, store, event, window, cx| match event {
                StoreEvent::VaultChanged => {
                    if store.read(cx).is_unlocked() {
                        // The app starts on a local terminal.
                        if !workspace.started && workspace.tabs.is_empty() {
                            workspace.open_terminal(SessionTarget::Local, window, cx);
                        }
                        workspace.started = true;
                        workspace.focus_current(window, cx);
                    } else {
                        workspace.lock_screen.update(cx, |lock, cx| lock.focus_password(window, cx));
                    }
                }
            }),
        ];
        Self::watch_idle(cx);
        Self {
            services,
            tabs: Vec::new(),
            active: None,
            page: Page::Hosts,
            next_tab_id: 0,
            last_terminal: None,
            lock_screen,
            hosts: None,
            keychain: None,
            forwards: None,
            snippets: None,
            known_hosts: None,
            logs: None,
            settings: None,
            sftp: None,
            focus_handle: cx.focus_handle(),
            palette: None,
            side_panel: None,
            last_activity: Instant::now(),
            started: false,
            renaming: None,
            _subscriptions: subscriptions,
        }
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
        match self.active.and_then(|ix| self.tabs.get(ix)) {
            Some(Tab {
                content: TabContent::Terminal(tab),
                ..
            }) => window.focus(&tab.focus_handle(cx), cx),
            Some(Tab {
                content: TabContent::Changes(view),
                ..
            }) => window.focus(&view.focus_handle(cx), cx),
            None => window.focus(&self.focus_handle, cx),
        }
    }

    // ---- tabs ----

    pub fn open_terminal(&mut self, target: SessionTarget, window: &mut Window, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let dir = matches!(target, SessionTarget::Local).then(|| self.previous_local_dir(cx)).flatten();
        let tab = cx.new(|cx| TerminalTab::new(target, dir, services, window, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let subscription = cx.subscribe_in(&tab, window, move |workspace, _, event, window, cx| match event {
            TerminalTabEvent::Changed => cx.notify(),
            TerminalTabEvent::Empty => workspace.remove_tab(id, window, cx),
            TerminalTabEvent::OpenChanges(dir) => workspace.open_changes(dir.clone(), window, cx),
        });
        self.tabs.push(Tab {
            id,
            content: TabContent::Terminal(tab),
            _subscription: subscription,
        });
        self.activate(Some(self.tabs.len() - 1), window, cx);
    }

    /// Shows the git changes of the repo that holds `dir`, in its own tab.
    fn open_changes(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.tabs.iter().position(|tab| match &tab.content {
            TabContent::Changes(view) => view.read(cx).shows(&dir),
            TabContent::Terminal(_) => false,
        });
        if let Some(ix) = open {
            if let TabContent::Changes(view) = &self.tabs[ix].content {
                view.update(cx, |view, cx| view.reload(cx));
            }
            self.activate(Some(ix), window, cx);
            return;
        }
        let view = cx.new(|cx| ChangesView::new(dir, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let subscription = cx.observe(&view, |_, _, cx| cx.notify());
        self.tabs.push(Tab {
            id,
            content: TabContent::Changes(view),
            _subscription: subscription,
        });
        self.activate(Some(self.tabs.len() - 1), window, cx);
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
        if !Store::global(cx).read(cx).is_unlocked() {
            return;
        }
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
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
            PaletteAction::QuickConnect(text) => {
                self.quick_connect(&text, None, window, cx);
            }
            PaletteAction::Host(id) => self.connect_host(id, window, cx),
            PaletteAction::Snippet(script) => self.send_snippet(&script, true, window, cx),
            PaletteAction::NewLocalTerminal => self.open_terminal(SessionTarget::Local, window, cx),
            PaletteAction::Page(page) => self.show_page(page, window, cx),
            PaletteAction::Lock => Store::global(cx).update(cx, |store, cx| store.lock(cx)),
        }
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
            SidePanelEvent::ShowAllLogs => workspace.show_page(Page::Logs, window, cx),
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

    fn activate(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index.filter(|ix| *ix < self.tabs.len());
        match self.active {
            Some(ix) => {
                if let TabContent::Terminal(_) = self.tabs[ix].content {
                    self.last_terminal = Some(self.tabs[ix].id);
                }
            }
            None => self.reload_page(cx),
        }
        self.focus_current(window, cx);
        cx.notify();
    }

    fn remove_tab(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t.id == id) else { return };
        self.tabs.remove(ix);
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
        self.activate(next, window, cx);
    }

    /// Moves the tab with `id` to position `to`, keeping the same tab in front.
    fn move_tab(&mut self, id: usize, to: usize, cx: &mut Context<Self>) {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else { return };
        let to = to.min(self.tabs.len() - 1);
        if from == to {
            return;
        }
        let active_id = self.active.map(|ix| self.tabs[ix].id);
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = active_id.and_then(|id| self.tabs.iter().position(|t| t.id == id));
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
            TabContent::Changes(_) => false,
        }
    }

    /// Closes a tab, asking first when sessions in it are still connected.
    fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let unsaved = match &tab.content {
            TabContent::Terminal(term) => term.read(cx).has_unsaved(cx),
            TabContent::Changes(_) => false,
        };
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

    fn active_terminal(&self) -> Option<Entity<TerminalTab>> {
        match &self.tabs.get(self.active?)?.content {
            TabContent::Terminal(tab) => Some(tab.clone()),
            TabContent::Changes(_) => None,
        }
    }

    fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Position 0 is the home screen, tabs follow.
        let count = self.tabs.len() + 1;
        let current = self.active.map(|ix| ix + 1).unwrap_or(0);
        let next = if forward { (current + 1) % count } else { (current + count - 1) % count };
        self.activate(next.checked_sub(1), window, cx);
    }

    pub fn show_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.activate(None, window, cx);
    }

    /// Opens the SFTP screen with `host_id` on the right side.
    pub fn open_sftp(&mut self, host_id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.sftp
            .get_or_insert_with(|| cx.new(|cx| SftpPage::new(window, cx)))
            .update(cx, |page, cx| page.open_host(host_id, window, cx));
        self.show_page(Page::Sftp, window, cx);
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
            Page::Logs => {
                if let Some(logs) = &self.logs {
                    logs.update(cx, |logs, cx| logs.reload(cx));
                }
            }
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
            Some(_) => {
                if let Some(ix) = self.active {
                    self.request_close_tab(ix, window, cx);
                }
            }
            None => window.remove_window(),
        }
    }

    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active {
            self.request_close_tab(ix, window, cx);
        }
    }

    fn on_split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_terminal() {
            tab.update(cx, |tab, cx| tab.split(axis, window, cx));
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
        let home_active = self.active.is_none();
        let home = div()
            .id("tab-home")
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .border_1()
            .text_sm()
            .cursor_pointer()
            .when(home_active, |el| {
                el.bg(tab_active).border_color(theme.border).text_color(theme.foreground)
            })
            .when(!home_active, |el| {
                el.bg(tab_bg)
                    .border_color(theme.transparent)
                    .text_color(theme.tab_foreground)
                    .hover(|el| el.bg(tab_hover))
            })
            .child(Icon::new(Lucide::LayoutGrid).size(px(14.)))
            .child("Vault")
            // Keeps a press here from moving the window, as the title bar behind would.
            .block_mouse_except_scroll()
            .on_click(cx.listener(|workspace, _, window, cx| workspace.activate(None, window, cx)));

        let workspace = cx.entity().downgrade();
        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let selected = self.active == Some(ix);
            let changes = matches!(tab.content, TabContent::Changes(_));
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
            let menu_workspace = workspace.clone();
            div()
                .id(("tab", tab.id))
                .group("tab")
                .h(px(26.))
                .min_w(px(110.))
                .max_w(px(220.))
                .pl_2()
                .pr_1()
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
                .when(changes, |el| el.child(Icon::new(Lucide::FileDiff).size(px(13.))))
                .when(!changes, |el| el.child(div().flex_none().size(px(7.)).rounded_full().bg(dot)))
                .when(broadcast, |el| {
                    el.child(Icon::new(Lucide::RadioTower).size(px(12.)).text_color(theme.warning))
                })
                .child(label)
                .child(
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
                .on_click(cx.listener(move |workspace, event: &ClickEvent, window, cx| {
                    if event.click_count() == 2 && !changes {
                        workspace.start_rename(ix, window, cx);
                    } else if workspace.renaming.as_ref().is_none_or(|r| r.tab_id != tab_id) {
                        workspace.activate(Some(ix), window, cx);
                    }
                }))
                .block_mouse_except_scroll()
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |workspace, _, window, cx| workspace.request_close_tab(ix, window, cx)),
                )
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
                .drag_over::<DraggedTab>(|style, _, _, cx| style.border_color(cx.theme().primary))
                .on_drop(cx.listener(move |workspace, dragged: &DraggedTab, _, cx| {
                    workspace.move_tab(dragged.id, ix, cx);
                }))
                .context_menu(move |menu, _, _| {
                    let (rename, reset, close) = (menu_workspace.clone(), menu_workspace.clone(), menu_workspace.clone());
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
                    .item(PopupMenuItem::new("Close Tab").icon(IconName::Close).on_click(move |_, window, cx| {
                        let _ = close.update(cx, |workspace, cx| workspace.request_close_tab(ix, window, cx));
                    }))
                })
        });

        let new_tab = Button::new("new-tab")
            .ghost()
            .xsmall()
            .icon(IconName::Plus)
            .tooltip("New tab")
            .dropdown_menu(|menu, _, _| {
                menu.menu_with_icon("Local Terminal", Icon::new(Lucide::SquareTerminal), Box::new(NewLocalTerminal))
                    .menu_with_icon("Split Right", Icon::new(Lucide::Columns2), Box::new(SplitRight))
                    .menu_with_icon("Split Down", Icon::new(Lucide::Rows2), Box::new(SplitDown))
                    .separator()
                    .menu_with_icon("Broadcast Input", Icon::new(Lucide::RadioTower), Box::new(ToggleBroadcast))
            });

        h_flex()
            .id("tab-strip")
            .flex_1()
            .min_w_0()
            .gap_1()
            .overflow_x_scroll()
            .child(home)
            .children(tabs)
            .child(new_tab)
    }

    fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unlocked = Store::global(cx).read(cx).is_unlocked();
        TitleBar::new()
            // The kit keeps room for the window buttons, which full screen hides.
            .when(window.is_fullscreen(), |bar| bar.pl_0())
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .pr_2()
                    .gap_2()
                    .when(unlocked, |bar| bar.child(self.render_tab_strip(cx)))
                    .when(!unlocked, |bar| bar.child(div().flex_1().text_sm().child(APP_NAME)))
                    .when(unlocked && self.side_panel_fits(cx), |bar| {
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
                    .when(unlocked, |bar| {
                        bar.child(
                            Button::new("palette")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Search)
                                .tooltip("Search and connect (⌘P)")
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.toggle_palette(window, cx))),
                        )
                        .child(
                            Button::new("lock")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(Lucide::Lock))
                                .tooltip("Lock vault")
                                .on_click(cx.listener(|workspace, _, window, cx| workspace.on_lock(&LockVault, window, cx))),
                        )
                    }),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = Store::global(cx);
        let account = store.read(cx).account.clone();
        let items = Page::ALL.into_iter().map(|page| {
            SidebarMenuItem::new(page.label())
                .icon(page.icon())
                .active(self.page == page && self.active.is_none())
                .on_click(cx.listener(move |workspace, _, window, cx| workspace.show_page(page, window, cx)))
        });
        let account_label: SharedString = account
            .email
            .clone()
            .unwrap_or_else(|| "Not signed in".to_owned())
            .into();
        Sidebar::new("nav")
            .collapsible(false)
            .header(
                SidebarHeader::new().child(
                    h_flex()
                        .gap_2()
                        .child(logo(px(20.)))
                        .child(div().font_semibold().child(APP_NAME)),
                ),
            )
            .child(SidebarGroup::new("Vault").child(SidebarMenu::new().children(items)))
            .footer(
                SidebarFooter::new().child(
                    h_flex()
                        .id("account")
                        .w_full()
                        .gap_2()
                        .cursor_pointer()
                        .on_click(cx.listener(|workspace, _, window, cx| workspace.show_account(window, cx)))
                        .child(Icon::new(Lucide::CircleUser).size(px(18.)))
                        .child(div().text_sm().truncate().child(account_label)),
                ),
            )
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
            Page::Logs => {
                let workspace = cx.entity().downgrade();
                self.logs
                    .get_or_insert_with(|| cx.new(|cx| LogsPage::new(workspace, window, cx)))
                    .clone()
                    .into_any_element()
            }
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
        let body: AnyElement = if !unlocked {
            self.lock_screen.clone().into_any_element()
        } else if let Some(Tab {
            content: TabContent::Changes(view),
            ..
        }) = self.active.and_then(|ix| self.tabs.get(ix))
        {
            view.clone().into_any_element()
        } else if let Some(Tab {
            content: TabContent::Terminal(term),
            ..
        }) = self.active.and_then(|ix| self.tabs.get(ix))
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
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_lock))
            .on_action(cx.listener(|w, _: &NextTab, window, cx| w.cycle_tab(true, window, cx)))
            .on_action(cx.listener(|w, _: &PreviousTab, window, cx| w.cycle_tab(false, window, cx)))
            .on_action(cx.listener(|w, _: &ShowHome, window, cx| w.activate(None, window, cx)))
            .on_action(cx.listener(|w, _: &SplitRight, window, cx| w.on_split(Axis::Horizontal, window, cx)))
            .on_action(cx.listener(|w, _: &SplitDown, window, cx| w.on_split(Axis::Vertical, window, cx)))
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
            .on_action(cx.listener(|w, _: &SelectTab1, window, cx| w.activate(Some(0), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab2, window, cx| w.activate(Some(1), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab3, window, cx| w.activate(Some(2), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab4, window, cx| w.activate(Some(3), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab5, window, cx| w.activate(Some(4), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab6, window, cx| w.activate(Some(5), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab7, window, cx| w.activate(Some(6), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab8, window, cx| w.activate(Some(7), window, cx)))
            .on_action(cx.listener(|w, _: &SelectTab9, window, cx| {
                let last = w.tabs.len().checked_sub(1);
                w.activate(last, window, cx)
            }))
            .child(self.render_title_bar(window, cx))
            .child(div().flex_1().min_h_0().child(body))
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
