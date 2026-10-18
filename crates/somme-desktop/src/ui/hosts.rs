use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::Select;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use somme_application::dto::{GroupInput, GroupView, HostView};
use somme_domain::Id;

use crate::host_os;
use crate::store::{Store, mutate};
use crate::ui::common::{
    Lucide, card, confirm_delete, empty_state, host_badge, notify_error, notify_ok, page_header, subtle,
};
use crate::ui::fields::{choice_select, group_choices, selected};
use crate::ui::filter::{host_matches, parse_quick_connect};
use crate::ui::host_editor::HostEditor;
use crate::ui::workspace::Workspace;

pub struct HostsPage {
    workspace: WeakEntity<Workspace>,
    search: Entity<InputState>,
    /// The group being browsed; `None` is the top level.
    group: Option<Id>,
    tag: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl HostsPage {
    pub fn new(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search hosts or type user@host to connect"));
        let subscriptions = vec![
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |page, _, event, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => page.connect_first(window, cx),
                _ => {}
            }),
        ];
        Self {
            workspace,
            search,
            group: None,
            tag: None,
            _subscriptions: subscriptions,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_owned()
    }

    /// Hosts to show: everything matching the search, or the current group's hosts.
    fn visible_hosts(&self, cx: &App) -> Vec<HostView> {
        let store = Store::global(cx);
        let store = store.read(cx);
        let query = self.query(cx);
        let mut hosts: Vec<HostView> = store
            .hosts
            .iter()
            .filter(|h| {
                if !query.is_empty() {
                    host_matches(h, &query)
                } else if self.tag.is_some() {
                    true
                } else {
                    h.group_id == self.group
                }
            })
            .filter(|h| self.tag.as_ref().is_none_or(|t| h.tags.contains(t)))
            .cloned()
            .collect();
        hosts.sort_by_key(|h| h.label.to_lowercase());
        hosts
    }

    fn visible_groups(&self, cx: &App) -> Vec<GroupView> {
        if !self.query(cx).is_empty() || self.tag.is_some() {
            return Vec::new();
        }
        let store = Store::global(cx);
        let mut groups: Vec<GroupView> = store
            .read(cx)
            .groups
            .iter()
            .filter(|g| g.parent_id == self.group)
            .cloned()
            .collect();
        groups.sort_by_key(|g| g.name.to_lowercase());
        groups
    }

    fn hosts_in_group(id: Id, cx: &App) -> usize {
        Store::global(cx).read(cx).hosts.iter().filter(|h| h.group_id == Some(id)).count()
    }

    fn connect(&self, host_id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self
            .workspace
            .update(cx, |workspace, cx| workspace.connect_host(host_id, window, cx));
    }

