//! The chat in an automation: the assistant does jobs with the user. It
//! looks at the machine with commands that only look, asks before each
//! change, and checks what it did; what a change deleted or changed can be
//! put back. When the job should happen again, it makes the automation on
//! the canvas. Its words show as they are written, and each command it runs
//! shows as a row whose output opens. A turn's changes to the steps can be
//! undone, and one that failed tried again.

use std::sync::Arc;
use std::time::Duration;

use futures::channel::oneshot;
use futures::{FutureExt as _, StreamExt as _};

use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, AsyncWindowContext, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use somme_ai::llm::{Effort, Message};
use somme_automation::agent::{self, Event, Job, Session};
use somme_automation::fence::{self, Fence};
use somme_automation::nodes::{Approval, Approved};
use somme_automation::{Automation, HostRef, layout, nodes};
use somme_domain::Id;

use super::editor::{FlowEditor, Tab};
use crate::ai::{self, Clients, Feature, GuardAsk};
use crate::automations::assistant::{self, ChatEvent, Link};
use crate::automations::builder::Facts;
use crate::automations::tools::AppTools;
use crate::automations::{memory, skills};
use crate::runtime;
use crate::services::Services;
use crate::settings::Settings;
use crate::store::Store;
use crate::ui::attachments::{self, Attached};
use crate::ui::common::{Lucide, ink_button};

/// Bytes read from each attached file.
const FILE_LIMIT: u64 = 64 * 1024;
/// Tool calls in one turn, at most.
const MAX_STEPS: u32 = 80;
/// How long one turn may take, waits for the user included.
const TURN_TIME: Duration = Duration::from_secs(60 * 60);
/// Lines of a command's output shown when its row opens.
const OUTPUT_LINES: usize = 40;

pub(super) struct Chat {
    input: Entity<TextareaState>,
    turns: Vec<Turn>,
    /// The talk with the assistant, from one message to the next. Empty
    /// until the first message.
    session: Session,
    task: Option<Task<()>>,
    /// Kept at the newest words while a turn is going.
    scroll: ScrollHandle,
    _subscription: Subscription,
}

struct Turn {
    you: String,
    /// Names of the files attached to it.
    files: Vec<String>,
    /// What the assistant was sent, with the files in it, to send again on Retry.
    asked: String,
    items: Vec<Item>,
    /// The end of what it is writing now; gone once that part is done.
    live: String,
    error: Option<String>,
    working: bool,
    /// The steps before this turn, to undo it.
    before: Automation,
    /// The talk before this turn: on Stop or Retry, it goes back to this.
    talk: Session,
    undone: bool,
    /// Bin folders its changes filled: the server (None: this computer)
    /// and the folder.
    kept: Vec<(Option<String>, String)>,
    put_back: PutBack,
}

#[derive(Clone, PartialEq)]
enum PutBack {
    No,
    Working,
    Done(String),
}

/// One thing in a turn, in order.
enum Item {
    /// Its words, as Markdown. `rev` goes up when they change.
    Said { text: String, rev: usize },
    /// A command, file or request: what came back, once it did.
    Run { id: String, line: String, result: Option<String>, open: bool },
    /// A change waiting for the user's OK, or the OK given.
    Approve { approval: Approval, answer: Option<oneshot::Sender<Approved>>, chose: Option<Approved> },
    /// The guard wants the user's OK for something.
    Guard { ask: Option<GuardAsk>, host: String, script: String, reason: String, chose: Option<bool> },
    /// A question, and the user's answer once given.
    Question { text: String, details: String, choices: Vec<String>, answer: Option<oneshot::Sender<String>>, said: Option<String> },
    /// A change to the steps on the canvas.
    Change(String),
    /// Something it did that needs only a line: read a skill, remembered a fact.
    Line { icon: Lucide, text: String },
}

impl Chat {
    pub(super) fn new(window: &mut Window, cx: &mut Context<FlowEditor>) -> Self {
        // Enter sends; Shift+Enter starts a new line, and the box grows with it.
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8).submit_on_enter(true).placeholder("Ask for a job, like \u{201c}free up disk space\u{201d}"));
        let subscription = cx.subscribe_in(&input, window, |editor, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                editor.send_chat(window, cx);
            }
        });
        Self { input, turns: Vec::new(), session: Session::default(), task: None, scroll: ScrollHandle::new(), _subscription: subscription }
    }

    pub(super) fn is_working(&self) -> bool {
        self.task.is_some()
    }

    /// The question the newest turn waits on, if any.
    fn waiting_question(&mut self) -> Option<&mut Item> {
        let turn = self.turns.last_mut().filter(|turn| turn.working)?;
        turn.items.iter_mut().rev().find(|item| matches!(item, Item::Question { answer: Some(_), .. }))
    }
}

impl FlowEditor {
    /// Puts the cursor in the chat box, when the Chat tab shows.
    pub fn focus_chat(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == Tab::Chat {
            self.chat.input.update(cx, |input, cx| input.focus(window, cx));
        }
    }
}

