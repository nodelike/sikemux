//! Reads markdown into a small tree of the elements the app draws. Each
//! top-level block is its own entry, so a message still being written can
//! redraw only the block at its end.

mod autolink;
mod delimiters;
mod doc;
mod mend;
pub mod paths;
mod reference;
mod sentences;
mod url;

pub use doc::{diff, Doc, MdChange, MdOp, Patch};

/// The language a block of math is marked with, from `$$…$$` or a ```` ```math ```` fence.
pub const MATH: &str = "math";
pub use reference::name as reference;
pub use sentences::unsourced;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, LinkType, Options as ParserOptions, Parser, Tag, TagEnd,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownOptions {
    /// Tables, strikethrough, task lists, footnotes and bare addresses as links.
    #[serde(default)]
    pub gfm: bool,
    /// Shows markup as the characters that were typed instead of dropping it.
    #[serde(default)]
    pub html_as_text: bool,
    /// Keeps `file://` and drive-letter links, which are dropped otherwise.
    #[serde(default)]
    pub file_links: bool,
    /// Keeps the pictures in embedded `<img>` tags, which GitHub writes for an
    /// uploaded image, while the rest of the markup is still dropped.
    #[serde(default)]
    pub html_images: bool,
    /// Keeps what GitHub draws of embedded markup besides pictures: the alignment `<div>` and
    /// `<p>` give the blocks inside them, and `<h1>` to `<h6>` headings.
    #[serde(default)]
    pub html_layout: bool,
    /// Marks the names in prose and between backticks that could be files, for a reader to
    /// check and draw as files when they are.
    #[serde(default)]
    pub file_paths: bool,
    /// Writes a link that shows its own address, to a pull request or issue, as GitHub
    /// writes a reference to one, such as `owner/repo#12`.
    #[serde(default)]
    pub references: bool,
    /// Reads `$…$` as inline math and `$$…$$` as a block of math, kept as LaTeX.
    #[serde(default)]
    pub math: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MdNode {
    Text(String),
    Element(MdElement),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MdAlign {
    Left,
    Center,
    Right,
}

type Cell = Vec<MdNode>;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum MdElement {
    P {
        c: Vec<MdNode>,
    },
    H {
        l: u8,
        c: Vec<MdNode>,
    },
    Quote {
        c: Vec<MdNode>,
    },
    Ul {
        c: Vec<MdNode>,
    },
    Ol {
        #[serde(skip_serializing_if = "Option::is_none")]
        start: Option<u64>,
        c: Vec<MdNode>,
    },
    Li {
        #[serde(skip_serializing_if = "Option::is_none")]
        checked: Option<bool>,
        c: Vec<MdNode>,
    },
    Pre {
        #[serde(skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
        /// The file the block shows, when its fence names one.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        v: String,
    },
    Hr,
    Table {
        align: Vec<Option<MdAlign>>,
        /// Missing when every heading cell is blank.
        #[serde(skip_serializing_if = "Option::is_none")]
        head: Option<Vec<Cell>>,
        rows: Vec<Vec<Cell>>,
    },
    /// Footnotes are numbered in the order they are first mentioned.
    Fndef {
        n: usize,
        c: Vec<MdNode>,
    },
    Em {
        c: Vec<MdNode>,
    },
    Strong {
        c: Vec<MdNode>,
    },
    Del {
        c: Vec<MdNode>,
    },
    /// Words drawn faint: sentences an answer left out, put back for a reader who asked to see
    /// them. Reading markdown never makes one.
    Faint {
        c: Vec<MdNode>,
    },
    Code {
        v: String,
        /// `v` is LaTeX math the message wrote between single dollar signs.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        math: bool,
    },
    A {
        href: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// `href` names a file rather than a place on the web.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        file: bool,
        c: Vec<MdNode>,
    },
    Img {
        src: String,
        alt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Br,
    Fnref {
        n: usize,
    },
    /// A name that could be a file, `:line` and all, holding the words as written, between
    /// backticks when `code` is set.
    Path {
        href: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        code: bool,
        c: Vec<MdNode>,
    },
    /// Blocks embedded HTML lines up to one side, as `<div align="center">` does.
    Div {
        align: MdAlign,
        c: Vec<MdNode>,
    },
    /// A `<details>` fold from embedded HTML, open or not as it was written.
    Details {
        summary: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        open: bool,
        c: Vec<MdNode>,
    },
}

/// Parses `text` and returns its top-level blocks from index `skip` on. A
/// caller that already holds the blocks before `skip` gets only the rest.
pub fn parse(text: &str, options: MarkdownOptions, skip: usize) -> Vec<MdElement> {
    read(text, options, skip).0
}

/// Parses `text` into its top-level blocks, each with the byte in `text` it starts at.
pub fn parse_blocks(text: &str, options: MarkdownOptions) -> (Vec<MdElement>, Vec<usize>) {
    read(text, options, 0)
}

/// The words `text` says with its markdown taken off, each block on a line of its own and
/// code blocks left out, such as a first message read as a chat's title.
pub fn words(text: &str) -> String {
    let flags = ParserOptions::ENABLE_TABLES | ParserOptions::ENABLE_STRIKETHROUGH;
    let mut out = String::with_capacity(text.len());
    let mut in_code_block = false;
    for event in Parser::new_ext(text, flags) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Text(said) | Event::Code(said) | Event::InlineHtml(said) | Event::Html(said)
                if !in_code_block =>
            {
                out.push_str(&said)
            }
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableCell,
            ) => out.push('\n'),
            _ => {}
        }
    }
    out
}

