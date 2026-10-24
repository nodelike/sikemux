//! The one clock every moving view keeps time by. A view drawn since the last tick is drawn
//! again in place on the next, so all that moves costs one small frame together and never
//! draws the rows around it again. Views hold still while the window is behind others or out
//! of sight, and start again when it comes back.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWeakEntity, App, Bounds, Context, Element, ElementId, Entity, Global,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Render, Task, Window,
};

use crate::workbench::Workbench;

const FRAMES_PER_SECOND: u64 = 30;

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// How long the clock has run, which every moving view reads where it is from.
pub fn seconds() -> f32 {
    STARTED.elapsed().as_secs_f32()
}

/// Starts what moves again when the window comes back to the front or into sight.
pub fn follow(window: &mut Window, cx: &mut Context<Workbench>) {
    cx.observe_window_activation(window, |_, _, cx| wake(cx))
        .detach();
    cx.observe_window_visibility(window, |_, _, _, cx| wake(cx))
        .detach();
}

/// Has the clock draw the view `cx` draws again on its next tick, or once the window is back
/// when it is behind others or out of sight; answers whether the view moves now.
pub fn tick<V: 'static>(window: &Window, cx: &mut Context<V>) -> bool {
    let view = AnyWeakEntity::from(cx.weak_entity());
    let clock = cx.default_global::<Clock>();
    if !(window.is_window_active() && window.is_visible()) {
        add(&mut clock.resting, view);
        return false;
    }
    add(&mut clock.drawn, view);
    if clock.ticking.is_some() {
        return true;
    }
    let tick = cx.spawn(async |_, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(1000 / FRAMES_PER_SECOND))
            .await;
        cx.update(|cx| {
            let clock = cx.default_global::<Clock>();
            clock.ticking = None;
            let drawn = std::mem::take(&mut clock.drawn);
            for view in drawn.iter().filter_map(AnyWeakEntity::upgrade) {
                cx.repaint(view.entity_id());
            }
        });
    });
    cx.default_global::<Clock>().ticking = Some(tick);
    true
}

/// The views drawn since the clock last moved them, those resting until the window is back,
/// and its next move.
#[derive(Default)]
struct Clock {
    drawn: Vec<AnyWeakEntity>,
    resting: Vec<AnyWeakEntity>,
    ticking: Option<Task<()>>,
}

impl Global for Clock {}

fn wake(cx: &mut App) {
    let resting = std::mem::take(&mut cx.default_global::<Clock>().resting);
    for view in resting.iter().filter_map(AnyWeakEntity::upgrade) {
        cx.repaint(view.entity_id());
    }
}

fn add(views: &mut Vec<AnyWeakEntity>, view: AnyWeakEntity) {
    if !views
        .iter()
        .any(|known| known.entity_id() == view.entity_id())
    {
        views.push(view);
    }
}

/// Where a moving view sits in a row: it keeps the view under `key` from one frame to the
/// next, so the clock can draw it again on its own, and puts in what the row says it is now.
pub struct Slot<V> {
    key: ElementId,
    view: Option<V>,
    drawn: Option<AnyElement>,
}

impl<V: Render> Slot<V> {
    pub fn new(key: impl Into<ElementId>, view: V) -> Self {
        Self {
            key: key.into(),
            view: Some(view),
            drawn: None,
        }
    }
}

impl<V: Render> IntoElement for Slot<V> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<V: Render> Element for Slot<V> {
    type RequestLayoutState = ();
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
    ) -> (LayoutId, ()) {
        let mut fresh = self.view.take();
        let view: Entity<V> = window.use_keyed_state(self.key.clone(), cx, |_, _| {
            fresh.take().expect("a slot is laid out once a frame")
        });
        if let Some(fresh) = fresh {
            view.update(cx, |view, _| *view = fresh);
        }
        let mut drawn = view.into_any_element();
        let layout = drawn.request_layout(window, cx);
        self.drawn = Some(drawn);
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(drawn) = self.drawn.as_mut() {
            drawn.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(drawn) = self.drawn.as_mut() {
            drawn.paint(window, cx);
        }
    }
}
