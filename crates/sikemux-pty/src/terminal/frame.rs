use std::collections::HashMap;

use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape};
use sikemux_wire::grid::{
    self as wire, Color, Cursor, FrameHeader, FrameWriter, SpanStyle, FRAME_RESET,
};

use super::history::{self, LineWriter};
use super::Terminal;

/// What one client was last sent: the frame after which it holds exactly
/// these rows. The next frame turns them into the terminal's current rows.
#[derive(Clone, Debug, Default)]
pub struct View {
    frame: u32,
    version: u64,
    rows: Vec<u64>,
    cols: u16,
    palette_version: Option<u32>,
}

impl View {
    /// The number of the last frame this view was sent.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// The terminal's version that frame brought it to.
    pub fn version(&self) -> u64 {
        self.version
    }
}

/// Each screen row's content hash, kept for the version it was taken at.
#[derive(Default)]
pub(super) struct Hashes {
    version: u64,
    rows: Vec<u64>,
    blank: u64,
}

const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::ITALIC)
    .union(Flags::DIM)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT)
    .union(Flags::ALL_UNDERLINES);

fn style_flags(flags: Flags) -> u16 {
    let mut style = 0;
    for (flag, bit) in [
        (Flags::BOLD, wire::STYLE_BOLD),
        (Flags::ITALIC, wire::STYLE_ITALIC),
        (Flags::DIM, wire::STYLE_DIM),
        (Flags::INVERSE, wire::STYLE_INVERSE),
        (Flags::HIDDEN, wire::STYLE_HIDDEN),
        (Flags::STRIKEOUT, wire::STYLE_STRIKE),
        (Flags::UNDERLINE, wire::STYLE_UNDERLINE),
        (Flags::DOUBLE_UNDERLINE, wire::STYLE_DOUBLE_UNDERLINE),
        (Flags::UNDERCURL, wire::STYLE_CURLY_UNDERLINE),
        (Flags::DOTTED_UNDERLINE, wire::STYLE_DOTTED_UNDERLINE),
        (Flags::DASHED_UNDERLINE, wire::STYLE_DASHED_UNDERLINE),
    ] {
        if flags.contains(flag) {
            style |= bit;
        }
    }
    style
}

fn color(color: TermColor) -> Color {
    match color {
        TermColor::Named(named) => Color::Named(named as u16),
        TermColor::Indexed(index) => Color::Indexed(index),
        TermColor::Spec(rgb) => Color::Rgb([rgb.r, rgb.g, rgb.b]),
    }
}

fn color_bits(color: TermColor) -> u64 {
    match color {
        TermColor::Named(named) => named as u64,
        TermColor::Indexed(index) => 0x1_0000 | u64::from(index),
        TermColor::Spec(rgb) => {
            0x100_0000 | u64::from(rgb.r) << 16 | u64::from(rgb.g) << 8 | u64::from(rgb.b)
        }
    }
}

struct Hasher(u64);

