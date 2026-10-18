//! A plain text editor for files too big for the code editor, like logs of a
//! gigabyte. Only the lines on screen are laid out, and the scroll position
//! counts lines instead of pixels, so it stays smooth however long the file is.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Copy, Cut, Escape, Input, InputEvent, InputState, Paste, Rope, SelectAll};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels, Point, Render,
    ScrollWheelEvent, ShapedLine, SharedString, Styled as _, Subscription, Task, TextAlign, TextRun, UTF16Selection,
    Window, canvas, div, fill, point, prelude::FluentBuilder as _, px, size,
};
use regex::bytes::{Regex, RegexBuilder};
use ropey::LineType;

use crate::editor::tools::Tool;
use crate::settings::Settings;
use crate::terminal::element::{search_color, terminal_font};

const LF: LineType = LineType::LF;
/// Columns a tab takes.
const TAB: usize = 4;
/// Longer lines are laid out only around the part on screen.
const LONG_LINE: usize = 4096;
/// Space between the line numbers and the text.
const GAP: Pixels = px(16.);
const SCROLLBAR: Pixels = px(12.);

pub enum BigTextEvent {
    Changed,
    /// The find bar's jq button was clicked.
    Jq,
}

/// One change that can be undone: `removed` was at `at`, and `inserted` took its place.
struct Edit {
    at: usize,
    removed: String,
    inserted: String,
    /// Cursor and anchor before the change.
    before: (usize, usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BarMode {
    Find,
    GoToLine,
}

/// The bar on top, for finding text or going to a line.
struct Bar {
    mode: BarMode,
    input: Entity<InputState>,
    /// What was last searched for. Enter also sends a change, with the same text.
    searched: Option<String>,
    /// How many matches the whole text has, once counted.
    count: Option<usize>,
    /// The last search found nothing.
    missed: bool,
    /// Tells searches off the UI thread to stop, once the query changes.
    cancel: Arc<AtomicBool>,
    _tasks: Vec<Task<()>>,
    _subscription: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Select,
    Scrollbar,
}

/// Sizes from the last frame, for turning mouse positions into text positions.
#[derive(Clone)]
struct Metrics {
    bounds: Bounds<Pixels>,
    /// Where column zero of the text is drawn, before scrolling sideways.
    text_x: Pixels,
    line_height: Pixels,
    cell_width: Pixels,
    font: Font,
    font_size: Pixels,
    /// How far the widest line on screen lets the text scroll sideways.
    max_x: Pixels,
}

impl Metrics {
    fn rows(&self) -> f64 {
        f64::from(self.bounds.size.height / self.line_height)
    }

    fn text_width(&self) -> Pixels {
        (self.bounds.right() - SCROLLBAR - self.text_x).max(px(0.))
    }

    fn thumb_height(&self, lines: usize) -> Pixels {
        let share = (self.rows() / lines.max(1) as f64).min(1.) as f32;
        (self.bounds.size.height * share).max(px(24.))
    }
}

pub struct BigText {
    text: Rope,
    focus_handle: FocusHandle,
    /// The line at the top of the screen. The fraction is how far into it the view is scrolled.
    scroll: f64,
    scroll_x: Pixels,
    /// Where typing goes, as a byte offset.
    cursor: usize,
    /// The other end of the selection. Equal to `cursor` when nothing is selected.
    anchor: usize,
    /// The column that up and down keep to.
    goal: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// The last edit was typing, so more typing joins it for undo.
    typing: bool,
    bar: Option<Bar>,
    /// The find bar offers jq, for JSON files.
    jq: bool,
    metrics: Option<Metrics>,
    drag: Drag,
}

impl EventEmitter<BigTextEvent> for BigText {}

impl BigText {
    pub fn new(text: Rope, cx: &mut Context<Self>) -> Self {
        Self {
            text,
            focus_handle: cx.focus_handle(),
            scroll: 0.,
            scroll_x: px(0.),
            cursor: 0,
            anchor: 0,
            goal: None,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            bar: None,
            jq: false,
            metrics: None,
            drag: Drag::None,
        }
    }

    /// Offers jq in the find bar.
    pub fn with_jq(mut self) -> Self {
        self.jq = true;
        self
    }

    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_bar(BarMode::Find, window, cx);
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// The cursor's line and column, counting from zero.
    pub fn cursor_position(&self) -> (usize, usize) {
        let line = self.line_of(self.cursor);
        let start = self.line_start(line);
        (line, self.text.slice(start..self.cursor).len_chars())
    }

    // ---- positions ----

    fn lines(&self) -> usize {
        self.text.len_lines(LF)
    }

    fn line_of(&self, offset: usize) -> usize {
        self.text.byte_to_line_idx(offset, LF)
    }

    fn line_start(&self, line: usize) -> usize {
        self.text.line_to_byte_idx(line, LF)
    }

    fn line_end(&self, line: usize) -> usize {
        line_end(&self.text, line)
    }

    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn prev_char(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let mut prev = offset - 1;
        while prev > 0 && !self.text.is_char_boundary(prev) {
            prev -= 1;
        }
        // A Windows line break is one step.
        if prev > 0 && self.text.byte(prev) == b'\n' && self.text.byte(prev - 1) == b'\r' {
            prev -= 1;
        }
        prev
    }

    fn next_char(&self, offset: usize) -> usize {
        let len = self.text.len();
        if offset >= len {
            return len;
        }
        if self.text.byte(offset) == b'\r' && offset + 1 < len && self.text.byte(offset + 1) == b'\n' {
            return offset + 2;
        }
        let mut next = offset + 1;
        while next < len && !self.text.is_char_boundary(next) {
            next += 1;
        }
        next
    }

    /// The start of the word before `offset`, on the same line.
    fn word_left(&self, offset: usize) -> usize {
        let start = self.line_start(self.line_of(offset));
        if offset == start {
            return self.prev_char(offset);
        }
        let before = self.text.slice(start..offset).to_string();
        let chars: Vec<(usize, char)> = before.char_indices().collect();
        let mut ix = chars.len();
        while ix > 0 && chars[ix - 1].1.is_whitespace() {
            ix -= 1;
        }
        if ix > 0 {
            let kind = word_kind(chars[ix - 1].1);
            while ix > 0 && !chars[ix - 1].1.is_whitespace() && word_kind(chars[ix - 1].1) == kind {
                ix -= 1;
            }
        }
        start + chars.get(ix).map_or(before.len(), |(at, _)| *at)
    }

    /// The end of the word after `offset`, on the same line.
    fn word_right(&self, offset: usize) -> usize {
        let end = self.line_end(self.line_of(offset));
        if offset >= end {
            return self.next_char(offset);
        }
        let after = self.text.slice(offset..end).to_string();
        let mut chars = after.char_indices().peekable();
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        if let Some(&(_, first)) = chars.peek() {
            let kind = word_kind(first);
            while chars.next_if(|(_, c)| !c.is_whitespace() && word_kind(*c) == kind).is_some() {}
        }
        offset + chars.peek().map_or(after.len(), |(at, _)| *at)
    }

    /// The word or run of spaces around `offset`.
    fn word_at(&self, offset: usize) -> Range<usize> {
        let line = self.line_of(offset);
        let (start, end) = (self.line_start(line), self.line_end(line));
        let text = self.text.slice(start..end).to_string();
        let at = offset.saturating_sub(start).min(text.len());
        let class = |c: char| (c.is_whitespace(), word_kind(c));
        // The character under the pointer, or the one before it at the end of the line.
        let Some(kind) = text[at..].chars().next().or_else(|| text[..at].chars().next_back()).map(class) else {
            return offset..offset;
        };
        let from = text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| class(*c) == kind)
            .last()
            .map_or(at, |(ix, _)| ix);
        let to = at + text[at..].chars().take_while(|c| class(*c) == kind).map(char::len_utf8).sum::<usize>();
        start + from..start + to
    }

    /// The column of `offset` in its line: characters, with a tab as four.
    fn column_of(&self, offset: usize) -> usize {
        let start = self.line_start(self.line_of(offset));
        self.text.slice(start..offset).chars().map(width).sum()
    }

    /// The offset in `line` closest to `column`, without passing it.
    fn offset_at_column(&self, line: usize, column: usize) -> usize {
        let (start, end) = (self.line_start(line), self.line_end(line));
        let (mut col, mut offset) = (0, start);
        for c in self.text.slice(start..end).chars() {
            if col + width(c) > column {
                break;
            }
            col += width(c);
            offset += c.len_utf8();
        }
        offset
    }

    /// The text offset under a point on screen.
    fn offset_at(&self, position: Point<Pixels>, window: &mut Window) -> usize {
        let Some(m) = &self.metrics else { return self.cursor };
        let y = f64::from((position.y - m.bounds.top()) / m.line_height) + self.scroll;
        if y < 0. {
            return 0;
        }
        let line = y.floor() as usize;
        if line >= self.lines() {
            return self.text.len();
        }
        let row = Row::new(&self.text, line, self.scroll_x, m.cell_width, m.text_width());
        let x = position.x - m.text_x + self.scroll_x - row.x;
        if x <= px(0.) {
            return row.start;
        }
        let shaped = shape(window, &row.display, &m.font, m.font_size, Hsla::default());
        (row.start + row.to_text(shaped.closest_index_for_x(x))).min(row.end)
    }

    // ---- scrolling ----

    fn max_scroll(&self) -> f64 {
        let rows = self.metrics.as_ref().map_or(1., Metrics::rows);
        (self.lines() as f64 - rows).max(0.)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0., self.max_scroll());
        self.scroll_x = self.scroll_x.max(px(0.));
    }

