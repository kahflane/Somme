//! Building by chatting. The user says what they want, and the LLM adds,
//! changes and joins steps with tools; each change shows on the canvas at
//! once. A turn can be undone in one step. Nothing runs while building.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AppContext as _, AsyncWindowContext, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity,
    Window, div, prelude::FluentBuilder as _, px,
};
use somme_ai::AiError;
use somme_ai::llm::{ChatReply, ChatRequest, Llm, Message};
use somme_automation::{Automation, layout};

use super::editor::{FlowEditor, Tab};
use crate::ai::{self, Clients, Feature};
use crate::automations::builder::{self, Facts};
use crate::runtime;
use crate::services::Services;
use crate::settings::Settings;
use crate::store::Store;
use crate::ui::attachments::{self, Attached};
use crate::ui::common::{Lucide, ink_button};

/// Tool rounds in one turn, at most.
const MAX_ROUNDS: usize = 16;
/// Bytes read from each attached file.
const FILE_LIMIT: u64 = 64 * 1024;
/// Changes listed under a turn before the rest are counted.
const SHOWN_CHANGES: usize = 5;

pub(super) struct Chat {
    input: Entity<InputState>,
    turns: Vec<Turn>,
    /// What the LLM was told and said, without the system message.
    history: Vec<Message>,
    task: Option<Task<()>>,
    /// The server can't call tools, so it sends whole flows instead.
    no_tools: bool,
    _subscription: Subscription,
}

struct Turn {
    you: String,
    /// Names of the files attached to it.
    files: Vec<String>,
    said: String,
    changes: Vec<String>,
    error: Option<String>,
    working: bool,
    /// The flow before this turn, to undo it.
    before: Automation,
    /// How long `history` was before this turn.
    history_len: usize,
    undone: bool,
    /// A whole flow to use, from a server without tools.
    offer: Option<Automation>,
}

impl Chat {
    pub(super) fn new(window: &mut Window, cx: &mut Context<FlowEditor>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Say what it should do"));
        let subscription = cx.subscribe_in(&input, window, |editor, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                editor.send_chat(window, cx);
            }
        });
        Self { input, turns: Vec::new(), history: Vec::new(), task: None, no_tools: false, _subscription: subscription }
    }

    pub(super) fn is_working(&self) -> bool {
        self.task.is_some()
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
}

