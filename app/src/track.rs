//! Things laid side by side while they move between one another: the stage's tabs, the
//! tab rail's pages and the Git pane's lists. A swipe drags them, a landing springs them
//! onto one, and a switch made some other way travels one place over.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    canvas, div, px, relative, AnimationExt, AnyElement, App, Context, DispatchPhase, Div,
    ElementId, Hsla, IntoElement, Modifiers, MouseDownEvent, ParentElement, Point, ScrollDelta,
    ScrollEvent, ScrollSource, Styled, Task, TouchPhase, Window,
};

use crate::panes;
use crate::swipe::{self, Ends, Swipe, Swiped};
use crate::workbench::Workbench;
use sikemux_view::prefs::TabRailPage;

type SwipedFn = Rc<dyn Fn(&ScrollEvent, &mut App) -> bool>;

struct ActiveSwipe {
    owner: usize,
    #[cfg(target_os = "macos")]
    window_view: Option<usize>,
    swiped: SwipedFn,
}

thread_local! {
    /// The innermost row under the pointer as the gesture began, which alone follows it, so
    /// the Git pane's lists swipe without the stage's tabs swiping around them.
    static OWNER: Cell<usize> = const { Cell::new(0) };
    /// Where things that scroll sideways were last drawn; a swipe starting over one is theirs.
    static SCROLLERS: RefCell<Vec<gpui_kit::Bounds<gpui_kit::Pixels>>> = const { RefCell::new(Vec::new()) };
    /// The row currently holding an in-flight horizontal swipe, so a missed end event
    /// settles it cleanly instead of leaving the lane stranded partway.
    static ACTIVE: RefCell<Option<ActiveSwipe>> = const { RefCell::new(None) };
}

/// The owner while a swipe began over a sideways scroller, which no row ever is.
const SIDEWAYS: usize = usize::MAX;

#[derive(Clone, Default)]
pub(crate) struct Handle {
    /// The row's width as last drawn, in points.
    width: Rc<Cell<f32>>,
    /// Whether this row's swipe is currently dragging the lane.
    held: Rc<Cell<bool>>,
}

/// One row of things that slide: what a hand or a switch is doing to it now.
pub struct Slide<K> {
    swipe: Swipe,
    motion: Option<Motion<K>>,
    /// What was in front as the slide last knew it, to tell a switch from a redraw.
    front: Option<K>,
    handle: Handle,
    moves: usize,
    settle: Option<Task<()>>,
}

