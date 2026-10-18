//! Voice typing in a terminal: the mic button in the command box, the offer to
//! download the speech model, and the shortcut. The words go in as if pasted.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px, relative,
};

use super::{TerminalView, ToggleVoice};
use crate::settings::Settings;
use crate::ui::common::Lucide;
use crate::voice::{Phase, Voice, VoiceModel, megabytes};

/// The shortcut, as shown in hints.
pub(super) const SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘⇧Space" } else { "Ctrl+Shift+Space" };

/// Colors from the terminal's theme, so the button sits well with the chips.
pub(super) struct Palette {
    pub foreground: Hsla,
    pub muted: Hsla,
    pub red: Hsla,
    pub accent: Hsla,
}

impl TerminalView {
    /// Starts listening, or stops and types what was said. Before the model
    /// is downloaded, offers to download it.
    pub(super) fn toggle_voice(&mut self, _: &ToggleVoice, _: &mut Window, cx: &mut Context<Self>) {
        let voice = Voice::global(cx);
        let me = cx.entity_id();
        match voice.read(cx).phase() {
            Phase::Listening { owner } if owner == me => self.finish_voice(cx),
            Phase::Writing { owner } if owner == me => {}
            Phase::Listening { .. } | Phase::Writing { .. } => {
                self.voice_failed("Voice typing is on in another tab. Stop it there first.".to_owned(), cx);
            }
            Phase::Idle => {
                if !voice.read(cx).ready(Voice::chosen(cx)) {
                    if self.shows_mic(cx) {
                        self.voice_offer = !self.voice_offer;
                        cx.notify();
                    } else {
                        self.voice_failed("Download the speech model first, in Settings → Voice.".to_owned(), cx);
                    }
                    return;
                }
                self.voice_offer = false;
                match voice.update(cx, |voice, cx| voice.listen(me, cx)) {
                    Ok(()) if !self.shows_mic(cx) => {
                        self.voice_note = Some(self.add_note(format!("Listening… press {SHORTCUT} to stop"), cx));
                    }
                    Ok(()) => cx.notify(),
                    Err(err) => self.voice_failed(err, cx),
                }
            }
        }
    }

