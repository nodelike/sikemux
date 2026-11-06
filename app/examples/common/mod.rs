//! What the examples that time the window share: the CPU a thread has used, the draw each
//! frame took, how a run keeps pace with the screen and how its numbers are printed. Each
//! example uses only some of it.
#![allow(dead_code)]

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Element, ElementId, GlobalElementId,
    HeadlessAppContext, InspectorElementId, IntoElement, LayoutId, Pixels, Window,
};

const FRAME: Duration = Duration::from_micros(8_333);

thread_local! {
    /// Each window draw's time from layout to paint, in ms.
    pub static DRAWS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

/// An element whose layout, prepaint and paint add to the last of [`DRAWS`]. The test platform
/// also draws whenever the window changes, so draws are timed wherever they happen.
pub struct Timed(pub AnyElement);

impl IntoElement for Timed {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Timed {
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
        let began = Instant::now();
        let layout = self.0.request_layout(window, cx);
        add_to_draw(began);
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
        let began = Instant::now();
        self.0.prepaint(window, cx);
        add_to_draw(began);
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
        let began = Instant::now();
        self.0.paint(window, cx);
        add_to_draw(began);
    }
}

fn add_to_draw(began: Instant) {
    DRAWS.with_borrow_mut(|draws| {
        if let Some(draw) = draws.last_mut() {
            *draw += began.elapsed().as_secs_f64() * 1e3;
        }
    });
}

/// The time this thread has run, which other processes busy on the machine do not inflate.
pub fn thread_cpu_ms() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime only writes the struct it is handed.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec as f64 * 1e3 + time.tv_nsec as f64 / 1e6
}

pub fn spread(mut values: Vec<f64>) -> String {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| {
        values
            .get(((values.len().max(1) - 1) as f64 * q).round() as usize)
            .copied()
            .unwrap_or(0.)
    };
    format!(
        "p50 {:.2} ms, p95 {:.2}, p99 {:.2}, max {:.2} ({} frames)",
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.),
        values.len()
    )
}

/// Keeps a run on the screen's beat, with the test clock moved on with the real one so the
/// window's timers fire.
pub struct Pace {
    last: Instant,
    clock: Instant,
}

impl Pace {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            clock: Instant::now(),
        }
    }

    /// Runs what is ready, as a window's thread would between frames. Never waits to park:
    /// while chats stream or terminals flood, more is always arriving.
    pub fn pump(&mut self, cx: &mut HeadlessAppContext) {
        cx.advance_clock(self.clock.elapsed());
        self.clock = Instant::now();
        let began = Instant::now();
        while cx.background_executor().tick() && began.elapsed() < Duration::from_millis(4) {}
    }

    /// Waits out the frame, runs what is ready and draws the window once.
    pub fn frame(&mut self, cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
        let wait = FRAME.saturating_sub(self.last.elapsed());
        std::thread::sleep(wait);
        self.last = Instant::now();
        self.pump(cx);
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .ok();
    }

    /// Starts the next frame's wait from now, after work that was not a frame.
    pub fn restart(&mut self) {
        self.last = Instant::now();
    }
}
