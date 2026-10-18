use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use somme_domain::Id;

use crate::store::Store;
use crate::ui::common::{Lucide, parse_color};
use crate::ui::filter::{fuzzy_score, host_haystack, parse_quick_connect};
use crate::ui::workspace::Page;

/// Long lists slow typing down and nobody scrolls that far.
const MAX_ROWS: usize = 50;

/// What a palette row does when picked.
#[derive(Clone)]
pub enum PaletteAction {
    /// Saves typed `user@host:port` as a host and connects.
    QuickConnect(String),
    Host(Id),
    /// Runs a snippet in the terminal used last.
    Snippet(String),
    NewLocalTerminal,
    Page(Page),
    Lock,
    /// Opens the SSH section, which asks for the master password.
    Unlock,
}

pub enum PaletteEvent {
    Pick(PaletteAction),
    Dismiss,
}

struct Row {
    action: PaletteAction,
    icon: Icon,
    title: SharedString,
    detail: SharedString,
    kind: &'static str,
}

/// Search box over hosts, snippets and commands. Typing an address offers to connect to it.
pub struct CommandPalette {
    input: Entity<InputState>,
    rows: Vec<Row>,
    selected: usize,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search hosts, snippets, commands, or type user@host"));
        let subscriptions = vec![cx.subscribe(&input, |palette, _, event: &InputEvent, cx| match event {
            InputEvent::Change => palette.refresh(cx),
            InputEvent::PressEnter { .. } => palette.pick(palette.selected, cx),
            _ => {}
        })];
        let mut palette = Self {
            input,
            rows: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        palette.refresh(cx);
        palette
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_owned();
        self.rows = rows(&query, cx);
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            cx.emit(PaletteEvent::Pick(row.action.clone()));
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
}

fn rows(query: &str, cx: &App) -> Vec<Row> {
    let store = Store::global(cx).read(cx);
    // Rows keep their section order (hosts, commands, snippets) until a query ranks them.
    let mut rows: Vec<(i32, Row)> = Vec::new();
    let mut add = |text: &str, row: Row| {
        if let Some(score) = fuzzy_score(query, text) {
            rows.push((score, row));
        }
    };

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
        let mut icon = Icon::new(Lucide::Server);
        if let Some(color) = parse_color(host.color.as_deref()) {
            icon = icon.text_color(color);
        }
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

    add(
        "New Local Terminal",
        Row {
            action: PaletteAction::NewLocalTerminal,
            icon: Icon::new(Lucide::SquareTerminal),
            title: "New Local Terminal".into(),
            detail: "".into(),
            kind: "Command",
        },
    );
    for page in Page::ALL {
        let title = format!("Go to {}", page.label());
        add(
            &title,
            Row {
                action: PaletteAction::Page(page),
                icon: page.icon(),
                title: title.clone().into(),
                detail: "".into(),
                kind: "Command",
            },
        );
    }
    let (title, action, icon) = if store.is_unlocked() {
        ("Lock Saved Servers and Keys", PaletteAction::Lock, Lucide::Lock)
    } else {
        ("Unlock Saved Servers and Keys", PaletteAction::Unlock, Lucide::LockOpen)
    };
    add(
        title,
        Row {
            action,
            icon: Icon::new(icon),
            title: title.into(),
            detail: "".into(),
            kind: "Command",
        },
    );

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

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected;
        let list = if self.rows.is_empty() {
            div()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No matches")
                .into_any_element()
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
                        .on_click(cx.listener(move |palette, _, _, cx| palette.pick(ix, cx)))
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

        v_flex()
            .id("palette")
            .w(px(560.))
            .max_h(px(440.))
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
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div().px_2().py_1().border_b_1().border_color(theme.border).child(
                    Input::new(&self.input)
                        .appearance(false)
                        .prefix(Icon::new(IconName::Search).size(px(16.))),
                ),
            )
            .child(list)
    }
}
