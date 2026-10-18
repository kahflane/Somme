//! The script log beside Settings: every script agents ran, in a table, and
//! one run's full details in a panel of its own.

use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};

use crate::mcp::{Agents, Run};
use crate::settings::Settings;
use crate::ui::common::{APP_NAME, Lucide, ago, copy_text, notify_ok};

/// How a run went, for its icon, colour and word.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Running,
    Unknown,
    Failed,
    TimedOut,
    Error,
    Ok,
}

impl Outcome {
    fn of(run: &Run, live: bool) -> Self {
        let Some(end) = &run.end else {
            return if live { Outcome::Running } else { Outcome::Unknown };
        };
        if end.error.is_some() {
            Outcome::Failed
        } else if end.timed_out {
            Outcome::TimedOut
        } else if end.exit_status == Some(0) {
            Outcome::Ok
        } else {
            Outcome::Error
        }
    }

    fn icon(self) -> Lucide {
        match self {
            Outcome::Running => Lucide::LoaderCircle,
            Outcome::Unknown => Lucide::CircleQuestionMark,
            Outcome::Failed => Lucide::CircleAlert,
            Outcome::TimedOut => Lucide::Clock,
            Outcome::Error => Lucide::CircleX,
            Outcome::Ok => Lucide::CircleCheck,
        }
    }

    fn tint(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Outcome::Running => theme.info,
            Outcome::Unknown => theme.muted_foreground,
            Outcome::Failed | Outcome::Error => theme.danger,
            Outcome::TimedOut => theme.warning,
            Outcome::Ok => theme.success,
        }
    }

    /// One or two words for the table.
    fn word(self, run: &Run) -> String {
        let end = run.end.as_ref();
        match self {
            Outcome::Running => "Running".into(),
            Outcome::Unknown => "No end".into(),
            Outcome::Failed => "Did not run".into(),
            Outcome::TimedOut => "Timed out".into(),
            Outcome::Ok => "Exit 0".into(),
            Outcome::Error => match end.and_then(|end| end.exit_status) {
                Some(code) => format!("Exit {code}"),
                None => end.and_then(|end| end.exit_signal.clone()).map_or("Stopped".into(), |s| format!("Signal {s}")),
            },
        }
    }

    /// The full sentence for the details.
    fn sentence(self, run: &Run) -> String {
        let end = run.end.as_ref();
        match self {
            Outcome::Running => "Running now.".into(),
            Outcome::Unknown => format!("{APP_NAME} closed before the script ended."),
            Outcome::Failed => end.and_then(|end| end.error.clone()).unwrap_or_default(),
            Outcome::TimedOut => format!("Stopped after its {} s limit.", run.timeout_seconds),
            Outcome::Ok => "Ended with exit code 0.".into(),
            Outcome::Error => self.word(run),
        }
    }
}

fn took(run: &Run) -> Option<f64> {
    run.end.as_ref().map(|end| (end.at - run.started) as f64 / 1000.)
}

fn local_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%b %-d, %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn first_line(script: &str) -> String {
    script.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim().to_owned()
}

const COLUMNS: &[(&str, &str, f32)] = &[
    ("status", "Status", 110.),
    ("time", "Time", 140.),
    ("host", "Server", 150.),
    ("script", "Script", 220.),
    ("agent", "Agent", 130.),
    ("took", "Took", 70.),
];

/// The table's rows: the log, filtered and sorted.
struct RunTable {
    runs: Vec<Run>,
    live: HashSet<String>,
    rows: Vec<usize>,
    query: String,
    sort: Option<(usize, ColumnSort)>,
}

impl RunTable {
    fn row(&self, ix: usize) -> Option<&Run> {
        self.runs.get(*self.rows.get(ix)?)
    }

    fn rebuild(&mut self) {
        let query = self.query.to_lowercase();
        let found = |run: &Run| {
            query.is_empty()
                || [&run.host.name, &run.host.address, &run.agent, &run.script]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&query))
        };
        self.rows = (0..self.runs.len()).filter(|ix| found(&self.runs[*ix])).collect();
        let Some((col, sort)) = self.sort else { return };
        let runs = &self.runs;
        let live = &self.live;
        self.rows.sort_by(|a, b| {
            let (a, b) = (&runs[*a], &runs[*b]);
            let order = match COLUMNS[col].0 {
                "status" => Outcome::of(a, live.contains(&a.id)).cmp(&Outcome::of(b, live.contains(&b.id))),
                "host" => a.host.name.to_lowercase().cmp(&b.host.name.to_lowercase()),
                "agent" => a.agent.cmp(&b.agent),
                "script" => a.script.cmp(&b.script),
                "took" => took(a).unwrap_or(f64::MAX).total_cmp(&took(b).unwrap_or(f64::MAX)),
                _ => b.started.cmp(&a.started),
            };
            if sort == ColumnSort::Descending { order.reverse() } else { order }
        });
    }
}

