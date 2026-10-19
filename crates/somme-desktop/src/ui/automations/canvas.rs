//! The canvas: steps as cards, lines between them, a palette to add steps,
//! and zoom buttons. Drag the background to move around, drag a card to move
//! it, and drag from a step's way out to another step to join them. In the
//! Runs tab the cards show how the shown run went.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, point, prelude::FluentBuilder as _, px,
};
use somme_automation::history::{StepRecord, StepStatus};
use somme_automation::layout::{STEP_HEIGHT, STEP_WIDTH};
use somme_automation::schema::{Direction, FAILED};
use somme_automation::{Node, NodeKind};

use super::editor::{Drag, FlowEditor, Picked, WIDE_CANVAS, new_kind};
use crate::ai::{self, Feature};
use crate::settings::Settings;
use crate::store::Store;
use crate::ui::common::Lucide;

const PORT: f32 = 12.;

/// What the palette offers, in order.
const PALETTE: &[(&str, &str, Lucide)] = &[
    ("command", "Run a command", Lucide::Server),
    ("local", "Run on this computer", Lucide::Laptop),
    ("copy", "Copy a file", Lucide::ArrowUpDown),
    ("if", "If", Lucide::Split),
    ("wait", "Wait", Lucide::Timer),
    ("ask", "Ask me", Lucide::Hand),
    ("notify", "Notify", Lucide::Bell),
    ("http", "HTTP request", Lucide::Globe),
    ("tls", "Certificate", Lucide::ShieldCheck),
    ("dns", "DNS lookup", Lucide::Network),
    ("decide", "Decide", Lucide::CircleQuestionMark),
    ("write", "Write", Lucide::FilePen),
    ("agent", "Agent", Lucide::Bot),
];

/// Whether the palette offers `kind`: AI steps only while their AI feature is on.
fn offered(kind: &str, settings: &Settings) -> bool {
    match kind {
        "decide" => ai::allowed(settings, Feature::Decide),
        "write" => ai::allowed(settings, Feature::Write),
        "agent" => ai::allowed(settings, Feature::Agent),
        _ => true,
    }
}

pub(super) fn kind_icon(kind: &NodeKind) -> Lucide {
    match kind {
        NodeKind::Command { .. } => Lucide::Server,
        NodeKind::Local { .. } => Lucide::Laptop,
        NodeKind::If { .. } => Lucide::Split,
        NodeKind::Wait { .. } => Lucide::Timer,
        NodeKind::Notify { .. } => Lucide::Bell,
        NodeKind::Decide { .. } => Lucide::CircleQuestionMark,
        NodeKind::Write { .. } => Lucide::FilePen,
        NodeKind::Copy { .. } => Lucide::ArrowUpDown,
        NodeKind::Ask { .. } => Lucide::Hand,
        NodeKind::Http { .. } => Lucide::Globe,
        NodeKind::Tls { .. } => Lucide::ShieldCheck,
        NodeKind::Dns { .. } => Lucide::Network,
        NodeKind::Agent { .. } => Lucide::Bot,
    }
}

