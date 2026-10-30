//! Finding text in a transcript, or in one markdown drawn on its own: every place a query is
//! found, in reading order.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{Pixels, SharedString};

use crate::draw::LayoutBudget;
use crate::select::{RowHit, Selecting, Shown};
use crate::{Markdown, Pen, Style};

/// What a reader looks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
}

/// A place the query was found: the text it is in, by row, doc, block and text, and
/// its bytes there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub at: (usize, usize, usize, u32),
    pub range: Range<usize>,
}

/// The places `query` is found in `text`, in order, each a range of `text` itself.
pub(crate) fn ranges_in(text: &str, query: &Query) -> Vec<Range<usize>> {
    if query.text.is_empty() {
        return Vec::new();
    }
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let whole = |range: &Range<usize>| {
        !query.whole_word
            || (!text[..range.start].chars().next_back().is_some_and(word)
                && !text[range.end..].chars().next().is_some_and(word))
    };
    let found: Vec<Range<usize>> = if query.match_case {
        found_in(text, &query.text)
    } else if text.is_ascii() && query.text.is_ascii() {
        found_in(&text.to_ascii_lowercase(), &query.text.to_ascii_lowercase())
    } else {
        folded_found_in(text, &query.text)
    };
    found.into_iter().filter(whole).collect()
}

fn found_in(hay: &str, needle: &str) -> Vec<Range<usize>> {
    hay.match_indices(needle)
        .map(|(at, found)| at..at + found.len())
        .collect()
}

/// Finds `needle` in `text` with both lowered a character at a time, as `É` lowers to `é`
/// but `İ` to two characters, and answers each place as the whole characters of `text` it covers.
fn folded_found_in(text: &str, needle: &str) -> Vec<Range<usize>> {
    let needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    let mut hay = String::with_capacity(text.len());
    let mut from: Vec<Range<usize>> = Vec::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        let source = at..at + c.len_utf8();
        hay.extend(c.to_lowercase());
        from.resize(hay.len(), source);
    }
    found_in(&hay, &needle)
        .into_iter()
        .filter_map(|found| {
            let (first, last) = (&from[found.start], &from[found.end - 1]);
            let starts = found.start == 0 || from[found.start - 1] != *first;
            let ends = from.get(found.end).is_none_or(|next| next != last);
            (starts && ends).then_some(first.start..last.end)
        })
        .collect()
}

/// The transcript came to find something else, or the reader moved to another place found.
pub struct FoundChanged;

/// Find in one markdown drawn on its own, as a file's preview is: the places found, the one the
/// reader is on, and where the last frame drew its texts.
#[derive(Clone)]
pub struct DocFind {
    hits: Vec<Hit>,
    current: usize,
    shown: Rc<RefCell<Vec<Shown>>>,
    budget: Rc<LayoutBudget>,
}

impl DocFind {
    pub fn new(doc: &Markdown, query: &Query) -> DocFind {
        let mut hits = Vec::new();
        for block in 0..doc.blocks().len() {
            for (n, text) in (1..).zip(doc.texts(block)) {
                hits.extend(ranges_in(&text, query).into_iter().map(|range| Hit {
                    at: (0, 0, block, n),
                    range,
                }));
            }
        }
        DocFind {
            hits,
            current: 0,
            shown: Rc::default(),
            budget: Rc::default(),
        }
    }

    /// Which place found the reader is on, counting from 0, and how many there are.
    pub fn found(&self) -> (usize, usize) {
        (self.current, self.hits.len())
    }

    /// Moves to the next place found, or the one before, round the ends.
    pub fn step(&mut self, forward: bool) {
        let total = self.hits.len();
        if total == 0 {
            return;
        }
        self.current = match forward {
            true => (self.current + 1) % total,
            false => (self.current + total - 1) % total,
        };
    }

    /// A pen that draws `doc` with every place found marked, and the reader's own apart.
    pub fn pen(&self, style: Rc<Style>, doc: &Markdown) -> Pen {
        self.shown.borrow_mut().clear();
        self.budget.reset();
        let hits = (self.hits.iter().enumerate())
            .map(|(ix, hit)| RowHit {
                text: (hit.at.1, hit.at.2, hit.at.3),
                range: hit.range.clone(),
                current: ix == self.current,
            })
            .collect();
        let selecting = Selecting {
            row: SharedString::default(),
            ix: 0,
            docs: vec![doc as *const Markdown],
            span: None,
            shown: self.shown.clone(),
            hits,
            budget: self.budget.clone(),
        };
        Pen::selecting(style, Some(selecting))
    }

    /// How far below the top of `doc` the place the reader is on is, as the last frame laid it
    /// out: its line once that frame drew its text, else the top of its block.
    pub fn current_offset(&self, doc: &Markdown, style: &Style) -> Option<Pixels> {
        let hit = self.hits.get(self.current)?;
        let (_, _, block, n) = hit.at;
        let line = self.shown.borrow().iter().find_map(|text| {
            let held = (text.key.1, text.key.2, text.key.3) == (0, block, n)
                && (text.offset..=text.offset + text.len).contains(&hit.range.start);
            held.then(|| {
                text.layout
                    .position_for_index(hit.range.start - text.offset)
            })?
        });
        if let (Some(line), Some(top)) = (line, doc.top.get()) {
            return Some(line.y - top);
        }
        doc.blocks()
            .get(..block)?
            .iter()
            .try_fold(Pixels::ZERO, |sum, node| {
                Some(sum + node.size.get()?.height + style.gap)
            })
    }
}
