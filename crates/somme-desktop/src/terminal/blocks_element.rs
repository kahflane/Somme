//! Draws a shell's command blocks: a header with the folder, branch and
//! time, the command, then what it printed. The newest block sits at the bottom.

use std::time::Duration;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor};
use gpui_kit::{
    App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, Font, FontWeight, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, ShapedLine, Style, TextAlign, TextRun, Window,
    fill, point, px, relative, size,
};

use crate::settings::Settings;
use crate::terminal::blocks::{Block, BlockState, format_took, short_path};
use crate::terminal::colors::{self, to_hsla};
use crate::terminal::element::{CellPainter, CursorPaint, PADDING, PaintedCell, cell_metrics, cursor_shape, search_color, terminal_font};
use crate::terminal::session::{GridSize, TerminalSession};
use crate::terminal::view::TerminalView;

/// Space above and below each block's content.
const GAP: Pixels = px(8.);
/// Width of the red bar beside a failed command.
const BAR: Pixels = px(3.);
/// Accepted lines show their output only after this long, so the shell's
/// redraw of the line never flashes up before the command starts.
pub const ACCEPTED_HIDE: Duration = Duration::from_millis(250);

/// Where one block sits on screen, kept for mouse hits and search.
#[derive(Clone, Copy, Debug)]
pub struct BlockGeom {
    pub top: Pixels,
    pub bottom: Pixels,
    pub output_origin: Point<Pixels>,
    pub rows: usize,
    pub first_line: Line,
}

/// The blocks as drawn in the last frame.
#[derive(Clone, Debug)]
pub struct BlocksLayout {
    pub bounds: Bounds<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub columns: usize,
    /// One per block, in the same order as the session's list.
    pub blocks: Vec<BlockGeom>,
}

impl BlocksLayout {
    /// The block under a screen height.
    pub fn block_at(&self, y: Pixels) -> Option<usize> {
        self.blocks.iter().position(|geom| y >= geom.top && y < geom.bottom)
    }

    /// The output cell under a screen position, kept inside the block's rows.
    pub fn point_in(&self, block: usize, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let geom = self.blocks.get(block)?;
        if geom.rows == 0 {
            return None;
        }
        let last_column = self.columns.saturating_sub(1);
        let y = position.y - geom.output_origin.y;
        if y < px(0.) {
            return Some((GridPoint::new(geom.first_line, Column(0)), Side::Left));
        }
        let row = (y / self.line_height).floor() as usize;
        if row >= geom.rows {
            let line = geom.first_line + (geom.rows - 1) as i32;
            return Some((GridPoint::new(line, Column(last_column)), Side::Right));
        }
        let x = (position.x - geom.output_origin.x).max(px(0.)) / self.cell_width;
        let column = (x.floor() as usize).min(last_column);
        let side = if x.fract() > 0.5 { Side::Right } else { Side::Left };
        Some((GridPoint::new(geom.first_line + row as i32, Column(column)), side))
    }

    /// Where an output row of a block is on screen.
    pub fn row_y(&self, block: usize, line: Line) -> Option<Pixels> {
        let geom = self.blocks.get(block)?;
        Some(geom.output_origin.y + self.line_height * (line.0 - geom.first_line.0) as f32)
    }
}

/// Keeps the view still while output grows below it, and inside the content.
pub fn settle_scroll(scroll: Pixels, last_total: Pixels, total: Pixels, viewport: Pixels) -> Pixels {
    let mut scroll = scroll;
    if scroll > px(0.) {
        scroll += total - last_total;
    }
    scroll.clamp(px(0.), (total - viewport).max(px(0.)))
}

/// Splits a command into rows of at most `columns` characters.
fn command_rows(command: &str, columns: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in command.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
        }
        for chunk in chars.chunks(columns.max(1)) {
            rows.push(chunk.iter().collect());
        }
    }
    rows
}

fn output_rows(block: &Block) -> usize {
    if block.state == BlockState::Accepted && block.started.elapsed() < ACCEPTED_HIDE {
        0
    } else {
        block.rows()
    }
}

struct Text {
    origin: Point<Pixels>,
    line: ShapedLine,
    height: Pixels,
}

pub struct PrepaintState {
    background: Hsla,
    tints: Vec<(Bounds<Pixels>, Hsla)>,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    texts: Vec<Text>,
    cursor: Option<CursorPaint>,
    line_height: Pixels,
}

pub struct BlocksElement {
    view: Entity<TerminalView>,
    session: Entity<TerminalSession>,
    focused: bool,
    cursor_visible: bool,
}

impl BlocksElement {
    pub fn new(view: Entity<TerminalView>, session: Entity<TerminalSession>, focused: bool, cursor_visible: bool) -> Self {
        Self {
            view,
            session,
            focused,
            cursor_visible,
        }
    }
}

