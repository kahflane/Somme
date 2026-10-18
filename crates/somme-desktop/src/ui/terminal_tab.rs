use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, h_resizable, resizable_panel, v_flex, v_resizable,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use crate::editor::pane::{cant_open, read_text, read_text_as_root};
use crate::editor::sudo;
use crate::editor::system_open;
use crate::editor::{EditorPane, EditorPaneEvent};
use crate::services::Services;
use crate::terminal::disk::{Disk, DiskError};
use crate::terminal::session::Status;
use crate::terminal::view::TerminalViewEvent;
use crate::terminal::{SessionTarget, TerminalSession, TerminalView};
use crate::ui::common::Lucide;

enum PaneNode {
    Leaf(Entity<TerminalView>),
    Split(Axis, Vec<PaneNode>),
}

impl PaneNode {
    fn leaves(&self, out: &mut Vec<Entity<TerminalView>>) {
        match self {
            Self::Leaf(view) => out.push(view.clone()),
            Self::Split(_, children) => children.iter().for_each(|c| c.leaves(out)),
        }
    }

    /// Puts `new` next to `target`, splitting along `axis`. `before` puts it on
    /// the left or on top.
    fn split(&mut self, target: EntityId, new: Entity<TerminalView>, axis: Axis, before: bool) -> bool {
        match self {
            Self::Leaf(view) if view.entity_id() == target => {
                let (old, new) = (Self::Leaf(view.clone()), Self::Leaf(new));
                *self = Self::Split(axis, if before { vec![new, old] } else { vec![old, new] });
                true
            }
            Self::Leaf(_) => false,
            Self::Split(split_axis, children) => {
                let found = children
                    .iter()
                    .position(|c| matches!(c, Self::Leaf(v) if v.entity_id() == target));
                if let Some(ix) = found
                    && *split_axis == axis
                {
                    children.insert(if before { ix } else { ix + 1 }, Self::Leaf(new));
                    return true;
                }
                children.iter_mut().any(|c| c.split(target, new.clone(), axis, before))
            }
        }
    }

    /// Removes a pane. Returns false when nothing is left.
    fn remove(&mut self, target: EntityId) -> bool {
        match self {
            Self::Leaf(view) => view.entity_id() != target,
            Self::Split(_, children) => {
                children.retain_mut(|c| c.remove(target));
                match children.len() {
                    0 => false,
                    1 => {
                        let only = children.pop().expect("one child");
                        *self = only;
                        true
                    }
                    _ => true,
                }
            }
        }
    }
}

pub enum TerminalTabEvent {
    Changed,
    Empty,
    /// Show the git changes of the repo that holds this folder.
    OpenChanges(PathBuf),
}

/// One tab of terminals: a tree of split panes with an optional broadcast mode.
pub struct TerminalTab {
    root: PaneNode,
    active: Entity<TerminalView>,
    pub broadcast: bool,
    pub target: SessionTarget,
    /// A name the user gave the tab. Without one the tab names the folder or the shell's title.
    pub custom_title: Option<SharedString>,
    /// A snippet to run as soon as the first pane connects.
    pending_run: Option<String>,
    services: Arc<Services>,
    subscriptions: Vec<(EntityId, Subscription)>,
    /// Files opened from the terminal, shown on the right.
    editor: Option<(Entity<EditorPane>, Subscription)>,
    /// Panes put away in the bar at the bottom. They keep running.
    minimized: Vec<Entity<TerminalView>>,
    /// The pane filling the whole tab, with the others kept behind it.
    maximized: Option<Entity<TerminalView>>,
}

impl EventEmitter<TerminalTabEvent> for TerminalTab {}

impl TerminalTab {
    pub fn new(
        target: SessionTarget,
        start_dir: Option<PathBuf>,
        services: Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::new_view(&target, start_dir, &services, window, cx);
        let mut tab = Self {
            root: PaneNode::Leaf(view.clone()),
            active: view.clone(),
            broadcast: false,
            target,
            custom_title: None,
            pending_run: None,
            services,
            subscriptions: Vec::new(),
            editor: None,
            minimized: Vec::new(),
            maximized: None,
        };
        tab.watch(&view, window, cx);
        tab
    }

