//! The Mac app's motion: its curves and timings, so what moves here moves as it does there.
//! Nothing summoned animates in, leaving is front-loaded, and nothing runs past 200 ms.
//! What the Mac stills while macOS asks for less motion is stilled here too; the few it lets
//! move anyway, here as there, say so with `Ease::moves_when_reduced`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::base::motion::Interpolate;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, Animation, AnyElement, App, Bounds, Div, Element, ElementId, GlobalElementId,
    Hsla, InspectorElementId, InteractiveElement, IntoElement, LayoutId, ParentElement, Pixels,
    Point, Rgba, SharedString, Styled, Svg, Transformation, Window,
};

/// How a value moves to a new target: over how long, on which curve, and whether it moves
/// while macOS asks for less motion.
#[derive(Clone, Copy)]
pub struct Ease {
    pub duration: Duration,
    pub curve: Curve,
    pub moves_when_reduced: bool,
}

#[derive(Clone, Copy)]
pub enum Curve {
    Linear,
    /// A CSS `cubic-bezier(x1, y1, x2, y2)`.
    Bezier(f32, f32, f32, f32),
}

impl Curve {
    /// Most of the movement early, then a long settle.
    pub const OUT: Curve = Curve::Bezier(0.16, 1., 0.3, 1.);
    /// CSS's plain `ease`.
    pub const CSS: Curve = Curve::Bezier(0.25, 0.1, 0.25, 1.);
    /// CSS's `ease-out`.
    pub const CSS_OUT: Curve = Curve::Bezier(0., 0., 0.58, 1.);
}

impl Ease {
    pub const fn new(millis: u64, curve: Curve) -> Self {
        Ease {
            duration: Duration::from_millis(millis),
            curve,
            moves_when_reduced: false,
        }
    }

    pub const fn moves_when_reduced(mut self) -> Self {
        self.moves_when_reduced = true;
        self
    }

    /// Where the curve is a share `t` of the way through.
    pub fn at(self, t: f32) -> f32 {
        match self.curve {
            Curve::Linear => t,
            Curve::Bezier(x1, y1, x2, y2) => cubic_bezier(x1, y1, x2, y2)(t),
        }
    }
}

/// A row's tint and text colour as it is chosen or hovered.
pub const ROW_TINT: Ease = Ease::new(90, Curve::Linear);
/// A control's wash and ink as it is hovered or pressed, as the rails' header actions.
pub const CONTROL_TINT: Ease = Ease::new(90, Curve::CSS);
/// A menu's item as the pointer comes over it, which the Mac fades even while macOS asks for
/// less motion.
pub const MENU_ITEM: Ease = Ease::new(100, Curve::CSS).moves_when_reduced();
/// A tab's or pill's tint as it is chosen or hovered.
pub const PILL_TINT: Ease = Ease::new(200, Curve::OUT);
/// A chevron turning as what it folds opens: in a chat or a review, where the Mac turns it
/// even while macOS asks for less motion.
pub const DETAIL_FOLD: Ease = Ease::new(120, Curve::CSS).moves_when_reduced();
/// A file tree's chevron.
pub const TREE_FOLD: Ease = Ease::new(140, Curve::CSS);
/// A list's group, a run's step or a release's assets.
pub const LIST_FOLD: Ease = Ease::new(90, Curve::CSS);
/// A team in the agents rail, or a lane on the board.
pub const LANE_FOLD: Ease = Ease::new(90, Curve::CSS_OUT);

/// A dialog's backdrop and the dialog rising over it, and a tooltip fading in.
pub const SCRIM_IN: Ease = Ease::new(140, Curve::CSS_OUT);
pub const DIALOG_IN: Ease = Ease::new(220, Curve::OUT);
pub const TOOLTIP_IN: Ease = Ease::new(110, Curve::CSS_OUT);

/// A docked rail tucking under the stage or coming out from under it, as a drawer does.
pub const RAIL_DRAWER: Ease = Ease::new(400, Curve::Bezier(0.25, 1., 0.5, 1.));
pub const RAIL_DRAWER_SHORTEST: Duration = Duration::from_millis(120);
/// A closed rail brought out over the stage at its edge, and put away again.
pub const PEEK_IN: Ease = Ease::new(180, Curve::Bezier(0.2, 0.8, 0.2, 1.));
pub const PEEK_OUT: Ease = Ease::new(140, Curve::Bezier(0.4, 0., 0.8, 0.2));