    /// Scrolls so the cursor is on screen.
    fn reveal(&mut self) {
        let Some(m) = self.metrics.clone() else { return };
        let line = self.line_of(self.cursor) as f64;
        let rows = m.rows().floor().max(1.);
        if line < self.scroll {
            self.scroll = line;
        } else if line + 1. > self.scroll + rows {
            self.scroll = line + 1. - rows;
        }
        let x = m.cell_width * self.column_of(self.cursor) as f32;
        let margin = m.cell_width * 4.;
        if x < self.scroll_x {
            self.scroll_x = (x - margin).max(px(0.));
        } else if x + margin > self.scroll_x + m.text_width() {
            self.scroll_x = x + margin - m.text_width();
        }
        self.clamp_scroll();
    }

    /// Puts the cursor's line in the middle of the screen.
    fn center(&mut self) {
        let rows = self.metrics.as_ref().map_or(1., Metrics::rows);
        self.scroll = self.line_of(self.cursor) as f64 - (rows / 2.).floor();
        self.clamp_scroll();
        self.reveal();
    }

    fn scroll_to_y(&mut self, y: Pixels) {
        let Some(m) = &self.metrics else { return };
        let thumb = m.thumb_height(self.lines());
        let track = (m.bounds.size.height - thumb).max(px(1.));
        let share = ((y - m.bounds.top() - thumb / 2.) / track).clamp(0., 1.);
        self.scroll = f64::from(share) * self.max_scroll();
    }