    /// Enter in the search box: open the first match, or connect to what was typed.
    fn connect_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(host) = self.visible_hosts(cx).first() {
            self.connect(host.id, window, cx);
            return;
        }
        self.quick_connect(window, cx);
    }

    /// Saves `user@host:port` as a host, then connects to it.
    fn quick_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.query(cx);
        let group = self.group;
        let started = self
            .workspace
            .update(cx, |workspace, cx| workspace.quick_connect(&text, group, window, cx))
            .unwrap_or(false);
        if started {
            self.search.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn import_ssh_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        mutate(
            window,
            cx,
            |services| async move { services.import.import_ssh_config().await },
            |_, result, window, cx| match result {
                Ok(0) => notify_ok(cx, "No new hosts found in ~/.ssh/config"),
                Ok(n) => notify_ok(cx, format!("Imported {n} hosts from ~/.ssh/config")),
                Err(err) => notify_error(window, cx, "Import failed", err),
            },
        );
    }

    fn delete_host(&mut self, host: HostView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = host.id;
        confirm_delete(
            format!("Delete {}?", host.label),
            "Its port forwarding rules are deleted too. This cannot be undone.",
            move |window, cx| {
                let _ = page.update(cx, |_, cx| {
                    mutate(
                        window,
                        cx,
                        move |services| async move { services.hosts.delete(id).await },
                        |_, result, window, cx| {
                            if let Err(err) = result {
                                notify_error(window, cx, "Could not delete host", err);
                            }
                        },
                    );
                });
            },
            window,
            cx,
        );
    }

    fn edit_group(&mut self, group: Option<GroupView>, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Group name")
                .default_value(group.as_ref().map(|g| g.name.clone()).unwrap_or_default())
        });
        let parent_value = group.as_ref().map(|g| g.parent_id).unwrap_or(self.group);
        let own_id = group.as_ref().map(|g| g.id);
        let choices = group_choices(cx)
            .into_iter()
            .filter(|c| c.value.is_none() || c.value != own_id)
            .map(|mut c| {
                if c.value.is_none() {
                    c.label = "Top level".into();
                }
                c
            })
            .collect();
        let parent = choice_select(choices, &parent_value, window, cx);
        let page = cx.entity().downgrade();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                name.update(cx, |input, cx| input.focus(window, cx));
            }
            let save = {
                let name = name.clone();
                let parent = parent.clone();
                let page = page.clone();
                move |window: &mut Window, cx: &mut App| {
                    let text = name.read(cx).value().trim().to_owned();
                    if text.is_empty() {
                        return false;
                    }
                    let input = GroupInput {
                        id: own_id,
                        name: text,
                        parent_id: selected(&parent, cx).flatten(),
                    };
                    let _ = page.update(cx, |_, cx| {
                        mutate(
                            window,
                            cx,
                            move |services| async move { services.groups.save(input).await },
                            |_, result, window, cx| {
                                if let Err(err) = result {
                                    notify_error(window, cx, "Could not save group", err);
                                }
                            },
                        );
                    });
                    true
                }
            };
            let save_ok = save.clone();
            dialog
                .title(if own_id.is_some() { "Rename Group" } else { "New Group" })
                .w(px(420.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(Input::new(&name))
                        .child(v_flex().gap_1().child(div().text_sm().child("Inside")).child(Select::new(&parent))),
                )
                .on_ok(move |_, window, cx| save_ok(window, cx))
                .footer(
                    DialogFooter::new()
                        .child(Button::new("group-cancel").label("Cancel").on_click(|_, w, cx| w.close_dialog(cx)))
                        .child(Button::new("group-save").primary().label("Save").on_click(move |_, window, cx| {
                            if save(window, cx) {
                                window.close_dialog(cx);
                            }
                        })),
                )
        });
    }

    fn delete_group(&mut self, group: GroupView, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.entity().downgrade();
        let id = group.id;
        confirm_delete(
            format!("Delete group {}?", group.name),
            "Hosts inside it are kept and moved out of the group.",
            move |window, cx| {
                let _ = page.update(cx, |page, cx| {
                    if page.group == Some(id) {
                        page.group = None;
                    }
                    mutate(
                        window,
                        cx,
                        move |services| async move { services.groups.delete(id).await },
                        |_, result, window, cx| {
                            if let Err(err) = result {
                                notify_error(window, cx, "Could not delete group", err);
                            }
                        },
                    );
                });
            },
            window,
            cx,
        );
    }

    // ---- rendering ----

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let group = self.group;
        page_header("Hosts", cx)
            .child(
                div().w(px(340.)).child(
                    Input::new(&self.search)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).size(px(14.))),
                ),
            )
            .child(
                Button::new("new-host")
                    .primary()
                    .small()
                    .icon(IconName::Plus)
                    .label("New Host")
                    .on_click(move |_, window, cx| HostEditor::open(None, group, false, window, cx)),
            )
            .child(
                Button::new("hosts-more")
                    .small()
                    .icon(IconName::Ellipsis)
                    .dropdown_menu({
                        let page = cx.entity().downgrade();
                        move |menu, _, _| {
                            let new_group = page.clone();
                            let import = page.clone();
                            menu.item(PopupMenuItem::new("New Group").icon(IconName::FolderClosed).on_click(
                                move |_, window, cx| {
                                    let _ = new_group.update(cx, |page, cx| page.edit_group(None, window, cx));
                                },
                            ))
                            .item(PopupMenuItem::new("Import ~/.ssh/config").icon(IconName::Inbox).on_click(
                                move |_, window, cx| {
                                    let _ = import.update(cx, |page, cx| page.import_ssh_config(window, cx));
                                },
                            ))
                        }
                    }),
            )
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let store = Store::global(cx);
        let tags = store.read(cx).tags();
        let mut crumbs = vec![(None, SharedString::from("All hosts"))];
        let mut current = self.group;
        let mut chain = Vec::new();
        while let Some(id) = current {
            let Some(group) = store.read(cx).group(id) else { break };
            chain.push((Some(id), SharedString::from(group.name.clone())));
            current = group.parent_id;
            if chain.len() > 16 {
                break;
            }
        }
        chain.reverse();
        crumbs.extend(chain);
        let last = crumbs.len() - 1;

        h_flex()
            .px_5()
            .py_2()
            .gap_1()
            .flex_wrap()
            .items_center()
            .children(crumbs.into_iter().enumerate().flat_map(|(ix, (id, name))| {
                let mut parts: Vec<gpui_kit::AnyElement> = Vec::new();
                if ix > 0 {
                    parts.push(
                        Icon::new(IconName::ChevronRight)
                            .size(px(14.))
                            .text_color(theme.muted_foreground)
                            .into_any_element(),
                    );
                }
                parts.push(
                    Button::new(("crumb", ix))
                        .ghost()
                        .xsmall()
                        .label(name)
                        .when(ix == last && self.tag.is_none(), |b| b.selected(true))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.group = id;
                            page.tag = None;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                parts
            }))
            .when(!tags.is_empty(), |row| {
                row.child(div().w(px(1.)).h(px(16.)).mx_2().bg(theme.border)).children(tags.into_iter().map(|tag| {
                    let active = self.tag.as_ref() == Some(&tag);
                    let value = tag.clone();
                    Button::new(SharedString::from(format!("tag-{tag}")))
                        .xsmall()
                        .when(active, |b| b.primary())
                        .when(!active, |b| b.ghost())
                        .icon(Icon::new(Lucide::Tag))
                        .label(tag)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.tag = if page.tag.as_ref() == Some(&value) { None } else { Some(value.clone()) };
                            cx.notify();
                        }))
                }))
            })
    }

    fn render_group(&self, group: GroupView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let count = Self::hosts_in_group(group.id, cx);
        let id = group.id;
        let page = cx.entity().downgrade();
        card(cx)
            .id(SharedString::from(format!("group-{id}")))
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .child(host_badge(None, Icon::new(IconName::FolderClosed), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().truncate().child(group.name.clone()))
                    .child(subtle(format!("{count} hosts"), cx)),
            )
            .child(
                Button::new(SharedString::from(format!("group-menu-{id}")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Ellipsis)
                    .dropdown_menu(move |menu, _, _| {
                        let rename = (page.clone(), group.clone());
                        let delete = (page.clone(), group.clone());
                        menu.item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                            let (page, group) = rename.clone();
                            let _ = page.update(cx, |page, cx| page.edit_group(Some(group), window, cx));
                        }))
                        .item(PopupMenuItem::new("Delete").on_click(move |_, window, cx| {
                            let (page, group) = delete.clone();
                            let _ = page.update(cx, |page, cx| page.delete_group(group, window, cx));
                        }))
                    }),
            )
            .on_click(cx.listener(move |page, _, _, cx| {
                page.group = Some(id);
                cx.notify();
            }))
    }

    fn render_host(&self, host: HostView, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let id = host.id;
        let user = host.username.clone().unwrap_or_default();
        let address = if host.port == 22 { host.address.clone() } else { format!("{}:{}", host.address, host.port) };
        let subtitle = if user.is_empty() { format!("ssh, {address}") } else { format!("ssh, {user}@{address}") };
        let page = cx.entity().downgrade();
        let menu_host = host.clone();
        card(cx)
            .id(SharedString::from(format!("host-{id}")))
            .group("host")
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .cursor_pointer()
            .hover(|el| el.bg(theme.muted))
            .child(host_os::badge(&host, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(div().font_medium().truncate().child(host.label.clone()))
                    .child(subtle(subtitle, cx))
                    .when(!host.tags.is_empty(), |col| {
                        col.child(
                            h_flex().gap_1().children(host.tags.iter().take(4).map(|tag| {
                                div()
                                    .px_1p5()
                                    .rounded_sm()
                                    .text_xs()
                                    .bg(theme.muted)
                                    .text_color(theme.muted_foreground)
                                    .child(tag.clone())
                            })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_0p5()
                    .invisible()
                    .group_hover("host", |el| el.visible())
                    .child(
                        Button::new(SharedString::from(format!("edit-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Pencil))
                            .tooltip("Edit")
                            .on_click({
                                let host = host.clone();
                                move |_: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    HostEditor::open(Some(host.clone()), None, false, window, cx);
                                }
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("more-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .dropdown_menu(move |menu, _, _| {
                                let connect = page.clone();
                                let files = page.clone();
                                let duplicate = menu_host.clone();
                                let delete = (page.clone(), menu_host.clone());
                                menu.item(PopupMenuItem::new("Connect").icon(Icon::new(Lucide::Plug)).on_click(
                                    move |_, window, cx| {
                                        let _ = connect.update(cx, |page, cx| page.connect(id, window, cx));
                                    },
                                ))
                                .item(PopupMenuItem::new("Open in SFTP").icon(Icon::new(Lucide::FolderOpen)).on_click(
                                    move |_, window, cx| {
                                        let _ = files.update(cx, |page, cx| {
                                            let _ = page
                                                .workspace
                                                .update(cx, |workspace, cx| workspace.open_sftp(id, window, cx));
                                        });
                                    },
                                ))
                                .item(PopupMenuItem::new("Duplicate").icon(IconName::Copy).on_click(
                                    move |_, window, cx| {
                                        HostEditor::open(Some(duplicate.clone()), None, true, window, cx);
                                    },
                                ))
                                .separator()
                                .item(PopupMenuItem::new("Delete").icon(IconName::Delete).on_click(
                                    move |_, window, cx| {
                                        let (page, host) = delete.clone();
                                        let _ = page.update(cx, |page, cx| page.delete_host(host, window, cx));
                                    },
                                ))
                            }),
                    ),
            )
            .on_click(cx.listener(move |page, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    page.connect(id, window, cx);
                }
            }))
    }
}

impl Render for HostsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hosts = self.visible_hosts(cx);
        let groups = self.visible_groups(cx);
        let query = self.query(cx);
        let theme = cx.theme().clone();
        let total = Store::global(cx).read(cx).hosts.len();

        let quick = (!query.is_empty())
            .then(|| parse_quick_connect(&query))
            .flatten()
            .filter(|_| hosts.is_empty());

        let mut body = v_flex().px_5().pb_6().gap_5();
        if let Some((user, address, port)) = quick.clone() {
            let target = match user {
                Some(user) => format!("{user}@{address}:{port}"),
                None => format!("{address}:{port}"),
            };
            body = body.child(
                card(cx)
                    .id("quick-connect")
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.muted))
                    .child(host_badge(None, Icon::new(Lucide::Zap), cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().font_medium().child(format!("Connect to {target}")))
                            .child(subtle("Press Enter. The host is saved so you can reuse it.", cx)),
                    )
                    .on_click(cx.listener(|page, _, window, cx| page.quick_connect(window, cx))),
            );
        }
        if !groups.is_empty() {
            body = body.child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().text_color(theme.muted_foreground).child("Groups"))
                    .child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap_3()
                            .children(groups.into_iter().map(|g| self.render_group(g, cx))),
                    ),
            );
        }
        if !hosts.is_empty() {
            body = body.child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().text_color(theme.muted_foreground).child("Hosts"))
                    .child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap_3()
                            .children(hosts.into_iter().map(|h| self.render_host(h, cx))),
                    ),
            );
        } else if quick.is_none() && total == 0 {
            let group = self.group;
            body = body.child(
                empty_state(
                    Icon::new(Lucide::Server),
                    "No hosts yet",
                    "Add a server to connect to, or import the hosts from your ~/.ssh/config.",
                    cx,
                )
                .child(
                    h_flex()
                        .gap_2()
                        .pt_2()
                        .child(
                            Button::new("empty-new")
                                .primary()
                                .icon(IconName::Plus)
                                .label("New Host")
                                .on_click(move |_, window, cx| HostEditor::open(None, group, false, window, cx)),
                        )
                        .child(
                            Button::new("empty-import")
                                .icon(IconName::Inbox)
                                .label("Import ~/.ssh/config")
                                .on_click(cx.listener(|page, _, window, cx| page.import_ssh_config(window, cx))),
                        ),
                ),
            );
        } else if quick.is_none() {
            body = body.child(subtle(if query.is_empty() { "No hosts here." } else { "No hosts match." }, cx));
        }

        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .child(self.render_filters(cx))
            .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
    }
}
