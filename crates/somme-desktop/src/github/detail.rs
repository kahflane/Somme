//! One pull request or issue, laid out like its page on GitHub: the
//! conversation, the merge box and the comment box, and for a pull request
//! its commits, checks and changed files.

use std::future::Future;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffFile, DiffMode, DiffState};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, HighlightStyle, Hsla,
    InteractiveElement as _, StyledText,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Pixels, Subscription, Task, Window, canvas, div, prelude::FluentBuilder as _, px,
};

use super::api::{
    ApiError, Check, Commit, Detail, Event, GitHub, Kind, Label, LineComment, MergeMethod, RepoInfo, ReviewEvent,
    User, short_sha,
};
use super::mentions::{self, Mentions};
use super::panel::error_line;
use super::repo::Repo;
use super::widgets::{self, Tone};
use crate::runtime;
use crate::store::Store;
use crate::ui::common::Lucide;

/// How often to ask again while GitHub works out whether a pull request can merge.
const MERGEABLE_RETRIES: u32 = 4;

pub enum DetailEvent {
    Back,
    /// Something changed on GitHub, so the list is out of date.
    Changed,
    Failed(ApiError),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Conversation,
    Commits,
    Checks,
    Files,
}

/// Loaded on first view.
enum Lazy<T> {
    NotYet,
    Loading,
    Ready(T),
    Failed(String),
}

pub struct DetailView {
    client: GitHub,
    repo: Repo,
    kind: Kind,
    number: u64,
    detail: Option<Detail>,
    info: RepoInfo,
    events: Vec<Event>,
    checks: Vec<Check>,
    error: Option<String>,
    section: Section,
    commits: Lazy<Vec<Commit>>,
    diff: Lazy<Entity<DiffState>>,
    /// How wide the diff was last drawn. See [`DetailView::diff_width`].
    diff_width: Option<Pixels>,
    _diff_heights: Task<()>,
    comment: Entity<TextareaState>,
    mentions: Entity<Mentions>,
    title: Option<Entity<InputState>>,
    /// The repository's labels, for the label picker.
    repo_labels: Vec<Label>,
    /// An action is on its way to GitHub.
    busy: bool,
    merge_method: MergeMethod,
    confirming_merge: bool,
    generation: u64,
    mergeable_retries: u32,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DetailEvent> for DetailView {}

impl DetailView {
    pub fn new(client: GitHub, repo: Repo, kind: Kind, number: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let comment = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(3, 14).placeholder("Add a comment. Markdown works here.")
        });
        let subscriptions = vec![cx.observe(&comment, |_, _, cx| cx.notify())];
        let mentions = cx.new(|cx| Mentions::new(comment.clone(), client.clone(), repo.clone(), cx));
        let mut view = Self {
            client,
            repo,
            kind,
            number,
            detail: None,
            info: RepoInfo::default(),
            events: Vec::new(),
            checks: Vec::new(),
            error: None,
            section: Section::Conversation,
            commits: Lazy::NotYet,
            diff: Lazy::NotYet,
            diff_width: None,
            _diff_heights: Task::ready(()),
            comment,
            mentions,
            title: None,
            repo_labels: Vec::new(),
            busy: false,
            merge_method: MergeMethod::Merge,
            confirming_merge: false,
            generation: 0,
            mergeable_retries: 0,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        view.load(cx);
        view
    }

    fn pull(&self) -> bool {
        self.kind == Kind::Pulls
    }