impl IntoElement for BlocksElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn run(len: usize, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

impl Element for BlocksElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let settings = Settings::get(cx).clone();
        let theme = colors::active_theme(cx);
        let (font, font_size, cell_width, line_height) = cell_metrics(&settings, window);
        let small_font = terminal_font(&settings.font_family);
        let small_size = (font_size * 0.85).round();
        let bold_font = Font {
            weight: FontWeight::SEMIBOLD,
            ..font.clone()
        };

        let left = bounds.origin.x + PADDING + BAR;
        let inner_width = (bounds.size.width - PADDING * 2. - BAR).max(cell_width);
        let inner_height = (bounds.size.height - PADDING * 2.).max(line_height);
        let columns = ((inner_width / cell_width).floor() as usize).max(2);
        let lines = ((inner_height / line_height).floor() as usize).max(1);
        self.session.update(cx, |session, _| {
            session.resize(GridSize {
                columns,
                lines,
                cell_width: cell_width.as_f32(),
                cell_height: line_height.as_f32(),
            });
        });

        let foreground = to_hsla(theme.foreground);
        let muted = foreground.opacity(0.5);
        let green = to_hsla(theme.ansi[2]);
        let red = to_hsla(theme.ansi[1]);
        let separator = foreground.opacity(0.1);

        let (last_scroll, last_total) = {
            let view = self.view.read(cx);
            (view.blocks_scroll, view.blocks_total)
        };
        let session = self.session.read(cx);
        let empty = Vec::new();
        let list = session.blocks.as_ref().map_or(&empty, |blocks| &blocks.list);
        let home = session.blocks.as_ref().and_then(|blocks| blocks.home.as_deref());

        // Heights first, so the newest block can sit at the bottom.
        let heights: Vec<(Vec<String>, usize, Pixels)> = list
            .iter()
            .map(|block| {
                let rows = output_rows(block);
                if block.startup {
                    return (Vec::new(), rows, GAP * 2. + line_height * rows as f32 + px(1.));
                }
                let command = command_rows(&block.command, columns);
                let mut height = GAP * 2. + line_height * (1 + command.len()) as f32 + px(1.);
                if rows > 0 {
                    height += GAP / 2. + line_height * rows as f32;
                }
                (command, rows, height)
            })
            .collect();
        let total: Pixels = heights.iter().map(|(_, _, height)| *height).fold(px(0.), |a, b| a + b);
        let viewport = bounds.size.height;
        let scroll = settle_scroll(last_scroll, last_total, total, viewport);
        let mut y = bounds.origin.y + viewport - total + scroll;

        let search = self.view.read(cx).block_search_matches();
        let link = self.view.read(cx).hovered_link().cloned();
        let mut geoms = Vec::with_capacity(list.len());
        let mut tints = Vec::new();
        let mut rects = Vec::new();
        let mut texts = Vec::new();
        let mut cursor = None;
        let text_system = window.text_system().clone();
        let visible = |top: Pixels, height: Pixels| top + height > bounds.origin.y && top < bounds.origin.y + viewport;
        let mut row_key = 0usize;

        for (index, (block, (command, rows, height))) in list.iter().zip(heights).enumerate() {
            let top = y;
            let bottom = y + height;
            y = bottom;
            let header_y = top + GAP;
            let command_y = header_y + line_height;
            let output_y = if block.startup {
                top + GAP
            } else {
                command_y + line_height * command.len() as f32 + GAP / 2.
            };
            geoms.push(BlockGeom {
                top,
                bottom,
                output_origin: point(left, output_y),
                rows,
                first_line: block.first_line(),
            });
            if !visible(top, height) {
                continue;
            }

            let failed = block.exit_code().is_some_and(|code| code != 0);
            let block_bounds = Bounds::new(point(bounds.origin.x, top), size(bounds.size.width, height - px(1.)));
            if failed {
                tints.push((block_bounds, red.opacity(0.07)));
                tints.push((Bounds::new(block_bounds.origin, size(BAR, block_bounds.size.height)), red));
            }
            tints.push((
                Bounds::new(point(bounds.origin.x, bottom - px(1.)), size(bounds.size.width, px(1.))),
                separator,
            ));

            // Header: folder, branch, and how long it took.
            let mut header = block.cwd.as_deref().map(|cwd| short_path(cwd, home)).unwrap_or_default();
            let mut header_runs = vec![run(header.len(), &small_font, muted)];
            if let Some(branch) = &block.branch {
                let text = format!(" git:({branch})");
                header_runs.push(run(text.len(), &small_font, muted));
                header.push_str(&text);
            }
            let status = match block.state {
                BlockState::Done { took, .. } => format!(" ({})", format_took(took)),
                BlockState::Running => " running…".to_owned(),
                BlockState::Accepted => String::new(),
            };
            if !status.is_empty() {
                let color = if block.state == BlockState::Running { green } else { muted };
                header_runs.push(run(status.len(), &small_font, color));
                header.push_str(&status);
            }
            if !block.startup && !header.is_empty() && visible(header_y, line_height) {
                let line = text_system.shape_line(header.into(), small_size, &header_runs, None);
                // Sits on the same baseline area as a full row.
                texts.push(Text {
                    origin: point(left, header_y),
                    line,
                    height: line_height,
                });
            }

            // The command, with the program name in green.
            for (i, text) in command.iter().enumerate() {
                let row_y = command_y + line_height * i as f32;
                if text.is_empty() || !visible(row_y, line_height) {
                    continue;
                }
                let runs = if i == 0 {
                    let word = text.trim_start().split_whitespace().next().unwrap_or("");
                    let lead = text.len() - text.trim_start().len();
                    let split = lead + word.len();
                    let mut runs = vec![run(split, &bold_font, green)];
                    if split < text.len() {
                        runs.push(run(text.len() - split, &bold_font, foreground));
                    }
                    runs
                } else {
                    vec![run(text.len(), &bold_font, foreground)]
                };
                let force = Some(cell_width);
                let line = text_system.shape_line(text.clone().into(), font_size, &runs, force);
                texts.push(Text {
                    origin: point(left, row_y),
                    line,
                    height: line_height,
                });
            }

            // The output, only the rows on screen.
            if rows == 0 {
                continue;
            }
            let term = &block.term;
            let grid = term.grid();
            let content = term.renderable_content();
            let mut painter = CellPainter::new(content.colors, theme, cell_width, line_height);
            let default_bg = painter.default_bg;
            let selection = term.selection.as_ref().and_then(|selection| selection.to_range(term));
            let cursor_point = content.cursor.point;
            let show_cursor = block.state == BlockState::Running
                && content.cursor.shape != CursorShape::Hidden
                && (self.cursor_visible || !self.focused);
            let mut cursor_cell: Option<(Point<Pixels>, Option<PaintedCell>)> = None;
            let first = block.first_line();
            let width = columns.min(grid.columns());
            for row in 0..rows {
                let row_y = output_y + line_height * row as f32;
                if !visible(row_y, line_height) {
                    continue;
                }
                let line = first + row as i32;
                let row_origin = point(left, row_y);
                row_key += 1;
                let cells = &grid[line];
                for column in 0..width {
                    let at = GridPoint::new(line, Column(column));
                    let selected = selection.is_some_and(|range| range.contains(at)).then_some(theme.selection);
                    let painted = painter.cell(row_key, row_origin, column, &cells[Column(column)], selected);
                    if show_cursor && at == cursor_point {
                        cursor_cell = Some((point(left + cell_width * column as f32, row_y), painted));
                    }
                }
            }
            let (cell_rects, cell_texts) = painter.finish(&font, font_size, window);
            rects.extend(cell_rects);
            texts.extend(cell_texts.into_iter().map(|(origin, line)| Text {
                origin,
                line,
                height: line_height,
            }));
            for (start, end, active) in search.iter().filter(|m| m.0 == index).map(|m| (m.1, m.2, m.3)) {
                if start.line != end.line || start.line < first || start.line >= first + rows as i32 {
                    continue;
                }
                let row_y = output_y + line_height * (start.line.0 - first.0) as f32;
                let count = end.column.0.saturating_sub(start.column.0) + 1;
                rects.push((
                    Bounds::new(
                        point(left + cell_width * start.column.0 as f32, row_y),
                        size(cell_width * count as f32, line_height),
                    ),
                    search_color(active),
                ));
            }
            // An underline under the file or folder name the mouse is on.
            if let Some(link) = link.as_ref().filter(|link| link.block == index) {
                let row = link.line.0 - first.0;
                let row_y = output_y + line_height * row as f32;
                if row >= 0 && (row as usize) < rows && visible(row_y, line_height) {
                    rects.push((
                        Bounds::new(
                            point(left + cell_width * link.columns.start as f32, row_y + line_height - px(2.)),
                            size(cell_width * link.columns.len() as f32, px(1.)),
                        ),
                        foreground,
                    ));
                }
            }
            if let Some((origin, under)) = cursor_cell {
                cursor = Some(CursorPaint::new(
                    origin,
                    under,
                    cursor_shape(content.cursor.shape, content.mode, settings.cursor_shape),
                    content.colors[NamedColor::Cursor].unwrap_or(theme.cursor),
                    default_bg,
                    self.focused,
                    (&font, font_size, cell_width, line_height),
                    window,
                ));
            }
        }

        let layout = BlocksLayout {
            bounds,
            cell_width,
            line_height,
            columns,
            blocks: geoms,
        };
        self.view.update(cx, |view, _| {
            view.blocks_scroll = scroll;
            view.blocks_total = total;
            view.blocks_layout = Some(layout);
        });

        PrepaintState {
            background: to_hsla(theme.background),
            tints,
            rects,
            texts,
            cursor,
            line_height,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.view.clone()), cx);
        window.paint_quad(fill(bounds, state.background));
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (rect, color) in state.tints.drain(..) {
                window.paint_quad(fill(rect, color));
            }
            for (rect, color) in state.rects.drain(..) {
                window.paint_quad(fill(rect, color));
            }
            if let Some(cursor) = state.cursor.as_ref() {
                cursor.paint_under(window);
            }
            for text in &state.texts {
                let _ = text.line.paint(text.origin, text.height, TextAlign::Left, None, window, cx);
            }
            if let Some(cursor) = state.cursor.take() {
                cursor.paint_over(state.line_height, window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_commands_wrap() {
        assert_eq!(command_rows("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(command_rows("a\n\nb", 4), vec!["a", "", "b"]);
    }
}
