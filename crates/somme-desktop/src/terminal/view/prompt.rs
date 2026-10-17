//! The command editor under the blocks: chips for the folder and git state,
//! and a text field with history, suggestions and Tab completion.

use std::collections::HashSet;
use std::ops::Range;

use gpui_kit::component::input::{
    Copy as InputCopy, Escape, IndentInline, InputEvent, MoveDown, MoveEnd, MoveRight, MoveUp, Search as InputSearch,
    Textarea, TextareaState,
};
use gpui_kit::component::{Icon, Size, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    Focusable as _, KeyBinding, MouseButton, ParentElement as _, Styled as _, Subscription, Window,
    actions, div, prelude::FluentBuilder as _, px, rems, transparent_black,
};

use super::picker::PickerKind;
use super::{TerminalView, TerminalViewEvent};
use crate::settings::Settings;
use crate::terminal::blocks::short_path;
use crate::terminal::colors::{self, to_hsla};
use crate::terminal::complete::{self, Completion, finished};
use crate::terminal::element::PADDING;
use crate::terminal::history::{self, Entry};
use crate::terminal::log::now_secs;
use crate::ui::common::{Lucide, ago};

actions!(terminal_prompt, [ClearLine, ClearBlocks, SendEof]);

const CONTEXT: &str = "TerminalPrompt";
/// The prompt also takes the terminal's own shortcuts, like Clear and Find.
const KEY_CONTEXT: &str = "Terminal TerminalPrompt";
/// Completion choices or past commands shown at once.
const MENU_ROWS: usize = 8;

pub(super) fn init(cx: &mut App) {
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-c", ClearLine, ctx),
        KeyBinding::new("ctrl-l", ClearBlocks, ctx),
        KeyBinding::new("ctrl-d", SendEof, ctx),
    ]);
}

/// Up and Down walking the history.
struct Browse {
    /// Matching commands, newest first.
    entries: Vec<Entry>,
    at: usize,
    /// What was typed before, put back after the newest entry.
    draft: String,
}

struct Menu {
    completion: Completion,
    selected: Option<usize>,
}

pub(super) struct Prompt {
    pub editor: Entity<TextareaState>,
    /// The user's zsh history file, oldest first.
    file_history: Vec<Entry>,
    /// Commands run from this editor, oldest first.
    ran: Vec<Entry>,
    browse: Option<Browse>,
    menu: Option<Menu>,
    /// The text the view itself put in, so that change is not taken as typing.
    expected: Option<String>,
    /// The rest of a past command that starts with what is typed, shown in grey,
    /// with the typed text it was found for.
    ghost: Option<(String, String)>,
    _subscription: Subscription,
}

impl Prompt {
    pub(super) fn new(window: &mut Window, cx: &mut Context<TerminalView>) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("Type a command")
        });
        let subscription = cx.subscribe_in(&editor, window, |view, editor, event, window, cx| match event {
            InputEvent::PressEnter { secondary: false, shift: false } => view.prompt_enter(window, cx),
            InputEvent::Change => {
                let value = editor.read(cx).value().to_string();
                if let Some(prompt) = view.prompt.as_mut()
                    && prompt.expected.take().as_deref() != Some(value.as_str())
                {
                    prompt.browse = None;
                    prompt.menu = None;
                }
                view.update_ghost(cx);
                cx.notify();
            }
            InputEvent::Focus => {
                // Clicking back into the editor leaves the folder picker.
                if view.picker.take().is_some() {
                    cx.notify();
                }
                cx.emit(TerminalViewEvent::Focused);
            }
            _ => {}
        });
        cx.spawn(async move |this, cx| {
            let loaded = cx.background_spawn(async { history::load_zsh_history() }).await;
            let _ = this.update(cx, |view, _| {
                if let Some(prompt) = view.prompt.as_mut() {
                    prompt.file_history = loaded;
                }
            });
        })
        .detach();
        Self {
            editor,
            file_history: Vec::new(),
            ran: Vec::new(),
            browse: None,
            menu: None,
            expected: None,
            ghost: None,
            _subscription: subscription,
        }
    }
}

