use std::collections::BTreeSet;
use std::future::Future;

use chrono::{Local, TimeZone as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement as _, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::FluentBuilder as _, px, uniform_list,
};
use somme_application::dto::SftpEntryView;
use somme_domain::Id;

use crate::runtime;
use crate::sftp::{self, Endpoint, FsResult};
use crate::store::Store;
use crate::ui::common::{
    Lucide, confirm_delete, copy_text, form_dialog, host_badge, human_size, notify_error, page_header, subtle,
};
use crate::ui::filter::host_matches;

/// What a pane is showing.
enum Source {
    /// Waiting for the user to pick the local disk or a host.
    Picker,
    Local,
    Connecting { label: String },
    Remote { sftp_id: Id, label: String, home: String },
}

/// Files dragged from one pane to the other.
#[derive(Clone)]
struct DraggedEntries {
    side: usize,
    entries: Vec<SftpEntryView>,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_sm()
            .child(Icon::new(Lucide::Files).size(px(14.)))
            .child(self.0.clone())
    }
}

enum PaneEvent {
    /// Copy these to the other pane.
    Send(Vec<SftpEntryView>),
    /// Copy these from the other pane into this one.
    Receive(Vec<SftpEntryView>),
    /// Files dropped in from Finder.
    ReceiveLocal(Vec<SftpEntryView>),
}

/// One side of the SFTP screen: a folder on this computer or on a host.
struct FilePane {
    side: usize,
    source: Source,
    /// Bumped whenever the source changes, so late answers for an old source are ignored.
    generation: usize,
    path: String,
    /// Where to go back to when a link turns out not to be a folder.
    fallback: Option<String>,
    entries: Vec<SftpEntryView>,
    /// `entries` without hidden files, unless they are shown.
    rows: Vec<SftpEntryView>,
    selected: BTreeSet<String>,
    anchor: Option<usize>,
    loading: bool,
    error: Option<String>,
    show_hidden: bool,
    path_input: Entity<InputState>,
    host_search: Entity<InputState>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaneEvent> for FilePane {}

impl FilePane {
    fn new(side: usize, local: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path_input = cx.new(|cx| InputState::new(window, cx).placeholder("Path"));
        let host_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search hosts"));
        let subscriptions = vec![
            cx.subscribe_in(&path_input, window, |pane, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let path = input.read(cx).value().trim().to_owned();
                    if !path.is_empty() {
                        pane.navigate(path, window, cx);
                    }
                }
            }),
            cx.subscribe(&host_search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
        ];
        let mut pane = Self {
            side,
            source: Source::Picker,
            generation: 0,
            path: String::new(),
            fallback: None,
            entries: Vec::new(),
            rows: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            loading: false,
            error: None,
            show_hidden: false,
            path_input,
            host_search,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        if local {
            pane.open_local(window, cx);
        }
        pane
    }

    fn endpoint(&self) -> Option<Endpoint> {
        match &self.source {
            Source::Local => Some(Endpoint::Local),
            Source::Remote { sftp_id, .. } => Some(Endpoint::Remote(*sftp_id)),
            Source::Picker | Source::Connecting { .. } => None,
        }
    }

    fn current_dir(&self) -> Option<(Endpoint, String)> {
        self.endpoint().map(|endpoint| (endpoint, self.path.clone()))
    }

    fn label(&self) -> String {
        match &self.source {
            Source::Picker => "Choose…".to_owned(),
            Source::Local => "Local".to_owned(),
            Source::Connecting { label } | Source::Remote { label, .. } => label.clone(),
        }
    }

    /// Leaves the current source and closes its SFTP session.
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        if let Source::Remote { sftp_id, .. } = self.source {
            let services = Store::services(cx);
            runtime::handle().spawn(async move { services.sftp.close(sftp_id).await });
        }
        self.source = Source::Picker;
        self.entries.clear();
        self.rows.clear();
        self.selected.clear();
        self.anchor = None;
        self.loading = false;
        self.error = None;
        cx.notify();
    }