fn read(text: &str, options: MarkdownOptions, skip: usize) -> (Vec<MdElement>, Vec<usize>) {
    let mut flags = ParserOptions::empty();
    if options.gfm {
        flags |= ParserOptions::ENABLE_TABLES
            | ParserOptions::ENABLE_STRIKETHROUGH
            | ParserOptions::ENABLE_TASKLISTS
            | ParserOptions::ENABLE_FOOTNOTES;
    }
    if options.math {
        flags |= ParserOptions::ENABLE_MATH;
    }
    let mut builder = Builder {
        options,
        stack: Vec::new(),
        blocks: Vec::new(),
        starts: Vec::new(),
        opening: None,
        seen: 0,
        skip,
        links_open: 0,
        footnotes: Vec::new(),
    };
    let rewritten = options.math.then(|| delimiters::as_dollars(text)).flatten();
    let (read, moved) = match &rewritten {
        Some((dollars, moved)) => (dollars.as_str(), Some(moved)),
        None => (text, None),
    };
    for (event, range) in Parser::new_ext(read, flags).into_offset_iter() {
        if builder.stack.is_empty() {
            let start = moved.map_or(range.start, |moved| moved.original(range.start));
            builder.opening.get_or_insert(start);
        }
        builder.event(event);
        if builder.stack.is_empty() {
            builder.opening = None;
        }
    }
    while builder.closes_html() {}
    (builder.blocks, builder.starts)
}

/// Text that came from markup the reader typed. It stays apart from the text
/// around it so that nothing reads it as an address.
enum Piece {
    MdNode(MdNode),
    Typed(String),
}

enum Kind {
    Paragraph,
    Heading(u8),
    Quote,
    List(Option<u64>),
    Item(Option<bool>),
    FootnoteDefinition(usize),
    Emphasis,
    Strong,
    Strikethrough,
    Link {
        href: String,
        title: Option<String>,
    },
    Image {
        src: String,
        title: Option<String>,
    },
    Code {
        lang: Option<String>,
        name: Option<String>,
        text: String,
    },
    Html(String),
    Table {
        align: Vec<Option<MdAlign>>,
        head: Option<Vec<Cell>>,
        rows: Vec<Vec<Cell>>,
    },
    Row(Vec<Cell>),
    TableCell,
    Details {
        summary: String,
        open: bool,
    },
    /// A `<div>` or `<p>` from embedded HTML, and the side it lines its blocks up to.
    Aligned(Option<MdAlign>),
    Unsupported,
}

