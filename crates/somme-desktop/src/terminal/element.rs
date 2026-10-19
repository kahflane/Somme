use alacritty_terminal::index::Point as GridPoint;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
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
    // The bundled font first, so letters the chosen font lacks still fit the grid.
    let fallbacks = std::iter::once(crate::fonts::BUNDLED).chain(FALLBACK_FONTS.iter().copied());
    Font {
        family: SharedString::from(family.to_owned()),
        features: FontFeatures::disable_ligatures(),
        fallbacks: Some(FontFallbacks::from_fonts(fallbacks.map(str::to_owned).collect())),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// Measures one cell of the terminal grid for the current font settings.
pub fn cell_metrics(settings: &Settings, window: &Window) -> (Font, Pixels, Pixels, Pixels) {
    let font = terminal_font(&settings.mono_font());
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
pub struct CellStyle {
    fg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strike: bool,
}

struct Batch {
    /// Tells rows apart, so a batch never spans two of them.
    row: usize,
    origin: Point<Pixels>,
    /// The column right after the last cell in this batch.
    end: usize,
    text: String,
    style: CellStyle,
    wide: bool,
}

/// A background run on one row: row, where it starts, first column, end column, color.
type RectRun = (usize, Point<Pixels>, usize, usize, Rgb);

/// Turns grid cells into background rectangles and shaped runs of text.
pub struct CellPainter<'a> {
    overrides: &'a Colors,
    theme: &'a TerminalTheme,
    pub default_bg: Rgb,
    cell_width: Pixels,
    line_height: Pixels,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    batches: Vec<Batch>,
    current: Option<Batch>,
    last_rect: Option<RectRun>,
}

/// What a painted cell looks like, kept for drawing the cursor over it.
#[derive(Clone)]
pub struct PaintedCell {
    pub c: char,
    pub style: CellStyle,
    pub wide: bool,
}

impl<'a> CellPainter<'a> {
    pub fn new(overrides: &'a Colors, theme: &'a TerminalTheme, cell_width: Pixels, line_height: Pixels) -> Self {
        Self {
            overrides,
            theme,
            default_bg: colors::resolve(Color::Named(NamedColor::Background), overrides, theme),
            cell_width,
            line_height,
            rects: Vec::new(),
            batches: Vec::new(),
            current: None,
            last_rect: None,
        }
    }

    fn flush_rect(&mut self) {
        if let Some((_, origin, start, end, color)) = self.last_rect.take() {
            self.rects.push((
                Bounds::new(
                    point(origin.x + self.cell_width * start as f32, origin.y),
                    size(self.cell_width * (end - start) as f32, self.line_height),
                ),
                to_hsla(color),
            ));
        }
    }

    /// Adds one cell. `row` tells rows apart and `row_origin` is where the row's first column sits.
    pub fn cell(&mut self, row: usize, row_origin: Point<Pixels>, column: usize, cell: &Cell, selected: Option<Rgb>) -> Option<PaintedCell> {
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            return None;
        }
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2 } else { 1 };
        let (fg, mut bg) = cell_colors(cell.fg, cell.bg, cell.flags, self.overrides, self.theme);
        if let Some(selection) = selected {
            bg = selection;
        }

        if bg != self.default_bg {
            match self.last_rect.as_mut() {
                Some((r, _, _, end, color)) if *r == row && *end == column && *color == bg => *end = column + width,
                _ => {
                    self.flush_rect();
                    self.last_rect = Some((row, row_origin, column, column + width, bg));
                }
            }
        }

        let style = cell_style(fg, cell.flags);
        let painted = PaintedCell {
            c: cell.c,
            style: style.clone(),
            wide,
        };
        let blank = cell.c == ' ' || cell.c == '\0' || cell.c == '\t';
        let decorated = style.underline || style.undercurl || style.strike;
        let continues = self.current.as_ref().is_some_and(|batch| {
            !batch.wide && !wide && batch.row == row && batch.style == style && batch.end == column
        });
        if blank && !decorated && !continues {
            if let Some(batch) = self.current.take() {
                self.batches.push(batch);
            }
            return Some(painted);
        }
        if !continues && let Some(batch) = self.current.take() {
            self.batches.push(batch);
        }
        let cell_width = self.cell_width;
        let batch = self.current.get_or_insert_with(|| Batch {
            row,
            origin: point(row_origin.x + cell_width * column as f32, row_origin.y),
            end: column,
            text: String::new(),
            style,
            wide,
        });
        batch.text.push(if blank { ' ' } else { cell.c });
        batch.end = column + width;
        if let Some(extra) = cell.zerowidth() {
            batch.text.extend(extra.iter().copied());
        }
        if wide {
            self.batches.push(self.current.take().expect("batch was just created"));
        }
        Some(painted)
    }

    /// Shapes the collected text. Returns the background rectangles and the text runs.
    pub fn finish(mut self, font: &Font, font_size: Pixels, window: &Window) -> (Vec<(Bounds<Pixels>, Hsla)>, Vec<(Point<Pixels>, ShapedLine)>) {
        if let Some(batch) = self.current.take() {
            self.batches.push(batch);
        }
        self.flush_rect();
        let text_system = window.text_system().clone();
        let cell_width = self.cell_width;
        let lines = self
            .batches
            .into_iter()
            .filter_map(|batch| {
                let decorated = batch.style.underline || batch.style.undercurl || batch.style.strike;
                let text = if decorated { batch.text } else { batch.text.trim_end().to_owned() };
                if text.is_empty() {
                    return None;
                }
                let run = text_run(text.len(), font, &batch.style);
                let force = if batch.wide { cell_width * 2. } else { cell_width };
                let shaped = text_system.shape_line(text.into(), font_size, &[run], Some(force));
                Some((batch.origin, shaped))
            })
            .collect();
        (self.rects, lines)
    }
}

