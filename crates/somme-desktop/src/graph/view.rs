//! The commit graph beside the terminals: every branch's commits with the
//! lines between them, and what the picked commit changed.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Bounds, Context, EventEmitter, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task, UniformListScrollHandle,
    Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, rgb, size, uniform_list,
};

use super::log::{self, Commit, Details, Log, Ref, RefKind, Row};
use crate::ui::common::{Lucide, ago};

const ROW: f32 = 26.;
const LANE: f32 = 14.;
/// The graph is cut off past this many columns.
const MAX_LANES: usize = 16;

/// Branch colours, as in most commit graphs.
const COLORS: [u32; 8] = [0x4493f8, 0xe3a008, 0xdb61a2, 0x3fb950, 0xa371f7, 0x2ab0a6, 0xf0883e, 0x8b949e];

fn color(ix: usize) -> Hsla {
    rgb(COLORS[ix % COLORS.len()]).into()
}

pub enum GraphEvent {
    Close,
    /// Show what this commit changed, in the repo at this folder.
    OpenCommit(PathBuf, String),
}

enum State {
    Loading,
    Failed(String),
    Ready { log: Rc<Log>, rows: Rc<Vec<Row>>, lanes: usize },
}

enum Picked {
    Loading,
    Failed(String),
    Ready(Details),
}

pub struct GraphView {
    /// The shell's folder.
    dir: PathBuf,
    state: State,
    /// How many commits to read.
    count: usize,
    refreshing: bool,
    selected: Option<usize>,
    picked: Option<(String, Picked)>,
    scroll: UniformListScrollHandle,
    _reading: Task<()>,
    _picking: Task<()>,
}

impl EventEmitter<GraphEvent> for GraphView {}

