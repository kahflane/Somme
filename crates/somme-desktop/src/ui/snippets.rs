use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px,
};
use somme_application::dto::{SnippetInput, SnippetView};
use somme_domain::Id;

use crate::store::{Store, mutate, mutate_weak, report};
use crate::ui::common::{
    Lucide, card, confirm_delete, copy_text, empty_state, form_dialog, host_badge, labeled, page_header,
    subtle,
};
use crate::ui::workspace::Workspace;

/// Saved commands that can be run in a terminal or on several hosts at once.
pub struct SnippetsPage {
    workspace: WeakEntity<Workspace>,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SnippetsPage {
    pub fn new(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search snippets"));
        let subscriptions = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        Self {
            workspace,
            search,
            _subscriptions: subscriptions,
        }
    }

    fn send(&self, script: String, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self
            .workspace
            .update(cx, |workspace, cx| workspace.send_snippet(&script, run, window, cx));
    }

    fn delete(&mut self, snippet: SnippetView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = snippet.id;
        confirm_delete(
            format!("Delete snippet {}?", snippet.label),
            "Hosts that run it on connect will stop running it. This cannot be undone.",
            move |window, cx| {
                mutate_weak(
                    &page,
                    window,
                    cx,
                    move |services| async move { services.snippets.delete(id).await },
                    report("Could not delete snippet"),
                );
            },
            window,
            cx,
        );
    }

    fn render_snippet(&self, snippet: SnippetView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = snippet.id;
        let page = cx.entity().downgrade();
        let preview: String = snippet.script.lines().take(3).collect::<Vec<_>>().join("\n");
        let run_script = snippet.script.clone();
        let menu_snippet = snippet.clone();
        let edit = snippet.clone();
        card(cx)
            .id(SharedString::from(format!("snippet-{id}")))
            .group("snippet")
            .p_3()
            .flex()
            .gap_3()
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .child(host_badge(None, Icon::new(Lucide::SquareCode), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().font_medium().truncate().child(snippet.label.clone()))
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .line_clamp(3)
                            .child(preview),
                    ),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .items_start()
                    .invisible()
                    .group_hover("snippet", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("snippet-run-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Play))
                            .tooltip("Run in terminal")
                            .on_click({
                                let page = page.clone();
                                move |_: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    let script = run_script.clone();
                                    let _ = page.update(cx, |page, cx| page.send(script, true, window, cx));
                                }
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("snippet-menu-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .dropdown_menu(move |menu, _, _| {
                                let s = menu_snippet.clone();
                                let run = (page.clone(), s.script.clone());
                                let paste = (page.clone(), s.script.clone());
                                let hosts = (page.clone(), s.clone());
                                let copy = s.script.clone();
                                let delete = (page.clone(), s.clone());
                                menu.item(PopupMenuItem::new("Run in Terminal").icon(Icon::new(Lucide::Play)).on_click(
                                    move |_, window, cx| {
                                        let (page, script) = run.clone();
                                        let _ = page.update(cx, |page, cx| page.send(script, true, window, cx));
                                    },
                                ))
                                .item(
                                    PopupMenuItem::new("Paste into Terminal")
                                        .icon(Icon::new(Lucide::ClipboardPaste))
                                        .on_click(move |_, window, cx| {
                                            let (page, script) = paste.clone();
                                            let _ = page.update(cx, |page, cx| page.send(script, false, window, cx));
                                        }),
                                )
                                .item(PopupMenuItem::new("Run on Hosts…").icon(Icon::new(Lucide::Server)).on_click(
                                    move |_, window, cx| {
                                        let (page, snippet) = hosts.clone();
                                        let workspace = page.upgrade().map(|p| p.read(cx).workspace.clone());
                                        if let Some(workspace) = workspace {
                                            HostPicker::open(workspace, snippet, window, cx);
                                        }
                                    },
                                ))
                                .item(PopupMenuItem::new("Copy").icon(IconName::Copy).on_click(move |_, _, cx| {
                                    copy_text(copy.clone(), cx)
                                }))
                                .separator()
                                .item(PopupMenuItem::new("Delete").icon(IconName::Delete).on_click(
                                    move |_, window, cx| {
                                        let (page, snippet) = delete.clone();
                                        let _ = page.update(cx, |page, cx| page.delete(snippet, window, cx));
                                    },
                                ))
                            }),
                    ),
            )
            .on_click(move |_, window, cx| SnippetEditor::open(Some(edit.clone()), window, cx))
    }
}

impl Render for SnippetsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let query = self.search.read(cx).value().trim().to_lowercase();
        let all = Store::global(cx).read(cx).snippets.clone();
        let total = all.len();
        let snippets: Vec<SnippetView> = all
            .into_iter()
            .filter(|s| {
                query.is_empty() || s.label.to_lowercase().contains(&query) || s.script.to_lowercase().contains(&query)
            })
            .collect();