    /// Reads the page again: the item, the repository's rules and the conversation.
    pub(super) fn load(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let (client, repo, kind, number, pull) = (self.client.clone(), self.repo.clone(), self.kind, self.number, self.pull());
        let first = self.detail.is_none();
        cx.spawn(async move |this, cx| {
            let loaded = runtime::io(async move {
                let (detail, info, events) = futures::join!(
                    client.detail(&repo, kind, number),
                    client.repo_info(&repo),
                    client.timeline(&repo, number, pull),
                );
                let detail = detail?;
                let info = info?;
                let checks = match detail.head.as_ref() {
                    Some(head) => client.checks(&repo, &head.sha).await.unwrap_or_default(),
                    None => Vec::new(),
                };
                let labels = if first && info.can_push { client.labels(&repo).await.unwrap_or_default() } else { Vec::new() };
                Ok::<_, ApiError>((detail, info, events?, checks, labels))
            })
            .await;
            let _ = this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                match loaded {
                    Ok((detail, info, events, checks, labels)) => {
                        view.merge_method = [
                            (info.merge_commit, MergeMethod::Merge),
                            (info.squash, MergeMethod::Squash),
                            (info.rebase, MergeMethod::Rebase),
                        ]
                        .into_iter()
                        .find_map(|(allowed, method)| allowed.then_some(method))
                        .unwrap_or(MergeMethod::Merge);
                        let unknown = detail.is_pull() && detail.state == "open" && detail.mergeable.is_none();
                        let taking_part = std::iter::once(detail.user.clone()).chain(events.iter().filter_map(|event| match event {
                            Event::Comment { author, .. } | Event::Review { author, .. } => Some(author.clone()),
                            Event::Note { actor, .. } => Some(actor.clone()),
                            _ => None,
                        }));
                        view.mentions.update(cx, |mentions, _| mentions.set_taking_part(taking_part));
                        view.detail = Some(detail);
                        view.info = info;
                        view.events = events;
                        view.checks = checks;
                        if !labels.is_empty() {
                            view.repo_labels = labels;
                        }
                        if unknown && view.mergeable_retries < MERGEABLE_RETRIES {
                            view.mergeable_retries += 1;
                            view.load_later(cx);
                        }
                    }
                    Err(err) => view.fail(err, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// GitHub works out whether a pull request merges cleanly in the background.
    fn load_later(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |view, cx| view.load(cx));
        })
        .detach();
    }

    fn fail(&mut self, err: ApiError, cx: &mut Context<Self>) {
        if matches!(err, ApiError::Unauthorized) {
            cx.emit(DetailEvent::Failed(err));
            return;
        }
        self.error = Some(err.to_string());
        cx.notify();
    }

    /// Sends a change to GitHub, then reads the page again. `clear` empties
    /// the comment box once it went through.
    fn act<F, Fut>(&mut self, clear: bool, call: F, window: &mut Window, cx: &mut Context<Self>)
    where
        F: FnOnce(GitHub, Repo) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ApiError>> + Send + 'static,
    {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let (client, repo) = (self.client.clone(), self.repo.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = runtime::io(async move { call(client, repo).await }).await;
            let _ = this.update_in(cx, |view, window, cx| {
                view.busy = false;
                match result {
                    Ok(()) => {
                        view.confirming_merge = false;
                        view.title = None;
                        if clear {
                            view.comment.update(cx, |comment, cx| comment.set_value("", window, cx));
                        }
                        view.load(cx);
                        cx.emit(DetailEvent::Changed);
                    }
                    Err(err) => view.fail(err, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn comment_text(&self, cx: &App) -> String {
        self.comment.read(cx).value().trim().to_owned()
    }

    fn send_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.comment_text(cx);
        if body.is_empty() {
            return;
        }
        let number = self.number;
        self.act(true, move |client, repo| async move { client.comment(&repo, number, &body).await }, window, cx);
    }

    /// Closes or reopens, with the comment typed so far.
    fn set_open(&mut self, open: bool, not_planned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.comment_text(cx);
        let number = self.number;
        self.act(
            true,
            move |client, repo| async move {
                if !body.is_empty() {
                    client.comment(&repo, number, &body).await?;
                }
                client.set_open(&repo, number, open, not_planned).await
            },
            window,
            cx,
        );
    }

    fn review(&mut self, event: ReviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.comment_text(cx);
        if event != ReviewEvent::Approve && body.is_empty() {
            self.error = Some("Write what should change in the comment box first.".to_owned());
            cx.notify();
            return;
        }
        let number = self.number;
        self.act(true, move |client, repo| async move { client.review(&repo, number, event, &body).await }, window, cx);
    }

    fn merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(sha) = self.detail.as_ref().and_then(|d| d.head.as_ref()).map(|head| head.sha.clone()) else { return };
        let (number, method) = (self.number, self.merge_method);
        self.act(false, move |client, repo| async move { client.merge(&repo, number, method, &sha).await }, window, cx);
    }

    fn ready_for_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.detail.as_ref().map(|d| d.node_id.clone()) else { return };
        self.act(false, move |client, _| async move { client.ready_for_review(&id).await }, window, cx);
    }

    fn delete_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.detail.as_ref().and_then(|d| d.head.as_ref()).map(|head| head.name.clone()) else {
            return;
        };
        self.act(false, move |client, repo| async move { client.delete_branch(&repo, &branch).await }, window, cx);
    }

    fn toggle_label(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let mut names: Vec<String> = detail.labels.iter().map(|label| label.name.clone()).collect();
        match names.iter().position(|n| *n == name) {
            Some(ix) => {
                names.remove(ix);
            }
            None => names.push(name),
        }
        let number = self.number;
        self.act(false, move |client, repo| async move { client.set_labels(&repo, number, &names).await }, window, cx);
    }

    fn edit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let title = detail.title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| input.focus(window, cx));
        self._subscriptions.push(cx.subscribe_in(&input, window, |view, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                view.save_title(window, cx);
            }
        }));
        self.title = Some(input);
        cx.notify();
    }

