use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, h_resizable, resizable_panel, v_resizable};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Subscription, Window, div,
};

use crate::editor::{EditorPane, EditorPaneEvent};
use crate::services::Services;
use crate::terminal::session::Status;
use crate::terminal::view::TerminalViewEvent;
use crate::terminal::{SessionTarget, TerminalSession, TerminalView};

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

    /// Puts `new` next to `target`, splitting along `axis`.
    fn split(&mut self, target: EntityId, new: Entity<TerminalView>, axis: Axis) -> bool {
        match self {
            Self::Leaf(view) if view.entity_id() == target => {
                let old = Self::Leaf(view.clone());
                *self = Self::Split(axis, vec![old, Self::Leaf(new)]);
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
                    children.insert(ix + 1, Self::Leaf(new));
                    return true;
                }
                children.iter_mut().any(|c| c.split(target, new.clone(), axis))
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
            TerminalViewEvent::OpenFile(path) => tab.open_file(path.clone(), window, cx),
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

    /// Opens a file in the editor beside the terminals.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Language servers are looked for on the shell's PATH.
        let shell_path = self.active.read(cx).session.read(cx).blocks.as_ref().and_then(|b| b.path.clone());
        let pane = match &self.editor {
            Some((pane, _)) => pane.clone(),
            None => {
                let pane = cx.new(EditorPane::new);
                let subscription = cx.subscribe_in(&pane, window, |tab, _, event, window, cx| match event {
                    EditorPaneEvent::Empty => {
                        tab.editor = None;
                        window.focus(&tab.active.focus_handle(cx), cx);
                        cx.notify();
                    }
                });
                self.editor = Some((pane.clone(), subscription));
                pane
            }
        };
        pane.update(cx, |pane, cx| pane.open(path, shell_path.as_deref(), window, cx));
        cx.notify();
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

    pub fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        // A local split opens in the folder of the pane it splits from.
        let session = self.active.read(cx).session.read(cx);
        let (target, dir) = (session.target.clone(), session.current_dir());
        let view = Self::new_view(&target, dir, &self.services, window, cx);
        self.root.split(self.active.entity_id(), view.clone(), axis);
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
        if !self.root.remove(id) {
            cx.emit(TerminalTabEvent::Empty);
            return;
        }
        if let Some(next) = self.panes().into_iter().next() {
            self.active = next.clone();
            window.focus(&next.focus_handle(cx), cx);
        }
        cx.emit(TerminalTabEvent::Changed);
        cx.notify();
    }

    pub fn focus_next(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.panes();
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

    fn render_node(&self, node: &PaneNode, path: String, cx: &App) -> AnyElement {
        match node {
            PaneNode::Leaf(view) => {
                let several = matches!(self.root, PaneNode::Split(..));
                let active = *view == self.active;
                let theme = cx.theme();
                div()
                    .size_full()
                    .when_some_border(several, active, theme.primary, theme.border)
                    .child(view.clone())
                    .into_any_element()
            }
            PaneNode::Split(axis, children) => {
                let id = SharedString::from(format!("split-{path}"));
                let panels = children
                    .iter()
                    .enumerate()
                    .map(|(ix, child)| resizable_panel().child(self.render_node(child, format!("{path}.{ix}"), cx)));
                match axis {
                    Axis::Horizontal => h_resizable(id).children(panels).into_any_element(),
                    Axis::Vertical => v_resizable(id).children(panels).into_any_element(),
                }
            }
        }
    }
}

trait PaneBorder: Styled + Sized {
    /// Outlines panes when there are several, highlighting the focused one.
    fn when_some_border(self, several: bool, active: bool, on: gpui_kit::Hsla, off: gpui_kit::Hsla) -> Self {
        if !several {
            return self;
        }
        self.border_1().border_color(if active { on.opacity(0.6) } else { off })
    }
}

use gpui_kit::Styled;

impl<T: Styled> PaneBorder for T {}

impl Focusable for TerminalTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active.focus_handle(cx)
    }
}

impl Render for TerminalTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let terminals = self.render_node(&self.root, "0".into(), cx);
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
