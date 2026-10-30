//! The lines that scrolled off the screen, kept as runs of same-style text
//! rather than as emulator cells. A line of a hundred characters takes about
//! a hundred and fifty bytes here, where the emulator spends twenty-four
//! bytes on every column whether it holds anything or not.

use std::collections::VecDeque;
use std::ops::Range;

use sikemux_wire::grid::{Color, SpanStyle};

const WIDE: u8 = 1;
const CLUSTER: u8 = 2;
const UNDERLINE_COLOR: u8 = 4;
const LINK: u8 = 8;

fn put_varint(out: &mut Vec<u8>, mut value: usize) {
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
            out.extend_from_slice(&index.to_le_bytes());
        }
        Color::Indexed(index) => out.extend_from_slice(&[1, index]),
        Color::Rgb([r, g, b]) => out.extend_from_slice(&[2, r, g, b]),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> u8 {
        let byte = self.bytes[self.at];
        self.at += 1;
        byte
    }

    fn varint(&mut self) -> usize {
        let mut value = 0;
        let mut shift = 0;
        loop {
            let byte = self.byte();
            value |= usize::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return value;
            }
            shift += 7;
        }
    }

    fn color(&mut self) -> Color {
        match self.byte() {
            0 => {
                let index = u16::from_le_bytes([self.byte(), self.byte()]);
                Color::Named(index)
            }
            1 => Color::Indexed(self.byte()),
            _ => Color::Rgb([self.byte(), self.byte(), self.byte()]),
        }
    }

    fn text(&mut self) -> &'a str {
        let len = self.varint();
        let bytes = &self.bytes[self.at..self.at + len];
        self.at += len;
        std::str::from_utf8(bytes).unwrap_or_default()
    }
}

/// Builds one stored line from its spans, in a buffer kept between lines.
pub(super) struct LineWriter<'a> {
    bytes: &'a mut Vec<u8>,
}

impl<'a> LineWriter<'a> {
    pub(super) fn new(bytes: &'a mut Vec<u8>) -> Self {
        bytes.clear();
        bytes.push(0);
        Self { bytes }
    }

    pub(super) fn span(&mut self, style: &SpanStyle<'_>, wide: bool, cluster: bool, text: &str) {
        let mut flags = 0;
        if wide {
            flags |= WIDE;
        }
        if cluster {
            flags |= CLUSTER;
        }
        if style.underline_color.is_some() {
            flags |= UNDERLINE_COLOR;
        }
        if style.link.is_some() {
            flags |= LINK;
        }
        self.bytes.push(flags);
        self.bytes.extend_from_slice(&style.flags.to_le_bytes());
        put_color(self.bytes, style.fg);
        put_color(self.bytes, style.bg);
        if let Some(color) = style.underline_color {
            put_color(self.bytes, color);
        }
        if let Some(link) = style.link {
            put_varint(self.bytes, link.len());
            self.bytes.extend_from_slice(link.as_bytes());
        }
        put_varint(self.bytes, text.len());
        self.bytes.extend_from_slice(text.as_bytes());
    }

    pub(super) fn finish(self, wrapped: bool) -> &'a [u8] {
        self.bytes[0] = u8::from(wrapped);
        self.bytes
    }
}

pub(super) fn wrapped(line: &[u8]) -> bool {
    line.first().is_some_and(|first| *first != 0)
}

/// Calls `each` with every span of a stored line, in order, and answers
/// whether the line goes on in the next one.
pub(super) fn walk(line: &[u8], mut each: impl FnMut(&SpanStyle<'_>, bool, bool, &str)) -> bool {
    let mut reader = Reader { bytes: line, at: 1 };
    while reader.at < line.len() {
        let flags = reader.byte();
        let style_flags = u16::from_le_bytes([reader.byte(), reader.byte()]);
        let fg = reader.color();
        let bg = reader.color();
        let underline_color = (flags & UNDERLINE_COLOR != 0).then(|| reader.color());
        let link = (flags & LINK != 0).then(|| reader.text());
        let text = reader.text();
        let style = SpanStyle {
            fg,
            bg,
            flags: style_flags,
            underline_color,
            link,
        };
        each(&style, flags & WIDE != 0, flags & CLUSTER != 0, text);
    }
    wrapped(line)
}

/// One cell of a line being rewrapped, its text a range of the line's text, so a run of
/// cells is one slice of it.
struct Cell {
    style: usize,
    text: Range<usize>,
    wide: bool,
    cluster: bool,
}

struct OwnedStyle {
    fg: Color,
    bg: Color,
    flags: u16,
    underline_color: Option<Color>,
    link: Option<String>,
}

impl OwnedStyle {
    fn of(style: &SpanStyle<'_>) -> Self {
        Self {
            fg: style.fg,
            bg: style.bg,
            flags: style.flags,
            underline_color: style.underline_color,
            link: style.link.map(str::to_owned),
        }
    }

    fn same(&self, style: &SpanStyle<'_>) -> bool {
        self.fg == style.fg
            && self.bg == style.bg
            && self.flags == style.flags
            && self.underline_color == style.underline_color
            && self.link.as_deref() == style.link
    }

    fn borrowed(&self) -> SpanStyle<'_> {
        SpanStyle {
            fg: self.fg,
            bg: self.bg,
            flags: self.flags,
            underline_color: self.underline_color,
            link: self.link.as_deref(),
        }
    }
}

