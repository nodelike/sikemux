//! A message's markdown as a tree of blocks, kept up to date by patches.
//! The README describes the tree's JSON and the patches.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Pixels, SharedString, Size, TextAlign};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub enum Kind {
    #[default]
    Text,
    Paragraph,
    Heading(u8),
    Quote,
    Bullets,
    Numbers(u64),
    Item(Option<bool>),
    /// A code block's language, and the file it shows when its fence names one.
    Code(Option<SharedString>, Option<SharedString>),
    Rule,
    Table(Box<Table>),
    /// A list with no tag of its own, as a table's rows and its cells are.
    Group,
    Footnote(u64),
    Details {
        summary: SharedString,
        open: bool,
    },
    /// Blocks lined up to one side, as embedded HTML asked.
    Aligned(TextAlign),
    Emphasis,
    Strong,
    Strike,
    /// Words drawn in the faint ink, as words the reader asked to see that were left out.
    Faint,
    InlineCode,
    /// LaTeX math in a line of text, kept as written and drawn as Unicode.
    Math,
    Link(SharedString),
    /// A name that could be a file, as the message wrote it.
    Path(SharedString),
    Image {
        src: SharedString,
        alt: SharedString,
    },
    Break,
    FootnoteRef(u64),
    Unknown,
}

