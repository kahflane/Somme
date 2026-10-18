//! Suggests people while an `@mention` is typed in a comment, as GitHub does:
//! those who can be mentioned in the repository, such as the members of its
//! organization, and those who took part in the conversation first.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::input::{Enter, Escape, IndentInline, InputEvent, MoveDown, MoveUp, TextareaState};
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Context, Div, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, anchored, deferred, div,
    point, prelude::FluentBuilder as _, px,
};

use super::api::{GitHub, Person, User};
use super::repo::Repo;
use super::widgets;
use crate::runtime;

const SHOWN: usize = 8;
/// GitHub is asked once typing pauses this long.
const PAUSE: Duration = Duration::from_millis(150);

/// The `@mention` being typed just before `cursor`: where its `@` is, and
/// what follows it so far.
fn typing_at(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = text.get(..cursor)?;
    let at = before.rfind('@')?;
    let typed = &before[at + 1..];
    if typed.len() > 39 || !typed.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    // Not the middle of a word, like an email address.
    let after_word = before[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '`');
    (!after_word).then_some((at, typed))
}

fn matches(person: &Person, typed: &str) -> bool {
    let typed = typed.to_lowercase();
    person.login.to_lowercase().starts_with(&typed)
        || person.name.as_deref().is_some_and(|name| name.to_lowercase().split_whitespace().any(|word| word.starts_with(&typed)))
}

pub struct Mentions {
    input: Entity<TextareaState>,
    client: GitHub,
    repo: Repo,
    /// People who took part, suggested before anyone else.
    taking_part: Vec<Person>,
    /// What GitHub suggested for each thing typed.
    found: HashMap<String, Vec<Person>>,
    /// The mention being typed: where its `@` is, and what follows it.
    typing: Option<(usize, String)>,
    shown: Vec<Person>,
    selected: usize,
    _asking: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Mentions {
    pub fn new(input: Entity<TextareaState>, client: GitHub, repo: Repo, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // The caret moves without a change too.
            cx.observe(&input, |mentions, _, cx| mentions.follow(cx)),
            cx.subscribe(&input, |mentions, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Blur) {
                    mentions.close(cx);
                }
            }),
        ];
        Self {
            input,
            client,
            repo,
            taking_part: Vec::new(),
            found: HashMap::new(),
            typing: None,
            shown: Vec::new(),
            selected: 0,
            _asking: Task::ready(()),
            _subscriptions: subscriptions,
        }
    }

    /// Suggests these people first, as they took part in the conversation.
    pub fn set_taking_part(&mut self, users: impl IntoIterator<Item = User>) {
        self.taking_part.clear();
        for user in users {
            if !user.login.is_empty() && !self.taking_part.iter().any(|known| known.login == user.login) {
                self.taking_part.push(Person {
                    login: user.login,
                    name: None,
                    avatar_url: user.avatar_url,
                });
            }
        }
    }

    fn is_open(&self) -> bool {
        self.typing.is_some() && !self.shown.is_empty()
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let state = self.input.read(cx);
        let text = state.value();
        let typing = typing_at(&text, state.cursor()).map(|(at, typed)| (at, typed.to_owned()));
        if typing == self.typing {
            return;
        }
        let changed = typing.as_ref().map(|(_, typed)| typed) != self.typing.as_ref().map(|(_, typed)| typed);
        self.typing = typing;
        if changed {
            self.selected = 0;
            self.ask(cx);
        }
        self.show(cx);
    }

    /// Asks GitHub who matches, unless it already said.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let Some((_, typed)) = self.typing.clone() else { return };
        if self.found.contains_key(&typed) {
            return;
        }
        let (client, repo) = (self.client.clone(), self.repo.clone());
        self._asking = cx.spawn(async move |mentions, cx| {
            cx.background_executor().timer(PAUSE).await;
            let asked = typed.clone();
            let found = runtime::io(async move { client.mentionable(&repo, &asked).await }).await;
            let _ = mentions.update(cx, |mentions, cx| {
                match found {
                    Ok(people) => {
                        mentions.found.insert(typed, people);
                    }
                    Err(err) => tracing::debug!(%err, "no one to mention"),
                }
                mentions.show(cx);
            });
        });
    }

    fn show(&mut self, cx: &mut Context<Self>) {
        self.shown.clear();
        if let Some((_, typed)) = &self.typing {
            // While GitHub is asked, what it said for less of the name still fits.
            let found: &[Person] = (0..=typed.len())
                .rev()
                .find_map(|end| typed.get(..end).and_then(|start| self.found.get(start)))
                .map_or(&[], Vec::as_slice);
            for person in self.taking_part.iter().chain(found) {
                if self.shown.len() < SHOWN
                    && matches(person, typed)
                    && !self.shown.iter().any(|shown| shown.login == person.login)
                {
                    // GitHub's answer has their full name.
                    let person = found.iter().find(|it| it.login == person.login).unwrap_or(person);
                    self.shown.push(person.clone());
                }
            }
        }
        self.selected = self.selected.min(self.shown.len().saturating_sub(1));
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.typing.take().is_some() {
            self.shown.clear();
            cx.notify();
        }
    }

    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some((at, _)), Some(person)) = (self.typing.take(), self.shown.get(ix).cloned()) else { return };
        self.shown.clear();
        self.input.update(cx, |state, cx| {
            let cursor = state.cursor();
            state.set_selected_range(at..cursor, cx);
            state.replace(format!("@{} ", person.login), window, cx);
        });
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.shown.len() as isize;
        self.selected = (self.selected as isize + by).rem_euclid(count.max(1)) as usize;
        cx.notify();
    }
}

