//! The panel on the right of a terminal: snippets to paste or run, this
//! host's session logs, and quick font and theme changes.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Div, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::settings::Settings;
use crate::store::Store;
use crate::terminal::log::{self, LogEntry};
use crate::ui::common::{Lucide, ago};
use crate::ui::logs::started;
use crate::ui::settings_page::monospace_fonts;
use crate::ui::theme_picker::{self, section};

pub const PANEL_WIDTH: f32 = 284.;

/// A small switch with its label, bound to one settings field.
fn toggle(id: &'static str, label: &'static str, on: bool, set: fn(&mut Settings, bool)) -> Div {
    h_flex()
        .gap_2()
        .child(Switch::new(id).checked(on).small().on_click(move |on: &bool, _, cx| {
            let on = *on;
            Settings::update(cx, |s| set(s, on));
        }))
        .child(div().text_sm().child(label))
}

/// A "label  [-] size [+]" row for one of the font size settings.
fn size_row(
    ids: [&'static str; 2],
    label: &'static str,
    size: f32,
    (min, max): (f32, f32),
    set: fn(&mut Settings, f32),
    cx: &App,
) -> Div {
    let step = |id: &'static str, icon: IconName, by: f32| {
        Button::new(id)
            .outline()
            .small()
            .icon(icon)
            .disabled(!(min..=max).contains(&(size + by)))
            .on_click(move |_, _, cx| Settings::update(cx, |s| set(s, (size + by).clamp(min, max))))
    };
    h_flex()
        .gap_2()
        .child(div().flex_1().text_sm().text_color(cx.theme().muted_foreground).child(label))
        .child(step(ids[0], IconName::Minus, -1.))
        .child(div().w(px(28.)).text_center().text_sm().child(format!("{size}")))
        .child(step(ids[1], IconName::Plus, 1.))
}

pub enum SidePanelEvent {
    /// Send a snippet to the focused terminal. `run` presses Enter after it.
    Send { script: String, run: bool },
    ManageSnippets,
    /// Play a recorded session back in a new tab.
    OpenLog(LogEntry),
    ShowAllLogs,
    /// Give the keyboard back to the terminal.
    FocusTerminal,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Snippets,
    Logs,
    Appearance,
}

