//! The find bar of JSON and JSON Lines files: find text, or pull data out
//! with a jq filter.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Escape, Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityId, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px, relative,
};

use super::{Body, EditorPane, OpenFile};
use crate::editor::jq;
use crate::editor::language;
use crate::settings::Settings;
use crate::ui::common::{Lucide, copy_text};

/// Typing pauses this long before a filter runs.
const JQ_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FindMode {
    Text,
    Jq,
}

/// What the last filter gave, without the text, which is in the results view.
struct Summary {
    count: usize,
    cut: bool,
    error: Option<String>,
    errors: usize,
}

pub(super) struct FindBar {
    mode: FindMode,
    input: Entity<InputState>,
    /// jq: every value in one array, as `jq --slurp`.
    slurp: bool,
    results: Entity<EditorState>,
    outcome: Option<Result<Summary, String>>,
    running: bool,
    /// Tells the filter running off the UI thread to stop.
    cancel: Arc<AtomicBool>,
    _run: Option<Task<()>>,
    _subscription: Subscription,
}

impl FindBar {
    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self._run = None;
        self.running = false;
    }
}

/// JSON, or a JSON value per line.
pub(super) fn is_json(file: &OpenFile) -> bool {
    language::detect(&file.path).is_some_and(|language| language.grammar == "json")
}

fn placeholder(mode: FindMode) -> &'static str {
    match mode {
        FindMode::Text => "Find",
        FindMode::Jq => "jq filter, like .items[] | select(.ok) | .name",
    }
}

impl EditorPane {
    fn file_by_id(&mut self, id: EntityId) -> Option<&mut OpenFile> {
        self.files.iter_mut().find(|file| file.body.id() == id)
    }

