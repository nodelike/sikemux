//! A terminal's cells as the core sends them. The core runs the terminal and
//! sends each client only what changed since the last frame it sent that
//! client; lines that scrolled off the top are sent in pages when asked for.
//!
//! A frame is the header, the styles and links its rows use, then row
//! operations that turn the client's previous rows into the new ones.
//! Numbers are LEB128 varints unless noted. Lines are numbered from the
//! first line the terminal ever pushed into its history, so a page fetched
//! earlier stays valid while output scrolls, until `epoch` changes.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The client drops every row it holds; rows the frame does not set are blank.
pub const FRAME_RESET: u8 = 1;
/// The frame carries the colours programs set, replacing any sent before.
pub const FRAME_PALETTE: u8 = 2;

pub const MODE_ALT_SCREEN: u32 = 1;
pub const MODE_MOUSE_CLICK: u32 = 1 << 1;
pub const MODE_MOUSE_DRAG: u32 = 1 << 2;
pub const MODE_MOUSE_MOTION: u32 = 1 << 3;
/// The wheel on the alternate screen moves the program's cursor.
pub const MODE_ALTERNATE_SCROLL: u32 = 1 << 4;
pub const MODE_BRACKETED_PASTE: u32 = 1 << 5;
pub const MODE_FOCUS_REPORTS: u32 = 1 << 6;
pub const MODE_APP_CURSOR: u32 = 1 << 7;
pub const MODE_MOUSE_ANY: u32 = MODE_MOUSE_CLICK | MODE_MOUSE_DRAG | MODE_MOUSE_MOTION;

pub const STYLE_BOLD: u16 = 1;
pub const STYLE_ITALIC: u16 = 1 << 1;
pub const STYLE_DIM: u16 = 1 << 2;
pub const STYLE_INVERSE: u16 = 1 << 3;
pub const STYLE_HIDDEN: u16 = 1 << 4;
pub const STYLE_STRIKE: u16 = 1 << 5;
pub const STYLE_UNDERLINE: u16 = 1 << 6;
pub const STYLE_DOUBLE_UNDERLINE: u16 = 1 << 7;
pub const STYLE_CURLY_UNDERLINE: u16 = 1 << 8;
pub const STYLE_DOTTED_UNDERLINE: u16 = 1 << 9;
pub const STYLE_DASHED_UNDERLINE: u16 = 1 << 10;
const STYLE_UNDERLINE_COLOR: u16 = 1 << 14;
const STYLE_LINK: u16 = 1 << 15;

pub const CURSOR_HIDDEN: u8 = 0;
pub const CURSOR_BLOCK: u8 = 1;
pub const CURSOR_UNDERLINE: u8 = 2;
pub const CURSOR_BEAM: u8 = 3;
pub const CURSOR_HOLLOW: u8 = 4;
pub const CURSOR_BLINKS: u8 = 0x80;

/// Each character in the span takes two columns.
const SPAN_WIDE: u8 = 1;
/// The whole span is one cell: a character with marks combined onto it.
const SPAN_CLUSTER: u8 = 2;
const ROW_WRAPPED: u8 = 1;

const OP_END: u8 = 0;
const OP_COPY: u8 = 1;
const OP_CLEAR: u8 = 2;
const OP_ROW: u8 = 3;

/// A cell's colour as the program named it; the client's theme decides what
/// a named or indexed colour looks like. Named colours number as in
/// xterm: 0-15 the ANSI colours, 256 foreground, 257 background, 258 cursor,
/// 259-266 the dim colours, 267 bright foreground, 268 dim foreground.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Named(u16),
    Indexed(u8),
    Rgb([u8; 3]),
}

pub const NAMED_FOREGROUND: u16 = 256;
pub const NAMED_BACKGROUND: u16 = 257;

impl Default for Color {
    fn default() -> Self {
        Color::Named(NAMED_FOREGROUND)
    }
}

/// The theme's colours, which the core answers a program's colour queries
/// with: some programs pick light or dark output from the background.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalColors {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: Option<[u8; 3]>,
    pub ansi: [[u8; 3]; 16],
}

impl Default for TerminalColors {
    fn default() -> Self {
        Self {
            foreground: [0xd8, 0xd8, 0xe0],
            background: [0x15, 0x14, 0x1b],
            cursor: None,
            ansi: [
                [0x00, 0x00, 0x00],
                [0xcd, 0x31, 0x31],
                [0x0d, 0xbc, 0x79],
                [0xe5, 0xe5, 0x10],
                [0x24, 0x72, 0xc8],
                [0xbc, 0x3f, 0xbc],
                [0x11, 0xa8, 0xcd],
                [0xe5, 0xe5, 0xe5],
                [0x66, 0x66, 0x66],
                [0xf1, 0x4c, 0x4c],
                [0x23, 0xd1, 0x8b],
                [0xf5, 0xf5, 0x43],
                [0x3b, 0x8e, 0xea],
                [0xd6, 0x70, 0xd6],
                [0x29, 0xb8, 0xdb],
                [0xff, 0xff, 0xff],
            ],
        }
    }
}