    // ---- moving ----

    fn move_to(&mut self, offset: usize, select: bool, cx: &mut Context<Self>) {
        self.cursor = offset;
        if !select {
            self.anchor = offset;
        }
        self.goal = None;
        self.typing = false;
        self.reveal();
        cx.notify();
    }

    fn move_lines(&mut self, rows: isize, select: bool, cx: &mut Context<Self>) {
        let goal = self.goal.unwrap_or_else(|| self.column_of(self.cursor));
        let last = self.lines().saturating_sub(1) as isize;
        let line = (self.line_of(self.cursor) as isize + rows).clamp(0, last) as usize;
        self.cursor = self.offset_at_column(line, goal);
        if !select {
            self.anchor = self.cursor;
        }
        self.goal = Some(goal);
        self.typing = false;
        self.reveal();
        cx.notify();
    }

    fn page(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let rows = self.metrics.as_ref().map_or(1., |m| (m.rows() - 1.).max(1.).floor());
        self.scroll += if down { rows } else { -rows };
        self.clamp_scroll();
        self.move_lines(if down { rows as isize } else { -(rows as isize) }, select, cx);
    }

    // ---- editing ----

    /// Puts `new` in place of `range`, and the cursor after it.
    fn replace(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        if range.is_empty() && new.is_empty() {
            return;
        }
        let removed = self.text.slice(range.clone()).to_string();
        self.text.remove(range.clone());
        self.text.insert(range.start, new);
        let joins = self.typing
            && removed.is_empty()
            && !new.contains('\n')
            && self.undo.last().is_some_and(|last| last.removed.is_empty() && last.at + last.inserted.len() == range.start);
        match self.undo.last_mut() {
            Some(last) if joins => last.inserted.push_str(new),
            _ => self.undo.push(Edit {
                at: range.start,
                removed,
                inserted: new.to_owned(),
                before: (self.cursor, self.anchor),
            }),
        }
        self.redo.clear();
        self.cursor = range.start + new.len();
        self.anchor = self.cursor;
        self.goal = None;
        self.typing = !new.is_empty() && !new.contains('\n');
        self.reveal();
        cx.emit(BigTextEvent::Changed);
        cx.notify();
    }

    fn insert(&mut self, new: &str, cx: &mut Context<Self>) {
        self.replace(self.selection(), new, cx);
    }