    fn new_view(
        target: &SessionTarget,
        start_dir: Option<PathBuf>,
        services: &Arc<Services>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalView> {
        let session = TerminalSession::new(target.clone(), start_dir, services.clone(), cx);
        cx.new(|cx| TerminalView::new(session, window, cx))
    }

    fn watch(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        let subscription = cx.subscribe_in(view, window, |tab, view, event, window, cx| match event {
            TerminalViewEvent::Focused => {
                tab.active = view.clone();
                cx.emit(TerminalTabEvent::Changed);
                cx.notify();
            }
            TerminalViewEvent::StatusChanged => {
                let connected = view.read(cx).session.read(cx).status == Status::Connected;
                if connected && let Some(text) = tab.pending_run.take() {
                    view.update(cx, |view, cx| view.run_text(&text, cx));
                }
                cx.emit(TerminalTabEvent::Changed);
                cx.notify();
            }
            TerminalViewEvent::TitleChanged => {
                cx.emit(TerminalTabEvent::Changed);
                cx.notify();
            }
            TerminalViewEvent::OpenFile(path) => tab.open_file(view, path.clone(), window, cx),
            TerminalViewEvent::OpenChanges(dir) => cx.emit(TerminalTabEvent::OpenChanges(dir.clone())),
            TerminalViewEvent::Typed(bytes) if tab.broadcast => {
                for other in tab.panes() {
                    if other != *view {
                        let bytes = bytes.clone();
                        other.update(cx, |other, cx| other.session.update(cx, |s, cx| s.input(bytes, cx)));
                    }
                }
            }
            TerminalViewEvent::Pasted(text) if tab.broadcast => {
                for other in tab.panes() {
                    if other != *view {
                        other.update(cx, |other, cx| other.session.update(cx, |s, cx| s.paste(text, cx)));
                    }
                }
            }
            TerminalViewEvent::Ran(text) if tab.broadcast => {
                for other in tab.panes() {
                    if other != *view {
                        other.update(cx, |other, cx| other.session.update(cx, |s, cx| s.run(text, cx)));
                    }
                }
            }
            _ => {}
        });
        self.subscriptions.push((view.entity_id(), subscription));
    }

    /// Opens a file from `view`'s shell in the editor beside the terminals. For
    /// an SSH shell, the file is on the server. The editor opens only once the
    /// file is read. Images, PDFs and the like open in the system's app instead.
    pub fn open_file(&mut self, view: &Entity<TerminalView>, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let session = view.read(cx).session.read(cx);
        // Language servers run here, and are looked for on the shell's PATH.
        let shell_path = session.is_local().then(|| session.blocks.as_ref().and_then(|b| b.path.clone())).flatten();
        let disk = session.disk();
        cx.spawn_in(window, async move |tab, cx| {
            let Some(disk) = disk.await else { return };
            if system_open::opens_outside(&path) {
                let local = system_open::local_copy(disk, path.clone()).await;
                let _ = tab.update_in(cx, |_, window, cx| match local {
                    Ok(local) => cx.open_with_system(&local),
                    Err(err) => cant_open(&path, &err, window, cx),
                });
                return;
            }
            let open = tab.update_in(cx, |tab, window, cx| {
                tab.editor.as_ref().is_some_and(|(pane, _)| pane.update(cx, |pane, cx| pane.show_open(&path, &disk, window, cx)))
            });
            if open.unwrap_or(true) {
                return;
            }
            let text = disk.clone().run({
                let path = path.clone();
                move |disk| read_text(disk, &path)
            });
            let (text, as_root) = match text.await {
                Err(DiskError::Denied) if disk.can_sudo() => {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let title = format!("Open {name} with sudo?");
                    let detail = "Your account can't read this file. Somme can open it as root with sudo, \
                                  and save your changes the same way.";
                    if !sudo::ask(&title, detail, "Open with sudo", cx).await {
                        return;
                    }
                    let read = {
                        let path = path.clone();
                        move |disk: &Disk, password: Option<&str>| read_text_as_root(disk, &path, password)
                    };
                    (sudo::as_root(&disk, read, cx).await, true)
                }
                text => (text, false),
            };
            let _ = tab.update_in(cx, |tab, window, cx| match text {
                Ok(text) => {
                    let pane = tab.editor_pane(window, cx);
                    pane.update(cx, |pane, cx| pane.add(path, disk, text, as_root, shell_path.as_deref(), window, cx));
                    cx.notify();
                }
                Err(err) => cant_open(&path, &err, window, cx),
            });
        })
        .detach();
    }

    /// The editor beside the terminals, opened if needed.
    fn editor_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorPane> {
        match &self.editor {
            Some((pane, _)) => pane.clone(),
            None => {
                let pane = cx.new(EditorPane::new);
                let subscription = cx.subscribe_in(&pane, window, |tab, _, event, window, cx| match event {
                    EditorPaneEvent::Empty => {
                        tab.editor = None;
                        window.focus(&tab.active.focus_handle(cx), cx);
                        cx.notify();
                    }
                    EditorPaneEvent::FilesChanged => {}
                });
                self.editor = Some((pane.clone(), subscription));
                pane
            }
        }
    }

    pub fn panes(&self) -> Vec<Entity<TerminalView>> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    pub fn active(&self) -> &Entity<TerminalView> {
        &self.active
    }

    pub fn title(&self, cx: &App) -> SharedString {
        match &self.custom_title {
            Some(title) => title.clone(),
            None => self.active.read(cx).session.read(cx).title(),
        }
    }

    /// Names the tab. A blank name goes back to the automatic one.
    pub fn rename(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.trim();
        self.custom_title = (!name.is_empty()).then(|| SharedString::from(name.to_owned()));
        cx.emit(TerminalTabEvent::Changed);
        cx.notify();
    }

    /// Whether a file open in the editor has unsaved changes.
    pub fn has_unsaved(&self, cx: &App) -> bool {
        self.editor.as_ref().is_some_and(|(editor, _)| editor.read(cx).has_changes())
    }

    pub fn is_running(&self, cx: &App) -> bool {
        self.panes().iter().any(|p| p.read(cx).session.read(cx).is_running())
    }

    pub fn split(&mut self, axis: Axis, before: bool, window: &mut Window, cx: &mut Context<Self>) {
        // A local split opens in the folder of the pane it splits from.
        let session = self.active.read(cx).session.read(cx);
        let (target, dir) = (session.target.clone(), session.current_dir());
        let view = Self::new_view(&target, dir, &self.services, window, cx);
        self.root.split(self.active.entity_id(), view.clone(), axis, before);
        // The new pane goes beside the old one, so both must show.
        self.maximized = None;
        self.watch(&view, window, cx);
        self.active = view.clone();
        window.focus(&view.focus_handle(cx), cx);
        cx.emit(TerminalTabEvent::Changed);
        cx.notify();
    }

    /// Closes the focused pane. The tab reports `Empty` when it was the last one.
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.active.entity_id();
        self.subscriptions.retain(|(view, _)| *view != id);
        self.minimized.retain(|view| view.entity_id() != id);
        if self.maximized.as_ref().is_some_and(|view| view.entity_id() == id) {
            self.maximized = None;
        }
        if !self.root.remove(id) {
            cx.emit(TerminalTabEvent::Empty);
            return;
        }
        let next = self.shown().into_iter().next().or_else(|| self.panes().into_iter().next());
        if let Some(next) = next {
            // The last pane on screen can't hide in the bar.
            self.minimized.retain(|view| *view != next);
            self.active = next.clone();
            window.focus(&next.focus_handle(cx), cx);
        }
        cx.emit(TerminalTabEvent::Changed);
        cx.notify();
    }