impl<K> Default for Slide<K> {
    fn default() -> Self {
        Self {
            swipe: Swipe::default(),
            motion: None,
            front: None,
            handle: Handle::default(),
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

/// What a landing does: what it shows and where from, its speed, the step it took (-1, 0
/// or 1), and what is in front after when the row holds it.
type Landing<K> = (Vec<(K, i32)>, f32, f32, i32, Option<K>);

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

/// What lies on either side of the thing in front. An end can be open with nothing to draw
/// there, when the step leads somewhere the row does not hold.
pub(crate) struct Around<K> {
    pub previous: Option<K>,
    pub next: Option<K>,
    pub ends: Ends,
}

impl<K: Clone + PartialEq> Around<K> {
    /// Around `front` in `order`.
    pub(crate) fn in_order(order: &[K], front: &K) -> Self {
        let slot = order.iter().position(|item| item == front).unwrap_or(0);
        let at = |way: isize| {
            slot.checked_add_signed(way)
                .and_then(|at| order.get(at))
                .cloned()
        };
        let (previous, next) = (at(-1), at(1));
        let ends = Ends {
            previous: previous.is_some(),
            next: next.is_some(),
        };
        Self {
            previous,
            next,
            ends,
        }
    }

    fn beside(&self, way: i32) -> Option<K> {
        match way {
            -1 => self.previous.clone(),
            1 => self.next.clone(),
            _ => None,
        }
    }
}

impl<K: Clone + PartialEq> Slide<K> {
    pub(crate) fn handle(&self) -> Handle {
        self.handle.clone()
    }

    fn reset_swipe(&mut self) -> bool {
        let was_held = self.swipe.held();
        self.swipe = Swipe::default();
        self.handle.held.set(false);
        clear_active(Rc::as_ptr(&self.handle.width) as usize);
        was_held
    }

    /// What the row shows with `front` in front and `around` beside it: each thing and its
    /// place, where the row is, and how it moves.
    pub(crate) fn shown(&self, front: K, around: impl FnOnce() -> Around<K>) -> Shown<K> {
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
            shown.extend(around().beside(way).map(|next| (next, way)));
            return (shown, self.swipe.offset, None);
        }
        (vec![(front, 0)], 0., None)
    }

    /// Follows a scroll over the row, with `front` in front and `around` beside it.
    pub(crate) fn scroll(
        &mut self,
        event: &ScrollEvent,
        front: &K,
        around: &Around<K>,
    ) -> Scrolled<K> {
        let beside = |way: i32| around.beside(way);
        let swiped = self
            .swipe
            .scroll(event, self.handle.width.get(), around.ends);
        self.handle.held.set(self.swipe.held());
        match swiped {
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
                    return Scrolled::Landed((shown, from, speed, 0, None));
                }
                let Some(target) = beside(step) else {
                    return Scrolled::Landed((vec![(front.clone(), 0)], 0., speed, step, None));
                };
                self.front = Some(target.clone());
                let shown = vec![(front.clone(), -step), (target.clone(), 0)];
                Scrolled::Landed((shown, from, speed, step, Some(target)))
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
        let Some(front) = self.tab() else {
            if self.track.reset_swipe() {
                self.parts.redraw_stage(cx);
            }
            return false;
        };
        if event.touch_phase == TouchPhase::Started && !pages_sideways(front) {
            if self.track.reset_swipe() {
                self.parts.redraw_stage(cx);
            }
            return false;
        }
        let front = front.id.clone();
        let around = self.around_visit();
        match self.track.scroll(event, &front, &around) {
            Scrolled::Ignored => return false,
            Scrolled::Glided => return true,
            Scrolled::Moved => {}
            Scrolled::Landed((shown, from, speed, step, target)) => {
                if step != 0 {
                    self.step_visit(step as isize, cx);
                }
                if step == 0 || target.is_some() {
                    let moved = self.track.travel(shown, from, Some(speed));
                    settle_part(
                        self,
                        moved,
                        |bench: &mut Self| &mut bench.track,
                        crate::workbench::PartSet::STAGE,
                        cx,
                    );
                }
            }
        }
        self.parts.redraw_stage(cx);
        true
    }

    /// Follows a scroll over the tab rail; true when it took it as a swipe between pages.
    fn swipe_rail(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) -> bool {
        let front = self.rails.page;
        let around = Around::in_order(&crate::pane_kind::PAGES, &front);
        match self.rails.slide.scroll(event, &front, &around) {
            Scrolled::Ignored => return false,
            Scrolled::Glided => return true,
            Scrolled::Moved => {}
            Scrolled::Landed((shown, from, speed, _, target)) => {
                if let Some(target) = target {
                    self.rails.page = target;
                    self.pages.leave();
                }
                let moved = self.rails.slide.travel(shown, from, Some(speed));
                settle_part(
                    self,
                    moved,
                    |bench: &mut Self| &mut bench.rails.slide,
                    crate::workbench::PartSet::AGENTS,
                    cx,
                );
            }
        }
        self.parts.redraw(&self.parts.agents, cx);
        true
    }

    /// Shows another of the tab rail's pages, springing over to it.
    pub fn show_rail_page(&mut self, page: TabRailPage, cx: &mut Context<Self>) {
        let was = std::mem::replace(&mut self.rails.page, page);
        if was != page {
            self.pages.leave();
            self.keep_rails(cx);
        }
        if let Some(moved) = self.rails.slide.turn(was, page, &crate::pane_kind::PAGES) {
            settle_part(
                self,
                moved,
                |bench: &mut Self| &mut bench.rails.slide,
                crate::workbench::PartSet::AGENTS,
                cx,
            );
        }
        self.parts.redraw(&self.parts.agents, cx);
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
        settle_part(
            self,
            moved,
            |bench: &mut Self| &mut bench.track,
            crate::workbench::PartSet::STAGE,
            cx,
        );
        self.parts.redraw_stage(cx);
    }
}

fn settle_part<K: 'static>(
    bench: &mut Workbench,
    moved: (usize, Duration),
    slide: fn(&mut Workbench) -> &mut Slide<K>,
    part: crate::workbench::PartSet,
    cx: &mut Context<Workbench>,
) {
    settle_with(bench, moved, slide, cx, move |bench, cx| {
        bench.parts.redraw_set(part, cx);
    });
}

/// Stops drawing a motion's extra things once it has arrived.
pub(crate) fn settle<T: 'static, K: 'static>(
    owner: &mut T,
    moved: (usize, Duration),
    slide: fn(&mut T) -> &mut Slide<K>,
    cx: &mut Context<T>,
) {
    settle_with(owner, moved, slide, cx, |_, cx| cx.notify());
}

fn settle_with<T: 'static, K: 'static>(
    owner: &mut T,
    (id, lasts): (usize, Duration),
    slide: fn(&mut T) -> &mut Slide<K>,
    cx: &mut Context<T>,
    redraw: impl FnOnce(&mut T, &mut Context<T>) + 'static,
) {
    let task = cx.spawn(async move |this, cx| {
        cx.background_executor().timer(lasts).await;
        let _ = this.update(cx, |owner, cx| {
            let row = slide(owner);
            if row.motion.as_ref().is_some_and(|motion| motion.id == id) {
                row.motion = None;
                redraw(owner, cx);
            }
        });
    });
    slide(owner).settle = Some(task);
}

/// Whether a swipe over this tab pages between tabs; an editor or a page scrolls sideways
/// itself, so its gestures stay its own.
fn pages_sideways(tab: &sikemux_view::layout::Tab) -> bool {
    crate::tabs::first_view(&tab.pane)
        .is_none_or(|(kind, _)| crate::pane_kind::of(kind).swipe_pages)
}

/// The stage's tabs: the one in front, with the one beside it while they move.
pub fn stage(
    bench: &Workbench,
    front: &sikemux_view::layout::Tab,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let strip = bench.strip();
    let tabs = strip.as_ref().map_or(&[][..], |strip| bench.tabs(strip));
    let (shown, start, motion) = bench.track.shown(front.id.clone(), || bench.around_visit());
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
        &bench.track.handle,
        cards,
        start,
        motion,
        Between::Gap(crate::theme::gap()),
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
    page: impl Fn(TabRailPage, &mut Context<Workbench>) -> AnyElement,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let front = bench.rails.page;
    let around = Around::in_order(&crate::pane_kind::PAGES, &front);
    let (shown, start, motion) = bench.rails.slide.shown(front, || around);
    let cards = shown
        .into_iter()
        .map(|(shown, place)| (page(shown, cx), place))
        .collect();
    let this = cx.entity().downgrade();
    row(
        "rail-track",
        &bench.rails.slide.handle,
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
    handle: &Handle,
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
        .child(listener(handle, gap, swiped))
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

/// Settles any swipe still held when focus leaves the window.
pub(crate) fn cancel_active(cx: &mut App) {
    finish_active(TouchPhase::Cancelled, cx);
}

fn active_owner() -> Option<usize> {
    ACTIVE.with(|slot| slot.borrow().as_ref().map(|active| active.owner))
}

fn clear_active(owner: usize) {
    ACTIVE.with(|slot| {
        let mut active = slot.borrow_mut();
        if active.as_ref().is_some_and(|held| held.owner == owner) {
            *active = None;
        }
    });
}

fn sync_active(
    owner: usize,
    held: bool,
    #[cfg(target_os = "macos")] window_view: Option<usize>,
    swiped: &SwipedFn,
) {
    if held {
        ACTIVE.with(|slot| {
            *slot.borrow_mut() = Some(ActiveSwipe {
                owner,
                #[cfg(target_os = "macos")]
                window_view,
                swiped: swiped.clone(),
            });
        });
    } else {
        clear_active(owner);
    }
}

fn finish_active(phase: TouchPhase, cx: &mut App) {
    let Some(active) = ACTIVE.with(|slot| slot.borrow_mut().take()) else {
        return;
    };
    let event = ScrollEvent {
        position: Point::default(),
        delta: ScrollDelta::Pixels(Point::default()),
        touch_phase: phase,
        modifiers: Modifiers::default(),
        source: ScrollSource::Pointer,
    };
    let _ = (active.swiped)(&event, cx);
}

/// Measures one place along its row, a thing and the gap after it, and hears scrolls over
/// it before what is under the pointer does, so a swipe does not also scroll what it starts on.
fn listener(
    handle: &Handle,
    gap: f32,
    swiped: impl Fn(&ScrollEvent, &mut App) -> bool + 'static,
) -> impl IntoElement {
    let handle = handle.clone();
    let swiped: SwipedFn = Rc::new(swiped);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            handle.width.set(f32::from(bounds.size.width) + gap);
            SCROLLERS.with(|scrollers| {
                scrollers
                    .borrow_mut()
                    .retain(|scroller| !bounds.contains(&scroller.center()))
            });
            #[cfg(target_os = "macos")]
            let window_view = macos::watch(window, cx);
            #[cfg(not(target_os = "macos"))]
            let _ = cx;
            let me = Rc::as_ptr(&handle.width) as usize;
            let held = handle.held.clone();
            let swiped = swiped.clone();
            window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                if event.touch_phase == TouchPhase::Started {
                    if active_owner() == Some(me) {
                        finish_active(TouchPhase::Cancelled, cx);
                    }
                    if !bounds.contains(&event.position) {
                        return;
                    }
                    gpui_transcript::scroll_latch::gesture_starts();
                    let over = SCROLLERS.with(|scrollers| {
                        scrollers
                            .borrow()
                            .iter()
                            .any(|scroller| scroller.contains(&event.position))
                    });
                    OWNER.set(if over { SIDEWAYS } else { me });
                    if over {
                        finish_active(TouchPhase::Cancelled, cx);
                    }
                } else if OWNER.get() != me {
                    return;
                }
                let inside = gpui_transcript::scroll_latch::taken_inside();
                let finishing =
                    matches!(event.touch_phase, TouchPhase::Ended | TouchPhase::Cancelled);
                if OWNER.get() == me && (!inside || (finishing && held.get())) {
                    let taken = swiped(event, cx);
                    sync_active(
                        me,
                        held.get(),
                        #[cfg(target_os = "macos")]
                        window_view,
                        &swiped,
                    );
                    if taken {
                        cx.stop_propagation();
                    }
                }
            });
            window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture && active_owner() == Some(me) {
                    finish_active(TouchPhase::Cancelled, cx);
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

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::{Cell, RefCell};
    use std::ffi::CStr;
    use std::ptr::NonNull;

    use block2::RcBlock;
    use gpui_kit::{App, TouchPhase, Window};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSView};
    use objc2_foundation::NSObjectProtocol;
    use tokio::sync::mpsc;

    use super::{finish_active, ACTIVE};

    thread_local! {
        static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
        static FINISH_TX: RefCell<Option<mpsc::UnboundedSender<TouchPhase>>> =
            const { RefCell::new(None) };
        static STARTED_OVER_NATIVE: Cell<bool> = const { Cell::new(false) };
    }

    /// Watches macOS scroll phases directly: GPUI maps `NSEventPhase::Cancelled` to `Moved`,
    /// and a native view (like a desk WKWebView) can slide under the pointer mid-swipe and
    /// swallow the gesture's `Ended` event.
    pub(super) fn watch(window: &Window, cx: &mut App) -> Option<usize> {
        let view = gpui_native_view::window_view(window)?;
        let ptr = Retained::as_ptr(&view) as usize;
        if MONITOR.with(|slot| slot.borrow().is_some()) {
            return Some(ptr);
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        FINISH_TX.with(|slot| *slot.borrow_mut() = Some(tx));
        cx.spawn(async move |cx| {
            while let Some(phase) = rx.recv().await {
                cx.update(|cx| finish_active(phase, cx));
            }
        })
        .detach();
        let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a live event for the length of the call.
            handle_scroll(unsafe { event.as_ref() }, event.as_ptr())
        });
        // SAFETY: main thread, as every element's paint is. AppKit copies the block and the
        // returned monitor stays alive in `MONITOR`.
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &block)
        };
        MONITOR.with(|slot| *slot.borrow_mut() = monitor);
        Some(ptr)
    }

    fn active_window_view() -> Option<usize> {
        ACTIVE.with(|slot| slot.borrow().as_ref().and_then(|active| active.window_view))
    }

    fn has_active() -> bool {
        ACTIVE.with(|slot| slot.borrow().is_some())
    }

    fn queue_finish(phase: TouchPhase) {
        FINISH_TX.with(|slot| {
            if let Some(tx) = slot.borrow().as_ref() {
                let _ = tx.send(phase);
            }
        });
    }

    fn handle_scroll(event: &NSEvent, raw: *mut NSEvent) -> *mut NSEvent {
        let phase = event.phase();
        let momentum = event.momentumPhase();
        if phase.intersects(NSEventPhase::MayBegin | NSEventPhase::Began) {
            let over_native = hit_native_view(event).is_some();
            STARTED_OVER_NATIVE.set(over_native);
            if over_native && has_active() {
                queue_finish(TouchPhase::Cancelled);
            }
            return raw;
        }
        let Some(active_view) = active_window_view() else {
            return raw;
        };
        let started_native = STARTED_OVER_NATIVE.get();
        let over_native_gpui = (!started_native)
            .then(|| hit_native_view(event))
            .flatten()
            .filter(|gpui_view| Retained::as_ptr(gpui_view) as usize == active_view);

        if phase.contains(NSEventPhase::Cancelled) {
            queue_finish(TouchPhase::Cancelled);
            return if over_native_gpui.is_some() {
                std::ptr::null_mut()
            } else {
                raw
            };
        }
        if phase.contains(NSEventPhase::Ended) {
            if let Some(gpui_view) = over_native_gpui {
                // SAFETY: `gpui_view` is a live `GPUIView` on the main thread and `raw` is a
                // live `NSEvent` for this call.
                let _: () = unsafe { msg_send![&*gpui_view, scrollWheel: raw] };
                if has_active() {
                    queue_finish(TouchPhase::Ended);
                }
                return std::ptr::null_mut();
            }
            queue_finish(TouchPhase::Ended);
            return raw;
        }
        if phase.intersects(NSEventPhase::Changed | NSEventPhase::Stationary) {
            if let Some(gpui_view) = over_native_gpui {
                // SAFETY: `gpui_view` is a live `GPUIView` on the main thread and `raw` is a
                // live `NSEvent` for this call.
                let _: () = unsafe { msg_send![&*gpui_view, scrollWheel: raw] };
                return std::ptr::null_mut();
            }
            return raw;
        }
        if phase.is_empty() && !momentum.is_empty() {
            queue_finish(TouchPhase::Ended);
            if over_native_gpui.is_some() {
                return std::ptr::null_mut();
            }
        }
        raw
    }

    /// Returns the window's `GPUIView` when the scroll event hits a native subview (like
    /// `WKWebView`), or `None` when GPUI itself owns the hit target.
    fn hit_native_view(event: &NSEvent) -> Option<Retained<NSView>> {
        let window = MainThreadMarker::new().and_then(|mtm| event.window(mtm))?;
        let content = window.contentView()?;
        let hit = content.hitTest(event.locationInWindow())?;
        if drawn_by_gpui(&hit) {
            return None;
        }
        gpui_view_in(content)
    }

    fn drawn_by_gpui(view: &NSView) -> bool {
        let is = |name: &CStr| AnyClass::get(name).is_some_and(|class| view.isKindOfClass(class));
        is(c"GPUIView") || is(c"GPUIOverlayView")
    }

    fn gpui_view_in(content: Retained<NSView>) -> Option<Retained<NSView>> {
        let class = AnyClass::get(c"GPUIView")?;
        let inside = content.subviews();
        std::iter::once(content)
            .chain(inside.to_vec())
            .find(|view| view.isKindOfClass(class))
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        div, point, px, Context, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render,
        ScrollDelta, ScrollEvent, ScrollSource, Styled, TestAppContext, TouchPhase, Window,
    };

    use super::{cancel_active, row, settle, Around, Between, Scrolled, Slide};

    struct SwipeHarness {
        order: Vec<usize>,
        front: usize,
        slide: Slide<usize>,
    }

    impl SwipeHarness {
        fn new() -> Self {
            Self {
                order: vec![0, 1, 2],
                front: 1,
                slide: Slide::default(),
            }
        }

        fn swipe(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) -> bool {
            let around = Around::in_order(&self.order, &self.front);
            match self.slide.scroll(event, &self.front, &around) {
                Scrolled::Ignored => return false,
                Scrolled::Glided => return true,
                Scrolled::Moved => {}
                Scrolled::Landed((shown, from, speed, _, target)) => {
                    if let Some(target) = target {
                        self.front = target;
                    }
                    let moved = self.slide.travel(shown, from, Some(speed));
                    settle(self, moved, |view: &mut Self| &mut view.slide, cx);
                }
            }
            cx.notify();
            true
        }
    }

    impl Render for SwipeHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let around = Around::in_order(&self.order, &self.front);
            let (shown, start, motion) = self.slide.shown(self.front, || around);
            let cards = shown
                .into_iter()
                .map(|(_, place)| (div().size_full().into_any_element(), place))
                .collect();
            let this = cx.entity().downgrade();
            div().size_full().flex().child(
                row(
                    "test-track",
                    &self.slide.handle(),
                    cards,
                    start,
                    motion,
                    Between::Gap(0.),
                    move |event, cx| {
                        this.update(cx, |view, cx| view.swipe(event, cx))
                            .unwrap_or(false)
                    },
                )
                .w(px(400.))
                .h_full(),
            )
        }
    }

    fn scroll(x: f32, dx: f32, touch_phase: TouchPhase) -> ScrollEvent {
        ScrollEvent {
            position: point(px(x), px(100.)),
            delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
            touch_phase,
            modifiers: gpui_kit::Modifiers::default(),
            source: ScrollSource::Pointer,
        }
    }

    #[gpui::test]
    fn swipe_ends_cleanly_after_pointer_leaves_row_bounds(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| SwipeHarness::new());
        cx.run_until_parked();

        cx.simulate_event(scroll(200., 0., TouchPhase::Started));
        cx.simulate_event(scroll(200., -40., TouchPhase::Moved));
        view.read_with(cx, |view, _| {
            assert!(view.slide.swipe.held());
            assert!(view.slide.swipe.offset > 0.);
        });

        // Pointer drifts past the 400px row into the right rail before fingers lift.
        cx.simulate_event(scroll(520., 0., TouchPhase::Ended));
        view.read_with(cx, |view, _| {
            assert!(!view.slide.swipe.held());
            assert_eq!(view.slide.swipe.offset, 0.);
            assert!(view.slide.motion.is_some());
        });
    }

    #[gpui::test]
    fn stranded_swipe_settles_on_cancel_click_or_new_gesture_elsewhere(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| SwipeHarness::new());
        cx.run_until_parked();

        // Missed Ended (e.g. macOS Cancelled or swallowed by WKWebView) settled by cancel_active.
        cx.simulate_event(scroll(200., 0., TouchPhase::Started));
        cx.simulate_event(scroll(200., -40., TouchPhase::Moved));
        view.read_with(cx, |view, _| assert!(view.slide.swipe.held()));
        cx.update(|_, cx| cancel_active(cx));
        view.read_with(cx, |view, _| {
            assert!(!view.slide.swipe.held());
            assert_eq!(view.slide.swipe.offset, 0.);
        });

        // Missed Ended settled by a click anywhere in the window.
        cx.simulate_event(scroll(200., 0., TouchPhase::Started));
        cx.simulate_event(scroll(200., -40., TouchPhase::Moved));
        view.read_with(cx, |view, _| assert!(view.slide.swipe.held()));
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(520.), px(100.)),
            modifiers: gpui_kit::Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        view.read_with(cx, |view, _| {
            assert!(!view.slide.swipe.held());
            assert_eq!(view.slide.swipe.offset, 0.);
        });

        // Missed Ended settled when a new scroll starts outside the row.
        cx.simulate_event(scroll(200., 0., TouchPhase::Started));
        cx.simulate_event(scroll(200., -40., TouchPhase::Moved));
        view.read_with(cx, |view, _| assert!(view.slide.swipe.held()));
        cx.simulate_event(scroll(520., 0., TouchPhase::Started));
        view.read_with(cx, |view, _| {
            assert!(!view.slide.swipe.held());
            assert_eq!(view.slide.swipe.offset, 0.);
        });
    }
}