/// One line of what a step does, for its card.
pub(super) fn step_line(node: &Node, cx: &gpui_kit::App) -> String {
    let first = |text: &str| text.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim().to_owned();
    match &node.kind {
        NodeKind::Command { hosts, command, .. } => {
            let store = Store::global(cx).read(cx);
            let mut names: Vec<String> = hosts
                .ids
                .iter()
                .map(|id| store.hosts.iter().find(|host| host.id.to_string() == *id).map_or_else(|| "a removed server".to_owned(), |host| host.label.clone()))
                .collect();
            names.extend(hosts.names.iter().cloned());
            if let Some(tag) = hosts.tag.as_ref().filter(|tag| !tag.trim().is_empty()) {
                names.push(format!("#{}", tag.trim()));
            }
            match (names.is_empty(), command.trim().is_empty()) {
                (true, true) => String::new(),
                (true, false) => format!("No servers · {}", first(command)),
                (false, true) => names.join(", "),
                (false, false) => format!("{} · {}", names.join(", "), first(command)),
            }
        }
        NodeKind::Local { command, .. } => first(command),
        NodeKind::If { value, test, against } if !value.trim().is_empty() => {
            if test.needs_against() { format!("{} {} {}", first(value), test.words(), against) } else { format!("{} {}", first(value), test.words()) }
        }
        NodeKind::If { .. } => String::new(),
        NodeKind::Wait { seconds } => match seconds {
            1 => "1 second".to_owned(),
            60 => "1 minute".to_owned(),
            n if n % 60 == 0 => format!("{} minutes", n / 60),
            n => format!("{n} seconds"),
        },
        NodeKind::Notify { title, .. } => first(title),
        NodeKind::Decide { yes_no, .. } => first(yes_no),
        NodeKind::Write { prompt } => first(prompt),
        NodeKind::Copy { host, direction, local, remote } => {
            let store = Store::global(cx).read(cx);
            let server = host
                .ids
                .first()
                .and_then(|id| store.hosts.iter().find(|saved| saved.id.to_string() == *id))
                .map(|saved| saved.label.clone())
                .or_else(|| host.names.first().cloned())
                .unwrap_or_else(|| "No server".to_owned());
            match direction {
                Direction::Upload => format!("{server} \u{00b7} {local} \u{2192} {remote}"),
                Direction::Download => format!("{server} \u{00b7} {remote} \u{2192} {local}"),
            }
        }
        NodeKind::Ask { title, .. } => first(title),
        NodeKind::Http { method, url, .. } if !url.trim().is_empty() => format!("{} {}", method.to_uppercase(), url.trim()),
        NodeKind::Http { .. } => String::new(),
        NodeKind::Tls { host, port: 443 } => host.trim().to_owned(),
        NodeKind::Tls { host, port } => format!("{}:{port}", host.trim()),
        NodeKind::Dns { name, record } => format!("{} {}", record.to_uppercase(), name.trim()),
        NodeKind::Agent { goal, .. } => first(goal),
    }
}

/// Where a step's way out sits, at 100%, from its card's top left.
fn port_at(ports: usize, ix: usize) -> [f32; 2] {
    [STEP_WIDTH * (ix as f32 + 1.) / (ports as f32 + 1.), STEP_HEIGHT]
}

/// Milliseconds as a short time, like `1.2 s` or `3 min`.
pub(super) fn took(ms: i64) -> String {
    match ms {
        ..1000 => format!("{ms} ms"),
        1000..60_000 => format!("{:.1} s", ms as f32 / 1000.),
        _ => format!("{} min", ms / 60_000),
    }
}

struct Wire {
    from: Point<Pixels>,
    to: Point<Pixels>,
    color: Hsla,
    width: f32,
}

impl FlowEditor {
    /// A point at 100% on the canvas, as a point in the canvas element.
    fn to_view(&self, at: [f32; 2]) -> Point<Pixels> {
        self.pan + point(px(at[0] * self.zoom), px(at[1] * self.zoom))
    }