struct Frame {
    kind: Kind,
    children: Vec<Piece>,
}

struct Builder {
    options: MarkdownOptions,
    stack: Vec<Frame>,
    blocks: Vec<MdElement>,
    /// Where each of `blocks` starts in the text.
    starts: Vec<usize>,
    /// Where the block being read now starts.
    opening: Option<usize>,
    seen: usize,
    skip: usize,
    links_open: usize,
    footnotes: Vec<String>,
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower.get(from..)?.find(name) {
        let start = from + at;
        from = start + name.len();
        let after_space = lower.get(..start)?.ends_with(char::is_whitespace);
        let Some(rest) = tag.get(from..)?.trim_start().strip_prefix('=') else {
            continue;
        };
        if !after_space {
            continue;
        }
        let rest = rest.trim_start();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let value = rest.get(1..)?;
        return value
            .find(quote)
            .and_then(|close| value.get(..close))
            .map(str::to_owned);
    }
    None
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in html.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tag's name, with `/` before it for a closing tag: `div` for `<div align="center">`.
fn tag_name(tag: &str) -> &str {
    let inside = tag.trim_start_matches('<');
    let end = inside
        .char_indices()
        .skip(1)
        .find(|(_, c)| c.is_whitespace() || matches!(c, '>' | '/'))
        .map_or(inside.len(), |(at, _)| at);
    inside.get(..end).unwrap_or_default()
}

fn aligned(side: &str) -> Option<MdAlign> {
    match side.trim().to_ascii_lowercase().as_str() {
        "left" => Some(MdAlign::Left),
        "center" => Some(MdAlign::Center),
        "right" => Some(MdAlign::Right),
        _ => None,
    }
}

fn nonempty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

fn align(alignment: Alignment) -> Option<MdAlign> {
    match alignment {
        Alignment::None => None,
        Alignment::Left => Some(MdAlign::Left),
        Alignment::Center => Some(MdAlign::Center),
        Alignment::Right => Some(MdAlign::Right),
    }
}

fn plain_text(nodes: &[MdNode], out: &mut String) {
    for node in nodes {
        match node {
            MdNode::Text(text) => out.push_str(text),
            MdNode::Element(MdElement::Code { v, .. } | MdElement::Pre { v, .. }) => {
                out.push_str(v)
            }
            MdNode::Element(
                MdElement::P { c }
                | MdElement::H { c, .. }
                | MdElement::Em { c }
                | MdElement::Strong { c }
                | MdElement::Del { c }
                | MdElement::Faint { c }
                | MdElement::A { c, .. }
                | MdElement::Path { c, .. },
            ) => plain_text(c, out),
            MdNode::Element(_) => {}
        }
    }
}

fn is_blank(cells: &[Cell]) -> bool {
    let mut text = String::new();
    for cell in cells {
        plain_text(cell, &mut text);
    }
    text.trim().is_empty()
}

fn fit_row(mut cells: Vec<Cell>, width: usize) -> Vec<Cell> {
    cells.resize_with(width, Vec::new);
    cells
}

impl Builder {
    fn open(&mut self, kind: Kind) {
        self.stack.push(Frame {
            kind,
            children: Vec::new(),
        });
    }

    fn push(&mut self, piece: Piece) {
        let Some(frame) = self.stack.last_mut() else {
            if let Piece::MdNode(MdNode::Element(element)) = piece {
                self.finish_block(element);
            }
            return;
        };
        if let (Piece::MdNode(MdNode::Text(more)), Some(Piece::MdNode(MdNode::Text(text)))) =
            (&piece, frame.children.last_mut())
        {
            text.push_str(more);
            return;
        }
        frame.children.push(piece);
    }