impl TerminalColors {
    /// The colour a named colour number stands for under this theme.
    pub fn color(&self, index: usize) -> [u8; 3] {
        const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
        let dim = |[r, g, b]: [u8; 3]| [r / 3 * 2, g / 3 * 2, b / 3 * 2];
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let cube = index - 16;
                [CUBE[cube / 36], CUBE[cube / 6 % 6], CUBE[cube % 6]]
            }
            232..=255 => {
                let level = (8 + (index - 232) * 10) as u8;
                [level, level, level]
            }
            257 => self.background,
            258 => self.cursor.unwrap_or(self.foreground),
            259..=266 => dim(self.ansi[index - 259]),
            268 => dim(self.foreground),
            _ => self.foreground,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
    pub underline_color: Option<Color>,
    pub link: Option<Arc<str>>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            fg: Color::Named(NAMED_FOREGROUND),
            bg: Color::Named(NAMED_BACKGROUND),
            flags: 0,
            underline_color: None,
            link: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    /// One of the `CURSOR_` shapes, with `CURSOR_BLINKS` set when it blinks.
    pub shape: u8,
}

/// What a frame says about the terminal besides its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameHeader {
    pub frame: u32,
    pub flags: u8,
    pub cols: u16,
    pub rows: u16,
    /// Changes whenever line numbers are no longer the ones sent before,
    /// such as after the history was cleared or reflowed by a resize.
    pub epoch: u32,
    /// The number of the line shown on the top row.
    pub history_end: u64,
    /// How many lines before `history_end` can still be fetched.
    pub history_len: u32,
    pub cursor: Cursor,
    pub modes: u32,
    /// The colours programs set, by named colour number. Sent with
    /// `FRAME_PALETTE`.
    pub palette: Vec<(u16, [u8; 3])>,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_color(out: &mut Vec<u8>, color: Color) {
    match color {
        Color::Named(index) => {
            out.push(0);
            put_varint(out, u64::from(index));
        }
        Color::Indexed(index) => {
            out.push(1);
            out.push(index);
        }
        Color::Rgb(rgb) => {
            out.push(2);
            out.extend_from_slice(&rgb);
        }
    }
}

/// Builds one frame, or one page of history lines.
#[derive(Default)]
pub struct FrameWriter {
    body: Vec<u8>,
    styles: Vec<u8>,
    style_count: u32,
    style_index: HashMap<StyleKey, u32>,
    links: Vec<u8>,
    link_count: u32,
    link_index: HashMap<String, u32>,
    last_style: Option<(StyleKey, u32)>,
}

/// A span's style as the encoder has it, its link borrowed from the cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanStyle<'a> {
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
    pub underline_color: Option<Color>,
    pub link: Option<&'a str>,
}

/// A style as the encoder sees it, with its link already numbered.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StyleKey {
    fg: Color,
    bg: Color,
    flags: u16,
    underline_color: Option<Color>,
    link: Option<u32>,
}