    /// Deletes the selection, or from the cursor to `to` when nothing is selected.
    fn delete_to(&mut self, to: usize, cx: &mut Context<Self>) {
        let range = match self.selection() {
            range if !range.is_empty() => range,
            _ => self.cursor.min(to)..self.cursor.max(to),
        };
        self.typing = false;
        self.replace(range, "", cx);
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.undo.pop() else { return };
        self.text.remove(edit.at..edit.at + edit.inserted.len());
        self.text.insert(edit.at, &edit.removed);
        (self.cursor, self.anchor) = edit.before;
        self.redo.push(edit);
        self.after_history(cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.redo.pop() else { return };
        self.text.remove(edit.at..edit.at + edit.removed.len());
        self.text.insert(edit.at, &edit.inserted);
        self.cursor = edit.at + edit.inserted.len();
        self.anchor = self.cursor;
        self.undo.push(edit);
        self.after_history(cx);
    }

    fn after_history(&mut self, cx: &mut Context<Self>) {
        self.typing = false;
        self.goal = None;
        self.reveal();
        cx.emit(BigTextEvent::Changed);
        cx.notify();
    }

    fn copy(&self, cx: &mut App) {
        let range = self.selection();
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text.slice(range).to_string()));
        }
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        self.copy(cx);
        if !self.selection().is_empty() {
            self.delete_to(self.cursor, cx);
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.typing = false;
            self.insert(&text, cx);
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.anchor = 0;
        self.cursor = self.text.len();
        cx.notify();
    }

    /// What is selected, empty at the cursor when nothing is.
    pub fn selected_range(&self) -> Range<usize> {
        self.selection()
    }

    /// Puts `new` in place of `range` as one change to undo, from a tool.
    /// A selection stays selected; a whole file keeps the cursor near where it was.
    pub fn replace_range(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        let whole = range == (0..self.text.len());
        let cursor = self.cursor;
        self.typing = false;
        self.replace(range.clone(), new, cx);
        if whole {
            let mut at = cursor.min(self.text.len());
            while !self.text.is_char_boundary(at) {
                at -= 1;
            }
            (self.cursor, self.anchor) = (at, at);
        } else {
            (self.anchor, self.cursor) = (range.start, range.start + new.len());
        }
        self.reveal();
        cx.notify();
    }

    // ---- finding ----

    fn query(&self, cx: &App) -> Option<String> {
        let bar = self.bar.as_ref().filter(|bar| bar.mode == BarMode::Find)?;
        Some(bar.input.read(cx).value().to_string()).filter(|query| !query.is_empty() && !query.contains('\n'))
    }

    fn open_bar(&mut self, mode: BarMode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = self.bar.as_ref().filter(|bar| bar.mode == mode) {
            bar.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            return;
        }
        // A short selection on one line is what to find.
        let selected = self.selection();
        let prefill = (mode == BarMode::Find && !selected.is_empty() && selected.len() <= 200)
            .then(|| self.text.slice(selected).to_string())
            .filter(|text| !text.contains('\n'))
            .unwrap_or_default();
        let placeholder = match mode {
            BarMode::Find => "Find",
            BarMode::GoToLine => "Go to line",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(prefill));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::Change => this.query_changed(cx),
            InputEvent::PressEnter { shift, .. } => match this.bar.as_ref().map(|bar| bar.mode) {
                Some(BarMode::Find) => this.find_next(!shift, cx),
                Some(BarMode::GoToLine) => this.go_to_typed_line(window, cx),
                None => {}
            },
            _ => {}
        });
        self.bar = Some(Bar {
            mode,
            input,
            searched: None,
            count: None,
            missed: false,
            cancel: Arc::new(AtomicBool::new(false)),
            _tasks: Vec::new(),
            _subscription: subscription,
        });
        self.query_changed(cx);
        cx.notify();
    }

    fn close_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = self.bar.take() {
            bar.cancel.store(true, Ordering::Relaxed);
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Counts the matches of a new query, and selects the first one from the cursor on.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let Some(bar) = self.bar.as_mut().filter(|bar| bar.searched != query) else { return };
        bar.searched = query.clone();
        bar.cancel.store(true, Ordering::Relaxed);
        bar.cancel = Arc::new(AtomicBool::new(false));
        bar.count = None;
        bar.missed = false;
        bar._tasks.clear();
        cx.notify();
        let Some(query) = query else { return };
        let (text, cancel) = (self.text.clone(), bar.cancel.clone());
        let count = cx.background_executor().spawn({
            let query = query.clone();
            async move { count_matches(&text, &query, &cancel) }
        });
        bar._tasks.push(cx.spawn(async move |this, cx| {
            let count = count.await;
            let _ = this.update(cx, |this, cx| {
                if let (Some(bar), Some(count)) = (this.bar.as_mut(), count) {
                    bar.count = Some(count);
                    cx.notify();
                }
            });
        }));
        self.find_from(self.selection().start, true, cx);
    }

    /// Finds the next match, or opens the find bar.
    fn find_or_next(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.bar.as_ref().is_some_and(|bar| bar.mode == BarMode::Find) {
            self.find_next(forward, cx);
        } else {
            self.open_bar(BarMode::Find, window, cx);
        }
    }

    fn find_next(&mut self, forward: bool, cx: &mut Context<Self>) {
        let range = self.selection();
        self.find_from(if forward { range.end } else { range.start }, forward, cx);
    }

    /// Selects the next match after `from`, or the one before it, wrapping around.
    fn find_from(&mut self, from: usize, forward: bool, cx: &mut Context<Self>) {
        let Some(query) = self.query(cx) else { return };
        let Some(bar) = self.bar.as_mut() else { return };
        let (text, cancel) = (self.text.clone(), bar.cancel.clone());
        let len = query.len();
        let found = cx
            .background_executor()
            .spawn(async move { find(&text, &query, from, forward, &cancel) });
        bar._tasks.push(cx.spawn(async move |this, cx| {
            let found = found.await;
            let _ = this.update(cx, |this, cx| {
                let Some(bar) = this.bar.as_mut() else { return };
                bar.missed = found.is_none();
                if let Some(start) = found {
                    this.anchor = start;
                    this.cursor = start + len;
                    this.typing = false;
                    this.center();
                }
                cx.notify();
            });
        }));
    }

    fn go_to_typed_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = &self.bar else { return };
        let Ok(line) = bar.input.read(cx).value().trim().parse::<usize>() else { return };
        self.go_to_line(line, cx);
        self.close_bar(window, cx);
    }

    /// Puts the cursor at the start of `line`, counting from 1, in the middle of the view.
    pub fn go_to_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let line = line.clamp(1, self.lines()) - 1;
        self.cursor = self.line_start(line);
        self.anchor = self.cursor;
        self.goal = None;
        self.center();
        cx.notify();
    }

    // ---- input ----

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let (command, shift) = (modifiers.secondary(), modifiers.shift);
        let word = if cfg!(target_os = "macos") { modifiers.alt } else { modifiers.control };
        let line = self.line_of(self.cursor);
        let selected = self.selection();
        match keystroke.key.as_str() {
            "left" => {
                let to = if command {
                    self.line_start(line)
                } else if word {
                    self.word_left(self.cursor)
                } else if !shift && !selected.is_empty() {
                    selected.start
                } else {
                    self.prev_char(self.cursor)
                };
                self.move_to(to, shift, cx);
            }
            "right" => {
                let to = if command {
                    self.line_end(line)
                } else if word {
                    self.word_right(self.cursor)
                } else if !shift && !selected.is_empty() {
                    selected.end
                } else {
                    self.next_char(self.cursor)
                };
                self.move_to(to, shift, cx);
            }
            "up" if command => self.move_to(0, shift, cx),
            "down" if command => self.move_to(self.text.len(), shift, cx),
            "up" => self.move_lines(-1, shift, cx),
            "down" => self.move_lines(1, shift, cx),
            "home" if command => self.move_to(0, shift, cx),
            "end" if command => self.move_to(self.text.len(), shift, cx),
            "home" => self.move_to(self.line_start(line), shift, cx),
            "end" => self.move_to(self.line_end(line), shift, cx),
            "pageup" => self.page(false, shift, cx),
            "pagedown" => self.page(true, shift, cx),
            "backspace" => {
                let to = if command {
                    self.line_start(line)
                } else if word {
                    self.word_left(self.cursor)
                } else {
                    self.prev_char(self.cursor)
                };
                self.delete_to(to, cx);
            }
            "delete" => {
                let to = if word { self.word_right(self.cursor) } else { self.next_char(self.cursor) };
                self.delete_to(to, cx);
            }
            "enter" if !command => self.insert("\n", cx),
            "tab" if !command => self.insert("\t", cx),
            "escape" if self.bar.is_some() => self.close_bar(window, cx),
            "escape" if !selected.is_empty() => self.move_to(self.cursor, false, cx),
            "a" if command => self.select_all(cx),
            "c" if command => self.copy(cx),
            "x" if command => self.cut(cx),
            "v" if command => self.paste(cx),
            "z" if command && shift => self.redo(cx),
            "z" if command => self.undo(cx),
            "f" if command => self.open_bar(BarMode::Find, window, cx),
            "g" if command => self.find_or_next(!shift, window, cx),
            "l" if command => self.open_bar(BarMode::GoToLine, window, cx),
            "g" if modifiers.control => self.open_bar(BarMode::GoToLine, window, cx),
            // Typed text comes in through the input handler.
            _ => return,
        }
        cx.stop_propagation();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let Some(m) = &self.metrics else { return };
        if event.position.x >= m.bounds.right() - SCROLLBAR && self.max_scroll() > 0. {
            self.drag = Drag::Scrollbar;
            self.scroll_to_y(event.position.y);
            cx.notify();
            return;
        }
        let offset = self.offset_at(event.position, window);
        match event.click_count {
            2 => {
                let word = self.word_at(offset);
                (self.anchor, self.cursor) = (word.start, word.end);
            }
            3 => {
                let line = self.line_of(offset);
                self.anchor = self.line_start(line);
                self.cursor = self.line_start(line + 1);
            }
            _ => {
                self.cursor = offset;
                if !event.modifiers.shift {
                    self.anchor = offset;
                }
            }
        }
        self.drag = Drag::Select;
        self.goal = None;
        self.typing = false;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = Drag::None;
            return;
        }
        match self.drag {
            Drag::None => {}
            Drag::Scrollbar => {
                self.scroll_to_y(event.position.y);
                cx.notify();
            }
            Drag::Select => {
                let Some(bounds) = self.metrics.as_ref().map(|m| m.bounds) else { return };
                // Dragging past the top or bottom scrolls.
                if event.position.y < bounds.top() {
                    self.scroll -= 1.;
                } else if event.position.y > bounds.bottom() {
                    self.scroll += 1.;
                }
                self.clamp_scroll();
                self.cursor = self.offset_at(event.position, window);
                cx.notify();
            }
        }
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.metrics else { return };
        let delta = event.delta.pixel_delta(m.line_height);
        let (dx, dy) = if event.modifiers.shift && delta.x == px(0.) { (delta.y, px(0.)) } else { (delta.x, delta.y) };
        let max_x = m.max_x.max(self.scroll_x);
        self.scroll -= f64::from(dy / m.line_height);
        self.scroll_x = (self.scroll_x - dx).min(max_x);
        self.clamp_scroll();
        cx.notify();
        cx.stop_propagation();
    }

    fn render_bar(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let bar = self.bar.as_ref()?;
        let theme = cx.theme();
        let status = match (bar.mode, bar.count) {
            (BarMode::GoToLine, _) => format!("of {}", self.lines()),
            (BarMode::Find, _) if bar.input.read(cx).value().is_empty() => String::new(),
            (BarMode::Find, Some(0)) => "No matches".to_owned(),
            (BarMode::Find, Some(1)) => "1 match".to_owned(),
            (BarMode::Find, Some(count)) => format!("{count} matches"),
            (BarMode::Find, None) if bar.missed => "No matches".to_owned(),
            (BarMode::Find, None) => "Counting…".to_owned(),
        };
        let find = bar.mode == BarMode::Find;
        Some(
            h_flex()
                .h(px(36.))
                .flex_none()
                .px_2()
                .gap_1()
                .items_center()
                .border_b_1()
                .border_color(theme.border)
                .capture_action(cx.listener(|this, _: &Escape, window, cx| this.close_bar(window, cx)))
                // Find next and previous also work while typing the query.
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    let modifiers = event.keystroke.modifiers;
                    if modifiers.secondary() && event.keystroke.key == "g" {
                        this.find_or_next(!modifiers.shift, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .when(find && self.jq, |row| {
                    row.child(Button::new("big-find-text").ghost().xsmall().label("Text").selected(true)).child(
                        Button::new("big-find-jq").ghost().xsmall().label("jq").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.close_bar(window, cx);
                                cx.emit(BigTextEvent::Jq);
                            },
                        )),
                    )
                })
                .child(div().w(px(280.)).child(Input::new(&bar.input).small()))
                .child(div().px_1().text_xs().text_color(theme.muted_foreground).child(status))
                .when(find, |row| {
                    row.child(
                        Button::new("big-find-prev")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronUp)
                            .tooltip("Previous (⇧⏎)")
                            .on_click(cx.listener(|this, _, _, cx| this.find_next(false, cx))),
                    )
                    .child(
                        Button::new("big-find-next")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronDown)
                            .tooltip("Next (⏎)")
                            .on_click(cx.listener(|this, _, _, cx| this.find_next(true, cx))),
                    )
                })
                .child(div().flex_1())
                .child(
                    Button::new("big-bar-close")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .on_click(cx.listener(|this, _, window, cx| this.close_bar(window, cx))),
                ),
        )
    }
}

