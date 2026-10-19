use std::ops::Range;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Direction, Point as GridPoint};
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::search::RegexSearch;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, ScrollWheelEvent, StatefulInteractiveElement as _, Styled as _,
    Subscription, UTF16Selection, Window, actions, div, prelude::FluentBuilder as _, px,
};

use crate::settings::{BellSetting, Settings};
use crate::terminal::blocks_element::{ACCEPTED_HIDE, BlocksElement, BlocksLayout};
use crate::terminal::element::{TerminalElement, grid_point};
use crate::terminal::mouse::{self, ReportButton, ReportKind};
use crate::terminal::session::{SessionEvent, Status, TerminalSession};
use crate::terminal::keys;
use crate::ui::common::logo;
use crate::ui::workspace::{SplitDown, SplitLeft, SplitRight, SplitUp};

mod blocks;
mod drop;
mod links;
mod picker;
mod prompt;
mod voice;

use prompt::Prompt;

actions!(
    terminal,
    [
        Copy,
        Paste,
        SelectAll,
        Clear,
        Find,
        FindNext,
        FindPrevious,
        CloseFind,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
        Reconnect,
        CopyCommand,
        CopyOutput,
        CopyLastCommand,
        CopyLastOutput,
        ToggleVoice,
    ]
);

const CONTEXT: &str = "Terminal";
const BLINK_INTERVAL: Duration = Duration::from_millis(530);
const BELL_FLASH: Duration = Duration::from_millis(150);

pub fn init(cx: &mut App) {
    let ctx = Some(CONTEXT);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, ctx),
        KeyBinding::new("cmd-v", Paste, ctx),
        KeyBinding::new("cmd-a", SelectAll, ctx),
        KeyBinding::new("cmd-k", Clear, ctx),
        KeyBinding::new("cmd-f", Find, ctx),
        KeyBinding::new("cmd-g", FindNext, ctx),
        KeyBinding::new("cmd-shift-g", FindPrevious, ctx),
        KeyBinding::new("cmd-=", ZoomIn, ctx),
        KeyBinding::new("cmd-+", ZoomIn, ctx),
        KeyBinding::new("cmd--", ZoomOut, ctx),
        KeyBinding::new("cmd-0", ZoomReset, ctx),
        KeyBinding::new("cmd-up", ScrollPageUp, ctx),
        KeyBinding::new("cmd-down", ScrollPageDown, ctx),
        KeyBinding::new("cmd-home", ScrollToTop, ctx),
        KeyBinding::new("cmd-end", ScrollToBottom, ctx),
        KeyBinding::new("shift-pageup", ScrollPageUp, ctx),
        KeyBinding::new("shift-pagedown", ScrollPageDown, ctx),
        KeyBinding::new("cmd-shift-space", ToggleVoice, ctx),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-c", Copy, ctx),
        KeyBinding::new("ctrl-shift-v", Paste, ctx),
        KeyBinding::new("ctrl-shift-a", SelectAll, ctx),
        KeyBinding::new("ctrl-shift-k", Clear, ctx),
        KeyBinding::new("ctrl-shift-f", Find, ctx),
        KeyBinding::new("f3", FindNext, ctx),
        KeyBinding::new("shift-f3", FindPrevious, ctx),
        KeyBinding::new("ctrl-=", ZoomIn, ctx),
        KeyBinding::new("ctrl-+", ZoomIn, ctx),
        KeyBinding::new("ctrl--", ZoomOut, ctx),
        KeyBinding::new("ctrl-0", ZoomReset, ctx),
        KeyBinding::new("shift-pageup", ScrollPageUp, ctx),
        KeyBinding::new("shift-pagedown", ScrollPageDown, ctx),
        KeyBinding::new("ctrl-shift-home", ScrollToTop, ctx),
        KeyBinding::new("ctrl-shift-end", ScrollToBottom, ctx),
        KeyBinding::new("ctrl-shift-space", ToggleVoice, ctx),
    ]);
    cx.bind_keys([KeyBinding::new("escape", CloseFind, Some("TerminalSearch"))]);
    prompt::init(cx);
}