    /// Closes one pane, focused or not.
    fn close_pane(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = view.clone();
        self.close_active(window, cx);
    }

    /// The panes on screen: not minimized, or only the maximized one.
    fn shown(&self) -> Vec<Entity<TerminalView>> {
        match &self.maximized {
            Some(view) => vec![view.clone()],
            None => self.panes().into_iter().filter(|view| !self.minimized.contains(view)).collect(),
        }
    }

    /// Puts a pane away in the bar at the bottom. The last pane on screen stays.
    fn minimize(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.shown();
        if shown.len() < 2 || !shown.contains(view) {
            return;
        }
        self.minimized.push(view.clone());
        if *view == self.active
            && let Some(next) = self.shown().into_iter().next()
        {
            self.focus_pane(&next, window, cx);
        }
        cx.notify();
    }

    /// Brings a minimized pane back where it was.
    fn restore(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        self.minimized.retain(|other| other != view);
        self.maximized = None;
        self.focus_pane(view, window, cx);
        cx.notify();
    }

    /// Fills the tab with one pane, or puts the panes back as they were.
    fn toggle_maximize(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        if self.maximized.as_ref() == Some(view) {
            self.maximized = None;
        } else {
            self.minimized.retain(|other| other != view);
            self.maximized = Some(view.clone());
        }
        self.focus_pane(view, window, cx);
        cx.notify();
    }