impl TableDelegate for RunTable {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, width) = COLUMNS[col_ix];
        Column::new(key, name).width(px(width)).sortable()
    }

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, _: &mut Window, _: &mut Context<TableState<Self>>) {
        self.sort = (sort != ColumnSort::Default).then_some((col_ix, sort));
        self.rebuild();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(run) = self.row(row_ix) else { return div().into_any_element() };
        let muted = cx.theme().muted_foreground;
        match COLUMNS[col_ix].0 {
            "status" => {
                let outcome = Outcome::of(run, self.live.contains(&run.id));
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(Icon::new(outcome.icon()).size(px(14.)).text_color(outcome.tint(cx)))
                    .child(outcome.word(run))
                    .into_any_element()
            }
            "time" => div().child(local_time(run.started)).into_any_element(),
            "host" => div().truncate().child(SharedString::from(run.host.name.clone())).into_any_element(),
            "script" => div()
                .truncate()
                .font_family(Settings::get(cx).font_family.clone())
                .child(first_line(&run.script))
                .into_any_element(),
            "agent" => div().truncate().text_color(muted).child(SharedString::from(run.agent.clone())).into_any_element(),
            _ => div()
                .text_color(muted)
                .child(took(run).map(|s| format!("{s:.1} s")).unwrap_or_default())
                .into_any_element(),
        }
    }
}

pub enum AgentLogsEvent {
    Close,
    Open(String),
}

/// The log as a table, newest first.
pub struct AgentLogsPanel {
    agents: Entity<Agents>,
    table: Entity<TableState<RunTable>>,
    filter: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AgentLogsEvent> for AgentLogsPanel {}

impl AgentLogsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let agents = Agents::global(cx);
        let delegate = RunTable {
            runs: Vec::new(),
            live: HashSet::new(),
            rows: Vec::new(),
            query: String::new(),
            sort: None,
        };
        let table = cx.new(|cx| TableState::new(delegate, window, cx).col_movable(false));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by server, agent or script"));
        let subscriptions = vec![
            cx.observe(&agents, |panel, _, cx| panel.reload(cx)),
            cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
                if let TableEvent::SelectRow(ix) = event
                    && let Some(run) = table.read(cx).delegate().row(*ix)
                {
                    let id = run.id.clone();
                    cx.emit(AgentLogsEvent::Open(id));
                }
            }),
            cx.subscribe(&filter, |panel, filter, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let query = filter.read(cx).value().trim().to_owned();
                    panel.table.update(cx, |table, cx| {
                        table.delegate_mut().query = query;
                        table.delegate_mut().rebuild();
                        table.refresh(cx);
                    });
                }
            }),
        ];
        let mut panel = Self {
            agents,
            table,
            filter,
            _subscriptions: subscriptions,
        };
        panel.reload(cx);
        panel
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let agents = self.agents.read(cx);
        let (runs, live) = (agents.runs.clone(), agents.live.clone());
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.runs = runs;
            delegate.live = live;
            delegate.rebuild();
            table.refresh(cx);
        });
        cx.notify();
    }
}

impl Render for AgentLogsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let agents = self.agents.read(cx);
        let shown = self.table.read(cx).delegate().rows.len();
        let total = agents.runs.len();
        let count = if shown == total { format!("{total} runs") } else { format!("{shown} of {total} runs") };
        let path = agents.log_path().display().to_string();
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(44.))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Icon::new(Lucide::ScrollText).size(px(16.)))
                    .child(div().font_semibold().child("Script log"))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(count))
                    .child(div().flex_1())
                    .child(
                        Button::new("agent-logs-close")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::X))
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AgentLogsEvent::Close))),
                    ),
            )
            .child(div().flex_none().p_2().child(Input::new(&self.filter).small().cleanable(true)))
            .child(div().flex_1().min_h_0().px_2().child(DataTable::new(&self.table).stripe(true).small()))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(format!("Kept in {path}"))),
            )
    }
}

pub enum RunDetailEvent {
    Close,
}

/// Everything about one run.
pub struct RunDetailPanel {
    agents: Entity<Agents>,
    id: String,
    scroll: ScrollHandle,
    _watch: Subscription,
}

impl EventEmitter<RunDetailEvent> for RunDetailPanel {}