pub struct CursorPaint {
    bounds: Bounds<Pixels>,
    color: Hsla,
    shape: CursorShapeSetting,
    hollow: bool,
    glyph: Option<(Point<Pixels>, ShapedLine)>,
}

impl CursorPaint {
    /// The cursor over `under`, the cell it sits on.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        origin: Point<Pixels>,
        under: Option<PaintedCell>,
        shape: CursorShapeSetting,
        color: Rgb,
        background: Rgb,
        focused: bool,
        metrics: (&Font, Pixels, Pixels, Pixels),
        window: &Window,
    ) -> Self {
        let (font, font_size, cell_width, line_height) = metrics;
        let wide = under.as_ref().is_some_and(|cell| cell.wide);
        let width = if wide { cell_width * 2. } else { cell_width };
        let glyph = under
            .filter(|cell| shape == CursorShapeSetting::Block && focused && cell.c != ' ' && cell.c != '\0')
            .map(|cell| {
                let inverted = CellStyle {
                    fg: background,
                    ..cell.style
                };
                let text = cell.c.to_string();
                let run = text_run(text.len(), font, &inverted);
                let shaped = window.text_system().shape_line(text.into(), font_size, &[run], Some(width));
                (origin, shaped)
            });
        Self {
            bounds: Bounds::new(origin, size(width, line_height)),
            color: to_hsla(color),
            shape,
            hollow: !focused,
            glyph,
        }
    }

    /// Paints the part that goes under the text.
    pub fn paint_under(&self, window: &mut Window) {
        if !self.hollow && self.shape == CursorShapeSetting::Block {
            window.paint_quad(fill(self.bounds, self.color));
        }
    }

    /// Paints the part that goes over the text.
    pub fn paint_over(self, line_height: Pixels, window: &mut Window, cx: &mut App) {
        let b = self.bounds;
        if self.hollow {
            window.paint_quad(outline(b, self.color, BorderStyle::Solid));
            return;
        }
        match self.shape {
            CursorShapeSetting::Block => {
                if let Some((origin, glyph)) = self.glyph {
                    let _ = glyph.paint(origin, line_height, TextAlign::Left, None, window, cx);
                }
            }
            CursorShapeSetting::Bar => {
                window.paint_quad(fill(Bounds::new(b.origin, size(px(2.), b.size.height)), self.color));
            }
            CursorShapeSetting::Underline => {
                let y = b.origin.y + b.size.height - px(2.);
                window.paint_quad(fill(Bounds::new(point(b.origin.x, y), size(b.size.width, px(2.))), self.color));
            }
        }
    }
}