impl Hasher {
    #[inline]
    fn add(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

fn hash_row(row: &Row<Cell>, cols: usize) -> u64 {
    let mut hasher = Hasher(0x9e37_79b9_7f4a_7c15);
    for column in 0..cols {
        let cell = &row[Column(column)];
        hasher.add(u64::from(cell.c as u32) | u64::from(cell.flags.bits()) << 32);
        hasher.add(color_bits(cell.fg) | color_bits(cell.bg) << 32);
        if cell.extra.is_some() {
            for mark in cell.zerowidth().into_iter().flatten() {
                hasher.add(u64::from(*mark as u32) | 1 << 40);
            }
            if let Some(underline) = cell.underline_color() {
                hasher.add(color_bits(underline) | 1 << 41);
            }
            if let Some(link) = cell.hyperlink() {
                for byte in link.uri().bytes() {
                    hasher.add(u64::from(byte) | 1 << 42);
                }
            }
        }
    }
    hasher.0
}

fn blank_cell(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\t')
        && cell.extra.is_none()
        && cell.bg == TermColor::Named(alacritty_terminal::vte::ansi::NamedColor::Background)
        && !cell
            .flags
            .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
}

/// Calls `each` with a row's cells as spans of the same style, left to
/// right, leaving out blank cells after the last written one.
pub(super) fn walk_row(
    row: &Row<Cell>,
    cols: usize,
    text: &mut String,
    mut each: impl FnMut(&SpanStyle<'_>, bool, bool, &str),
) {
    let end = (0..cols)
        .rev()
        .find(|&column| !blank_cell(&row[Column(column)]))
        .map_or(0, |column| column + 1);
    let mut column = 0;
    while column < end {
        let cell = &row[Column(column)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            column += 1;
            continue;
        }
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        let link = cell.hyperlink();
        let style = SpanStyle {
            fg: color(cell.fg),
            bg: color(cell.bg),
            flags: style_flags(cell.flags),
            underline_color: cell.underline_color().map(color),
            link: link.as_ref().map(|link| link.uri()),
        };
        text.clear();
        let marks = cell.zerowidth().filter(|marks| !marks.is_empty());
        if let Some(marks) = marks {
            text.push(cell.c);
            text.extend(marks.iter());
            each(&style, wide, true, text);
            column += if wide { 2 } else { 1 };
            continue;
        }
        let leading_spacer = cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER);
        text.push(if cell.c == '\t' || leading_spacer {
            ' '
        } else {
            cell.c
        });
        column += if wide { 2 } else { 1 };
        while column < end {
            let next = &row[Column(column)];
            if next.flags.contains(Flags::WIDE_CHAR_SPACER) {
                column += 1;
                continue;
            }
            let same = next.fg == cell.fg
                && next.bg == cell.bg
                && (next.flags & STYLE_FLAGS) == (cell.flags & STYLE_FLAGS)
                && next.flags.contains(Flags::WIDE_CHAR) == wide
                && next.extra == cell.extra;
            if !same || next.zerowidth().is_some_and(|marks| !marks.is_empty()) {
                break;
            }
            let leading_spacer = next.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER);
            text.push(if next.c == '\t' || leading_spacer {
                ' '
            } else {
                next.c
            });
            column += if wide { 2 } else { 1 };
        }
        each(&style, wide, false, text);
    }
}

/// A row as a line of history, built in `scratch`.
pub(super) fn store_row<'a>(
    row: &Row<Cell>,
    cols: usize,
    text: &mut String,
    scratch: &'a mut Vec<u8>,
) -> &'a [u8] {
    let mut line = LineWriter::new(scratch);
    walk_row(row, cols, text, |style, wide, cluster, text| {
        line.span(style, wide, cluster, text)
    });
    line.finish(wrapped(row, cols))
}

pub(super) fn wrapped(row: &Row<Cell>, cols: usize) -> bool {
    cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE)
}

fn modes(mode: TermMode) -> u32 {
    let mut modes = 0;
    for (flag, bit) in [
        (TermMode::ALT_SCREEN, wire::MODE_ALT_SCREEN),
        (TermMode::MOUSE_REPORT_CLICK, wire::MODE_MOUSE_CLICK),
        (TermMode::MOUSE_DRAG, wire::MODE_MOUSE_DRAG),
        (TermMode::MOUSE_MOTION, wire::MODE_MOUSE_MOTION),
        (TermMode::ALTERNATE_SCROLL, wire::MODE_ALTERNATE_SCROLL),
        (TermMode::BRACKETED_PASTE, wire::MODE_BRACKETED_PASTE),
        (TermMode::FOCUS_IN_OUT, wire::MODE_FOCUS_REPORTS),
        (TermMode::APP_CURSOR, wire::MODE_APP_CURSOR),
    ] {
        if mode.contains(flag) {
            modes |= bit;
        }
    }
    modes
}

impl Terminal {
    fn refresh_hashes(&mut self) {
        if self.frames.version == self.version && !self.frames.rows.is_empty() {
            return;
        }
        let cols = self.term.columns();
        let rows = self.term.screen_lines();
        let grid = self.term.grid();
        self.frames.rows.clear();
        self.frames
            .rows
            .extend((0..rows).map(|line| hash_row(&grid[Line(line as i32)], cols)));
        self.frames.blank = hash_row(&Row::new(cols), cols);
        self.frames.version = self.version;
    }

