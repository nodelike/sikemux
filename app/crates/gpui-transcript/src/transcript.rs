use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    actions, div, list, px, AnyElement, AnyView, App, ClipboardItem, Context, ElementId,
    EventEmitter, FocusHandle, FollowMode, InteractiveElement, IntoElement, KeyBinding,
    ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, SharedString, Styled, Window,
};

use crate::draw::{LayoutBudget, Pen, Style};
use crate::find::{ranges_in, FoundChanged, Hit, Query};
use crate::markdown::Markdown;
use crate::select::{spot_at, word_at, Key, Place, RowHit, Selecting, Shown, Span, Spot};

actions!(transcript, [Copy]);

const CONTEXT: &str = "Transcript";

/// Binds the keys a transcript takes; call it once when the app starts.
pub fn init(cx: &mut App) {
    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    cx.bind_keys([KeyBinding::new(copy, Copy, Some(CONTEXT))]);
}

/// A row of a transcript, known by an id that stays the same for as long as the row lives.
pub trait Row: 'static {
    fn id(&self) -> &str;

    /// The markdown the row draws, in the order it draws it, for copying a selection.
    fn docs(&self) -> Vec<&Markdown> {
        Vec::new()
    }

    /// Whether find looks in the markdown `docs` gives at `doc`.
    fn findable(&self, _doc: usize) -> bool {
        true
    }

    /// The host's name for the doc `docs` gives at `doc`, and the doc a name is.
    fn doc_name(&self, _doc: usize) -> Option<&str> {
        None
    }

    fn doc_named(&self, _name: &str) -> Option<usize> {
        None
    }
}

/// One change to a transcript's rows, as a host maps its own feed onto them.
pub enum Change<R> {
    /// The row replaces the one with its id, or goes at the end.
    Put(R),
    Update(SharedString, Box<dyn FnOnce(&mut R)>),
    /// The row and every row after it go.
    DropFrom(SharedString),
    /// Only these rows stay, from the first of them on.
    Keep(Vec<SharedString>),
    /// An earlier page, oldest first, goes above the rows held; any already held are skipped.
    Prepend(Vec<R>),
    /// A later page, oldest first, goes below the rows held; any already held are skipped.
    Append(Vec<R>),
    /// Every row before this one goes.
    DropBefore(SharedString),
    Reset(Vec<R>),
    /// Rows from the middle of the history in place of all held, the reader kept where they
    /// scroll as rows arrive below rather than following the newest.
    Place(Vec<R>),
}

type DrawRow<R> = Rc<dyn Fn(&[R], usize, &Pen, &mut Window, &mut App) -> AnyElement>;
type Ask = Rc<dyn Fn(&mut Window, &mut App)>;

/// Rows within this many of the first one count as the top, where an earlier page is asked for,
/// and as many of the last one as the bottom, where a later one is.
const NEAR_TOP: usize = 8;

/// A hidden transcript draws nothing, so it holds changes until it is shown,
/// but no more than this many before it applies them anyway.
const MOST_QUEUED: usize = 1024;

/// A chat's rows drawn from the bottom up, only those in view. Changes pushed
/// in one frame are applied together when it draws; rows added above never
/// move what the reader sees, and the end stays in view while they are at it.
/// The reader selects text across rows by dragging or shift-clicking, and ⌘C copies it.
pub struct Transcript<R: Row> {
    rows: Vec<R>,
    list: ListState,
    queue: Vec<Change<R>>,
    style: Rc<Style>,
    draw_row: DrawRow<R>,
    /// Drawn after the last row, scrolling with the rows.
    footer: Option<AnyView>,
    layout_budget: Rc<LayoutBudget>,
    on_reach_top: Option<Ask>,
    asked_earlier: bool,
    on_reach_bottom: Option<Ask>,
    asked_later: bool,
    /// The rows in view as the list last laid them out.
    visible: Range<usize>,
    focus: FocusHandle,
    ends: Option<(Spot, Spot)>,
    dragging: bool,
    /// The reader scrolled up from the latest rows.
    away: bool,
    shown: Rc<RefCell<Vec<Shown>>>,
    /// What the reader looks for, every place it is found in reading order, and the one
    /// they are on. The places are found again in the rows that change.
    query: Option<Query>,
    hits: Vec<Hit>,
    current: usize,
    stale: Stale,
    /// The place found the reader stepped to, while it is brought into view.
    chasing: Option<Chase>,
    /// A place found to go to once the rows hold it: the row, its doc by name, and which of the
    /// places found in that doc.
    pending: Option<(SharedString, SharedString, usize)>,
    /// Where each row was last placed in the window, as its texts were.
    row_tops: Rc<RefCell<HashMap<SharedString, Pixels>>>,
    /// How a row that just arrived rises into place, when rows do.
    rise: Option<Rc<dyn Fn(f32) -> f32>>,
    /// The rows that just arrived, and when.
    arrived: Vec<(SharedString, Instant)>,
}