/// The cursor shape to draw, from what the program asked for and the setting.
pub fn cursor_shape(asked: CursorShape, mode: TermMode, setting: CursorShapeSetting) -> CursorShapeSetting {
    match asked {
        CursorShape::Beam => CursorShapeSetting::Bar,
        CursorShape::Underline => CursorShapeSetting::Underline,
        _ if mode.contains(TermMode::ALT_SCREEN) => CursorShapeSetting::Block,
        _ => setting,
    }
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

pub fn cell_style(fg: Rgb, flags: Flags) -> CellStyle {
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

pub fn text_run(text_len: usize, font: &Font, style: &CellStyle) -> TextRun {
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
fn cell_colors(fg: Color, bg: Color, flags: Flags, overrides: &Colors, theme: &TerminalTheme) -> (Rgb, Rgb) {
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
        let theme = colors::active_theme(cx);
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
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let mode = content.mode;

        let mut painter = CellPainter::new(overrides, theme, cell_width, line_height);
        let default_bg = painter.default_bg;
        let mut cursor_cell: Option<PaintedCell> = None;
        for indexed in content.display_iter {
            let grid_point = indexed.point;
            let line = (grid_point.line.0 + display_offset) as usize;
            let column = grid_point.column.0;
            if line >= lines || column >= columns {
                continue;
            }
            let selected = selection.is_some_and(|range| range.contains(grid_point)).then_some(theme.selection);
            let row_origin = point(origin.x, origin.y + line_height * line as f32);
            let painted = painter.cell(line, row_origin, column, indexed.cell, selected);
            if grid_point == cursor_point {
                cursor_cell = painted;
            }
        }
        let (rects, lines_out) = painter.finish(&font, font_size, window);

        let cell_origin = |line: usize, column: usize| -> Point<Pixels> {
            point(origin.x + cell_width * column as f32, origin.y + line_height * line as f32)
        };
        let cursor_line = cursor_point.line.0 + display_offset;
        let show_cursor = cursor_shape != CursorShape::Hidden
            && (0..lines as i32).contains(&cursor_line)
            && cursor_point.column.0 < columns
            && (self.cursor_visible || !self.focused);
        let cursor = show_cursor.then(|| {
            CursorPaint::new(
                cell_origin(cursor_line as usize, cursor_point.column.0),
                cursor_cell,
                self::cursor_shape(cursor_shape, mode, settings.cursor_shape),
                overrides[NamedColor::Cursor].unwrap_or(theme.cursor),
                default_bg,
                self.focused,
                (&font, font_size, cell_width, line_height),
                window,
            )
        });

        let search_rects = search
            .into_iter()
            .filter_map(|(start, end, active)| {
                let line = start.line.0 + display_offset;
                if !(0..lines as i32).contains(&line) || start.line != end.line {
                    return None;
                }
                let columns = end.column.0.saturating_sub(start.column.0) + 1;
                Some((
                    Bounds::new(cell_origin(line as usize, start.column.0), size(cell_width * columns as f32, line_height)),
                    search_color(active),
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

        if let Some(cursor) = state.cursor.as_ref() {
            cursor.paint_under(window);
        }

        for (origin, line) in &state.lines {
            let _ = line.paint(*origin, state.line_height, TextAlign::Left, None, window, cx);
        }

        if let Some(cursor) = state.cursor.take() {
            cursor.paint_over(state.line_height, window, cx);
        }
    }
}

/// The highlight behind a search match.
pub fn search_color(active: bool) -> Hsla {
    if active {
        gpui_kit::hsla(0.11, 0.95, 0.55, 0.55)
    } else {
        gpui_kit::hsla(0.13, 0.9, 0.6, 0.28)
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