/// An agent's mark popping in as its state changes, and the two rings a mark that now needs
/// the person sends out, the second a little after the first.
pub const STATE_POP: Ease = Ease::new(200, Curve::OUT);
pub const NEEDS_YOU: Ease = Ease::new(900, Curve::Linear);
pub const RING: Ease = Ease::new(520, Curve::Bezier(0.2, 0.6, 0.3, 1.));
pub const SECOND_RING: Duration = Duration::from_millis(380);

/// An animation that plays once as its element first draws, on `ease`. It holds still while
/// macOS asks for less motion, so it is only for what the Mac stills then too.
pub fn entrance(ease: Ease) -> Animation {
    Animation::new(ease.duration).with_easing(move |t| ease.at(t))
}

/// An agent's row arriving in the rail: its slot opens and it fades in, and it slides in a
/// touch longer.
pub const ARRIVE: Ease = Ease::new(170, Curve::OUT);
pub const ARRIVE_SLIDE: Ease = Ease::new(190, Curve::OUT);

/// A run of tool calls opening onto its calls, and folding them away.
pub const UNFOLD: Ease = Ease::new(180, Curve::OUT);
pub const FOLD_AWAY: Ease = Ease::new(140, Curve::Bezier(0.4, 0., 1., 1.));

/// The angle of a fold's chevron, open or shut.
pub fn fold_angle(open: bool) -> f32 {
    if open {
        std::f32::consts::FRAC_PI_2
    } else {
        0.
    }
}

pub fn ease_out() -> impl Fn(f32) -> f32 {
    cubic_bezier(0.16, 1., 0.3, 1.)
}

/// `target` as it is drawn now: when it changes, the drawn value follows it from wherever it
/// was, as a CSS transition does. The first value is drawn as it is.
pub fn follow<T: Interpolate + PartialEq + 'static>(
    id: impl Into<ElementId>,
    target: T,
    ease: Ease,
    window: &mut Window,
    cx: &mut App,
) -> T {
    follow_over(id.into(), target, ease, |_, _| ease.duration, window, cx)
}

/// A share from 0 to 1, such as how far a rail is tucked away, following `target` as `follow`
/// does; a move turned round part way takes the share of `ease`'s time its distance is, and
/// never less than `shortest`.
pub fn follow_share(
    id: impl Into<ElementId>,
    target: f32,
    ease: Ease,
    shortest: Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let over = |from: &f32, to: &f32| {
        ease.duration
            .mul_f32((to - from).abs().min(1.))
            .max(shortest)
    };
    follow_over(id.into(), target, ease, over, window, cx)
}

fn follow_over<T: Interpolate + PartialEq + 'static>(
    id: ElementId,
    target: T,
    ease: Ease,
    over: impl Fn(&T, &T) -> Duration,
    window: &mut Window,
    cx: &mut App,
) -> T {
    let now = Instant::now();
    let state = window.use_keyed_state(id, cx, |_, _| Following {
        from: target.clone(),
        to: target.clone(),
        since: now,
        over: Duration::ZERO,
    });
    if state.read(cx).to != target {
        let still = cx.reduce_motion() && !ease.moves_when_reduced;
        let shown = state.read(cx).at(now, ease);
        let from = if still { target.clone() } else { shown };
        let over = over(&from, &target);
        state.update(cx, |following, _| {
            *following = Following {
                from,
                to: target.clone(),
                since: now,
                over,
            }
        });
    }
    let following = state.read(cx);
    if following.from != following.to && now - following.since < following.over {
        window.request_animation_frame();
    }
    following.at(now, ease)
}

/// The colour `channel` of the element `id` draws now, fading to `target`.
pub fn fade(
    id: &ElementId,
    channel: &'static str,
    target: Hsla,
    ease: Ease,
    window: &mut Window,
    cx: &mut App,
) -> Hsla {
    let id = ElementId::NamedChild(Arc::new(id.clone()), channel.into());
    follow(id, Paint(target.into()), ease, window, cx).0.into()
}