/// Lets `mentions` take the keys it needs from the text box in `field`, and
/// adds its list.
pub fn field(field: Div, mentions: &Entity<Mentions>) -> Div {
    let handle = |mentions: &Entity<Mentions>| mentions.downgrade();
    let (up, down, enter, tab, escape) =
        (handle(mentions), handle(mentions), handle(mentions), handle(mentions), handle(mentions));
    field
        .capture_action(move |_: &MoveUp, _, cx| {
            let _ = up.update(cx, |mentions, cx| {
                if mentions.is_open() {
                    mentions.step(-1, cx);
                    cx.stop_propagation();
                }
            });
        })
        .capture_action(move |_: &MoveDown, _, cx| {
            let _ = down.update(cx, |mentions, cx| {
                if mentions.is_open() {
                    mentions.step(1, cx);
                    cx.stop_propagation();
                }
            });
        })
        .capture_action(move |_: &Enter, window, cx| {
            let _ = enter.update(cx, |mentions, cx| {
                if mentions.is_open() {
                    mentions.pick(mentions.selected, window, cx);
                    cx.stop_propagation();
                }
            });
        })
        .capture_action(move |_: &IndentInline, window, cx| {
            let _ = tab.update(cx, |mentions, cx| {
                if mentions.is_open() {
                    mentions.pick(mentions.selected, window, cx);
                    cx.stop_propagation();
                }
            });
        })
        .capture_action(move |_: &Escape, _, cx| {
            let _ = escape.update(cx, |mentions, cx| {
                if mentions.is_open() {
                    mentions.close(cx);
                    cx.stop_propagation();
                }
            });
        })
        .child(mentions.clone())
}

impl Render for Mentions {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((at, _)) = self.typing.as_ref().filter(|_| !self.shown.is_empty()) else {
            return div().into_any_element();
        };
        let Some(spot) = self.input.read(cx).range_to_bounds(&(*at..*at)) else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let rows = self.shown.iter().enumerate().map(|(ix, person)| {
            let selected = ix == self.selected;
            h_flex()
                .id(("mention", ix))
                .px_2()
                .py_1()
                .gap_2()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(selected, |row| row.bg(theme.accent).text_color(theme.accent_foreground))
                .when(!selected, |row| row.hover(|row| row.bg(theme.accent.opacity(0.5))))
                .child(widgets::avatar(person.avatar_url.as_deref(), px(20.), cx))
                .child(div().font_semibold().child(SharedString::from(person.login.clone())))
                .when_some(person.name.clone(), |row, name| {
                    row.child(div().min_w_0().truncate().text_color(theme.muted_foreground).child(name))
                })
                .on_click(cx.listener(move |mentions, _, window, cx| mentions.pick(ix, window, cx)))
        });
        let rows: Vec<_> = rows.collect();
        deferred(
            anchored().position(point(spot.left(), spot.bottom() + px(4.))).snap_to_window_with_margin(px(8.)).child(
                v_flex()
                    .id("mentions")
                    .occlude()
                    .w(px(300.))
                    .p_1()
                    .text_sm()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .shadow_lg()
                    .children(rows),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_starts_with_an_at_outside_a_word() {
        assert_eq!(typing_at("@", 1), Some((0, "")));
        assert_eq!(typing_at("hi @ann", 7), Some((3, "ann")));
        assert_eq!(typing_at("(@ann-b", 7), Some((1, "ann-b")));
        assert_eq!(typing_at("hi @ann there", 13), None);
        assert_eq!(typing_at("mail me@ann", 11), None);
        assert_eq!(typing_at("hi @ann", 5), Some((3, "a")));
        assert_eq!(typing_at("no at", 5), None);
    }

    #[test]
    fn people_match_by_login_or_any_name() {
        let ann = Person {
            login: "ann-dev".into(),
            name: Some("Ann Lee".into()),
            avatar_url: None,
        };
        assert!(matches(&ann, ""));
        assert!(matches(&ann, "AN"));
        assert!(matches(&ann, "lee"));
        assert!(!matches(&ann, "dev"));
    }
}