impl RunDetailPanel {
    pub fn new(id: String, cx: &mut Context<Self>) -> Self {
        let agents = Agents::global(cx);
        // A running script's end shows up as soon as it comes.
        let watch = cx.observe(&agents, |_, _, cx| cx.notify());
        Self {
            agents,
            id,
            scroll: ScrollHandle::new(),
            _watch: watch,
        }
    }

    pub fn show(&mut self, id: String, cx: &mut Context<Self>) {
        self.id = id;
        self.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
        cx.notify();
    }
}

/// `text` as a selectable code block. The fence is longer than any run of
/// backticks inside, so the text cannot end it.
fn code_view(id: SharedString, lang: &str, text: &str) -> TextView {
    let mut fence = "```".to_owned();
    while text.contains(fence.as_str()) {
        fence.push('`');
    }
    TextView::markdown(id, format!("{fence}{lang}\n{text}\n{fence}")).selectable(true)
}

impl Render for RunDetailPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let agents = self.agents.read(cx);
        let run = agents.runs.iter().find(|run| run.id == self.id).cloned();
        let live = agents.live.contains(&self.id);
        let header = |title: SharedString, outcome: Option<Outcome>| {
            h_flex()
                .flex_none()
                .h(px(44.))
                .px_3()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .children(outcome.map(|o| Icon::new(o.icon()).size(px(16.)).text_color(o.tint(cx))))
                .child(div().flex_1().min_w_0().truncate().font_semibold().child(title))
                .child(
                    Button::new("agent-run-close")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::X))
                        .tooltip("Close")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(RunDetailEvent::Close))),
                )
        };
        let Some(run) = run else {
            return v_flex()
                .size_full()
                .bg(theme.background)
                .border_l_1()
                .border_color(theme.border)
                .child(header("Run".into(), None))
                .child(div().p_3().text_sm().text_color(theme.muted_foreground).child("This run is no longer listed."));
        };
        let outcome = Outcome::of(&run, live);
        let end = run.end.clone();
        let fact = |label: &'static str, value: String| {
            h_flex()
                .gap_3()
                .items_start()
                .text_sm()
                .child(div().w(px(84.)).flex_none().text_color(theme.muted_foreground).child(label))
                .child(div().flex_1().min_w_0().child(SharedString::from(value)))
        };
        let mut facts = v_flex()
            .gap_1p5()
            .child(fact("Result", outcome.sentence(&run)))
            .child(fact("Server", format!("{} ({})", run.host.name, run.host.address)))
            .child(fact("Agent", run.agent.clone()))
            .child(fact("Started", format!("{} · {}", local_time(run.started), ago(run.started))));
        if let Some(end) = &end {
            facts = facts.child(fact("Ended", local_time(end.at)));
        }
        if let Some(seconds) = took(&run) {
            facts = facts.child(fact("Took", format!("{seconds:.1} s")));
        }
        facts = facts.child(fact("Time limit", format!("{} s", run.timeout_seconds))).child(fact("Run id", run.id.clone()));
        let label = |text: &'static str| div().pt_2().text_xs().font_semibold().text_color(theme.muted_foreground).child(text);
        let script = run.script.clone();
        let mut body = v_flex()
            .p_3()
            .gap_2()
            .child(facts)
            .child(
                h_flex()
                    .pt_2()
                    .items_center()
                    .child(div().flex_1().text_xs().font_semibold().text_color(theme.muted_foreground).child("SCRIPT"))
                    .child(
                        Button::new("agent-run-copy")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::Copy))
                            .label("Copy")
                            .on_click(move |_, _, cx| {
                                copy_text(script.clone(), cx);
                                notify_ok(cx, "Copied the script");
                            }),
                    ),
            )
            .child(code_view(format!("agent-run-script-{}", run.id).into(), "sh", &run.script));
        match &end {
            Some(end) => {
                for (name, id, text) in [("STDOUT", "out", &end.stdout), ("STDERR", "err", &end.stderr)] {
                    body = body.child(label(name)).child(if text.is_empty() {
                        div().text_sm().text_color(theme.muted_foreground).child("Nothing.").into_any_element()
                    } else {
                        code_view(format!("agent-run-{id}-{}", run.id).into(), "text", text.trim_end()).into_any_element()
                    });
                }
            }
            None if live => body = body.child(label("OUTPUT")).child(div().text_sm().child("Shows when the script ends.")),
            None => {}
        }
        v_flex()
            .size_full()
            .bg(theme.background)
            .border_l_1()
            .border_color(theme.border)
            .child(header(format!("{} · {}", run.host.name, first_line(&run.script)).into(), Some(outcome)))
            .child(
                div()
                    .id("agent-run-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}
