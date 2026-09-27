//! Reads markdown into a small tree of the elements the app draws. Each
//! top-level block is its own entry, so a message still being written can
//! redraw only the block at its end.

mod autolink;
mod url;

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
}

/// Parses `text` and returns its top-level blocks from index `skip` on. A
/// caller that already holds the blocks before `skip` gets only the rest.
pub fn parse(text: &str, options: Options, skip: usize) -> Vec<Element> {
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
        seen: 0,
        skip,
        links_open: 0,
        footnotes: Vec::new(),
    };
    for event in Parser::new_ext(text, flags) {
        builder.event(event);
    }
    builder.blocks
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
    seen: usize,
    skip: usize,
    links_open: usize,
    footnotes: Vec<String>,
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
                | Element::A { c, .. },
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
        }
        self.seen += 1;
    }

    fn children(&self, pieces: Vec<Piece>) -> Vec<Node> {
        Self::nodes(pieces, self.options.gfm && self.links_open == 0)
    }

    fn nodes(pieces: Vec<Piece>, link_addresses: bool) -> Vec<Node> {
        let mut nodes = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match piece {
                Piece::Node(Node::Text(text)) if link_addresses => {
                    autolink::link_addresses(text, &mut nodes)
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

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_element(Element::Code {
                v: code.into_string(),
            }),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.push_text(&math),
            Event::Html(html) => self.push_text(&html),
            Event::InlineHtml(html) => {
                if self.options.html_as_text {
                    self.push(Piece::Typed(html.into_string()));
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
                c: Self::nodes(frame.children, false),
            },
            Kind::Image { src, title } => {
                let mut alt = String::new();
                plain_text(&Self::nodes(frame.children, false), &mut alt);
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
                    return;
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