/// What `build` makes of `target` as it is drawn now, following it as `follow` does; for
/// drawing that has no window to hand.
pub fn animated<T: Interpolate + PartialEq + 'static>(
    id: impl Into<ElementId>,
    target: T,
    ease: Ease,
    build: impl FnOnce(T) -> AnyElement + 'static,
) -> Animated<T> {
    Animated {
        id: id.into(),
        target: Some(target),
        ease,
        build: Some(Box::new(build)),
    }
}

/// `icon` turned by `angle`, turning to a new angle as `ease` says, as a fold's chevron does.
pub fn turn(key: impl Into<SharedString>, angle: f32, ease: Ease, icon: Svg) -> Animated<f32> {
    animated(ElementId::Name(key.into()), angle, ease, move |angle| {
        icon.with_transformation(Transformation::rotate(radians(angle)))
            .into_any_element()
    })
}

pub struct Animated<T> {
    id: ElementId,
    target: Option<T>,
    ease: Ease,
    build: Option<Box<dyn FnOnce(T) -> AnyElement>>,
}

impl<T: Interpolate + PartialEq + 'static> IntoElement for Animated<T> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<T: Interpolate + PartialEq + 'static> Element for Animated<T> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let (Some(target), Some(build)) = (self.target.take(), self.build.take()) else {
            unreachable!("an animated element is laid out once");
        };
        let value = follow(self.id.clone(), target, self.ease, window, cx);
        let mut element = build(value);
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

/// What `build` makes of how far it is through playing once since `key` last changed: from
/// 0 to 1 over `ease`, and 1 when `key` was as it is when the element first drew.
pub fn replay<K: PartialEq + Clone + 'static>(
    id: impl Into<ElementId>,
    key: K,
    ease: Ease,
    build: impl FnOnce(f32) -> AnyElement + 'static,
) -> Replay<K> {
    Replay {
        id: id.into(),
        key: Some(key),
        ease,
        build: Some(Box::new(build)),
    }
}

pub struct Replay<K> {
    id: ElementId,
    key: Option<K>,
    ease: Ease,
    build: Option<Box<dyn FnOnce(f32) -> AnyElement>>,
}

impl<K: PartialEq + Clone + 'static> IntoElement for Replay<K> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<K: PartialEq + Clone + 'static> Element for Replay<K> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let (Some(key), Some(build)) = (self.key.take(), self.build.take()) else {
            unreachable!("a replaying element is laid out once");
        };
        let ease = self.ease;
        let kept = key.clone();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| (kept, None::<Instant>));
        if state.read(cx).0 != key {
            let moves = !cx.reduce_motion() || ease.moves_when_reduced;
            let since = moves.then(Instant::now);
            state.update(cx, |state, _| *state = (key, since));
        }
        let through = state.read(cx).1.map_or(1., |since| {
            (since.elapsed().as_secs_f32() / ease.duration.as_secs_f32()).min(1.)
        });
        if through < 1. {
            window.request_animation_frame();
        }
        let mut element = build(through);
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

/// `body` folded `shown` of the way open, from 0 shut to 1 at its own height; it fades as it
/// folds. Its height is the one it last drew at, so it must be drawn while it folds.
pub fn folded(
    id: impl Into<ElementId>,
    shown: f32,
    body: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    fold_along(id.into(), Along::Down, shown, false, body, window, cx)
}

/// Which way a slot opens and closes: in height, as a list's row, or in width, as a tab.
#[derive(Clone, Copy)]
pub enum Along {
    Down,
    Across,
}

/// `body` in a slot `shown` of its own size along `along`, faded as much. Before its size is
/// known it draws whole when `whole_first`, else shut.
fn fold_along(
    id: ElementId,
    along: Along,
    shown: f32,
    whole_first: bool,
    body: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let measured = window
        .use_keyed_state(id, cx, |_, _| Rc::new(Cell::new(0.)))
        .read(cx)
        .clone();
    let size = measured.get();
    let open = shown >= 1. || (whole_first && size == 0.);
    let slot = div().flex().overflow_hidden().opacity(shown);
    let slot = match along {
        Along::Down => slot.flex_col().when(!open, |slot| slot.h(px(size * shown))),
        Along::Across => slot.when(!open, |slot| slot.w(px(size * shown)).min_w_0()),
    };
    slot.child(div().flex_none().child(body))
        .on_children_prepainted(move |bounds, _, _| {
            if let Some(body) = bounds.first() {
                measured.set(match along {
                    Along::Down => body.size.height.into(),
                    Along::Across => body.size.width.into(),
                });
            }
        })
        .into_any_element()
}

