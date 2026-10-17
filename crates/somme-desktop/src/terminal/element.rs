use alacritty_terminal::index::Point as GridPoint;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use gpui_kit::{
    App, BorderStyle, Bounds, Element, ElementId, ElementInputHandler, Entity, Font, FontFallbacks, FontFeatures,
    FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point,
    ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, outline,
    point, px, relative, size,
};

use crate::settings::{CursorShapeSetting, Settings};
use crate::terminal::colors::{self, TerminalTheme, to_hsla};
use crate::terminal::session::{GridSize, TerminalSession};
use crate::terminal::view::{TermLayout, TerminalView};

pub const PADDING: Pixels = px(10.);

/// Glyph sources tried after the chosen font, so icon fonts and emoji show up.
const FALLBACK_FONTS: &[&str] = &[
    "Symbols Nerd Font Mono",
    "Symbols Nerd Font",
    "Hack Nerd Font Mono",
    "JetBrainsMono Nerd Font Mono",
    "MesloLGS NF",
    "Apple Color Emoji",
    "Noto Color Emoji",
    "Segoe UI Emoji",
    "Segoe UI Symbol",
];

pub fn terminal_font(family: &str) -> Font {
    Font {
        family: SharedString::from(family.to_owned()),
        features: FontFeatures::disable_ligatures(),
        fallbacks: Some(FontFallbacks::from_fonts(
            FALLBACK_FONTS.iter().map(|name| (*name).to_owned()).collect(),
        )),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// Measures one cell of the terminal grid for the current font settings.
pub fn cell_metrics(settings: &Settings, window: &Window) -> (Font, Pixels, Pixels, Pixels) {
    let font = terminal_font(&settings.font_family);
    let font_size = px(settings.font_size.clamp(6., 72.));
    let text_system = window.text_system();
    let font_id = text_system.resolve_font(&font);
    let cell_width = text_system
        .advance(font_id, font_size, 'm')
        .map(|advance| advance.width)
        .unwrap_or(font_size * 0.6);
    let line_height = (font_size * settings.line_height.clamp(0.8, 3.)).round();
    (font, font_size, cell_width, line_height)
}

#[derive(Clone, PartialEq)]
struct CellStyle {
    fg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strike: bool,
}

struct Batch {
    line: usize,
    column: usize,
    /// The column right after the last cell in this batch.
    end: usize,
    text: String,
    style: CellStyle,
    wide: bool,
}

struct CursorPaint {
    bounds: Bounds<Pixels>,
    color: Hsla,
    shape: CursorShapeSetting,
    hollow: bool,
    glyph: Option<(Point<Pixels>, ShapedLine)>,
}

pub struct PrepaintState {
    background: Hsla,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<CursorPaint>,
    line_height: Pixels,
    search_rects: Vec<(Bounds<Pixels>, Hsla)>,
}

pub struct TerminalElement {
    view: Entity<TerminalView>,
    session: Entity<TerminalSession>,
    focused: bool,
    cursor_visible: bool,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>, session: Entity<TerminalSession>, focused: bool, cursor_visible: bool) -> Self {
        Self {
            view,
            session,
            focused,
            cursor_visible,
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn cell_style(fg: Rgb, flags: Flags) -> CellStyle {
    CellStyle {
        fg,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(
            Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE,
        ),
        undercurl: flags.contains(Flags::UNDERCURL),
        strike: flags.contains(Flags::STRIKEOUT),
    }
}

fn styled_font(base: &Font, style: &CellStyle) -> Font {
    let mut font = base.clone();
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    font
}

fn text_run(text_len: usize, font: &Font, style: &CellStyle) -> TextRun {
    let color = to_hsla(style.fg);
    TextRun {
        len: text_len,
        font: styled_font(font, style),
        color,
        background_color: None,
        underline: (style.underline || style.undercurl).then_some(UnderlineStyle {
            color: Some(color),
            thickness: px(1.),
            wavy: style.undercurl,
        }),
        strikethrough: style.strike.then_some(StrikethroughStyle {
            color: Some(color),
            thickness: px(1.),
        }),
    }
}

/// Final foreground and background for a cell, after bold, dim, inverse and hidden.
fn cell_colors(fg: Color, bg: Color, flags: Flags, overrides: &alacritty_terminal::term::color::Colors, theme: &TerminalTheme) -> (Rgb, Rgb) {
    let mut fg = fg;
    if flags.contains(Flags::BOLD) {
        fg = colors::brighten(fg);
    }
    if flags.contains(Flags::DIM) {
        fg = colors::dimmed(fg);
    }
    let mut fg = colors::resolve(fg, overrides, theme);
    let mut bg = colors::resolve(bg, overrides, theme);
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

impl Element for TerminalElement {
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
        let theme = colors::theme_named(&settings.terminal_theme);
        let (font, font_size, cell_width, line_height) = cell_metrics(&settings, window);

        let origin = point(bounds.origin.x + PADDING, bounds.origin.y + PADDING);
        let inner_width = (bounds.size.width - PADDING * 2.).max(cell_width);
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
        let layout = TermLayout {
            origin,
            cell_width,
            line_height,
            columns,
            lines,
        };
        self.view.update(cx, |view, _| view.layout = Some(layout));
        let search = self.view.read(cx).search_matches();

        let session = self.session.read(cx);
        let term = session.term();
        let content = term.renderable_content();
        let overrides = content.colors;
        let display_offset = content.display_offset as i32;
        let selection = content.selection;
        let default_bg = colors::resolve(Color::Named(NamedColor::Background), overrides, theme);
        let selection_bg = theme.selection;
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let mode = content.mode;

        let cell_origin = |line: usize, column: usize| -> Point<Pixels> {
            point(origin.x + cell_width * column as f32, origin.y + line_height * line as f32)
        };

        let mut rects: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
        let mut batches: Vec<Batch> = Vec::new();
        let mut current: Option<Batch> = None;
        let mut last_rect: Option<(usize, usize, usize, Rgb)> = None;
        let mut cursor_cell: Option<(char, CellStyle, Rgb, bool)> = None;

        for indexed in content.display_iter {
            let grid_point = indexed.point;
            let line = (grid_point.line.0 + display_offset) as usize;
            let column = grid_point.column.0;
            if line >= lines || column >= columns {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            let width = if wide { 2 } else { 1 };
            let (fg, mut bg) = cell_colors(cell.fg, cell.bg, cell.flags, overrides, theme);
            let selected = selection.is_some_and(|range| range.contains(grid_point));
            if selected {
                bg = selection_bg;
            }
            if grid_point == cursor_point {
                cursor_cell = Some((cell.c, cell_style(fg, cell.flags), bg, wide));
            }

            if bg != default_bg {
                match last_rect.as_mut() {
                    Some((l, start, end, color)) if *l == line && *end == column && *color == bg => {
                        *end = column + width;
                    }
                    _ => {
                        if let Some((l, start, end, color)) = last_rect.take() {
                            rects.push((
                                Bounds::new(cell_origin(l, start), size(cell_width * (end - start) as f32, line_height)),
                                to_hsla(color),
                            ));
                        }
                        last_rect = Some((line, column, column + width, bg));
                    }
                }
            }

            let style = cell_style(fg, cell.flags);
            let blank = cell.c == ' ' || cell.c == '\0' || cell.c == '\t';
            let decorated = style.underline || style.undercurl || style.strike;
            let continues = current.as_ref().is_some_and(|batch| {
                !batch.wide
                    && !wide
                    && batch.line == line
                    && batch.style == style
                    && batch.end == column
            });
            if blank && !decorated && !continues {
                if let Some(batch) = current.take() {
                    batches.push(batch);
                }
                continue;
            }
            if !continues && let Some(batch) = current.take() {
                batches.push(batch);
            }
            let batch = current.get_or_insert_with(|| Batch {
                line,
                column,
                end: column,
                text: String::new(),
                style: style.clone(),
                wide,
            });
            batch.text.push(if blank { ' ' } else { cell.c });
            batch.end = column + width;
            if let Some(extra) = cell.zerowidth() {
                batch.text.extend(extra.iter().copied());
            }
            if wide {
                batches.push(current.take().expect("batch was just created"));
            }
        }
        if let Some(batch) = current.take() {
            batches.push(batch);
        }
        if let Some((l, start, end, color)) = last_rect.take() {
            rects.push((
                Bounds::new(cell_origin(l, start), size(cell_width * (end - start) as f32, line_height)),
                to_hsla(color),
            ));
        }

        let text_system = window.text_system().clone();
        let lines_out: Vec<(Point<Pixels>, ShapedLine)> = batches
            .into_iter()
            .filter_map(|batch| {
                let decorated = batch.style.underline || batch.style.undercurl || batch.style.strike;
                let text = if decorated { batch.text } else { batch.text.trim_end().to_owned() };
                if text.is_empty() {
                    return None;
                }
                let run = text_run(text.len(), &font, &batch.style);
                let force = if batch.wide { cell_width * 2. } else { cell_width };
                let shaped = text_system.shape_line(text.into(), font_size, &[run], Some(force));
                Some((cell_origin(batch.line, batch.column), shaped))
            })
            .collect();

        let cursor_line = cursor_point.line.0 + display_offset;
        let show_cursor = cursor_shape != CursorShape::Hidden
            && (0..lines as i32).contains(&cursor_line)
            && cursor_point.column.0 < columns
            && (self.cursor_visible || !self.focused);
        let cursor = show_cursor.then(|| {
            let line = cursor_line as usize;
            let column = cursor_point.column.0;
            let (ch, style, _, wide) = cursor_cell.clone().unwrap_or((
                ' ',
                cell_style(theme.foreground, Flags::empty()),
                default_bg,
                false,
            ));
            let cursor_color = overrides[NamedColor::Cursor].unwrap_or(theme.cursor);
            let width = if wide { cell_width * 2. } else { cell_width };
            let shape = match cursor_shape {
                CursorShape::Beam => CursorShapeSetting::Bar,
                CursorShape::Underline => CursorShapeSetting::Underline,
                _ if mode.contains(TermMode::ALT_SCREEN) => CursorShapeSetting::Block,
                _ => settings.cursor_shape,
            };
            let glyph = (shape == CursorShapeSetting::Block && self.focused && ch != ' ' && ch != '\0').then(|| {
                let inverted = CellStyle {
                    fg: default_bg,
                    ..style
                };
                let text = ch.to_string();
                let run = text_run(text.len(), &font, &inverted);
                let shaped = text_system.shape_line(text.into(), font_size, &[run], Some(width));
                (cell_origin(line, column), shaped)
            });
            CursorPaint {
                bounds: Bounds::new(cell_origin(line, column), size(width, line_height)),
                color: to_hsla(cursor_color),
                shape,
                hollow: !self.focused,
                glyph,
            }
        });

        let search_rects = search
            .into_iter()
            .filter_map(|(start, end, active)| {
                let line = start.line.0 + display_offset;
                if !(0..lines as i32).contains(&line) || start.line != end.line {
                    return None;
                }
                let columns = end.column.0.saturating_sub(start.column.0) + 1;
                let color = if active {
                    gpui_kit::hsla(0.11, 0.95, 0.55, 0.55)
                } else {
                    gpui_kit::hsla(0.13, 0.9, 0.6, 0.28)
                };
                Some((
                    Bounds::new(cell_origin(line as usize, start.column.0), size(cell_width * columns as f32, line_height)),
                    color,
                ))
            })
            .collect();

        PrepaintState {
            background: to_hsla(default_bg),
            rects,
            lines: lines_out,
            cursor,
            line_height,
            search_rects,
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
        for (rect, color) in state.rects.drain(..) {
            window.paint_quad(fill(rect, color));
        }
        for (rect, color) in state.search_rects.drain(..) {
            window.paint_quad(fill(rect, color));
        }

        if let Some(cursor) = state.cursor.as_ref()
            && !cursor.hollow
            && cursor.shape == CursorShapeSetting::Block
        {
            window.paint_quad(fill(cursor.bounds, cursor.color));
        }

        for (origin, line) in &state.lines {
            let _ = line.paint(*origin, state.line_height, TextAlign::Left, None, window, cx);
        }

        if let Some(cursor) = state.cursor.take() {
            let b = cursor.bounds;
            if cursor.hollow {
                window.paint_quad(outline(b, cursor.color, BorderStyle::Solid));
            } else {
                match cursor.shape {
                    CursorShapeSetting::Block => {
                        if let Some((origin, glyph)) = cursor.glyph {
                            let _ = glyph.paint(origin, state.line_height, TextAlign::Left, None, window, cx);
                        }
                    }
                    CursorShapeSetting::Bar => {
                        window.paint_quad(fill(Bounds::new(b.origin, size(px(2.), b.size.height)), cursor.color));
                    }
                    CursorShapeSetting::Underline => {
                        let y = b.origin.y + b.size.height - px(2.);
                        window.paint_quad(fill(Bounds::new(point(b.origin.x, y), size(b.size.width, px(2.))), cursor.color));
                    }
                }
            }
        }
    }
}

/// A cell on screen, converted to a grid point that accounts for scrollback.
pub fn grid_point(layout: &TermLayout, position: Point<Pixels>, display_offset: usize) -> (GridPoint, alacritty_terminal::index::Side) {
    let x = (position.x - layout.origin.x).max(px(0.));
    let y = (position.y - layout.origin.y).max(px(0.));
    let column_f = x / layout.cell_width;
    let column = (column_f.floor() as usize).min(layout.columns.saturating_sub(1));
    let line = ((y / layout.line_height).floor() as usize).min(layout.lines.saturating_sub(1));
    let side = if column_f.fract() > 0.5 {
        alacritty_terminal::index::Side::Right
    } else {
        alacritty_terminal::index::Side::Left
    };
    (
        GridPoint::new(
            alacritty_terminal::index::Line(line as i32 - display_offset as i32),
            alacritty_terminal::index::Column(column),
        ),
        side,
    )
}
