//! Reads markdown into a small tree of the elements the app draws. Each
//! top-level block is its own entry, so a message still being written can
//! redraw only the block at its end.

mod autolink;
mod doc;
mod mend;
pub mod paths;
mod reference;
mod url;

pub use doc::{diff, Doc, Op, Patch};
pub use reference::name as reference;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, LinkType, Options as ParserOptions, Parser, Tag, TagEnd,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
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
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Node {
    Text(String),
    Element(Element),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
}

type Cell = Vec<Node>;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Element {
    P {
        c: Vec<Node>,
    },
    H {
        l: u8,
        c: Vec<Node>,
    },
    Quote {
        c: Vec<Node>,
    },
    Ul {
        c: Vec<Node>,
    },
    Ol {
        #[serde(skip_serializing_if = "Option::is_none")]
        start: Option<u64>,
        c: Vec<Node>,
    },
    Li {
        #[serde(skip_serializing_if = "Option::is_none")]
        checked: Option<bool>,
        c: Vec<Node>,
    },
    Pre {
        #[serde(skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
        v: String,
    },
    Hr,
    Table {
        align: Vec<Option<Align>>,
        /// Missing when every heading cell is blank.
        #[serde(skip_serializing_if = "Option::is_none")]
        head: Option<Vec<Cell>>,
        rows: Vec<Vec<Cell>>,
    },
    /// Footnotes are numbered in the order they are first mentioned.
    Fndef {
        n: usize,
        c: Vec<Node>,
    },
    Em {
        c: Vec<Node>,
    },
    Strong {
        c: Vec<Node>,
    },
    Del {
        c: Vec<Node>,
    },
    Code {
        v: String,
    },
    A {
        href: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        c: Vec<Node>,
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
        c: Vec<Node>,
    },
    /// Blocks embedded HTML lines up to one side, as `<div align="center">` does.
    Div {
        align: Align,
        c: Vec<Node>,
    },
    /// A `<details>` fold from embedded HTML, open or not as it was written.
    Details {
        summary: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        open: bool,
        c: Vec<Node>,
    },
}

/// Parses `text` and returns its top-level blocks from index `skip` on. A
/// caller that already holds the blocks before `skip` gets only the rest.
pub fn parse(text: &str, options: Options, skip: usize) -> Vec<Element> {
    read(text, options, skip).0
}

