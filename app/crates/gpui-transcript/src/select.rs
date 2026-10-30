use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Pixels, Point, SharedString, TextLayout};

use crate::draw::LayoutBudget;
use crate::Markdown;

/// A text a transcript draws: its row, which markdown in the row, which block, which text in the block.
pub(crate) type Key = (SharedString, usize, usize, u32);

/// One end of a selection: a text and a byte in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Spot {
    pub key: Key,
    pub at: usize,
}

/// A place in reading order: the row's index, then the rest of a key, then the byte.
pub(crate) type Place = (usize, usize, usize, u32, usize);

/// A selection's ends in reading order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Span(pub Place, pub Place);

impl Span {
    /// The bytes of a text `len` long that the span covers.
    pub fn range(&self, text: (usize, usize, usize, u32), len: usize) -> Option<Range<usize>> {
        let (start, end) = (self.0, self.1);
        let (first, last) = (
            (start.0, start.1, start.2, start.3),
            (end.0, end.1, end.2, end.3),
        );
        if text < first || text > last {
            return None;
        }
        let from = if text == first { start.4 } else { 0 };
        let to = if text == last { end.4.min(len) } else { len };
        (from < to).then_some(from..to)
    }
}

impl Selecting {
    pub fn place_of(&self, doc: &Markdown) -> usize {
        self.docs
            .iter()
            .position(|held| std::ptr::eq(*held, doc))
            .unwrap_or(self.docs.len())
    }
}

/// A text as it was last placed, for finding what the pointer is over.
pub(crate) struct Shown {
    pub key: Key,
    pub layout: TextLayout,
    pub len: usize,
    /// Where in its text the part laid out starts, for a text drawn in parts.
    pub offset: usize,
}

impl Shown {
    fn distance(&self, point: Point<Pixels>) -> (f32, f32) {
        let bounds = self.layout.bounds();
        let off = |at: Pixels, low: Pixels, high: Pixels| {
            f32::from(if at < low {
                low - at
            } else if at > high {
                at - high
            } else {
                Pixels::ZERO
            })
        };
        (
            off(point.y, bounds.top(), bounds.bottom()),
            off(point.x, bounds.left(), bounds.right()),
        )
    }
}

/// The text nearest `point` among those last placed, and the byte under it.
pub(crate) fn spot_at(shown: &[Shown], point: Point<Pixels>) -> Option<Spot> {
    let nearest = shown.iter().min_by(|a, b| {
        let (a, b) = (a.distance(point), b.distance(point));
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
    })?;
    let at = match nearest.layout.index_for_position(point) {
        Ok(at) | Err(at) => at.min(nearest.len) + nearest.offset,
    };
    Some(Spot {
        key: nearest.key.clone(),
        at,
    })
}

/// The bytes of the word, run of spaces or run of punctuation under byte `at` of `text`.
pub(crate) fn word_at(text: &str, at: usize) -> Range<usize> {
    let kind = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            1
        } else {
            2
        }
    };
    let at = at.min(text.len());
    let (before, after) = (text[..at].chars().next_back(), text[at..].chars().next());
    let Some(under) = (match (before, after) {
        (Some(b), Some(a)) if kind(a) != 0 && kind(b) == 0 => Some(b),
        (_, Some(a)) => Some(a),
        (b, None) => b,
    }) else {
        return at..at;
    };
    let class = kind(under);
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| kind(*c) == class)
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| kind(*c) != class)
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// What a row's drawing needs from the transcript to show and record a selection.
pub(crate) struct Selecting {
    pub row: SharedString,
    pub ix: usize,
    /// Where each of the row's markdown is held, in the order `Row::docs` lists them.
    pub docs: Vec<*const Markdown>,
    pub span: Option<Span>,
    pub shown: Rc<RefCell<Vec<Shown>>>,
    /// The places found in the row's texts.
    pub hits: Vec<RowHit>,
    /// What the frame may still spend laying out new parts of code blocks.
    pub budget: Rc<LayoutBudget>,
}

/// A place found in a row: the text it is in, by doc, block and text, its bytes there, and
/// whether it is the one the reader is on.
pub struct RowHit {
    pub text: (usize, usize, u32),
    pub range: Range<usize>,
    pub current: bool,
}