/// Lines are written back to back into chunks this big, so storing one costs
/// no allocation of its own, and a chunk is freed once its last line goes.
const CHUNK_BYTES: usize = 64 * 1024;

/// Where a line is: its chunk's number, and its place in that chunk.
#[derive(Clone, Copy)]
struct Place {
    chunk: u64,
    start: u32,
    len: u32,
}

pub(super) struct History {
    chunks: VecDeque<Vec<u8>>,
    /// The number of the chunk at the front of `chunks`.
    first_chunk: u64,
    lines: VecDeque<Place>,
    limit: usize,
}

impl History {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            first_chunk: 0,
            lines: VecDeque::new(),
            limit,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.lines.len()
    }

    pub(super) fn line(&self, index: usize) -> Option<&[u8]> {
        let place = self.lines.get(index)?;
        let chunk = self.chunks.get((place.chunk - self.first_chunk) as usize)?;
        chunk.get(place.start as usize..(place.start + place.len) as usize)
    }

    pub(super) fn push(&mut self, line: &[u8]) {
        if self.limit == 0 {
            return;
        }
        let full = self
            .chunks
            .back()
            .is_none_or(|chunk| chunk.len() + line.len() > chunk.capacity());
        if full {
            self.chunks
                .push_back(Vec::with_capacity(CHUNK_BYTES.max(line.len())));
        }
        let chunk_number = self.first_chunk + self.chunks.len() as u64 - 1;
        let Some(chunk) = self.chunks.back_mut() else {
            return;
        };
        self.lines.push_back(Place {
            chunk: chunk_number,
            start: chunk.len() as u32,
            len: line.len() as u32,
        });
        chunk.extend_from_slice(line);
        if self.lines.len() > self.limit {
            self.lines.pop_front();
            self.drop_unused_chunks();
        }
    }

    fn drop_unused_chunks(&mut self) {
        let oldest = self
            .lines
            .front()
            .map_or(self.first_chunk + self.chunks.len() as u64, |place| {
                place.chunk
            });
        while self.first_chunk < oldest && !self.chunks.is_empty() {
            self.chunks.pop_front();
            self.first_chunk += 1;
        }
    }

    pub(super) fn clear(&mut self) {
        self.lines = VecDeque::new();
        self.chunks = VecDeque::new();
    }

    pub(super) fn limit(&self) -> usize {
        self.limit
    }

    pub(super) fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        let extra = self.lines.len().saturating_sub(limit);
        self.lines.drain(..extra);
        self.lines.shrink_to_fit();
        self.drop_unused_chunks();
    }

    /// Wraps the lines again at `cols` columns, as the screen is after a
    /// resize. Answers how many lines there are now.
    pub(super) fn rewrap(&mut self, cols: usize) -> usize {
        let old = std::mem::replace(self, History::new(self.limit));
        let mut styles: Vec<OwnedStyle> = Vec::new();
        let mut cells: Vec<Cell> = Vec::new();
        let mut texts = String::new();
        for line in (0..old.len()).filter_map(|index| old.line(index)) {
            let wrapped = walk(line, |style, wide, cluster, text| {
                let index = match styles.iter().rposition(|known| known.same(style)) {
                    Some(index) => index,
                    None => {
                        styles.push(OwnedStyle::of(style));
                        styles.len() - 1
                    }
                };
                let base = texts.len();
                texts.push_str(text);
                if cluster {
                    cells.push(Cell {
                        style: index,
                        text: base..texts.len(),
                        wide,
                        cluster,
                    });
                } else {
                    cells.extend(text.char_indices().map(|(at, character)| Cell {
                        style: index,
                        text: base + at..base + at + character.len_utf8(),
                        wide,
                        cluster,
                    }));
                }
            });
            if wrapped {
                continue;
            }
            self.wrap_cells(&cells, &texts, &styles, cols);
            cells.clear();
            texts.clear();
            styles.clear();
        }
        if !cells.is_empty() {
            self.wrap_cells(&cells, &texts, &styles, cols);
        }
        self.lines.len()
    }

    fn wrap_cells(&mut self, cells: &[Cell], texts: &str, styles: &[OwnedStyle], cols: usize) {
        let mut scratch = Vec::new();
        let mut start = 0;
        loop {
            let mut width = 0;
            let mut end = start;
            while end < cells.len() {
                let cell_width = if cells[end].wide { 2 } else { 1 };
                if width + cell_width > cols {
                    break;
                }
                width += cell_width;
                end += 1;
            }
            if end == start && end < cells.len() {
                end += 1;
            }
            let mut writer = LineWriter::new(&mut scratch);
            let mut run = start;
            while run < end {
                let first = &cells[run];
                let mut next = run + 1;
                if !first.cluster {
                    while next < end
                        && cells[next].style == first.style
                        && cells[next].wide == first.wide
                        && !cells[next].cluster
                    {
                        next += 1;
                    }
                }
                writer.span(
                    &styles[first.style].borrowed(),
                    first.wide,
                    first.cluster,
                    &texts[first.text.start..cells[next - 1].text.end],
                );
                run = next;
            }
            let line = writer.finish(end < cells.len());
            self.push(line);
            if end >= cells.len() {
                return;
            }
            start = end;
        }
    }
}
