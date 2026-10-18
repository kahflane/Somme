//! The pull requests or issues of the terminal's repository, beside the
//! terminals. A click on one opens it in [`DetailView`].

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::api::{ApiError, GitHub, Item, Kind, Page, State};
use super::counts::Counts;
use super::mentions::{self, Mentions};
use super::detail::{DetailEvent, DetailView};
use super::repo::Repo;
use super::widgets::{self, Tone};
use crate::runtime;
use crate::services::Services;
use crate::ui::common::Lucide;

pub enum GitHubPanelEvent {
    Close,
}

/// A new issue being written.
struct Draft {
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    mentions: Option<Entity<Mentions>>,
    busy: bool,
}

pub struct GitHubPanel {
    services: Arc<Services>,
    /// The repository as the folder's git remote names it.
    repo: Repo,
    /// Where it is on GitHub now, once known. Its new name is the one to ask with.
    real: Option<Repo>,
    /// The open count the title bar showed last, to see when it changes.
    seen: Option<u64>,
    kind: Kind,
    state: State,
    /// None until GitHub gave Somme access.
    client: Option<GitHub>,
    connecting: bool,
    filter: Entity<InputState>,
    page: Option<Page>,
    loading: bool,
    error: Option<String>,
    /// Bumped by every load, so an older answer can't land on a newer list.
    generation: u64,
    detail: Option<(Entity<DetailView>, Subscription)>,
    draft: Option<Draft>,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GitHubPanelEvent> for GitHubPanel {}

impl GitHubPanel {
    pub fn new(repo: Repo, kind: Kind, services: Arc<Services>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter, like label:bug or author:octocat"));
        let subscriptions = vec![
            cx.subscribe_in(&filter, window, |panel, _, event, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    panel.load(false, cx);
                }
            }),
            cx.observe_global::<Counts>(|panel, cx| panel.follow_counts(cx)),
        ];
        let client = services.sync.github_token().and_then(|token| GitHub::new(token).ok());
        let mut panel = Self {
            services,
            real: Counts::real(&repo, cx),
            seen: None,
            repo,
            kind,
            state: State::Open,
            client,
            connecting: false,
            filter,
            page: None,
            loading: false,
            error: None,
            generation: 0,
            detail: None,
            draft: None,
            scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        panel.load(false, cx);
        panel
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Shows the list of `kind` for `repo`, from the top.
    pub fn show(&mut self, repo: Repo, kind: Kind, cx: &mut Context<Self>) {
        if repo == self.repo && kind == self.kind && self.detail.is_none() && self.draft.is_none() {
            return;
        }
        self.real = Counts::real(&repo, cx);
        self.seen = None;
        self.repo = repo;
        self.kind = kind;
        self.state = State::Open;
        self.detail = None;
        self.draft = None;
        self.page = None;
        self.load(false, cx);
    }

    /// The name to ask GitHub with.
    fn api_repo(&self) -> Repo {
        self.real.clone().unwrap_or_else(|| self.repo.clone())
    }

    /// Shows the list again when the title bar's number changes.
    fn follow_counts(&mut self, cx: &mut Context<Self>) {
        let Some(summary) = Counts::summary(&self.repo, cx) else { return };
        let open = if self.kind == Kind::Pulls { summary.pulls } else { summary.issues };
        let changed = self.seen.is_some_and(|seen| seen != open);
        self.seen = Some(open);
        let plain_list = self.state == State::Open
            && self.detail.is_none()
            && self.draft.is_none()
            && self.filter.read(cx).value().trim().is_empty()
            // Not while the user reads further down.
            && self.scroll.offset().y >= px(0.);
        if changed && plain_list && !self.loading {
            self.load(false, cx);
        }
    }

    fn load(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let after = if more {
            match self.page.as_ref().and_then(|page| page.next.clone()) {
                Some(after) => Some(after),
                None => return,
            }
        } else {
            None
        };
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let (repo, real, kind, state) = (self.repo.clone(), self.real.clone(), self.kind, self.state);
        let query = self.filter.read(cx).value().to_string();
        cx.spawn(async move |this, cx| {
            let result = runtime::io(async move {
                // Search finds nothing under a moved repository's old name.
                let summary = match real {
                    Some(_) => None,
                    None => Some(client.summary(&repo).await?),
                };
                let real = real.or_else(|| summary.as_ref().map(|summary| summary.repo.clone())).unwrap_or(repo);
                let page = client.list(&real, kind, state, &query, after).await?;
                Ok::<_, ApiError>((summary, page))
            })
            .await;
            let _ = this.update(cx, |panel, cx| {
                if panel.generation != generation {
                    return;
                }
                panel.loading = false;
                let result = result.map(|(summary, page)| {
                    if let Some(summary) = summary {
                        panel.real = Some(summary.repo.clone());
                        Counts::learned(panel.repo.clone(), summary, cx);
                    }
                    page
                });
                match result {
                    Ok(page) => match panel.page.as_mut().filter(|_| more) {
                        Some(shown) => {
                            shown.items.extend(page.items);
                            shown.next = page.next;
                        }
                        None => {
                            panel.page = Some(page);
                            panel.scroll.set_offset(Default::default());
                        }
                    },
                    Err(err) => panel.failed(err, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows what went wrong. A refused token is dropped, so Connect shows again.
    fn failed(&mut self, err: ApiError, cx: &mut Context<Self>) {
        if matches!(err, ApiError::Unauthorized) {
            let _ = self.services.sync.forget_github();
            Counts::forget_client(cx);
            self.client = None;
            self.detail = None;
        }
        self.error = Some(err.to_string());
        cx.notify();
    }

    /// Asks GitHub, in the browser, to let Somme see the user's repositories.
    fn connect(&mut self, cx: &mut Context<Self>) {
        self.connecting = true;
        self.error = None;
        let sync = self.services.sync.clone();
        cx.spawn(async move |this, cx| {
            let token = runtime::io(async move { sync.connect_github().await }).await;
            let _ = this.update(cx, |panel, cx| {
                panel.connecting = false;
                match token.map_err(|err| err.to_string()).and_then(|t| GitHub::new(t).map_err(|e| e.to_string())) {
                    Ok(client) => {
                        Counts::use_client(client.clone(), cx);
                        panel.client = Some(client);
                        panel.load(false, cx);
                    }
                    Err(err) => panel.error = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks GitHub again for what the panel shows, and for the title bar's numbers.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        Counts::refresh(&self.repo, &self.services, true, cx);
        match &self.detail {
            Some((detail, _)) => detail.update(cx, |detail, cx| detail.load(cx)),
            None => self.load(false, cx),
        }
    }

    /// Opens Somme's page on github.com, where the user can let it see more
    /// organizations. An organization may also need an owner to approve it.
    fn manage_access(&mut self, cx: &mut Context<Self>) {
        let sync = self.services.sync.clone();
        cx.spawn(async move |this, cx| {
            let page = runtime::io(async move { sync.github_access_page().await }).await;
            let _ = this.update(cx, |panel, cx| match page {
                Ok(page) => cx.open_url(&page),
                Err(err) => {
                    panel.error = Some(err.to_string());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn cancel_connect(&mut self, cx: &mut Context<Self>) {
        self.services.sync.cancel_sign_in();
        self.connecting = false;
        cx.notify();
    }

    fn set_state(&mut self, state: State, cx: &mut Context<Self>) {
        if self.state != state {
            self.state = state;
            self.load(false, cx);
        }
    }

    pub fn open(&mut self, number: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else { return };
        let (repo, kind) = (self.api_repo(), self.kind);
        let view = cx.new(|cx| DetailView::new(client, repo, kind, number, window, cx));
        let subscription = cx.subscribe(&view, |panel, _, event, cx| match event {
            DetailEvent::Back => {
                panel.detail = None;
                cx.notify();
            }
            // A change shows in the list too.
            DetailEvent::Changed => panel.load(false, cx),
            DetailEvent::Failed(err) => panel.failed(err.clone(), cx),
        });
        self.draft = None;
        self.detail = Some((view, subscription));
        cx.notify();
    }

    fn new_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        let body = cx.new(|cx| TextareaState::new(window, cx).auto_grow(8, 24).placeholder("Add a description"));
        title.update(cx, |title, cx| title.focus(window, cx));
        let repo = self.api_repo().clone();
        let mentions = self.client.clone().map(|client| cx.new(|cx| Mentions::new(body.clone(), client, repo, cx)));
        self.detail = None;
        self.draft = Some(Draft {
            title,
            body,
            mentions,
            busy: false,
        });
        cx.notify();
    }

    fn submit_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(client), Some(draft)) = (self.client.clone(), self.draft.as_mut()) else { return };
        let title = draft.title.read(cx).value().trim().to_owned();
        if title.is_empty() || draft.busy {
            return;
        }
        let body = draft.body.read(cx).value().to_string();
        draft.busy = true;
        self.error = None;
        let repo = self.api_repo();
        cx.spawn_in(window, async move |this, cx| {
            let created = runtime::io(async move { client.create_issue(&repo, &title, &body).await }).await;
            let _ = this.update_in(cx, |panel, window, cx| match created {
                Ok(number) => {
                    panel.kind = Kind::Issues;
                    panel.load(false, cx);
                    panel.open(number, window, cx);
                }
                Err(err) => {
                    if let Some(draft) = panel.draft.as_mut() {
                        draft.busy = false;
                    }
                    panel.failed(err, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let repo = self.api_repo();
        let url = format!("{}/{}", repo.url(), if self.kind == Kind::Pulls { "pulls" } else { "issues" });
        h_flex()
            .h(px(40.))
            .flex_none()
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(Lucide::Github).size(px(16.)))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_semibold()
                    .child(SharedString::from(repo.full_name())),
            )
            .child(div().flex_1())
            .when(self.client.is_some(), |bar| {
                bar.child(
                    Button::new("gh-refresh")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::RefreshCw))
                        .tooltip("Refresh")
                        .on_click(cx.listener(|panel, _, _, cx| panel.refresh(cx))),
                )
                .child(
                    Button::new("gh-access")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::Building2))
                        .tooltip("Choose which organizations Somme can see")
                        .on_click(cx.listener(|panel, _, _, cx| panel.manage_access(cx))),
                )
            })
            .child(
                Button::new("gh-open-web")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ExternalLink))
                    .tooltip("Open on GitHub")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .child(
                Button::new("gh-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Close")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(GitHubPanelEvent::Close))),
            )
    }

    fn render_connect(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let action = if self.connecting {
            v_flex()
                .items_center()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .child(Spinner::new().small())
                        .child(div().text_sm().child("Waiting for GitHub in your browser…")),
                )
                .child(
                    Button::new("gh-cancel-connect")
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|panel, _, _, cx| panel.cancel_connect(cx))),
                )
                .into_any_element()
        } else {
            Button::new("gh-connect")
                .primary()
                .label("Connect GitHub")
                .icon(Icon::new(Lucide::Github))
                .on_click(cx.listener(|panel, _, _, cx| panel.connect(cx)))
                .into_any_element()
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(Icon::new(Lucide::Github).size(px(40.)).text_color(theme.muted_foreground))
            .child(div().text_base().font_semibold().child("See pull requests and issues here"))
            .child(
                div()
                    .max_w(px(360.))
                    .text_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(
                        "GitHub asks you once, in your browser, to let Somme read and work on your repositories. \
                         The key stays in this Mac's keychain.",
                    ),
            )
            .child(action)
            .children(self.error.clone().map(|err| error_line(err, cx)))
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pull = self.kind == Kind::Pulls;
        let (open, closed) = self.page.as_ref().map_or((None, None), |page| (Some(page.open), Some(page.closed)));
        let count = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_default();
        let toggle = |id: &'static str, icon: Lucide, text: String, state: State, cx: &mut Context<Self>| {
            let on = self.state == state;
            h_flex()
                .id(id)
                .gap_1()
                .items_center()
                .cursor_pointer()
                .text_sm()
                .when(on, |el| el.font_semibold().text_color(theme.foreground))
                .when(!on, |el| el.text_color(theme.muted_foreground).hover(|el| el.text_color(theme.foreground)))
                .child(Icon::new(icon).size(px(14.)))
                .child(text)
                .on_click(cx.listener(move |panel, _, _, cx| panel.set_state(state, cx)))
        };
        let open_toggle = toggle(
            "gh-open",
            if pull { Lucide::GitPullRequest } else { Lucide::CircleDot },
            format!("{} Open", count(open)),
            State::Open,
            cx,
        );
        let closed_toggle = toggle("gh-closed", Lucide::Check, format!("{} Closed", count(closed)), State::Closed, cx);
        let bar = h_flex()
            .flex_none()
            .px_3()
            .py_2()
            .gap_4()
            .items_center()
            .bg(theme.muted.opacity(0.5))
            .border_b_1()
            .border_color(theme.border)
            .child(open_toggle)
            .child(closed_toggle)
            .child(div().flex_1())
            .when(self.loading, |bar| bar.child(Spinner::new().xsmall()));

        let items = self.page.as_ref().map(|page| page.items.as_slice()).unwrap_or_default();
        let rows: Vec<AnyElement> = items.iter().map(|item| self.render_row(item, cx)).collect();
        let more = self.page.as_ref().is_some_and(|page| page.next.is_some());
        let empty = self.page.is_some() && items.is_empty() && !self.loading;
        let what = if pull { "pull requests" } else { "issues" };
        let state_word = if self.state == State::Open { "open" } else { "closed" };

        let mut filter_row = h_flex()
            .flex_none()
            .p_2()
            .gap_2()
            .child(div().flex_1().child(Input::new(&self.filter).small().prefix(Icon::new(IconName::Search).xsmall())));
        if !pull {
            filter_row = filter_row.child(
                Button::new("gh-new-issue")
                    .small()
                    .success()
                    .label("New issue")
                    .on_click(cx.listener(|panel, _, window, cx| panel.new_issue(window, cx))),
            );
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .child(filter_row)
            .children(self.error.clone().map(|err| {
                v_flex().px_2().pb_2().gap_1().items_start().child(error_line(err, cx)).child(
                    // Most often the repository belongs to an organization Somme can't see yet.
                    Button::new("gh-access-error")
                        .ghost()
                        .xsmall()
                        .label("Choose which organizations Somme can see")
                        .on_click(cx.listener(|panel, _, _, cx| panel.manage_access(cx))),
                )
            }))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .mx_2()
                    .mb_2()
                    .border_1()
                    .border_color(theme.border)
                    .rounded_md()
                    .overflow_hidden()
                    .child(bar)
                    .child(
                        v_flex()
                            .id("gh-rows")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .children(rows)
                            .when(empty, |list| {
                                list.child(
                                    v_flex()
                                        .py_12()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Icon::new(if pull { Lucide::GitPullRequest } else { Lucide::CircleDot })
                                                .size(px(24.))
                                                .text_color(theme.muted_foreground),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(theme.muted_foreground)
                                                .child(format!("No {state_word} {what} match.")),
                                        ),
                                )
                            })
                            .when(more, |list| {
                                list.child(
                                    div().p_2().flex().justify_center().child(
                                        Button::new("gh-more")
                                            .small()
                                            .ghost()
                                            .label("Show more")
                                            .disabled(self.loading)
                                            .on_click(cx.listener(|panel, _, _, cx| panel.load(true, cx))),
                                    ),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_row(&self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pull = self.kind == Kind::Pulls;
        let (icon, tone) = widgets::item_state(item, pull);
        let number = item.number;
        let author = item.author.login.clone();
        let line = match (item.state.as_str(), item.closed_at.as_deref()) {
            ("MERGED", Some(at)) => format!("#{number} by {author} was merged {}", widgets::when(at)),
            ("CLOSED", Some(at)) => format!("#{number} by {author} was closed {}", widgets::when(at)),
            _ => format!("#{number} opened {} by {author}", widgets::when(&item.created_at)),
        };
        let review = match item.review.as_deref() {
            Some("APPROVED") => Some(("Approved", Tone::Open)),
            Some("CHANGES_REQUESTED") => Some(("Changes requested", Tone::Closed)),
            Some("REVIEW_REQUIRED") => Some(("Review required", Tone::Muted)),
            _ => None,
        }
        .filter(|_| item.state == "OPEN");
        let checks = match item.checks.as_deref() {
            Some("SUCCESS") => Some((Lucide::Check, Tone::Open.color(cx))),
            Some("FAILURE" | "ERROR") => Some((Lucide::X, Tone::Closed.color(cx))),
            Some("PENDING" | "EXPECTED") => Some((Lucide::Dot, theme.warning)),
            _ => None,
        };
        let labels: Vec<_> = item.labels.iter().map(|label| widgets::label_chip(label, cx)).collect();
        h_flex()
            .id(("gh-row", number as usize))
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .items_start()
            .border_b_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(|row| row.bg(theme.muted.opacity(0.5)))
            .on_click(cx.listener(move |panel, _, window, cx| panel.open(number, window, cx)))
            .child(div().pt(px(2.)).child(Icon::new(icon).size(px(16.)).text_color(tone.color(cx))))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .items_center()
                            .child(div().text_sm().font_semibold().child(SharedString::from(item.title.clone())))
                            .children(labels),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(line)
                            .when(item.draft && item.state == "OPEN", |el| el.child("· Draft"))
                            .when_some(review, |el, (word, tone)| {
                                el.child("·").child(div().text_color(tone.color(cx)).child(word))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when_some(checks, |el, (icon, color)| el.child(Icon::new(icon).size(px(14.)).text_color(color)))
                    .when(item.comments > 0, |el| {
                        el.child(
                            h_flex()
                                .gap_0p5()
                                .items_center()
                                .child(Icon::new(Lucide::MessageSquare).size(px(13.)))
                                .child(item.comments.to_string()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_draft(&self, draft: &Draft, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .id("gh-draft")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("gh-draft-back")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ArrowLeft))
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.draft = None;
                                cx.notify();
                            })),
                    )
                    .child(div().text_base().font_semibold().child("New issue")),
            )
            .child(Input::new(&draft.title))
            .child({
                let body = div().font_family(theme.mono_font_family.clone()).child(Textarea::new(&draft.body));
                match &draft.mentions {
                    Some(mentions) => mentions::field(body, mentions),
                    None => body,
                }
            })
            .children(self.error.clone().map(|err| error_line(err, cx)))
            .child(
                h_flex().justify_end().child(
                    Button::new("gh-draft-submit")
                        .success()
                        .small()
                        .label(if draft.busy { "Submitting…" } else { "Submit new issue" })
                        .disabled(draft.busy)
                        .on_click(cx.listener(|panel, _, window, cx| panel.submit_issue(window, cx))),
                ),
            )
            .into_any_element()
    }
}

/// A red line with what went wrong.
pub fn error_line(err: String, cx: &gpui_kit::App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .gap_1p5()
        .items_start()
        .text_sm()
        .text_color(theme.danger)
        .child(div().pt(px(2.)).child(Icon::new(Lucide::CircleAlert).size(px(14.))))
        .child(div().flex_1().child(err))
}

impl Focusable for GitHubPanel {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitHubPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.client.is_none() {
            self.render_connect(cx)
        } else if let Some((detail, _)) = &self.detail {
            detail.clone().into_any_element()
        } else if let Some(draft) = &self.draft {
            self.render_draft(draft, cx)
        } else {
            self.render_list(cx)
        };
        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .border_l_1()
            .border_color(cx.theme().border)
            .child(self.render_header(cx))
            // Holds the body to the room under the header, so its own scroll reaches the end.
            .child(v_flex().flex_1().min_h_0().child(body))
    }
}