/// Where the grid sits on screen, filled in each frame by the element.
#[derive(Clone, Copy, Debug)]
pub struct TermLayout {
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub columns: usize,
    pub lines: usize,
}

struct SearchMatch {
    /// The block it is in, when the view shows blocks.
    block: Option<usize>,
    start: GridPoint,
    end: GridPoint,
}

struct Search {
    input: Entity<InputState>,
    matches: Vec<SearchMatch>,
    active: Option<usize>,
    _subscription: Subscription,
}

/// What a terminal tells its tab: typed input (for broadcast) and focus.
pub enum TerminalViewEvent {
    Typed(Vec<u8>),
    Pasted(String),
    /// A command was run from the editor.
    Ran(String),
    Focused,
    TitleChanged,
    StatusChanged,
    /// The shell moved in or out of a GitHub repository.
    RepoChanged,
    /// A command started, or the shell is back at its prompt.
    PhaseChanged,
    /// A file was picked to open in the editor.
    OpenFile(std::path::PathBuf),
    /// Show the uncommitted git changes of the repo that holds this folder.
    OpenChanges(std::path::PathBuf),
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

pub struct TerminalView {
    pub session: Entity<TerminalSession>,
    pub focus_handle: FocusHandle,
    pub layout: Option<TermLayout>,
    selecting: bool,
    reported_button: Option<ReportButton>,
    scroll_remainder: f32,
    search: Option<Search>,
    cursor_visible: bool,
    blink_generation: usize,
    bell_until: Option<Instant>,
    /// The blocks as drawn last frame, and how far they are scrolled up from the bottom.
    pub blocks_layout: Option<BlocksLayout>,
    pub blocks_scroll: Pixels,
    pub blocks_total: Pixels,
    selecting_block: Option<usize>,
    /// The block right-clicked last, for the menu's copy items.
    menu_block: Option<usize>,
    /// The file or folder name under the mouse in a block's output.
    link: Option<links::PathLink>,
    link_probe: Option<links::Probe>,
    /// The link pressed, opened if the mouse comes up on it without a drag.
    link_press: Option<links::PathLink>,
    prompt: Option<Prompt>,
    /// The folder picker over the command editor.
    picker: Option<picker::Picker>,
    /// Text pasted from outside the window, put in the editor at the next frame.
    pending_insert: Option<String>,
    /// A command was sent and the shell has not started it yet.
    awaiting_block: bool,
    /// Uploads of files dropped on a server's shell.
    notes: Vec<drop::Note>,
    next_note: usize,
    /// The offer to download the speech model is open.
    voice_offer: bool,
    /// Says voice typing is listening, where the command box isn't shown.
    voice_note: Option<usize>,
    /// Space pressed in the command box, which may become holding to talk.
    space: Option<voice::SpaceHold>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(session: Entity<TerminalSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe_in(&session, window, |view, _, event, window, cx| match event {
                SessionEvent::Bell => view.ring_bell(window, cx),
                SessionEvent::TitleChanged => cx.emit(TerminalViewEvent::TitleChanged),
                SessionEvent::StatusChanged => {
                    view.load_server_history(cx);
                    cx.emit(TerminalViewEvent::StatusChanged);
                }
                SessionEvent::PhaseChanged => view.phase_changed(window, cx),
                SessionEvent::RepoChanged => cx.emit(TerminalViewEvent::RepoChanged),
            }),
            cx.on_focus(&focus_handle, window, |view, window, cx| {
                view.reset_blink(cx);
                if view.wants_prompt(cx) {
                    view.focus_prompt(window, cx);
                }
                cx.emit(TerminalViewEvent::Focused);
            }),
            cx.observe(&crate::voice::Voice::global(cx), |_, _, cx| cx.notify()),
            // A closed tab stops listening for it.
            cx.on_release({
                let me = cx.entity_id();
                move |_, cx| {
                    let voice = crate::voice::Voice::global(cx);
                    voice.update(cx, |voice, cx| voice.release(me, cx));
                }
            }),
        ];
        let (has_blocks, local) = (session.read(cx).blocks.is_some(), session.read(cx).is_local());
        let prompt = has_blocks.then(|| Prompt::new(local, window, cx));
        let mut view = Self {
            session,
            focus_handle,
            layout: None,
            selecting: false,
            reported_button: None,
            scroll_remainder: 0.,
            search: None,
            cursor_visible: true,
            blink_generation: 0,
            bell_until: None,
            blocks_layout: None,
            blocks_scroll: px(0.),
            blocks_total: px(0.),
            selecting_block: None,
            menu_block: None,
            link: None,
            link_probe: None,
            link_press: None,
            prompt,
            picker: None,
            pending_insert: None,
            awaiting_block: false,
            notes: Vec::new(),
            next_note: 0,
            voice_offer: false,
            voice_note: None,
            space: None,
            _subscriptions: subscriptions,
        };
        view.reset_blink(cx);
        view
    }