impl GraphView {
    pub fn new(dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            dir,
            state: State::Loading,
            count: log::PAGE,
            refreshing: false,
            selected: None,
            picked: None,
            scroll: UniformListScrollHandle::new(),
            _reading: Task::ready(()),
            _picking: Task::ready(()),
        };
        view.reload(cx);
        view
    }

    /// Reads the graph again after the shell ran a command in `dir`, which
    /// may have made commits or moved to another repo.
    pub fn follow(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if !dir.starts_with(self.root()) {
            self.dir = dir;
            self.count = log::PAGE;
            self.selected = None;
            self.picked = None;
            self.state = State::Loading;
        }
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.refreshing = true;
        let (dir, count) = (self.dir.clone(), self.count);
        self._reading = cx.spawn(async move |view, cx| {
            let loaded = cx
                .background_spawn(async move {
                    log::load(&dir, count).map(|log| {
                        let rows = log::layout(&log.commits);
                        (log, rows)
                    })
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.refreshing = false;
                let selected = view.selected_sha();
                view.state = match loaded {
                    Ok((log, rows)) => {
                        let lanes = rows.iter().map(Row::width).max().unwrap_or(1).min(MAX_LANES);
                        State::Ready { log: Rc::new(log), rows: Rc::new(rows), lanes }
                    }
                    Err(err) => State::Failed(err),
                };
                // The picked commit stays picked when it is still there.
                view.selected = selected.and_then(|sha| view.commits().iter().position(|commit| commit.sha == sha));
                if view.selected.is_none() {
                    view.picked = None;
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn commits(&self) -> &[Commit] {
        match &self.state {
            State::Ready { log, .. } => &log.commits,
            _ => &[],
        }
    }

    fn selected_sha(&self) -> Option<String> {
        self.selected.and_then(|ix| self.commits().get(ix)).map(|commit| commit.sha.clone())
    }

    fn root(&self) -> PathBuf {
        match &self.state {
            State::Ready { log, .. } => log.root.clone(),
            _ => self.dir.clone(),
        }
    }

    fn older(&mut self, cx: &mut Context<Self>) {
        self.count += log::PAGE;
        self.reload(cx);
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selected == Some(ix) {
            self.selected = None;
            self.picked = None;
            cx.notify();
            return;
        }
        let Some(sha) = self.commits().get(ix).map(|commit| commit.sha.clone()) else { return };
        self.selected = Some(ix);
        self.picked = Some((sha.clone(), Picked::Loading));
        let root = self.root();
        self._picking = cx.spawn(async move |view, cx| {
            let asked = sha.clone();
            let details = cx.background_spawn(async move { log::details(&root, &asked) }).await;
            let _ = view.update(cx, |view, cx| {
                if view.picked.as_ref().is_some_and(|(picked, _)| *picked == sha) {
                    view.picked = Some((
                        sha,
                        match details {
                            Ok(details) => Picked::Ready(details),
                            Err(err) => Picked::Failed(err),
                        },
                    ));
                    cx.notify();
                }
            });
        });
        cx.notify();
    }

    fn open_commit(&self, cx: &mut Context<Self>) {
        if let Some(sha) = self.selected_sha() {
            cx.emit(GraphEvent::OpenCommit(self.root(), sha));
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = self.root().file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        h_flex()
            .flex_none()
            .h(px(40.))
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Lucide::GitGraph).size(px(16.)))
            .child(div().font_semibold().child("Graph"))
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(theme.muted_foreground).child(name))
            .child(
                Button::new("graph-refresh")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::RefreshCw))
                    .loading(self.refreshing)
                    .tooltip("Refresh")
                    .on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
            )
            .child(
                Button::new("graph-close")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::X))
                    .tooltip("Close")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::Close))),
            )
    }

    fn render_list(&self, log: &Rc<Log>, rows: &Rc<Vec<Row>>, lanes: usize, cx: &mut Context<Self>) -> AnyElement {
        let (log, rows) = (log.clone(), rows.clone());
        let selected = self.selected;
        let count = log.commits.len() + usize::from(log.more);
        uniform_list(
            "graph-commits",
            count,
            cx.processor(move |_view, range: std::ops::Range<usize>, _, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| {
                        let Some((commit, row)) = log.commits.get(ix).zip(rows.get(ix)) else {
                            return div()
                                .id(ix)
                                .h(px(ROW))
                                .px_3()
                                .flex()
                                .items_center()
                                .child(
                                    Button::new("graph-older")
                                        .ghost()
                                        .xsmall()
                                        .label("Show older commits")
                                        .on_click(cx.listener(|view, _, _, cx| view.older(cx))),
                                )
                                .into_any_element();
                        };
                        let row = row.clone();
                        let (head, merge) = (commit.head, commit.parents.len() > 1);
                        let background = theme.background;
                        let graph = canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| paint_row(&row, head, merge, bounds, background, window),
                        )
                        .flex_none()
                        .w(px(LANE * lanes as f32 + 4.))
                        .h_full();
                        let lane_color = color(rows[ix].color);
                        h_flex()
                            .id(ix)
                            .h(px(ROW))
                            .pr_2()
                            .gap_1p5()
                            .items_center()
                            .text_sm()
                            .cursor_pointer()
                            .when(selected == Some(ix), |el| el.bg(theme.accent))
                            .when(selected != Some(ix), |el| el.hover(|el| el.bg(theme.accent.opacity(0.4))))
                            .child(graph)
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .when(merge, |el| el.text_color(theme.muted_foreground))
                                            .when(head, |el| el.font_semibold())
                                            .child(SharedString::from(commit.subject.clone())),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .max_w(px(140.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(SharedString::from(commit.author.clone())),
                                    ),
                            )
                            .children(commit.refs.iter().take(3).map(|found| badge(found, lane_color, cx)))
                            .when(commit.refs.len() > 3, |el| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("+{}", commit.refs.len() - 3)),
                                )
                            })
                            .on_click(cx.listener(move |view, _, _, cx| view.pick(ix, cx)))
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn render_picked(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let commit = self.selected.and_then(|ix| self.commits().get(ix))?;
        let (_, picked) = self.picked.as_ref()?;
        let theme = cx.theme().clone();
        let body = match picked {
            Picked::Loading => div().text_sm().text_color(theme.muted_foreground).child("Reading…").into_any_element(),
            Picked::Failed(err) => div().text_sm().text_color(theme.danger).child(err.clone()).into_any_element(),
            Picked::Ready(details) => {
                let rest = details.message.split_once('\n').map(|(_, rest)| rest.trim().to_owned()).unwrap_or_default();
                let files = details.files.iter().enumerate().map(|(ix, (path, added, removed))| {
                    let counts = match (added, removed) {
                        (Some(added), Some(removed)) => h_flex()
                            .flex_none()
                            .gap_1()
                            .child(div().text_color(theme.success).child(format!("+{added}")))
                            .child(div().text_color(theme.danger).child(format!("−{removed}"))),
                        _ => h_flex().flex_none().text_color(theme.muted_foreground).child("binary"),
                    };
                    h_flex()
                        .id(("graph-file", ix))
                        .gap_2()
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|el| el.bg(theme.accent.opacity(0.5)))
                        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(path.clone())))
                        .child(counts)
                        .on_click(cx.listener(|view, _, _, cx| view.open_commit(cx)))
                });
                v_flex()
                    .gap_2()
                    .when(!rest.is_empty(), |el| {
                        el.child(div().text_sm().text_color(theme.muted_foreground).whitespace_normal().child(rest))
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(match details.files.len() {
                                1 => "1 file changed".to_owned(),
                                files => format!("{files} files changed"),
                            }),
                    )
                    .child(v_flex().text_xs().font_family(theme.mono_font_family.clone()).children(files))
                    .into_any_element()
            }
        };
        let author = match &self.picked {
            Some((_, Picked::Ready(details))) if !details.email.is_empty() => {
                format!("{} <{}>", commit.author, details.email)
            }
            _ => commit.author.clone(),
        };
        Some(
            v_flex()
                .id("graph-picked")
                .flex_none()
                .max_h(px(280.))
                .overflow_y_scroll()
                .p_3()
                .gap_2()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(div().flex_1().min_w_0().font_semibold().child(SharedString::from(commit.subject.clone())))
                        .child(
                            Button::new("graph-open-commit")
                                .xsmall()
                                .icon(Icon::new(Lucide::FileDiff))
                                .label("Open changes")
                                .on_click(cx.listener(|view, _, _, cx| view.open_commit(cx))),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().min_w_0().truncate().child(author))
                        .child("·")
                        .child(ago(commit.time))
                        .child("·")
                        .child(div().font_family(theme.mono_font_family.clone()).child(commit.sha[..7].to_owned())),
                )
                .child(body)
                .into_any_element(),
        )
    }
}