/// How an item comes into a list: its slot opens on `grow`, and it slides in from `slide`
/// points to the left on its own ease.
#[derive(Clone, Copy)]
pub struct Arrival {
    pub along: Along,
    pub grow: Ease,
    pub slide: Option<(Ease, f32)>,
}

/// How an item leaves a list: what it shows fades on `fade`, sliding `slide` points left,
/// while its slot closes on `close`.
#[derive(Clone, Copy)]
pub struct Departure {
    pub along: Along,
    pub fade: Ease,
    pub slide: f32,
    pub close: Ease,
}

/// An agent's row coming into the rail, and leaving it: content gone in 50 ms, the slot
/// closing front-loaded, so the row answers the click at once.
pub const ROW_ARRIVAL: Arrival = Arrival {
    along: Along::Down,
    grow: ARRIVE,
    slide: Some((ARRIVE_SLIDE, 6.)),
};
pub const ROW_DEPARTURE: Departure = Departure {
    along: Along::Down,
    fade: Ease::new(50, Curve::Linear),
    slide: 6.,
    close: Ease::new(120, Curve::Bezier(0.25, 0.8, 0.25, 1.)),
};
/// A tab opening in its strip by widening, and closing by narrowing away.
pub const TAB_ARRIVAL: Arrival = Arrival {
    along: Along::Across,
    grow: Ease::new(160, Curve::OUT),
    slide: None,
};
pub const TAB_DEPARTURE: Departure = Departure {
    along: Along::Across,
    fade: Ease::new(50, Curve::Linear),
    slide: 0.,
    close: Ease::new(120, Curve::Bezier(0.25, 0.8, 0.25, 1.)),
};

impl Arrival {
    fn lasts(&self) -> Duration {
        self.grow
            .duration
            .max(self.slide.map_or(Duration::ZERO, |(ease, _)| ease.duration))
    }
}

/// An item that has just come into a list, as `arrival` says. It plays once, from when it
/// first draws.
pub fn arriving(
    id: impl Into<ElementId>,
    arrival: Arrival,
    body: impl IntoElement + 'static,
) -> WithWindow {
    let id: ElementId = id.into();
    with_window(move |window, cx| {
        let born = *window
            .use_keyed_state(id.clone(), cx, |_, _| Instant::now())
            .read(cx);
        let age = born.elapsed();
        if cx.reduce_motion() || age >= arrival.lasts() {
            return body.into_any_element();
        }
        window.request_animation_frame();
        let share = |ease: Ease| ease.at((age.as_secs_f32() / ease.duration.as_secs_f32()).min(1.));
        let body = match arrival.slide {
            Some((ease, from)) => div()
                .relative()
                .left(px(-from * (1. - share(ease))))
                .child(body)
                .into_any_element(),
            None => body.into_any_element(),
        };
        let slot = ElementId::NamedChild(Arc::new(id), "slot".into());
        fold_along(
            slot,
            arrival.along,
            share(arrival.grow),
            false,
            body,
            window,
            cx,
        )
    })
}

/// An item that left its list at `since`, drawn on its way out as `departure` says; nothing
/// once it has gone.
pub fn departing(
    id: impl Into<ElementId>,
    since: Instant,
    departure: Departure,
    body: impl IntoElement + 'static,
) -> WithWindow {
    let id: ElementId = id.into();
    with_window(move |window, cx| {
        let age = since.elapsed().as_secs_f32();
        let through = |ease: Ease| ease.at((age / ease.duration.as_secs_f32()).min(1.));
        if cx.reduce_motion() || through(departure.close) >= 1. {
            return div().into_any_element();
        }
        window.request_animation_frame();
        let faded = through(departure.fade);
        let body = div()
            .relative()
            .left(px(-departure.slide * faded))
            .opacity(1. - faded)
            .child(body);
        let left = 1. - through(departure.close);
        let slot = div().occlude().child(fold_along(
            id,
            departure.along,
            left,
            true,
            body,
            window,
            cx,
        ));
        slot.into_any_element()
    })
}