/// What a turn needs from the app, gathered before it starts.
struct Ask {
    services: Arc<Services>,
    settings: Settings,
    text: String,
    attached: Vec<Attached>,
    facts: Facts,
    hosts: Vec<HostRef>,
    draft: Automation,
    talk: Session,
}

impl FlowEditor {
    pub(super) fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.chat.input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        // Words typed while a question waits answer it.
        if let Some(Item::Question { answer, said, .. }) = self.chat.waiting_question() {
            if let Some(answer) = answer.take() {
                let _ = answer.send(text.clone());
                *said = Some(text);
            }
            self.chat.input.update(cx, |input, cx| input.set_value("", window, cx));
            cx.notify();
            return;
        }
        if self.chat.is_working() {
            return;
        }
        let settings = Settings::get(cx).clone();
        if !ai::allowed(&settings, Feature::Builder) {
            return;
        }
        self.chat.input.update(cx, |input, cx| input.set_value("", window, cx));
        let attached = attachments::take(cx);
        self.chat.turns.push(Turn {
            you: text.clone(),
            files: attached.iter().map(file_name).collect(),
            asked: text.clone(),
            items: Vec::new(),
            live: String::new(),
            error: None,
            working: true,
            before: self.draft.clone(),
            talk: self.chat.session.clone(),
            undone: false,
            kept: Vec::new(),
            put_back: PutBack::No,
        });
        self.start_turn(text, attached, settings, window, cx);
    }

    /// Works on the newest turn.
    fn start_turn(&mut self, text: String, attached: Vec<Attached>, settings: Settings, window: &mut Window, cx: &mut Context<Self>) {
        let store = Store::global(cx).read(cx);
        let facts = Facts {
            hosts: store.hosts.iter().map(|host| (host.label.clone(), host.tags.clone())).collect(),
            snippets: store.snippets.iter().map(|snippet| (snippet.label.clone(), snippet.script.clone())).collect(),
            decide: ai::allowed(&settings, Feature::Decide),
            write: ai::allowed(&settings, Feature::Write),
            agent: ai::allowed(&settings, Feature::Agent),
            screen: ai::allowed(&settings, Feature::Computer),
            vault: crate::automations::Automations::global(cx).read(cx).vault_names.clone().unwrap_or_default(),
            computer: this_computer(),
        };
        let hosts = store
            .hosts
            .iter()
            .map(|host| HostRef { id: host.id.to_string(), name: host.label.clone(), address: host.address.clone(), tags: host.tags.clone() })
            .collect();
        let ask = Ask { services: Store::services(cx), settings, text, attached, facts, hosts, draft: self.draft.clone(), talk: self.chat.session.clone() };
        self.chat.scroll.scroll_to_bottom();
        self.chat.task = Some(cx.spawn_in(window, async move |editor, cx| {
            let ended = converse(&editor, ask, cx).await;
            let _ = editor.update(cx, |editor, cx| editor.end_turn(ended, cx));
        }));
        cx.notify();
    }

    /// Asks again for the newest turn, after it failed or was stopped. What
    /// it changed in the steps is put back first.
    fn retry_turn(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.is_working() || ix + 1 != self.chat.turns.len() {
            return;
        }
        let settings = Settings::get(cx).clone();
        if !ai::allowed(&settings, Feature::Builder) {
            return;
        }
        let Some(turn) = self.chat.turns.get_mut(ix) else { return };
        let undo = has_changes(turn) && !turn.undone;
        let before = turn.before.clone();
        self.chat.session = turn.talk.clone();
        let text = turn.asked.clone();
        turn.items.clear();
        turn.error = None;
        turn.working = true;
        turn.undone = false;
        if undo {
            self.draft = Automation { id: self.draft.id.clone(), ..before };
            self.after_chat_change(true, window, cx);
        }
        self.start_turn(text, Vec::new(), settings, window, cx);
    }

    fn end_turn(&mut self, ended: Result<(Session, Option<String>), String>, cx: &mut Context<Self>) {
        self.chat.task = None;
        let Some(turn) = self.chat.turns.last_mut() else { return };
        turn.working = false;
        turn.live.clear();
        // Anything still waiting on the user is past.
        for item in &mut turn.items {
            match item {
                Item::Approve { answer, .. } => *answer = None,
                Item::Guard { ask, .. } => *ask = None,
                Item::Question { answer, .. } => *answer = None,
                _ => {}
            }
        }
        match ended {
            Ok((session, error)) => {
                self.chat.session = session;
                turn.error = error;
            }
            Err(error) => {
                // Cut short: the talk goes back to before this turn. What it
                // changed stays, to keep, put back or undo.
                self.chat.session = turn.talk.clone();
                turn.error = Some(error);
            }
        }
        self.chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn stop_chat(&mut self, cx: &mut Context<Self>) {
        if self.chat.task.take().is_some() {
            self.end_turn(Err("Stopped.".into()), cx);
        }
    }

    /// Puts the steps back as they were before turn `ix`, and every turn after it.
    fn undo_turn(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.is_working() {
            return;
        }
        let Some(turn) = self.chat.turns.get(ix) else { return };
        let before = turn.before.clone();
        for turn in &mut self.chat.turns[ix..] {
            turn.undone = true;
        }
        // Keep what the user named it since.
        self.draft = Automation { id: self.draft.id.clone(), ..before };
        self.after_chat_change(true, window, cx);
    }

    fn after_chat_change(&mut self, reshaped: bool, window: &mut Window, cx: &mut Context<Self>) {
        if reshaped {
            layout::tidy(&mut self.draft);
            self.fit_pending = true;
            self.keep_fitted = true;
        }
        self.rebuild_forms(window, cx);
        self.changed(cx);
    }

    /// Shows what the assistant does as it does it.
    fn on_chat_event(&mut self, event: ChatEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(turn) = self.chat.turns.last_mut().filter(|turn| turn.working) else { return };
        match event {
            ChatEvent::Words(words) => {
                turn.live.push_str(&words);
                if turn.live.len() > 4000 {
                    let mut cut = turn.live.len() - 2000;
                    while !turn.live.is_char_boundary(cut) {
                        cut += 1;
                    }
                    turn.live.drain(..cut);
                }
            }
            ChatEvent::Calling(name) => turn.live = tool_words(&name).to_owned(),
            ChatEvent::Agent(event) => {
                turn.live.clear();
                match event {
                    Event::Said(text) => turn.items.push(Item::Said { text, rev: 0 }),
                    Event::Doing { id, name, line } => match name.as_str() {
                        "exec_command" | "read_file" | "apply_patch" | "http_request" => turn.items.push(Item::Run { id, line, result: None, open: false }),
                        "read_skill" => turn.items.push(Item::Line { icon: Lucide::BookOpen, text: line }),
                        "remember" => turn.items.push(Item::Line { icon: Lucide::Brain, text: line }),
                        _ => {}
                    },
                    Event::Done { id, result } => {
                        if let Some(Item::Run { result: slot, open, .. }) = turn.items.iter_mut().find(|item| matches!(item, Item::Run { id: run, .. } if *run == id)) {
                            // What failed shows at once.
                            *open = result.starts_with("Not done");
                            *slot = Some(result);
                        }
                    }
                    Event::Kept { host, folder, said } => {
                        turn.kept.push((host, folder));
                        turn.items.push(Item::Line { icon: Lucide::Archive, text: said });
                    }
                }
            }
            ChatEvent::Approve { approval, answer } => turn.items.push(Item::Approve { approval, answer: Some(answer), chose: None }),
            ChatEvent::Question { question, details, choices, answer } => {
                turn.items.push(Item::Question { text: question, details, choices, answer: Some(answer), said: None });
            }
            ChatEvent::Draft { automation, change, reshaped } => {
                turn.items.push(Item::Change(change));
                // Keep what the user named it since.
                self.draft = Automation { id: self.draft.id.clone(), ..*automation };
                self.chat.scroll.scroll_to_bottom();
                self.after_chat_change(reshaped, window, cx);
                return;
            }
        }
        self.chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// The guard wants the user's OK, in the chat rather than a dialog.
    fn on_guard_ask(&mut self, ask: GuardAsk, cx: &mut Context<Self>) {
        let Some(turn) = self.chat.turns.last_mut().filter(|turn| turn.working) else { return };
        let (host, script, reason) = (ask.host.clone(), ask.script.clone(), ask.reason.clone());
        turn.items.push(Item::Guard { ask: Some(ask), host, script, reason, chose: None });
        self.chat.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn answer_approval(&mut self, ix: usize, item: usize, chose: Approved, cx: &mut Context<Self>) {
        if let Some(Item::Approve { answer, chose: slot, .. }) = self.chat.turns.get_mut(ix).and_then(|turn| turn.items.get_mut(item))
            && let Some(answer) = answer.take()
        {
            let _ = answer.send(chose);
            *slot = Some(chose);
        }
        cx.notify();
    }

    fn answer_guard(&mut self, ix: usize, item: usize, allow: bool, cx: &mut Context<Self>) {
        if let Some(Item::Guard { ask, chose, .. }) = self.chat.turns.get_mut(ix).and_then(|turn| turn.items.get_mut(item))
            && let Some(ask) = ask.take()
        {
            let _ = ask.answer.send(allow);
            *chose = Some(allow);
        }
        cx.notify();
    }

    fn answer_question(&mut self, ix: usize, item: usize, text: String, cx: &mut Context<Self>) {
        if let Some(Item::Question { answer, said, .. }) = self.chat.turns.get_mut(ix).and_then(|turn| turn.items.get_mut(item))
            && let Some(answer) = answer.take()
        {
            let _ = answer.send(text.clone());
            *said = Some(text);
        }
        cx.notify();
    }

    fn toggle_run(&mut self, ix: usize, item: usize, cx: &mut Context<Self>) {
        if let Some(Item::Run { open, .. }) = self.chat.turns.get_mut(ix).and_then(|turn| turn.items.get_mut(item)) {
            *open = !*open;
        }
        cx.notify();
    }

    /// Puts back what turn `ix`'s changes deleted or changed, from the Somme bin.
    fn put_back(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(turn) = self.chat.turns.get_mut(ix).filter(|turn| !turn.kept.is_empty() && turn.put_back != PutBack::Working) else { return };
        turn.put_back = PutBack::Working;
        let kept = turn.kept.clone();
        let services = Store::services(cx);
        let hosts: Vec<(String, Id)> = Store::global(cx).read(cx).hosts.iter().map(|host| (host.label.clone(), host.id)).collect();
        cx.spawn_in(window, async move |editor, cx| {
            let said = runtime::io(async move {
                let mut said = Vec::new();
                // Newest first, so older copies win.
                for (host, folder) in kept.iter().rev() {
                    let script = fence::put_back_script(folder);
                    let out = match host {
                        None => nodes::run_local(&script, &[], None, Duration::from_secs(120)).await.map(|out| format!("{}{}", out.stdout, out.stderr)),
                        Some(name) => match hosts.iter().find(|(label, _)| label == name) {
                            Some((_, id)) => services
                                .scripts
                                .run(*id, &script, Duration::from_secs(120), 64 * 1024)
                                .await
                                .map(|out| format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
                                .map_err(|err| err.to_string()),
                            None => Err(format!("{name} is no longer a saved server")),
                        },
                    };
                    said.push(match out {
                        Ok(text) => text.trim().to_owned(),
                        Err(err) => format!("Could not put them back: {err}"),
                    });
                }
                said.join("\n")
            })
            .await;
            let _ = editor.update(cx, |editor, cx| {
                if let Some(turn) = editor.chat.turns.get_mut(ix) {
                    turn.put_back = PutBack::Done(said);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_chat(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let working = self.chat.is_working();
        // Only the newest turn that is not undone can be undone, and turns before it with it.
        let last_kept = self.chat.turns.iter().rposition(|turn| !turn.undone && has_changes(turn));
        let newest = self.chat.turns.len().checked_sub(1);
        let turns = self.chat.turns.iter().enumerate().map(|(ix, turn)| {
            let you = v_flex()
                .items_end()
                .gap_1()
                .child(
                    div()
                        .max_w(px(280.))
                        .px_2p5()
                        .py_1p5()
                        .rounded_lg()
                        .bg(theme.secondary)
                        .text_sm()
                        .whitespace_normal()
                        .child(words(format!("chat-you-{ix}"), turn.you.clone())),
                )
                .child(Clipboard::new(("chat-copy-you", ix)).value(turn.you.clone()).tooltip("Copy"))
                .children(turn.files.iter().map(|name| {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Lucide::File).size(px(12.)))
                        .child(SharedString::from(name.clone()))
                }));
            let items = turn.items.iter().enumerate().map(|(item_ix, item)| item_view(ix, item_ix, item, turn.undone, cx));
            let undo = (!working && last_kept == Some(ix)).then(|| {
                Button::new(("chat-undo", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::RotateCcw))
                    .label("Undo steps")
                    .tooltip("Put the steps back as they were before this")
                    .on_click(cx.listener(move |editor, _, window, cx| editor.undo_turn(ix, window, cx)))
            });
            let retry = (!working && turn.error.is_some() && newest == Some(ix)).then(|| {
                Button::new(("chat-retry", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::RefreshCw))
                    .label("Retry")
                    .tooltip("Ask again")
                    .on_click(cx.listener(move |editor, _, window, cx| editor.retry_turn(ix, window, cx)))
            });
            let put_back = (!turn.kept.is_empty() && !turn.working).then(|| match &turn.put_back {
                PutBack::No => Button::new(("chat-put-back", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ArchiveRestore))
                    .label("Put back")
                    .tooltip("Put back what this deleted or changed, from the Somme bin")
                    .on_click(cx.listener(move |editor, _, window, cx| editor.put_back(ix, window, cx)))
                    .into_any_element(),
                PutBack::Working => h_flex().gap_1().items_center().text_xs().child(Spinner::new().xsmall()).child("Putting back\u{2026}").into_any_element(),
                PutBack::Done(_) => div().into_any_element(),
            });
            let put_back_said = match &turn.put_back {
                PutBack::Done(said) if !said.is_empty() => Some(said.clone()),
                _ => None,
            };
            // What it said, and why it stopped, to copy.
            let said: Vec<&str> = turn.items.iter().filter_map(|item| if let Item::Said { text, .. } = item { Some(text.trim()) } else { None }).collect();
            let copied = said.into_iter().chain(turn.error.as_deref()).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n\n");
            let copy = (!turn.working && !copied.is_empty()).then(|| Clipboard::new(("chat-copy-said", ix)).value(copied).tooltip("Copy"));
            let actions = h_flex().gap_1().items_center().children(copy).children(put_back).children(retry).children(undo);
            v_flex()
                .gap_2()
                .child(you)
                .children(items)
                .when(turn.working, |el| {
                    el.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(Spinner::new().xsmall())
                            .child(if turn.items.iter().any(|item| matches!(item, Item::Approve { answer: Some(_), .. } | Item::Guard { ask: Some(_), .. } | Item::Question { answer: Some(_), .. })) {
                                "Waiting for you\u{2026}"
                            } else {
                                "Working\u{2026}"
                            }),
                    )
                    .children(live_tail(&turn.live).map(|tail| div().text_xs().italic().text_color(theme.muted_foreground).whitespace_normal().child(tail)))
                })
                .when_some(turn.error.clone(), |el, error| {
                    el.child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .px_2p5()
                            .py_1p5()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.muted)
                            .text_sm()
                            .child(Icon::new(Lucide::CircleAlert).size(px(14.)).mt_0p5().text_color(theme.muted_foreground))
                            .child(div().flex_1().min_w_0().whitespace_normal().child(words(format!("chat-error-{ix}"), error))),
                    )
                })
                .children(put_back_said.map(|said| {
                    h_flex()
                        .gap_1p5()
                        .items_start()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Lucide::ArchiveRestore).size(px(12.)).mt_0p5())
                        .child(div().flex_1().min_w_0().whitespace_normal().child(words(format!("chat-put-back-{ix}"), said)))
                }))
                .when(turn.undone, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child("Steps undone.")))
                .child(actions)
        });
        let intro = self.chat.turns.is_empty().then(|| {
            v_flex()
                .gap_2()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_color(theme.foreground)
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(Icon::new(Lucide::WandSparkles).size(px(16.)).text_color(theme.muted_foreground))
                        .child("Get it done by chatting"),
                )
                .child("Say what you want done, like \u{201c}free up space on this Mac\u{201d} or \u{201c}find out why web-1 is slow\u{201d}.")
                .child("The assistant looks around, asks before it changes anything, and checks its work. What it deletes or changes can be put back. When you want it to happen again, it makes the automation here.")
        });
        let footer = h_flex()
            .flex_none()
            .p_2()
            .gap_2()
            .items_end()
            .border_t_1()
            .border_color(theme.border)
            .child(div().flex_1().min_w_0().child(Textarea::new(&self.chat.input).small()))
            .child(if working {
                Button::new("chat-stop").outline().small().icon(Icon::new(Lucide::Square)).tooltip("Stop").on_click(cx.listener(|editor, _, _, cx| editor.stop_chat(cx)))
            } else {
                Button::new("chat-send")
                    .custom(ink_button(cx))
                    .small()
                    .icon(Icon::new(Lucide::ArrowUp))
                    .tooltip("Send")
                    .on_click(cx.listener(|editor, _, window, cx| editor.send_chat(window, cx)))
            });
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .id("chat-turns")
                    .track_scroll(&self.chat.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_4()
                    .children(intro)
                    .children(turns),
            )
            .child(footer)
            .into_any_element()
    }

    /// Whether the Chat tab shows: AI and building by chatting are on.
    pub(super) fn chat_allowed(cx: &gpui_kit::App) -> bool {
        ai::allowed(Settings::get(cx), Feature::Builder)
    }

    /// The tab to start on: Chat for an empty flow, when it can be used.
    pub(super) fn first_tab(automation: &Automation, cx: &gpui_kit::App) -> Tab {
        if automation.nodes.is_empty() && Self::chat_allowed(cx) { Tab::Chat } else { Tab::Settings }
    }
}