    fn header(&self, frame: u32, flags: u8, palette: bool) -> FrameHeader {
        let grid = self.term.grid();
        let mode = *self.term.mode();
        let point = grid.cursor.point;
        let style = self.term.cursor_style();
        let shape = if !mode.contains(TermMode::SHOW_CURSOR) {
            wire::CURSOR_HIDDEN
        } else {
            let shape = match style.shape {
                CursorShape::Block => wire::CURSOR_BLOCK,
                CursorShape::Underline => wire::CURSOR_UNDERLINE,
                CursorShape::Beam => wire::CURSOR_BEAM,
                CursorShape::HollowBlock => wire::CURSOR_HOLLOW,
                CursorShape::Hidden => wire::CURSOR_HIDDEN,
            };
            if style.blinking {
                shape | wire::CURSOR_BLINKS
            } else {
                shape
            }
        };
        let colors = self.term.colors();
        FrameHeader {
            frame,
            flags: flags | if palette { wire::FRAME_PALETTE } else { 0 },
            cols: self.term.columns() as u16,
            rows: self.term.screen_lines() as u16,
            epoch: self.epoch,
            history_end: self.pushed,
            history_len: self.history_len() as u32,
            cursor: Cursor {
                row: point.line.0.max(0) as u16,
                col: point.column.0 as u16,
                shape,
            },
            modes: modes(mode),
            palette: if palette {
                (0..269)
                    .filter_map(|index| {
                        colors[index].map(|rgb| (index as u16, [rgb.r, rgb.g, rgb.b]))
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    /// The frame that brings `view` up to date, or nothing when it already
    /// shows this version.
    pub fn frame(&mut self, view: &mut View) -> Option<Vec<u8>> {
        if view.version == self.version {
            return None;
        }
        self.refresh_hashes();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();
        let reset = view.rows.len() != rows || usize::from(view.cols) != cols;
        let palette = view.palette_version != Some(self.palette_version);
        let mut writer = FrameWriter::new();
        let mut text = String::new();
        let grid = self.term.grid();
        let mut old: Option<HashMap<u64, u16>> = None;
        let mut copy: Option<(u16, u16, u16)> = None;
        let mut clear: Option<(u16, u16)> = None;
        let flush_copy = |writer: &mut FrameWriter, copy: &mut Option<(u16, u16, u16)>| {
            if let Some((dst, src, count)) = copy.take() {
                writer.copy(dst, src, count);
            }
        };
        let flush_clear = |writer: &mut FrameWriter, clear: &mut Option<(u16, u16)>| {
            if let Some((row, count)) = clear.take() {
                writer.clear(row, count);
            }
        };
        for (index, hash) in self.frames.rows.iter().enumerate() {
            let at = index as u16;
            if !reset && view.rows.get(index) == Some(hash) {
                flush_copy(&mut writer, &mut copy);
                flush_clear(&mut writer, &mut clear);
                continue;
            }
            if *hash == self.frames.blank {
                flush_copy(&mut writer, &mut copy);
                if !reset {
                    clear = match clear {
                        Some((row, count)) => Some((row, count + 1)),
                        None => Some((at, 1)),
                    };
                }
                continue;
            }
            flush_clear(&mut writer, &mut clear);
            let source = if reset {
                None
            } else {
                old.get_or_insert_with(|| {
                    let mut map = HashMap::with_capacity(view.rows.len());
                    for (row, hash) in view.rows.iter().enumerate().rev() {
                        map.insert(*hash, row as u16);
                    }
                    map
                })
                .get(hash)
                .copied()
            };
            if let Some(src) = source {
                copy = match copy {
                    Some((dst, from, count)) if dst + count == at && from + count == src => {
                        Some((dst, from, count + 1))
                    }
                    other => {
                        if let Some((dst, from, count)) = other {
                            writer.copy(dst, from, count);
                        }
                        Some((at, src, 1))
                    }
                };
                continue;
            }
            flush_copy(&mut writer, &mut copy);
            let row = &grid[Line(index as i32)];
            writer.row(at, wrapped(row, cols));
            walk_row(row, cols, &mut text, |style, wide, cluster, text| {
                writer.span(style, wide, cluster, text)
            });
            writer.end_row();
        }
        flush_copy(&mut writer, &mut copy);
        flush_clear(&mut writer, &mut clear);
        view.frame = view.frame.wrapping_add(1);
        let header = self.header(view.frame, if reset { FRAME_RESET } else { 0 }, palette);
        view.version = self.version;
        view.rows.clone_from(&self.frames.rows);
        view.cols = cols as u16;
        view.palette_version = Some(self.palette_version);
        Some(writer.finish_frame(&header))
    }

    /// History lines `start..start + count`, as many as are still kept.
    pub fn lines(&self, start: u64, count: u32) -> Vec<u8> {
        let mut writer = FrameWriter::new();
        let first = self.pushed - self.history.len() as u64;
        let start = start.max(first);
        let end = start.saturating_add(u64::from(count)).min(self.pushed);
        let count = end.saturating_sub(start) as u32;
        for line in start..end {
            let stored = self
                .history
                .line((line - first) as usize)
                .unwrap_or_default();
            writer.line(history::wrapped(stored));
            history::walk(stored, |style, wide, cluster, text| {
                writer.span(style, wide, cluster, text)
            });
            writer.end_row();
        }
        writer.finish_lines(self.epoch, start, count)
    }
}