impl TerminalView {
    pub(super) fn prompt_focused(&self, window: &Window, cx: &App) -> bool {
        self.prompt
            .as_ref()
            .is_some_and(|prompt| prompt.editor.read(cx).focus_handle(cx).is_focused(window))
    }

    pub(super) fn focus_prompt(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prompt) = &self.prompt {
            prompt.editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
    }

    /// Types text into the editor, as a paste or a key pressed over the blocks.
    pub(super) fn insert_in_prompt(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = &self.prompt else { return };
        let text = text.to_owned();
        prompt.editor.update(cx, |editor, cx| {
            editor.focus(window, cx);
            editor.insert(text, window, cx);
        });
    }

    fn set_prompt_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        prompt.expected = Some(text.to_owned());
        let text = text.to_owned();
        prompt.editor.update(cx, |editor, cx| {
            let end = text.len();
            editor.set_value(text, window, cx);
            editor.set_selected_range(end..end, cx);
        });
    }

    fn replace_in_prompt(&mut self, range: Range<usize>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        let mut value = prompt.editor.read(cx).value().to_string();
        let Some(range) = (range.end <= value.len()).then_some(range) else { return };
        value.replace_range(range.clone(), text);
        prompt.expected = Some(value);
        let text = text.to_owned();
        prompt.editor.update(cx, |editor, cx| {
            editor.set_selected_range(range, cx);
            editor.replace(text, window, cx);
        });
    }

    fn prompt_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self
            .prompt
            .as_mut()
            .and_then(|prompt| prompt.menu.take())
            .and_then(|menu| Some((menu.completion.range.clone(), menu.completion.choices.get(menu.selected?)?.clone())));
        if let Some((range, choice)) = picked {
            self.replace_in_prompt(range, &finished(&choice), window, cx);
            return;
        }
        self.submit_prompt(window, cx);
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        let text = prompt.editor.read(cx).value().to_string();
        if text.trim().is_empty() || !self.session.read(cx).at_prompt() {
            return;
        }
        prompt.browse = None;
        prompt.menu = None;
        prompt.ran.push(Entry {
            command: text.clone(),
            time: Some(now_secs()),
        });
        self.set_prompt_text("", window, cx);
        self.session.update(cx, |session, cx| {
            session.run_command(&text, cx);
        });
        self.blocks_scroll = px(0.);
        // Keys now go to the command, until the shell is back at its prompt.
        self.awaiting_block = true;
        window.focus(&self.focus_handle, cx);
        cx.emit(TerminalViewEvent::Ran(text));
    }

    /// Up on the first line opens the list of past commands; Up and Down then move
    /// through it. Returns false to let the editor move the cursor.
    fn history_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let session_history = self.session.read(cx).history.clone();
        let Some(prompt) = self.prompt.as_mut() else { return false };
        let (value, cursor) = {
            let editor = prompt.editor.read(cx);
            (editor.value().to_string(), editor.cursor().min(editor.value().len()))
        };
        let edge = if back { &value[..cursor] } else { &value[cursor..] };
        if prompt.browse.is_none() && edge.contains('\n') {
            return false;
        }
        let text = match (prompt.browse.as_mut(), back) {
            (None, false) => return false,
            (None, true) => {
                let entries = past_commands(&prompt.ran, &session_history, &prompt.file_history, &value);
                let Some(first) = entries.first().map(|entry| entry.command.clone()) else { return true };
                prompt.browse = Some(Browse {
                    entries,
                    at: 0,
                    draft: value,
                });
                first
            }
            (Some(browse), true) => {
                if browse.at + 1 >= browse.entries.len() {
                    return true;
                }
                browse.at += 1;
                browse.entries[browse.at].command.clone()
            }
            (Some(browse), false) => {
                if browse.at == 0 {
                    let draft = std::mem::take(&mut browse.draft);
                    prompt.browse = None;
                    draft
                } else {
                    browse.at -= 1;
                    browse.entries[browse.at].command.clone()
                }
            }
        };
        self.set_prompt_text(&text, window, cx);
        cx.notify();
        true
    }

    /// Closes the list of past commands and puts back what was typed.
    fn close_history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(browse) = self.prompt.as_mut().and_then(|prompt| prompt.browse.take()) else { return false };
        self.set_prompt_text(&browse.draft, window, cx);
        cx.notify();
        true
    }

    /// Puts a past command from the list in the editor.
    fn pick_history(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(browse) = self.prompt.as_mut().and_then(|prompt| prompt.browse.take()) else { return };
        if let Some(entry) = browse.entries.get(index) {
            self.set_prompt_text(&entry.command, window, cx);
        }
        self.focus_prompt(window, cx);
        cx.notify();
    }

    /// Finds the newest past command that starts with what is typed.
    fn update_ghost(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.as_ref() else { return };
        let value = prompt.editor.read(cx).value().to_string();
        let ghost = if prompt.browse.is_some() || value.trim().is_empty() || value.contains('\n') {
            None
        } else {
            let session = self.session.read(cx);
            prompt
                .ran
                .iter()
                .rev()
                .map(|entry| entry.command.as_str())
                .chain(session.history.iter().rev().map(String::as_str))
                .chain(prompt.file_history.iter().rev().map(|entry| entry.command.as_str()))
                .find(|command| {
                    command.len() > value.len() && command.starts_with(&value) && !command[value.len()..].contains('\n')
                })
                .map(|command| (value.clone(), command[value.len()..].to_owned()))
        };
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.ghost = ghost;
        }
    }

    /// The grey suggestion, while the cursor is at the end of the line.
    fn visible_ghost(&self, cx: &App) -> Option<String> {
        let prompt = self.prompt.as_ref()?;
        if prompt.browse.is_some() || prompt.menu.is_some() || self.picker.is_some() {
            return None;
        }
        let editor = prompt.editor.read(cx);
        let at_end = editor.selected_range().is_empty() && editor.cursor() >= editor.value().len();
        // Text the view puts in itself may not report a change, so an old one can linger.
        let (typed, rest) = prompt.ghost.as_ref()?;
        (at_end && *typed == editor.value().as_str()).then(|| rest.clone())
    }

    /// Right or End at the end of the line takes the grey suggestion.
    fn accept_ghost(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ghost) = self.visible_ghost(cx) else { return false };
        self.insert_in_prompt(&ghost, window, cx);
        true
    }

    fn complete_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.as_mut() else { return };
        if let Some(menu) = prompt.menu.as_mut() {
            let count = menu.completion.choices.len();
            menu.selected = Some(menu.selected.map_or(0, |i| (i + 1) % count));
            cx.notify();
            return;
        }
        let (line, cursor) = {
            let editor = prompt.editor.read(cx);
            (editor.value().to_string(), editor.cursor())
        };
        let (cwd, path_var) = {
            let blocks = self.session.read(cx).blocks.as_ref();
            (blocks.and_then(|b| b.cwd.clone()), blocks.and_then(|b| b.path.clone()))
        };
        let cwd = cwd.or_else(dirs::home_dir).unwrap_or_default();
        let Some(found) = complete::complete(&line, cursor, &cwd, path_var.as_deref()) else { return };
        if let [only] = found.choices.as_slice() {
            let text = finished(only);
            self.replace_in_prompt(found.range, &text, window, cx);
            return;
        }
        let mut range = found.range.clone();
        let common = found.common().unwrap_or_default();
        if common.len() > range.len() {
            self.replace_in_prompt(range.clone(), &common, window, cx);
            range = range.start..range.start + common.len();
        }
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.menu = Some(Menu {
                completion: Completion {
                    range,
                    choices: found.choices,
                },
                selected: None,
            });
        }
        cx.notify();
    }

    fn move_menu(&mut self, up: bool, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.prompt.as_mut().and_then(|prompt| prompt.menu.as_mut()) else { return false };
        let count = menu.completion.choices.len();
        menu.selected = Some(match (menu.selected, up) {
            (None, true) => count - 1,
            (None, false) => 0,
            (Some(i), true) => (i + count - 1) % count,
            (Some(i), false) => (i + 1) % count,
        });
        cx.notify();
        true
    }

    fn pick_choice(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.prompt.as_mut().and_then(|prompt| prompt.menu.take()) else { return };
        if let Some(choice) = menu.completion.choices.get(index) {
            self.replace_in_prompt(menu.completion.range, &finished(choice), window, cx);
        }
        self.focus_prompt(window, cx);
    }

    /// Copies the selected block text when the editor has nothing selected.
    fn prompt_copy(&mut self, _: &InputCopy, _: &mut Window, cx: &mut Context<Self>) {
        let editor_empty = self
            .prompt
            .as_ref()
            .is_none_or(|prompt| prompt.editor.read(cx).selected_range().is_empty());
        if editor_empty && let Some(text) = self.blocks_selection_text(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.stop_propagation();
        }
    }

    pub(super) fn render_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = Settings::get(cx).clone();
        let theme = colors::active_theme(cx);
        let foreground = to_hsla(theme.foreground);
        let muted = foreground.opacity(0.55);
        let green = to_hsla(theme.ansi[2]);
        let red = to_hsla(theme.ansi[1]);
        let session = self.session.read(cx);
        let blocks = session.blocks.as_ref();
        let ready = blocks.is_some_and(|b| b.ready);
        let running = blocks.is_some_and(|b| b.live().is_some());

        let chip = |icon: Lucide| {
            h_flex()
                .gap_1()
                .items_center()
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .bg(foreground.opacity(0.07))
                .text_xs()
                .text_color(muted)
                .child(Icon::new(icon).size(px(12.)))
        };
        let mut chips = h_flex().gap_1p5().items_center().flex_wrap();
        if let Some(cwd) = blocks.and_then(|b| b.cwd.as_deref()) {
            let open = self.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Folders);
            chips = chips.child(
                chip(Lucide::Folder)
                    .id("cwd-chip")
                    .cursor_pointer()
                    .when(open, |chip| chip.bg(foreground.opacity(0.14)).text_color(foreground))
                    .hover(|chip| chip.bg(foreground.opacity(0.12)).text_color(foreground))
                    .child(short_path(cwd))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, window, cx| {
                            cx.stop_propagation();
                            view.close_history(window, cx);
                            if let Some(prompt) = view.prompt.as_mut() {
                                prompt.menu = None;
                            }
                            view.toggle_picker(PickerKind::Folders, window, cx);
                        }),
                    ),
            );
        }
        if let Some(git) = blocks.and_then(|b| b.git.as_ref()) {
            let open = self.picker.as_ref().is_some_and(|p| p.kind == PickerKind::Branches);
            chips = chips.child(
                chip(Lucide::GitBranch)
                    .id("branch-chip")
                    .cursor_pointer()
                    .when(open, |chip| chip.bg(foreground.opacity(0.14)).text_color(foreground))
                    .hover(|chip| chip.bg(foreground.opacity(0.12)).text_color(foreground))
                    .child(git.branch.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, window, cx| {
                            cx.stop_propagation();
                            view.close_history(window, cx);
                            if let Some(prompt) = view.prompt.as_mut() {
                                prompt.menu = None;
                            }
                            view.toggle_picker(PickerKind::Branches, window, cx);
                        }),
                    ),
            );
            if git.files > 0 {
                chips = chips.child(
                    chip(Lucide::FileDiff)
                        .id("changes-chip")
                        .cursor_pointer()
                        .hover(|chip| chip.bg(foreground.opacity(0.12)).text_color(foreground))
                        .child(format!("{} •", git.files))
                        .child(div().text_color(green).child(format!("+{}", git.added)))
                        .child(div().text_color(red).child(format!("-{}", git.removed)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, _, cx| {
                                cx.stop_propagation();
                                let cwd = view.session.read(cx).blocks.as_ref().and_then(|b| b.cwd.clone());
                                if let Some(cwd) = cwd {
                                    cx.emit(TerminalViewEvent::OpenChanges(cwd));
                                }
                            }),
                        ),
                );
            }
        }

        let menu = self.prompt.as_ref().and_then(|prompt| prompt.menu.as_ref()).map(|menu| {
            let choices = &menu.completion.choices;
            let selected = menu.selected.unwrap_or(0);
            let start = selected.saturating_sub(MENU_ROWS - 1).min(choices.len().saturating_sub(MENU_ROWS));
            let rows = choices.iter().enumerate().skip(start).take(MENU_ROWS).map(|(i, choice)| {
                let active = menu.selected == Some(i);
                div()
                    .id(("completion", i))
                    .px_1p5()
                    .rounded_sm()
                    .when(active, |row| row.bg(foreground.opacity(0.12)))
                    .text_color(if choice.dir { to_hsla(theme.ansi[4]) } else { foreground })
                    .child(choice.label.clone())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            view.pick_choice(i, window, cx);
                            cx.stop_propagation();
                        }),
                    )
            });
            let more = choices.len().saturating_sub(MENU_ROWS);
            v_flex()
                .gap_0p5()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(foreground.opacity(0.12))
                .bg(foreground.opacity(0.04))
                .font_family(settings.font_family.clone())
                .text_size(px(settings.font_size.clamp(6., 72.) * 0.9))
                .children(rows)
                .when(more > 0, |list| {
                    list.child(div().px_1p5().text_xs().text_color(muted).child(format!(
                        "{} choices · Tab or ↑↓ to pick, Enter to use",
                        choices.len()
                    )))
                })
        });

        let history = self.prompt.as_ref().and_then(|prompt| prompt.browse.as_ref()).map(|browse| {
            // Oldest at the top and newest at the bottom, next to the editor.
            let bottom = browse.at.saturating_sub(MENU_ROWS - 1);
            let shown = bottom..(bottom + MENU_ROWS).min(browse.entries.len());
            let rows = shown.rev().map(|i| {
                let entry = &browse.entries[i];
                let mut lines = entry.command.lines();
                let first = lines.next().unwrap_or_default().to_owned();
                let label = if lines.next().is_some() { format!("{first} …") } else { first };
                h_flex()
                    .id(("history", i))
                    .gap_2()
                    .items_center()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .when(i == browse.at, |row| row.bg(foreground.opacity(0.12)))
                    .child(Icon::new(Lucide::SquareTerminal).size(px(14.)).text_color(muted))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when_some(entry.time, |row, time| {
                        row.child(div().flex_none().text_xs().text_color(muted).child(ago(time)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            view.pick_history(i, window, cx);
                            cx.stop_propagation();
                        }),
                    )
            });
            v_flex()
                .gap_0p5()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(foreground.opacity(0.12))
                .bg(foreground.opacity(0.04))
                .child(div().px_1p5().text_xs().text_color(muted).child("HISTORY"))
                .child(
                    v_flex()
                        .gap_0p5()
                        .font_family(settings.font_family.clone())
                        .text_size(px(settings.font_size.clamp(6., 72.) * 0.9))
                        .children(rows),
                )
                .child(
                    div()
                        .px_1p5()
                        .text_xs()
                        .text_color(muted)
                        .child("↑ ↓ to move · Enter to run · Esc to close"),
                )
        });

        let line: AnyElement = match &self.prompt {
            Some(prompt) if ready && !running => {
                let font_size = px(settings.font_size.clamp(6., 72.));
                let typed = prompt.editor.read(cx).value().to_string();
                // Only while the typed text fits on one line, so the grey part lines up after it.
                let fits = self.blocks_layout.as_ref().is_some_and(|layout| {
                    let room = layout.bounds.size.width - Size::Medium.input_px() * 2.;
                    layout.cell_width * (typed.chars().count() + 1) as f32 <= room
                });
                let ghost = self.visible_ghost(cx).filter(|_| fits);
                div()
                    .relative()
                    .w_full()
                    .child(
                        Textarea::new(&prompt.editor)
                            .appearance(false)
                            .font_family(settings.font_family.clone())
                            .text_size(font_size)
                            .text_color(foreground),
                    )
                    .when_some(ghost, |line, ghost| {
                        // Sits where the editor draws its text: the typed part is see-through.
                        line.child(
                            h_flex()
                                .absolute()
                                .top(Size::Medium.input_py())
                                .left(Size::Medium.input_px())
                                .right(Size::Medium.input_px())
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family(settings.font_family.clone())
                                .text_size(font_size)
                                .line_height(rems(1.25))
                                .child(div().flex_none().text_color(transparent_black()).child(typed))
                                .child(div().flex_none().text_color(muted).child(ghost)),
                        )
                    })
                    .into_any_element()
            }
            _ => {
                let hint = if running {
                    "Running · keys go to the program · Ctrl+C stops it"
                } else {
                    "Starting shell…"
                };
                div().py_0p5().text_sm().text_color(muted).child(hint).into_any_element()
            }
        };

        v_flex()
            .key_context(KEY_CONTEXT)
            .w_full()
            .flex_none()
            .gap_1p5()
            .px(PADDING)
            .pt_2()
            .pb_2()
            .border_t_1()
            .border_color(foreground.opacity(0.1))
            .bg(to_hsla(theme.background))
            .capture_action(cx.listener(|view, _: &MoveUp, window, cx| {
                if view.move_picker(true, cx) || view.move_menu(true, cx) || view.history_step(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|view, _: &MoveDown, window, cx| {
                if view.move_picker(false, cx) || view.move_menu(false, cx) || view.history_step(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|view, _: &IndentInline, window, cx| {
                if view.picker.is_none() {
                    view.complete_word(window, cx);
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|view, _: &Escape, window, cx| {
                if view.close_picker(window, cx) || view.close_history(window, cx) {
                    cx.stop_propagation();
                } else if let Some(prompt) = view.prompt.as_mut()
                    && prompt.menu.take().is_some()
                {
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|view, _: &MoveRight, window, cx| {
                if view.accept_ghost(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|view, _: &MoveEnd, window, cx| {
                if view.accept_ghost(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(Self::prompt_copy))
            // The editor takes Cmd+F for its own search; here it finds in the blocks.
            .capture_action(cx.listener(|view, _: &InputSearch, window, cx| {
                view.open_find(&super::Find, window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|view, _: &ClearLine, window, cx| {
                if let Some(prompt) = view.prompt.as_mut() {
                    prompt.browse = None;
                    prompt.menu = None;
                    prompt.ghost = None;
                }
                view.set_prompt_text("", window, cx);
            }))
            .on_action(cx.listener(|view, _: &ClearBlocks, _, cx| {
                view.session.update(cx, |session, cx| session.clear_blocks(cx));
            }))
            .on_action(cx.listener(|view, _: &SendEof, _, cx| {
                let empty = view.prompt.as_ref().is_none_or(|p| p.editor.read(cx).value().is_empty());
                if empty {
                    view.session.update(cx, |session, _| session.send(vec![0x04]));
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|view, _: &super::Clear, window, cx| view.clear(&super::Clear, window, cx)))
            .on_action(cx.listener(|view, _: &super::Find, window, cx| view.open_find(&super::Find, window, cx)))
            .on_action(cx.listener(|view, _: &super::ZoomIn, _, cx| view.zoom(1., cx)))
            .on_action(cx.listener(|view, _: &super::ZoomOut, _, cx| view.zoom(-1., cx)))
            .on_action(cx.listener(|view, _: &super::ZoomReset, _, cx| view.zoom(0., cx)))
            .children(self.render_picker(cx))
            .children(history)
            .children(menu)
            .child(chips)
            .child(line)
            .into_any_element()
    }
}

/// Past commands that start with `prefix`, newest first, each once.
fn past_commands(ran: &[Entry], session: &[String], file: &[Entry], prefix: &str) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let session = session.iter().rev().map(|command| Entry {
        command: command.clone(),
        time: None,
    });
    ran.iter()
        .rev()
        .cloned()
        .chain(session)
        .chain(file.iter().rev().cloned())
        .filter(|entry| entry.command.starts_with(prefix) && entry.command != prefix && seen.insert(entry.command.clone()))
        .collect()
}