/// A selection gliding from the item it was on to the one it is on now, as tabs and the rail's
/// agent rows do on the Mac: the list draws a copy of the chosen surface moving between the
/// two, under its items, while the chosen item hides its own until the copy lands.
#[derive(Default)]
pub struct GlideTrack {
    /// Where the list's container and each item were drawn last, in the window.
    origin: Rc<Cell<Point<Pixels>>>,
    drawn: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    ids: Vec<String>,
    chosen: Option<String>,
    glide: Option<(Bounds<Pixels>, Bounds<Pixels>, Instant)>,
}

/// A tab's or an agent row's selection moving to another.
pub const GLIDE: Ease = Ease::new(180, Curve::OUT);

impl GlideTrack {
    /// Takes the chosen item, and answers whether it hides its own surface while the copy
    /// glides onto it from where the last one was drawn.
    pub fn choose(&mut self, chosen: Option<String>, reduced: bool) -> bool {
        if chosen != self.chosen && !reduced {
            let drawn = self.drawn.borrow();
            let at = |id: &Option<String>| {
                let ix = self.ids.iter().position(|each| Some(each) == id.as_ref())?;
                drawn.get(ix).copied()
            };
            self.glide = at(&self.chosen)
                .zip(at(&chosen))
                .map(|(from, to)| (from, to, Instant::now()));
        }
        if self
            .glide
            .is_some_and(|(_, _, since)| since.elapsed() >= GLIDE.duration)
        {
            self.glide = None;
        }
        self.chosen = chosen;
        self.glide.is_some()
    }

    /// Takes the list's items in the order they are drawn, after `choose`.
    pub fn set_order(&mut self, ids: Vec<String>) {
        self.ids = ids;
    }

    /// The gliding copy, drawn with `surface`, to put first in the list's container; none
    /// while nothing glides.
    pub fn copy(&self, surface: impl FnOnce(Div) -> Div + 'static) -> Option<AnyElement> {
        let (from, to, since) = self.glide?;
        let origin = self.origin.get();
        Some(
            with_window(move |window, _| {
                window.request_animation_frame();
                let through = GLIDE
                    .at((since.elapsed().as_secs_f32() / GLIDE.duration.as_secs_f32()).min(1.));
                let at = from.interpolate(&to, through);
                surface(
                    div()
                        .absolute()
                        .left(at.origin.x - origin.x)
                        .top(at.origin.y - origin.y)
                        .w(at.size.width)
                        .h(at.size.height),
                )
                .into_any_element()
            })
            .into_any_element(),
        )
    }

    /// Keeps where the list's container was drawn: give it to the `on_children_prepainted` of
    /// the element holding the container, with the container its first child.
    pub fn keep_origin(&self) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
        let origin = self.origin.clone();
        move |bounds, _, _| {
            if let Some(container) = bounds.first() {
                origin.set(container.origin);
            }
        }
    }

    /// Keeps where each item was drawn: give it to the container's `on_children_prepainted`,
    /// with `skip` children before the items, such as the gliding copy.
    pub fn keep_items(
        &self,
        skip: usize,
    ) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
        let drawn = self.drawn.clone();
        move |bounds, _, _| *drawn.borrow_mut() = bounds.into_iter().skip(skip).collect()
    }
}

/// What a list drew last and in which view of it: the items that have just come into it, and
/// copies of those that have just left it, with where they were, to draw on their way out.
pub struct ListMemory<T> {
    view: String,
    shown: Vec<(String, T)>,
    arrived: Vec<(String, Instant)>,
    left: Vec<Left<T>>,
}

/// An item that left a list: where it was, what it was, and when.
pub struct Left<T> {
    pub at: usize,
    pub id: String,
    pub item: T,
    pub since: Instant,
}

impl<T> Default for ListMemory<T> {
    fn default() -> Self {
        ListMemory {
            view: String::new(),
            shown: Vec::new(),
            arrived: Vec::new(),
            left: Vec::new(),
        }
    }
}

