//! Things laid side by side while they move between one another: the stage's tabs, the
//! tab rail's pages and the Git pane's lists. A swipe drags them, a landing springs them
//! onto one, and a switch made some other way travels one place over.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    canvas, div, px, relative, AnimationExt, AnyElement, App, Context, DispatchPhase, Div,
    ElementId, Hsla, IntoElement, ParentElement, ScrollEvent, Styled, Task, TouchPhase, Window,
};
use sikemux_wire::protocol::PaneKind;

use crate::panes;
use crate::swipe::{self, Ends, Swipe, Swiped};
use crate::workbench::{RailPage, Workbench};

thread_local! {
    /// The innermost row under the pointer as the gesture began, which alone follows it, so
    /// the Git pane's lists swipe without the stage's tabs swiping around them.
    static OWNER: Cell<usize> = const { Cell::new(0) };
    /// Where things that scroll sideways were last drawn; a swipe starting over one is theirs.
    static SCROLLERS: std::cell::RefCell<Vec<gpui_kit::Bounds<gpui_kit::Pixels>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The owner while a swipe began over a sideways scroller, which no row ever is.
const SIDEWAYS: usize = usize::MAX;

/// One row of things that slide: what a hand or a switch is doing to it now.
pub struct Slide<K> {
    swipe: Swipe,
    motion: Option<Motion<K>>,
    /// What was in front as the slide last knew it, to tell a switch from a redraw.
    front: Option<K>,
    /// The row's width as last drawn, in points.
    width: Rc<Cell<f32>>,
    moves: usize,
    settle: Option<Task<()>>,
}

impl<K> Default for Slide<K> {
    fn default() -> Self {
        Self {
            swipe: Swipe::default(),
            motion: None,
            front: None,
            width: Rc::default(),
            moves: 0,
            settle: None,
        }
    }
}

/// Things travelling along the row: each at its place from where the row comes to rest,
/// the row going from `start` to rest at 0.
struct Motion<K> {
    id: usize,
    shown: Vec<(K, i32)>,
    start: f32,
    /// The landing's spring and its start speed, in places a second; none for a pan.
    spring: Option<f32>,
}

/// What a landing does: what it shows and where from, its speed, and what is in front after.
type Landing<K> = (Vec<(K, i32)>, f32, f32, Option<K>);

/// What a row draws: each thing and its place, where the row is, and the motion moving it
/// with its spring.
pub(crate) type Shown<K> = (Vec<(K, i32)>, f32, Option<(usize, Option<f32>)>);

/// What a row did with one scroll.
pub(crate) enum Scrolled<K> {
    Ignored,
    /// Taken without changing what shows.
    Glided,
    Moved,
    Landed(Landing<K>),
}

fn beside<K: Clone>(order: &[K], slot: usize, way: i32) -> Option<K> {
    slot.checked_add_signed(way as isize)
        .and_then(|at| order.get(at))
        .cloned()
}

impl<K: Clone + PartialEq> Slide<K> {
    pub(crate) fn width(&self) -> Rc<Cell<f32>> {
        self.width.clone()
    }

    /// What the row shows with `front` in front of `order`: each thing and its place,
    /// where the row is, and how it moves.
    pub(crate) fn shown(&self, front: K, order: &[K]) -> Shown<K> {
        let slot = order.iter().position(|item| *item == front).unwrap_or(0);
        let beside = |way: i32| beside(order, slot, way);
        if let Some(motion) = &self.motion {
            return (
                motion.shown.clone(),
                motion.start,
                Some((motion.id, motion.spring)),
            );
        }
        if self.swipe.held() && self.swipe.offset != 0. {
            let way = self.swipe.offset.signum() as i32;
            let mut shown = vec![(front, 0)];
            shown.extend(beside(way).map(|next| (next, way)));
            return (shown, self.swipe.offset, None);
        }
        (vec![(front, 0)], 0., None)
    }

    /// Follows a scroll over the row, where `front` is one of `order`.
    pub(crate) fn scroll(&mut self, event: &ScrollEvent, order: &[K], front: &K) -> Scrolled<K> {
        let slot = order.iter().position(|item| item == front).unwrap_or(0);
        let ends = Ends {
            previous: slot > 0,
            next: slot + 1 < order.len(),
        };
        let beside = |way: i32| beside(order, slot, way);
        match self.swipe.scroll(event, self.width.get(), ends) {
            Swiped::Ignored => Scrolled::Ignored,
            Swiped::Gliding => Scrolled::Glided,
            Swiped::Moved => {
                self.motion = None;
                Scrolled::Moved
            }
            Swiped::Landed { step, from, speed } => {
                if step == 0 {
                    let way = from.signum() as i32;
                    let mut shown = vec![(front.clone(), 0)];
                    shown.extend(beside(way).map(|next| (next, way)));
                    return Scrolled::Landed((shown, from, speed, None));
                }
                let Some(target) = beside(step) else {
                    return Scrolled::Glided;
                };
                self.front = Some(target.clone());
                let shown = vec![(front.clone(), -step), (target.clone(), 0)];
                Scrolled::Landed((shown, from, speed, Some(target)))
            }
        }
    }

    /// Springs from `was` over to `now`, for a switch made with a click or a key.
    pub(crate) fn turn(&mut self, was: K, now: K, order: &[K]) -> Option<(usize, Duration)> {
        self.front = Some(now.clone());
        let at = |item: &K| order.iter().position(|kept| kept == item);
        let (from, to) = (at(&was)?, at(&now)?);
        if from == to {
            return None;
        }
        let way = if to > from { 1 } else { -1 };
        Some(self.travel(vec![(was, -way), (now, 0)], -way as f32, Some(0.)))
    }

    /// Starts things travelling; answers the motion and how long until it has arrived.
    pub(crate) fn travel(
        &mut self,
        shown: Vec<(K, i32)>,
        start: f32,
        spring: Option<f32>,
    ) -> (usize, Duration) {
        self.moves += 1;
        self.motion = Some(Motion {
            id: self.moves,
            shown,
            start,
            spring,
        });
        let lasts = if spring.is_some() {
            swipe::SPRING_LONGEST
        } else {
            swipe::PAN
        };
        (self.moves, lasts)
    }

    /// Notes what is in front now, answering what was before when it changed.
    fn moved_to(&mut self, now: Option<K>) -> Option<(K, K)> {
        let was = std::mem::replace(&mut self.front, now.clone());
        match (was, now) {
            (Some(was), Some(now)) if was != now && !self.swipe.held() => Some((was, now)),
            _ => None,
        }
    }
}

impl Workbench {
    /// Follows a scroll over the stage; true when the stage took it as a swipe between tabs.
    fn swipe_stage(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) -> bool {
        let Some(strip) = self.strip() else {
            return false;
        };
        let order: Vec<String> = self.tabs(&strip).iter().map(|tab| tab.id.clone()).collect();
        let Some(front) = self.tab() else {
            return false;
        };
        if event.touch_phase == TouchPhase::Started && !pages_sideways(front) {
            self.track.swipe = Swipe::default();
            return false;
        }
        let front = front.id.clone();
        match self.track.scroll(event, &order, &front) {
            Scrolled::Ignored => return false,
            Scrolled::Glided => return true,
            Scrolled::Moved => {}
            Scrolled::Landed((shown, from, speed, target)) => {
                if let Some(target) = target {
                    self.activate_tab(&strip, &target);
                }
                let moved = self.track.travel(shown, from, Some(speed));
                settle(self, moved, |bench: &mut Self| &mut bench.track, cx);
            }
        }
        self.parts.redraw_stage(cx);
        true
    }

    /// Follows a scroll over the tab rail; true when it took it as a swipe between pages.
    fn swipe_rail(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) -> bool {
        let front = self.rails.page;
        match self.rails.slide.scroll(event, &RailPage::ALL, &front) {
            Scrolled::Ignored => return false,
            Scrolled::Glided => return true,
            Scrolled::Moved => {}
            Scrolled::Landed((shown, from, speed, target)) => {
                if let Some(target) = target {
                    self.rails.page = target;
                }
                let moved = self.rails.slide.travel(shown, from, Some(speed));
                settle(self, moved, |bench: &mut Self| &mut bench.rails.slide, cx);
            }
        }
        self.parts.redraw(&self.parts.tabs, cx);
        true
    }

    /// Shows another of the tab rail's pages, springing over to it.
    pub fn show_rail_page(&mut self, page: RailPage, cx: &mut Context<Self>) {
        let was = std::mem::replace(&mut self.rails.page, page);
        if was != page {
            self.keep_rails(cx);
        }
        if let Some(moved) = self.rails.slide.turn(was, page, &RailPage::ALL) {
            settle(self, moved, |bench: &mut Self| &mut bench.rails.slide, cx);
        }
        cx.notify();
    }

    /// Pans one tab over when the tab in front changed some other way than a swipe.
    pub(crate) fn front_moved(&mut self, cx: &mut Context<Self>) {
        let Some(strip) = self.strip() else {
            self.track.front = None;
            return;
        };
        let order: Vec<String> = self.tabs(&strip).iter().map(|tab| tab.id.clone()).collect();
        let now = self.tab().map(|tab| tab.id.clone());
        let Some((was, now)) = self.track.moved_to(now) else {
            return;
        };
        let at = |id: &str| order.iter().position(|tab| tab == id);
        let (Some(from), Some(to)) = (at(&was), at(&now)) else {
            return;
        };
        let way = if to > from { 1 } else { -1 };
        let moved = self
            .track
            .travel(vec![(was, -way), (now, 0)], -way as f32, None);
        settle(self, moved, |bench: &mut Self| &mut bench.track, cx);
        self.parts.redraw_stage(cx);
    }
}

/// Stops drawing a motion's extra things once it has arrived.
pub(crate) fn settle<T: 'static, K: 'static>(
    owner: &mut T,
    (id, lasts): (usize, Duration),
    slide: fn(&mut T) -> &mut Slide<K>,
    cx: &mut Context<T>,
) {
    let task = cx.spawn(async move |this, cx| {
        cx.background_executor().timer(lasts).await;
        let _ = this.update(cx, |owner, cx| {
            let row = slide(owner);
            if row.motion.as_ref().is_some_and(|motion| motion.id == id) {
                row.motion = None;
                cx.notify();
            }
        });
    });
    slide(owner).settle = Some(task);
}

/// Whether a swipe over this tab pages between tabs; an editor or a page scrolls sideways
/// itself, so its gestures stay its own.
fn pages_sideways(tab: &sikemux_wire::protocol::Tab) -> bool {
    !matches!(
        crate::tabs::first_view(&tab.pane),
        Some((
            PaneKind::Files | PaneKind::Desk | PaneKind::Browser | PaneKind::Plugin,
            _
        ))
    )
}

/// The stage's tabs: the one in front, with the one beside it while they move.
pub fn stage(
    bench: &Workbench,
    front: &sikemux_wire::protocol::Tab,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let strip = bench.strip();
    let tabs = strip.as_ref().map_or(&[][..], |strip| bench.tabs(strip));
    let order: Vec<String> = tabs.iter().map(|tab| tab.id.clone()).collect();
    let (shown, start, motion) = bench.track.shown(front.id.clone(), &order);
    let cards = shown
        .into_iter()
        .filter_map(|(id, place)| {
            let tab = tabs.iter().find(|tab| tab.id == id)?;
            Some((panes::tab(bench, tab, window, cx), place))
        })
        .collect();
    let this = cx.entity().downgrade();
    row(
        "stage-track",
        &bench.track.width,
        cards,
        start,
        motion,
        Between::Gap(panes::GAP),
        move |event, cx| {
            this.update(cx, |bench, cx| bench.swipe_stage(event, cx))
                .unwrap_or(false)
        },
    )
    .flex_1()
    .min_w_0()
    .h_full()
    .into_any_element()
}

/// The tab rail's pages: the one shown, with the one beside it while they move.
pub fn rail(
    bench: &Workbench,
    page: impl Fn(RailPage, &mut Context<Workbench>) -> AnyElement,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let (shown, start, motion) = bench.rails.slide.shown(bench.rails.page, &RailPage::ALL);
    let cards = shown
        .into_iter()
        .map(|(shown, place)| (page(shown, cx), place))
        .collect();
    let this = cx.entity().downgrade();
    row(
        "rail-track",
        &bench.rails.slide.width,
        cards,
        start,
        motion,
        Between::Seam(crate::theme::Tokens::get(cx).border),
        move |event, cx| {
            this.update(cx, |bench, cx| bench.swipe_rail(event, cx))
                .unwrap_or(false)
        },
    )
    .flex_1()
    .min_h_0()
    .w_full()
    .into_any_element()
}

/// What shows between things while they move side by side.
#[derive(Clone, Copy)]
pub(crate) enum Between {
    /// A hairline of this colour.
    Seam(Hsla),
    /// Empty room this many points wide, showing what lies behind the row.
    Gap(f32),
}

/// A row's frame: what it shows, and the hearing of its swipes; the caller sizes it.
pub(crate) fn row(
    id: &'static str,
    width: &Rc<Cell<f32>>,
    cards: Vec<(AnyElement, i32)>,
    start: f32,
    motion: Option<(usize, Option<f32>)>,
    between: Between,
    swiped: impl Fn(&ScrollEvent, &mut App) -> bool + 'static,
) -> Div {
    let gap = match between {
        Between::Gap(gap) => gap,
        Between::Seam(_) => 0.,
    };
    div()
        .relative()
        .overflow_hidden()
        .child(listener(width, gap, swiped))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(px(-gap))
                .child(lane(id, cards, start, motion, between)),
        )
}

