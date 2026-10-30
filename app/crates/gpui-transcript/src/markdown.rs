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
    Code(Option<SharedString>),
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
    InlineCode,
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
                | Kind::InlineCode
                | Kind::Link(_)
                | Kind::Path(_)
                | Kind::Break
                | Kind::FootnoteRef(_)
        )
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
    /// A top-level block's size when last laid out, at the width it had then.
    pub(crate) size: Cell<Option<Size<Pixels>>>,
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
        let inner = || {
            self.children
                .iter()
                .map(Node::inline_source)
                .collect::<String>()
        };
        match &self.kind {
            Kind::Text => self.text.replace('|', "\\|"),
            Kind::InlineCode => format!("`{}`", self.text.replace('|', "\\|")),
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
            if let Kind::Code(_) = self.kind {
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
            Kind::Paragraph if self.pictures().is_some() => {}
            _ => {
                for piece in self.flow() {
                    match piece {
                        Piece::Text(rich) => texts.push(rich.text.clone()),
                        Piece::Block(ix) => self.children[*ix].walk(texts, open),
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
        let Kind::Code(_) = self.kind else {
            return None;
        };
        let end = self
            .text
            .char_indices()
            .nth(limit)
            .map_or(self.text.len(), |(at, _)| at);
        let head = &self.text[..end];
        let end = match head.rfind('\n') {
            Some(at) if head[..at].chars().count() >= limit / 2 => at,
            _ => end,
        };
        let mut head = self.clone();
        head.text.truncate(end);
        head.touched();
        Some(head)
    }

    fn touched(&mut self) {
        self.flow = OnceCell::new();
        self.size.set(None);
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
            Some((range, last)) if range.end == start && *last == marks => range.end = end,
            _ => self.1.push((start..end, marks)),
        }
    }

    fn flatten(&mut self, node: &Node, marks: Marks, link: Option<&SharedString>) {
        let with = |mark: Marks| Marks(marks.0 | mark.0);
        let (marks, link) = match &node.kind {
            Kind::Text => return self.push(&node.text, marks),
            Kind::InlineCode => return self.push(&node.text, with(Marks::CODE)),
            Kind::Break => return self.push("\n", marks),
            Kind::FootnoteRef(n) => return self.push(&superscript(*n), marks),
            Kind::Image { alt, .. } => return self.push(alt, marks),
            Kind::Strong => (with(Marks::STRONG), link),
            Kind::Emphasis => (with(Marks::EMPHASIS), link),
            Kind::Strike => (with(Marks::STRIKE), link),
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
        .map(|digit| DIGITS[usize::from(digit - b'0')])
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
    /// A table's side for each column, or the one side of an aligned block.
    align: Value,
    summary: SharedString,
    open: bool,
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
            "pre" => Kind::Code(element.lang),
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
    let mut counted = 0;
    for (at, ch) in text.char_indices() {
        if counted >= units {
            return at;
        }
        counted += ch.len_utf16();
    }
    text.len()
}

/// One message's blocks, as the last patch left them, and what drawing them
/// last left: the width they had and the folds the reader opened or closed.
#[derive(Default)]
pub struct Markdown {
    blocks: Vec<Rc<Node>>,
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

    /// The texts of block `ix` a reader can select, in the order they are drawn.
    pub(crate) fn texts(&self, ix: usize) -> Vec<SharedString> {
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
        while count < lens.len() && kept + lens[count] <= limit {
            kept += lens[count];
            count += 1;
        }
        let blocks = if count > 0 {
            self.blocks[..count].to_vec()
        } else {
            let head = self.blocks[0].code_head(limit)?;
            kept = head.shown_len();
            vec![Rc::new(head)]
        };
        let hidden = total - kept;
        (hidden >= limit / 10).then_some((blocks, hidden))
    }

    /// Whether the tree matches the text; when it does not, draw the text as it is.
    pub fn is_current(&self) -> bool {
        self.units.is_some()
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
            return;
        };
        let Some(block) = self.blocks.get_mut(*first) else {
            return;
        };
        let mut node = Rc::make_mut(block);
        node.touched();
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
        } else if let Some(added) = op.s {
            node.text.truncate(byte_at_units(&node.text, op.at));
            node.text.push_str(&added);
        }
    }
}