    fn push_text(&mut self, text: &str) {
        match self.stack.last_mut().map(|frame| &mut frame.kind) {
            Some(Kind::Code { text: code, .. } | Kind::Html(code)) => code.push_str(text),
            _ => self.push(Piece::MdNode(MdNode::Text(text.to_owned()))),
        }
    }

    fn push_element(&mut self, element: MdElement) {
        self.push(Piece::MdNode(MdNode::Element(element)));
    }

    fn finish_block(&mut self, element: MdElement) {
        if self.seen >= self.skip {
            self.blocks.push(element);
            self.starts.push(self.opening.unwrap_or_default());
        }
        self.seen += 1;
    }

    fn children(&self, pieces: Vec<Piece>) -> Vec<MdNode> {
        let outside_links = self.links_open == 0;
        let nodes = Self::nodes(
            pieces,
            self.options.gfm && outside_links,
            self.options.file_paths && outside_links,
        );
        if self.options.references {
            nodes.into_iter().map(reference::fold).collect()
        } else {
            nodes
        }
    }

    fn nodes(pieces: Vec<Piece>, link_addresses: bool, file_paths: bool) -> Vec<MdNode> {
        let mut nodes = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match piece {
                Piece::MdNode(MdNode::Text(text)) if link_addresses || file_paths => {
                    let mut linked = Vec::new();
                    if link_addresses {
                        autolink::link_addresses(text, &mut linked);
                    } else {
                        linked.push(MdNode::Text(text));
                    }
                    for node in linked {
                        match node {
                            MdNode::Text(text) if file_paths => paths::mark_paths(text, &mut nodes),
                            node => nodes.push(node),
                        }
                    }
                }
                Piece::MdNode(node) => nodes.push(node),
                Piece::Typed(text) => nodes.push(MdNode::Text(text)),
            }
        }
        nodes
    }

    fn footnote(&mut self, label: &str) -> usize {
        match self.footnotes.iter().position(|seen| seen == label) {
            Some(index) => index + 1,
            None => {
                self.footnotes.push(label.to_owned());
                self.footnotes.len()
            }
        }
    }

    fn url(&self, url: &str) -> String {
        url::sanitize(url, self.options.file_links)
    }