    /// Whether keys belong in the command editor rather than the shell.
    fn wants_prompt(&self, cx: &App) -> bool {
        let session = self.session.read(cx);
        !self.awaiting_block && session.at_prompt() && session.shows_blocks()
    }

    /// The shell started a command or came back to its prompt: move focus to match.
    fn phase_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.awaiting_block = false;
        if self.focus_handle.is_focused(window) || self.prompt_focused(window, cx) {
            if self.wants_prompt(cx) {
                self.focus_prompt(window, cx);
            } else if !self.focus_handle.is_focused(window) {
                window.focus(&self.focus_handle, cx);
            }
        }
        // Accepted lines show their output after a moment; draw again then.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ACCEPTED_HIDE + Duration::from_millis(10)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
        cx.emit(TerminalViewEvent::PhaseChanged);
        cx.notify();
    }

    fn ring_bell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match Settings::get(cx).bell {
            BellSetting::Off => {}
            BellSetting::Sound => window.play_system_bell(),
            BellSetting::Visual => {
                self.bell_until = Some(Instant::now() + BELL_FLASH);
                cx.notify();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(BELL_FLASH).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
    }

    /// Shows the cursor and restarts blinking, so it stays visible while typing.
    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        self.blink_generation += 1;
        let generation = self.blink_generation;
        cx.notify();
        if !Settings::get(cx).cursor_blink {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let keep_going = this.update(cx, |view, cx| {
                    if view.blink_generation != generation {
                        return false;
                    }
                    view.cursor_visible = !view.cursor_visible;
                    cx.notify();
                    true
                });
                if !matches!(keep_going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn write(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| session.input(bytes.clone(), cx));
        self.reset_blink(cx);
        cx.emit(TerminalViewEvent::Typed(bytes));
    }

    /// Types text into the terminal as if pasted, e.g. from a snippet.
    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.wants_prompt(cx) && self.prompt.is_some() {
            self.pending_insert = Some(text.to_owned());
            cx.notify();
        } else {
            self.session.update(cx, |session, cx| session.paste(text, cx));
        }
        cx.emit(TerminalViewEvent::Pasted(text.to_owned()));
    }

    /// Sends text followed by Enter, used to run snippets.
    pub fn run_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.session.read(cx).at_prompt() {
            let text = text.trim_end_matches(['\r', '\n']).to_owned();
            self.session.update(cx, |session, cx| {
                session.run_command(&text, cx);
            });
            self.blocks_scroll = px(0.);
            cx.emit(TerminalViewEvent::Ran(text));
            return;
        }
        let mut bytes = text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
        if bytes.last() != Some(&b'\r') {
            bytes.push(b'\r');
        }
        self.write(bytes, cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The find box sits inside the terminal, so its keys pass through here too.
        if self.find_focused(window, cx) {
            return;
        }
        if self.wants_prompt(cx) {
            // Typing over the blocks goes to the editor.
            self.focus_prompt(window, cx);
            return;
        }
        let mode = self.session.read(cx).mode();
        let option_as_meta = Settings::get(cx).option_as_meta;
        if let Some(bytes) = keys::to_esc_str(&event.keystroke, &mode, option_as_meta) {
            self.write(bytes.into_owned().into_bytes(), cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = if self.session.read(cx).shows_blocks() {
            self.blocks_selection_text(cx)
        } else {
            self.session.read(cx).selection_text()
        };
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.paste_text(&text, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).shows_blocks() {
            self.blocks_select_all(cx);
        } else {
            self.session.update(cx, |session, cx| session.select_all(cx));
        }
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| session.clear_scrollback(cx));
    }

    fn zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        Settings::update(cx, |settings| {
            settings.font_size = if delta == 0. {
                Settings::default().font_size
            } else {
                (settings.font_size + delta).clamp(8., 40.)
            };
        });
    }

    fn scroll_lines(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        if self.session.read(cx).shows_blocks() {
            self.blocks_scroll_to(scroll, cx);
        } else {
            self.session.update(cx, |session, cx| session.scroll_to(scroll, cx));
        }
    }

    fn reconnect(&mut self, _: &Reconnect, window: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            if !session.is_running() && !session.is_replay() {
                session.connect(cx);
            }
        });
        window.focus(&self.focus_handle, cx);
    }

    // ---- mouse ----

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).shows_blocks() {
            self.blocks_mouse_down(event, window, cx);
            return;
        }
        window.focus(&self.focus_handle, cx);
        let Some(layout) = self.layout else { return };
        let mode = self.session.read(cx).mode();
        let button = match event.button {
            MouseButton::Left => ReportButton::Left,
            MouseButton::Middle => ReportButton::Middle,
            MouseButton::Right => ReportButton::Right,
            _ => return,
        };
        if !event.modifiers.shift && mode.intersects(TermMode::MOUSE_MODE) {
            if let Some(bytes) = self.report(event.position, ReportKind::Press, button, &event.modifiers, cx) {
                self.reported_button = Some(button);
                self.session.update(cx, |session, _| session.send(bytes));
                return;
            }
        }
        if event.button != MouseButton::Left {
            return;
        }
        let offset = self.session.read(cx).term().grid().display_offset();
        let (point, side) = grid_point(&layout, event.position, offset);
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.session.update(cx, |session, cx| {
            if event.modifiers.shift && session.term().selection.is_some() {
                session.update_selection(point, side);
            } else {
                session.start_selection(kind, point, side);
            }
            cx.notify();
        });
        self.selecting = true;
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).shows_blocks() {
            self.blocks_mouse_move(event, cx);
            return;
        }
        let Some(layout) = self.layout else { return };
        if let Some(button) = self.reported_button {
            if let Some(bytes) = self.report(event.position, ReportKind::Motion, button, &event.modifiers, cx) {
                self.session.update(cx, |session, _| session.send(bytes));
            }
            return;
        }
        let mode = self.session.read(cx).mode();
        if event.pressed_button.is_none() && mode.contains(TermMode::MOUSE_MOTION) {
            if let Some(bytes) = self.report(event.position, ReportKind::Motion, ReportButton::NoButton, &event.modifiers, cx) {
                self.session.update(cx, |session, _| session.send(bytes));
            }
            return;
        }
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        // Dragging past the top or bottom edge scrolls the history.
        let top = layout.origin.y;
        let bottom = layout.origin.y + layout.line_height * layout.lines as f32;
        if event.position.y < top {
            self.session.update(cx, |session, cx| session.scroll(1, cx));
        } else if event.position.y > bottom {
            self.session.update(cx, |session, cx| session.scroll(-1, cx));
        }
        let offset = self.session.read(cx).term().grid().display_offset();
        let (point, side) = grid_point(&layout, event.position, offset);
        self.session.update(cx, |session, cx| {
            session.update_selection(point, side);
            cx.notify();
        });
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting_block.is_some() {
            self.blocks_mouse_up(event, window, cx);
            return;
        }
        if let Some(button) = self.reported_button.take() {
            if let Some(bytes) = self.report(event.position, ReportKind::Release, button, &event.modifiers, cx) {
                self.session.update(cx, |session, _| session.send(bytes));
            }
            return;
        }
        if !self.selecting {
            return;
        }
        self.selecting = false;
        let session = self.session.read(cx);
        let empty = session.term().selection.as_ref().is_none_or(|s| s.is_empty());
        if empty {
            self.session.update(cx, |session, cx| {
                session.clear_selection();
                cx.notify();
            });
        } else if Settings::get(cx).copy_on_select
            && let Some(text) = session.selection_text()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).shows_blocks() {
            self.blocks_scroll_wheel(event, cx);
            return;
        }
        let Some(layout) = self.layout else { return };
        let delta = event.delta.pixel_delta(layout.line_height).y;
        self.scroll_remainder += delta / layout.line_height;
        let lines = self.scroll_remainder.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;
        let mode = self.session.read(cx).mode();

        if mode.intersects(TermMode::MOUSE_MODE) && !event.modifiers.shift {
            let button = if lines > 0 { ReportButton::WheelUp } else { ReportButton::WheelDown };
            for _ in 0..lines.unsigned_abs().min(10) {
                if let Some(bytes) = self.report(event.position, ReportKind::Press, button, &event.modifiers, cx) {
                    self.session.update(cx, |session, _| session.send(bytes));
                }
            }
            return;
        }
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) && !event.modifiers.shift {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let key = match (lines > 0, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            let bytes = key.repeat(lines.unsigned_abs() as usize).into_bytes();
            self.session.update(cx, |session, _| session.send(bytes));
            return;
        }
        self.session.update(cx, |session, cx| session.scroll(lines, cx));
    }

    fn report(
        &self,
        position: Point<Pixels>,
        kind: ReportKind,
        button: ReportButton,
        modifiers: &gpui_kit::Modifiers,
        cx: &App,
    ) -> Option<Vec<u8>> {
        let layout = self.layout?;
        let mode = self.session.read(cx).mode();
        let (point, _) = grid_point(&layout, position, 0);
        mouse::encode(&mode, kind, button, modifiers, point.column.0, point.line.0.max(0) as usize)
    }

    // ---- search ----

    fn open_find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(search) = &self.search {
            search.input.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let subscription = cx.subscribe_in(&input, window, |view, input, event, window, cx| match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                view.run_search(&query, cx);
            }
            InputEvent::PressEnter { shift, .. } => {
                if *shift {
                    view.step_search(false, cx);
                } else {
                    view.step_search(true, cx);
                }
                let _ = window;
            }
            _ => {}
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.search = Some(Search {
            input,
            matches: Vec::new(),
            active: None,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn find_focused(&self, window: &Window, cx: &App) -> bool {
        self.search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).focus_handle(cx).is_focused(window))
    }

    fn close_find(&mut self, _: &CloseFind, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn run_search(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.search.is_none() {
            return;
        }
        let regex = (!query.is_empty())
            .then(|| RegexSearch::new(&format!("(?i){}", regex_escape(query))).ok())
            .flatten();
        let matches = match regex {
            None => Vec::new(),
            Some(mut regex) if self.session.read(cx).shows_blocks() => self.search_blocks(&mut regex, cx),
            Some(mut regex) => search_screen(self.session.read(cx).term(), &mut regex),
        };
        let Some(search) = self.search.as_mut() else { return };
        search.active = matches.len().checked_sub(1);
        search.matches = matches;
        if search.active.is_some() {
            self.reveal_active(cx);
        }
        cx.notify();
    }

    fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut() else { return };
        if search.matches.is_empty() {
            return;
        }
        let count = search.matches.len();
        search.active = Some(match (search.active, forward) {
            (Some(i), true) => (i + 1) % count,
            (Some(i), false) => (i + count - 1) % count,
            (None, _) => count - 1,
        });
        self.reveal_active(cx);
        cx.notify();
    }

    fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let Some((block, start)) = self
            .search
            .as_ref()
            .and_then(|s| s.active.and_then(|i| s.matches.get(i)).map(|m| (m.block, m.start)))
        else {
            return;
        };
        if let Some(block) = block {
            self.reveal_block_line(block, start.line, cx);
            return;
        }
        self.session.update(cx, |session, cx| {
            session.term_mut().scroll_to_point(start);
            cx.notify();
        });
    }

    fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step_search(true, cx);
    }

    fn find_previous(&mut self, _: &FindPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.step_search(false, cx);
    }

    /// Matches to highlight, as (start, end, is_active).
    pub fn search_matches(&self) -> Vec<(GridPoint, GridPoint, bool)> {
        let Some(search) = &self.search else { return Vec::new() };
        search
            .matches
            .iter()
            .enumerate()
            .filter(|(_, m)| m.block.is_none())
            .map(|(i, m)| (m.start, m.end, search.active == Some(i)))
            .collect()
    }

    /// Matches inside blocks, as (block, start, end, is_active).
    pub fn block_search_matches(&self) -> Vec<(usize, GridPoint, GridPoint, bool)> {
        let Some(search) = &self.search else { return Vec::new() };
        search
            .matches
            .iter()
            .enumerate()
            .filter_map(|(i, m)| Some((m.block?, m.start, m.end, search.active == Some(i))))
            .collect()
    }

    fn render_search(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let search = self.search.as_ref()?;
        let total = search.matches.len();
        let position = search.active.map(|i| i + 1).unwrap_or(0);
        let theme = cx.theme();
        Some(
            h_flex()
                .key_context("TerminalSearch")
                .on_action(cx.listener(Self::close_find))
                // A click here must not reach the terminal behind, which takes focus.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .absolute()
                .top_2()
                .right_4()
                .w(px(320.))
                .gap_1()
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.popover)
                .shadow_lg()
                .child(div().flex_1().child(Input::new(&search.input).small().cleanable(true)))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .min_w(px(44.))
                        .child(format!("{position}/{total}")),
                )
                .child(
                    Button::new("find-prev")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronUp)
                        .on_click(cx.listener(|view, _, _, cx| view.step_search(false, cx))),
                )
                .child(
                    Button::new("find-next")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronDown)
                        .on_click(cx.listener(|view, _, _, cx| view.step_search(true, cx))),
                )
                .child(
                    Button::new("find-close")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .on_click(cx.listener(|view, _, window, cx| view.close_find(&CloseFind, window, cx))),
                ),
        )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let session = self.session.read(cx);
        let theme = cx.theme();
        let label = session.host_label();
        let (title, detail, busy) = match &session.status {
            Status::Connected => return None,
            Status::Closed(_) if session.is_replay() => return None,
            Status::Connecting => {
                // Local shells start instantly; only show the overlay for slow connections.
                if session.started_at.elapsed() < Duration::from_millis(250) {
                    return None;
                }
                (format!("Connecting to {label}…"), None, true)
            }
            Status::Failed(reason) => (format!("Could not connect to {label}"), Some(reason.clone()), false),
            Status::Closed(reason) => (
                "Session closed".to_owned(),
                Some(reason.clone().unwrap_or_else(|| format!("The connection to {label} ended."))),
                false,
            ),
        };
        let closed_banner = matches!(session.status, Status::Closed(_));
        let card = v_flex()
            .gap_3()
            .p_5()
            .w(px(380.))
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_xl()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(busy, |row| row.child(Spinner::new().small()))
                    .when(!busy, |row| {
                        row.child(Icon::new(IconName::TriangleAlert).text_color(if closed_banner {
                            theme.muted_foreground
                        } else {
                            theme.danger
                        }))
                    })
                    .child(div().font_semibold().child(title)),
            )
            .when_some(detail, |card, detail| {
                card.child(div().text_sm().text_color(theme.muted_foreground).child(detail))
            })
            .when(!busy, |card| {
                card.child(
                    h_flex().justify_end().child(
                        Button::new("reconnect")
                            .primary()
                            .small()
                            .label("Reconnect")
                            .on_click(cx.listener(|view, _, window, cx| view.reconnect(&Reconnect, window, cx))),
                    ),
                )
            });
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .when(!closed_banner, |overlay| overlay.bg(theme.background.opacity(0.6)))
                .child(card),
        )
    }
}