impl FrameWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rows `src..src + count` of the client's previous rows become rows
    /// `dst..dst + count`. Copies read the rows from before this frame.
    pub fn copy(&mut self, dst: u16, src: u16, count: u16) {
        self.body.push(OP_COPY);
        put_varint(&mut self.body, u64::from(dst));
        put_varint(&mut self.body, u64::from(src));
        put_varint(&mut self.body, u64::from(count));
    }

    pub fn clear(&mut self, row: u16, count: u16) {
        self.body.push(OP_CLEAR);
        put_varint(&mut self.body, u64::from(row));
        put_varint(&mut self.body, u64::from(count));
    }

    /// Starts row `index` of the screen. Spans follow, then `end_row`.
    pub fn row(&mut self, index: u16, wrapped: bool) {
        self.body.push(OP_ROW);
        put_varint(&mut self.body, u64::from(index));
        self.line(wrapped);
    }

    /// Starts the next line of a page of history.
    pub fn line(&mut self, wrapped: bool) {
        self.body.push(if wrapped { ROW_WRAPPED } else { 0 });
    }

    fn link(&mut self, uri: &str) -> u32 {
        if let Some(index) = self.link_index.get(uri) {
            return *index;
        }
        let index = self.link_count;
        self.link_count += 1;
        put_varint(&mut self.links, uri.len() as u64);
        self.links.extend_from_slice(uri.as_bytes());
        self.link_index.insert(uri.to_owned(), index);
        index
    }

    fn style(&mut self, style: &SpanStyle<'_>) -> u32 {
        let link = style.link.map(|uri| self.link(uri));
        let key = StyleKey {
            fg: style.fg,
            bg: style.bg,
            flags: style.flags,
            underline_color: style.underline_color,
            link,
        };
        if let Some((last, index)) = self.last_style {
            if last == key {
                return index;
            }
        }
        let index = match self.style_index.get(&key) {
            Some(index) => *index,
            None => {
                let index = self.style_count;
                self.style_count += 1;
                let mut flags = key.flags & !(STYLE_UNDERLINE_COLOR | STYLE_LINK);
                if key.underline_color.is_some() {
                    flags |= STYLE_UNDERLINE_COLOR;
                }
                if key.link.is_some() {
                    flags |= STYLE_LINK;
                }
                put_varint(&mut self.styles, u64::from(flags));
                put_color(&mut self.styles, key.fg);
                put_color(&mut self.styles, key.bg);
                if let Some(color) = key.underline_color {
                    put_color(&mut self.styles, color);
                }
                if let Some(link) = key.link {
                    put_varint(&mut self.styles, u64::from(link));
                }
                self.style_index.insert(key, index);
                index
            }
        };
        self.last_style = Some((key, index));
        index
    }

    /// Cells of one style. Each character is a cell, two columns wide when
    /// `wide`; with `cluster` the whole text is a single cell.
    pub fn span(&mut self, style: &SpanStyle<'_>, wide: bool, cluster: bool, text: &str) {
        let index = self.style(style);
        put_varint(&mut self.body, u64::from(index) + 1);
        let mut flags = 0;
        if wide {
            flags |= SPAN_WIDE;
        }
        if cluster {
            flags |= SPAN_CLUSTER;
        }
        self.body.push(flags);
        put_varint(&mut self.body, text.len() as u64);
        self.body.extend_from_slice(text.as_bytes());
    }

    pub fn end_row(&mut self) {
        self.body.push(0);
    }

    fn tables(&self, out: &mut Vec<u8>) {
        put_varint(out, u64::from(self.style_count));
        out.extend_from_slice(&self.styles);
        put_varint(out, u64::from(self.link_count));
        out.extend_from_slice(&self.links);
    }

    pub fn finish_frame(self, header: &FrameHeader) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + self.styles.len() + 64);
        put_varint(&mut out, u64::from(header.frame));
        let mut flags = header.flags & !FRAME_PALETTE;
        if !header.palette.is_empty() || header.flags & FRAME_PALETTE != 0 {
            flags |= FRAME_PALETTE;
        }
        out.push(flags);
        put_varint(&mut out, u64::from(header.cols));
        put_varint(&mut out, u64::from(header.rows));
        put_varint(&mut out, u64::from(header.epoch));
        put_varint(&mut out, header.history_end);
        put_varint(&mut out, u64::from(header.history_len));
        put_varint(&mut out, u64::from(header.cursor.row));
        put_varint(&mut out, u64::from(header.cursor.col));
        out.push(header.cursor.shape);
        put_varint(&mut out, u64::from(header.modes));
        if flags & FRAME_PALETTE != 0 {
            put_varint(&mut out, header.palette.len() as u64);
            for (index, rgb) in &header.palette {
                put_varint(&mut out, u64::from(*index));
                out.extend_from_slice(rgb);
            }
        }
        self.tables(&mut out);
        out.extend_from_slice(&self.body);
        out.push(OP_END);
        out
    }

    /// A page of `count` history lines starting at line `start`.
    pub fn finish_lines(self, epoch: u32, start: u64, count: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.body.len() + self.styles.len() + 16);
        put_varint(&mut out, u64::from(epoch));
        put_varint(&mut out, start);
        put_varint(&mut out, u64::from(count));
        self.tables(&mut out);
        out.extend_from_slice(&self.body);
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub style: Arc<Style>,
    pub text: String,
    /// The column the span starts in.
    pub col: u16,
    /// Columns the span covers.
    pub width: u16,
    /// Each character takes two columns.
    pub wide: bool,
    /// The whole text is one cell: a character with marks combined onto it.
    pub cluster: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// The line goes on in the next row: the program wrote past the edge.
    pub wrapped: bool,
    /// Left to right; columns no span covers are blank.
    pub spans: Vec<Span>,
}