    fn save_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.title else { return };
        let title = input.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let number = self.number;
        self.act(false, move |client, repo| async move { client.rename(&repo, number, &title).await }, window, cx);
    }

    fn show_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        match section {
            Section::Commits if matches!(self.commits, Lazy::NotYet) => self.load_commits(cx),
            Section::Files if matches!(self.diff, Lazy::NotYet) => self.load_diff(cx),
            _ => {}
        }
        cx.notify();
    }

    fn load_commits(&mut self, cx: &mut Context<Self>) {
        self.commits = Lazy::Loading;
        let (client, repo, number) = (self.client.clone(), self.repo.clone(), self.number);
        cx.spawn(async move |this, cx| {
            let commits = runtime::io(async move { client.commits(&repo, number).await }).await;
            let _ = this.update(cx, |view, cx| {
                view.commits = match commits {
                    Ok(commits) => Lazy::Ready(commits),
                    Err(err) => Lazy::Failed(err.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn load_diff(&mut self, cx: &mut Context<Self>) {
        self.diff = Lazy::Loading;
        self.diff_width = None;
        let (client, repo, number) = (self.client.clone(), self.repo.clone(), self.number);
        cx.spawn(async move |this, cx| {
            let files = runtime::io(async move {
                let patch = client.diff(&repo, number).await.map_err(|err| err.to_string())?;
                DiffFile::parse(&patch).map_err(|err| format!("Can't read the diff: {err}"))
            })
            .await;
            let _ = this.update(cx, |view, cx| {
                view.diff = match files {
                    Ok(files) => Lazy::Ready(cx.new(|cx| DiffState::new(files, cx).with_mode(DiffMode::Unified))),
                    Err(err) => Lazy::Failed(err),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// The diff's list forgets how tall its rows are when it is first drawn
    /// and whenever its width changes. Rows not drawn yet then count as no
    /// height, so it stops scrolling early. Once the width settles, each row
    /// is given a code line's height again.
    fn diff_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if self.diff_width == Some(width) {
            return;
        }
        self.diff_width = Some(width);
        self._diff_heights = cx.spawn(async move |view, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let _ = view.update(cx, |view, cx| {
                if let Lazy::Ready(state) = &view.diff {
                    // Changing a setting and back is the one way to ask for it.
                    state.update(cx, |state, cx| {
                        let lines = state.min_collapsed_lines();
                        state.set_min_collapsed_lines(lines + 1, cx);
                        state.set_min_collapsed_lines(lines, cx);
                    });
                }
            });
        });
    }

    fn toggle_split(&mut self, cx: &mut Context<Self>) {
        if let Lazy::Ready(state) = &self.diff {
            state.update(cx, |state, cx| {
                let mode = if state.mode() == DiffMode::Split { DiffMode::Unified } else { DiffMode::Split };
                state.set_mode(mode, cx);
            });
        }
    }

    // ---- rendering ----

    fn me(cx: &App) -> Option<String> {
        Store::global(cx).read(cx).account.login.clone()
    }

    /// Whether the signed-in user may close, reopen and retitle it.
    fn can_manage(&self, detail: &Detail, cx: &App) -> bool {
        self.info.can_push || Self::me(cx).is_some_and(|me| me.eq_ignore_ascii_case(&detail.user.login))
    }

    fn render_header(&self, detail: &Detail, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (icon, word, tone) = widgets::detail_state(detail);
        let title: AnyElement = match &self.title {
            Some(input) => h_flex()
                .flex_1()
                .gap_1()
                .child(div().flex_1().child(Input::new(input).small()))
                .child(
                    Button::new("gh-title-save")
                        .small()
                        .label("Save")
                        .disabled(self.busy)
                        .on_click(cx.listener(|view, _, window, cx| view.save_title(window, cx))),
                )
                .child(
                    Button::new("gh-title-cancel")
                        .small()
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.title = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
            None => h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .items_start()
                .child({
                    // "Title #7", the number in grey on the same line, as GitHub shows it.
                    let text = format!("{} #{}", detail.title, detail.number);
                    let number = detail.title.len() + 1..text.len();
                    let grey = HighlightStyle { color: Some(theme.muted_foreground), ..Default::default() };
                    div().flex_1().min_w_0().text_lg().child(StyledText::new(text).with_highlights([(number, grey)]))
                })
                .when(self.can_manage(detail, cx), |row| {
                    row.child(
                        Button::new("gh-title-edit")
                            .xsmall()
                            .ghost()
                            .label("Edit")
                            .on_click(cx.listener(|view, _, window, cx| view.edit_title(window, cx))),
                    )
                })
                .into_any_element(),
        };
        let author = detail.user.login.clone();
        let summary = match (&detail.head, &detail.base) {
            (Some(head), Some(base)) => {
                let commits = plural(detail.commits, "commit");
                if detail.merged {
                    let by = detail.merged_by.as_ref().map_or(author.clone(), |user| user.login.clone());
                    let at = detail.merged_at.as_deref().map(widgets::when).unwrap_or_default();
                    format!("{by} merged {commits} into {} from {} {at}", base.name, head.label)
                } else {
                    format!("{author} wants to merge {commits} into {} from {}", base.name, head.label)
                }
            }
            _ => format!(
                "{author} opened this issue {} · {}",
                widgets::when(&detail.created_at),
                plural(detail.comments, "comment")
            ),
        };
        v_flex()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(theme.border)
            .child(title)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(widgets::state_badge(icon, word, tone, cx))
                    .child(div().flex_1().min_w_0().text_sm().text_color(theme.muted_foreground).child(summary)),
            )
    }

    /// Reviewers, assignees, labels and milestone, the way GitHub's sidebar lists them.
    fn render_facts(&self, detail: &Detail, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let people = |users: &[User], none: &'static str, cx: &mut Context<Self>| -> AnyElement {
            if users.is_empty() {
                return div().text_color(theme.muted_foreground).child(none).into_any_element();
            }
            h_flex()
                .flex_wrap()
                .gap_2()
                .children(users.iter().map(|user| {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(widgets::avatar(user.avatar_url.as_deref(), px(16.), cx))
                        .child(SharedString::from(user.login.clone()))
                }))
                .into_any_element()
        };
        let fact = |name: &'static str, value: AnyElement| {
            h_flex()
                .gap_3()
                .items_start()
                .child(div().w(px(80.)).flex_none().text_color(theme.muted_foreground).child(name))
                .child(div().flex_1().min_w_0().child(value))
        };
        let labels: AnyElement = if detail.labels.is_empty() {
            div().text_color(theme.muted_foreground).child("None yet").into_any_element()
        } else {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(detail.labels.iter().map(|label| widgets::label_chip(label, cx)))
                .into_any_element()
        };
        let label_picker = (self.info.can_push && !self.repo_labels.is_empty()).then(|| {
            let view = cx.entity().downgrade();
            let on: Vec<String> = detail.labels.iter().map(|label| label.name.clone()).collect();
            let all = self.repo_labels.clone();
            Button::new("gh-labels")
                .xsmall()
                .ghost()
                .icon(Icon::new(Lucide::Settings))
                .tooltip("Change labels")
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.max_h(px(320.)).scrollable(true);
                    for label in &all {
                        let (view, name) = (view.clone(), label.name.clone());
                        menu = menu.item(PopupMenuItem::new(label.name.clone()).checked(on.contains(&label.name)).on_click(
                            move |_, window, cx| {
                                let name = name.clone();
                                let _ = view.update(cx, |view, cx| view.toggle_label(name, window, cx));
                            },
                        ));
                    }
                    menu
                })
        });
        let mut facts = v_flex().gap_2().py_3().text_sm().border_b_1().border_color(theme.border);
        if detail.is_pull() {
            facts = facts.child(fact("Reviewers", people(&detail.requested_reviewers, "No reviews asked for", cx)));
        }
        facts = facts
            .child(fact("Assignees", people(&detail.assignees, "No one", cx)))
            .child(fact(
                "Labels",
                h_flex().gap_1().items_start().child(div().flex_1().child(labels)).children(label_picker).into_any_element(),
            ));
        if let Some(milestone) = &detail.milestone {
            facts = facts.child(fact("Milestone", div().child(SharedString::from(milestone.title.clone())).into_any_element()));
        }
        facts
    }

    fn render_conversation(&self, detail: &Detail, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let body = detail.body.clone().filter(|body| !body.trim().is_empty());
        out.push(self.comment_card(
            ("gh-body", detail.number as usize).into(),
            &detail.user,
            &detail.created_at,
            body,
            cx,
        ));
        for (ix, event) in self.events.iter().enumerate() {
            out.push(match event {
                Event::Comment { id, author, body, at } => {
                    self.comment_card(("gh-comment", *id as usize).into(), author, at, Some(body.clone()), cx)
                }
                Event::Review { id, author, state, body, at, comments } => {
                    self.review_entry(*id, author, state, body, at, comments, cx)
                }
                Event::Commit { sha, message, author, at } => commit_line(ix, sha, message, author, at, cx),
                Event::Note { kind, actor, text, at, label } => note_line(kind, actor, text, at, label.as_ref(), cx),
            });
        }
        out
    }

    fn comment_card(
        &self,
        id: ElementId,
        author: &User,
        at: &str,
        body: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let picture = widgets::avatar(author.avatar_url.as_deref(), px(28.), cx);
        let text: AnyElement = match body {
            Some(body) => markdown(id, body).into_any_element(),
            None => div().italic().text_color(theme.muted_foreground).child("No description provided.").into_any_element(),
        };
        h_flex()
            .gap_2()
            .items_start()
            .child(picture)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .px_3()
                            .py_1p5()
                            .gap_1()
                            .text_sm()
                            .bg(theme.muted.opacity(0.5))
                            .border_b_1()
                            .border_color(theme.border)
                            .child(div().font_semibold().child(SharedString::from(author.login.clone())))
                            .child(div().text_color(theme.muted_foreground).child(format!("commented {}", widgets::when(at)))),
                    )
                    .child(div().px_3().py_2().text_sm().child(text)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn review_entry(
        &self,
        id: u64,
        author: &User,
        state: &str,
        body: &str,
        at: &str,
        comments: &[LineComment],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let (icon, tint, what) = match state {
            "APPROVED" => (Lucide::Check, Some(Tone::Open.color(cx)), "approved these changes"),
            "CHANGES_REQUESTED" => (Lucide::FileDiff, Some(Tone::Closed.color(cx)), "requested changes"),
            "DISMISSED" => (Lucide::X, None, "had a review dismissed"),
            _ => (Lucide::Eye, None, "reviewed"),
        };
        let line = h_flex()
            .gap_2()
            .items_center()
            .text_sm()
            .child(widgets::event_icon(icon, tint, cx))
            .child(widgets::avatar(author.avatar_url.as_deref(), px(18.), cx))
            .child(div().font_semibold().child(SharedString::from(author.login.clone())))
            .child(div().text_color(theme.muted_foreground).child(format!("{what} {}", widgets::when(at))));
        let mut entry = v_flex().gap_2().child(line);
        if !body.trim().is_empty() {
            entry = entry.child(
                div()
                    .ml(px(36.))
                    .px_3()
                    .py_2()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .text_sm()
                    .child(markdown(("gh-review", id as usize), body.to_owned())),
            );
        }
        for (ix, comment) in comments.iter().enumerate() {
            let place = match comment.line {
                Some(line) => format!("{}:{line} · {}", comment.path, comment.user.login),
                None => format!("{} · {}", comment.path, comment.user.login),
            };
            entry = entry.child(
                v_flex()
                    .ml(px(36.))
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .bg(theme.muted.opacity(0.5))
                            .border_b_1()
                            .border_color(theme.border)
                            .child(place),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .text_sm()
                            .child(markdown(("gh-line", (id as usize) * 1000 + ix), comment.body.clone())),
                    ),
            );
        }
        entry.into_any_element()
    }

    fn render_merge_box(&self, detail: &Detail, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !detail.is_pull() {
            return None;
        }
        let theme = cx.theme().clone();
        let card = |border: Hsla| v_flex().border_1().border_color(border).rounded_md().overflow_hidden();
        let row = |icon: Lucide, color: Hsla, title: String, detail: Option<String>| {
            h_flex()
                .px_3()
                .py_2()
                .gap_2()
                .items_start()
                .border_b_1()
                .border_color(theme.border)
                .child(div().pt(px(2.)).child(Icon::new(icon).size(px(16.)).text_color(color)))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_semibold().child(title))
                        .children(detail.map(|d| div().text_xs().text_color(theme.muted_foreground).child(d))),
                )
        };
        let (green, red, purple, grey) =
            (Tone::Open.color(cx), Tone::Closed.color(cx), Tone::Merged.color(cx), Tone::Muted.color(cx));

        if detail.merged {
            let branch_gone = self.events.iter().any(|e| matches!(e, Event::Note { kind, .. } if kind == "head_ref_deleted"));
            let own_branch = detail
                .head
                .as_ref()
                .and_then(|head| head.repo.as_ref())
                .is_some_and(|repo| repo.full_name.eq_ignore_ascii_case(&self.repo.full_name()));
            let deletable = self.info.can_push && own_branch && !branch_gone && !self.info.delete_branch_on_merge;
            return Some(
                card(purple)
                    .child(row(
                        Lucide::GitMerge,
                        purple,
                        "Pull request successfully merged and closed".into(),
                        Some(if deletable {
                            "The branch can be safely deleted.".into()
                        } else {
                            "You're all set.".into()
                        }),
                    ))
                    .when(deletable, |card| {
                        card.child(
                            h_flex().px_3().py_2().justify_end().child(
                                Button::new("gh-delete-branch")
                                    .small()
                                    .label("Delete branch")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|view, _, window, cx| view.delete_branch(window, cx))),
                            ),
                        )
                    })
                    .into_any_element(),
            );
        }
        if detail.state != "open" {
            return Some(
                card(theme.border)
                    .child(row(
                        Lucide::GitPullRequestClosed,
                        red,
                        "Closed with unmerged commits".into(),
                        Some("This pull request is closed.".into()),
                    ))
                    .into_any_element(),
            );
        }

        let mut card = card(theme.border);
        // Checks.
        if !self.checks.is_empty() {
            let failing = self.checks.iter().filter(|c| c.result == "failure").count();
            let pending = self.checks.iter().filter(|c| c.result == "pending").count();
            let passed = self.checks.len() - failing - pending;
            card = card.child(if failing > 0 {
                row(
                    Lucide::X,
                    red,
                    "Some checks were not successful".into(),
                    Some(format!("{failing} failing, {passed} successful, {pending} in progress")),
                )
            } else if pending > 0 {
                row(
                    Lucide::Dot,
                    theme.warning,
                    "Some checks haven't completed yet".into(),
                    Some(format!("{pending} in progress, {passed} successful")),
                )
            } else {
                row(Lucide::Check, green, "All checks have passed".into(), Some(format!("{passed} successful checks")))
            });
        }
        // Whether it can merge.
        let state = detail.mergeable_state.as_deref().unwrap_or("unknown");
        card = card.child(if detail.draft {
            row(
                Lucide::GitPullRequestDraft,
                grey,
                "This pull request is still a work in progress".into(),
                Some("Draft pull requests cannot be merged.".into()),
            )
        } else {
            match (detail.mergeable, state) {
                (Some(false), _) | (_, "dirty") => row(
                    Lucide::TriangleAlert,
                    red,
                    "This branch has conflicts that must be resolved".into(),
                    Some("Resolve them on GitHub or on the command line.".into()),
                ),
                (_, "blocked") => row(
                    Lucide::CircleAlert,
                    red,
                    "Merging is blocked".into(),
                    Some("The base branch's rules aren't met yet, like required reviews or checks.".into()),
                ),
                (_, "behind") => row(
                    Lucide::CircleAlert,
                    theme.warning,
                    "This branch is out of date with the base branch".into(),
                    Some("Merge the latest changes from the base branch into it.".into()),
                ),
                (None, _) => row(Lucide::Clock, grey, "Checking whether this branch can be merged…".into(), None),
                _ => row(Lucide::Check, green, "This branch has no conflicts with the base branch".into(), None),
            }
        });
        // Actions.
        let actions = if !self.info.can_push {
            h_flex()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Only people with write access can merge this pull request.")
        } else if detail.draft {
            h_flex().px_3().py_2().justify_end().child(
                Button::new("gh-ready")
                    .small()
                    .label("Ready for review")
                    .disabled(self.busy)
                    .on_click(cx.listener(|view, _, window, cx| view.ready_for_review(window, cx))),
            )
        } else if self.confirming_merge {
            h_flex()
                .px_3()
                .py_2()
                .gap_2()
                .justify_end()
                .child(
                    Button::new("gh-merge-cancel")
                        .small()
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.confirming_merge = false;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("gh-merge-confirm")
                        .small()
                        .success()
                        .label(match self.merge_method {
                            MergeMethod::Merge => "Confirm merge",
                            MergeMethod::Squash => "Confirm squash and merge",
                            MergeMethod::Rebase => "Confirm rebase and merge",
                        })
                        .loading(self.busy)
                        .disabled(self.busy)
                        .on_click(cx.listener(|view, _, window, cx| view.merge(window, cx))),
                )
        } else {
            let methods: Vec<MergeMethod> = [
                (self.info.merge_commit, MergeMethod::Merge),
                (self.info.squash, MergeMethod::Squash),
                (self.info.rebase, MergeMethod::Rebase),
            ]
            .into_iter()
            .filter_map(|(allowed, method)| allowed.then_some(method))
            .collect();
            let view = cx.entity().downgrade();
            let current = self.merge_method;
            let clean = detail.mergeable == Some(true) && state != "blocked";
            h_flex()
                .px_3()
                .py_2()
                .gap_1()
                .justify_end()
                .child(
                    Button::new("gh-merge")
                        .small()
                        .when(clean, |button| button.success())
                        .label(current.label().replace("Create a merge commit", "Merge pull request"))
                        .disabled(self.busy || detail.mergeable != Some(true))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.confirming_merge = true;
                            cx.notify();
                        })),
                )
                .when(methods.len() > 1, |row| {
                    row.child(
                        Button::new("gh-merge-method")
                            .small()
                            .when(clean, |button| button.success())
                            .icon(IconName::ChevronDown)
                            .dropdown_menu(move |mut menu, _, _| {
                                for method in &methods {
                                    let (view, method) = (view.clone(), *method);
                                    menu = menu.item(
                                        PopupMenuItem::new(method.label()).checked(method == current).on_click(
                                            move |_, _, cx| {
                                                let _ = view.update(cx, |view, cx| {
                                                    view.merge_method = method;
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    );
                                }
                                menu
                            }),
                    )
                })
        };
        Some(card.child(actions).into_any_element())
    }

    fn render_comment_box(&self, detail: &Detail, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if detail.locked && !self.info.can_push {
            return div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("This conversation has been locked.")
                .into_any_element();
        }
        let typed = !self.comment_text(cx).is_empty();
        let open = detail.state == "open";
        let manage = self.can_manage(detail, cx);
        let pull = detail.is_pull();
        let mine = Self::me(cx).is_some_and(|me| me.eq_ignore_ascii_case(&detail.user.login));
        let mut buttons = h_flex().gap_2().justify_end().flex_wrap();

        if pull && open && !mine {
            let view = cx.entity().downgrade();
            buttons = buttons.child(
                Button::new("gh-review")
                    .small()
                    .label("Review")
                    .icon(IconName::ChevronDown)
                    .disabled(self.busy)
                    .dropdown_menu(move |menu, _, _| {
                        let (approve, changes, comment) = (view.clone(), view.clone(), view.clone());
                        menu.item(PopupMenuItem::new("Approve").icon(Icon::new(Lucide::Check)).on_click(
                            move |_, window, cx| {
                                let _ = approve.update(cx, |view, cx| view.review(ReviewEvent::Approve, window, cx));
                            },
                        ))
                        .item(PopupMenuItem::new("Request changes").icon(Icon::new(Lucide::FileDiff)).on_click(
                            move |_, window, cx| {
                                let _ = changes
                                    .update(cx, |view, cx| view.review(ReviewEvent::RequestChanges, window, cx));
                            },
                        ))
                        .item(PopupMenuItem::new("Comment as a review").icon(Icon::new(Lucide::MessageSquare)).on_click(
                            move |_, window, cx| {
                                let _ = comment.update(cx, |view, cx| view.review(ReviewEvent::Comment, window, cx));
                            },
                        ))
                    }),
            );
        }
        if manage && !detail.merged {
            if open && !pull {
                let view = cx.entity().downgrade();
                buttons = buttons.child(
                    Button::new("gh-close-issue")
                        .small()
                        .icon(Icon::new(Lucide::CircleCheck))
                        .label(if typed { "Close with comment" } else { "Close issue" })
                        .disabled(self.busy)
                        .dropdown_menu(move |menu, _, _| {
                            let (done, not_planned) = (view.clone(), view.clone());
                            menu.item(PopupMenuItem::new("Close as completed").icon(Icon::new(Lucide::CircleCheck)).on_click(
                                move |_, window, cx| {
                                    let _ = done.update(cx, |view, cx| view.set_open(false, false, window, cx));
                                },
                            ))
                            .item(PopupMenuItem::new("Close as not planned").icon(Icon::new(Lucide::CircleSlash)).on_click(
                                move |_, window, cx| {
                                    let _ = not_planned.update(cx, |view, cx| view.set_open(false, true, window, cx));
                                },
                            ))
                        }),
                );
            } else {
                let label = match (open, pull, typed) {
                    (true, _, true) => "Close with comment",
                    (true, _, false) => "Close pull request",
                    (false, true, _) => "Reopen pull request",
                    (false, false, _) => "Reopen issue",
                };
                buttons = buttons.child(
                    Button::new("gh-open-close")
                        .small()
                        .label(label)
                        .disabled(self.busy)
                        .on_click(cx.listener(move |view, _, window, cx| view.set_open(!open, false, window, cx))),
                );
            }
        }
        buttons = buttons.child(
            Button::new("gh-comment")
                .small()
                .success()
                .label("Comment")
                .loading(self.busy)
                .disabled(self.busy || !typed)
                .on_click(cx.listener(|view, _, window, cx| view.send_comment(window, cx))),
        );
        let me = Store::global(cx).read(cx).avatar.clone();
        h_flex()
            .gap_2()
            .items_start()
            .child(crate::ui::common::avatar(me, px(28.), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(mentions::field(
                        div().font_family(theme.mono_font_family.clone()).child(Textarea::new(&self.comment)),
                        &self.mentions,
                    ))
                    .child(buttons),
            )
            .into_any_element()
    }

    fn render_commits(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let commits = match &self.commits {
            Lazy::NotYet | Lazy::Loading => return loading(cx),
            Lazy::Failed(err) => return error_line(err.clone(), cx).into_any_element(),
            Lazy::Ready(commits) => commits,
        };
        let base = self.repo.url();
        v_flex()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .overflow_hidden()
            .children(commits.iter().enumerate().map(|(ix, commit)| {
                let url = format!("{base}/commit/{}", commit.sha);
                h_flex()
                    .id(("gh-commit", ix))
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(|row| row.bg(theme.muted.opacity(0.5)))
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .child(widgets::avatar(commit.avatar.as_deref(), px(20.), cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .truncate()
                                    .child(SharedString::from(first_line(&commit.message).to_owned())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{} committed {}", commit.author, widgets::when(&commit.at))),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(short_sha(&commit.sha).to_owned())),
                    )
            }))
            .into_any_element()
    }

    fn render_checks(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.checks.is_empty() {
            return div().text_sm().text_color(theme.muted_foreground).child("No checks ran on this pull request.").into_any_element();
        }
        v_flex()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .overflow_hidden()
            .children(self.checks.iter().enumerate().map(|(ix, check)| {
                let (icon, color) = match check.result.as_str() {
                    "success" => (Lucide::Check, Tone::Open.color(cx)),
                    "failure" => (Lucide::X, Tone::Closed.color(cx)),
                    "pending" => (Lucide::Dot, theme.warning),
                    _ => (Lucide::CircleSlash, Tone::Muted.color(cx)),
                };
                let url = check.url.clone().filter(|url| url.starts_with("https://"));
                h_flex()
                    .id(("gh-check", ix))
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(url.is_some(), |row| row.cursor_pointer().hover(|row| row.bg(theme.muted.opacity(0.5))))
                    .when_some(url, |row, url| row.on_click(move |_, _, cx| cx.open_url(&url)))
                    .child(Icon::new(icon).size(px(16.)).text_color(color))
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(SharedString::from(check.name.clone())))
                    .children(check.detail.clone().map(|detail| {
                        div().max_w(px(220.)).truncate().text_xs().text_color(theme.muted_foreground).child(detail)
                    }))
            }))
            .into_any_element()
    }

    fn render_files(&self, detail: &Detail, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let state = match &self.diff {
            Lazy::NotYet | Lazy::Loading => return div().p_3().child(loading(cx)).into_any_element(),
            Lazy::Failed(err) => return div().p_3().child(error_line(err.clone(), cx)).into_any_element(),
            Lazy::Ready(state) => state,
        };
        let split = state.read(cx).mode() == DiffMode::Split;
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_color(Tone::Open.color(cx)).child(format!("+{}", detail.additions)))
                    .child(div().text_color(Tone::Closed.color(cx)).child(format!("−{}", detail.deletions)))
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.muted_foreground)
                            .child(format!("in {}", plural(detail.changed_files, "file"))),
                    )
                    .child(
                        Button::new("gh-diff-mode")
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(if split { Lucide::Rows2 } else { Lucide::Columns2 }))
                            .label(if split { "Unified" } else { "Split" })
                            .on_click(cx.listener(|view, _, _, cx| view.toggle_split(cx))),
                    ),
            )
            .child({
                let view = cx.entity().downgrade();
                div().flex_1().min_h_0().relative().child(Diff::new(state)).child(
                    canvas(
                        move |bounds, _, cx| {
                            let _ = view.update(cx, |view, cx| view.diff_width(bounds.size.width, cx));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .into_any_element()
    }
}

impl Render for DetailView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let back = h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_1()
            .items_center()
            .child(
                Button::new("gh-back")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ArrowLeft))
                    .label(if self.pull() { "Pull requests" } else { "Issues" })
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Back))),
            )
            .child(div().flex_1())
            .child({
                let url = format!(
                    "{}/{}/{}",
                    self.repo.url(),
                    if self.pull() { "pull" } else { "issues" },
                    self.number
                );
                Button::new("gh-detail-web")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ExternalLink))
                    .tooltip("Open on GitHub")
                    .on_click(move |_, _, cx| cx.open_url(&url))
            });
        let Some(detail) = self.detail.clone() else {
            return v_flex()
                .size_full()
                .child(back)
                .child(match self.error.clone() {
                    Some(err) => div().p_3().child(error_line(err, cx)).into_any_element(),
                    None => div().p_3().child(loading(cx)).into_any_element(),
                });
        };

        let header = div().px_3().pt_1().child(self.render_header(&detail, cx));
        let tabs = detail.is_pull().then(|| {
            let view = cx.entity().downgrade();
            let selected = match self.section {
                Section::Conversation => 0,
                Section::Commits => 1,
                Section::Checks => 2,
                Section::Files => 3,
            };
            div().px_3().pt_2().child(
                TabBar::new("gh-sections")
                    .underline()
                    .xsmall()
                    .selected_index(selected)
                    .child(Tab::new().label(format!("Conversation {}", detail.comments)))
                    .child(Tab::new().label(format!("Commits {}", detail.commits)))
                    .child(Tab::new().label(format!("Checks {}", self.checks.len())))
                    .child(Tab::new().label(format!("Files changed {}", detail.changed_files)))
                    .on_click(move |ix, window, cx| {
                        // The underline slides on the next frame.
                        window.refresh();
                        let section = match ix {
                            1 => Section::Commits,
                            2 => Section::Checks,
                            3 => Section::Files,
                            _ => Section::Conversation,
                        };
                        let _ = view.update(cx, |view, cx| view.show_section(section, cx));
                    }),
            )
        });
        let error = self.error.clone().map(|err| div().px_3().pt_2().child(error_line(err, cx)));

        let body: AnyElement = match self.section {
            Section::Files => self.render_files(&detail, cx),
            section => {
                let mut content = v_flex().px_3().py_3().gap_4();
                match section {
                    Section::Commits => content = content.child(self.render_commits(cx)),
                    Section::Checks => content = content.child(self.render_checks(cx)),
                    _ => {
                        content = content
                            .child(self.render_facts(&detail, cx))
                            .children(self.render_conversation(&detail, cx))
                            .children(self.render_merge_box(&detail, cx))
                            .child(div().h(px(1.)).bg(theme.border))
                            .child(self.render_comment_box(&detail, cx));
                    }
                }
                div()
                    .id("gh-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(content)
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .child(back)
            .child(header)
            .children(tabs)
            .children(error)
            .child(body)
    }
}

/// GitHub-flavoured text. Links open in the browser, and only web links do.
fn markdown(id: impl Into<ElementId>, text: String) -> TextView {
    TextView::markdown(id, text).selectable(true).on_link_click(|url, _, _, cx| {
        if url.starts_with("https://") || url.starts_with("http://") {
            cx.open_url(url);
        }
    })
}

fn commit_line(ix: usize, sha: &str, message: &str, author: &str, at: &str, cx: &mut Context<DetailView>) -> AnyElement {
    let theme = cx.theme().clone();
    h_flex()
        .id(("gh-tl-commit", ix))
        .gap_2()
        .items_center()
        .text_sm()
        .child(widgets::event_icon(Lucide::GitCommitVertical, None, cx))
        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(first_line(message).to_owned())))
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{author} · {}", widgets::when(at))),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(SharedString::from(short_sha(sha).to_owned())),
        )
        .into_any_element()
}

