use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::sidebar::{Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, TitleBar, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, actions, div, prelude::FluentBuilder as _, px,
};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use somme_application::dto::{HostInput, HostView, SecretUpdate};
use somme_domain::Id;

use crate::services::Services;
use crate::settings::Settings;
use crate::store::{Store, StoreEvent, mutate};
use crate::terminal::SessionTarget;
use crate::ui::common::{Lucide, notify_error, parse_color};
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
}

struct Tab {
    id: usize,
    content: TabContent,
    _subscription: Subscription,
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
    /// Last key press, click or scroll. Auto-lock counts idle time from here.
    last_activity: Instant,
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
            last_activity: Instant::now(),
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
            None => window.focus(&self.focus_handle, cx),
        }
    }

    // ---- tabs ----

    pub fn open_terminal(&mut self, target: SessionTarget, window: &mut Window, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let tab = cx.new(|cx| TerminalTab::new(target, services, window, cx));
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let subscription = cx.subscribe_in(&tab, window, move |workspace, _, event, window, cx| match event {
            TerminalTabEvent::Changed => cx.notify(),
            TerminalTabEvent::Empty => workspace.remove_tab(id, window, cx),
        });
        self.tabs.push(Tab {
            id,
            content: TabContent::Terminal(tab),
            _subscription: subscription,
        });
        self.activate(Some(self.tabs.len() - 1), window, cx);
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
        let ix = self
            .active
            .or_else(|| self.tabs.iter().position(|t| Some(t.id) == self.last_terminal))
            .or_else(|| self.tabs.len().checked_sub(1));
        let Some(ix) = ix else {
            notify_error(window, cx, "No terminal open", "Connect to a host or open a local terminal first.");
            return;
        };
        self.activate(Some(ix), window, cx);
        match &self.tabs[ix].content {
            TabContent::Terminal(tab) => tab.update(cx, |tab, cx| tab.send_snippet(script, run, cx)),
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
            Some(ix) => self.last_terminal = Some(self.tabs[ix].id),
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

    fn tab_is_running(&self, ix: usize, cx: &App) -> bool {
        match &self.tabs[ix].content {
            TabContent::Terminal(tab) => tab.read(cx).is_running(cx),
        }
    }

    /// Closes a tab, asking first when sessions in it are still connected.
    fn request_close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.tabs.get(ix).map(|t| t.id) else { return };
        if !Settings::get(cx).confirm_close_tab || !self.tab_is_running(ix, cx) {
            self.remove_tab(id, window, cx);
            return;
        }
        let workspace = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let workspace = workspace.clone();
            alert
                .confirm()
                .title("Close this tab?")
                .description("The sessions in this tab are still connected. They will be disconnected.")
                .ok_text("Close")
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let _ = workspace.update(cx, |workspace, cx| workspace.remove_tab(id, window, cx));
                    true
                })
        });
    }

    fn active_terminal(&self) -> Option<Entity<TerminalTab>> {
        match &self.tabs.get(self.active?)?.content {
            TabContent::Terminal(tab) => Some(tab.clone()),
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
        let home_active = self.active.is_none();
        let home = div()
            .id("tab-home")
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .when(home_active, |el| el.bg(theme.tab_active).text_color(theme.tab_active_foreground))
            .when(!home_active, |el| el.text_color(theme.tab_foreground).hover(|el| el.bg(theme.muted)))
            .child(Icon::new(Lucide::LayoutGrid).size(px(14.)))
            .child("Vault")
            .on_click(cx.listener(|workspace, _, window, cx| workspace.activate(None, window, cx)));

        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let selected = self.active == Some(ix);
            let (title, color, broadcast, running) = match &tab.content {
                TabContent::Terminal(term) => {
                    let term = term.read(cx);
                    let color = match &term.target {
                        SessionTarget::Ssh { host_id, .. } => Store::global(cx)
                            .read(cx)
                            .host(*host_id)
                            .and_then(|h| parse_color(h.color.as_deref())),
                        SessionTarget::Local | SessionTarget::Replay { .. } => None,
                    };
                    (term.title(cx), color, term.broadcast, term.is_running(cx))
                }
            };
            let dot = color.unwrap_or(if running { theme.success } else { theme.muted_foreground });
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
                .text_sm()
                .cursor_pointer()
                .when(selected, |el| el.bg(theme.tab_active).text_color(theme.tab_active_foreground))
                .when(!selected, |el| el.text_color(theme.tab_foreground).hover(|el| el.bg(theme.muted)))
                .child(div().flex_none().size(px(7.)).rounded_full().bg(dot))
                .when(broadcast, |el| {
                    el.child(Icon::new(Lucide::RadioTower).size(px(12.)).text_color(theme.warning))
                })
                .child(div().flex_1().min_w_0().truncate().child(title))
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
                .on_click(cx.listener(move |workspace, _, window, cx| workspace.activate(Some(ix), window, cx)))
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |workspace, _, window, cx| workspace.request_close_tab(ix, window, cx)),
                )
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

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let unlocked = Store::global(cx).read(cx).is_unlocked();
        TitleBar::new().child(
            h_flex()
                .flex_1()
                .min_w_0()
                .pr_2()
                .gap_2()
                .when(unlocked, |bar| bar.child(self.render_tab_strip(cx)))
                .when(!unlocked, |bar| bar.child(div().flex_1().text_sm().child("Somme")))
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
                        .child(Icon::new(Lucide::SquareTerminal).size(px(18.)))
                        .child(div().font_semibold().child("Somme")),
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
        } else if let Some(tab) = self.active.and_then(|ix| self.tabs.get(ix)) {
            match &tab.content {
                TabContent::Terminal(term) => term.clone().into_any_element(),
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
            .child(self.render_title_bar(cx))
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
