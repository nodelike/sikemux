//! Draws a terminal's grid of cells in GPUI, from cells emulated anywhere.
//!
//! - [`Grid`] is what the drawing reads: rows of styled spans, a cursor, highlights
//!   and text an input method is composing.
//! - [`Metrics`] measures a font's cell and maps between points and cells.
//! - [`Layout`] shapes a grid in a canvas prepaint and paints it.

mod shapes;

use std::ops::Range;

use gpui::{
    fill, point, px, size, App, Bounds, Font, FontStyle, FontWeight, Hsla, LetterSpacing, Path,
    Pixels, Point, ShapedLine, SharedString, Size, TextAlign, TextRun, UnderlineStyle, Window,
};

/// The cells a terminal shows, row by row from the top of the drawn area.
pub trait Grid {
    /// Columns and rows.
    fn size(&self) -> (u16, u16);

    /// Calls `each` with the row's spans, left to right. Columns no span
    /// covers are blank.
    fn row(&self, row: u16, each: &mut dyn FnMut(Span<'_>));

    fn cursor(&self) -> Option<Cursor> {
        None
    }

    /// Calls `each` with the cells to tint behind the text, such as a
    /// selection or search matches.
    fn highlights(&self, _each: &mut dyn FnMut(Highlight)) {}

    /// Text an input method is composing, drawn over the grid until it is committed.
    fn marked(&self) -> Option<Marked<'_>> {
        None
    }
}

/// Text in one look, starting at a column.
#[derive(Clone, Copy)]
pub struct Span<'a> {
    pub column: u16,
    pub text: &'a str,
    /// Each character takes two columns.
    pub wide: bool,
    /// The whole text is one cell: a character with marks combined onto it.
    pub cluster: bool,
    pub look: Look,
}

impl Span<'_> {
    /// Columns the span covers.
    pub fn width(&self) -> u16 {
        let cells = if self.cluster {
            1
        } else {
            self.text.chars().count() as u16
        };
        cells * if self.wide { 2 } else { 1 }
    }
}

/// How a span looks, its colours already resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub fg: Hsla,
    /// `None` leaves the cells clear, so whatever is behind the grid shows.
    pub bg: Option<Hsla>,
    pub bold: bool,
    pub italic: bool,
    pub underline: Option<Underline>,
    /// The underline's own colour; the text's when `None`.
    pub underline_color: Option<Hsla>,
    pub strike: bool,
}