    fn focus_pane(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = view.clone();
        window.focus(&view.focus_handle(cx), cx);
        cx.emit(TerminalTabEvent::Changed);
    }

    pub fn focus_next(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.shown();
        let Some(ix) = panes.iter().position(|p| *p == self.active) else { return };
        let next = if forward { (ix + 1) % panes.len() } else { (ix + panes.len() - 1) % panes.len() };
        self.active = panes[next].clone();
        window.focus(&self.active.focus_handle(cx), cx);
        cx.notify();
    }

    /// Runs `text` in the focused pane, or types it without Enter when `run` is false.
    pub fn send_snippet(&mut self, text: &str, run: bool, cx: &mut Context<Self>) {
        self.active.update(cx, |view, cx| {
            if run {
                view.run_text(text, cx);
            } else {
                view.paste_text(text, cx);
            }
        });
    }

    /// Runs `text` once the session is connected.
    pub fn run_when_connected(&mut self, text: String, cx: &mut Context<Self>) {
        if self.active.read(cx).session.read(cx).status == Status::Connected {
            self.send_snippet(&text, true, cx);
        } else {
            self.pending_run = Some(text);
        }
    }

    pub fn toggle_broadcast(&mut self, cx: &mut Context<Self>) {
        self.broadcast = !self.broadcast;
        cx.emit(TerminalTabEvent::Changed);
        cx.notify();
    }