fn word_kind(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn width(c: char) -> usize {
    if c == '\t' { TAB } else { 1 }
}

/// Where a line's text ends, before its line break.
fn line_end(text: &Rope, line: usize) -> usize {
    let start = text.line_to_byte_idx(line, LF);
    let mut end = text.line_to_byte_idx(line + 1, LF);
    if end > start && text.byte(end - 1) == b'\n' {
        end -= 1;
    }
    if end > start && text.byte(end - 1) == b'\r' {
        end -= 1;
    }
    end
}

fn shape(window: &Window, text: &str, font: &Font, font_size: Pixels, color: Hsla) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window.text_system().shape_line(SharedString::from(text.to_owned()), font_size, &[run], None)
}

/// The part of a line that gets laid out, with tabs as spaces.
struct Row {
    /// Where the laid out part starts and ends in the text.
    start: usize,
    end: usize,
    display: String,
    /// Where tabs were, in bytes from `start`.
    tabs: Vec<usize>,
    /// How far right of the line's start the laid out part is drawn.
    x: Pixels,
}

impl Row {
    fn new(text: &Rope, line: usize, scroll_x: Pixels, cell_width: Pixels, width: Pixels) -> Self {
        let mut start = text.line_to_byte_idx(line, LF);
        let mut end = line_end(text, line);
        let mut x = px(0.);
        if end - start > LONG_LINE {
            // Only the part on screen, counting each character as one column.
            let first = text.byte_to_char_idx(start);
            let chars = text.byte_to_char_idx(end) - first;
            let skip = ((scroll_x / cell_width).floor().max(0.) as usize).min(chars);
            let take = (width / cell_width).ceil() as usize + 2;
            start = text.char_to_byte_idx(first + skip);
            end = text.char_to_byte_idx(first + (skip + take).min(chars));
            x = cell_width * skip as f32;
        }
        let raw = text.slice(start..end).to_string();
        let mut tabs = Vec::new();
        let mut display = String::with_capacity(raw.len());
        for (ix, c) in raw.char_indices() {
            if c == '\t' {
                tabs.push(ix);
                display.push_str(&" ".repeat(TAB));
            } else {
                display.push(c);
            }
        }
        Self { start, end, display, tabs, x }
    }