        let header = page_header("Snippets", cx)
            .child(
                div().w(px(280.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size(px(14.))),
                ),
            )
            .child(
                Button::new("new-snippet")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("New Snippet")
                    .on_click(|_, window, cx| SnippetEditor::open(None, window, cx)),
            );

        let body = if total == 0 {
            empty_state(
                Icon::new(Lucide::SquareCode),
                "No snippets yet",
                "Save commands you type often. Run them in a terminal, on many hosts at once, or every time you connect to a host.",
                cx,
            )
            .child(
                div().pt_2().child(
                    Button::new("empty-snippet")
                        .primary()
                        .icon(IconName::Plus)
                        .label("New Snippet")
                        .on_click(|_, window, cx| SnippetEditor::open(None, window, cx)),
                ),
            )
            .into_any_element()
        } else if snippets.is_empty() {
            v_flex().px_5().py_4().child(subtle("No snippets match.", cx)).into_any_element()
        } else {
            v_flex()
                .px_5()
                .py_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child(format!("Snippets ({})", snippets.len())),
                )
                .child(
                    div()
                        .grid()
                        .grid_cols(2)
                        .gap_3()
                        .children(snippets.into_iter().map(|s| self.render_snippet(s, cx))),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .child(header)
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }
}

// ---- editor ----

/// The form for adding or changing a snippet, shown in a side sheet.
pub struct SnippetEditor {
    id: Option<Id>,
    label: Entity<InputState>,
    script: Entity<TextareaState>,
    error: Option<String>,
}

impl SnippetEditor {
    pub fn open(snippet: Option<SnippetView>, window: &mut Window, cx: &mut App) {
        let s = snippet.as_ref();
        let editor = cx.new(|cx| Self {
            id: s.map(|s| s.id),
            label: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("e.g. Disk usage")
                    .default_value(s.map(|s| s.label.clone()).unwrap_or_default())
            }),
            script: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(14)
                    .placeholder("df -h")
                    .default_value(s.map(|s| s.script.clone()).unwrap_or_default())
            }),
            error: None,
        });
        let title = if snippet.is_some() { "Edit Snippet" } else { "New Snippet" };
        let first = std::cell::Cell::new(true);
        window.open_sheet(cx, move |sheet, window, cx| {
            if first.replace(false) {
                let input = editor.read(cx).label.clone();
                input.update(cx, |input, cx| input.focus(window, cx));
            }
            let save = editor.clone();
            sheet
                .title(title)
                .size(px(480.))
                .child(editor.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("snippet-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_sheet(cx)),
                        )
                        .child(
                            Button::new("snippet-save")
                                .primary()
                                .label("Save")
                                .on_click(move |_, window, cx| save.update(cx, |e, cx| e.save(window, cx))),
                        ),
                )
        });
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let script = self.script.read(cx).value().trim_end().to_owned();
        if script.trim().is_empty() {
            self.error = Some("Enter the commands to run.".into());
            cx.notify();
            return;
        }
        let mut label = self.label.read(cx).value().trim().to_owned();
        if label.is_empty() {
            label = script.lines().next().unwrap_or_default().chars().take(40).collect();
        }
        let input = SnippetInput {
            id: self.id,
            label,
            script,
        };
        self.error = None;
        mutate(
            window,
            cx,
            move |services| async move { services.snippets.save(input).await },
            |editor, result, window, cx| {
                match result {
                    Ok(_) => window.close_sheet(cx),
                    Err(err) => editor.error = Some(err.to_string()),
                }
                cx.notify();
            },
        );
    }
}