pub struct SidePanel {
    section: Section,
    search: Entity<InputState>,
    /// Whose logs to show: a host id, or `None` for local terminals.
    log_host: Option<String>,
    logs: Vec<LogEntry>,
    fonts: Vec<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidePanelEvent> for SidePanel {}

impl SidePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search snippets"));
        let subscriptions = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        // Scanning system fonts takes a moment, so the list fills in when ready.
        cx.spawn(async move |panel, cx| {
            let fonts = crate::runtime::io(async { Ok::<_, std::convert::Infallible>(monospace_fonts()) }).await;
            let _ = panel.update(cx, |panel, cx| {
                panel.fonts = fonts.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
        Self {
            section: Section::Snippets,
            search,
            log_host: None,
            logs: Vec::new(),
            fonts: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Follows the focused tab, so the Logs list is about its host.
    pub fn set_log_host(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        if self.log_host != host {
            self.log_host = host;
            self.reload_logs(cx);
        }
    }

    fn reload_logs(&mut self, cx: &mut Context<Self>) {
        let data_dir = Store::services(cx).data_dir.clone();
        self.logs = log::list(&data_dir).into_iter().filter(|entry| entry.host_id == self.log_host).collect();
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, icon: Lucide, name: &'static str, section: Section, cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(Icon::new(icon))
                .tooltip(name)
                .selected(self.section == section)
                .on_click(cx.listener(move |panel, _, _, cx| {
                    panel.section = section;
                    // A session started since the last look adds a log.
                    if section == Section::Logs {
                        panel.reload_logs(cx);
                    }
                    cx.notify();
                }))
        };
        h_flex()
            .h(px(40.))
            .flex_none()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(tab("panel-snippets", Lucide::SquareCode, "Snippets", Section::Snippets, cx))
            .child(tab("panel-logs", Lucide::Clock, "Logs", Section::Logs, cx))
            .child(tab("panel-appearance", Lucide::Palette, "Appearance", Section::Appearance, cx))
            .child(div().flex_1())
            .child(
                Button::new("panel-close")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::PanelRightClose))
                    .tooltip("Hide panel (⌘⇧S)")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SidePanelEvent::Close))),
            )
    }

    fn render_snippets(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let query = self.search.read(cx).value().trim().to_lowercase();
        let snippets = Store::global(cx).read(cx).snippets.clone();
        let total = snippets.len();
        let matches: Vec<_> = snippets
            .into_iter()
            .filter(|s| {
                query.is_empty() || s.label.to_lowercase().contains(&query) || s.script.to_lowercase().contains(&query)
            })
            .collect();
        let shown = matches.len();
        let mono = SharedString::from(Settings::get(cx).font_family.clone());

        let rows = matches.into_iter().enumerate().map(|(ix, snippet)| {
            let first_line = snippet.script.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned();
            let paste = snippet.script.clone();
            let run = snippet.script.clone();
            v_flex()
                .id(("panel-snippet", ix))
                .px_2()
                .py_1p5()
                .gap_0p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|row| row.bg(theme.sidebar_accent))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(Lucide::SquareCode).size(px(14.)).text_color(theme.muted_foreground))
                        .child(div().flex_1().min_w_0().truncate().text_sm().child(snippet.label.clone()))
                        .child(
                            Button::new(("panel-run", ix))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(Lucide::Play))
                                .tooltip("Run")
                                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(SidePanelEvent::Send { script: run.clone(), run: true });
                                })),
                        ),
                )
                .child(
                    div()
                        .pl(px(22.))
                        .truncate()
                        .text_xs()
                        .font_family(mono.clone())
                        .text_color(theme.muted_foreground)
                        .child(first_line),
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(SidePanelEvent::Send { script: paste.clone(), run: false });
                }))
        });

        let empty = |text: &'static str| {
            div().py_6().text_sm().text_center().text_color(theme.muted_foreground).child(text)
        };
        let list = v_flex()
            .id("panel-snippet-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_1()
            .when(total == 0, |list| list.child(empty("No snippets yet.")))
            .when(total > 0 && shown == 0, |list| list.child(empty("No matches.")))
            .children(rows);

        v_flex()
            .size_full()
            .child(
                v_flex()
                    .p_2()
                    .gap_1p5()
                    .child(
                        Input::new(&self.search)
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).size(px(14.))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Click to paste. Press ▶ to run."),
                    ),
            )
            .child(list)
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(
                    Button::new("panel-manage")
                        .ghost()
                        .small()
                        .w_full()
                        .icon(IconName::Plus)
                        .label(if total == 0 { "New snippet" } else { "Manage snippets" })
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidePanelEvent::ManageSnippets))),
                ),
            )
            .into_any_element()
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let recording = Settings::get(cx).log_sessions;
        let rows = self.logs.iter().enumerate().map(|(ix, entry)| {
            let open = entry.clone();
            h_flex()
                .id(("panel-log", ix))
                .px_2()
                .py_1p5()
                .gap_2()
                .rounded_md()
                .cursor_pointer()
                .hover(|row| row.bg(theme.sidebar_accent))
                .child(Icon::new(Lucide::Clock).size(px(14.)).text_color(theme.muted_foreground))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(started(entry.started_at)))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(ago(entry.started_at))),
                )
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(SidePanelEvent::OpenLog(open.clone()))))
        });
        let empty = if !recording {
            "Recording is off. Turn it on to keep a copy of what each session prints."
        } else {
            "No logs for this host yet. New sessions are recorded."
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .child(
                        Switch::new("panel-record")
                            .checked(recording)
                            .small()
                            .on_click(|on: &bool, _, cx| {
                                let on = *on;
                                Settings::update(cx, |s| s.log_sessions = on);
                            }),
                    )
                    .child(div().text_sm().child("Record new sessions")),
            )
            .child(
                v_flex()
                    .id("panel-log-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .when(self.logs.is_empty(), |list| {
                        list.child(
                            div().p_4().text_sm().text_center().text_color(theme.muted_foreground).child(empty),
                        )
                    })
                    .children(rows),
            )
            .child(
                div().p_2().border_t_1().border_color(theme.border).child(
                    Button::new("panel-all-logs")
                        .ghost()
                        .small()
                        .w_full()
                        .label("All logs")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidePanelEvent::ShowAllLogs))),
                ),
            )
            .into_any_element()
    }

    fn render_font(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx);
        let current = SharedString::from(settings.font_family.clone());
        let (size, ui_size) = (settings.font_size, settings.ui_font_size);
        let mut fonts = self.fonts.clone();
        if !fonts.contains(&current) {
            fonts.insert(0, current.clone());
        }
        let family = Button::new("panel-font")
            .outline()
            .small()
            .w_full()
            .label(current.clone())
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let menu = fonts.iter().fold(menu, |menu, font| {
                    let font = font.clone();
                    menu.item(PopupMenuItem::new(font.clone()).checked(font == current).on_click(move |_, _, cx| {
                        let font = font.to_string();
                        Settings::update(cx, |s| s.font_family = font);
                    }))
                });
                menu.scrollable(true)
            });
        v_flex()
            .w_full()
            .gap_2()
            .child(div().text_sm().font_medium().child("Font"))
            .child(family)
            .child(toggle("panel-font-everywhere", "Use for the whole app", settings.font_everywhere, |s, on| {
                s.font_everywhere = on
            }))
            .child(size_row(
                ["panel-app-font-smaller", "panel-app-font-bigger"],
                "App size",
                ui_size,
                (12., 24.),
                |s, v| s.ui_font_size = v,
                cx,
            ))
            .child(size_row(
                ["panel-font-smaller", "panel-font-bigger"],
                "Terminal size",
                size,
                (8., 32.),
                |s, v| s.font_size = v,
                cx,
            ))
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        // The current mode first.
        let dark = cx.theme().is_dark();
        let (first, second) = if dark { (true, false) } else { (false, true) };
        let title = |mode_dark: bool| if mode_dark { "Dark app theme" } else { "Light app theme" };
        let matching = Settings::get(cx).terminal_matches_theme;
        let schemes = v_flex()
            .gap_2()
            .child(toggle("panel-match-theme", "Match app theme", matching, |s, on| s.terminal_matches_theme = on))
            .child(theme_picker::terminal_schemes(cx));
        v_flex()
            .id("panel-theme-list")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_5()
            // Nothing here types, so keep the keyboard in the terminal after a pick.
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SidePanelEvent::FocusTerminal)))
            .child(self.render_font(cx))
            .child(section(title(first), theme_picker::app_themes(first, cx), cx))
            .child(section("Terminal colors", schemes, cx))
            .child(section(title(second), theme_picker::app_themes(second, cx), cx))
            .into_any_element()
    }
}

impl Render for SidePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.section {
            Section::Snippets => self.render_snippets(cx),
            Section::Logs => self.render_logs(cx),
            Section::Appearance => self.render_appearance(cx),
        };
        let theme = cx.theme();
        v_flex()
            .w(px(PANEL_WIDTH))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().child(body))
    }
}