/// Whether a turn changed the steps.
fn has_changes(turn: &Turn) -> bool {
    turn.items.iter().any(|item| matches!(item, Item::Change(_)))
}

/// One item of turn `ix`.
fn item_view(ix: usize, item_ix: usize, item: &Item, undone: bool, cx: &Context<FlowEditor>) -> AnyElement {
    let theme = cx.theme().clone();
    let key = ix * 1000 + item_ix;
    let line = |icon: Lucide, text: String| {
        h_flex()
            .gap_1p5()
            .items_start()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).size(px(12.)).mt_0p5())
            .child(div().flex_1().min_w_0().whitespace_normal().child(words(format!("chat-line-{key}"), text)))
    };
    let card = || v_flex().gap_2().px_2p5().py_2().rounded_lg().border_1().border_color(theme.border).bg(theme.muted);
    let mono = theme.mono_font_family.clone();
    match item {
        Item::Said { text, rev } => said(key, *rev, text).into_any_element(),
        Item::Run { line: text, result, open, .. } => {
            let failed = result.as_deref().is_some_and(|result| result.starts_with("Not done") || exit_code(result).is_some_and(|code| code != 0));
            let icon = match (result, failed) {
                (None, _) => Lucide::LoaderCircle,
                (Some(_), true) => Lucide::CircleX,
                (Some(_), false) => Lucide::Terminal,
            };
            let row = h_flex()
                .id(("chat-run", key))
                .gap_1p5()
                .items_start()
                .text_xs()
                .text_color(theme.muted_foreground)
                .cursor_pointer()
                .child(Icon::new(icon).size(px(12.)).mt_0p5())
                .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).child(words(format!("chat-run-{key}"), text.clone())))
                .child(Icon::new(if *open { Lucide::ChevronDown } else { Lucide::ChevronRight }).size(px(12.)).mt_0p5())
                .on_click(cx.listener(move |editor, _, _, cx| editor.toggle_run(ix, item_ix, cx)));
            v_flex()
                .gap_1()
                .child(row)
                .when_some(result.as_ref().filter(|_| *open), |el, result| {
                    el.child(
                        div()
                            .ml_4()
                            .px_2()
                            .py_1p5()
                            .rounded_md()
                            .bg(theme.muted)
                            .text_xs()
                            .font_family(mono.clone())
                            .whitespace_normal()
                            .child(words(format!("chat-output-{key}"), last_lines(result, OUTPUT_LINES))),
                    )
                })
                .into_any_element()
        }
        Item::Approve { approval, answer, chose } => {
            let waiting = answer.is_some();
            let gone = if approval.for_good {
                "What it deletes is gone for good: it can be made again."
            } else {
                "What it deletes goes to the Somme bin for 7 days, and files are copied before they change."
            };
            card()
                .child(h_flex().gap_1p5().items_center().text_sm().child(Icon::new(Lucide::ShieldQuestionMark).size(px(14.))).child(SharedString::from(approval.title.clone())))
                .when(!approval.why.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).whitespace_normal().child(words(format!("chat-why-{key}"), approval.why.clone()))))
                .child(
                    div()
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .bg(theme.background)
                        .text_xs()
                        .font_family(mono.clone())
                        .whitespace_normal()
                        .child(words(format!("chat-detail-{key}"), last_lines(&approval.detail, 30))),
                )
                .child(div().text_xs().text_color(theme.muted_foreground).child(if approval.place.is_empty() { gone.to_owned() } else { format!("On {}. {gone}", approval.place) }))
                .child(match (waiting, chose) {
                    (true, _) => h_flex()
                        .flex_wrap()
                        .gap_1p5()
                        .child(Button::new(("chat-allow", key)).custom(ink_button(cx)).xsmall().label("Allow").on_click(cx.listener(move |editor, _, _, cx| editor.answer_approval(ix, item_ix, Approved::Once, cx))))
                        .child(
                            Button::new(("chat-allow-rest", key))
                                .outline()
                                .xsmall()
                                .label("Allow the rest of this task")
                                .on_click(cx.listener(move |editor, _, _, cx| editor.answer_approval(ix, item_ix, Approved::Rest, cx))),
                        )
                        .child(Button::new(("chat-deny", key)).ghost().xsmall().label("Deny").on_click(cx.listener(move |editor, _, _, cx| editor.answer_approval(ix, item_ix, Approved::No, cx))))
                        .into_any_element(),
                    (false, Some(Approved::Once)) => line(Lucide::Check, "You allowed it.".into()).into_any_element(),
                    (false, Some(Approved::Rest)) => line(Lucide::CheckCheck, "You allowed it and the rest of this task.".into()).into_any_element(),
                    (false, Some(Approved::No)) => line(Lucide::X, "You denied it.".into()).into_any_element(),
                    (false, None) => line(Lucide::Minus, "Not answered.".into()).into_any_element(),
                })
                .into_any_element()
        }
        Item::Guard { ask, host, script, reason, chose } => card()
            .child(
                h_flex()
                    .gap_1p5()
                    .items_start()
                    .text_sm()
                    .child(Icon::new(Lucide::ShieldAlert).size(px(14.)).mt_0p5())
                    .child(div().flex_1().min_w_0().whitespace_normal().child(words(format!("chat-guard-{key}"), format!("Your guard wants your OK first: {reason}")))),
            )
            .child(
                div()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(theme.background)
                    .text_xs()
                    .font_family(mono.clone())
                    .whitespace_normal()
                    .child(words(format!("chat-guard-script-{key}"), last_lines(script, 30))),
            )
            .when(!host.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).child(format!("On {host}."))))
            .child(match (ask.is_some(), chose) {
                (true, _) => h_flex()
                    .gap_1p5()
                    .child(Button::new(("chat-guard-allow", key)).custom(ink_button(cx)).xsmall().label("Allow").on_click(cx.listener(move |editor, _, _, cx| editor.answer_guard(ix, item_ix, true, cx))))
                    .child(Button::new(("chat-guard-deny", key)).ghost().xsmall().label("Deny").on_click(cx.listener(move |editor, _, _, cx| editor.answer_guard(ix, item_ix, false, cx))))
                    .into_any_element(),
                (false, Some(true)) => line(Lucide::Check, "You allowed it.".into()).into_any_element(),
                (false, Some(false)) => line(Lucide::X, "You denied it.".into()).into_any_element(),
                (false, None) => line(Lucide::Minus, "Not answered.".into()).into_any_element(),
            })
            .into_any_element(),
        Item::Question { text, details, choices, answer, said: answered } => {
            let waiting = answer.is_some();
            card()
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_start()
                        .text_sm()
                        .child(Icon::new(Lucide::MessageCircleQuestionMark).size(px(14.)).mt_0p5().text_color(theme.muted_foreground))
                        .child(div().flex_1().min_w_0().whitespace_normal().child(words(format!("chat-question-{key}"), text.clone()))),
                )
                .when(!details.is_empty(), |el| el.child(div().text_xs().text_color(theme.muted_foreground).whitespace_normal().child(words(format!("chat-details-{key}"), details.clone()))))
                .when(waiting, |el| {
                    let choices = if choices.is_empty() { vec!["Yes".to_owned(), "No".to_owned()] } else { choices.clone() };
                    el.child(h_flex().flex_wrap().gap_1p5().children(choices.into_iter().enumerate().map(|(choice_ix, choice)| {
                        let picked = choice.clone();
                        Button::new(("chat-choice", key * 8 + choice_ix))
                            .outline()
                            .xsmall()
                            .label(SharedString::from(choice))
                            .on_click(cx.listener(move |editor, _, _, cx| editor.answer_question(ix, item_ix, picked.clone(), cx)))
                    })))
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Or type your answer below."))
                })
                .children(answered.clone().map(|answered| {
                    h_flex().justify_end().child(
                        div().max_w(px(280.)).px_2p5().py_1p5().rounded_lg().bg(theme.secondary).text_sm().whitespace_normal().child(words(format!("chat-answer-{key}"), answered)),
                    )
                }))
                .into_any_element()
        }
        Item::Change(change) => h_flex()
            .gap_1p5()
            .items_center()
            .text_xs()
            .text_color(theme.muted_foreground)
            .when(undone, |row| row.line_through())
            .child(Icon::new(Lucide::Check).size(px(12.)))
            .child(div().flex_1().min_w_0().child(words(format!("chat-change-{key}"), change.clone())))
            .into_any_element(),
        Item::Line { icon, text } => line(*icon, text.clone()).into_any_element(),
    }
}