/// How long a row that just arrived takes to rise into place, and from how far below.
const RISE: Duration = Duration::from_millis(200);
const RISE_FROM: Pixels = px(10.);
/// More rows than this arriving at once are a transcript filling in, which just shows.
const MOST_RISING: usize = 2;

/// Bringing a place found into view, over the frames it takes its row to be laid out and
/// its block drawn.
struct Chase {
    hit: usize,
    steps: u8,
}

/// The most frames spent bringing a place found into view.
const MOST_CHASE_STEPS: u8 = 8;
/// Room kept above a place brought into view, under the find bar drawn over the rows, and below.
const ROOM_ABOVE: Pixels = px(96.);
const ROOM_BELOW: Pixels = px(40.);

/// The rows whose places found are out of date.
enum Stale {
    None,
    Rows(BTreeSet<usize>),
    All,
}

impl Stale {
    fn row(&mut self, ix: usize) {
        match self {
            Stale::None => *self = Stale::Rows(BTreeSet::from([ix])),
            Stale::Rows(rows) => {
                rows.insert(ix);
            }
            Stale::All => {}
        }
    }
}

impl<R: Row> Transcript<R> {
    /// `draw_row` draws the row at an index with the pen it is handed; it gets
    /// every row, so a row can look at its neighbours.
    pub fn new(
        rows: Vec<R>,
        style: Rc<Style>,
        draw_row: impl Fn(&[R], usize, &Pen, &mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = ListState::new(rows.len() + 1, ListAlignment::Bottom, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        let this = cx.weak_entity();
        list.set_scroll_handler(move |event, window, cx| {
            let (visible, away) = (event.visible_range.clone(), !event.is_following_tail);
            this.update(cx, |this, cx| this.scrolled(visible, away, window, cx))
                .ok();
        });
        Transcript {
            rows,
            list,
            queue: Vec::new(),
            style,
            draw_row: Rc::new(draw_row),
            footer: None,
            on_reach_top: None,
            asked_earlier: false,
            on_reach_bottom: None,
            asked_later: false,
            visible: 0..0,
            focus: cx.focus_handle(),
            ends: None,
            dragging: false,
            away: false,
            shown: Rc::default(),
            query: None,
            hits: Vec::new(),
            current: 0,
            stale: Stale::None,
            chasing: None,
            layout_budget: Rc::default(),
            pending: None,
            row_tops: Rc::default(),
            rise: None,
            arrived: Vec::new(),
        }
    }

    /// Looks for `query` in every row, or stops looking with none.
    pub fn find(&mut self, query: Option<Query>, cx: &mut Context<Self>) {
        if query == self.query {
            return;
        }
        self.hits.clear();
        (self.query, self.current, self.stale) = (query, 0, Stale::All);
        cx.notify();
    }

    /// Moves to the next place found, or the one before, round the ends, and brings it into view.
    pub fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let total = self.hits.len();
        if total == 0 {
            return;
        }
        self.current = if forward {
            (self.current + 1) % total
        } else {
            (self.current + total - 1) % total
        };
        self.reveal();
        cx.emit(FoundChanged);
        cx.notify();
    }

    /// The reader scrolled up from the latest rows, so they no longer follow as rows arrive.
    pub fn away(&self) -> bool {
        self.away && !self.rows.is_empty()
    }

    /// Brings the latest rows into view, and follows them again as rows arrive.
    pub fn to_latest(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.away = false;
        cx.emit(AwayChanged(false));
        cx.notify();
    }

    /// Which place found the reader is on, counting from 0, and how many there are.
    pub fn found(&self) -> (usize, usize) {
        (self.current, self.hits.len())
    }

    /// Where the place the reader is on is: its row, its doc by name, and which of the places
    /// found in that doc.
    pub fn found_at(&self) -> Option<(&str, &str, usize)> {
        let hit = self.hits.get(self.current)?;
        let (row, doc) = (hit.at.0, hit.at.1);
        let nth = self.hits[..self.current]
            .iter()
            .filter(|before| before.at.0 == row && before.at.1 == doc)
            .count();
        let row = self.rows.get(row)?;
        Some((row.id(), row.doc_name(doc)?, nth))
    }

    /// Whether the rows hold `row` now.
    pub fn holds(&self, row: &str) -> bool {
        self.position(row).is_some()
    }

    /// Goes to the `nth` place found in doc `doc` of row `row`, or its last, once the rows hold
    /// that row, and brings it into view.
    pub fn go_to(
        &mut self,
        row: SharedString,
        doc: SharedString,
        nth: usize,
        cx: &mut Context<Self>,
    ) {
        self.pending = Some((row, doc, nth));
        cx.notify();
    }

    fn resolve_pending(&mut self, cx: &mut Context<Self>) {
        let Some((row, doc, nth)) = self.pending.as_ref() else {
            return;
        };
        let Some(ix) = self.position(row) else {
            return;
        };
        let (doc, nth) = (self.rows[ix].doc_named(doc), *nth);
        self.pending = None;
        let Some(doc) = doc else {
            return;
        };
        let in_doc: Vec<usize> = (0..self.hits.len())
            .filter(|&hit| self.hits[hit].at.0 == ix && self.hits[hit].at.1 == doc)
            .collect();
        if let Some(&hit) = in_doc.get(nth).or(in_doc.last()) {
            self.current = hit;
            self.reveal();
            cx.emit(FoundChanged);
        }
    }

    /// Brings the place the reader is on into view as the transcript next draws.
    fn reveal(&mut self) {
        if self.hits.get(self.current).is_some() {
            self.chasing = Some(Chase {
                hit: self.current,
                steps: 0,
            });
        }
    }

    /// Puts the line of the place being brought into view under the find bar, leaving it where
    /// it is when the last frame drew it in view. It is aimed from where layout put it in its
    /// row: how far down the row its message starts, the heights of the blocks before its own,
    /// and its line in that block once a frame has drawn the block. Scrolling to that place in
    /// the row keeps rows measured again elsewhere from moving the aim, and it aims again from
    /// each frame's layout until a frame shows the line in view, as a message can be laid out
    /// anew at another width once it is drawn.
    fn chase(&mut self, window: &mut Window) {
        let Some(mut chase) = self.chasing.take() else {
            return;
        };
        if chase.steps >= MOST_CHASE_STEPS {
            return;
        }
        chase.steps += 1;
        let Some(hit) = self.hits.get(chase.hit) else {
            return;
        };
        let (row, doc, block, n) = hit.at;
        let Some(found) = self.rows.get(row) else {
            return;
        };
        let id = found.id().to_owned();
        let docs = found.docs();
        let Some(markdown) = docs.get(doc) else {
            return;
        };
        let before = markdown.blocks().get(..block).and_then(|blocks| {
            blocks.iter().try_fold(px(0.), |sum, node| {
                Some(sum + node.size.get()?.height + self.style.gap)
            })
        });
        let in_row = markdown
            .top
            .get()
            .zip(self.row_tops.borrow().get(id.as_str()).copied())
            .map(|(top, row)| (top, top - row));
        let (Some((top, in_row)), Some(before)) = (in_row, before) else {
            self.list.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
            self.chasing = Some(chase);
            return window.request_animation_frame();
        };
        let line = self
            .shown
            .borrow()
            .iter()
            .find(|text| {
                text.key.0.as_ref() == id
                    && (text.key.1, text.key.2, text.key.3) == (doc, block, n)
                    && (text.offset..=text.offset + text.len).contains(&hit.range.start)
            })
            .and_then(|text| {
                let at = text
                    .layout
                    .position_for_index(hit.range.start - text.offset)?;
                Some((at.y, text.layout.line_height()))
            });
        if let Some((top, height)) = line {
            let view = self.list.viewport_bounds();
            if top >= view.top() + ROOM_ABOVE && top + height <= view.bottom() - ROOM_BELOW {
                return;
            }
        }
        let in_block = line.map_or(px(0.), |(line, _)| line - (top + before));
        let offset = in_row + before + in_block - ROOM_ABOVE;
        self.list.scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: offset.max(px(0.)),
        });
        if offset < px(0.) {
            self.list.scroll_by(offset);
        }
        self.chasing = Some(chase);
        window.request_animation_frame();
    }

    /// Finds the query again in the rows that changed.
    fn find_again(&mut self, cx: &mut Context<Self>) {
        let stale = std::mem::replace(&mut self.stale, Stale::None);
        let Some(query) = &self.query else {
            return;
        };
        let was = self.hits.len();
        let whole = matches!(stale, Stale::All);
        match stale {
            Stale::None => return,
            Stale::All => {
                self.hits = (0..self.rows.len())
                    .flat_map(|ix| hits_in(&self.rows[ix], ix, query))
                    .collect();
            }
            Stale::Rows(rows) => {
                for ix in rows.into_iter().filter(|ix| *ix < self.rows.len()) {
                    let from = self.hits.partition_point(|hit| hit.at.0 < ix);
                    let to = self.hits.partition_point(|hit| hit.at.0 <= ix);
                    let found = hits_in(&self.rows[ix], ix, query);
                    self.hits.splice(from..to, found);
                }
            }
        }
        self.current = self.current.min(self.hits.len().saturating_sub(1));
        if was == 0 && !self.hits.is_empty() {
            self.reveal();
        }
        if whole || was != self.hits.len() {
            cx.emit(FoundChanged);
        }
    }

    /// Has a row that just arrived, one or two at a time, open its place and fade and rise into
    /// it on `easing`, so the rows above it glide up rather than jump.
    pub fn rise_in(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.rise = Some(Rc::new(easing));
        self
    }

    /// Called once each time the reader scrolls near the first row held, to fetch an earlier page.
    pub fn on_reach_top(mut self, ask: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reach_top = Some(Rc::new(ask));
        self
    }

    /// Called once each time the reader scrolls near the last row held, to fetch a later page
    /// when the host dropped the rows after it.
    pub fn on_reach_bottom(mut self, ask: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reach_bottom = Some(Rc::new(ask));
        self
    }

    /// The rows in view as the list last laid them out.
    pub fn visible(&self) -> Range<usize> {
        self.visible.clone()
    }

    /// The rows as of the last frame drawn; changes pushed since are not in them yet.
    /// Draws every row in `style` from now on, measured again.
    pub fn restyle(&mut self, style: Rc<Style>, cx: &mut Context<Self>) {
        self.style = style;
        self.list.remeasure();
        cx.notify();
    }

    pub fn rows(&self) -> &[R] {
        &self.rows
    }

    /// Draws `footer` after the last row, scrolling with the rows.
    pub fn set_footer(&mut self, footer: AnyView, cx: &mut Context<Self>) {
        self.footer = Some(footer);
        cx.notify();
    }

    pub fn list(&self) -> &ListState {
        &self.list
    }

    pub fn push(&mut self, change: Change<R>, cx: &mut Context<Self>) {
        self.extend([change], cx);
    }

    pub fn extend(&mut self, changes: impl IntoIterator<Item = Change<R>>, cx: &mut Context<Self>) {
        self.queue.extend(changes);
        if self.queue.len() >= MOST_QUEUED {
            self.apply(cx);
        }
        cx.notify();
    }

    fn reset(&mut self, rows: Vec<R>, follow: FollowMode) {
        self.stale = Stale::All;
        self.rows = rows;
        self.list.reset(self.rows.len() + 1);
        self.list.set_follow_mode(follow);
        self.asked_earlier = false;
        self.asked_later = false;
    }

    /// Where the reader is, as a row and how far into it. A list scrolled to its end says only
    /// that it is at the end, which rows added below would move, scrolling the reader past them.
    fn place_now(&self) -> ListOffset {
        let place = self.list.logical_scroll_top();
        if place.item_ix < self.rows.len() {
            return place;
        }
        let top = self.list.viewport_bounds().top();
        let row = self.visible.start;
        let offset = (self.list.bounds_for_item(row)).map_or(px(0.), |row| top - row.top());
        ListOffset {
            item_ix: row,
            offset_in_item: offset.max(px(0.)),
        }
    }

    /// Most changes reach the newest rows, so the search starts from the end.
    fn position(&self, id: &str) -> Option<usize> {
        self.rows.iter().rposition(|row| row.id() == id)
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let was_empty = self.rows.is_empty();
        let changes = std::mem::take(&mut self.queue);
        let changed = !changes.is_empty();
        let mut fresh = Vec::new();
        for change in changes {
            let end = self.rows.len();
            match change {
                Change::Put(row) => match self.position(row.id()) {
                    Some(ix) => {
                        self.rows[ix] = row;
                        self.list.remeasure_items(ix..ix + 1);
                        self.stale.row(ix);
                    }
                    None => {
                        fresh.push(SharedString::from(row.id().to_owned()));
                        self.rows.push(row);
                        self.list.splice(end..end, 1);
                        self.stale.row(end);
                    }
                },
                Change::Update(id, change) => {
                    if let Some(ix) = self.position(&id) {
                        change(&mut self.rows[ix]);
                        self.list.remeasure_items(ix..ix + 1);
                        self.stale.row(ix);
                    }
                }
                Change::DropFrom(id) => {
                    if let Some(ix) = self.position(&id) {
                        self.rows.truncate(ix);
                        self.list.splice(ix..end, 0);
                        self.stale = Stale::All;
                    }
                }
                Change::Keep(ids) => {
                    self.stale = Stale::All;
                    let kept: HashSet<&str> = ids.iter().map(|id| id.as_ref()).collect();
                    let first = self.rows.iter().position(|row| kept.contains(row.id()));
                    for ix in (first.unwrap_or(0)..end).rev() {
                        if !kept.contains(self.rows[ix].id()) {
                            self.rows.remove(ix);
                            self.list.splice(ix..ix + 1, 0);
                        }
                    }
                }
                Change::Prepend(rows) => {
                    let held: HashSet<&str> = self.rows.iter().map(Row::id).collect();
                    let older: Vec<R> = rows
                        .into_iter()
                        .filter(|row| !held.contains(row.id()))
                        .collect();
                    self.list.splice(0..0, older.len());
                    self.rows.splice(0..0, older);
                    self.stale = Stale::All;
                    self.asked_earlier = false;
                }
                Change::Append(rows) => {
                    let held: HashSet<&str> = self.rows.iter().map(Row::id).collect();
                    let later: Vec<R> = rows
                        .into_iter()
                        .filter(|row| !held.contains(row.id()))
                        .collect();
                    let place = self.place_now();
                    self.list.splice(end..end, later.len());
                    self.list.scroll_to(place);
                    self.rows.extend(later);
                    self.stale = Stale::All;
                    self.asked_later = false;
                }
                Change::DropBefore(id) => {
                    if let Some(ix) = self.rows.iter().position(|row| row.id() == id.as_ref()) {
                        self.rows.drain(..ix);
                        self.list.splice(0..ix, 0);
                        self.stale = Stale::All;
                    }
                }
                Change::Reset(rows) => self.reset(rows, FollowMode::Tail),
                Change::Place(rows) => self.reset(rows, FollowMode::Normal),
            }
        }
        self.arrived
            .retain(|(_, at)| at.elapsed() < RISE && self.rise.is_some());
        if self.rise.is_some() && fresh.len() <= MOST_RISING {
            let now = Instant::now();
            self.arrived.extend(fresh.into_iter().map(|id| (id, now)));
        }
        if self.rows.is_empty() != was_empty {
            cx.emit(EmptyChanged(self.rows.is_empty()));
        }
        if self.row_tops.borrow().len() > self.rows.len() {
            let held: HashSet<&str> = self.rows.iter().map(Row::id).collect();
            self.row_tops
                .borrow_mut()
                .retain(|id, _| held.contains(id.as_ref()));
        }
        if changed {
            cx.emit(RowsChanged);
        }
    }

    fn scrolled(
        &mut self,
        visible: Range<usize>,
        away: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if away != self.away {
            self.away = away;
            cx.emit(AwayChanged(away));
        }
        let near_top = visible.start < NEAR_TOP;
        let near_bottom = visible.end + NEAR_TOP > self.rows.len();
        self.visible = visible;
        if !near_top {
            self.asked_earlier = false;
        }
        if !near_bottom {
            self.asked_later = false;
        }
        if self.rows.is_empty() {
            return;
        }
        if near_top && !self.asked_earlier {
            self.asked_earlier = true;
            if let Some(ask) = self.on_reach_top.clone() {
                ask(window, cx);
            }
        }
        if near_bottom && !self.asked_later {
            self.asked_later = true;
            if let Some(ask) = self.on_reach_bottom.clone() {
                ask(window, cx);
            }
        }
    }

    fn place(&self, spot: &Spot) -> Option<Place> {
        let (row, doc, block, text) = &spot.key;
        Some((self.position(row)?, *doc, *block, *text, spot.at))
    }

    /// The selection's ends in reading order, while both rows are still held.
    fn span(&self) -> Option<Span> {
        let (anchor, head) = self.ends.as_ref()?;
        let (anchor, head) = (self.place(anchor)?, self.place(head)?);
        Some(Span(anchor.min(head), anchor.max(head)))
    }

    fn press(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some(spot) = spot_at(&self.shown.borrow(), event.position) else {
            return;
        };
        if event.click_count > 1 {
            if let Some(text) = self.text_of(&spot.key) {
                let range = match event.click_count {
                    2 => word_at(&text, spot.at),
                    _ => 0..text.len(),
                };
                let end = Spot {
                    key: spot.key.clone(),
                    at: range.end,
                };
                self.ends = Some((
                    Spot {
                        at: range.start,
                        ..spot
                    },
                    end,
                ));
                self.dragging = false;
                cx.notify();
                return;
            }
        }
        self.ends = match self.ends.take() {
            Some((anchor, _)) if event.modifiers.shift => Some((anchor, spot)),
            _ => Some((spot.clone(), spot)),
        };
        self.dragging = true;
        cx.notify();
    }

    fn text_of(&self, (row, doc, block, n): &Key) -> Option<SharedString> {
        let row = &self.rows[self.position(row)?];
        let texts = row.docs().get(*doc)?.texts(*block);
        texts.get((*n as usize).checked_sub(1)?).cloned()
    }

    /// Moves the selection's free end, scrolling when the pointer is past the top or bottom.
    fn drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.dragging || !event.dragging() {
            return;
        }
        let view = self.list.viewport_bounds();
        let (y, top, bottom) = (event.position.y, view.top(), view.bottom());
        if y < top || y > bottom {
            self.list
                .scroll_by(if y < top { y - top } else { y - bottom });
        }
        let inside = Point::new(event.position.x, y.clamp(top, bottom - px(1.)));
        let spot = spot_at(&self.shown.borrow(), inside);
        if let (Some(spot), Some((_, head))) = (spot, self.ends.as_mut()) {
            *head = spot;
        }
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        match self.selected_text() {
            Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            None => cx.propagate(),
        }
    }

    /// The text selected here while the keyboard is in the transcript.
    pub fn focused_selection(&self, window: &Window) -> Option<String> {
        self.focus
            .is_focused(window)
            .then(|| self.selected_text())
            .flatten()
    }

    /// The selected text, read from the rows themselves, so rows out of view copy too.
    fn selected_text(&self) -> Option<String> {
        let span = self.span()?;
        let mut blocks = Vec::new();
        for ix in span.0 .0..=span.1 .0 {
            for (doc, markdown) in self.rows[ix].docs().into_iter().enumerate() {
                for block in 0..markdown.blocks().len() {
                    let texts = markdown.texts(block);
                    let picked: Vec<&str> = (1..)
                        .zip(&texts)
                        .filter_map(|(n, text)| {
                            text.get(span.range((ix, doc, block, n), text.len())?)
                        })
                        .collect();
                    if !picked.is_empty() {
                        blocks.push(picked.join("\n"));
                    }
                }
            }
        }
        (!blocks.is_empty()).then(|| blocks.join("\n\n"))
    }
}

