//! One automation open for editing: the canvas in the middle, the settings
//! of the picked step (or of the whole flow) on the right, and its runs.
//! Changes save on their own a moment after the last one.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    AnyElement, KeyDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Size, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
    point, prelude::FluentBuilder as _, px,
};
use somme_automation::layout::{self, STEP_HEIGHT, STEP_WIDTH};
use somme_automation::schema::{self, free_id, problems};
use somme_automation::{Automation, Edge, Node, NodeKind, RunRecord, RunStatus};

use super::chat::Chat;
use super::panel::{StartForm, StepForm};
use crate::automations::Automations;
use crate::ui::common::{Lucide, ink_button, notify_error};
use crate::ui::native;

/// How long after the last change it saves.
const SAVE_AFTER: Duration = Duration::from_millis(600);
pub(super) const MIN_ZOOM: f32 = 0.3;
pub(super) const MAX_ZOOM: f32 = 2.0;
const PANEL_WIDTH: f32 = 340.;
/// Below this canvas width the palette shows icons only.
pub(super) const WIDE_CANVAS: f32 = 760.;
/// The width the palette takes on the canvas's left, with its words and without.
const PALETTE_SPACE: f32 = 180.;
const PALETTE_ICONS: f32 = 56.;

pub enum EditorEvent {
    /// It was deleted; show the list.
    Deleted,
}