/// A branch or tag name beside its commit.
fn badge(found: &Ref, lane: Hsla, cx: &mut Context<GraphView>) -> AnyElement {
    let theme = cx.theme();
    let (icon, tint) = match found.kind {
        RefKind::Head => (Lucide::CircleDot, lane),
        RefKind::Branch => (Lucide::GitBranch, lane),
        RefKind::Remote => (Lucide::Cloud, theme.muted_foreground),
        RefKind::Tag => (Lucide::Tag, theme.warning),
    };
    h_flex()
        .flex_none()
        .max_w(px(150.))
        .h(px(18.))
        .px_1p5()
        .gap_1()
        .items_center()
        .rounded_full()
        .text_xs()
        .when(found.kind == RefKind::Head, |el| el.bg(tint).text_color(rgb(0xffffff)).font_semibold())
        .when(found.kind != RefKind::Head, |el| el.bg(tint.opacity(0.18)).text_color(theme.foreground))
        .child(Icon::new(icon).size(px(11.)))
        .child(div().min_w_0().truncate().child(SharedString::from(found.name.clone())))
        .into_any_element()
}

fn paint_row(row: &Row, head: bool, merge: bool, bounds: Bounds<Pixels>, background: Hsla, window: &mut Window) {
    let x = |column: usize| bounds.left() + px(2. + LANE * column.min(MAX_LANES - 1) as f32 + LANE / 2.);
    let (top, mid, bottom) = (bounds.top(), bounds.center().y, bounds.bottom());
    for &(column, lane) in &row.through {
        line(point(x(column), top), point(x(column), bottom), color(lane), window);
    }
    for &(column, lane) in &row.into {
        line(point(x(column), top), point(x(row.column), mid), color(lane), window);
    }
    for &(column, lane) in &row.out {
        line(point(x(row.column), mid), point(x(column), bottom), color(lane), window);
    }
    let tint = color(row.color);
    let radius = px(if merge { 3.5 } else { 4.5 });
    let dot = Bounds::centered_at(point(x(row.column), mid), size(radius * 2., radius * 2.));
    if head {
        // The commit the work folder is at: a ring, as in other graphs.
        window.paint_quad(fill(dot, background).corner_radii(radius).border_widths(px(2.)).border_color(tint));
    } else {
        window.paint_quad(fill(dot, tint).corner_radii(radius));
    }
}

/// A line between two points, curved where it changes column.
fn line(from: Point<Pixels>, to: Point<Pixels>, tint: Hsla, window: &mut Window) {
    let mut path = PathBuilder::stroke(px(1.6));
    path.move_to(from);
    if from.x == to.x {
        path.line_to(to);
    } else {
        let middle = from.y + (to.y - from.y) / 2.;
        path.cubic_bezier_to(to, point(from.x, middle), point(to.x, middle));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, tint);
    }
}

impl Render for GraphView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body = match &self.state {
            State::Loading => div().p_3().text_sm().text_color(theme.muted_foreground).child("Reading commits…").into_any_element(),
            State::Failed(err) => div().p_3().text_sm().text_color(theme.danger).child(err.clone()).into_any_element(),
            State::Ready { log, .. } if log.commits.is_empty() => {
                div().p_3().text_sm().text_color(theme.muted_foreground).child("No commits yet.").into_any_element()
            }
            State::Ready { log, rows, lanes } => {
                let (log, rows, lanes) = (log.clone(), rows.clone(), *lanes);
                self.render_list(&log, &rows, lanes, cx)
            }
        };
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(self.render_header(cx))
            .child(v_flex().flex_1().min_h_0().child(body))
            .children(self.render_picked(cx))
    }
}