/// The transcript came to hold rows, or came to hold none, as `0` says.
pub struct EmptyChanged(pub bool);

impl<R: Row> EventEmitter<EmptyChanged> for Transcript<R> {}

/// Changes pushed to the rows were applied, as they are when the transcript draws.
pub struct RowsChanged;

impl<R: Row> EventEmitter<RowsChanged> for Transcript<R> {}

impl<R: Row> EventEmitter<FoundChanged> for Transcript<R> {}

/// The reader scrolled away from the latest rows, or came back to them.
pub struct AwayChanged(pub bool);

impl<R: Row> EventEmitter<AwayChanged> for Transcript<R> {}

impl<R: Row> Render for Transcript<R> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.layout_budget.reset();
        self.apply(cx);
        if self.query.is_some() {
            self.find_again(cx);
            self.resolve_pending(cx);
        } else {
            self.stale = Stale::None;
        }
        self.chase(window);
        self.shown.borrow_mut().clear();
        let span = self.span();
        let this = cx.entity();
        let rows = list(self.list.clone(), move |ix, window, cx| {
            this.update(cx, |this, cx| {
                if ix == this.rows.len() {
                    return match this.footer.clone() {
                        Some(footer) => footer.into_any_element(),
                        None => div().into_any_element(),
                    };
                }
                let selecting = this.rows.get(ix).map(|row| Selecting {
                    row: row.id().to_owned().into(),
                    ix,
                    docs: row
                        .docs()
                        .into_iter()
                        .map(|doc| doc as *const Markdown)
                        .collect(),
                    span,
                    shown: this.shown.clone(),
                    hits: this
                        .hits
                        .iter()
                        .enumerate()
                        .filter(|(_, hit)| hit.at.0 == ix)
                        .map(|(at, hit)| {
                            let (_, doc, block, n) = hit.at;
                            RowHit {
                                text: (doc, block, n),
                                range: hit.range.clone(),
                                current: at == this.current,
                            }
                        })
                        .collect(),
                    budget: this.layout_budget.clone(),
                });
                let pen = Pen::selecting(this.style.clone(), selecting);
                let draw_row = this.draw_row.clone();
                #[cfg(feature = "counts")]
                crate::counts::add(&crate::counts::ROWS, 1);
                let drawn = draw_row(&this.rows, ix, &pen, window, cx);
                let (Some(row), tops) = (this.rows.get(ix), this.row_tops.clone()) else {
                    return drawn;
                };
                let id: SharedString = row.id().to_owned().into();
                let rise = this.rise.clone().and_then(|easing| {
                    let (_, at) = this.arrived.iter().find(|(arrived, _)| *arrived == id)?;
                    let through = at.elapsed().as_secs_f32() / RISE.as_secs_f32();
                    (through < 1.).then(|| easing(through))
                });
                let rising =
                    ElementId::NamedChild(std::sync::Arc::new(id.clone().into()), "rise".into());
                let row = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(drawn)
                    .on_children_prepainted(move |bounds, _, _| {
                        if let Some(row) = bounds.first() {
                            tops.borrow_mut().insert(id.clone(), row.top());
                        }
                    });
                let Some(risen) = rise else {
                    return row.into_any_element();
                };
                window.request_animation_frame();
                let measured = window
                    .use_keyed_state(rising, cx, |_, _| Rc::new(Cell::new(Pixels::ZERO)))
                    .read(cx)
                    .clone();
                let row = row
                    .flex_none()
                    .opacity(risen)
                    .relative()
                    .top(RISE_FROM * (1. - risen));
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .h(measured.get() * risen)
                    .child(row)
                    .on_children_prepainted(move |bounds, _, _| {
                        if let Some(row) = bounds.first() {
                            measured.set(row.size.height);
                        }
                    })
                    .into_any_element()
            })
        });
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.press(event, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, event, _, cx| this.drag(event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .child(rows.size_full())
    }
}

/// Every place `query` is found in row `ix`, in reading order.
fn hits_in<R: Row>(row: &R, ix: usize, query: &Query) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (doc, markdown) in row.docs().into_iter().enumerate() {
        if !row.findable(doc) {
            continue;
        }
        for block in 0..markdown.blocks().len() {
            for (n, text) in (1..).zip(markdown.texts(block)) {
                hits.extend(ranges_in(&text, query).into_iter().map(|range| Hit {
                    at: (ix, doc, block, n),
                    range,
                }));
            }
        }
    }
    hits
}