    /// The panes under `node` that are not minimized. None when all of them are.
    fn render_node(&self, node: &PaneNode, path: String, cx: &Context<Self>) -> Option<AnyElement> {
        match node {
            PaneNode::Leaf(view) if self.minimized.contains(view) => None,
            PaneNode::Leaf(view) => Some(self.render_pane(view, cx)),
            PaneNode::Split(axis, children) => {
                let mut shown: Vec<AnyElement> = children
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, child)| self.render_node(child, format!("{path}.{ix}"), cx))
                    .collect();
                if shown.len() < 2 {
                    return shown.pop();
                }
                let id = SharedString::from(format!("split-{path}-{}", shown.len()));
                let panels = shown.into_iter().map(|child| resizable_panel().child(child));
                Some(match axis {
                    Axis::Horizontal => h_resizable(id).children(panels).into_any_element(),
                    Axis::Vertical => v_resizable(id).children(panels).into_any_element(),
                })
            }
        }
    }

    /// A terminal pane. With several panes, a bar on top names it and holds
    /// its minimize, maximize and close buttons.
    fn render_pane(&self, view: &Entity<TerminalView>, cx: &Context<Self>) -> AnyElement {
        if !matches!(self.root, PaneNode::Split(..)) {
            return div().size_full().child(view.clone()).into_any_element();
        }
        let theme = cx.theme();
        let active = *view == self.active;
        let maximized = self.maximized.as_ref() == Some(view);
        let can_minimize = !maximized && self.shown().len() > 1;
        let title = view.read(cx).session.read(cx).title();
        let key = view.entity_id().as_u64() as usize;
        let (minimize, maximize, close, focus) = (view.clone(), view.clone(), view.clone(), view.clone());
        let header = h_flex()
            .h(px(24.))
            .flex_none()
            .pl_2()
            .pr_1()
            .gap_0p5()
            .items_center()
            .bg(theme.foreground.opacity(if active { 0.06 } else { 0.03 }))
            .border_b_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(if active { theme.foreground } else { theme.muted_foreground })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |tab, _, window, cx| tab.focus_pane(&focus, window, cx)),
            )
            .child(div().flex_1().min_w_0().truncate().child(title))
            .when(can_minimize, |bar| {
                bar.child(
                    Button::new(("pane-minimize", key))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::Minus))
                        .tab_stop(false)
                        .tooltip("Minimize")
                        .on_click(cx.listener(move |tab, _, window, cx| tab.minimize(&minimize, window, cx))),
                )
            })
            .child(
                Button::new(("pane-maximize", key))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(if maximized { Lucide::Minimize2 } else { Lucide::Maximize2 }))
                    .tab_stop(false)
                    .tooltip(if maximized { "Restore" } else { "Maximize" })
                    .on_click(cx.listener(move |tab, _, window, cx| tab.toggle_maximize(&maximize, window, cx))),
            )
            .child(
                Button::new(("pane-close", key))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tab_stop(false)
                    .tooltip("Close")
                    .on_click(cx.listener(move |tab, _, window, cx| tab.close_pane(&close, window, cx))),
            );
        v_flex()
            .size_full()
            .border_1()
            .border_color(if active { theme.primary.opacity(0.6) } else { theme.border })
            .child(header)
            .child(div().flex_1().min_h_0().child(view.clone()))
            .into_any_element()
    }

    /// The minimized panes. A click brings one back.
    fn render_minimized(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let chips = self.minimized.iter().map(|view| {
            let title = view.read(cx).session.read(cx).title();
            let restore = view.clone();
            h_flex()
                .id(("pane-minimized", view.entity_id().as_u64() as usize))
                .h(px(22.))
                .max_w(px(200.))
                .px_2()
                .gap_1p5()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .bg(theme.foreground.opacity(0.05))
                .hover(|chip| chip.bg(theme.foreground.opacity(0.1)))
                .child(Icon::new(Lucide::SquareTerminal).size(px(12.)))
                .child(div().min_w_0().truncate().child(title))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Restore").build(window, cx))
                .on_click(cx.listener(move |tab, _, window, cx| tab.restore(&restore, window, cx)))
        });
        h_flex()
            .h(px(30.))
            .flex_none()
            .px_2()
            .gap_1()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .children(chips)
    }
}

impl Focusable for TerminalTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active.focus_handle(cx)
    }
}

impl Render for TerminalTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let panes = match &self.maximized {
            Some(view) => Some(self.render_pane(view, cx)),
            None => self.render_node(&self.root, "0".into(), cx),
        };
        let terminals = v_flex()
            .size_full()
            .child(div().flex_1().min_h_0().children(panes))
            .when(!self.minimized.is_empty(), |col| col.child(self.render_minimized(cx)))
            .into_any_element();
        let body = match &self.editor {
            Some((editor, _)) => h_resizable("editor-split")
                .child(resizable_panel().child(terminals))
                .child(resizable_panel().child(editor.clone()))
                .into_any_element(),
            None => terminals,
        };
        div().size_full().bg(theme.background).child(body)
    }
}
