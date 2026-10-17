//! Mouse, copy, scroll and search over command blocks.

use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Boundary, Column, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::search::RegexSearch;
use gpui_kit::{
    ClipboardItem, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, Window, px,
};

use super::{SearchMatch, TerminalView};
use crate::settings::Settings;

impl TerminalView {
    pub(super) fn blocks_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // At the prompt the editor keeps focus, so typing goes on while text is selected.
        if self.session.read(cx).at_prompt() {
            if !self.prompt_focused(window, cx) {
                self.focus_prompt(window, cx);
            }
        } else {
            window.focus(&self.focus_handle, cx);
        }
        if event.button != MouseButton::Left {
            return;
        }
        let Some(layout) = &self.blocks_layout else { return };
        let hit = layout.block_at(event.position.y).and_then(|index| {
            let geom = layout.blocks[index];
            let output_bottom = geom.output_origin.y + layout.line_height * geom.rows as f32;
            let in_output = event.position.y >= geom.output_origin.y && event.position.y < output_bottom;
            in_output.then(|| layout.point_in(index, event.position).map(|point| (index, point)))?
        });
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        let extend = event.modifiers.shift;
        // A plain click on a file or folder name opens it, unless it turns into a drag.
        self.link_press = (hit.is_some() && event.click_count == 1 && !extend)
            .then(|| self.link_at(event.position))
            .flatten();
        self.session.update(cx, |session, cx| {
            let Some(blocks) = session.blocks.as_mut() else { return };
            if extend
                && let Some((index, (point, side))) = hit
                && let Some(selection) = blocks.list[index].term.selection.as_mut()
            {
                selection.update(point, side);
                cx.notify();
                return;
            }
            for block in &mut blocks.list {
                block.term.selection = None;
            }
            if let Some((index, (point, side))) = hit {
                blocks.list[index].term.selection = Some(Selection::new(kind, point, side));
            }
            cx.notify();
        });
        self.selecting_block = hit.map(|(index, _)| index);
    }

    pub(super) fn blocks_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() {
            self.hover_link(event.position, cx);
        }
        let Some(index) = self.selecting_block else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(layout) = &self.blocks_layout else { return };
        // Dragging past the top or bottom edge scrolls.
        if event.position.y < layout.bounds.top() {
            self.blocks_scroll += layout.line_height;
        } else if event.position.y > layout.bounds.bottom() {
            self.blocks_scroll -= layout.line_height;
        }
        let Some((point, side)) = layout.point_in(index, event.position) else { return };
        self.session.update(cx, |session, cx| {
            if let Some(block) = session.blocks.as_mut().and_then(|blocks| blocks.list.get_mut(index))
                && let Some(selection) = block.term.selection.as_mut()
            {
                selection.update(point, side);
                cx.notify();
            }
        });
    }

    pub(super) fn blocks_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pressed = self.link_press.take();
        let Some(index) = self.selecting_block.take() else { return };
        let empty = self
            .session
            .read(cx)
            .blocks
            .as_ref()
            .and_then(|blocks| blocks.list.get(index))
            .and_then(|block| block.term.selection.as_ref())
            .is_none_or(|selection| selection.is_empty());
        if empty {
            self.session.update(cx, |session, cx| {
                if let Some(block) = session.blocks.as_mut().and_then(|blocks| blocks.list.get_mut(index)) {
                    block.term.selection = None;
                }
                cx.notify();
            });
            if let Some(link) = pressed.filter(|link| self.link_at(event.position).as_ref() == Some(link)) {
                self.open_link(link, window, cx);
            }
        } else if Settings::get(cx).copy_on_select
            && let Some(text) = self.blocks_selection_text(cx)
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn blocks_scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line_height = self.blocks_layout.as_ref().map_or(px(16.), |layout| layout.line_height);
        // The text moves away from under the mouse.
        self.clear_link(cx);
        self.blocks_scroll += event.delta.pixel_delta(line_height).y;
        self.blocks_scroll = self.blocks_scroll.max(px(0.));
        cx.notify();
    }

    pub(super) fn blocks_scroll_to(&mut self, scroll: Scroll, cx: &mut Context<Self>) {
        let page = self.blocks_layout.as_ref().map_or(px(400.), |layout| layout.bounds.size.height * 0.9);
        self.blocks_scroll = match scroll {
            Scroll::PageUp => self.blocks_scroll + page,
            Scroll::PageDown => (self.blocks_scroll - page).max(px(0.)),
            Scroll::Top => self.blocks_total,
            Scroll::Bottom => px(0.),
            Scroll::Delta(lines) => {
                let line_height = self.blocks_layout.as_ref().map_or(px(16.), |layout| layout.line_height);
                (self.blocks_scroll + line_height * lines as f32).max(px(0.))
            }
        };
        cx.notify();
    }

    /// The selected text across all blocks.
    pub(super) fn blocks_selection_text(&self, cx: &gpui_kit::App) -> Option<String> {
        let blocks = self.session.read(cx).blocks.as_ref()?;
        let parts: Vec<String> = blocks
            .list
            .iter()
            .filter_map(|block| block.term.selection_to_string())
            .filter(|text| !text.is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    pub(super) fn blocks_select_all(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            let Some(blocks) = session.blocks.as_mut() else { return };
            for block in &mut blocks.list {
                let rows = block.rows();
                if rows == 0 {
                    block.term.selection = None;
                    continue;
                }
                let first = block.first_line();
                let last_column = block.term.last_column();
                let mut selection = Selection::new(SelectionType::Simple, GridPoint::new(first, Column(0)), Side::Left);
                selection.update(GridPoint::new(first + (rows - 1) as i32, last_column), Side::Right);
                block.term.selection = Some(selection);
            }
            cx.notify();
        });
    }

    /// Copies a block's command or its output, from the right-click menu.
    pub(super) fn copy_block(&mut self, output: bool, cx: &mut Context<Self>) {
        let Some(index) = self.menu_block else { return };
        let text = self
            .session
            .read(cx)
            .blocks
            .as_ref()
            .and_then(|blocks| blocks.list.get(index))
            .map(|block| if output { block.output_text() } else { block.command.clone() });
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn search_blocks(&self, regex: &mut RegexSearch, cx: &gpui_kit::App) -> Vec<SearchMatch> {
        let mut matches = Vec::new();
        let Some(blocks) = self.session.read(cx).blocks.as_ref() else { return matches };
        for (index, block) in blocks.list.iter().enumerate() {
            let rows = block.rows();
            if rows == 0 {
                continue;
            }
            let term = &block.term;
            let first = block.first_line();
            let mut origin = GridPoint::new(first, Column(0));
            let end = GridPoint::new(first + (rows - 1) as i32, term.last_column());
            while let Some(found) = term.regex_search_right(regex, origin, end) {
                let (start, stop) = (*found.start(), *found.end());
                matches.push(SearchMatch {
                    block: Some(index),
                    start,
                    end: stop,
                });
                if matches.len() >= 2000 {
                    return matches;
                }
                let next = stop.add(term, Boundary::None, 1);
                if next <= stop || next > end {
                    break;
                }
                origin = next;
            }
        }
        matches
    }

    /// Scrolls so a block's output line is in view.
    pub(super) fn reveal_block_line(&mut self, block: usize, line: alacritty_terminal::index::Line, cx: &mut Context<Self>) {
        let Some(layout) = &self.blocks_layout else { return };
        let Some(y) = layout.row_y(block, line) else { return };
        let bounds = layout.bounds;
        if y < bounds.top() + layout.line_height || y > bounds.bottom() - layout.line_height * 2. {
            self.blocks_scroll += bounds.center().y - y;
            self.blocks_scroll = self.blocks_scroll.max(px(0.));
            cx.notify();
        }
    }
}