    fn finish_voice(&mut self, cx: &mut Context<Self>) {
        let task = Voice::global(cx).update(cx, |voice, cx| voice.finish(cx));
        if let Some(id) = self.voice_note {
            self.set_note_text(id, "Writing what you said…".to_owned(), cx);
        }
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| {
                if let Some(id) = view.voice_note.take() {
                    view.remove_note(id, cx);
                }
                match result {
                    Ok(text) => view.paste_text(&text, cx),
                    Err(err) => view.voice_failed(err, cx),
                }
            });
        })
        .detach();
    }

    /// Esc while listening stops without typing anything.
    pub(super) fn cancel_voice(&mut self, cx: &mut Context<Self>) -> bool {
        let voice = Voice::global(cx);
        if voice.read(cx).phase() == (Phase::Listening { owner: cx.entity_id() }) {
            voice.update(cx, |voice, cx| voice.cancel(cx));
            if let Some(id) = self.voice_note.take() {
                self.remove_note(id, cx);
            }
            return true;
        }
        if self.voice_offer {
            self.voice_offer = false;
            cx.notify();
            return true;
        }
        false
    }

    fn voice_failed(&mut self, text: String, cx: &mut Context<Self>) {
        let id = self.add_note(String::new(), cx);
        self.fail_note(id, text, cx);
    }

    /// Whether the command box, with its mic button, is on screen.
    fn shows_mic(&self, cx: &App) -> bool {
        self.prompt.is_some() && self.session.read(cx).shows_blocks()
    }

    /// The mic at the end of the chips row.
    pub(super) fn render_mic(&self, colors: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let voice = Voice::global(cx);
        let voice = voice.read(cx);
        let me = cx.entity_id();
        let (listening, writing) = match voice.phase() {
            Phase::Listening { owner } => (owner == me, false),
            Phase::Writing { owner } => (false, owner == me),
            Phase::Idle => (false, false),
        };
        let downloading = voice.download().map(|download| {
            let share = download.done.saturating_mul(100) / download.total.max(1);
            format!("{share}%")
        });
        let chip = h_flex()
            .id("voice-chip")
            .gap_1()
            .items_center()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .cursor_pointer();
        let chip = if listening {
            chip.bg(colors.red.opacity(0.18))
                .text_color(colors.red)
                .hover(|chip| chip.bg(colors.red.opacity(0.26)))
                .child(Icon::new(Lucide::Mic).size(px(12.)))
                .child("Listening · click to stop")
        } else if writing {
            chip.bg(colors.foreground.opacity(0.07))
                .text_color(colors.muted)
                .child(Spinner::new().xsmall())
                .child("Writing…")
        } else {
            chip.bg(colors.foreground.opacity(if self.voice_offer { 0.14 } else { 0.07 }))
                .text_color(if self.voice_offer { colors.foreground } else { colors.muted })
                .hover(|chip| chip.bg(colors.foreground.opacity(0.12)).text_color(colors.foreground))
                .child(Icon::new(Lucide::Mic).size(px(12.)))
                .when_some(downloading, |chip, share| chip.child(share))
        };
        chip.tooltip(|window, cx| Tooltip::new("Voice typing").action(&ToggleVoice, None).build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    cx.stop_propagation();
                    view.close_history(window, cx);
                    view.toggle_voice(&ToggleVoice, window, cx);
                }),
            )
            .into_any_element()
    }

    /// The offer to download the speech model, over the command box.
    pub(super) fn render_voice_offer(&self, colors: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.voice_offer {
            return None;
        }
        let voice = Voice::global(cx);
        let state = voice.read(cx);
        let chosen = Voice::chosen(cx);
        let text = |text: String| div().text_sm().text_color(colors.foreground).child(text);
        let note = |text: String| div().text_xs().text_color(colors.muted).child(text);
        let view = cx.entity().downgrade();
        let close = move |id: &'static str| {
            let view = view.clone();
            Button::new(id).small().ghost().label("Not now").on_click(move |_, _, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.voice_offer = false;
                    cx.notify();
                });
            })
        };
        let body = if let Some(download) = state.download() {
            let share = download.done as f32 / download.total.max(1) as f32;
            v_flex()
                .gap_2()
                .child(text(format!(
                    "Downloading the speech model… {} of {}",
                    megabytes(download.done),
                    megabytes(download.total)
                )))
                .child(
                    div()
                        .h(px(4.))
                        .w_full()
                        .rounded_full()
                        .bg(colors.foreground.opacity(0.1))
                        .child(div().h_full().rounded_full().bg(colors.accent).w(relative(share.clamp(0., 1.)))),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child({
                            let voice = voice.clone();
                            Button::new("voice-cancel")
                                .small()
                                .label("Stop download")
                                .on_click(move |_, _, cx| voice.update(cx, |voice, cx| voice.cancel_download(cx)))
                        })
                        .child(close("voice-hide").label("Hide")),
                )
        } else if state.ready(chosen) {
            let view = cx.entity().downgrade();
            v_flex()
                .gap_2()
                .child(text(format!(
                    "The speech model is ready. Click the mic or press {SHORTCUT}, talk, then press it again to stop."
                )))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("voice-start")
                                .small()
                                .primary()
                                .icon(Icon::new(Lucide::Mic))
                                .label("Start talking")
                                .on_click(move |_, window, cx| {
                                    let _ = view.update(cx, |view, cx| view.toggle_voice(&ToggleVoice, window, cx));
                                }),
                        )
                        .child(close("voice-close").label("Close")),
                )
        } else {
            let download = |id: &'static str, model: VoiceModel, label: String| {
                let voice = voice.clone();
                Button::new(id).small().label(label).on_click(move |_, _, cx| {
                    Settings::update(cx, |s| s.voice_model = model);
                    voice.update(cx, |voice, cx| voice.start_download(model, cx));
                })
            };
            v_flex()
                .gap_2()
                .child(text(
                    "Talk, and your words are typed here. The speech model runs on this computer, so nothing you say leaves it. It is downloaded once."
                        .to_owned(),
                ))
                .when_some(state.download_error.clone(), |body, error| {
                    body.child(div().text_xs().text_color(colors.red).child(error))
                })
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            download("voice-best", VoiceModel::Best, format!("Download ({})", VoiceModel::Best.size()))
                                .primary()
                                .icon(Icon::new(Lucide::Download)),
                        )
                        .child(download(
                            "voice-small",
                            VoiceModel::Small,
                            format!("Small model ({})", VoiceModel::Small.size()),
                        ))
                        .child(close("voice-later")),
                )
                .child(note("The small model is less accurate, but quicker on older computers.".to_owned()))
        };
        Some(
            v_flex()
                .gap_1p5()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(colors.foreground.opacity(0.12))
                .bg(colors.foreground.opacity(0.04))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_xs()
                        .text_color(colors.muted)
                        .child(Icon::new(Lucide::Mic).size(px(12.)))
                        .child("VOICE TYPING"),
                )
                .child(body)
                .into_any_element(),
        )
    }
}