/// The exit code a command's result starts with.
fn exit_code(result: &str) -> Option<i32> {
    result.lines().next()?.strip_prefix("Exit code: ")?.trim().parse().ok()
}

/// The last `count` lines of `text`, marked when cut.
fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    if lines.len() <= count {
        return lines.join("\n");
    }
    format!("({} lines above)\n{}", lines.len() - count, lines[lines.len() - count..].join("\n"))
}

/// The end of what the LLM is writing, on one short run of lines.
fn live_tail(live: &str) -> Option<String> {
    const SHOWN: usize = 240;
    // Plain words: Markdown marks would show as they are.
    let flat = live.replace(['*', '`', '#'], "").split_whitespace().filter(|word| *word != "-").collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    let count = flat.chars().count();
    if count <= SHOWN {
        return Some(flat);
    }
    Some(format!("\u{2026}{}", flat.chars().skip(count - SHOWN).collect::<String>()))
}

/// What shows while it calls a tool.
fn tool_words(name: &str) -> &'static str {
    match name {
        "exec_command" => "Running a command\u{2026}",
        "read_file" => "Reading a file\u{2026}",
        "apply_patch" => "Changing files\u{2026}",
        "http_request" => "Sending a request\u{2026}",
        "ask_user" => "Asking you\u{2026}",
        "read_skill" => "Reading a skill\u{2026}",
        "remember" => "Remembering\u{2026}",
        "make_automatic" => "Making it automatic\u{2026}",
        "add_node" => "Adding a step\u{2026}",
        "update_node" => "Changing a step\u{2026}",
        "connect" => "Joining steps\u{2026}",
        "remove" => "Removing a step\u{2026}",
        "set_start" => "Setting when it runs\u{2026}",
        _ => "Working\u{2026}",
    }
}