fn note_line(kind: &str, actor: &User, text: &str, at: &str, label: Option<&Label>, cx: &mut Context<DetailView>) -> AnyElement {
    let theme = cx.theme().clone();
    let (icon, tint) = match kind {
        "labeled" | "unlabeled" => (Lucide::Tag, None),
        "closed" if text.contains("not planned") => (Lucide::CircleSlash, Some(Tone::Muted.color(cx))),
        "closed" => (Lucide::CircleCheck, Some(Tone::Merged.color(cx))),
        "reopened" => (Lucide::CircleDot, Some(Tone::Open.color(cx))),
        "merged" => (Lucide::GitMerge, Some(Tone::Merged.color(cx))),
        "assigned" | "unassigned" => (Lucide::User, None),
        "renamed" => (Lucide::Pencil, None),
        "review_requested" | "review_request_removed" | "ready_for_review" => (Lucide::Eye, None),
        "convert_to_draft" => (Lucide::GitPullRequestDraft, None),
        "milestoned" | "demilestoned" => (Lucide::Milestone, None),
        "locked" | "unlocked" => (Lucide::Lock, None),
        "cross-referenced" | "referenced" => (Lucide::Link, None),
        _ => (Lucide::GitBranch, None),
    };
    let picture = widgets::avatar(actor.avatar_url.as_deref(), px(18.), cx);
    let mut line: Div = h_flex()
        .gap_2()
        .items_center()
        .flex_wrap()
        .text_sm()
        .child(widgets::event_icon(icon, tint, cx))
        .child(picture)
        .child(div().font_semibold().child(SharedString::from(actor.login.clone())))
        .child(div().text_color(theme.muted_foreground).child(SharedString::from(text.to_owned())));
    if let Some(label) = label {
        line = line.child(widgets::label_chip(label, cx)).child(div().text_color(theme.muted_foreground).child("label"));
    }
    line.child(div().text_color(theme.muted_foreground).child(widgets::when(at))).into_any_element()
}

fn loading(cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(Spinner::new().small())
        .child("Loading…")
        .into_any_element()
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}