    /// Where a byte of the text (counted from `start`) is in `display`.
    fn to_display(&self, index: usize) -> usize {
        index + (TAB - 1) * self.tabs.partition_point(|&tab| tab < index)
    }

    /// The byte of the text (counted from `start`) at a place in `display`.
    fn to_text(&self, display: usize) -> usize {
        let mut extra = 0;
        for &tab in &self.tabs {
            let at = tab + extra;
            if display <= at {
                break;
            }
            if display < at + TAB {
                return if display - at > TAB / 2 { tab + 1 } else { tab };
            }
            extra += TAB - 1;
        }
        display - extra
    }
}

/// What a frame needs from the editor, taken when it renders.
struct Snapshot {
    text: Rope,
    scroll: f64,
    scroll_x: Pixels,
    cursor: usize,
    anchor: usize,
    focused: bool,
    query: Option<String>,
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
    colors: Colors,
}

struct Colors {
    text: Hsla,
    muted: Hsla,
    selection: Hsla,
    caret: Hsla,
    line: Hsla,
    background: Hsla,
    thumb: Hsla,
}

/// What to draw, worked out before painting.
struct Frame {
    metrics: Metrics,
    /// Behind everything, across the full width.
    under: Vec<(Bounds<Pixels>, Hsla)>,
    /// Behind the text, clipped to the text area.
    marks: Vec<(Bounds<Pixels>, Hsla)>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    caret: Option<Bounds<Pixels>>,
    thumb: Option<Bounds<Pixels>>,
}