    fn open_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.disconnect(cx);
        self.source = Source::Local;
        self.navigate(sftp::local_home(), window, cx);
    }

    fn open_host(&mut self, host_id: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = Store::global(cx).read(cx).host(host_id).cloned() else {
            return;
        };
        self.disconnect(cx);
        self.source = Source::Connecting {
            label: host.label.clone(),
        };
        let generation = self.generation;
        let services = Store::services(cx);
        cx.spawn_in(window, async move |pane, cx| {
            let sftp = services.sftp.clone();
            let result = runtime::io(async move { sftp.open(host_id).await }).await;
            let _ = pane.update_in(cx, |pane, window, cx| {
                if pane.generation != generation {
                    // The user moved on while this was connecting.
                    if let Ok(opened) = result {
                        runtime::handle().spawn(async move { services.sftp.close(opened.sftp_id).await });
                    }
                    return;
                }
                match result {
                    Ok(opened) => {
                        pane.source = Source::Remote {
                            sftp_id: opened.sftp_id,
                            label: host.label.clone(),
                            home: opened.home.clone(),
                        };
                        pane.navigate(opened.home, window, cx);
                    }
                    Err(err) => {
                        pane.source = Source::Picker;
                        pane.error = Some(format!("Could not connect to {}: {err}", host.label));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn navigate(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.path = path.clone();
        self.selected.clear();
        self.anchor = None;
        self.path_input.update(cx, |input, cx| input.set_value(path, window, cx));
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(window, cx);
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(endpoint) = self.endpoint() else {
            return;
        };
        self.loading = true;
        cx.notify();
        let path = self.path.clone();
        let services = Store::services(cx);
        cx.spawn_in(window, async move |pane, cx| {
            let request = path.clone();
            let result = runtime::io(async move { endpoint.list(&services, &request).await }).await;
            let _ = pane.update_in(cx, |pane, window, cx| {
                if pane.path != path || pane.endpoint() != Some(endpoint) {
                    return;
                }
                pane.loading = false;
                match result {
                    Ok(entries) => {
                        pane.fallback = None;
                        pane.entries = entries;
                        pane.error = None;
                    }
                    Err(err) => {
                        if let Some(previous) = pane.fallback.take() {
                            notify_error(window, cx, "Could not open", err);
                            pane.navigate(previous, window, cx);
                            return;
                        }
                        pane.entries.clear();
                        pane.error = Some(err);
                    }
                }
                pane.apply_filter();
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_filter(&mut self) {
        let show_hidden = self.show_hidden;
        self.rows = self
            .entries
            .iter()
            .filter(|e| show_hidden || !e.name.starts_with('.'))
            .cloned()
            .collect();
        let rows = &self.rows;
        self.selected.retain(|path| rows.iter().any(|e| &e.path == path));
        self.anchor = None;
    }

    fn toggle_hidden(&mut self, cx: &mut Context<Self>) {
        self.show_hidden = !self.show_hidden;
        self.apply_filter();
        cx.notify();
    }

    fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = self.endpoint().and_then(|endpoint| endpoint.parent(&self.path)) {
            self.navigate(parent, window, cx);
        }
    }

    fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = match &self.source {
            Source::Local => sftp::local_home(),
            Source::Remote { home, .. } => home.clone(),
            Source::Picker | Source::Connecting { .. } => return,
        };
        self.navigate(home, window, cx);
    }

    /// Double-click: folders open, files are copied to the other pane.
    fn open_entry(&mut self, entry: &SftpEntryView, window: &mut Window, cx: &mut Context<Self>) {
        if entry.is_dir {
            self.navigate(entry.path.clone(), window, cx);
        } else if entry.is_symlink {
            // Remote links do not say what they point to. Try it as a folder first.
            self.fallback = Some(self.path.clone());
            self.navigate(entry.path.clone(), window, cx);
        } else {
            cx.emit(PaneEvent::Send(vec![entry.clone()]));
        }
    }

    fn selected_entries(&self) -> Vec<SftpEntryView> {
        self.rows
            .iter()
            .filter(|e| self.selected.contains(&e.path))
            .cloned()
            .collect()
    }

    fn click_row(&mut self, ix: usize, event: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(entry) = self.rows.get(ix).cloned() else {
            return;
        };
        if event.click_count() >= 2 {
            self.open_entry(&entry, window, cx);
            return;
        }
        let modifiers = event.modifiers();
        match self.anchor {
            Some(anchor) if modifiers.shift => {
                let range = anchor.min(ix)..=anchor.max(ix);
                self.selected = self.rows[range].iter().map(|e| e.path.clone()).collect();
            }
            _ if modifiers.secondary() => {
                if !self.selected.remove(&entry.path) {
                    self.selected.insert(entry.path);
                }
                self.anchor = Some(ix);
            }
            _ => {
                self.selected = BTreeSet::from([entry.path]);
                self.anchor = Some(ix);
            }
        }
        cx.notify();
    }

    /// A right-click on a row outside the selection acts on that row alone.
    fn select_for_menu(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if let Some(entry) = self.rows.get(ix)
            && !self.selected.contains(&entry.path)
        {
            self.selected = BTreeSet::from([entry.path.clone()]);
            self.anchor = Some(ix);
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed in the path box belong to the path box.
        if !self.focus.is_focused(window) {
            return;
        }
        let modifiers = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "backspace" if modifiers.secondary() => self.delete_selected(window, cx),
            "delete" => self.delete_selected(window, cx),
            "backspace" => self.go_up(window, cx),
            "enter" => {
                if let [entry] = self.selected_entries().as_slice() {
                    self.open_entry(entry, window, cx);
                }
            }
            "a" if modifiers.secondary() => {
                self.selected = self.rows.iter().map(|e| e.path.clone()).collect();
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Runs a change on the files, then shows the folder again.
    fn run_op(
        &mut self,
        title: &'static str,
        op: impl Future<Output = FsResult<()>> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |pane, cx| {
            let result = runtime::io(op).await;
            let _ = pane.update_in(cx, |pane, window, cx| {
                if let Err(err) = result {
                    notify_error(window, cx, title, err);
                }
                pane.reload(window, cx);
            });
        })
        .detach();
    }

    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((endpoint, dir)) = self.current_dir() else {
            return;
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let pane = cx.entity().downgrade();
        let focus = name.focus_handle(cx);
        form_dialog(
            "New Folder",
            "Create",
            380.,
            Some(focus),
            {
                let name = name.clone();
                move |_, _| Input::new(&name).into_any_element()
            },
            move |window, cx| {
                let text = name.read(cx).value().trim().to_owned();
                if !valid_name(&text) {
                    return false;
                }
                let services = Store::services(cx);
                let path = endpoint.join(&dir, &text);
                let _ = pane.update(cx, |pane, cx| {
                    let op = async move { endpoint.mkdir(&services, &path).await };
                    pane.run_op("Could not create folder", op, window, cx);
                });
                true
            },
            window,
            cx,
        );
    }

    fn rename(&mut self, entry: SftpEntryView, window: &mut Window, cx: &mut Context<Self>) {
        let Some((endpoint, dir)) = self.current_dir() else {
            return;
        };
        let name = cx.new(|cx| InputState::new(window, cx).default_value(entry.name.clone()));
        let pane = cx.entity().downgrade();
        let focus = name.focus_handle(cx);
        form_dialog(
            "Rename",
            "Rename",
            380.,
            Some(focus),
            {
                let name = name.clone();
                move |_, _| Input::new(&name).into_any_element()
            },
            move |window, cx| {
                let text = name.read(cx).value().trim().to_owned();
                if !valid_name(&text) {
                    return false;
                }
                if text == entry.name {
                    return true;
                }
                let services = Store::services(cx);
                let from = entry.path.clone();
                let to = endpoint.join(&dir, &text);
                let _ = pane.update(cx, |pane, cx| {
                    let op = async move { endpoint.rename(&services, &from, &to).await };
                    pane.run_op("Could not rename", op, window, cx);
                });
                true
            },
            window,
            cx,
        );
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.selected_entries();
        let Some(endpoint) = self.endpoint() else {
            return;
        };
        let title = match entries.as_slice() {
            [] => return,
            [entry] => format!("Delete {}?", entry.name),
            many => format!("Delete {} items?", many.len()),
        };
        let pane = cx.entity().downgrade();
        confirm_delete(
            title,
            "Folders are deleted with everything in them. This cannot be undone.",
            move |window, cx| {
                let services = Store::services(cx);
                let entries = entries.clone();
                let _ = pane.update(cx, |pane, cx| {
                    let op = async move {
                        for entry in &entries {
                            endpoint.remove(&services, entry).await?;
                        }
                        Ok(())
                    };
                    pane.run_op("Could not delete", op, window, cx);
                });
            },
            window,
            cx,
        );
    }

    fn send_selected(&mut self, cx: &mut Context<Self>) {
        let entries = self.selected_entries();
        if !entries.is_empty() {
            cx.emit(PaneEvent::Send(entries));
        }
    }

    fn row_menu(pane: WeakEntity<Self>, entry: SftpEntryView, side: usize) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| {
            let Some(count) = pane.upgrade().map(|p| p.read(cx).selected.len().max(1)) else {
                return menu;
            };
            let single = count == 1;
            let send_label = match (side, count) {
                (0, 1) => "Copy to Right".to_owned(),
                (_, 1) => "Copy to Left".to_owned(),
                (0, n) => format!("Copy {n} Items to Right"),
                (_, n) => format!("Copy {n} Items to Left"),
            };
            let send_icon = if side == 0 { Lucide::ArrowRight } else { Lucide::ArrowLeft };
            let (open, send, rename, path, delete) =
                (pane.clone(), pane.clone(), pane.clone(), entry.path.clone(), pane.clone());
            let open_entry = entry.clone();
            let rename_entry = entry.clone();
            menu.when(single && (entry.is_dir || entry.is_symlink), |menu| {
                menu.item(PopupMenuItem::new("Open").icon(Icon::new(Lucide::FolderOpen)).on_click(
                    move |_, window, cx| {
                        let _ = open.update(cx, |pane, cx| pane.open_entry(&open_entry, window, cx));
                    },
                ))
            })
            .item(
                PopupMenuItem::new(send_label)
                    .icon(Icon::new(send_icon))
                    .on_click(move |_, _, cx| {
                        let _ = send.update(cx, |pane, cx| pane.send_selected(cx));
                    }),
            )
            .when(single, |menu| {
                menu.item(PopupMenuItem::new("Rename…").icon(Icon::new(Lucide::Pencil)).on_click(
                    move |_, window, cx| {
                        let _ = rename.update(cx, |pane, cx| pane.rename(rename_entry.clone(), window, cx));
                    },
                ))
                .item(
                    PopupMenuItem::new("Copy Path")
                        .icon(IconName::Copy)
                        .on_click(move |_, window, cx| copy_text(path.clone(), "Path copied", window, cx)),
                )
            })
            .separator()
            .item(PopupMenuItem::new("Delete…").icon(IconName::Delete).on_click(
                move |_, window, cx| {
                    let _ = delete.update(cx, |pane, cx| pane.delete_selected(window, cx));
                },
            ))
        }
    }

    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(entry) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let selected = self.selected.contains(&entry.path);
        let dragged = if selected { self.selected_entries() } else { vec![entry.clone()] };
        let drag_label: SharedString = match dragged.as_slice() {
            [one] => one.name.clone().into(),
            many => format!("{} items", many.len()).into(),
        };
        let icon = if entry.is_dir {
            Lucide::Folder
        } else if entry.is_symlink {
            Lucide::FileSymlink
        } else {
            Lucide::File
        };
        let pane = cx.entity().downgrade();
        h_flex()
            .id(SharedString::from(format!("sftp-{}-{}", self.side, entry.path)))
            .w_full()
            .h(px(30.))
            .px_3()
            .gap_2()
            .text_sm()
            .when(selected, |el| el.bg(theme.accent))
            .when(!selected, |el| el.hover(|el| el.bg(theme.muted)))
            .on_click(cx.listener(move |pane, event: &ClickEvent, window, cx| pane.click_row(ix, event, window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |pane, _, window, cx| pane.select_for_menu(ix, window, cx)),
            )
            .on_drag(
                DraggedEntries {
                    side: self.side,
                    entries: dragged,
                },
                move |_, _, _, cx| {
                    let label = drag_label.clone();
                    cx.new(|_| DragPreview(label))
                },
            )
            .child(
                Icon::new(icon)
                    .size(px(16.))
                    .text_color(if entry.is_dir { theme.blue } else { theme.muted_foreground }),
            )
            .child(div().flex_1().min_w_0().truncate().child(entry.name.clone()))
            .child(
                div()
                    .w(px(140.))
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(modified(entry.modified_at)),
            )
            .child(
                div()
                    .w(px(72.))
                    .flex_none()
                    .text_xs()
                    .text_right()
                    .text_color(theme.muted_foreground)
                    .child(if entry.is_dir { "—".to_owned() } else { human_size(entry.size) }),
            )
            .context_menu(Self::row_menu(pane, entry.clone(), self.side))
            .into_any_element()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let side = self.side;
        let ready = self.endpoint().is_some();
        let remote = matches!(self.source, Source::Remote { .. } | Source::Connecting { .. });
        let pane = cx.entity().downgrade();
        let source_icon = match self.source {
            Source::Local => Icon::new(Lucide::Laptop),
            _ => Icon::new(Lucide::Server),
        };
        let icon_button = |id: &'static str, icon: Icon, tooltip: &'static str| {
            Button::new((id, side)).ghost().small().icon(icon).tooltip(tooltip).disabled(!ready)
        };
        h_flex()
            .h(px(44.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new(("sftp-source", side))
                    .ghost()
                    .small()
                    .icon(source_icon)
                    .label(self.label())
                    .dropdown_caret(true)
                    .dropdown_menu(move |menu, _, _| {
                        let (local, picker, close) = (pane.clone(), pane.clone(), pane.clone());
                        menu.item(PopupMenuItem::new("Local").icon(Icon::new(Lucide::Laptop)).on_click(
                            move |_, window, cx| {
                                let _ = local.update(cx, |pane, cx| pane.open_local(window, cx));
                            },
                        ))
                        .item(PopupMenuItem::new("Choose Host…").icon(Icon::new(Lucide::Server)).on_click(
                            move |_, _, cx| {
                                let _ = picker.update(cx, |pane, cx| pane.disconnect(cx));
                            },
                        ))
                        .when(remote, |menu| {
                            menu.separator().item(
                                PopupMenuItem::new("Disconnect").icon(Icon::new(Lucide::Unplug)).on_click(
                                    move |_, _, cx| {
                                        let _ = close.update(cx, |pane, cx| pane.disconnect(cx));
                                    },
                                ),
                            )
                        })
                    }),
            )
            .child(
                icon_button("sftp-up", Icon::new(Lucide::ArrowUp), "Up one folder")
                    .on_click(cx.listener(|pane, _, window, cx| pane.go_up(window, cx))),
            )
            .child(
                icon_button("sftp-home", Icon::new(Lucide::House), "Home folder")
                    .on_click(cx.listener(|pane, _, window, cx| pane.go_home(window, cx))),
            )
            .child(
                icon_button("sftp-refresh", Icon::new(Lucide::RefreshCw), "Refresh")
                    .on_click(cx.listener(|pane, _, window, cx| pane.reload(window, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(ready, |el| el.child(Input::new(&self.path_input).small())),
            )
            .child(
                icon_button("sftp-mkdir", Icon::new(Lucide::FolderPlus), "New folder")
                    .on_click(cx.listener(|pane, _, window, cx| pane.new_folder(window, cx))),
            )
            .child(
                icon_button(
                    "sftp-hidden",
                    Icon::new(if self.show_hidden { Lucide::Eye } else { Lucide::EyeOff }),
                    if self.show_hidden { "Hide hidden files" } else { "Show hidden files" },
                )
                .on_click(cx.listener(|pane, _, _, cx| pane.toggle_hidden(cx))),
            )
    }

    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let query = self.host_search.read(cx).value().trim().to_owned();
        let mut hosts: Vec<_> = Store::global(cx)
            .read(cx)
            .hosts
            .iter()
            .filter(|h| host_matches(h, &query))
            .cloned()
            .collect();
        hosts.sort_by_key(|h| h.label.to_lowercase());
        let choice = |id: SharedString, badge: AnyElement, title: String, detail: String| {
            h_flex()
                .id(id)
                .gap_3()
                .p_2()
                .rounded_md()
                .cursor_pointer()
                .hover(|el| el.bg(theme.muted))
                .child(badge)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_sm().font_medium().truncate().child(title))
                        .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(detail)),
                )
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .p_3()
            .gap_2()
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_sm().text_color(theme.danger).child(error))
            })
            .child(
                Input::new(&self.host_search)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).size(px(14.))),
            )
            .child(
                v_flex()
                    .id(("sftp-picker", self.side))
                    .flex_1()
                    .min_h_0()
                    .gap_0p5()
                    .overflow_y_scroll()
                    .when(query.is_empty(), |list| {
                        list.child(
                            choice(
                                "pick-local".into(),
                                host_badge(None, Icon::new(Lucide::Laptop), cx).into_any_element(),
                                "Local".into(),
                                "Files on this computer".into(),
                            )
                            .on_click(cx.listener(|pane, _, window, cx| pane.open_local(window, cx))),
                        )
                    })
                    .children(hosts.into_iter().map(|host| {
                        let id = host.id;
                        let detail = match host.username.as_deref().filter(|u| !u.is_empty()) {
                            Some(user) => format!("{user}@{}", host.address),
                            None => host.address.clone(),
                        };
                        choice(
                            SharedString::from(format!("pick-{id}")),
                            host_badge(host.color.as_deref(), Icon::new(Lucide::Server), cx).into_any_element(),
                            host.label.clone(),
                            detail,
                        )
                        .on_click(cx.listener(move |pane, _, window, cx| pane.open_host(id, window, cx)))
                    })),
            )
            .into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let side = self.side;
        let body = if let Some(error) = &self.error {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .p_4()
                .child(div().text_sm().text_center().text_color(theme.danger).child(error.clone()))
                .child(
                    Button::new(("sftp-retry", side))
                        .small()
                        .label("Try Again")
                        .on_click(cx.listener(|pane, _, window, cx| pane.reload(window, cx))),
                )
                .into_any_element()
        } else if self.rows.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(if self.loading { "Loading…" } else { "This folder is empty" })
                .into_any_element()
        } else {
            uniform_list(
                ("sftp-rows", side),
                self.rows.len(),
                cx.processor(|pane, range: std::ops::Range<usize>, _, cx| {
                    range.map(|ix| pane.render_row(ix, cx)).collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .track_scroll(&self.scroll)
            .into_any_element()
        };

        v_flex()
            .id(("sftp-files", side))
            .flex_1()
            .min_h_0()
            .drag_over::<DraggedEntries>(move |style, dragged, _, cx| {
                if dragged.side == side { style } else { style.bg(cx.theme().drop_target) }
            })
            .drag_over::<ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_drop(cx.listener(move |_, dragged: &DraggedEntries, _, cx| {
                if dragged.side != side {
                    cx.emit(PaneEvent::Receive(dragged.entries.clone()));
                }
            }))
            .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                let entries: Vec<_> = paths.paths().iter().filter_map(|p| sftp::local_entry(p)).collect();
                if !entries.is_empty() {
                    cx.emit(PaneEvent::ReceiveLocal(entries));
                }
            }))
            .child(
                h_flex()
                    .h(px(26.))
                    .px_3()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().w(px(16.)))
                    .child(div().flex_1().child("Name"))
                    .child(div().w(px(140.)).child("Modified"))
                    .child(div().w(px(72.)).text_right().child("Size")),
            )
            .child(body)
            .into_any_element()
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

fn modified(millis: Option<i64>) -> String {
    millis
        .and_then(|ms| Local.timestamp_millis_opt(ms).single())
        .map(|t| t.format("%b %-d, %Y %H:%M").to_string())
        .unwrap_or_default()
}

impl Render for FilePane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = match &self.source {
            Source::Picker => self.render_picker(cx),
            Source::Connecting { label } => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Spinner::new())
                .child(subtle(format!("Connecting to {label}…"), cx))
                .into_any_element(),
            Source::Local | Source::Remote { .. } => self.render_files(cx),
        };
        v_flex()
            .size_full()
            .min_w_0()
            .bg(theme.background)
            .track_focus(&self.focus)
            .key_context("SftpPane")
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_header(cx))
            .child(body)
    }
}

enum TransferState {
    Running,
    Done(u64),
    Failed(String),
}

struct Transfer {
    name: String,
    route: String,
    state: TransferState,
}

/// Two file panes side by side. Files move between them by drag and drop or the row menu.
pub struct SftpPage {
    panes: [Entity<FilePane>; 2],
    transfers: Vec<Transfer>,
    _subscriptions: Vec<Subscription>,
}

impl SftpPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let left = cx.new(|cx| FilePane::new(0, true, window, cx));
        let right = cx.new(|cx| FilePane::new(1, false, window, cx));
        let subscriptions = [&left, &right]
            .map(|pane| cx.subscribe_in(pane, window, Self::on_pane_event))
            .into();
        Self {
            panes: [left, right],
            transfers: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Shows a host in the right pane.
    pub fn open_host(&mut self, host_id: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.panes[1].update(cx, |pane, cx| pane.open_host(host_id, window, cx));
    }

    fn on_pane_event(&mut self, pane: &Entity<FilePane>, event: &PaneEvent, window: &mut Window, cx: &mut Context<Self>) {
        let side = pane.read(cx).side;
        let other = 1 - side;
        match event {
            PaneEvent::Send(entries) => self.transfer(Some(side), other, entries.clone(), window, cx),
            PaneEvent::Receive(entries) => self.transfer(Some(other), side, entries.clone(), window, cx),
            PaneEvent::ReceiveLocal(entries) => self.transfer(None, side, entries.clone(), window, cx),
        }
    }

    /// Copies `entries` into the folder open in pane `to`. `from` is `None` for local files from Finder.
    fn transfer(
        &mut self,
        from: Option<usize>,
        to: usize,
        entries: Vec<SftpEntryView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (source, source_label) = match from {
            None => (Endpoint::Local, "Local".to_owned()),
            Some(side) => {
                let pane = self.panes[side].read(cx);
                let Some(endpoint) = pane.endpoint() else { return };
                (endpoint, pane.label())
            }
        };
        let target_pane = self.panes[to].read(cx);
        let Some((target, dest_dir)) = target_pane.current_dir() else {
            notify_error(window, cx, "Nowhere to copy to", "Open the local disk or a host in the other pane first.");
            return;
        };
        let route = format!("{source_label} → {}: {dest_dir}", target_pane.label());
        let services = Store::services(cx);
        for entry in entries {
            let ix = self.transfers.len();
            self.transfers.push(Transfer {
                name: entry.name.clone(),
                route: route.clone(),
                state: TransferState::Running,
            });
            let services = services.clone();
            let dest = dest_dir.clone();
            let pane = self.panes[to].downgrade();
            cx.spawn_in(window, async move |page, cx| {
                let copy_dest = dest.clone();
                let result =
                    runtime::io(async move { sftp::copy(&services, source, &entry, target, &copy_dest).await }).await;
                let _ = page.update(cx, |page, cx| {
                    if let Some(transfer) = page.transfers.get_mut(ix) {
                        transfer.state = match result {
                            Ok(files) => TransferState::Done(files),
                            Err(err) => TransferState::Failed(err),
                        };
                    }
                    cx.notify();
                });
                let _ = pane.update_in(cx, |pane, window, cx| {
                    if pane.current_dir() == Some((target, dest)) {
                        pane.reload(window, cx);
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn clear_finished(&mut self, cx: &mut Context<Self>) {
        // Running transfers find their row by index, so rows go only once nothing runs.
        if self.transfers.iter().all(|t| !matches!(t.state, TransferState::Running)) {
            self.transfers.clear();
        }
        cx.notify();
    }

    fn render_transfers(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let running = self.transfers.iter().any(|t| matches!(t.state, TransferState::Running));
        v_flex()
            .flex_none()
            .max_h(px(180.))
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .h(px(34.))
                    .px_3()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Transfers"))
                    .child(div().flex_1())
                    .child(
                        Button::new("sftp-clear")
                            .ghost()
                            .xsmall()
                            .label("Clear")
                            .disabled(running)
                            .on_click(cx.listener(|page, _, _, cx| page.clear_finished(cx))),
                    ),
            )
            .child(
                v_flex()
                    .id("sftp-transfers")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .pb_2()
                    .gap_1()
                    .children(self.transfers.iter().rev().map(|transfer| {
                        let (icon, status, color): (AnyElement, String, _) = match &transfer.state {
                            TransferState::Running => {
                                (Spinner::new().small().into_any_element(), "Copying…".into(), theme.muted_foreground)
                            }
                            TransferState::Done(files) => (
                                Icon::new(Lucide::CircleCheck)
                                    .size(px(14.))
                                    .text_color(theme.success)
                                    .into_any_element(),
                                match files {
                                    1 => "Done".to_owned(),
                                    n => format!("Done · {n} files"),
                                },
                                theme.muted_foreground,
                            ),
                            TransferState::Failed(err) => (
                                Icon::new(Lucide::CircleX).size(px(14.)).text_color(theme.danger).into_any_element(),
                                err.clone(),
                                theme.danger,
                            ),
                        };
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(div().w(px(16.)).flex_none().child(icon))
                            .child(div().flex_none().max_w(px(260.)).truncate().child(transfer.name.clone()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(transfer.route.clone()),
                            )
                            .child(div().flex_none().max_w(px(320.)).truncate().text_xs().text_color(color).child(status))
                    })),
            )
    }
}

impl Render for SftpPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .child(page_header("SFTP", cx).child(subtle("Drag files between the two sides to copy them.", cx)))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .border_r_1()
                            .border_color(theme.border)
                            .child(self.panes[0].clone()),
                    )
                    .child(div().flex_1().min_w_0().h_full().child(self.panes[1].clone())),
            )
            .when(!self.transfers.is_empty(), |page| page.child(self.render_transfers(cx)))
    }
}