impl Row {
    pub fn text(&self) -> String {
        let mut text = String::new();
        let mut col = 0;
        for span in &self.spans {
            text.extend(std::iter::repeat_n(
                ' ',
                usize::from(span.col.saturating_sub(col)),
            ));
            text.push_str(&span.text);
            col = span.col + span.width;
        }
        text.truncate(text.trim_end().len());
        text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Copy { dst: u16, src: u16, count: u16 },
    Clear { row: u16, count: u16 },
    Row { index: u16, row: Row },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub header: FrameHeader,
    pub ops: Vec<Op>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lines {
    pub epoch: u32,
    pub start: u64,
    pub rows: Vec<Row>,
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (first, rest) = self.bytes.split_first()?;
        self.bytes = rest;
        Some(*first)
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn small(&mut self) -> Option<u16> {
        u16::try_from(self.varint()?).ok()
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if len > self.bytes.len() {
            return None;
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Some(head)
    }

    fn color(&mut self) -> Option<Color> {
        match self.byte()? {
            0 => Some(Color::Named(self.small()?)),
            1 => Some(Color::Indexed(self.byte()?)),
            2 => {
                let rgb = self.take(3)?;
                Some(Color::Rgb([rgb[0], rgb[1], rgb[2]]))
            }
            _ => None,
        }
    }

    fn tables(&mut self) -> Option<Vec<Arc<Style>>> {
        let style_count = self.varint()? as usize;
        let mut raw = Vec::with_capacity(style_count.min(4096));
        for _ in 0..style_count {
            let flags = self.small()?;
            let fg = self.color()?;
            let bg = self.color()?;
            let underline_color = if flags & STYLE_UNDERLINE_COLOR != 0 {
                Some(self.color()?)
            } else {
                None
            };
            let link = if flags & STYLE_LINK != 0 {
                Some(self.varint()? as usize)
            } else {
                None
            };
            raw.push((
                flags & !(STYLE_UNDERLINE_COLOR | STYLE_LINK),
                fg,
                bg,
                underline_color,
                link,
            ));
        }
        let link_count = self.varint()? as usize;
        let mut links: Vec<Arc<str>> = Vec::with_capacity(link_count.min(4096));
        for _ in 0..link_count {
            let len = self.varint()? as usize;
            links.push(Arc::from(std::str::from_utf8(self.take(len)?).ok()?));
        }
        raw.into_iter()
            .map(|(flags, fg, bg, underline_color, link)| {
                Some(Arc::new(Style {
                    fg,
                    bg,
                    flags,
                    underline_color,
                    link: match link {
                        Some(index) => Some(links.get(index)?.clone()),
                        None => None,
                    },
                }))
            })
            .collect()
    }

    fn row(&mut self, styles: &[Arc<Style>]) -> Option<Row> {
        let wrapped = self.byte()? & ROW_WRAPPED != 0;
        let mut spans = Vec::new();
        let mut col = 0u16;
        loop {
            let style = self.varint()? as usize;
            if style == 0 {
                return Some(Row { wrapped, spans });
            }
            let style = styles.get(style - 1)?.clone();
            let flags = self.byte()?;
            let len = self.varint()? as usize;
            let text = std::str::from_utf8(self.take(len)?).ok()?.to_owned();
            let wide = flags & SPAN_WIDE != 0;
            let cluster = flags & SPAN_CLUSTER != 0;
            let cells = if cluster {
                1
            } else {
                text.chars().count() as u16
            };
            let width = cells * if wide { 2 } else { 1 };
            spans.push(Span {
                style,
                text,
                col,
                width,
                wide,
                cluster,
            });
            col += width;
        }
    }
}

pub fn decode_frame(payload: &[u8]) -> Option<Frame> {
    let mut reader = Reader { bytes: payload };
    let frame = u32::try_from(reader.varint()?).ok()?;
    let flags = reader.byte()?;
    let cols = reader.small()?;
    let rows = reader.small()?;
    let epoch = u32::try_from(reader.varint()?).ok()?;
    let history_end = reader.varint()?;
    let history_len = u32::try_from(reader.varint()?).ok()?;
    let cursor = Cursor {
        row: reader.small()?,
        col: reader.small()?,
        shape: reader.byte()?,
    };
    let modes = u32::try_from(reader.varint()?).ok()?;
    let mut palette = Vec::new();
    if flags & FRAME_PALETTE != 0 {
        for _ in 0..reader.varint()? {
            let index = reader.small()?;
            let rgb = reader.take(3)?;
            palette.push((index, [rgb[0], rgb[1], rgb[2]]));
        }
    }
    let styles = reader.tables()?;
    let mut ops = Vec::new();
    loop {
        match reader.byte()? {
            OP_END => break,
            OP_COPY => ops.push(Op::Copy {
                dst: reader.small()?,
                src: reader.small()?,
                count: reader.small()?,
            }),
            OP_CLEAR => ops.push(Op::Clear {
                row: reader.small()?,
                count: reader.small()?,
            }),
            OP_ROW => {
                let index = reader.small()?;
                ops.push(Op::Row {
                    index,
                    row: reader.row(&styles)?,
                });
            }
            _ => return None,
        }
    }
    Some(Frame {
        header: FrameHeader {
            frame,
            flags,
            cols,
            rows,
            epoch,
            history_end,
            history_len,
            cursor,
            modes,
            palette,
        },
        ops,
    })
}

pub fn decode_lines(payload: &[u8]) -> Option<Lines> {
    let mut reader = Reader { bytes: payload };
    let epoch = u32::try_from(reader.varint()?).ok()?;
    let start = reader.varint()?;
    let count = reader.varint()? as usize;
    let styles = reader.tables()?;
    let mut rows = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        rows.push(reader.row(&styles)?);
    }
    Some(Lines { epoch, start, rows })
}

/// A client's copy of a terminal's screen, kept up to date by frames.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    pub header: FrameHeader,
    /// Shared, so a frame that moves rows does not copy them.
    pub rows: Vec<Arc<Row>>,
}

impl Screen {
    pub fn apply(&mut self, frame: Frame) {
        let size = usize::from(frame.header.rows);
        let previous = if frame.header.flags & FRAME_RESET != 0 {
            Vec::new()
        } else {
            std::mem::take(&mut self.rows)
        };
        let mut rows = previous.clone();
        rows.resize_with(size, Arc::default);
        let palette = std::mem::take(&mut self.header.palette);
        for op in frame.ops {
            match op {
                Op::Copy { dst, src, count } => {
                    for offset in 0..count {
                        let (dst, src) = (usize::from(dst + offset), usize::from(src + offset));
                        if let (Some(row), Some(slot)) = (previous.get(src), rows.get_mut(dst)) {
                            *slot = row.clone();
                        }
                    }
                }
                Op::Clear { row, count } => {
                    for slot in rows
                        .iter_mut()
                        .skip(usize::from(row))
                        .take(usize::from(count))
                    {
                        *slot = Arc::default();
                    }
                }
                Op::Row { index, row } => {
                    if let Some(slot) = rows.get_mut(usize::from(index)) {
                        *slot = Arc::new(row);
                    }
                }
            }
        }
        let keeps_palette = frame.header.flags & FRAME_PALETTE == 0;
        self.header = frame.header;
        if keeps_palette {
            self.header.palette = palette;
        }
        self.rows = rows;
    }

