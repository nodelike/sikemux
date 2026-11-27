use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    px, Bounds, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ScrollHandle, SharedString, TextLayout, Window,
};

use crate::draw::LayoutBudget;
use crate::find::DocFind;
use crate::{Markdown, Pen, Style};

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

/// The most a selection dragged past an edge scrolls in one step.
const MOST_EDGE_STEP: f32 = 60.;

/// How far to scroll toward a pointer at `at`, past `low` or `high`, as a selection is dragged:
/// negative toward `low`, none while it is between them.
pub(crate) fn edge_step(at: Pixels, low: Pixels, high: Pixels) -> Pixels {
    let past = if at < low {
        at - low
    } else if at > high {
        at - high
    } else {
        Pixels::ZERO
    };
    px(f32::from(past).clamp(-MOST_EDGE_STEP, MOST_EDGE_STEP))
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
    let Some((head, tail)) = text.split_at_checked(at) else {
        return at..at;
    };
    let (before, after) = (head.chars().next_back(), tail.chars().next());
    let Some(under) = (match (before, after) {
        (Some(b), Some(a)) if kind(a) != 0 && kind(b) == 0 => Some(b),
        (_, Some(a)) => Some(a),
        (b, None) => b,
    }) else {
        return at..at;
    };
    let class = kind(under);
    let start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| kind(*c) == class)
        .last()
        .map_or(at, |(i, _)| i);
    let end = tail
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
    pub hits: Vec<RowHit>,
    /// What the frame may still spend laying out new parts of code blocks.
    pub budget: Rc<LayoutBudget>,
}

/// A place marked in a row: the text it is in, by doc, block and text, its bytes there, and why.
pub struct RowHit {
    pub text: (usize, usize, u32),
    pub range: Range<usize>,
    pub mark: Mark,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Found,
    /// The place found the reader is on.
    Current,
    /// A passage shown for a moment, as strongly as this.
    Passage(f32),
}

/// The text `span` covers in `markdown`, the `doc`th of row `ix`, a string per block it reaches.
pub(crate) fn picked(markdown: &Markdown, span: Span, ix: usize, doc: usize) -> Vec<String> {
    (0..markdown.blocks().len())
        .filter_map(|block| {
            let texts = markdown.texts(block);
            let picked: Vec<&str> = (1..)
                .zip(&texts)
                .filter_map(|(n, text)| text.get(span.range((ix, doc, block, n), text.len())?))
                .collect();
            (!picked.is_empty()).then(|| picked.join("\n"))
        })
        .collect()
}

/// Selecting in one markdown drawn on its own, as a file's preview is: the selection's ends
/// and where the last frame drew its texts. Clones share the same selection.
#[derive(Clone, Default)]
pub struct DocSelect {
    ends: Rc<RefCell<Option<(Spot, Spot)>>>,
    dragging: Rc<Cell<bool>>,
    pub(crate) shown: Rc<RefCell<Vec<Shown>>>,
    budget: Rc<LayoutBudget>,
}

impl DocSelect {
    pub fn clear(&self) {
        self.ends.borrow_mut().take();
        self.dragging.set(false);
    }

    fn span(&self) -> Option<Span> {
        let ends = self.ends.borrow();
        let (anchor, head) = ends.as_ref()?;
        let place = |spot: &Spot| (0, spot.key.1, spot.key.2, spot.key.3, spot.at);
        let (anchor, head) = (place(anchor), place(head));
        Some(Span(anchor.min(head), anchor.max(head)))
    }

    /// A pen that draws `doc` with the selection, and every place `found` found marked.
    pub fn pen(&self, style: Rc<Style>, doc: &Markdown, found: Option<&DocFind>) -> Pen {
        self.pen_many(style, &[doc], found)
    }

    /// A pen that draws any of `docs` with one shared selection, and every place `found` marked.
    pub fn pen_many(&self, style: Rc<Style>, docs: &[&Markdown], found: Option<&DocFind>) -> Pen {
        self.shown.borrow_mut().clear();
        self.budget.reset();
        Pen::selecting(
            style,
            Some(Selecting {
                row: SharedString::default(),
                ix: 0,
                docs: docs.iter().map(|doc| *doc as *const Markdown).collect(),
                span: self.span(),
                shown: self.shown.clone(),
                hits: found.map(DocFind::row_hits).unwrap_or_default(),
                budget: self.budget.clone(),
            }),
        )
    }

    /// Starts a selection at the press, or picks the word or text under a double or triple click.
    pub fn press(&self, event: &MouseDownEvent, doc: &Markdown) {
        self.press_many(event, &[doc]);
    }

    /// Starts a selection across `docs` at the press, or picks the word or text under a double
    /// or triple click.
    pub fn press_many(&self, event: &MouseDownEvent, docs: &[&Markdown]) {
        let Some(spot) = spot_at(&self.shown.borrow(), event.position) else {
            return;
        };
        let (_, doc_ix, block, n) = spot.key;
        let text = docs
            .get(doc_ix)
            .and_then(|doc| doc.texts(block).get((n as usize).wrapping_sub(1)).cloned());
        let mut ends = self.ends.borrow_mut();
        if let (true, Some(text)) = (event.click_count > 1, text) {
            let range = match event.click_count {
                2 => word_at(&text, spot.at),
                _ => 0..text.len(),
            };
            let end = Spot {
                key: spot.key.clone(),
                at: range.end,
            };
            *ends = Some((
                Spot {
                    at: range.start,
                    ..spot
                },
                end,
            ));
            self.dragging.set(false);
            return;
        }
        *ends = match ends.take() {
            Some((anchor, _)) if event.modifiers.shift => Some((anchor, spot)),
            _ => Some((spot.clone(), spot)),
        };
        self.dragging.set(true);
    }

    /// While a selection is dragged, called as each frame is painted: follows the pointer
    /// anywhere in the window, and scrolls `scroll` a step while the pointer is above or below
    /// `view`, until the button is let go.
    pub fn follow(&self, view: Bounds<Pixels>, scroll: &ScrollHandle, window: &mut Window) {
        if !self.dragging.get() {
            return;
        }
        let pointer = window.mouse_position();
        let step = edge_step(pointer.y, view.top(), view.bottom());
        if step != Pixels::ZERO {
            let (was, room) = (scroll.offset(), scroll.max_offset());
            let y = (was.y - step).clamp(-room.y, Pixels::ZERO);
            if y != was.y {
                scroll.set_offset(Point::new(was.x, y));
                window.request_animation_frame();
            }
        }
        let inside = Point::new(
            pointer.x,
            pointer.y.clamp(view.top(), view.bottom() - px(1.)),
        );
        let spot = spot_at(&self.shown.borrow(), inside);
        if let (Some(spot), Some((_, head))) = (spot, self.ends.borrow_mut().as_mut()) {
            if *head != spot {
                *head = spot;
                window.refresh();
            }
        }
        let moved = self.dragging.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
            if phase == DispatchPhase::Capture && moved.get() {
                moved.set(event.dragging());
                window.refresh();
            }
        });
        let released = self.dragging.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
            if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                released.set(false);
            }
        });
    }

    pub fn selected_text(&self, doc: &Markdown) -> Option<String> {
        self.selected_text_many(&[doc])
    }

    pub fn selected_text_many(&self, docs: &[&Markdown]) -> Option<String> {
        let span = self.span()?;
        let blocks: Vec<String> = docs
            .iter()
            .enumerate()
            .flat_map(|(ix, doc)| picked(doc, span, 0, ix))
            .collect();
        (!blocks.is_empty()).then(|| blocks.join("\n\n"))
    }
}