/// Parses `text` into its top-level blocks, each with the byte in `text` it starts at.
pub fn parse_blocks(text: &str, options: Options) -> (Vec<Element>, Vec<usize>) {
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

fn read(text: &str, options: Options, skip: usize) -> (Vec<Element>, Vec<usize>) {
    let mut flags = ParserOptions::empty();
    if options.gfm {
        flags |= ParserOptions::ENABLE_TABLES
            | ParserOptions::ENABLE_STRIKETHROUGH
            | ParserOptions::ENABLE_TASKLISTS
            | ParserOptions::ENABLE_FOOTNOTES;
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
    for (event, range) in Parser::new_ext(text, flags).into_offset_iter() {
        if builder.stack.is_empty() {
            builder.opening.get_or_insert(range.start);
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
    Node(Node),
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
        text: String,
    },
    Html(String),
    Table {
        align: Vec<Option<Align>>,
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
    Aligned(Option<Align>),
    Unsupported,
}

struct Frame {
    kind: Kind,
    children: Vec<Piece>,
}

struct Builder {
    options: Options,
    stack: Vec<Frame>,
    blocks: Vec<Element>,
    /// Where each of `blocks` starts in the text.
    starts: Vec<usize>,
    /// Where the block being read now starts.
    opening: Option<usize>,
    seen: usize,
    skip: usize,
    links_open: usize,
    footnotes: Vec<String>,
}

/// The value of `name="…"` or `name='…'` in one HTML tag.
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

/// The words in a run of HTML, with its tags taken out and its spaces collapsed.
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

fn aligned(side: &str) -> Option<Align> {
    match side.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Align::Left),
        "center" => Some(Align::Center),
        "right" => Some(Align::Right),
        _ => None,
    }
}

fn nonempty(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

fn align(alignment: Alignment) -> Option<Align> {
    match alignment {
        Alignment::None => None,
        Alignment::Left => Some(Align::Left),
        Alignment::Center => Some(Align::Center),
        Alignment::Right => Some(Align::Right),
    }
}

fn plain_text(nodes: &[Node], out: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Element(Element::Code { v } | Element::Pre { v, .. }) => out.push_str(v),
            Node::Element(
                Element::P { c }
                | Element::H { c, .. }
                | Element::Em { c }
                | Element::Strong { c }
                | Element::Del { c }
                | Element::A { c, .. }
                | Element::Path { c, .. },
            ) => plain_text(c, out),
            Node::Element(_) => {}
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
            if let Piece::Node(Node::Element(element)) = piece {
                self.finish_block(element);
            }
            return;
        };
        if let (Piece::Node(Node::Text(more)), Some(Piece::Node(Node::Text(text)))) =
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
            _ => self.push(Piece::Node(Node::Text(text.to_owned()))),
        }
    }

    fn push_element(&mut self, element: Element) {
        self.push(Piece::Node(Node::Element(element)));
    }

    fn finish_block(&mut self, element: Element) {
        if self.seen >= self.skip {
            self.blocks.push(element);
            self.starts.push(self.opening.unwrap_or_default());
        }
        self.seen += 1;
    }

    fn children(&self, pieces: Vec<Piece>) -> Vec<Node> {
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

    fn nodes(pieces: Vec<Piece>, link_addresses: bool, file_paths: bool) -> Vec<Node> {
        let mut nodes = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match piece {
                Piece::Node(Node::Text(text)) if link_addresses || file_paths => {
                    let mut linked = Vec::new();
                    if link_addresses {
                        autolink::link_addresses(text, &mut linked);
                    } else {
                        linked.push(Node::Text(text));
                    }
                    for node in linked {
                        match node {
                            Node::Text(text) if file_paths => paths::mark_paths(text, &mut nodes),
                            node => nodes.push(node),
                        }
                    }
                }
                Piece::Node(node) => nodes.push(node),
                Piece::Typed(text) => nodes.push(Node::Text(text)),
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
                            c.push(Node::Element(image));
                        }
                    }
                    let words = strip_tags(inner);
                    if !words.is_empty() {
                        c.push(Node::Text(words));
                    }
                    let heading = Element::H { l: level, c };
                    match attribute(tag, "align").and_then(|side| aligned(&side)) {
                        Some(align) => self.push_element(Element::Div {
                            align,
                            c: vec![Node::Element(heading)],
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
                            Some(href) => Element::A {
                                href: href.clone(),
                                title: None,
                                c: vec![Node::Element(image)],
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

    /// Puts the pictures read so far in a paragraph of their own.
    fn flush_pictures(&mut self, pictures: &mut Vec<Element>) {
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
                self.push_element(Element::Details { summary, open, c });
            }
            Kind::Aligned(Some(align)) => {
                let c = self.children(frame.children);
                self.push_element(Element::Div { align, c });
            }
            _ => {
                for piece in frame.children {
                    self.push(piece);
                }
            }
        }
        true
    }

    fn html_images(&self, html: &str) -> Vec<Element> {
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
                images.push(Element::Img {
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
                self.push(Piece::Node(paths::path(&code, true)));
            }
            Event::Code(code) => self.push_element(Element::Code {
                v: code.into_string(),
            }),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.push_text(&math),
            Event::Html(html) => self.push_text(&html),
            Event::InlineHtml(html) => {
                if self.options.html_as_text {
                    self.push(Piece::Typed(html.into_string()));
                } else if self.options.html_layout && tag_name(&html.to_ascii_lowercase()) == "br" {
                    self.push_element(Element::Br);
                } else if self.options.html_images {
                    for image in self.html_images(&html) {
                        self.push_element(image);
                    }
                }
            }
            Event::FootnoteReference(label) => {
                let n = self.footnote(&label);
                self.push_element(Element::Fnref { n });
            }
            Event::SoftBreak => self.push_text("\n"),
            Event::HardBreak => self.push_element(Element::Br),
            Event::Rule => self.push_element(Element::Hr),
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
            Tag::CodeBlock(kind) => Kind::Code {
                lang: match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_owned)
                    }
                    CodeBlockKind::Indented => None,
                },
                text: String::new(),
            },
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
            Kind::Paragraph => Element::P {
                c: self.children(frame.children),
            },
            Kind::Heading(level) => Element::H {
                l: level,
                c: self.children(frame.children),
            },
            Kind::Quote => Element::Quote {
                c: self.children(frame.children),
            },
            Kind::List(Some(start)) => Element::Ol {
                start: (start != 1).then_some(start),
                c: self.children(frame.children),
            },
            Kind::List(None) => Element::Ul {
                c: self.children(frame.children),
            },
            Kind::Item(checked) => Element::Li {
                checked,
                c: self.children(frame.children),
            },
            Kind::FootnoteDefinition(n) => Element::Fndef {
                n,
                c: self.children(frame.children),
            },
            Kind::Emphasis => Element::Em {
                c: self.children(frame.children),
            },
            Kind::Strong => Element::Strong {
                c: self.children(frame.children),
            },
            Kind::Strikethrough => Element::Del {
                c: self.children(frame.children),
            },
            Kind::Link { href, title } => Element::A {
                href,
                title,
                c: Self::nodes(frame.children, false, false),
            },
            Kind::Image { src, title } => {
                let mut alt = String::new();
                plain_text(&Self::nodes(frame.children, false, false), &mut alt);
                Element::Img { src, alt, title }
            }
            Kind::Code { lang, mut text } => {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                Element::Pre { lang, v: text }
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
            Kind::Table { align, head, rows } => Element::Table { align, head, rows },
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