    /// Opens the find bar of the active JSON file, in `mode` or as it was.
    pub(super) fn open_find(&mut self, mode: Option<FindMode>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get_mut(self.active).filter(|file| is_json(file)) else { return };
        let id = file.body.id();
        // The plain editor for big files has its own text search.
        let wants_text = mode.map_or(file.find.is_none(), |mode| mode == FindMode::Text);
        if let Body::Big(big) = &file.body
            && wants_text
        {
            let big = big.clone();
            self.close_find(window, cx);
            big.update(cx, |big, cx| big.open_find(window, cx));
            return;
        }
        if let Some(bar) = file.find.as_mut() {
            if let Some(mode) = mode.filter(|mode| *mode != bar.mode) {
                bar.mode = mode;
                bar.input.update(cx, |input, cx| input.set_placeholder(placeholder(mode), window, cx));
                if let Body::Code(editor) = &file.body {
                    editor.update(cx, |editor, cx| editor.close_search(cx));
                }
            }
            bar.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            self.find_changed(id, window, cx);
            return;
        }
        let mode = mode.unwrap_or(FindMode::Text);
        // A short selection on one line is what to find.
        let prefill = match &file.body {
            Body::Code(editor) if mode == FindMode::Text => {
                let editor = editor.read(cx);
                let range = editor.selected_range();
                Some(editor.text().slice(range).to_string())
                    .filter(|text| !text.is_empty() && text.len() <= 200 && !text.contains('\n'))
            }
            _ => None,
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(placeholder(mode)).default_value(prefill.unwrap_or_default())
        });
        let subscription = cx.subscribe_in(&input, window, move |pane, _, event, window, cx| match event {
            InputEvent::Change => pane.find_changed(id, window, cx),
            InputEvent::PressEnter { shift, .. } => pane.find_enter(id, !shift, window, cx),
            _ => {}
        });
        let results = cx.new(|cx| EditorState::new(window, cx).language("json").searchable(true));
        results.update(cx, |results, cx| results.set_readonly(true, cx));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        file.find = Some(FindBar {
            mode,
            input,
            slurp: false,
            results,
            outcome: None,
            running: false,
            cancel: Arc::default(),
            _run: None,
            _subscription: subscription,
        });
        self.find_changed(id, window, cx);
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get_mut(self.active) else { return };
        let Some(mut bar) = file.find.take() else { return };
        bar.stop();
        if let Body::Code(editor) = &file.body {
            editor.update(cx, |editor, cx| editor.close_search(cx));
        }
        file.body.focus(window, cx);
        cx.notify();
    }

    fn set_find_mode(&mut self, mode: FindMode, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(Some(mode), window, cx);
    }

    fn find_changed(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.file_by_id(id) else { return };
        let Some(bar) = file.find.as_mut() else { return };
        let query = bar.query(cx);
        match bar.mode {
            FindMode::Text => {
                if let Body::Code(editor) = &file.body {
                    editor.update(cx, |editor, cx| {
                        if query.is_empty() {
                            editor.close_search(cx);
                        } else {
                            editor.set_search_query(query, true, cx);
                            editor.next_search_match(cx);
                        }
                    });
                }
            }
            FindMode::Jq => {
                bar.stop();
                bar._run = Some(cx.spawn_in(window, async move |pane, cx| {
                    cx.background_executor().timer(JQ_DELAY).await;
                    let _ = pane.update_in(cx, |pane, window, cx| pane.run_jq(id, window, cx));
                }));
            }
        }
        cx.notify();
    }

    fn find_enter(&mut self, id: EntityId, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.file_by_id(id) else { return };
        let Some(bar) = file.find.as_ref() else { return };
        match bar.mode {
            FindMode::Text => self.find_step(forward, cx),
            FindMode::Jq => self.run_jq(id, window, cx),
        }
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(self.active) else { return };
        if let Body::Code(editor) = &file.body {
            editor.update(cx, |editor, cx| {
                if forward {
                    editor.next_search_match(cx);
                } else {
                    editor.previous_search_match(cx);
                }
            });
        }
        cx.notify();
    }

    /// Runs the filter off the UI thread and shows what it gives.
    fn run_jq(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.file_by_id(id) else { return };
        let text = file.body.text(cx);
        let Some(bar) = file.find.as_mut() else { return };
        bar.stop();
        let filter = bar.query(cx);
        if filter.trim().is_empty() {
            bar.outcome = None;
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        bar.cancel = cancel.clone();
        bar.running = true;
        let slurp = bar.slurp;
        bar._run = Some(cx.spawn_in(window, async move |pane, cx| {
            let result = cx
                .background_spawn(async move { jq::run(&filter, &text.to_string(), slurp, &cancel) })
                .await;
            let _ = pane.update_in(cx, |pane, window, cx| pane.jq_finished(id, result, window, cx));
        }));
        cx.notify();
    }

    fn jq_finished(&mut self, id: EntityId, result: Result<jq::Outcome, String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.file_by_id(id) else { return };
        let Some(bar) = file.find.as_mut() else { return };
        bar.running = false;
        bar._run = None;
        bar.outcome = Some(result.map(|out| {
            bar.results.update(cx, |results, cx| results.set_value(out.text, window, cx));
            Summary {
                count: out.count,
                cut: out.cut,
                error: out.error,
                errors: out.errors,
            }
        }));
        cx.notify();
    }

    fn toggle_slurp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = self.files.get_mut(self.active) else { return };
        let id = file.body.id();
        let Some(bar) = file.find.as_mut() else { return };
        bar.slurp = !bar.slurp;
        self.run_jq(id, window, cx);
    }

    pub(super) fn render_find(&self, file: &OpenFile, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let bar = file.find.as_ref()?;
        let theme = cx.theme();
        let big = matches!(file.body, Body::Big(_));
        let status: (SharedString, gpui_kit::Hsla) = match bar.mode {
            FindMode::Text => match &file.body {
                Body::Code(editor) if !bar.query(cx).is_empty() => {
                    let matcher = &editor.read(cx).search_session().matcher;
                    let label = if matcher.is_empty() { "No matches".to_owned() } else { matcher.label() };
                    (label.into(), theme.muted_foreground)
                }
                _ => (SharedString::default(), theme.muted_foreground),
            },
            FindMode::Jq => match (&bar.outcome, bar.running) {
                (_, true) => ("Running…".into(), theme.muted_foreground),
                (None, false) => (SharedString::default(), theme.muted_foreground),
                (Some(Err(_)), false) => ("The filter is not valid".into(), theme.danger),
                (Some(Ok(summary)), false) => {
                    let mut text = match summary.count {
                        1 => "1 result".to_owned(),
                        count => format!("{count} results"),
                    };
                    if summary.cut {
                        text.push_str(", more left out");
                    }
                    (text.into(), theme.muted_foreground)
                }
            },
        };
        let tab = |id: &'static str, label: &'static str, mode: FindMode| {
            Button::new(id)
                .ghost()
                .xsmall()
                .label(label)
                .selected(bar.mode == mode)
                .on_click(cx.listener(move |pane, _, window, cx| pane.set_find_mode(mode, window, cx)))
        };
        Some(
            h_flex()
                .h(px(36.))
                .flex_none()
                .px_2()
                .gap_1()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .capture_action(cx.listener(|pane, _: &Escape, window, cx| pane.close_find(window, cx)))
                .child(tab("find-text", "Text", FindMode::Text))
                .child(tab("find-jq", "jq", FindMode::Jq))
                .child(div().w(px(if big { 520. } else { 360. })).min_w(px(160.)).child(Input::new(&bar.input).small()))
                .child(div().px_1().text_xs().text_color(status.1).child(status.0))
                .when(bar.mode == FindMode::Text, |row| {
                    row.child(
                        Button::new("find-prev")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronUp)
                            .tooltip("Previous (⇧⏎)")
                            .on_click(cx.listener(|pane, _, _, cx| pane.find_step(false, cx))),
                    )
                    .child(
                        Button::new("find-next")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronDown)
                            .tooltip("Next (⏎)")
                            .on_click(cx.listener(|pane, _, _, cx| pane.find_step(true, cx))),
                    )
                })
                .when(bar.mode == FindMode::Jq, |row| {
                    row.child(
                        Button::new("find-slurp")
                            .ghost()
                            .xsmall()
                            .label("All as one array")
                            .selected(bar.slurp)
                            .tooltip("Give the filter every value at once, in one array, as jq --slurp does")
                            .on_click(cx.listener(|pane, _, window, cx| pane.toggle_slurp(window, cx))),
                    )
                })
                .child(div().flex_1())
                .child(
                    Button::new("find-close")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .on_click(cx.listener(|pane, _, window, cx| pane.close_find(window, cx))),
                ),
        )
    }

    /// The jq results, under the file.
    pub(super) fn render_jq_results(&self, file: &OpenFile, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let bar = file.find.as_ref().filter(|bar| bar.mode == FindMode::Jq)?;
        let outcome = bar.outcome.as_ref()?;
        let theme = cx.theme();
        let settings = Settings::get(cx);
        let mono = settings.mono_font();
        let size = px(settings.font_size.clamp(6., 72.));
        let body = match outcome {
            Err(error) => div()
                .p_3()
                .text_xs()
                .font_family(mono)
                .text_color(theme.danger)
                .child(SharedString::from(error.clone()))
                .into_any_element(),
            Ok(_) => Editor::new(&bar.results)
                .appearance(false)
                .bordered(false)
                .size_full()
                .font_family(mono)
                .text_size(size)
                .into_any_element(),
        };
        let note = match outcome {
            Ok(Summary { error: Some(error), errors, .. }) => Some(match errors {
                1 => format!("1 input failed: {error}"),
                errors => format!("{errors} inputs failed. The first: {error}"),
            }),
            _ => None,
        };
        let results = bar.results.clone();
        Some(
            v_flex()
                .h(relative(0.45))
                .flex_none()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .h(px(30.))
                        .flex_none()
                        .px_3()
                        .gap_2()
                        .items_center()
                        .bg(theme.foreground.opacity(0.03))
                        .border_b_1()
                        .border_color(theme.border)
                        .text_xs()
                        .child(Icon::new(Lucide::Braces).size(px(12.)).text_color(theme.muted_foreground))
                        .child(div().font_semibold().child("jq results"))
                        .when_some(note, |row, note| {
                            row.child(div().min_w_0().truncate().text_color(theme.warning).child(note))
                        })
                        .child(div().flex_1())
                        .when(outcome.is_ok(), |row| {
                            row.child(
                                Button::new("jq-copy")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(Lucide::Copy))
                                    .label("Copy")
                                    .on_click(move |_, _, cx| {
                                        copy_text(results.read(cx).text().to_string(), cx);
                                    }),
                            )
                        }),
                )
                .child(div().flex_1().min_h_0().child(body)),
        )
    }
}