fn layout(snap: &Snapshot, bounds: Bounds<Pixels>, window: &Window) -> Frame {
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&snap.font);
    let cell_width = text_system
        .advance(font_id, snap.font_size, 'm')
        .map(|advance| advance.width)
        .unwrap_or(snap.font_size * 0.6);
    let lh = snap.line_height;
    let lines = snap.text.len_lines(LF);
    let digits = lines.to_string().len().max(3);
    let text_x = bounds.left() + px(12.) + cell_width * digits as f32 + GAP;
    let mut metrics = Metrics {
        bounds,
        text_x,
        line_height: lh,
        cell_width,
        font: snap.font.clone(),
        font_size: snap.font_size,
        max_x: px(0.),
    };
    let text_width = metrics.text_width();
    let first = snap.scroll.floor() as usize;
    let fraction = (snap.scroll - first as f64) as f32;
    let selection = snap.cursor.min(snap.anchor)..snap.cursor.max(snap.anchor);
    let cursor_line = snap.text.byte_to_line_idx(snap.cursor, LF);
    let finder = snap.query.as_deref().and_then(finder);
    let mut frame = Frame {
        metrics: metrics.clone(),
        under: Vec::new(),
        marks: Vec::new(),
        numbers: Vec::new(),
        lines: Vec::new(),
        caret: None,
        thumb: None,
    };
    let rows = (bounds.size.height / lh).ceil() as usize + 1;
    for ix in 0..rows {
        let line = first + ix;
        if line >= lines {
            break;
        }
        let y = bounds.top() + lh * (ix as f32 - fraction);
        let current = line == cursor_line;
        if current && selection.is_empty() && snap.focused {
            frame.under.push((Bounds::new(point(bounds.left(), y), size(bounds.size.width, lh)), snap.colors.line));
        }
        let number = shape(window, &(line + 1).to_string(), &snap.font, snap.font_size, if current {
            snap.colors.text
        } else {
            snap.colors.muted
        });
        frame.numbers.push((point(text_x - GAP - number.width, y), number));

        let row = Row::new(&snap.text, line, snap.scroll_x, cell_width, text_width);
        let shaped = shape(window, &row.display, &snap.font, snap.font_size, snap.colors.text);
        let x = text_x - snap.scroll_x + row.x;
        metrics.max_x = metrics.max_x.max(row.x + shaped.width + cell_width * 4. - text_width);
        let span = |from: usize, to: usize| {
            let left = shaped.x_for_index(row.to_display(from - row.start));
            let right = shaped.x_for_index(row.to_display(to - row.start));
            Bounds::new(point(x + left, y), size(right - left, lh))
        };
        if let Some(finder) = &finder {
            for hit in finder.find_iter(row.display.as_bytes()) {
                let left = shaped.x_for_index(hit.start());
                let right = shaped.x_for_index(hit.end());
                frame.marks.push((Bounds::new(point(x + left, y), size(right - left, lh)), search_color(false)));
            }
        }
        let (from, to) = (selection.start.max(row.start), selection.end.min(row.end));
        if from < to {
            frame.marks.push((span(from, to), snap.colors.selection));
        }
        // A selected line break shows as a little block after the line.
        let line_end = line_end(&snap.text, line);
        if row.end == line_end && selection.start <= line_end && selection.end > line_end {
            let left = shaped.x_for_index(row.display.len());
            frame.marks.push((Bounds::new(point(x + left, y), size(cell_width * 0.6, lh)), snap.colors.selection));
        }
        if current && snap.focused && (row.start..=row.end).contains(&snap.cursor) {
            let left = shaped.x_for_index(row.to_display(snap.cursor - row.start));
            frame.caret = Some(Bounds::new(point(x + left, y), size(px(2.), lh)));
        }
        frame.lines.push((point(x, y), shaped));
    }
    if lines as f64 > metrics.rows() {
        let thumb = metrics.thumb_height(lines);
        let max_scroll = (lines as f64 - metrics.rows()).max(1.);
        let share = (snap.scroll / max_scroll).clamp(0., 1.) as f32;
        let y = bounds.top() + (bounds.size.height - thumb) * share;
        frame.thumb = Some(Bounds::new(point(bounds.right() - SCROLLBAR + px(3.), y), size(SCROLLBAR - px(6.), thumb)));
    }
    frame.metrics = metrics;
    frame
}

impl Focusable for BigText {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BigText {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = Settings::get(cx);
        let font_size = px(settings.font_size.clamp(6., 72.));
        let menu_focus = self.focus_handle.clone();
        let has_selection = !self.selection().is_empty();
        let theme = cx.theme();
        let snapshot = Snapshot {
            text: self.text.clone(),
            scroll: self.scroll,
            scroll_x: self.scroll_x,
            cursor: self.cursor,
            anchor: self.anchor,
            focused: self.focus_handle.is_focused(window),
            query: self.query(cx),
            font: terminal_font(&settings.font_family),
            font_size,
            line_height: (font_size * 1.5).round(),
            colors: Colors {
                text: theme.foreground,
                muted: theme.muted_foreground,
                selection: theme.selection,
                caret: theme.caret,
                line: theme.foreground.opacity(0.04),
                background: theme.background,
                thumb: theme.scrollbar_thumb,
            },
        };
        let (this, focus) = (cx.entity(), self.focus_handle.clone());
        let surface = canvas(
            move |bounds, window, cx| {
                let frame = layout(&snapshot, bounds, window);
                let metrics = frame.metrics.clone();
                this.update(cx, |big, _| big.metrics = Some(metrics));
                (frame, snapshot.colors, this)
            },
            move |bounds, (frame, colors, this), window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(bounds, this), cx);
                // The last row is cut in half at the bottom.
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(fill(bounds, colors.background));
                    for (rect, color) in frame.under {
                        window.paint_quad(fill(rect, color));
                    }
                    let lh = frame.metrics.line_height;
                    for (origin, number) in &frame.numbers {
                        let _ = number.paint(*origin, lh, TextAlign::Left, None, window, cx);
                    }
                    let text_area = Bounds::from_corners(
                        point(frame.metrics.text_x - px(2.), bounds.top()),
                        point(bounds.right() - SCROLLBAR, bounds.bottom()),
                    );
                    window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
                        for (rect, color) in frame.marks {
                            window.paint_quad(fill(rect, color));
                        }
                        for (origin, line) in &frame.lines {
                            let _ = line.paint(*origin, lh, TextAlign::Left, None, window, cx);
                        }
                        if let Some(caret) = frame.caret {
                            window.paint_quad(fill(caret, colors.caret));
                        }
                    });
                    if let Some(thumb) = frame.thumb {
                        window.paint_quad(fill(thumb, colors.thumb).corner_radii(px(3.)));
                    }
                });
            },
        )
        .size_full();
        v_flex()
            .size_full()
            .children(self.render_bar(cx))
            .child(
                div()
                    .id("big-text")
                    .flex_1()
                    .min_h_0()
                    .track_focus(&self.focus_handle)
                    .cursor(CursorStyle::IBeam)
                    .on_key_down(cx.listener(Self::key_down))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = Drag::None))
                    .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                    .on_action(cx.listener(|this, _: &Cut, _, cx| this.cut(cx)))
                    .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
                    .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
                    .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
                    .child(surface)
                    .context_menu(move |menu, _, _| {
                        let menu = menu
                            .action_context(menu_focus.clone())
                            .menu_with_disabled("Cut", Box::new(Cut), !has_selection)
                            .menu_with_disabled("Copy", Box::new(Copy), !has_selection)
                            .menu("Paste", Box::new(Paste))
                            .separator()
                            .menu("Select All", Box::new(SelectAll))
                            .separator();
                        Tool::ALL.into_iter().fold(menu, |menu, tool| menu.menu(tool.label(), tool.action()))
                    }),
            )
    }
}