/// A row of things, each a row's width and its gap, sitting `start` places along and
/// travelling to rest at 0 while it moves.
fn lane(
    id: &'static str,
    mut cards: Vec<(AnyElement, i32)>,
    start: f32,
    motion: Option<(usize, Option<f32>)>,
    between: Between,
) -> AnyElement {
    cards.sort_by_key(|(_, place)| *place);
    let first = cards.first().map_or(0, |(_, place)| *place);
    let last = cards.last().map_or(0, |(_, place)| *place);
    let count = (last - first + 1) as f32;
    let lane = div()
        .absolute()
        .top_0()
        .bottom_0()
        .flex()
        .w(relative(count))
        .left(relative(first as f32 - start))
        .children(cards.into_iter().enumerate().map(|(at, (card, _))| {
            let (seam, gap) = match between {
                Between::Seam(color) => (Some(color), 0.),
                Between::Gap(gap) => (None, gap),
            };
            let line = seam.filter(|_| at > 0).map(|color| {
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(1.))
                    .bg(color)
            });
            div()
                .relative()
                .h_full()
                .w(relative(1. / count))
                .pr(px(gap))
                .flex()
                .child(card)
                .children(line)
        }));
    let Some((moved, spring)) = motion else {
        return lane.into_any_element();
    };
    let animation = match spring {
        Some(speed) => swipe::spring(start, speed),
        None => swipe::pan(),
    };
    lane.with_animation(
        ElementId::NamedInteger(id.into(), moved as u64),
        animation,
        move |lane, travelled| lane.left(relative(first as f32 - start * (1. - travelled))),
    )
    .into_any_element()
}