impl<T: Clone> ListMemory<T> {
    /// Takes the list `shown` in `view`, and answers which items have just arrived and which
    /// have just left, over `lasting`. A view drawn first, another view, or one that changed
    /// more than `most` items at once is a different list, which just shows.
    pub fn update(
        &mut self,
        view: String,
        shown: Vec<(String, T)>,
        most: usize,
        lasting: Duration,
    ) -> (Vec<String>, &[Left<T>]) {
        self.arrived.retain(|(_, at)| at.elapsed() < lasting);
        self.left.retain(|left| left.since.elapsed() < lasting);
        let was: std::collections::HashSet<&str> =
            self.shown.iter().map(|(id, _)| id.as_str()).collect();
        let now: std::collections::HashSet<&str> =
            shown.iter().map(|(id, _)| id.as_str()).collect();
        let added = now.difference(&was).count();
        let removed = was.difference(&now).count();
        let kept = now.intersection(&was).count();
        let same =
            view == self.view && added <= most && removed <= most && (kept > 0 || was.is_empty());
        if same {
            let at = Instant::now();
            let fresh: Vec<String> = now.difference(&was).map(|id| (*id).to_owned()).collect();
            self.arrived.extend(fresh.into_iter().map(|id| (id, at)));
            for (ix, (id, item)) in self.shown.iter().enumerate() {
                if !now.contains(id.as_str()) {
                    self.left.push(Left {
                        at: ix,
                        id: id.clone(),
                        item: item.clone(),
                        since: at,
                    });
                }
            }
        } else {
            self.arrived.clear();
            self.left.clear();
        }
        self.view = view;
        self.shown = shown;
        let arrived = self.arrived.iter().map(|(id, _)| id.clone()).collect();
        (arrived, &self.left)
    }
}

/// What `build` draws, given the window to draw it in; for drawing that has none to hand.
pub fn with_window(
    build: impl FnOnce(&mut Window, &mut App) -> AnyElement + 'static,
) -> WithWindow {
    WithWindow {
        build: Some(Box::new(build)),
    }
}

/// What draws an element once it has the window.
type Build = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;

pub struct WithWindow {
    build: Option<Build>,
}

impl IntoElement for WithWindow {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for WithWindow {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let build = self.build.take().expect("drawn once a frame");
        let mut element = build(window, cx);
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

/// Whether the pointer was over the element `id` when it last moved, and what keeps that
/// up to date, to give the element's `on_hover`.
pub fn hovered(
    id: &ElementId,
    window: &mut Window,
    cx: &mut App,
) -> (bool, impl Fn(&bool, &mut Window, &mut App) + 'static) {
    let id = ElementId::NamedChild(Arc::new(id.clone()), "hovered".into());
    let state = window.use_keyed_state(id, cx, |_, _| false);
    let over = *state.read(cx);
    let keep = move |inside: &bool, _: &mut Window, cx: &mut App| {
        state.update(cx, |over, cx| {
            if *over != *inside {
                *over = *inside;
                cx.notify();
            }
        })
    };
    (over, keep)
}

struct Following<T> {
    from: T,
    to: T,
    since: Instant,
    over: Duration,
}

impl<T: Interpolate> Following<T> {
    fn at(&self, now: Instant, ease: Ease) -> T {
        let t = match self.over.is_zero() {
            true => 1.,
            false => ((now - self.since).as_secs_f32() / self.over.as_secs_f32()).min(1.),
        };
        self.from.interpolate(&self.to, ease.at(t))
    }
}

/// `to` blended over `from` by `share`, as a transition between them blends.
pub fn blend(from: Hsla, to: Hsla, share: f32) -> Hsla {
    Paint(from.into())
        .interpolate(&Paint(to.into()), share)
        .0
        .into()
}

/// A colour blended as CSS blends one in a transition: in sRGB, weighted by its alpha, so a
/// tint fading in from clear keeps its hue.
#[derive(Clone, PartialEq)]
struct Paint(Rgba);

impl Interpolate for Paint {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        let (from, to) = (self.0, target.0);
        let a = from.a + (to.a - from.a) * progress;
        let channel = |x: f32, y: f32| match a {
            0. => 0.,
            _ => (x * from.a + (y * to.a - x * from.a) * progress) / a,
        };
        Paint(Rgba {
            r: channel(from.r, to.r),
            g: channel(from.g, to.g),
            b: channel(from.b, to.b),
            a,
        })
    }
}