/// What is being dragged on the canvas.
pub(super) enum Drag {
    Pan { start: Point<Pixels>, pan: Point<Pixels> },
    Step { id: String, start: Point<Pixels>, at: [f32; 2], moved: bool },
    /// A new line from a step's way out, following the mouse.
    Line { from: String, port: String, to: Point<Pixels> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Picked {
    Nothing,
    Step(String),
    Line(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Settings,
    Runs,
    /// Building by chatting.
    Chat,
}

pub struct FlowEditor {
    pub(super) draft: Automation,
    /// As saved last, to know when there is something to save.
    saved: Automation,
    saving: bool,
    save_task: Option<Task<()>>,
    pub(super) pan: Point<Pixels>,
    pub(super) zoom: f32,
    /// Fit the flow in view at the next frame, once the canvas has a size.
    pub(super) fit_pending: bool,
    /// Keep it fitted as the canvas changes size, until the user moves the view.
    pub(super) keep_fitted: bool,
    /// The canvas size it was fitted to last.
    pub(super) fitted: Option<Size<Pixels>>,
    pub(super) drag: Option<Drag>,
    pub(super) picked: Picked,
    pub(super) tab: Tab,
    pub(super) step_form: Option<StepForm>,
    /// What saving the step's command as a script did: whether it went well, and what happened.
    pub(super) script_note: Option<(bool, String)>,
    pub(super) start_form: StartForm,
    /// The run shown on the canvas, in the Runs tab.
    pub(super) run: Option<String>,
    /// Past runs, read when the Runs tab opens.
    pub(super) history: Vec<RunRecord>,
    /// Values typed for the next run, by input name.
    pub(super) run_inputs: Vec<(String, Entity<InputState>)>,
    /// The dry run shows in the Runs tab.
    pub(super) dry: bool,
    /// Where the canvas is in the window, from the last frame.
    pub(super) bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(super) focus: FocusHandle,
    pub(super) chat: Chat,
    /// Commands the AI wrote that the user changed, by step: the AI's own.
    /// Each becomes a lesson once the change is saved.
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for FlowEditor {}

impl FlowEditor {
    pub fn new(automation: Automation, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let start_form = StartForm::new(&automation, window, cx);
        let tab = Self::first_tab(&automation, cx);
        let chat = Chat::new(window, cx);
        let subscription = cx.observe_in(&Automations::global(cx), window, |editor, automations, window, cx| {
            let saved = automations.read(cx).get(&editor.draft.id).map(|saved| saved.automation.clone());
            match saved {
                // Gone, deleted on another computer. An empty list is a locked vault.
                None if !automations.read(cx).saved.is_empty() => {
                    cx.emit(EditorEvent::Deleted);
                }
                // Changed elsewhere, like on another computer, while nothing here waits to save.
                Some(saved) if saved != editor.saved && editor.draft == editor.saved && !editor.saving => {
                    editor.saved = saved.clone();
                    editor.draft = saved;
                    editor.rebuild_forms(window, cx);
                }
                _ => {}
            }
            cx.notify();
        });
        let mut editor = Self {
            saved: automation.clone(),
            draft: automation,
            saving: false,
            save_task: None,
            pan: point(px(0.), px(0.)),
            zoom: 1.,
            fit_pending: true,
            keep_fitted: true,
            fitted: None,
            drag: None,
            picked: Picked::Nothing,
            tab,
            step_form: None,
            script_note: None,
            start_form,
            run: None,
            history: Vec::new(),
            run_inputs: Vec::new(),
            dry: false,
            bounds: Rc::new(Cell::new(Bounds::default())),
            focus: cx.focus_handle(),
            chat,
            _subscriptions: vec![subscription],
        };
        editor.rebuild_run_inputs(window, cx);
        editor
    }

    pub fn id(&self) -> &str {
        &self.draft.id
    }

    // ---- saving ----

    /// Call after every change to the draft. Saves a moment later.
    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        self.save_task = Some(cx.spawn(async move |editor, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            let _ = editor.update(cx, |editor, cx| editor.save_now(cx));
        }));
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        if self.draft == self.saved || self.saving {
            return;
        }
        let automation = self.draft.clone();
        let task = Automations::global(cx).update(cx, |automations, cx| automations.save(&automation, cx));
        self.saving = true;
        cx.spawn(async move |editor, cx| {
            let result = task.await;
            let _ = editor.update(cx, |editor, cx| {
                editor.saving = false;
                match result {
                    Ok(_) => {
                        if automation.start.schedule.is_some() {
                            cx.defer(super::offer_background);
                        }
                        editor.saved = automation;
                    }
                    Err(err) => tracing::warn!(%err, "could not save an automation"),
                }
                // Changes made while it saved go next.
                if editor.draft != editor.saved {
                    editor.changed(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn status_words(&self) -> &'static str {
        if self.saving || self.save_task.is_some() {
            "Saving…"
        } else if self.draft == self.saved {
            "Saved"
        } else {
            "Not saved"
        }
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.draft.name.clone();
        let Ok(id) = self.draft.id.parse() else { return };
        let editor = cx.entity().downgrade();
        native::confirm(
            &format!("Delete \u{201c}{name}\u{201d}?"),
            "Its steps and its run history go too. This can't be undone.",
            "Delete",
            move |_, cx| {
                Automations::global(cx).update(cx, |automations, cx| automations.delete(id, cx));
                let _ = editor.update(cx, |editor, cx| {
                    editor.save_task = None;
                    cx.emit(EditorEvent::Deleted);
                });
            },
            window,
            cx,
        );
    }

    // ---- steps and lines ----

    pub(super) fn add_step(&mut self, kind: NodeKind, window: &mut Window, cx: &mut Context<Self>) {
        let after = match &self.picked {
            Picked::Step(id) => Some(id.clone()),
            _ => None,
        };
        let base = match &kind {
            NodeKind::Command { .. } => "command",
            NodeKind::Local { .. } => "local",
            NodeKind::If { .. } => "check",
            NodeKind::Wait { .. } => "wait",
            NodeKind::Notify { .. } => "notify",
            NodeKind::Decide { .. } => "decide",
            NodeKind::Write { .. } => "write",
            NodeKind::Copy { .. } => "copy",
            NodeKind::Ask { .. } => "ask",
            NodeKind::Http { .. } => "http",
            NodeKind::Tls { .. } => "cert",
            NodeKind::Dns { .. } => "dns",
            NodeKind::Agent { .. } => "agent",
        };
        let id = free_id(&self.draft, base);
        let mut node = Node::new(id.clone(), kind);
        // A check looks at what the step before it said.
        if let Some(after) = &after {
            let said = format!("{{{{nodes.{after}.output}}}}");
            match &mut node.kind {
                NodeKind::If { value, .. } => *value = said,
                NodeKind::Decide { about, .. } => *about = said,
                NodeKind::Write { prompt } => *prompt = format!("Say in 3 short lines what this means:\n{said}"),
                NodeKind::Ask { message, .. } => *message = said,
                _ => {}
            }
        }
        node.at = layout::place_new(&self.draft, after.as_deref());
        self.draft.nodes.push(node);
        // The first step goes in the middle.
        self.fit_pending |= self.draft.nodes.len() == 1;
        if let Some(after) = after {
            self.draft.edges.push(Edge::new(after, id.clone()));
        }
        self.pick(Picked::Step(id), window, cx);
        self.changed(cx);
    }

    pub(super) fn connect(&mut self, from: &str, port: &str, to: &str, window: &mut Window, cx: &mut Context<Self>) {
        if from == to {
            return;
        }
        let first = self.draft.node(from).map(|node| node.kind.ports()[0]);
        let port = (first != Some(port)).then(|| port.to_owned());
        let edge = Edge { from: from.to_owned(), port, to: to.to_owned() };
        if self.draft.edges.contains(&edge) {
            return;
        }
        let mut with = self.draft.clone();
        with.edges.push(edge.clone());
        if problems(&with).iter().any(|problem| problem.contains("loop")) && !problems(&self.draft).iter().any(|p| p.contains("loop")) {
            notify_error(window, cx, "That line would make a loop", "A step can't lead back to a step before it.");
            return;
        }
        self.draft.edges.push(edge);
        self.changed(cx);
    }

    pub(super) fn remove_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.picked.clone() {
            Picked::Step(id) => {
                self.draft.nodes.retain(|node| node.id != id);
                self.draft.edges.retain(|edge| edge.from != id && edge.to != id);
            }
            Picked::Line(ix) if ix < self.draft.edges.len() => {
                self.draft.edges.remove(ix);
            }
            _ => return,
        }
        self.pick(Picked::Nothing, window, cx);
        self.changed(cx);
    }

    pub(super) fn pick(&mut self, picked: Picked, window: &mut Window, cx: &mut Context<Self>) {
        if picked == self.picked {
            return;
        }
        self.picked = picked;
        self.script_note = None;
        self.step_form = match &self.picked {
            Picked::Step(id) => self.draft.node(id).map(|node| StepForm::new(node, window, cx)),
            _ => None,
        };
        cx.notify();
    }

    pub(super) fn rebuild_forms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_form = StartForm::new(&self.draft, window, cx);
        self.step_form = match &self.picked {
            Picked::Step(id) => self.draft.node(id).map(|node| StepForm::new(node, window, cx)),
            _ => None,
        };
        if let Picked::Step(id) = &self.picked
            && self.draft.node(id).is_none()
        {
            self.picked = Picked::Nothing;
        }
        self.rebuild_run_inputs(window, cx);
    }

    /// The step being edited, to change.
    pub(super) fn step_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.draft.nodes.iter_mut().find(|node| node.id == id)
    }

    pub(super) fn tidy(&mut self, cx: &mut Context<Self>) {
        layout::tidy(&mut self.draft);
        self.fit_pending = true;
        self.changed(cx);
    }

    /// Shows the whole flow in the canvas.
    pub(super) fn fit(&mut self, bounds: Bounds<Pixels>) {
        let size = bounds.size;
        self.fit_pending = false;
        self.fitted = Some(size);
        if self.draft.nodes.is_empty() {
            self.zoom = 1.;
            self.pan = point(size.width / 2. - px(STEP_WIDTH / 2.), px(80.));
            return;
        }
        let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for node in &self.draft.nodes {
            left = left.min(node.at[0]);
            top = top.min(node.at[1]);
            right = right.max(node.at[0] + STEP_WIDTH);
            bottom = bottom.max(node.at[1] + STEP_HEIGHT);
        }
        let margin = 40.;
        let (width, height) = (size.width.as_f32(), size.height.as_f32());
        // Leave the palette's side free when there is room.
        let inset = if width >= WIDE_CANVAS {
            PALETTE_SPACE
        } else if width >= PALETTE_ICONS + STEP_WIDTH + margin * 2. {
            PALETTE_ICONS
        } else {
            0.
        };
        let room = width - inset;
        let zoom = ((room - margin * 2.) / (right - left)).min((height - margin * 2.) / (bottom - top)).clamp(MIN_ZOOM, 1.);
        self.zoom = zoom;
        let (mid_x, mid_y) = ((left + right) / 2., (top + bottom) / 2.);
        self.pan = point(px(inset + room / 2. - mid_x * zoom), px(height / 2. - mid_y * zoom));
    }

    /// Zooms by `factor`, keeping the point under `at` (in the canvas) still.
    pub(super) fn zoom_by(&mut self, factor: f32, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.keep_fitted = false;
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let world = (at - self.pan).map(|v| v.as_f32() / self.zoom);
        self.pan = at - world.map(|v| px(v * zoom));
        self.zoom = zoom;
        cx.notify();
    }

    // ---- runs ----

    fn rebuild_run_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: Vec<&str> = self.draft.inputs.iter().map(|input| input.name.as_str()).collect();
        let have: Vec<&str> = self.run_inputs.iter().map(|(name, _)| name.as_str()).collect();
        if names == have {
            return;
        }
        self.run_inputs = self
            .draft
            .inputs
            .iter()
            .map(|input| {
                let default = input.default.clone();
                (input.name.clone(), cx.new(|cx| InputState::new(window, cx).default_value(default)))
            })
            .collect();
    }

    pub(super) fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Runs {
            self.read_history(cx);
            self.rebuild_run_inputs(window, cx);
        }
        cx.notify();
    }

    pub(super) fn read_history(&mut self, cx: &mut Context<Self>) {
        self.history = Automations::global(cx).read(cx).history(&self.draft.id);
    }

    /// Runs it now, with the values typed in the Runs tab.
    pub(super) fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(problem) = problems(&self.draft).into_iter().next() {
            notify_error(window, cx, "Fix this first", problem);
            return;
        }
        self.save_now(cx);
        let inputs: BTreeMap<String, String> =
            self.run_inputs.iter().map(|(name, input)| (name.clone(), input.read(cx).value().to_string())).collect();
        let draft = self.draft.clone();
        match Automations::global(cx).update(cx, |automations, _| automations.run(&draft, inputs)) {
            Ok(run) => {
                self.run = Some(run);
                self.show_tab(Tab::Runs, window, cx);
            }
            Err(err) => notify_error(window, cx, "Can't run it", err),
        }
    }

    pub(super) fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = &self.run {
            Automations::global(cx).read(cx).cancel(run);
        }
    }

    /// The run shown, live when it is running now.
    pub(super) fn shown_run(&self, cx: &gpui_kit::App) -> Option<RunRecord> {
        if self.tab != Tab::Runs {
            return None;
        }
        let id = self.run.as_ref()?;
        let automations = Automations::global(cx);
        let automations = automations.read(cx);
        automations.runs.get(id).cloned().or_else(|| self.history.iter().find(|run| run.head.id == *id).cloned())
    }

    /// The runs to list: this session's live ones first, then the saved ones.
    pub(super) fn runs(&self, cx: &gpui_kit::App) -> Vec<RunRecord> {
        let automations = Automations::global(cx);
        let automations = automations.read(cx);
        let mut runs: Vec<RunRecord> =
            automations.runs.values().filter(|run| run.head.automation_id == self.draft.id).cloned().collect();
        for run in &self.history {
            if !runs.iter().any(|live| live.head.id == run.head.id) {
                runs.push(run.clone());
            }
        }
        runs.sort_by(|a, b| b.started.cmp(&a.started).then_with(|| b.head.id.cmp(&a.head.id)));
        runs.truncate(crate::automations::SHOWN_RUNS);
        runs
    }

    pub(super) fn is_running(&self, cx: &gpui_kit::App) -> bool {
        self.run.as_ref().is_some_and(|run| Automations::global(cx).read(cx).is_running(run))
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        match event.keystroke.key.as_str() {
            "delete" | "backspace" => self.remove_picked(window, cx),
            "escape" => {
                self.drag = None;
                self.pick(Picked::Nothing, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- drawing ----

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let running = self.is_running(cx);
        h_flex()
            .flex_none()
            .h(px(44.))
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Lucide::Workflow).size(px(16.)).text_color(theme.muted_foreground))
            .child(div().min_w_0().truncate().font_semibold().child(SharedString::from(self.draft.name.clone())))
            .child(div().text_xs().text_color(theme.muted_foreground).child(self.status_words()))
            .child(div().flex_1())
            .child(
                Button::new("flow-tidy")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::LayoutGrid))
                    .label("Tidy")
                    .tooltip("Line the steps up")
                    .disabled(self.draft.nodes.is_empty())
                    .on_click(cx.listener(|editor, _, _, cx| editor.tidy(cx))),
            )
            .child(if running {
                Button::new("flow-stop")
                    .outline()
                    .small()
                    .icon(Icon::new(Lucide::Square))
                    .label("Stop")
                    .on_click(cx.listener(|editor, _, _, cx| editor.stop(cx)))
            } else {
                Button::new("flow-run")
                    .custom(ink_button(cx))
                    .small()
                    .icon(Icon::new(Lucide::Play))
                    .label("Run")
                    .on_click(cx.listener(|editor, _, window, cx| editor.run(window, cx)))
            })
            .child(
                Button::new("flow-delete")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Trash))
                    .tooltip("Delete this automation")
                    .on_click(cx.listener(|editor, _, window, cx| editor.delete(window, cx))),
            )
            .into_any_element()
    }

    fn render_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let chat = Self::chat_allowed(cx);
        if self.tab == Tab::Chat && !chat {
            self.tab = Tab::Settings;
        }
        let tab_button = |id: &'static str, label: &'static str, tab: Tab, cx: &mut Context<Self>| {
            let on = self.tab == tab;
            div()
                .id(id)
                .flex_1()
                .py_1()
                .rounded_md()
                .text_sm()
                .text_center()
                .cursor_pointer()
                .when(on, |el| el.bg(theme.secondary).font_semibold())
                .when(!on, |el| el.text_color(theme.muted_foreground).hover(|el| el.bg(theme.secondary.opacity(0.5))))
                .child(label)
                .on_click(cx.listener(move |editor, _, window, cx| editor.show_tab(tab, window, cx)))
        };
        let tabs = h_flex()
            .flex_none()
            .p_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .when(chat, |tabs| tabs.child(tab_button("flow-tab-chat", "Chat", Tab::Chat, cx)))
            .child(tab_button("flow-tab-settings", "Settings", Tab::Settings, cx))
            .child(tab_button("flow-tab-runs", "Runs", Tab::Runs, cx));
        // The chat keeps its box at the bottom, so it scrolls on its own.
        let body = match self.tab {
            Tab::Settings => div().id("flow-panel-body").size_full().overflow_y_scroll().child(self.render_settings(window, cx)).into_any_element(),
            Tab::Runs => div().id("flow-panel-body").size_full().overflow_y_scroll().child(self.render_runs(window, cx)).into_any_element(),
            Tab::Chat => self.render_chat(cx),
        };
        v_flex()
            .flex_none()
            .w(px(PANEL_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
}

impl Focusable for FlowEditor {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FlowEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let canvas = self.render_canvas(window, cx);
        let panel = self.render_panel(window, cx);
        let toolbar = self.render_toolbar(cx);
        v_flex()
            .size_full()
            .on_key_down(cx.listener(Self::on_key))
            .child(toolbar)
            .child(h_flex().flex_1().min_h_0().child(div().flex_1().min_w_0().h_full().child(canvas)).child(panel))
    }
}

/// A new step of each kind, as the palette adds it.
pub(super) fn new_kind(kind: &str, flow_name: &str) -> NodeKind {
    match schema::blank(kind) {
        Some(NodeKind::Notify { .. }) | None => NodeKind::Notify { title: flow_name.to_owned(), body: String::new() },
        Some(kind) => kind,
    }
}

/// How a run went, in a word and a colour.
pub(super) fn run_words(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "Running",
        RunStatus::Ok => "Done",
        RunStatus::Failed => "Failed",
        RunStatus::Cancelled => "Stopped",
    }
}
