use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::channel::oneshot;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Entity, Focusable as _, ParentElement as _, PromptButton, PromptLevel, Styled as _, Window,
    div, px,
};
use somme_application::ports::{PromptAnswer, PromptKind};

use crate::services::PromptRequest;
use crate::ui::native;

type Answer = Rc<RefCell<Option<oneshot::Sender<PromptAnswer>>>>;

fn reply(answer: &Answer, value: PromptAnswer) {
    if let Some(sender) = answer.borrow_mut().take() {
        let _ = sender.send(value);
    }
}

/// Asks the user what the backend needs: a password, a passphrase, answers to
/// server questions, or whether to trust a host key.
pub fn show(request: PromptRequest, window: &mut Window, cx: &mut App) {
    let answer: Answer = Rc::new(RefCell::new(Some(request.answer)));
    let title = request.title;
    match request.kind {
        PromptKind::Password { username, host } => {
            let detail = format!("Password for {username}@{host}");
            ask_inputs(title, detail, vec![("Password".into(), false)], answer, window, cx);
        }
        PromptKind::Passphrase { key_label } => {
            let detail = format!("The key \u{201c}{key_label}\u{201d} is protected by a passphrase.");
            ask_inputs(title, detail, vec![("Passphrase".into(), false)], answer, window, cx);
        }
        PromptKind::KeyboardInteractive {
            name,
            instructions,
            prompts,
        } => {
            if prompts.is_empty() {
                reply(&answer, PromptAnswer::Answers(Vec::new()));
                return;
            }
            let detail = [name, instructions]
                .into_iter()
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let fields = prompts.into_iter().map(|p| (p.text, p.echo)).collect();
            ask_inputs(title, detail, fields, answer, window, cx);
        }
        PromptKind::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            previous_fingerprint,
        } => ask_host_key(host, port, key_type, fingerprint, previous_fingerprint, answer, window, cx),
    }
}

fn ask_inputs(
    title: String,
    detail: String,
    fields: Vec<(String, bool)>,
    answer: Answer,
    window: &mut Window,
    cx: &mut App,
) {
    let inputs: Vec<(String, Entity<InputState>)> = fields
        .into_iter()
        .map(|(label, echo)| {
            let placeholder = label.trim().trim_end_matches(':').to_owned();
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).masked(!echo));
            (label, input)
        })
        .collect();
    let first_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first_focus.replace(false)
            && let Some((_, input)) = inputs.first()
        {
            input.focus_handle(cx).focus(window, cx);
        }
        let collect = {
            let inputs = inputs.clone();
            let answer = answer.clone();
            move |cx: &App| {
                let values = inputs.iter().map(|(_, i)| i.read(cx).value().to_string()).collect();
                reply(&answer, PromptAnswer::Answers(values));
            }
        };
        let on_ok = {
            let collect = collect.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                collect(cx);
                true
            }
        };
        let on_close = {
            let answer = answer.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, _: &mut App| reply(&answer, PromptAnswer::Cancel)
        };
        let cancel = {
            let answer = answer.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                reply(&answer, PromptAnswer::Cancel);
                window.close_dialog(cx);
            }
        };
        let submit = move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            collect(cx);
            window.close_dialog(cx);
        };
        let theme = cx.theme();
        let body = v_flex()
            .gap_3()
            .when_text(&detail, theme.muted_foreground)
            .children(inputs.iter().map(|(label, input)| {
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child(label.clone()))
                    .child(Input::new(input).mask_toggle())
            }));
        dialog
            .title(title.clone())
            .w(px(440.))
            .overlay_closable(false)
            .child(body)
            .on_ok(on_ok)
            .on_close(on_close)
            .footer(
                DialogFooter::new()
                    .child(Button::new("prompt-cancel").label("Cancel").on_click(cancel))
                    .child(Button::new("prompt-ok").primary().label("Continue").on_click(submit)),
            )
    });
}

/// Asks whether to trust a server's key, in the system's alert.
#[allow(clippy::too_many_arguments)]
fn ask_host_key(
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    previous: Option<String>,
    answer: Answer,
    window: &mut Window,
    cx: &mut App,
) {
    let address = if port == 22 { host } else { format!("{host}:{port}") };
    let changed = previous.is_some();
    let (level, headline, detail, trust) = match previous {
        Some(previous) => (
            PromptLevel::Critical,
            "The host key has changed",
            format!(
                "{address} sent a different key than last time. This can mean someone is intercepting the \
                 connection. Only continue if you know the server was reinstalled or its key was replaced.\n\n\
                 {key_type} fingerprint:\n{fingerprint}\n\nPrevious fingerprint:\n{previous}"
            ),
            "Trust New Key",
        ),
        None => (
            PromptLevel::Warning,
            "New host",
            format!(
                "This is the first connection to {address}. Check the fingerprint before you trust it.\n\n\
                 {key_type} fingerprint:\n{fingerprint}"
            ),
            "Trust and Connect",
        ),
    };
    // A changed key keeps Cancel first, so Return does not trust it by accident.
    let cancel = PromptButton::cancel("Cancel");
    let trust = PromptButton::ok(trust);
    let (buttons, trust_ix) = if changed { ([cancel, trust], 1) } else { ([trust, cancel], 0) };
    let picked = native::ask(level, headline, Some(&detail), &buttons, window, cx);
    cx.spawn(async move |_| {
        let accepted = picked.await == Ok(trust_ix);
        reply(&answer, if accepted { PromptAnswer::Accept } else { PromptAnswer::Cancel });
    })
    .detach();
}

trait WhenText: gpui_kit::ParentElement + Sized {
    /// Adds a muted paragraph unless `text` is blank.
    fn when_text(self, text: &str, color: gpui_kit::Hsla) -> Self {
        if text.trim().is_empty() {
            return self;
        }
        self.child(div().text_sm().text_color(color).child(text.to_owned()))
    }
}

impl<T: gpui_kit::ParentElement> WhenText for T {}