/// Plain text the user can select and copy.
fn words(id: String, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(SharedString::from(id), text)
}

/// What the assistant said, as Markdown. Only web links open.
/// Keyed by `rev` too: each change is read in full and at once. Text added
/// to a view it already has is read in the background, and here that read
/// did not always reach the screen.
fn said(key: usize, rev: usize, text: &str) -> TextView {
    TextView::markdown(SharedString::from(format!("chat-said-{key}-{rev}")), text.to_owned()).selectable(true).text_sm().on_link_click(|url, _, _, cx| {
        if url.starts_with("https://") || url.starts_with("http://") {
            cx.open_url(url);
        }
    })
}

fn file_name(file: &Attached) -> String {
    file.path.file_name().map_or_else(|| file.path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// Whether the server turned the request down for having tools.
fn no_tool_support(err: &str) -> bool {
    let err = err.to_lowercase();
    (err.contains("tool") || err.contains("function")) && ["400", "404", "422", "500", "501", "not support"].iter().any(|sign| err.contains(sign))
}

/// One turn: the assistant works on the IO runtime, and what it does shows
/// here as it comes. Gives the talk after it, and why it stopped early.
async fn converse(editor: &WeakEntity<FlowEditor>, ask: Ask, cx: &mut AsyncWindowContext) -> Result<(Session, Option<String>), String> {
    let Ask { services, settings, text, attached, facts, hosts, draft, mut talk } = ask;
    let (events, mut arrived) = futures::channel::mpsc::unbounded::<ChatEvent>();
    let (guard_asks, mut guard_arrived) = futures::channel::mpsc::unbounded::<GuardAsk>();
    let mut work = Box::pin(runtime::io(async move {
        let clients = Clients::load(&services, &settings, Feature::Builder).await;
        let llm = Arc::new(clients.llm.ok_or("No LLM is set up. Add one in Settings \u{2192} AI.")?);
        let cleaner = Arc::new(clients.cleaner);
        let mut said = cleaner.clean(&text);
        for file in attached {
            let (name, path) = (file_name(&file), file.path.clone());
            match file.disk.run(move |disk| disk.read(&path, FILE_LIMIT)).await {
                Ok(bytes) => said.push_str(&format!("\n\nAttached file {name}:\n```\n{}\n```", cleaner.clean(&String::from_utf8_lossy(&bytes)))),
                Err(err) => said.push_str(&format!("\n\n(The attached file {name} could not be read: {err})")),
            }
        }
        let data_dir = services.data_dir.clone();
        let skills = skills::all(&data_dir);
        if talk.messages.is_empty() {
            let remembered = memory::newest(&data_dir);
            talk.messages.push(Message::system(cleaner.clean(&assistant::rules(&facts, &skills, &remembered, &draft))));
        }
        talk.messages.push(Message::user(said));
        let link = Arc::new(Link { events: events.clone(), llm, cleaner, data_dir, skills, facts, draft: parking_lot::Mutex::new(draft) });
        let tools = AppTools {
            services: services.clone(),
            who: "The assistant".into(),
            asks: Some(guard_asks),
            notes: None,
            banner: None,
            screen: Default::default(),
            ai_calls: Default::default(),
            secrets: Default::default(),
            chat: Some(link),
            cleared: Default::default(),
            automation: None,
        };
        let job = Job {
            hosts,
            local: true,
            local_first: true,
            ask_first: true,
            max_steps: MAX_STEPS,
            max_tokens: 4_000_000,
            timeout: TURN_TIME,
            // This computer's bin is in Somme's own folder.
            // Whole, as commands run in the home folder.
            fence: Some(Fence { bin_root: Some(std::path::absolute(services.data_dir.join("bin")).map_err(|err| err.to_string())?.display().to_string()), ..Fence::keep("chat", true) }),
            effort: Some(Effort::Low),
            ..Job::default()
        };
        let note = |event: Event| {
            let _ = events.unbounded_send(ChatEvent::Agent(event));
        };
        let finished = agent::resume(&job, &mut talk, &tools, &note).await;
        let answer = finished.result["answer"].as_str().unwrap_or_default().trim().to_owned();
        Ok::<_, String>((talk, answer, finished.error))
    }))
    .fuse();
    let gone = |_| "The automation was closed.".to_owned();
    let (talk, answer, error) = loop {
        futures::select! {
            event = arrived.next() => if let Some(event) = event {
                editor.update_in(cx, |editor, window, cx| editor.on_chat_event(event, window, cx)).map_err(gone)?;
            },
            ask = guard_arrived.next() => if let Some(ask) = ask {
                editor.update(cx, |editor, cx| editor.on_guard_ask(ask, cx)).map_err(gone)?;
            },
            done = work => break done?,
        }
    };
    // What came just before the end.
    while let Ok(event) = arrived.try_recv() {
        editor.update_in(cx, |editor, window, cx| editor.on_chat_event(event, window, cx)).map_err(gone)?;
    }
    editor
        .update(cx, |editor, cx| {
            if let Some(turn) = editor.chat.turns.last_mut()
                && !answer.is_empty()
                && !turn.items.iter().rev().find_map(|item| if let Item::Said { text, .. } = item { Some(text.trim() == answer) } else { None }).unwrap_or(false)
            {
                turn.items.push(Item::Said { text: answer, rev: 0 });
            }
            cx.notify();
        })
        .map_err(gone)?;
    let error = error.map(|error| {
        if no_tool_support(&error) { "This model can't use tools, which the assistant needs. Pick another in Settings \u{2192} AI.".to_owned() } else { error }
    });
    Ok((talk, error))
}

/// This computer, for the assistant: its system, shell and home folder.
fn this_computer() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{} ({}), commands run in /bin/sh, home {home}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_end_of_the_words_shows() {
        assert_eq!(live_tail("  **Done.** Runs `df -h /`\n- now "), Some("Done. Runs df -h / now".into()));
        let long = "word ".repeat(100);
        let tail = live_tail(&long).unwrap();
        assert!(tail.starts_with('\u{2026}') && tail.len() < 250, "{tail}");
        assert_eq!(live_tail(" \n "), None);
    }

    #[test]
    fn results_read_their_exit_code_and_end() {
        assert_eq!(exit_code("Exit code: 2\nnope"), Some(2));
        assert_eq!(exit_code("Not done: no"), None);
        assert_eq!(last_lines("a\nb\nc", 2), "(1 lines above)\nb\nc");
        assert!(no_tool_support("Status 400: tools are not supported") && !no_tool_support("Status 401: bad key"));
    }
}