/// Measures one place along its row, a thing and the gap after it, and hears scrolls over
/// it before what is under the pointer does, so a swipe does not also scroll what it starts on.
fn listener(
    width: &Rc<Cell<f32>>,
    gap: f32,
    swiped: impl Fn(&ScrollEvent, &mut App) -> bool + 'static,
) -> impl IntoElement {
    let (width, swiped) = (width.clone(), Rc::new(swiped));
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            width.set(f32::from(bounds.size.width) + gap);
            SCROLLERS.with(|scrollers| {
                scrollers
                    .borrow_mut()
                    .retain(|scroller| !bounds.contains(&scroller.center()))
            });
            let swiped = swiped.clone();
            let me = Rc::as_ptr(&width) as usize;
            window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
                if phase != DispatchPhase::Capture || !bounds.contains(&event.position) {
                    return;
                }
                if event.touch_phase == TouchPhase::Started {
                    gpui_transcript::scroll_latch::gesture_starts();
                    let over = SCROLLERS.with(|scrollers| {
                        scrollers
                            .borrow()
                            .iter()
                            .any(|scroller| scroller.contains(&event.position))
                    });
                    OWNER.set(if over { SIDEWAYS } else { me });
                }
                let inside = gpui_transcript::scroll_latch::taken_inside();
                if OWNER.get() == me && !inside && swiped(event, cx) {
                    cx.stop_propagation();
                }
            });
        },
    )
    .absolute()
    .size_full()
}

/// Laid over something that scrolls sideways, such as a strip of tabs, so a swipe that starts
/// over it scrolls it rather than swiping the rows around it.
pub(crate) fn sideways_scroller() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, _, _| SCROLLERS.with(|scrollers| scrollers.borrow_mut().push(bounds)),
    )
    .absolute()
    .inset_0()
}