/// Every match on the screen and in its history, up to a limit.
fn search_screen(term: &alacritty_terminal::Term<crate::terminal::session::Listener>, regex: &mut RegexSearch) -> Vec<SearchMatch> {
    let mut matches = Vec::new();
    let mut origin = GridPoint::new(term.topmost_line(), Column(0));
    let end = GridPoint::new(term.bottommost_line(), term.last_column());
    while let Some(found) = term.regex_search_right(regex, origin, end) {
        let (start, stop) = (*found.start(), *found.end());
        matches.push(SearchMatch {
            block: None,
            start,
            end: stop,
        });
        if matches.len() >= 2000 {
            break;
        }
        let next = stop.add(term, alacritty_terminal::index::Boundary::None, 1);
        if next <= stop {
            break;
        }
        origin = next;
    }
    matches
}

fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(text) = self.pending_insert.take() {
            self.insert_in_prompt(&text, window, cx);
        }
        let blocks_mode = self.session.read(cx).shows_blocks();
        let focused = self.focus_handle.is_focused(window);
        let flashing = self.bell_until.is_some_and(|until| Instant::now() < until);
        let has_selection = if blocks_mode {
            self.blocks_selection_text(cx).is_some()
        } else {
            self.session.read(cx).term().selection.is_some()
        };
        let paste_on_right_click = Settings::get(cx).paste_on_right_click;
        // A shell with nothing run yet shows the app's logo instead of a blank panel.
        let empty = blocks_mode
            && self.session.read(cx).blocks.as_ref().is_some_and(|blocks| blocks.ready && blocks.list.is_empty());
        let element = if blocks_mode {
            BlocksElement::new(cx.entity(), self.session.clone(), focused, self.cursor_visible).into_any_element()
        } else {
            self.blocks_layout = None;
            TerminalElement::new(cx.entity(), self.session.clone(), focused, self.cursor_visible).into_any_element()
        };

        let surface = div()
            .id("terminal")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .when(blocks_mode, |surface| surface.flex_1().min_h_0().w_full())
            .when(!blocks_mode, |surface| surface.size_full())
            .relative()
            .when(self.link.is_some(), |surface| surface.cursor_pointer())
            .when(self.link.is_none(), |surface| surface.cursor_text())
            .on_hover(cx.listener(|view, hovered: &bool, _, cx| {
                if !*hovered {
                    view.clear_link(cx);
                }
            }))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .on_action(cx.listener(Self::reconnect))
            .on_action(cx.listener(|view, _: &CopyCommand, _, cx| view.copy_block(view.menu_block, false, cx)))
            .on_action(cx.listener(|view, _: &CopyOutput, _, cx| view.copy_block(view.menu_block, true, cx)))
            .on_action(cx.listener(|view, _: &CopyLastCommand, _, cx| view.copy_block(None, false, cx)))
            .on_action(cx.listener(|view, _: &CopyLastOutput, _, cx| view.copy_block(None, true, cx)))
            .on_action(cx.listener(|view, _: &ZoomIn, _, cx| view.zoom(1., cx)))
            .on_action(cx.listener(|view, _: &ZoomOut, _, cx| view.zoom(-1., cx)))
            .on_action(cx.listener(|view, _: &ZoomReset, _, cx| view.zoom(0., cx)))
            .on_action(cx.listener(|view, _: &ScrollPageUp, _, cx| view.scroll_lines(Scroll::PageUp, cx)))
            .on_action(cx.listener(|view, _: &ScrollPageDown, _, cx| view.scroll_lines(Scroll::PageDown, cx)))
            .on_action(cx.listener(|view, _: &ScrollToTop, _, cx| view.scroll_lines(Scroll::Top, cx)))
            .on_action(cx.listener(|view, _: &ScrollToBottom, _, cx| view.scroll_lines(Scroll::Bottom, cx)))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    // With a command box, only a right-click on the box pastes; the
                    // rest of the panel shows the menu.
                    if blocks_mode {
                        view.menu_block = view.blocks_layout.as_ref().and_then(|layout| layout.block_at(event.position.y));
                        return;
                    }
                    let mode = view.session.read(cx).mode();
                    if mode.intersects(TermMode::MOUSE_MODE) && !event.modifiers.shift {
                        view.mouse_down(event, window, cx);
                        cx.stop_propagation();
                    } else if paste_on_right_click {
                        view.paste(&Paste, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(element)
            .when(empty, |surface| {
                surface.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(logo(px(112.)).opacity(0.9)),
                )
            })
            .when(flashing, |surface| {
                surface.child(div().absolute().inset_0().border_2().border_color(cx.theme().warning.opacity(0.7)))
            })
            .children(self.render_search(cx))
            .children(self.render_notes(cx))
            .children(self.render_status(cx));

        let focus = self.focus_handle.clone();
        let view = cx.entity().downgrade();
        let surface = surface.context_menu(move |menu, _, cx| {
            let on_block = blocks_mode && view.upgrade().is_some_and(|view| view.read(cx).menu_block.is_some());
            menu.action_context(focus.clone())
                .menu_with_disabled("Copy", Box::new(Copy), !has_selection)
                .menu("Paste", Box::new(Paste))
                .when(on_block, |menu| {
                    menu.separator()
                        .menu("Copy Command", Box::new(CopyCommand))
                        .menu("Copy Output", Box::new(CopyOutput))
                })
                .separator()
                .menu("Select All", Box::new(SelectAll))
                .menu("Find…", Box::new(Find))
                .separator()
                .menu("Split Right", Box::new(SplitRight))
                .menu("Split Left", Box::new(SplitLeft))
                .menu("Split Down", Box::new(SplitDown))
                .menu("Split Up", Box::new(SplitUp))
                .separator()
                .menu("Clear", Box::new(Clear))
        });
        let body = if blocks_mode {
            v_flex()
                .size_full()
                .child(surface)
                .child(self.render_prompt(cx))
                .into_any_element()
        } else {
            surface.into_any_element()
        };
        div()
            .size_full()
            .relative()
            .on_action(cx.listener(Self::toggle_voice))
            .child(body)
            .children(self.render_drop_target(cx))
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !text.is_empty() && self.wants_prompt(cx) {
            self.insert_in_prompt(text, window, cx);
            return;
        }
        if !text.is_empty() {
            self.write(text.as_bytes().to_vec(), cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Puts the input method's candidate window next to the cursor.
        if let Some(blocks) = &self.blocks_layout {
            let session = self.session.read(cx);
            let index = session.blocks.as_ref()?.list.len().checked_sub(1)?;
            let geom = blocks.blocks.get(index)?;
            let cursor = session.term().grid().cursor.point;
            let origin = gpui_kit::point(
                geom.output_origin.x + blocks.cell_width * cursor.column.0 as f32,
                geom.output_origin.y + blocks.line_height * (cursor.line.0 - geom.first_line.0) as f32,
            );
            return Some(Bounds::new(origin, gpui_kit::size(blocks.cell_width, blocks.line_height)));
        }
        let layout = self.layout?;
        let session = self.session.read(cx);
        let cursor = session.term().grid().cursor.point;
        let offset = session.term().grid().display_offset() as i32;
        let origin = gpui_kit::point(
            layout.origin.x + layout.cell_width * cursor.column.0 as f32,
            layout.origin.y + layout.line_height * (cursor.line.0 + offset) as f32,
        );
        let _ = element_bounds;
        Some(Bounds::new(origin, gpui_kit::size(layout.cell_width, layout.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

// Kept so `Direction` stays referenced for future word-motion helpers.
#[allow(dead_code)]
const _: Option<Direction> = None;