impl Look {
    pub fn plain(fg: Hsla) -> Self {
        Self {
            fg,
            bg: None,
            bold: false,
            italic: false,
            underline: None,
            underline_color: None,
            strike: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underline {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cursor {
    pub row: u16,
    pub column: u16,
    pub shape: CursorShape,
    pub color: Hsla,
    /// The colour of the character under a block cursor.
    pub text: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    /// An outlined block, as drawn while the terminal is not focused.
    Hollow,
    Underline,
    Beam,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub row: u16,
    pub columns: Range<u16>,
    pub color: Hsla,
    /// A line under the text, such as a hovered link, instead of a tint behind it.
    pub underline: bool,
}

/// Composed text starting at a cell, on its own opaque ground.
#[derive(Clone, Copy)]
pub struct Marked<'a> {
    pub row: u16,
    pub column: u16,
    pub text: &'a str,
    pub fg: Hsla,
    pub bg: Hsla,
}

/// A font's cell, snapped to whole device pixels so rows and columns tile.
#[derive(Clone, Debug)]
pub struct Metrics {
    fonts: [Font; 4],
    pub font_size: Pixels,
    pub cell: Size<Pixels>,
    /// From the top of a cell to the text's baseline.
    baseline: Pixels,
    /// The width of thin lines: underlines, box drawing, the beam cursor.
    stroke: Pixels,
}

impl Metrics {
    /// Measures `font` at `font_size`, spacing its letters a whole cell apart.
    /// Rows are `line_height` times the size tall, or the font's own height
    /// when that is taller.
    pub fn new(font: Font, font_size: Pixels, line_height: f32, window: &Window) -> Self {
        let scale = window.scale_factor();
        let snap = |value: f32| px((value * scale).round().max(1.) / scale);
        let text = window.text_system();
        let id = text.resolve_font(&font);
        let advance = text
            .advance(id, font_size, 'W')
            .map_or(f32::from(font_size) * 0.6, |size| size.width.into());
        let ascent = f32::from(text.ascent(id, font_size));
        let descent = f32::from(text.descent(id, font_size)).abs();
        let height = ((ascent + descent) * scale)
            .ceil()
            .max((f32::from(font_size) * line_height * scale).round())
            / scale;
        let width = snap(advance);
        let font = Font {
            letter_spacing: LetterSpacing((f32::from(width) - advance) / f32::from(font_size)),
            ..font
        };
        let bold = Font {
            weight: FontWeight::BOLD,
            ..font.clone()
        };
        let italic = Font {
            style: FontStyle::Italic,
            ..font.clone()
        };
        let bold_italic = Font {
            style: FontStyle::Italic,
            ..bold.clone()
        };
        Self {
            fonts: [font, bold, italic, bold_italic],
            font_size,
            cell: size(width, px(height)),
            baseline: snap((height - ascent - descent) / 2. + ascent),
            stroke: snap(f32::from(font_size) / 12.),
        }
    }

    /// The columns and rows that fit in `area`.
    pub fn fit(&self, area: Size<Pixels>) -> (u16, u16) {
        let count = |length: Pixels, cell: Pixels| {
            (length / cell).floor().clamp(1., f32::from(u16::MAX)) as u16
        };
        (
            count(area.width, self.cell.width),
            count(area.height, self.cell.height),
        )
    }

    /// The cell under `position`, measured from the grid's top left; points
    /// past an edge give the cell at that edge.
    pub fn cell_at(&self, position: Point<Pixels>, (columns, rows): (u16, u16)) -> (u16, u16) {
        let index = |offset: Pixels, cell: Pixels, count: u16| {
            ((offset / cell).floor().max(0.) as u16).min(count.saturating_sub(1))
        };
        (
            index(position.x, self.cell.width, columns),
            index(position.y, self.cell.height, rows),
        )
    }

    fn font(&self, look: &Look) -> &Font {
        &self.fonts[usize::from(look.bold) + 2 * usize::from(look.italic)]
    }

    fn cells(&self, origin: Point<Pixels>, row: u16, column: u16, width: u16) -> Bounds<Pixels> {
        Bounds::new(
            point(
                origin.x + self.cell.width * f32::from(column),
                origin.y + self.cell.height * f32::from(row),
            ),
            size(self.cell.width * f32::from(width), self.cell.height),
        )
    }
}

enum Mark {
    Fill(Bounds<Pixels>, Hsla),
    Path(Path<Pixels>, Hsla),
    Wavy(Point<Pixels>, Pixels, UnderlineStyle),
}

impl Mark {
    fn paint(self, window: &mut Window) {
        match self {
            Mark::Fill(bounds, color) => window.paint_quad(fill(bounds, color)),
            Mark::Path(path, color) => window.paint_path(path, color),
            Mark::Wavy(origin, width, style) => window.paint_underline(origin, width, &style),
        }
    }
}

/// A grid shaped for one frame: made in a canvas prepaint, painted in its paint.
pub struct Layout {
    line_height: Pixels,
    /// Backgrounds, highlights and a block cursor, drawn under the text.
    under: Vec<Mark>,
    block: Option<Mark>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    /// Box drawing, decorations and the other cursors, drawn over it.
    over: Vec<Mark>,
    cursor_bounds: Option<Bounds<Pixels>>,
    marked: Option<(Bounds<Pixels>, Hsla, ShapedLine)>,
}

/// Text placed on the grid but not yet shaped: cells of one width, side by side.
struct Pending {
    text: String,
    runs: Vec<TextRun>,
    column: u16,
    end: u16,
    force: Option<Pixels>,
}

impl Layout {
    pub fn new(
        grid: &impl Grid,
        origin: Point<Pixels>,
        metrics: &Metrics,
        window: &mut Window,
    ) -> Self {
        let mut layout = Layout {
            line_height: metrics.cell.height,
            under: Vec::new(),
            block: None,
            lines: Vec::new(),
            over: Vec::new(),
            cursor_bounds: None,
            marked: None,
        };
        let (columns, rows) = grid.size();
        let cursor = grid
            .cursor()
            .filter(|cursor| cursor.row < rows && cursor.column < columns);
        for row in 0..rows {
            let cursor = cursor.filter(|cursor| cursor.row == row);
            let block = cursor.filter(|cursor| cursor.shape == CursorShape::Block);
            let mut cursor_width = 1;
            let mut pending = Pending {
                text: String::new(),
                runs: Vec::new(),
                column: 0,
                end: 0,
                force: Some(metrics.cell.width),
            };
            grid.row(row, &mut |span| {
                let width = span.width();
                if span.column >= columns {
                    return;
                }
                let width = width.min(columns - span.column);
                let bounds = metrics.cells(origin, row, span.column, width);
                if let Some(bg) = span.look.bg {
                    layout.background(bounds, bg);
                }
                layout.decorate(&span, bounds, metrics);
                if span.wide || span.cluster {
                    layout.shape(&mut pending, metrics, row, origin, window);
                    if cursor.is_some_and(|cursor| {
                        (span.column..span.column + width).contains(&cursor.column)
                    }) {
                        cursor_width = if span.wide { 2 } else { 1 };
                    }
                    let mut alone = Pending {
                        text: String::new(),
                        runs: Vec::new(),
                        column: span.column,
                        end: span.column,
                        force: span.wide.then(|| metrics.cell.width * 2.),
                    };
                    let color = match block {
                        Some(block)
                            if (span.column..span.column + width).contains(&block.column) =>
                        {
                            block.text
                        }
                        _ => span.look.fg,
                    };
                    alone.push(span.text, metrics.font(&span.look), color);
                    layout.shape(&mut alone, metrics, row, origin, window);
                    return;
                }
                let font = metrics.font(&span.look);
                let mut buffer = [0u8; 4];
                for (column, character) in (span.column..).zip(span.text.chars()) {
                    if column >= columns {
                        break;
                    }
                    if pending.text.is_empty() {
                        pending.column = column;
                        pending.end = column;
                    } else if column > pending.end {
                        pending.pad(column - pending.end);
                    }
                    let color = match block {
                        Some(block) if block.column == column => block.text,
                        _ => span.look.fg,
                    };
                    let cell = metrics.cells(origin, row, column, 1);
                    if shapes::draw(character, cell, metrics.stroke, color, &mut layout.over) {
                        pending.push(" ", font, color);
                    } else {
                        pending.push(character.encode_utf8(&mut buffer), font, color);
                    }
                }
            });
            layout.shape(&mut pending, metrics, row, origin, window);
            if let Some(cursor) = cursor {
                let cell = metrics.cells(origin, row, cursor.column, cursor_width);
                layout.cursor_bounds = Some(cell);
                layout.cursor(cursor, cell, metrics.stroke);
            }
        }
        let mut highlights = Vec::new();
        grid.highlights(&mut |highlight| {
            if highlight.row < rows && highlight.columns.start < columns {
                let end = highlight.columns.end.min(columns);
                let width = end.saturating_sub(highlight.columns.start);
                let cells = metrics.cells(origin, highlight.row, highlight.columns.start, width);
                if highlight.underline {
                    let y = cells.origin.y
                        + (metrics.cell.height - metrics.stroke)
                            .min(metrics.baseline + metrics.stroke);
                    let line = Bounds::new(
                        point(cells.origin.x, y),
                        size(cells.size.width, metrics.stroke),
                    );
                    layout.over.push(Mark::Fill(line, highlight.color));
                } else {
                    highlights.push(Mark::Fill(cells, highlight.color));
                }
            }
        });
        layout.under.extend(highlights);
        layout.under.extend(layout.block.take());
        if let Some(marked) = grid
            .marked()
            .filter(|marked| marked.row < rows && !marked.text.is_empty())
        {
            let run = TextRun {
                len: marked.text.len(),
                font: metrics.fonts[0].clone(),
                color: marked.fg,
                background_color: None,
                underline: Some(UnderlineStyle {
                    thickness: metrics.stroke,
                    color: Some(marked.fg),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(marked.text.to_owned()),
                metrics.font_size,
                &[run],
                None,
            );
            let mut ground = metrics.cells(origin, marked.row, marked.column, 1);
            ground.size.width = ground.size.width.max(shaped.width);
            layout.marked = Some((ground, marked.bg, shaped));
        }
        layout
    }

    /// Paints a span's background, joining it to the one before when they touch in the same colour.
    fn background(&mut self, bounds: Bounds<Pixels>, color: Hsla) {
        if let Some(Mark::Fill(last, last_color)) = self.under.last_mut() {
            if *last_color == color
                && last.origin.y == bounds.origin.y
                && last.right() == bounds.origin.x
            {
                last.size.width += bounds.size.width;
                return;
            }
        }
        self.under.push(Mark::Fill(bounds, color));
    }

    fn decorate(&mut self, span: &Span<'_>, bounds: Bounds<Pixels>, metrics: &Metrics) {
        let stroke = metrics.stroke;
        let width = bounds.size.width;
        if span.look.strike {
            let y = bounds.origin.y + (metrics.cell.height / 2.).round();
            self.over.push(Mark::Fill(
                Bounds::new(point(bounds.origin.x, y), size(width, stroke)),
                span.look.fg,
            ));
        }
        let Some(underline) = span.look.underline else {
            return;
        };
        let color = span.look.underline_color.unwrap_or(span.look.fg);
        let y = bounds.origin.y + (metrics.cell.height - stroke).min(metrics.baseline + stroke);
        let line = |x: Pixels, y: Pixels, length: Pixels| {
            Mark::Fill(Bounds::new(point(x, y), size(length, stroke)), color)
        };
        match underline {
            Underline::Single => self.over.push(line(bounds.origin.x, y, width)),
            Underline::Double => {
                self.over.push(line(bounds.origin.x, y - stroke, width));
                self.over.push(line(bounds.origin.x, y + stroke, width));
            }
            Underline::Curly => self.over.push(Mark::Wavy(
                point(bounds.origin.x, y),
                width,
                UnderlineStyle {
                    thickness: stroke,
                    color: Some(color),
                    wavy: true,
                },
            )),
            Underline::Dotted | Underline::Dashed => {
                let dash = if underline == Underline::Dotted {
                    stroke
                } else {
                    metrics.cell.width / 2.
                };
                let mut x = bounds.origin.x;
                while x < bounds.right() {
                    self.over.push(line(x, y, dash.min(bounds.right() - x)));
                    x += dash * 2.;
                }
            }
        }
    }

    fn shape(
        &mut self,
        pending: &mut Pending,
        metrics: &Metrics,
        row: u16,
        origin: Point<Pixels>,
        window: &mut Window,
    ) {
        if pending.text.is_empty() {
            return;
        }
        let text = SharedString::from(std::mem::take(&mut pending.text));
        let shaped =
            window
                .text_system()
                .shape_line(text, metrics.font_size, &pending.runs, pending.force);
        pending.runs.clear();
        self.lines
            .push((metrics.cells(origin, row, pending.column, 1).origin, shaped));
    }

    fn cursor(&mut self, cursor: Cursor, cell: Bounds<Pixels>, stroke: Pixels) {
        let Bounds {
            origin,
            size: cell_size,
        } = cell;
        let bar = |x: Pixels, y: Pixels, width: Pixels, height: Pixels| {
            Mark::Fill(Bounds::new(point(x, y), size(width, height)), cursor.color)
        };
        match cursor.shape {
            CursorShape::Block => self.block = Some(Mark::Fill(cell, cursor.color)),
            CursorShape::Beam => {
                self.over
                    .push(bar(origin.x, origin.y, stroke * 2., cell_size.height))
            }
            CursorShape::Underline => self.over.push(bar(
                origin.x,
                cell.bottom() - stroke * 2.,
                cell_size.width,
                stroke * 2.,
            )),
            CursorShape::Hollow => {
                self.over
                    .push(bar(origin.x, origin.y, cell_size.width, stroke));
                self.over.push(bar(
                    origin.x,
                    cell.bottom() - stroke,
                    cell_size.width,
                    stroke,
                ));
                self.over
                    .push(bar(origin.x, origin.y, stroke, cell_size.height));
                self.over.push(bar(
                    cell.right() - stroke,
                    origin.y,
                    stroke,
                    cell_size.height,
                ));
            }
        }
    }

    pub fn paint(self, window: &mut Window, cx: &mut App) {
        for mark in self.under {
            mark.paint(window);
        }
        for (origin, line) in self.lines {
            let _ = line.paint(origin, self.line_height, TextAlign::Left, None, window, cx);
        }
        for mark in self.over {
            mark.paint(window);
        }
        if let Some((ground, bg, line)) = self.marked {
            window.paint_quad(fill(ground, bg));
            let _ = line.paint(
                ground.origin,
                self.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }

    /// The cursor's cell, where an input method places its candidates.
    pub fn cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        self.cursor_bounds
    }
}

impl Pending {
    fn push(&mut self, text: &str, font: &Font, color: Hsla) {
        self.text.push_str(text);
        self.end += 1;
        if let Some(last) = self.runs.last_mut() {
            if last.color == color && last.font == *font {
                last.len += text.len();
                return;
            }
        }
        self.runs.push(TextRun {
            len: text.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }

    fn pad(&mut self, columns: u16) {
        self.text
            .extend(std::iter::repeat_n(' ', usize::from(columns)));
        self.end += columns;
        if let Some(last) = self.runs.last_mut() {
            last.len += usize::from(columns);
        }
    }
}
