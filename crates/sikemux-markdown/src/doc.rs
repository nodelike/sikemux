//! A message read as it is written: each new piece of text re-reads only the
//! blocks at its end, and says what changed as a patch to the tree a client
//! already holds. Markup still open at the end is drawn as if closed until the
//! message is done.

use serde::Serialize;
use serde_json::Value;

use crate::{mend::mend, parse, parse_blocks, Element, Options};

/// The line being written may still join the block before it, as a table row
/// or a heading's underline does, so the last two blocks stay open.
const OPEN_BLOCKS: usize = 2;

/// One change to a tree a client holds. `p` is a path of indexes from the
/// top-level blocks down through each element's children (`c`, or a table's
/// `rows`, its cells and their contents). Positions in text count UTF-16
/// units, as clients slice it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Op {
    /// The list at `p` keeps its first `at` entries and gains `c`.
    List {
        p: Vec<usize>,
        at: usize,
        c: Vec<Value>,
    },
    /// The text at `p`, or a code block's `v` there, keeps its first `at` units and gains `s`.
    Text { p: Vec<usize>, at: usize, s: String },
}

/// The changes that turn one tree into the next, in order.
pub type Patch = Vec<Op>;

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The key an element keeps what grows inside it under.
const GROWS: [&str; 3] = ["c", "rows", "v"];

fn shared_text(old: &str, new: &str) -> usize {
    old.char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((at, _), _)| at)
}

fn text_op(path: Vec<usize>, old: &str, new: &str) -> Vec<Op> {
    let kept = shared_text(old, new);
    if kept == old.len() && kept == new.len() {
        return Vec::new();
    }
    vec![Op::Text {
        p: path,
        at: units(old.get(..kept).unwrap_or_default()),
        s: new.get(kept..).unwrap_or_default().to_owned(),
    }]
}

/// The changes inside `old` that make it `new`, when `new` is the same node grown.
fn grown(path: Vec<usize>, old: &Value, new: &Value) -> Option<Vec<Op>> {
    match (old, new) {
        (Value::String(a), Value::String(b)) => Some(text_op(path, a, b)),
        (Value::Array(a), Value::Array(b)) => Some(list_ops(path, a, b, 0)),
        (Value::Object(a), Value::Object(b)) => {
            let key = GROWS.into_iter().find(|key| a.contains_key(*key))?;
            let same_shell = a.len() == b.len()
                && a.iter()
                    .all(|(name, value)| name == key || b.get(name) == Some(value));
            if !same_shell {
                return None;
            }
            match (a.get(key)?, b.get(key)?) {
                (Value::String(a), Value::String(b)) => Some(text_op(path, a, b)),
                (Value::Array(a), Value::Array(b)) => Some(list_ops(path, a, b, 0)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn list_ops(path: Vec<usize>, old: &[Value], new: &[Value], first: usize) -> Vec<Op> {
    let same = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if same == old.len() && same == new.len() {
        return Vec::new();
    }
    let mut at = path.clone();
    at.push(first + same);
    let inside = old
        .get(same)
        .zip(new.get(same))
        .and_then(|(a, b)| grown(at, a, b));
    let (mut ops, from) = match inside {
        Some(ops) => (ops, same + 1),
        None => (Vec::new(), same),
    };
    if from < old.len().max(new.len()) {
        ops.push(Op::List {
            p: path,
            at: first + from,
            c: new.get(from..).unwrap_or_default().to_vec(),
        });
    }
    ops
}

/// The changes that turn the top-level blocks `old` into `new`, whose indexes start at `first`.
pub fn diff(old: &[Value], new: &[Value], first: usize) -> Patch {
    list_ops(Vec::new(), old, new, first)
}

fn values(blocks: Vec<Element>) -> Vec<Value> {
    blocks
        .into_iter()
        .filter_map(|block| serde_json::to_value(block).ok())
        .collect()
}

/// Footnotes and link definitions reach across the whole message, so a
/// message that has them is read from its start each time.
fn reaches_back(text: &str) -> bool {
    text.contains("[^") || text.contains("]:")
}

/// One message as it grows, and the blocks at its end its clients were last sent.
pub struct Doc {
    options: Options,
    text: String,
    units: usize,
    /// Where the open blocks start in `text`.
    base: usize,
    /// How many blocks before `base` no longer change.
    settled: usize,
    /// The blocks from `base` on, as last sent.
    open: Vec<Value>,
    whole: bool,
    /// Whether the last block sent had markup closed that the text leaves open.
    mended: bool,
}

impl Doc {
    /// A message whose clients hold nothing of its tree yet.
    pub fn new(options: Options) -> Self {
        Self {
            options,
            text: String::new(),
            units: 0,
            base: 0,
            settled: 0,
            open: Vec::new(),
            whole: false,
            mended: false,
        }
    }

    /// The whole tree of `text`, as one patch for a client that holds none of it.
    pub fn read(text: &str, options: Options) -> Patch {
        vec![Op::List {
            p: Vec::new(),
            at: 0,
            c: values(parse(text, options, 0)),
        }]
    }

    pub fn push(&mut self, more: &str) {
        let from = self.text.len().saturating_sub(1);
        self.text.push_str(more);
        self.units += units(more);
        if !self.whole && reaches_back(self.text.get(from..).unwrap_or(more)) {
            self.whole = true;
            self.base = 0;
            self.settled = 0;
            self.open.clear();
        }
    }

    /// How long the text is, in the UTF-16 units clients count it in.
    pub fn units(&self) -> usize {
        self.units
    }

    /// What the tree's clients must change to match the text so far, if anything.
    pub fn patch(&mut self) -> Patch {
        self.reread(true)
    }

    /// What the tree's clients must change once the message is done, to draw the markup that
    /// `patch` closed early as it was written.
    pub fn exact(&mut self) -> Patch {
        if !self.mended {
            return Vec::new();
        }
        self.reread(false)
    }

    fn reread(&mut self, mending: bool) -> Patch {
        let text = self.text.get(self.base..).unwrap_or_default();
        let (mut blocks, starts) = parse_blocks(text, self.options);
        let last = starts.len().saturating_sub(1);
        let mended = starts
            .last()
            .filter(|_| mending && !matches!(blocks.last(), Some(Element::Pre { .. })))
            .and_then(|&start| Some((text.get(..start)?, mend(text.get(start..)?)?)));
        self.mended = mended.is_some();
        if let Some((before, block)) = mended {
            blocks.truncate(last);
            blocks.extend(parse(&(before.to_owned() + &block), self.options, last));
        }
        let blocks = values(blocks);
        let patch = diff(&self.open, &blocks, self.settled);
        let closed = if self.whole {
            0
        } else {
            starts.len().saturating_sub(OPEN_BLOCKS)
        };
        if let Some(start) = starts.get(closed).filter(|_| closed > 0) {
            self.base += start;
            self.settled += closed;
        }
        self.open = blocks;
        self.open.drain(..closed);
        patch
    }
}