    /// A point in the window, as a point in the canvas element.
    fn in_canvas(&self, position: Point<Pixels>) -> Point<Pixels> {
        position - self.bounds.get().origin
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.pick(Picked::Nothing, window, cx);
        self.drag = Some(Drag::Pan { start: event.position, pan: self.pan });
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let zoom = self.zoom;
        match drag {
            Drag::Pan { start, pan } => {
                self.pan = *pan + (event.position - *start);
                self.keep_fitted = false;
            }
            Drag::Step { id, start, at, moved } => {
                let delta = event.position - *start;
                let to = [at[0] + delta.x.as_f32() / zoom, at[1] + delta.y.as_f32() / zoom];
                *moved = true;
                let id = id.clone();
                if let Some(node) = self.step_mut(&id) {
                    node.at = to;
                }
            }
            Drag::Line { to, .. } => *to = event.position,
        }
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Drag::Step { moved: true, .. }) = self.drag.take() {
            self.changed(cx);
        }
        cx.notify();
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(20.));
        if event.modifiers.platform || event.modifiers.control {
            let factor = (1. - delta.y.as_f32() * 0.004).clamp(0.5, 2.);
            let at = self.in_canvas(event.position);
            self.zoom_by(factor, at, cx);
        } else {
            self.pan += delta;
            self.keep_fitted = false;
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn render_canvas(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let run = self.shown_run(cx);
        let step = |id: &str| run.as_ref().and_then(|run| run.steps.get(id));

        // The lines, drawn under the cards.
        let mut wires = Vec::new();
        let mut chips = Vec::new();
        for (ix, edge) in self.draft.edges.iter().enumerate() {
            let (Some(from), Some(to)) = (self.draft.node(&edge.from), self.draft.node(&edge.to)) else { continue };
            let ports = from.kind.ports();
            let port = edge.port_of(&from.kind);
            let port_ix = ports.iter().position(|p| *p == port).unwrap_or(0);
            let out = port_at(ports.len(), port_ix);
            let start = self.to_view([from.at[0] + out[0], from.at[1] + out[1]]);
            let end = self.to_view([to.at[0] + STEP_WIDTH / 2., to.at[1]]);
            let picked = self.picked == Picked::Line(ix);
            let mut color = match port {
                FAILED => theme.foreground.opacity(0.6),
                _ => theme.muted_foreground.opacity(0.7),
            };
            let mut width = 1.6;
            if let Some(from_step) = step(&from.id) {
                if from_step.port.as_deref() == Some(port) {
                    color = theme.foreground;
                    width = 2.4;
                } else if from_step.status != StepStatus::Running {
                    color = color.opacity(0.25);
                }
            }
            if picked {
                color = theme.foreground;
                width = 2.4;
            }
            wires.push(Wire { from: start, to: end, color, width: width * self.zoom.max(0.6) });
            // A chip in the middle names the way out and picks the line.
            let mid = bezier_mid(start, end);
            let label = (ports.len() > 1).then(|| SharedString::from(port.to_owned()));
            chips.push(self.render_chip(ix, mid, label, color, picked, cx));
        }
        let pending = match &self.drag {
            Some(Drag::Line { from, port, to }) => self.draft.node(from).map(|node| {
                let ports = node.kind.ports();
                let ix = ports.iter().position(|p| p == port).unwrap_or(0);
                let out = port_at(ports.len(), ix);
                let start = self.to_view([node.at[0] + out[0], node.at[1] + out[1]]);
                Wire { from: start, to: self.in_canvas(*to), color: theme.foreground, width: 2. }
            }),
            _ => None,
        };
        wires.extend(pending);

        let bounds = self.bounds.clone();
        let editor = cx.entity().downgrade();
        let lines = canvas(
            move |area, _, cx| {
                bounds.set(area);
                if let Some(editor) = editor.upgrade() {
                    let editor_now = editor.read(cx);
                    let refit = editor_now.keep_fitted && editor_now.fitted != Some(area.size);
                    // After this frame: a change made while drawing would not show.
                    if editor_now.fit_pending || refit {
                        cx.defer(move |cx| {
                            editor.update(cx, |editor, cx| {
                                editor.fit(area);
                                cx.notify();
                            })
                        });
                    }
                }
            },
            move |area, _, window, _| {
                for wire in wires {
                    draw_wire(&wire, area.origin, window);
                }
            },
        )
        .absolute()
        .size_full();

        let cards: Vec<AnyElement> =
            self.draft.nodes.iter().map(|node| self.render_card(node, step(&node.id), cx)).collect();

        let empty = self.draft.nodes.is_empty().then(|| {
            v_flex()
                .absolute()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .text_color(theme.muted_foreground)
                .child(Icon::new(Lucide::Workflow).size(px(32.)))
                .child(div().text_sm().child("Add a first step from the left."))
        });

        div()
            .id("flow-canvas")
            .key_context("FlowCanvas")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(theme.muted.opacity(0.35))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(lines)
            .children(empty)
            .children(cards)
            .children(chips)
            .child(self.render_palette(cx))
            .child(self.render_zoom(cx))
            .into_any_element()
    }

    fn render_chip(&self, ix: usize, at: Point<Pixels>, label: Option<SharedString>, color: Hsla, picked: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let zoom = self.zoom.max(0.7);
        let chip = h_flex()
            .id(SharedString::from(format!("line-{ix}")))
            .absolute()
            .gap_1()
            .items_center()
            .rounded_full()
            .border_1()
            .border_color(color)
            .bg(theme.background)
            .text_color(if picked { theme.foreground } else { theme.muted_foreground })
            .text_size(px(11. * zoom))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, _: &MouseDownEvent, window, cx| {
                    window.focus(&editor.focus, cx);
                    editor.pick(Picked::Line(ix), window, cx);
                    cx.stop_propagation();
                }),
            );
        let chip = match (&label, picked) {
            (Some(label), _) => chip.px(px(6. * zoom)).child(label.clone()),
            (None, true) => chip.px(px(6. * zoom)).child("line"),
            (None, false) => chip.size(px(10. * zoom)),
        };
        let chip = chip.when(picked, |chip| {
            chip.child(
                div()
                    .id(SharedString::from(format!("line-remove-{ix}")))
                    .child(Icon::new(Lucide::X).size(px(11. * zoom)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, _: &MouseDownEvent, window, cx| {
                            editor.remove_picked(window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
        });
        // Centred on the point; its size is a guess, close enough at any zoom.
        let half = match (&label, picked) {
            (Some(label), _) => point(px((label.len() as f32 * 3.5 + 6.) * zoom), px(8. * zoom)),
            (None, true) => point(px(20. * zoom), px(8. * zoom)),
            (None, false) => point(px(5. * zoom), px(5. * zoom)),
        };
        let at = at - half;
        chip.left(at.x).top(at.y).into_any_element()
    }

    fn render_card(&self, node: &Node, step: Option<&StepRecord>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let zoom = self.zoom;
        let z = |v: f32| px(v * zoom);
        let at = self.to_view(node.at);
        let picked = self.picked == Picked::Step(node.id.clone());
        let line = step_line(node, cx);
        let (border, badge): (Hsla, Option<AnyElement>) = match step.map(|step| step.status) {
            Some(StepStatus::Running) => (theme.foreground, Some(Spinner::new().xsmall().into_any_element())),
            Some(StepStatus::Done) => (theme.muted_foreground, Some(Icon::new(Lucide::CircleCheck).size(z(14.)).text_color(theme.muted_foreground).into_any_element())),
            Some(StepStatus::Failed) => (theme.foreground, Some(Icon::new(Lucide::CircleX).size(z(14.)).text_color(theme.foreground).into_any_element())),
            Some(StepStatus::Skipped) => (theme.border, Some(Icon::new(Lucide::CircleMinus).size(z(14.)).text_color(theme.muted_foreground).into_any_element())),
            None if picked => (theme.foreground, None),
            None => (theme.border, None),
        };
        let time = step.and_then(|step| Some(took(step.ended? - step.started?)));
        let skipped = step.is_some_and(|step| step.status == StepStatus::Skipped);
        let ports = node.kind.ports();
        let id = node.id.clone();
        let title = node.title();

        let header = h_flex()
            .gap(z(6.))
            .items_center()
            .child(Icon::new(kind_icon(&node.kind)).size(z(14.)).text_color(theme.muted_foreground))
            .child(div().flex_1().min_w_0().truncate().font_semibold().text_size(z(13.)).child(SharedString::from(title)))
            .children(time.map(|time| div().text_size(z(11.)).text_color(theme.muted_foreground).child(time)))
            // An AI wrote its command and no one has checked it yet.
            .when(node.ai_wrote && node.kind.command().is_some(), |header| {
                header.child(Icon::new(Lucide::Sparkles).size(z(12.)).text_color(theme.muted_foreground))
            })
            .children(badge);
        let body = div()
            .min_w_0()
            .truncate()
            .text_size(z(12.))
            .when(line.is_empty(), |el| el.italic().text_color(theme.muted_foreground).child("Not set up yet"))
            .when(!line.is_empty(), |el| el.text_color(theme.muted_foreground).child(SharedString::from(line)));
        // The names of the ways out, over their dots.
        let port_names = (ports.len() > 1).then(|| {
            div().relative().h(z(14.)).children(ports.iter().enumerate().map(|(ix, port)| {
                // Less the card's padding, and half the label's width.
                let x = port_at(ports.len(), ix)[0] - 12. - 22.;
                let color = if *port == FAILED { theme.foreground } else { theme.muted_foreground };
                div().absolute().left(z(x)).w(z(44.)).text_center().text_size(z(10.)).text_color(color).child(*port)
            }))
        });

        let card = v_flex()
            .id(SharedString::from(format!("step-{}", node.id)))
            .absolute()
            .left(at.x)
            .top(at.y)
            .w(z(STEP_WIDTH))
            .h(z(STEP_HEIGHT))
            .px(z(12.))
            .pt(z(10.))
            .gap(z(4.))
            .rounded(z(8.))
            .border(if picked { z(2.) } else { z(1.) })
            .border_color(border)
            .bg(theme.background)
            .shadow_sm()
            .when(skipped, |card| card.opacity(0.55))
            .cursor_move()
            .child(header)
            .child(body)
            .children(port_names)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let id = id.clone();
                    move |editor, event: &MouseDownEvent, window, cx| {
                        window.focus(&editor.focus, cx);
                        editor.pick(Picked::Step(id.clone()), window, cx);
                        if let Some(node) = editor.draft.node(&id) {
                            editor.drag = Some(Drag::Step { id: id.clone(), start: event.position, at: node.at, moved: false });
                        }
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener({
                    let id = id.clone();
                    move |editor, _: &MouseUpEvent, window, cx| {
                        match editor.drag.take() {
                            Some(Drag::Line { from, port, .. }) => editor.connect(&from, &port, &id, window, cx),
                            Some(Drag::Step { moved: true, .. }) => editor.changed(cx),
                            _ => {}
                        }
                        cx.notify();
                    }
                }),
            );

        // The dot each line comes in at, and the dots lines leave from.
        let input = div()
            .absolute()
            .left(at.x + z(STEP_WIDTH / 2. - PORT / 2.))
            .top(at.y - z(PORT / 2.))
            .size(z(PORT))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background);
        let outputs = ports.iter().enumerate().map(|(ix, port)| {
            let spot = port_at(ports.len(), ix);
            let (from, port) = (id.clone(), (*port).to_owned());
            div()
                .id(SharedString::from(format!("port-{}-{port}", node.id)))
                .absolute()
                .left(at.x + z(spot[0] - PORT / 2.))
                .top(at.y + z(spot[1] - PORT / 2.))
                .size(z(PORT))
                .rounded_full()
                .border_2()
                .border_color(if port == FAILED { theme.foreground } else { theme.muted_foreground })
                .bg(theme.background)
                .cursor_crosshair()
                .hover(|dot| dot.bg(theme.foreground))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |editor, event: &MouseDownEvent, window, cx| {
                        window.focus(&editor.focus, cx);
                        editor.drag = Some(Drag::Line { from: from.clone(), port: port.clone(), to: event.position });
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
        });
        div().child(card).child(input).children(outputs).into_any_element()
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx).clone();
        // A narrow canvas keeps room for the steps: icons only, named on hover.
        let icons_only = self.bounds.get().size.width.as_f32() < WIDE_CANVAS;
        let theme = cx.theme();
        v_flex()
            .id("flow-palette")
            .absolute()
            .top_3()
            .left_3()
            .p_1()
            .gap_0p5()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(!icons_only, |palette| {
                palette.child(div().px_2().pt_1().pb_0p5().text_xs().text_color(theme.muted_foreground).child("Add a step"))
            })
            .children(PALETTE.iter().filter(|(kind, ..)| offered(kind, &settings)).map(|(kind, label, icon)| {
                let kind = *kind;
                h_flex()
                    .id(SharedString::from(format!("add-{kind}")))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|row| row.bg(theme.secondary))
                    .child(Icon::new(*icon).size(px(14.)).text_color(theme.muted_foreground))
                    .when(!icons_only, |row| row.child(*label))
                    .when(icons_only, |row| {
                        let tip = format!("Add: {label}");
                        row.py_1p5().tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                    })
                    .on_click(cx.listener(move |editor, _, window, cx| {
                        let kind = new_kind(kind, &editor.draft.name);
                        editor.add_step(kind, window, cx);
                    }))
            }))
    }

    fn render_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let middle = |editor: &FlowEditor| {
            let size = editor.bounds.get().size;
            point(size.width / 2., size.height / 2.)
        };
        h_flex()
            .id("flow-zoom")
            .absolute()
            .bottom_3()
            .left_3()
            .p_0p5()
            .gap_0p5()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Button::new("zoom-out").ghost().xsmall().icon(Icon::new(Lucide::ZoomOut)).tooltip("Zoom out").on_click(
                cx.listener(move |editor, _, _, cx| {
                    let at = middle(editor);
                    editor.zoom_by(1. / 1.2, at, cx);
                }),
            ))
            .child(
                div()
                    .w(px(40.))
                    .text_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{}%", (self.zoom * 100.).round() as i32)),
            )
            .child(Button::new("zoom-in").ghost().xsmall().icon(Icon::new(Lucide::ZoomIn)).tooltip("Zoom in").on_click(
                cx.listener(move |editor, _, _, cx| {
                    let at = middle(editor);
                    editor.zoom_by(1.2, at, cx);
                }),
            ))
            .child(Button::new("zoom-fit").ghost().xsmall().icon(Icon::new(Lucide::Maximize)).tooltip("Show all steps").on_click(
                cx.listener(|editor, _, _, cx| {
                    editor.fit_pending = true;
                    editor.keep_fitted = true;
                    cx.notify();
                }),
            ))
    }
}

