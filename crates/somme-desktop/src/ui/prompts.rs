use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures::channel::oneshot;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{App, AppContext as _, Entity, Focusable as _, ParentElement as _, Styled as _, Window, div, px};
use somme_application::ports::{PromptAnswer, PromptKind};

use crate::services::PromptRequest;

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
        } => ask_host_key(title, host, port, key_type, fingerprint, previous_fingerprint, answer, window, cx),
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

#[allow(clippy::too_many_arguments)]
fn ask_host_key(
    title: String,
    host: String,
    port: u16,
    key_type: String,
    fingerprint: String,
    previous: Option<String>,
    answer: Answer,
    window: &mut Window,
    cx: &mut App,
) {
    let changed = previous.is_some();
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let address = if port == 22 { host.clone() } else { format!("{host}:{port}") };
        let (headline, detail, color) = if changed {
            (
                "The host key has changed",
                format!(
                    "{address} sent a different key than last time. This can mean someone is intercepting the \
                     connection. Only continue if you know the server was reinstalled or its key was replaced."
                ),
                theme.danger,
            )
        } else {
            (
                "New host",
                format!("This is the first connection to {address}. Check the fingerprint before you trust it."),
                theme.warning,
            )
        };
        let fingerprint_row = |label: &str, value: &str| {
            v_flex()
                .gap_0p5()
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_owned()))
                .child(
                    div()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .p_2()
                        .rounded_md()
                        .bg(theme.muted)
                        .child(value.to_owned()),
                )
        };
        let mut body = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::TriangleAlert).text_color(color))
                    .child(div().font_semibold().child(headline)),
            )
            .child(div().text_sm().text_color(theme.muted_foreground).child(detail))
            .child(fingerprint_row(&format!("{key_type} fingerprint"), &fingerprint));
        if let Some(previous) = &previous {
            body = body.child(fingerprint_row("Previous fingerprint", previous));
        }
        let accept = {
            let answer = answer.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                reply(&answer, PromptAnswer::Accept);
                window.close_dialog(cx);
            }
        };
        let cancel = {
            let answer = answer.clone();
            move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                reply(&answer, PromptAnswer::Cancel);
                window.close_dialog(cx);
            }
        };
        let on_close = {
            let answer = answer.clone();
            move |_: &gpui_kit::ClickEvent, _: &mut Window, _: &mut App| reply(&answer, PromptAnswer::Cancel)
        };
        let accept_button = Button::new("hostkey-accept").label(if changed { "Trust New Key" } else { "Trust and Connect" });
        let accept_button = if changed { accept_button.danger() } else { accept_button.primary() };
        dialog
            .title(title.clone())
            .w(px(500.))
            .overlay_closable(false)
            .child(body)
            .on_close(on_close)
            .footer(
                DialogFooter::new()
                    .child(Button::new("hostkey-cancel").label("Cancel").on_click(cancel))
                    .child(accept_button.on_click(accept)),
            )
    });
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