impl Render for SnippetEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .gap_4()
            .overflow_y_scrollbar()
            .child(labeled("Label", Input::new(&self.label)))
            .child(labeled(
                "Script",
                div().font_family(theme.mono_font_family.clone()).child(Textarea::new(&self.script)),
            ))
            .child(subtle("Each line is sent as you would type it, followed by Enter.", cx))
            .when_some(self.error.clone(), |col, err| {
                col.child(div().text_sm().text_color(theme.danger).child(err))
            })
    }
}

// ---- run on hosts ----

/// Picks the hosts to run a snippet on.
struct HostPicker {
    chosen: HashSet<Id>,
    search: Entity<InputState>,
    _subscription: Subscription,
}

impl HostPicker {
    fn open(workspace: WeakEntity<Workspace>, snippet: SnippetView, window: &mut Window, cx: &mut App) {
        let picker = cx.new(|cx| {
            let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search hosts"));
            let subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
            Self {
                chosen: HashSet::new(),
                search,
                _subscription: subscription,
            }
        });
        let focus = picker.read(cx).search.focus_handle(cx);
        let body = picker.clone();
        form_dialog(
            format!("Run {} on…", snippet.label),
            "Run",
            460.,
            Some(focus),
            move |_, _| body.clone().into_any_element(),
            move |window, cx| {
                let hosts: Vec<Id> = {
                    let chosen = &picker.read(cx).chosen;
                    // Keep the on-screen order so tabs open in a sensible order.
                    Store::global(cx)
                        .read(cx)
                        .hosts
                        .iter()
                        .filter(|h| chosen.contains(&h.id))
                        .map(|h| h.id)
                        .collect()
                };
                if hosts.is_empty() {
                    return false;
                }
                let script = snippet.script.clone();
                let _ = workspace.update(cx, |workspace, cx| workspace.run_on_hosts(hosts, script, window, cx));
                true
            },
            window,
            cx,
        );
    }
}

impl Render for HostPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let mut hosts = Store::global(cx).read(cx).hosts.clone();
        hosts.sort_by_key(|h| h.label.to_lowercase());
        let hosts: Vec<_> = hosts
            .into_iter()
            .filter(|h| query.is_empty() || crate::ui::filter::host_matches(h, &query))
            .collect();
        let count = self.chosen.len();
        v_flex()
            .gap_3()
            .child(Input::new(&self.search).small().prefix(Icon::new(IconName::Search).size(px(14.))))
            .child(
                v_flex()
                    .id("host-picker")
                    .max_h(px(300.))
                    .overflow_y_scrollbar()
                    .gap_2()
                    .when(hosts.is_empty(), |col| col.child(subtle("No hosts.", cx)))
                    .children(hosts.into_iter().map(|host| {
                        let id = host.id;
                        Checkbox::new(SharedString::from(format!("pick-{id}")))
                            .checked(self.chosen.contains(&id))
                            .label(host.label.clone())
                            .on_click(cx.listener(move |picker, checked: &bool, _, cx| {
                                if *checked {
                                    picker.chosen.insert(id);
                                } else {
                                    picker.chosen.remove(&id);
                                }
                                cx.notify();
                            }))
                    })),
            )
            .child(subtle(
                match count {
                    0 => "Pick at least one host. Each one opens in a new tab.".to_owned(),
                    1 => "1 host picked.".to_owned(),
                    n => format!("{n} hosts picked."),
                },
                cx,
            ))
    }
}