/// The middle of the curve a line takes.
fn bezier_mid(from: Point<Pixels>, to: Point<Pixels>) -> Point<Pixels> {
    let (c1, c2) = controls(from, to);
    let x = (from.x.as_f32() + 3. * c1.x.as_f32() + 3. * c2.x.as_f32() + to.x.as_f32()) / 8.;
    let y = (from.y.as_f32() + 3. * c1.y.as_f32() + 3. * c2.y.as_f32() + to.y.as_f32()) / 8.;
    point(px(x), px(y))
}

/// Lines leave a step going down and come into the next one from above.
fn controls(from: Point<Pixels>, to: Point<Pixels>) -> (Point<Pixels>, Point<Pixels>) {
    let bend = px(((to.y - from.y).as_f32().abs() / 2.).max(40.));
    (point(from.x, from.y + bend), point(to.x, to.y - bend))
}

fn draw_wire(wire: &Wire, origin: Point<Pixels>, window: &mut Window) {
    let (from, to) = (wire.from + origin, wire.to + origin);
    let (c1, c2) = controls(from, to);
    let mut path = PathBuilder::stroke(px(wire.width));
    path.move_to(from);
    path.cubic_bezier_to(to, c1, c2);
    if let Ok(path) = path.build() {
        window.paint_path(path, wire.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_short() {
        assert_eq!(took(250), "250 ms");
        assert_eq!(took(1250), "1.2 s");
        assert_eq!(took(125_000), "2 min");
    }

    #[test]
    fn ways_out_spread_along_the_bottom() {
        assert_eq!(port_at(1, 0), [STEP_WIDTH / 2., STEP_HEIGHT]);
        let (ok, failed) = (port_at(2, 0), port_at(2, 1));
        assert!(ok[0] < failed[0]);
    }
}