impl FlowEditor {
    pub(super) fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.chat.input.read(cx).value().trim().to_owned();
        if text.is_empty() || self.chat.is_working() {
            return;
        }
        let settings = Settings::get(cx).clone();
        if !ai::allowed(&settings, Feature::Builder) {
            return;
        }
        self.chat.input.update(cx, |input, cx| input.set_value("", window, cx));
        let attached = attachments::take(cx);
        let store = Store::global(cx).read(cx);
        let facts = Facts {
            hosts: store.hosts.iter().map(|host| (host.label.clone(), host.tags.clone())).collect(),
            snippets: store.snippets.iter().map(|snippet| (snippet.label.clone(), snippet.script.clone())).collect(),
            decide: ai::allowed(&settings, Feature::Decide),
            write: ai::allowed(&settings, Feature::Write),
            agent: ai::allowed(&settings, Feature::Agent),
            screen: ai::allowed(&settings, Feature::Computer),
            vault: crate::automations::Automations::global(cx).read(cx).vault_names.clone().unwrap_or_default(),
        };
        self.chat.turns.push(Turn {
            you: text.clone(),
            files: attached.iter().map(file_name).collect(),
            said: String::new(),
            changes: Vec::new(),
            error: None,
            working: true,
            before: self.draft.clone(),
            history_len: self.chat.history.len(),
            undone: false,
            offer: None,
        });
        let ask = Ask { services: Store::services(cx), settings, text, attached, facts };
        self.chat.task = Some(cx.spawn_in(window, async move |editor, cx| {
            let outcome = converse(&editor, ask, cx).await;
            let _ = editor.update(cx, |editor, cx| editor.end_turn(outcome.err(), cx));
        }));
        cx.notify();
    }

    fn end_turn(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.chat.task = None;
        let Some(turn) = self.chat.turns.last_mut() else { return };
        turn.working = false;
        if let Some(error) = error {
            // A turn cut short leaves tool calls without answers; drop it from
            // what the LLM sees. Its changes stay, to keep or undo.
            self.chat.history.truncate(turn.history_len);
            turn.error = Some(error);
        }
        cx.notify();
    }

    fn stop_chat(&mut self, cx: &mut Context<Self>) {
        if self.chat.task.take().is_some() {
            self.end_turn(Some("Stopped.".into()), cx);
        }
    }

    /// Puts the flow back as it was before turn `ix`, and every turn after it.
    fn undo_turn(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.is_working() {
            return;
        }
        let Some(turn) = self.chat.turns.get(ix) else { return };
        let (before, history_len) = (turn.before.clone(), turn.history_len);
        for turn in &mut self.chat.turns[ix..] {
            turn.undone = true;
            turn.offer = None;
        }
        self.chat.history.truncate(history_len);
        // Keep what the user named it since.
        self.draft = Automation { id: self.draft.id.clone(), ..before };
        self.after_chat_change(true, window, cx);
    }

    fn use_offer(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flow) = self.chat.turns.get_mut(ix).and_then(|turn| turn.offer.take()) else { return };
        if let Some(turn) = self.chat.turns.get_mut(ix) {
            turn.changes.push(format!("Used the flow it sent: {} steps", flow.nodes.len()));
        }
        self.draft = flow;
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

    /// Applies one reply's tool calls. True when the LLM is done.
    fn take_reply(&mut self, reply: &ChatReply, facts: &Facts, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.chat.history.push(Message::from_reply(reply));
        let (mut reshaped, mut changed) = (false, false);
        let mut changes = Vec::new();
        for call in &reply.tool_calls {
            let applied = builder::apply(&mut self.draft, call, facts);
            self.chat.history.push(Message::tool_result(call.id.clone(), applied.result));
            changed |= applied.change.is_some();
            changes.extend(applied.change);
            reshaped |= applied.reshaped;
        }
        if let Some(turn) = self.chat.turns.last_mut() {
            turn.changes.extend(changes);
            let text = reply.text.trim();
            if !text.is_empty() {
                if !turn.said.is_empty() {
                    turn.said.push_str("\n\n");
                }
                turn.said.push_str(text);
            }
        }
        if changed {
            self.after_chat_change(reshaped, window, cx);
        } else {
            cx.notify();
        }
        reply.tool_calls.is_empty()
    }

    pub(super) fn render_chat(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let working = self.chat.is_working();
        // Only the newest turn that is not undone can be undone, and turns before it with it.
        let last_kept = self.chat.turns.iter().rposition(|turn| !turn.undone && !turn.changes.is_empty());
        let turns = self.chat.turns.iter().enumerate().map(|(ix, turn)| {
            let you = v_flex()
                .items_end()
                .gap_1()
                .child(div().max_w(px(280.)).px_2p5().py_1p5().rounded_lg().bg(theme.secondary).text_sm().child(SharedString::from(turn.you.clone())))
                .children(turn.files.iter().map(|name| {
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Icon::new(Lucide::File).size(px(12.)))
                        .child(SharedString::from(name.clone()))
                }));
            let shown = turn.changes.len().min(SHOWN_CHANGES);
            let changes = v_flex()
                .gap_0p5()
                .children(turn.changes[..shown].iter().map(|change| {
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .when(turn.undone, |row| row.line_through())
                        .child(Icon::new(Lucide::Check).size(px(12.)).text_color(theme.muted_foreground))
                        .child(SharedString::from(change.clone()))
                }))
                .when(turn.changes.len() > shown, |list| {
                    list.child(div().text_xs().text_color(theme.muted_foreground).child(format!("and {} more", turn.changes.len() - shown)))
                });
            let offer = turn.offer.as_ref().map(|flow| {
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(("chat-use", ix))
                            .custom(ink_button(cx))
                            .xsmall()
                            .label(format!("Use these {} steps", flow.nodes.len()))
                            .on_click(cx.listener(move |editor, _, window, cx| editor.use_offer(ix, window, cx))),
                    )
                    .child(
                        Button::new(("chat-skip", ix)).ghost().xsmall().label("Not this").on_click(cx.listener(move |editor, _, _, cx| {
                            if let Some(turn) = editor.chat.turns.get_mut(ix) {
                                turn.offer = None;
                            }
                            cx.notify();
                        })),
                    )
            });
            let undo = (!working && last_kept == Some(ix)).then(|| {
                h_flex().child(
                    Button::new(("chat-undo", ix))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(Lucide::RotateCcw))
                        .label("Undo")
                        .tooltip("Put the steps back as they were before this")
                        .on_click(cx.listener(move |editor, _, window, cx| editor.undo_turn(ix, window, cx))),
                )
            });
            v_flex()
                .gap_2()
                .child(you)
                .when(!turn.changes.is_empty(), |el| el.child(changes))
                .when(!turn.said.is_empty(), |el| el.child(div().text_sm().child(SharedString::from(turn.said.clone()))))
                .when(turn.working, |el| {
                    el.child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(Spinner::new().xsmall())
                            .child("Building\u{2026}"),
                    )
                })
                .when_some(turn.error.clone(), |el, error| el.child(div().text_sm().text_color(theme.foreground).child(SharedString::from(error))))
                .children(offer)
                .when(turn.undone, |el| el.child(div().text_xs().text_color(theme.muted_foreground).child("Undone.")))
                .children(undo)
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
                        .child("Build by chatting"),
                )
                .child("Say what it should do, like \u{201c}every hour check disk space on the web servers and tell me when one is over 90%\u{201d}.")
                .child("The steps show up on the canvas as they are made. Nothing runs until you press Run, and commands it writes are marked for you to check.")
        });
        let footer = h_flex()
            .flex_none()
            .p_2()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .child(div().flex_1().min_w_0().child(Input::new(&self.chat.input).small()))
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