impl Kind {
    fn is_inline(&self) -> bool {
        matches!(
            self,
            Kind::Text
                | Kind::Emphasis
                | Kind::Strong
                | Kind::Strike
                | Kind::Faint
                | Kind::InlineCode
                | Kind::Math
                | Kind::Link(_)
                | Kind::Path(_)
                | Kind::Break
                | Kind::FootnoteRef(_)
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CodeMetrics {
    pub(crate) starts: Vec<usize>,
    pub(crate) widest: usize,
    last_line_chars: usize,
    trimmed_len: usize,
}

impl CodeMetrics {
    fn from_text(text: &str) -> Self {
        let trimmed = text.trim_end_matches('\n');
        let mut metrics = Self {
            starts: vec![0],
            widest: 0,
            last_line_chars: 0,
            trimmed_len: 0,
        };
        metrics.extend_trimmed(trimmed);
        metrics
    }

    fn extend_trimmed(&mut self, trimmed: &str) {
        let tail = trimmed.get(self.trimmed_len..).unwrap_or_default();
        let base = self.trimmed_len;
        if tail.is_ascii() {
            let mut line_start = 0;
            for (at, &byte) in tail.as_bytes().iter().enumerate() {
                if byte == b'\n' {
                    self.last_line_chars += at - line_start;
                    self.widest = self.widest.max(self.last_line_chars);
                    self.last_line_chars = 0;
                    self.starts.push(base + at + 1);
                    line_start = at + 1;
                }
            }
            self.last_line_chars += tail.len() - line_start;
            self.widest = self.widest.max(self.last_line_chars);
        } else {
            for (at, ch) in tail.char_indices() {
                if ch == '\n' {
                    self.widest = self.widest.max(self.last_line_chars);
                    self.last_line_chars = 0;
                    self.starts.push(base + at + 1);
                } else {
                    self.last_line_chars += 1;
                }
            }
            self.widest = self.widest.max(self.last_line_chars);
        }
        self.trimmed_len = trimmed.len();
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Prefixes {
    pub(crate) tasks_before: Vec<usize>,
    pub(crate) parts_before: Vec<usize>,
    pub(crate) h2_before: Vec<usize>,
}

impl Prefixes {
    pub(crate) fn from_blocks(blocks: &[Rc<Node>]) -> Self {
        let mut prefixes = Self {
            tasks_before: Vec::with_capacity(blocks.len() + 1),
            parts_before: Vec::with_capacity(blocks.len() + 1),
            h2_before: Vec::with_capacity(blocks.len() + 1),
        };
        prefixes.tasks_before.push(0);
        prefixes.parts_before.push(0);
        prefixes.h2_before.push(0);
        prefixes.extend(blocks);
        prefixes
    }

    fn extend(&mut self, blocks: &[Rc<Node>]) {
        let mut tasks = self.tasks_before.last().copied().unwrap_or(0);
        let mut parts = self.parts_before.last().copied().unwrap_or(0);
        let mut h2 = self.h2_before.last().copied().unwrap_or(0);
        for block in blocks {
            tasks += block.tasks_count();
            parts += block.parts_count();
            h2 += usize::from(matches!(block.kind, Kind::Heading(2)));
            self.tasks_before.push(tasks);
            self.parts_before.push(parts);
            self.h2_before.push(h2);
        }
    }

    fn truncate(&mut self, len: usize) {
        self.tasks_before.truncate(len + 1);
        self.parts_before.truncate(len + 1);
        self.h2_before.truncate(len + 1);
    }

    fn update_at(&mut self, at: usize, block: &Node) {
        let shift = |vec: &mut Vec<usize>, new_count: usize| {
            let Some((&before, &after)) = vec.get(at).zip(vec.get(at + 1)) else {
                return;
            };
            let old_count = after - before;
            if new_count == old_count {
                return;
            }
            let delta = new_count as isize - old_count as isize;
            if let Some(tail) = vec.get_mut(at + 1..) {
                for slot in tail {
                    *slot = slot.wrapping_add_signed(delta);
                }
            }
        };
        shift(&mut self.tasks_before, block.tasks_count());
        shift(&mut self.parts_before, block.parts_count());
        shift(
            &mut self.h2_before,
            usize::from(matches!(block.kind, Kind::Heading(2))),
        );
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(from = "Wire")]
pub struct Node {
    pub kind: Kind,
    /// A text node's text, or a code element's.
    pub text: String,
    pub children: Vec<Node>,
    flow: OnceCell<Vec<Piece>>,
    code_metrics: OnceCell<Rc<CodeMetrics>>,
    /// A top-level block's size when last laid out, at the width it had then.
    pub(crate) size: Cell<Option<Size<Pixels>>>,
    /// The reply-gutter inset applied to a top-level block when `size` was measured.
    pub(crate) inset: Cell<Pixels>,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub align: Vec<TextAlign>,
    pub head: Option<Vec<Node>>,
}

/// What a run of text is marked as, combined from the elements around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks(u8);

impl Marks {
    pub const STRONG: Marks = Marks(1);
    pub const EMPHASIS: Marks = Marks(2);
    pub const STRIKE: Marks = Marks(4);
    pub const CODE: Marks = Marks(8);
    pub const LINK: Marks = Marks(16);
    pub const FAINT: Marks = Marks(32);
    /// A script in a formula that Unicode has no small letters for.
    pub const SMALL: Marks = Marks(64);

    pub fn has(self, other: Marks) -> bool {
        self.0 & other.0 != 0
    }
}

/// A run of inline content flattened into one string, ready to lay out.
#[derive(Clone, Debug, Default)]
pub struct Rich {
    pub text: SharedString,
    pub marks: Vec<(Range<usize>, Marks)>,
    pub links: Vec<(Range<usize>, SharedString)>,
    /// The runs that name what could be files, each with the name as written.
    pub paths: Vec<(Range<usize>, SharedString)>,
}

/// A picture drawn on its own, and where a click on it leads.
#[derive(Clone, Debug)]
pub struct Picture {
    pub src: SharedString,
    pub alt: SharedString,
    pub link: Option<SharedString>,
}

/// An element's content in reading order: runs of inline text, and the
/// children that stand as blocks of their own.
#[derive(Clone, Debug)]
pub enum Piece {
    Text(Rich),
    Block(usize),
}

impl Node {
    /// The table written back as markdown, so it pastes into a doc or another agent as a
    /// table; none for a node that is not one.
    pub fn table_source(&self) -> Option<String> {
        let Kind::Table(table) = &self.kind else {
            return None;
        };
        let row = |cells: &[Node]| {
            let cells: Vec<String> = cells.iter().map(Node::inline_source).collect();
            format!("| {} |", cells.join(" | "))
        };
        let rule = |align: &TextAlign| match align {
            TextAlign::Center => ":---:",
            TextAlign::Right => "---:",
            TextAlign::Left => "---",
        };
        let head = table.head.as_deref().map_or_else(
            || format!("| {} |", vec![""; table.align.len()].join(" | ")),
            row,
        );
        let rules: Vec<&str> = table.align.iter().map(rule).collect();
        let lines = [head, format!("| {} |", rules.join(" | "))]
            .into_iter()
            .chain(self.children.iter().map(|line| row(&line.children)));
        Some(lines.collect::<Vec<_>>().join("\n"))
    }

    /// A cell's content as the markdown that wrote it.
    fn inline_source(&self) -> String {
        self.written(true)
    }

    /// A paragraph or list item written back as markdown, as a reply quotes it.
    pub fn quoted(&self) -> String {
        match &self.kind {
            Kind::Bullets | Kind::Numbers(_) => (self.children.iter())
                .map(|item| format!("- {}", item.quoted()))
                .collect::<Vec<_>>()
                .join("\n"),
            Kind::Paragraph | Kind::Item(_) | Kind::Quote => {
                let mut lines: Vec<String> = Vec::new();
                let mut inline = String::new();
                for child in &self.children {
                    if child.kind.is_inline() {
                        inline.push_str(&child.written(false));
                        continue;
                    }
                    if !inline.trim().is_empty() {
                        lines.push(std::mem::take(&mut inline).trim().to_owned());
                    }
                    lines.push(child.quoted());
                }
                if !inline.trim().is_empty() {
                    lines.push(inline.trim().to_owned());
                }
                lines.join("\n")
            }
            _ => self.written(false),
        }
    }

    /// Inline content as the markdown that wrote it, with a cell's bars escaped when `cell`.
    fn written(&self, cell: bool) -> String {
        let inner = || {
            self.children
                .iter()
                .map(|child| child.written(cell))
                .collect::<String>()
        };
        let raw = |text: &str| match cell {
            true => text.replace('|', "\\|"),
            false => text.to_owned(),
        };
        match &self.kind {
            Kind::Text => raw(&self.text),
            Kind::InlineCode => format!("`{}`", raw(&self.text)),
            Kind::Math => format!("${}$", raw(&self.text)),
            Kind::Strong => format!("**{}**", inner()),
            Kind::Emphasis => format!("*{}*", inner()),
            Kind::Strike => format!("~~{}~~", inner()),
            Kind::Link(href) => format!("[{}]({href})", inner()),
            Kind::Image { src, alt } => format!("![{alt}]({src})"),
            Kind::Break => "<br>".to_owned(),
            Kind::FootnoteRef(n) => format!("[^{n}]"),
            _ => inner(),
        }
    }
}

impl Node {
    pub fn is_only_pictures(&self) -> bool {
        if !matches!(self.kind, Kind::Paragraph) {
            return false;
        }
        let mut any = false;
        for child in &self.children {
            match &child.kind {
                Kind::Image { .. } => any = true,
                Kind::Link(_) => {
                    for inner in &child.children {
                        if !matches!(inner.kind, Kind::Image { .. }) {
                            return false;
                        }
                        any = true;
                    }
                }
                Kind::Text if child.text.trim().is_empty() => {}
                Kind::Break => {}
                _ => return false,
            }
        }
        any
    }

    pub(crate) fn tasks_count(&self) -> usize {
        let own = usize::from(matches!(self.kind, Kind::Item(Some(_))));
        own + self.children.iter().map(Node::tasks_count).sum::<usize>()
    }

    pub(crate) fn parts_count(&self) -> usize {
        match self.kind {
            Kind::Paragraph => usize::from(!self.is_only_pictures()),
            Kind::Bullets | Kind::Numbers(_) => self.children.len(),
            _ => self.children.iter().map(Node::parts_count).sum(),
        }
    }

    pub(crate) fn code_metrics(&self) -> Rc<CodeMetrics> {
        self.code_metrics
            .get_or_init(|| Rc::new(CodeMetrics::from_text(&self.text)))
            .clone()
    }

    /// The pictures of a paragraph that holds only pictures, linked or not, with nothing but
    /// space between them, as a README's row of badges does; none for one with words.
    pub fn pictures(&self) -> Option<Vec<Picture>> {
        if !matches!(self.kind, Kind::Paragraph) {
            return None;
        }
        let mut pictures = Vec::new();
        for child in &self.children {
            match &child.kind {
                Kind::Image { src, alt } => pictures.push(Picture {
                    src: src.clone(),
                    alt: alt.clone(),
                    link: None,
                }),
                Kind::Link(href) => {
                    for inner in &child.children {
                        let Kind::Image { src, alt } = &inner.kind else {
                            return None;
                        };
                        pictures.push(Picture {
                            src: src.clone(),
                            alt: alt.clone(),
                            link: Some(href.clone()),
                        });
                    }
                }
                Kind::Text if child.text.trim().is_empty() => {}
                Kind::Break => {}
                _ => return None,
            }
        }
        (!pictures.is_empty()).then_some(pictures)
    }

    /// The node's content as it is drawn, worked out once until a patch reaches inside it.
    pub fn flow(&self) -> &[Piece] {
        self.flow.get_or_init(|| {
            if let Kind::Code(..) = self.kind {
                return vec![Piece::Text(Rich {
                    text: self.text.trim_end_matches('\n').to_owned().into(),
                    ..Rich::default()
                })];
            }
            let mut pieces = Vec::new();
            let mut rich: Option<Builder> = None;
            for (ix, child) in self.children.iter().enumerate() {
                if child.kind.is_inline() {
                    rich.get_or_insert_default()
                        .flatten(child, Marks::default(), None);
                    continue;
                }
                pieces.extend(rich.take().map(|rich| Piece::Text(rich.finish())));
                pieces.push(Piece::Block(ix));
            }
            pieces.extend(rich.map(|rich| Piece::Text(rich.finish())));
            pieces
        })
    }

    /// The texts a reader can select in this node, in the order they are drawn.
    /// `open` says whether each fold, met in order, shows what is in it.
    fn walk(&self, texts: &mut Vec<SharedString>, open: &mut dyn FnMut(bool) -> bool) {
        match &self.kind {
            Kind::Table(_) => {
                for cell in self.children.iter().flat_map(|row| &row.children) {
                    if let [Piece::Text(rich)] = cell.flow() {
                        texts.push(rich.text.clone());
                    }
                }
            }
            Kind::Details { open: written, .. } if !open(*written) => {}
            Kind::Paragraph if self.is_only_pictures() => {}
            _ => {
                for piece in self.flow() {
                    match piece {
                        Piece::Text(rich) => texts.push(rich.text.clone()),
                        Piece::Block(ix) => {
                            if let Some(child) = self.children.get(*ix) {
                                child.walk(texts, open);
                            }
                        }
                    }
                }
            }
        }
    }

    fn passage(
        &self,
        block: usize,
        texts: &mut Vec<SharedString>,
        open: &mut dyn FnMut(bool) -> bool,
    ) -> Passage {
        let from = texts.len() as u32 + 1;
        self.walk(texts, open);
        Passage {
            quoted: self.quoted(),
            block,
            texts: from..texts.len() as u32 + 1,
        }
    }

    /// Every part a reply can quote in reading order, its texts counted on from `texts`. One
    /// in a closed fold is given without texts.
    fn passages(
        &self,
        block: usize,
        texts: &mut Vec<SharedString>,
        open: &mut dyn FnMut(bool) -> bool,
        into: &mut Vec<Passage>,
    ) {
        match &self.kind {
            Kind::Paragraph if !self.is_only_pictures() => {
                into.push(self.passage(block, texts, open));
            }
            Kind::Bullets | Kind::Numbers(_) => {
                for item in &self.children {
                    into.push(item.passage(block, texts, open));
                }
            }
            Kind::Paragraph | Kind::Table(_) => self.walk(texts, open),
            Kind::Details { open: written, .. } if !open(*written) => {
                let mut held = Vec::new();
                for child in &self.children {
                    child.passages(block, &mut Vec::new(), &mut |_| true, &mut held);
                }
                into.extend(held.into_iter().map(|held| Passage {
                    texts: 0..0,
                    ..held
                }));
            }
            _ => {
                for piece in self.flow() {
                    match piece {
                        Piece::Text(rich) => texts.push(rich.text.clone()),
                        Piece::Block(ix) => {
                            if let Some(child) = self.children.get(*ix) {
                                child.passages(block, texts, open, into);
                            }
                        }
                    }
                }
            }
        }
    }

    /// How many characters of the node a reader sees, its folds as they were written.
    fn shown_len(&self) -> usize {
        let mut texts = Vec::new();
        self.walk(&mut texts, &mut |written| written);
        texts.iter().map(|text| text.chars().count()).sum()
    }

    /// A code block's first lines up to `limit` characters, ending at a line break when
    /// one comes in the second half.
    fn code_head(&self, limit: usize) -> Option<Node> {
        let Kind::Code(..) = self.kind else {
            return None;
        };
        let end = self
            .text
            .char_indices()
            .nth(limit)
            .map_or(self.text.len(), |(at, _)| at);
        let head = self.text.get(..end)?;
        let end = match head.rfind('\n') {
            Some(at)
                if head
                    .get(..at)
                    .is_some_and(|lines| lines.chars().count() >= limit / 2) =>
            {
                at
            }
            _ => end,
        };
        let mut head = self.clone();
        head.text.truncate(end);
        head.touched();
        Some(head)
    }

    fn touched(&mut self) {
        self.flow = OnceCell::new();
        self.code_metrics = OnceCell::new();
        self.size.set(None);
    }

    fn update_code_metrics(&mut self, cut: usize) {
        self.flow = OnceCell::new();
        self.size.set(None);
        let Some(metrics) = self.code_metrics.get_mut() else {
            return;
        };
        let trimmed = self.text.trim_end_matches('\n');
        if cut >= metrics.trimmed_len && trimmed.len() >= metrics.trimmed_len {
            Rc::make_mut(metrics).extend_trimmed(trimmed);
        } else {
            self.code_metrics = OnceCell::new();
        }
    }
}

/// Writes inline nodes into one string, recording the runs marked and the links.
#[derive(Default)]
struct Builder(
    String,
    Vec<(Range<usize>, Marks)>,
    Vec<(Range<usize>, SharedString)>,
    Vec<(Range<usize>, SharedString)>,
);

impl Builder {
    fn push(&mut self, text: &str, marks: Marks) {
        let start = self.0.len();
        self.0.push_str(text);
        let end = self.0.len();
        match self.1.last_mut() {
            _ if marks == Marks::default() || start == end => {}
            Some((range, last))
                if range.end == start && *last == marks && !marks.has(Marks::CODE) =>
            {
                range.end = end
            }
            _ => self.1.push((start..end, marks)),
        }
    }

    fn flatten(&mut self, node: &Node, marks: Marks, link: Option<&SharedString>) {
        let with = |mark: Marks| Marks(marks.0 | mark.0);
        let (marks, link) = match &node.kind {
            Kind::Text => return self.push(&node.text, marks),
            Kind::InlineCode => return self.push(&node.text, with(Marks::CODE)),
            Kind::Math => {
                for (text, look) in crate::math_text::runs(&node.text) {
                    let mut kept = marks;
                    if look.italic {
                        kept = Marks(kept.0 | Marks::EMPHASIS.0);
                    }
                    if look.small {
                        kept = Marks(kept.0 | Marks::SMALL.0);
                    }
                    self.push(&text, kept);
                }
                return;
            }
            Kind::Break => return self.push("\n", marks),
            Kind::FootnoteRef(n) => return self.push(&superscript(*n), marks),
            Kind::Image { alt, .. } => return self.push(alt, marks),
            Kind::Strong => (with(Marks::STRONG), link),
            Kind::Emphasis => (with(Marks::EMPHASIS), link),
            Kind::Strike => (with(Marks::STRIKE), link),
            Kind::Faint => (with(Marks::FAINT), link),
            Kind::Link(href) if link.is_none() => (with(Marks::LINK), Some(href)),
            _ => (marks, link),
        };
        let start = self.0.len();
        node.children
            .iter()
            .for_each(|child| self.flatten(child, marks, link));
        if let (Some(href), Kind::Link(_), true) = (link, &node.kind, self.0.len() > start) {
            self.2.push((start..self.0.len(), href.clone()));
        }
        if let (None, Kind::Path(name), true) = (link, &node.kind, self.0.len() > start) {
            self.3.push((start..self.0.len(), name.clone()));
        }
    }

    fn finish(self) -> Rich {
        Rich {
            text: self.0.into(),
            marks: self.1,
            links: self.2,
            paths: self.3,
        }
    }
}

pub fn superscript(n: u64) -> String {
    const DIGITS: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    n.to_string()
        .bytes()
        .filter_map(|digit| DIGITS.get(usize::from(digit - b'0')).copied())
        .collect()
}

/// A node as it arrives: a string is text, an array a group, an object an element.
#[derive(Deserialize)]
#[serde(untagged)]
enum Wire {
    Text(String),
    Group(Vec<Node>),
    Element(Box<Element>),
}

/// An element's fields; each element sends the few it has.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Element {
    t: String,
    c: Vec<Node>,
    rows: Vec<Node>,
    head: Option<Vec<Node>>,
    v: String,
    l: u8,
    n: u64,
    start: Option<u64>,
    checked: Option<bool>,
    lang: Option<SharedString>,
    name: Option<SharedString>,
    /// A table's side for each column, or the one side of an aligned block.
    align: Value,
    summary: SharedString,
    open: bool,
    math: bool,
    href: SharedString,
    src: SharedString,
    alt: SharedString,
}

impl From<Wire> for Node {
    fn from(wire: Wire) -> Node {
        let element = match wire {
            Wire::Text(text) => {
                return Node {
                    text,
                    ..Node::default()
                }
            }
            Wire::Group(children) => {
                return Node {
                    kind: Kind::Group,
                    children,
                    ..Node::default()
                }
            }
            Wire::Element(element) => *element,
        };
        let kind = match element.t.as_str() {
            "p" => Kind::Paragraph,
            "h" => Kind::Heading(element.l.clamp(1, 6)),
            "quote" => Kind::Quote,
            "ul" => Kind::Bullets,
            "ol" => Kind::Numbers(element.start.unwrap_or(1)),
            "li" => Kind::Item(element.checked),
            "pre" => Kind::Code(element.lang, element.name),
            "hr" => Kind::Rule,
            "table" => Kind::Table(Box::new(Table {
                align: (element.align.as_array().into_iter().flatten())
                    .map(side)
                    .collect(),
                head: element.head,
            })),
            "div" => Kind::Aligned(side(&element.align)),
            "fndef" => Kind::Footnote(element.n),
            "details" => Kind::Details {
                summary: element.summary,
                open: element.open,
            },
            "em" => Kind::Emphasis,
            "strong" => Kind::Strong,
            "del" => Kind::Strike,
            "faint" => Kind::Faint,
            "code" if element.math => Kind::Math,
            "code" => Kind::InlineCode,
            "a" => Kind::Link(element.href),
            "path" => Kind::Path(element.href),
            "img" => Kind::Image {
                src: element.src,
                alt: element.alt,
            },
            "br" => Kind::Break,
            "fnref" => Kind::FootnoteRef(element.n),
            _ => Kind::Unknown,
        };
        let children = if element.rows.is_empty() {
            element.c
        } else {
            element.rows
        };
        Node {
            kind,
            text: element.v,
            children,
            ..Node::default()
        }
    }
}

fn side(align: &Value) -> TextAlign {
    match align.as_str() {
        Some("center") => TextAlign::Center,
        Some("right") => TextAlign::Right,
        _ => TextAlign::Left,
    }
}

/// A patch to a message's tree, as the README describes it.
#[derive(Deserialize)]
struct Patch {
    n: usize,
    o: Vec<Op>,
}

#[derive(Deserialize)]
struct Op {
    p: Vec<usize>,
    at: usize,
    c: Option<Vec<Node>>,
    s: Option<String>,
}

/// The byte in `text` where its first `units` UTF-16 units end.
fn byte_at_units(text: &str, units: usize) -> usize {
    if text.is_ascii() {
        return units.min(text.len());
    }
    let mut counted = 0;
    for (at, ch) in text.char_indices() {
        if counted >= units {
            return at;
        }
        counted += ch.len_utf16();
    }
    text.len()
}

/// A part of a message a reply can quote: the markdown quoted, and where its texts are, by block
/// and as `Markdown::texts` counts them from 1.
#[derive(Clone, Debug, PartialEq)]
pub struct Passage {
    pub quoted: String,
    pub(crate) block: usize,
    pub(crate) texts: Range<u32>,
}

/// One message's blocks, as the last patch left them, and what drawing them
/// last left: the width they had and the folds the reader opened or closed.
#[derive(Default)]
pub struct Markdown {
    blocks: Vec<Rc<Node>>,
    prefixes: RefCell<Option<Rc<Prefixes>>>,
    /// The text's length the tree matches, or `None` once a patch did not fit
    /// and the tree waits for a whole one.
    units: Option<usize>,
    pub(crate) width: Rc<Cell<Option<Pixels>>>,
    /// The width the blocks took the first time they were all laid out, as wide as their
    /// longest line where nothing stretched them.
    pub(crate) natural: Rc<Cell<Option<Pixels>>>,
    /// Each fold the reader turned from how it was written, by block and its place in the block.
    pub(crate) turned: Rc<RefCell<HashSet<(usize, u32)>>>,
    /// Where its first block was placed in the window when last drawn.
    pub(crate) top: Rc<Cell<Option<Pixels>>>,
}

impl Markdown {
    /// A tree read from its JSON blocks, matching a text `units` UTF-16 units long.
    pub fn new(blocks: &Value, units: usize) -> Self {
        let blocks = Vec::<Node>::deserialize(blocks).unwrap_or_default();
        Markdown {
            blocks: blocks.into_iter().map(Rc::new).collect(),
            units: Some(units),
            ..Markdown::default()
        }
    }

    /// A message with no tree yet; one arrives as a patch that starts from nothing.
    pub fn empty() -> Self {
        Markdown::default()
    }

    pub fn blocks(&self) -> &[Rc<Node>] {
        &self.blocks
    }

    pub(crate) fn prefixes(&self) -> Rc<Prefixes> {
        let mut cache = self.prefixes.borrow_mut();
        cache
            .get_or_insert_with(|| Rc::new(Prefixes::from_blocks(&self.blocks)))
            .clone()
    }

    /// The texts of block `ix` a reader can select, in the order they are drawn.
    pub fn texts(&self, ix: usize) -> Vec<SharedString> {
        let mut texts = Vec::new();
        let turned = self.turned.borrow();
        let mut folds = 0;
        if let Some(block) = self.blocks.get(ix) {
            block.walk(&mut texts, &mut |written| {
                folds += 1;
                written != turned.contains(&(ix, folds))
            });
        }
        texts
    }

    /// Every part of the message a reply can quote, in the order they are drawn.
    pub fn passages(&self) -> Vec<Passage> {
        let mut passages = Vec::new();
        let turned = self.turned.borrow();
        for (ix, block) in self.blocks.iter().enumerate() {
            let mut folds = 0;
            block.passages(
                ix,
                &mut Vec::new(),
                &mut |written| {
                    folds += 1;
                    written != turned.contains(&(ix, folds))
                },
                &mut passages,
            );
        }
        passages
    }

    /// The whole message as one passage for each of its blocks.
    pub fn whole(&self) -> Vec<Passage> {
        (self.blocks.iter().enumerate())
            .map(|(ix, _)| Passage {
                quoted: String::new(),
                block: ix,
                texts: 1..self.texts(ix).len() as u32 + 1,
            })
            .collect()
    }

    /// The first whole blocks of a text over `limit` characters and how many characters they
    /// leave out, or the start of a first block of code longer than that; none when little
    /// would be left out.
    pub fn cut(&self, limit: usize) -> Option<(Vec<Rc<Node>>, usize)> {
        let lens: Vec<usize> = self.blocks.iter().map(|block| block.shown_len()).collect();
        let total: usize = lens.iter().sum();
        if total <= limit {
            return None;
        }
        let (mut kept, mut count) = (0, 0);
        while let Some(&len) = lens.get(count) {
            if kept + len > limit {
                break;
            }
            kept += len;
            count += 1;
        }
        let blocks = if count > 0 {
            self.blocks.iter().take(count).cloned().collect()
        } else {
            let head = self.blocks.first()?.code_head(limit)?;
            kept = head.shown_len();
            vec![Rc::new(head)]
        };
        let hidden = total - kept;
        (hidden >= limit / 10).then_some((blocks, hidden))
    }

    /// Applies a patch `{ n, o }`, leaving every block it does not reach as it was.
    pub fn write(&mut self, patch: &Value, units: usize) {
        let Ok(patch) = Patch::deserialize(patch) else {
            return;
        };
        let whole = patch
            .o
            .first()
            .is_some_and(|op| op.p.is_empty() && op.at == 0 && op.c.is_some());
        if !whole && self.units.is_none() {
            return;
        }
        for op in patch.o {
            self.apply(op);
        }
        self.units = (patch.n == units).then_some(units);
    }

    fn apply(&mut self, op: Op) {
        let Some((first, rest)) = op.p.split_first() else {
            self.blocks.truncate(op.at);
            self.blocks.extend(op.c.into_iter().flatten().map(Rc::new));
            if let Some(prefixes) = self.prefixes.get_mut().as_mut() {
                let prefixes = Rc::make_mut(prefixes);
                prefixes.truncate(op.at);
                if let Some(added) = self.blocks.get(op.at..) {
                    prefixes.extend(added);
                }
            }
            return;
        };
        let first = *first;
        let Some(block) = self.blocks.get_mut(first) else {
            return;
        };
        let mut node = Rc::make_mut(block);
        let text_in_place = rest.is_empty() && op.c.is_none() && op.s.is_some();
        if !text_in_place {
            node.touched();
        }
        for &ix in rest {
            let Some(child) = node.children.get_mut(ix) else {
                return;
            };
            node = child;
            node.touched();
        }
        if let Some(added) = op.c {
            node.children.truncate(op.at);
            node.children.extend(added);
            if let Some(prefixes) = self.prefixes.get_mut().as_mut() {
                if let Some(block) = self.blocks.get(first) {
                    Rc::make_mut(prefixes).update_at(first, block);
                }
            }
        } else if let Some(added) = op.s {
            let cut = byte_at_units(&node.text, op.at);
            node.text.truncate(cut);
            node.text.push_str(&added);
            if text_in_place {
                node.update_code_metrics(cut);
            } else if let Some(prefixes) = self.prefixes.get_mut().as_mut() {
                if let Some(block) = self.blocks.get(first) {
                    Rc::make_mut(prefixes).update_at(first, block);
                }
            }
        }
    }
}