    /// Reads a block of HTML the way GitHub draws it, in the order it writes things: folds,
    /// aligned blocks, headings and pictures, each picture inside the link around it. The rest
    /// of the markup is dropped.
    fn html_block(&mut self, html: &str) {
        let lower = html.to_ascii_lowercase();
        let mut pictures = Vec::new();
        let mut link: Option<String> = None;
        let mut from = 0;
        while let Some(at) = lower.get(from..).and_then(|rest| rest.find('<')) {
            let start = from + at;
            let close = lower
                .get(start..)
                .and_then(|tail| tail.find('>'))
                .map_or(html.len(), |end| start + end + 1);
            let tag = html.get(start..close).unwrap_or_default();
            let name = tag_name(lower.get(start..close).unwrap_or_default());
            from = close.max(start + 1);
            let folds = self.options.gfm;
            let layout = self.options.html_layout;
            match name {
                "details" | "summary" | "/details" | "div" | "p" | "/div" | "/p" | "h1" | "h2"
                | "h3" | "h4" | "h5" | "h6"
                    if folds || layout =>
                {
                    self.flush_pictures(&mut pictures);
                }
                _ => {}
            }
            match name {
                "details" if folds => {
                    let open = attribute(tag, "open").is_some()
                        || lower.get(start..close).is_some_and(|t| t.contains(" open"));
                    self.open(Kind::Details {
                        summary: String::new(),
                        open,
                    });
                }
                "summary" if folds => {
                    let end = lower
                        .get(close..)
                        .and_then(|tail| tail.find("</summary"))
                        .map_or(html.len(), |at| close + at);
                    let words = strip_tags(html.get(close..end).unwrap_or_default());
                    if let Some(Frame {
                        kind: Kind::Details { summary, .. },
                        ..
                    }) = self.stack.last_mut()
                    {
                        *summary = words;
                    }
                    from = end.max(close);
                }
                "/details" if folds => {
                    if self.top_is(|kind| matches!(kind, Kind::Details { .. })) {
                        self.closes_html();
                    }
                }
                "div" | "p" if layout => {
                    let side = attribute(tag, "align").and_then(|side| aligned(&side));
                    self.open(Kind::Aligned(side));
                }
                "/div" | "/p" if layout => {
                    if self.top_is(|kind| matches!(kind, Kind::Aligned(_))) {
                        self.closes_html();
                    }
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if layout => {
                    let level = name.as_bytes().get(1).map_or(1, |digit| digit - b'0');
                    let end = lower
                        .get(close..)
                        .and_then(|tail| tail.find(&format!("</{name}")))
                        .map_or(html.len(), |at| close + at);
                    let inner = html.get(close..end).unwrap_or_default();
                    let mut c = Vec::new();
                    if self.options.html_images {
                        for image in self.html_images(inner) {
                            c.push(MdNode::Element(image));
                        }
                    }
                    let words = strip_tags(inner);
                    if !words.is_empty() {
                        c.push(MdNode::Text(words));
                    }
                    let heading = MdElement::H { l: level, c };
                    match attribute(tag, "align").and_then(|side| aligned(&side)) {
                        Some(align) => self.push_element(MdElement::Div {
                            align,
                            c: vec![MdNode::Element(heading)],
                        }),
                        None => self.push_element(heading),
                    }
                    from = end.max(close);
                }
                "a" if self.options.html_images => {
                    link = attribute(tag, "href")
                        .map(|href| self.url(&href))
                        .filter(|href| !href.is_empty());
                }
                "/a" => link = None,
                "img" if self.options.html_images => {
                    for image in self.html_images(tag) {
                        pictures.push(match &link {
                            Some(href) => MdElement::A {
                                href: href.clone(),
                                title: None,
                                file: false,
                                c: vec![MdNode::Element(image)],
                            },
                            None => image,
                        });
                    }
                }
                _ => {}
            }
        }
        self.flush_pictures(&mut pictures);
    }

    fn flush_pictures(&mut self, pictures: &mut Vec<MdElement>) {
        if pictures.is_empty() {
            return;
        }
        self.open(Kind::Paragraph);
        for picture in pictures.drain(..) {
            self.push_element(picture);
        }
        self.end(TagEnd::Paragraph);
    }

    fn top_is(&self, kind: impl Fn(&Kind) -> bool) -> bool {
        self.stack.last().is_some_and(|frame| kind(&frame.kind))
    }

    /// Closes the fold or aligned block HTML opened last, if it is the one open; says
    /// whether it was.
    fn closes_html(&mut self) -> bool {
        if !self.top_is(|kind| matches!(kind, Kind::Details { .. } | Kind::Aligned(_))) {
            return false;
        }
        let Some(frame) = self.stack.pop() else {
            return false;
        };
        match frame.kind {
            Kind::Details { summary, open } => {
                let c = self.children(frame.children);
                self.push_element(MdElement::Details { summary, open, c });
            }
            Kind::Aligned(Some(align)) => {
                let c = self.children(frame.children);
                self.push_element(MdElement::Div { align, c });
            }
            _ => {
                for piece in frame.children {
                    self.push(piece);
                }
            }
        }
        true
    }

    fn html_images(&self, html: &str) -> Vec<MdElement> {
        let lower = html.to_ascii_lowercase();
        let mut images = Vec::new();
        let mut from = 0;
        while let Some(at) = lower.get(from..).and_then(|rest| rest.find("<img")) {
            let start = from + at;
            let end = lower
                .get(start..)
                .and_then(|rest| rest.find('>'))
                .map_or(html.len(), |close| start + close);
            let tag = html.get(start..end).unwrap_or_default();
            if let Some(src) = attribute(tag, "src")
                .map(|src| self.url(&src))
                .filter(|src| !src.is_empty())
            {
                images.push(MdElement::Img {
                    src,
                    alt: attribute(tag, "alt").unwrap_or_default(),
                    title: attribute(tag, "title"),
                });
            }
            from = end.max(start + 1);
        }
        images
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code)
                if self.options.file_paths
                    && self.links_open == 0
                    && paths::names_whole_path(&code) =>
            {
                self.push(Piece::MdNode(paths::path(&code, true)));
            }
            Event::Code(code) => self.push_element(MdElement::Code {
                v: code.into_string(),
                math: false,
            }),
            Event::InlineMath(math) => self.push_element(MdElement::Code {
                v: math.into_string(),
                math: true,
            }),
            Event::DisplayMath(math) => self.push_element(MdElement::Pre {
                lang: Some(MATH.to_owned()),
                name: None,
                v: format!("{}\n", math.trim()),
            }),
            Event::Html(html) => self.push_text(&html),
            Event::InlineHtml(html) => {
                if self.options.html_as_text {
                    self.push(Piece::Typed(html.into_string()));
                } else if self.options.html_layout && tag_name(&html.to_ascii_lowercase()) == "br" {
                    self.push_element(MdElement::Br);
                } else if self.options.html_images {
                    for image in self.html_images(&html) {
                        self.push_element(image);
                    }
                }
            }
            Event::FootnoteReference(label) => {
                let n = self.footnote(&label);
                self.push_element(MdElement::Fnref { n });
            }
            Event::SoftBreak => self.push_text("\n"),
            Event::HardBreak => self.push_element(MdElement::Br),
            Event::Rule => self.push_element(MdElement::Hr),
            Event::TaskListMarker(checked) => {
                if let Some(Frame {
                    kind: Kind::Item(state),
                    ..
                }) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find(|frame| matches!(frame.kind, Kind::Item(_)))
                {
                    *state = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        let kind = match tag {
            Tag::Paragraph => Kind::Paragraph,
            Tag::Heading { level, .. } => Kind::Heading(level as u8),
            Tag::BlockQuote(_) => Kind::Quote,
            Tag::CodeBlock(kind) => {
                let (lang, name) = match kind {
                    CodeBlockKind::Fenced(info) => fence_info(&info),
                    CodeBlockKind::Indented => (None, None),
                };
                Kind::Code {
                    lang,
                    name,
                    text: String::new(),
                }
            }
            Tag::HtmlBlock => Kind::Html(String::new()),
            Tag::List(start) => Kind::List(start),
            Tag::Item => Kind::Item(None),
            Tag::FootnoteDefinition(label) => Kind::FootnoteDefinition(self.footnote(&label)),
            Tag::Table(alignments) => Kind::Table {
                align: alignments.into_iter().map(align).collect(),
                head: None,
                rows: Vec::new(),
            },
            Tag::TableHead | Tag::TableRow => Kind::Row(Vec::new()),
            Tag::TableCell => Kind::TableCell,
            Tag::Emphasis => Kind::Emphasis,
            Tag::Strong => Kind::Strong,
            Tag::Strikethrough => Kind::Strikethrough,
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                self.links_open += 1;
                let href = if link_type == LinkType::Email {
                    format!("mailto:{dest_url}")
                } else {
                    dest_url.into_string()
                };
                Kind::Link {
                    href: self.url(&href),
                    title: nonempty(&title),
                }
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                self.links_open += 1;
                Kind::Image {
                    src: self.url(&dest_url),
                    title: nonempty(&title),
                }
            }
            _ => Kind::Unsupported,
        };
        self.open(kind);
    }

    fn end(&mut self, tag: TagEnd) {
        if matches!(tag, TagEnd::Link | TagEnd::Image) {
            self.links_open = self.links_open.saturating_sub(1);
        }
        let Some(frame) = self.stack.pop() else {
            return;
        };
        let element = match frame.kind {
            Kind::Paragraph => MdElement::P {
                c: self.children(frame.children),
            },
            Kind::Heading(level) => MdElement::H {
                l: level,
                c: self.children(frame.children),
            },
            Kind::Quote => MdElement::Quote {
                c: self.children(frame.children),
            },
            Kind::List(Some(start)) => MdElement::Ol {
                start: (start != 1).then_some(start),
                c: self.children(frame.children),
            },
            Kind::List(None) => MdElement::Ul {
                c: self.children(frame.children),
            },
            Kind::Item(checked) => MdElement::Li {
                checked,
                c: self.children(frame.children),
            },
            Kind::FootnoteDefinition(n) => MdElement::Fndef {
                n,
                c: self.children(frame.children),
            },
            Kind::Emphasis => MdElement::Em {
                c: self.children(frame.children),
            },
            Kind::Strong => MdElement::Strong {
                c: self.children(frame.children),
            },
            Kind::Strikethrough => MdElement::Del {
                c: self.children(frame.children),
            },
            Kind::Link { href, title } => MdElement::A {
                file: paths::names_whole_path(&href),
                href,
                title,
                c: Self::nodes(frame.children, false, false),
            },
            Kind::Image { src, title } => {
                let mut alt = String::new();
                plain_text(&Self::nodes(frame.children, false, false), &mut alt);
                MdElement::Img { src, alt, title }
            }
            Kind::Code {
                lang,
                name,
                mut text,
            } => {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                MdElement::Pre {
                    lang,
                    name,
                    v: text,
                }
            }
            Kind::Html(html) => {
                if !self.options.html_as_text {
                    return self.html_block(&html);
                }
                self.open(Kind::Paragraph);
                self.push(Piece::Typed(html.trim_end_matches(['\n', '\r']).to_owned()));
                return self.end(TagEnd::Paragraph);
            }
            Kind::TableCell => {
                let cell = self.children(frame.children);
                if let Some(Frame {
                    kind: Kind::Row(cells),
                    ..
                }) = self.stack.last_mut()
                {
                    cells.push(cell);
                }
                return;
            }
            Kind::Row(cells) => {
                if let Some(Frame {
                    kind: Kind::Table { align, head, rows },
                    ..
                }) = self.stack.last_mut()
                {
                    let cells = fit_row(cells, align.len());
                    if tag == TagEnd::TableHead {
                        *head = (!is_blank(&cells)).then_some(cells);
                    } else {
                        rows.push(cells);
                    }
                }
                return;
            }
            Kind::Table { align, head, rows } => MdElement::Table { align, head, rows },
            Kind::Details { .. } | Kind::Aligned(_) => return,
            Kind::Unsupported => {
                for piece in frame.children {
                    self.push(piece);
                }
                return;
            }
        };
        self.push_element(element);
    }
}

/// A fence's language and the file it names: `rust:src/main.rs`, `rust title="main.rs"`, or a
/// bare `main.rs`, whose language is its extension.
fn fence_info(info: &str) -> (Option<String>, Option<String>) {
    let mut words = info.split_whitespace();
    let Some(first) = words.next() else {
        return (None, None);
    };
    let named = words.find_map(|word| {
        let (key, value) = word.split_once('=')?;
        matches!(key, "title" | "file" | "filename" | "path")
            .then(|| value.trim_matches(['"', '\'']).to_owned())
            .filter(|value| !value.is_empty())
    });
    if let Some((lang, name)) = first
        .split_once(':')
        .filter(|(lang, name)| !lang.is_empty() && !name.is_empty())
    {
        return (
            Some(lang.to_owned()),
            named.or_else(|| Some(name.to_owned())),
        );
    }
    if first.contains(['.', '/']) {
        let extension = first
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_owned());
        return (extension, named.or_else(|| Some(first.to_owned())));
    }
    (Some(first.to_owned()), named)
}