fn file_name(file: &Attached) -> String {
    file.path.file_name().map_or_else(|| file.path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// Whether the server turned the request down for having tools.
fn no_tool_support(err: &AiError) -> bool {
    match err {
        AiError::Status { status, body, .. } => {
            let body = body.to_lowercase();
            matches!(status, 400 | 404 | 422 | 500 | 501) && (body.contains("tool") || body.contains("function"))
        }
        _ => false,
    }
}

/// One turn: talks with the LLM until it stops calling tools.
async fn converse(editor: &WeakEntity<FlowEditor>, ask: Ask, cx: &mut AsyncWindowContext) -> Result<(), String> {
    let Ask { services, settings, text, attached, mut facts } = ask;
    let gone = |_| "The automation was closed.".to_owned();
    let (llm, cleaner) = runtime::io(async move {
        let clients = Clients::load(&services, &settings, Feature::Builder).await;
        (clients.llm, clients.cleaner)
    })
    .await;
    let llm = Arc::new(llm.ok_or("No LLM is set up. Add one in Settings \u{2192} AI.")?);
    let cleaner = Arc::new(cleaner);

    let mut said = cleaner.clean(&text);
    for file in attached {
        let (name, path) = (file_name(&file), file.path.clone());
        match file.disk.run(move |disk| disk.read(&path, FILE_LIMIT)).await {
            Ok(bytes) => said.push_str(&format!("\n\nAttached file {name}:\n```\n{}\n```", cleaner.clean(&String::from_utf8_lossy(&bytes)))),
            Err(err) => said.push_str(&format!("\n\n(The attached file {name} could not be read: {err})")),
        }
    }
    for snippet in &mut facts.snippets {
        snippet.1 = cleaner.clean(&snippet.1);
    }
    editor.update(cx, |editor, _| editor.chat.history.push(Message::user(said))).map_err(gone)?;

    if !editor.read_with(cx, |editor, _| editor.chat.no_tools).map_err(gone)? {
        for round in 0..MAX_ROUNDS {
            let request = editor
                .read_with(cx, |editor, _| ChatRequest {
                    messages: std::iter::once(Message::system(cleaner.clean(&builder::system_prompt(&editor.draft, &facts))))
                        .chain(editor.chat.history.iter().cloned())
                        .collect(),
                    tools: builder::tools(),
                    temperature: Some(0.2),
                    max_tokens: Some(4000),
                    ..ChatRequest::default()
                })
                .map_err(gone)?;
            let reply = ask_llm(&llm, request).await;
            let reply = match reply {
                Ok(reply) => reply,
                Err(err) if round == 0 && no_tool_support(&err) => {
                    editor.update(cx, |editor, _| editor.chat.no_tools = true).map_err(gone)?;
                    break;
                }
                Err(err) => return Err(err.to_string()),
            };
            let done = editor.update_in(cx, |editor, window, cx| editor.take_reply(&reply, &facts, window, cx)).map_err(gone)?;
            if done {
                return Ok(());
            }
        }
        if !editor.read_with(cx, |editor, _| editor.chat.no_tools).map_err(gone)? {
            return Err("That took too many steps. Say what is left and it carries on.".into());
        }
    }

    // A server without tools sends the whole flow, for the user to accept.
    let request = editor
        .read_with(cx, |editor, _| ChatRequest {
            messages: std::iter::once(Message::system(cleaner.clean(&builder::json_prompt(&editor.draft, &facts))))
                .chain(editor.chat.history.iter().cloned())
                .collect(),
            json: true,
            temperature: Some(0.2),
            max_tokens: Some(6000),
            ..ChatRequest::default()
        })
        .map_err(gone)?;
    let reply = ask_llm(&llm, request).await.map_err(|err| err.to_string())?;
    editor
        .update(cx, |editor, cx| {
            editor.chat.history.push(Message::assistant(reply.text.clone()));
            let flow = builder::read_flow(&reply.text, &editor.draft, &facts)?;
            if let Some(turn) = editor.chat.turns.last_mut() {
                turn.said = "This server can't change steps one by one, so it sent the whole automation.".into();
                turn.offer = Some(flow);
            }
            cx.notify();
            Ok(())
        })
        .map_err(gone)?
}

async fn ask_llm(llm: &Arc<Llm>, request: ChatRequest) -> Result<ChatReply, AiError> {
    let llm = llm.clone();
    runtime::io(async move { llm.chat(&request).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_without_tools_is_told_apart() {
        let status = |status: u16, body: &str| AiError::Status { service: "The LLM", status, body: body.into() };
        assert!(no_tool_support(&status(400, "registry.ollama.ai/library/gemma does not support tools")));
        assert!(no_tool_support(&status(422, "Unknown field: functions")));
        assert!(!no_tool_support(&status(401, "bad key")));
        assert!(!no_tool_support(&status(400, "context too long")));
    }
}