impl EntityInputHandler for BigText {
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !text.is_empty() {
            self.insert(text, cx);
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
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Puts the input method's candidate window by the cursor.
        let m = self.metrics.as_ref()?;
        let row = self.line_of(self.cursor) as f64 - self.scroll;
        let x = m.text_x - self.scroll_x + m.cell_width * self.column_of(self.cursor) as f32;
        let y = m.bounds.top() + m.line_height * row as f32;
        Some(Bounds::new(point(x, y), size(m.cell_width, m.line_height)))
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

/// Finds `query`, ignoring case unless it has a capital letter.
fn finder(query: &str) -> Option<Regex> {
    let ignore_case = !query.chars().any(char::is_uppercase);
    RegexBuilder::new(&regex::escape(query)).case_insensitive(ignore_case).unicode(false).build().ok()
}

/// Calls `found` with the start of each match in `range`, in order, until it
/// returns false. Returns false when `cancel` stopped it.
fn scan(text: &Rope, query: &str, range: Range<usize>, cancel: &AtomicBool, mut found: impl FnMut(usize) -> bool) -> bool {
    let Some(finder) = finder(query).filter(|_| !query.is_empty() && !range.is_empty()) else {
        return true;
    };
    let (chunks, mut chunk_start) = text.chunks_at(range.start);
    // The end of the last chunk is kept, for matches that cross into the next.
    let mut window: Vec<u8> = Vec::new();
    let mut window_start = range.start;
    for chunk in chunks {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let chunk_end = chunk_start + chunk.len();
        let from = range.start.max(chunk_start) - chunk_start;
        let to = range.end.min(chunk_end).saturating_sub(chunk_start);
        if from < to {
            let drop = window.len().saturating_sub(query.len() - 1);
            window.drain(..drop);
            window_start += drop;
            window.extend_from_slice(&chunk.as_bytes()[from..to]);
            for hit in finder.find_iter(&window) {
                if !found(window_start + hit.start()) {
                    return true;
                }
            }
        }
        chunk_start = chunk_end;
        if chunk_end >= range.end {
            break;
        }
    }
    true
}

/// How many matches the text has. None when cancelled.
fn count_matches(text: &Rope, query: &str, cancel: &AtomicBool) -> Option<usize> {
    let mut count = 0;
    scan(text, query, 0..text.len(), cancel, |_| {
        count += 1;
        true
    })
    .then_some(count)
}

/// The first match at or after `from`, or the last one before it, wrapping around.
fn find(text: &Rope, query: &str, from: usize, forward: bool, cancel: &AtomicBool) -> Option<usize> {
    let len = text.len();
    let from = from.min(len);
    let first = |range: Range<usize>| {
        let mut hit = None;
        scan(text, query, range, cancel, |at| {
            hit = Some(at);
            false
        });
        hit
    };
    let last = |range: Range<usize>| {
        let mut hit = None;
        scan(text, query, range, cancel, |at| {
            hit = Some(at);
            true
        });
        hit
    };
    if forward {
        first(from..len).or_else(|| first(0..(from + query.len()).min(len)))
    } else {
        last(0..from).or_else(|| last(from..len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_turn_tabs_into_spaces_and_back() {
        let text = Rope::from("a\tb\tc\nnext");
        let row = Row::new(&text, 0, px(0.), px(8.), px(800.));
        assert_eq!(row.display, "a    b    c");
        assert_eq!(row.to_display(2), 5);
        assert_eq!(row.to_text(5), 2);
        assert_eq!(row.to_text(2), 1);
        assert_eq!(row.end, 5);
        assert_eq!(line_end(&Rope::from("one\r\ntwo"), 0), 3);
    }

    #[test]
    fn long_lines_lay_out_only_what_is_on_screen() {
        let text = Rope::from("x".repeat(LONG_LINE * 3));
        let row = Row::new(&text, 0, px(800.), px(8.), px(80.));
        assert_eq!(row.start, 100);
        assert_eq!(row.display.len(), 12);
        assert_eq!(row.x, px(800.));
    }

    #[test]
    fn search_finds_matches_across_chunks_and_wraps_around() {
        let body = "lorem ipsum ".repeat(5000) + "Needle" + &" dolor".repeat(5000);
        let text = Rope::from(body.as_str());
        let stop = AtomicBool::new(false);
        let at = body.find("Needle").unwrap();
        assert_eq!(find(&text, "needle", 0, true, &stop), Some(at));
        assert_eq!(find(&text, "needle", at + 1, true, &stop), Some(at));
        assert_eq!(find(&text, "Needle", text.len(), false, &stop), Some(at));
        assert_eq!(find(&text, "NEEDLE", 0, true, &stop), None);
        assert_eq!(count_matches(&text, "ipsum", &stop), Some(5000));
        assert_eq!(count_matches(&text, "dolor ", &stop), Some(4999));
    }
}
