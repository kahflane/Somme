use chrono::{Local, TimeZone as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::settings::Settings;
use crate::store::Store;
use crate::terminal::SessionTarget;
use crate::terminal::log::{self, LogEntry};
use crate::ui::common::{
    Lucide, ago, card, confirm_delete, empty_state, host_badge, human_size, notify_error, page_header, subtle,
};
use crate::ui::workspace::Workspace;

/// Recorded terminal sessions. Opening one plays it back in a read-only tab.
pub struct LogsPage {
    workspace: WeakEntity<Workspace>,
    search: Entity<InputState>,
    entries: Vec<(LogEntry, u64)>,
    _subscriptions: Vec<Subscription>,
}

impl LogsPage {
    pub fn new(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search logs"));
        let subscriptions = vec![cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })];
        let mut page = Self {
            workspace,
            search,
            entries: Vec::new(),
            _subscriptions: subscriptions,
        };
        page.reload(cx);
        page
    }

    /// Re-reads the log index. New sessions add to it while the page is hidden.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let data_dir = Store::services(cx).data_dir.clone();
        self.entries = log::list(&data_dir)
            .into_iter()
            .map(|entry| {
                let size = log::size(&data_dir, &entry);
                (entry, size)
            })
            .collect();
        cx.notify();
    }

    fn open(&mut self, entry: &LogEntry, window: &mut Window, cx: &mut Context<Self>) {
        let target = SessionTarget::Replay {
            label: entry.label.clone(),
            file: entry.file.clone(),
        };
        let _ = self
            .workspace
            .update(cx, |workspace, cx| workspace.open_terminal(target, window, cx));
    }

    fn delete(&mut self, entries: Vec<LogEntry>, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        confirm_delete(
            title,
            "Deleted logs cannot be brought back.",
            move |window, cx| {
                let data_dir = Store::services(cx).data_dir.clone();
                for entry in &entries {
                    if let Err(err) = log::delete(&data_dir, entry) {
                        notify_error(window, cx, "Could not delete log", err);
                        break;
                    }
                }
                let _ = page.update(cx, |page, cx| page.reload(cx));
            },
            window,
            cx,
        );
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = Settings::get(cx).log_sessions;
        let all: Vec<LogEntry> = self.entries.iter().map(|(e, _)| e.clone()).collect();
        let page = cx.entity().downgrade();
        page_header("Logs", cx)
            .child(
                h_flex()
                    .gap_2()
                    .mr_2()
                    .child(
                        Switch::new("record-sessions")
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
                div().w(px(240.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size(px(14.))),
                ),
            )
            .child(
                Button::new("logs-more")
                    .small()
                    .icon(IconName::Ellipsis)
                    .disabled(all.is_empty())
                    .dropdown_menu(move |menu, _, _| {
                        let page = page.clone();
                        let all = all.clone();
                        menu.item(PopupMenuItem::new("Delete All Logs").icon(IconName::Delete).on_click(
                            move |_, window, cx| {
                                let count = all.len();
                                let all = all.clone();
                                let _ = page.update(cx, |page, cx| {
                                    page.delete(all, format!("Delete all {count} logs?"), window, cx)
                                });
                            },
                        ))
                    }),
            )
    }

    fn render_row(&self, entry: LogEntry, size: u64, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = SharedString::from(entry.file.clone());
        let local = entry.host_id.is_none();
        let open = entry.clone();
        let delete = entry.clone();
        card(cx)
            .id(id.clone())
            .group("log")
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| page.open(&open, window, cx)))
            .child(host_badge(
                None,
                Icon::new(if local { Lucide::SquareTerminal } else { Lucide::Server }),
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().truncate().child(entry.label.clone()))
                    .child(subtle(format!("{} · {}", started(entry.started_at), ago(entry.started_at)), cx)),
            )
            .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(human_size(size)))
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover("log", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("delete-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip("Delete log")
                            .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                let title = format!("Delete log of {}?", delete.label);
                                page.delete(vec![delete.clone()], title, window, cx);
                            })),
                    ),
            )
    }
}

pub(crate) fn started(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|t| t.format("%b %-d, %Y %H:%M").to_string())
        .unwrap_or_default()
}

impl Render for LogsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let rows: Vec<(LogEntry, u64)> = self
            .entries
            .iter()
            .filter(|(e, _)| query.is_empty() || e.label.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let recording = Settings::get(cx).log_sessions;

        let body = if self.entries.is_empty() {
            empty_state(
                Icon::new(Lucide::Clock),
                "No logs yet",
                if recording {
                    "Every terminal session you open is recorded here, so you can read it again later."
                } else {
                    "Recording is off. Turn on \"Record new sessions\" to keep a copy of what each terminal prints."
                },
                cx,
            )
            .into_any_element()
        } else if rows.is_empty() {
            empty_state(IconName::Search, "Nothing matches", "Try a different search.", cx).into_any_element()
        } else {
            v_flex()
                .px_5()
                .py_4()
                .gap_2()
                .when(!recording, |list| {
                    list.child(subtle("Recording is off. Older logs are kept until you delete them.", &*cx))
                })
                .children(rows.into_iter().map(|(entry, size)| self.render_row(entry, size, cx)))
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }
}