    /// The screen's text, one line per row, without trailing blanks.
    pub fn text(&self) -> String {
        let lines: Vec<String> = self.rows.iter().map(|row| row.text()).collect();
        let mut text = lines.join("\n");
        text.truncate(text.trim_end().len());
        text
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub meta: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseAction {
    Press,
    Release,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

/// What a person does to a terminal. The core turns it into the bytes the
/// program expects under the modes it turned on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalInput {
    /// A key that is not plain text, by its `KeyboardEvent.key` name, or a
    /// character typed with control, option or command held.
    Key {
        key: String,
        #[serde(default)]
        mods: Mods,
    },
    /// Text typed, or composed by an input method.
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    /// Files dropped on the terminal, pasted as words the person's shell
    /// reads back as those paths.
    Paths {
        paths: Vec<String>,
    },
    Mouse {
        action: MouseAction,
        button: MouseButton,
        col: u16,
        row: u16,
        #[serde(default)]
        mods: Mods,
    },
    /// The wheel turned by `lines`, negative upward, over a cell.
    Wheel {
        col: u16,
        row: u16,
        lines: i32,
        #[serde(default)]
        mods: Mods,
    },
    Focus {
        focused: bool,
    },
}

/// A cell by line number, as clients number lines, and column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GridPoint {
    pub line: u64,
    pub col: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindOptions {
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub whole_word: bool,
}

/// The matches of a search, first to last, and which one is current.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    pub epoch: u32,
    /// Index into `matches` of the first match after where the search
    /// started, or before it when searching backwards.
    pub current: Option<usize>,
    /// Each match's first and last cell.
    pub matches: Vec<(GridPoint, GridPoint)>,
    /// There were more matches than `matches` holds.
    pub more: bool,
}
