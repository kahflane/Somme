//! Voice typing in a terminal: the mic button in the command box, holding
//! Space to talk, the offer to download the speech model, and the shortcut.
//! The words go in as if pasted.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers,
    MouseButton, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, px, relative,
};

use super::{TerminalView, ToggleVoice};
use crate::settings::Settings;
use crate::ui::common::Lucide;
use crate::voice::{Phase, Voice, VoiceModel, megabytes};

/// The shortcut, as shown in hints.
pub(super) const SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘⇧Space" } else { "Ctrl+Shift+Space" };

/// Holding Space this long in the command box starts listening, instead of typing a space.
const HOLD: Duration = Duration::from_millis(350);

/// Bars in the wave shown while listening, and how tall each is next to the
/// middle one, so the wave rises in the centre.
const WAVE: [f32; 9] = [0.35, 0.55, 0.75, 0.9, 1., 0.9, 0.75, 0.55, 0.35];

/// A Space press in the command box that may turn into holding to talk.
pub(super) struct SpaceHold {
    /// Listening started, so letting go of Space stops it.
    talking: bool,
    _timer: Task<()>,
}

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

    /// Space in the command box. Held, it talks; tapped, it types a space,
    /// which goes in when Space comes back up, or when another key comes first.
    pub(super) fn space_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let space = event.keystroke.key == "space" && event.keystroke.modifiers == Modifiers::none();
        if let Some(hold) = &self.space {
            if space {
                // Space repeats while held.
                cx.stop_propagation();
            } else if !hold.talking {
                self.space = None;
                self.insert_in_prompt(" ", window, cx);
            }
            return;
        }
        if !space || event.is_held || !self.can_hold_to_talk(window, cx) {
            return;
        }
        cx.stop_propagation();
        let timer = cx.spawn_in(window, async move |view, cx| {
            cx.background_executor().timer(HOLD).await;
            let _ = view.update_in(cx, |view, window, cx| {
                let Some(hold) = view.space.as_mut().filter(|hold| !hold.talking) else { return };
                hold.talking = true;
                view.toggle_voice(&ToggleVoice, window, cx);
            });
        });
        self.space = Some(SpaceHold { talking: false, _timer: timer });
    }

    pub(super) fn space_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "space" {
            return;
        }
        let Some(hold) = self.space.take() else { return };
        if !hold.talking {
            self.insert_in_prompt(" ", window, cx);
        } else if Voice::global(cx).read(cx).phase() == (Phase::Listening { owner: cx.entity_id() }) {
            self.finish_voice(cx);
        }
    }

    /// Holding Space talks only in the command line itself, not the folder
    /// search, and only once the speech model is here, so until then it types spaces.
    fn can_hold_to_talk(&self, window: &Window, cx: &App) -> bool {
        let voice = Voice::global(cx);
        let voice = voice.read(cx);
        self.shows_mic(cx)
            && self.prompt_focused(window, cx)
            && voice.phase() == Phase::Idle
            && voice.ready(Voice::chosen(cx))
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
                    Ok(text) => {
                        // Words said after typed ones need a space between.
                        let joins = view.prompt.as_ref().is_some_and(|prompt| {
                            let editor = prompt.editor.read(cx);
                            let before = editor.value().get(..editor.cursor()).unwrap_or_default().to_owned();
                            before.ends_with(|c: char| !c.is_whitespace())
                        });
                        view.paste_text(&if joins { format!(" {text}") } else { text }, cx);
                    }
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
                .hover(|chip| chip.bg(colors.red.opacity(0.26)))
                .child(wave(voice.level(), voice.listening_for(), colors.red))
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
        let tip = if listening { "Listening. Let go of Space, or click, to stop" } else { "Voice typing: hold Space and talk" };
        chip.tooltip(move |window, cx| Tooltip::new(tip).action(&ToggleVoice, None).build(window, cx))
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
                    "The speech model is ready. Hold Space and talk, then let go. Or click the mic or press {SHORTCUT} to start and stop."
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

/// Bars that rise and fall with the microphone's `level`. `time` keeps them
/// moving a little, so they show it is listening even in a quiet room.
fn wave(level: f32, time: Duration, color: Hsla) -> impl IntoElement {
    let loud = (level * 12.).sqrt().min(1.);
    let time = time.as_secs_f32();
    h_flex().h(px(14.)).gap(px(2.)).items_center().children(WAVE.iter().enumerate().map(|(i, shape)| {
        let sway = 0.6 + 0.4 * (time * 9. + i as f32 * 0.9).sin();
        let height = 3. + 11. * shape * (0.15 + 0.85 * loud) * sway;
        div().w(px(2.)).h(px(height)).rounded_full().bg(color)
    }))
}
