//! A two-finger trackpad swipe that pages between things laid side by side: the page
//! follows the hand one for one, moves at most one page per swipe, and when the hand
//! lifts a spring carries it from the hand's own speed onto the page it was going for.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use gpui_kit::{Animation, ScrollDelta, ScrollWheelEvent, TouchPhase};

/// Below this the gesture is diagonal enough to belong to whatever is under it.
const HORIZONTAL_RATIO: f32 = 1.5;
/// How far a page can be pulled past the last one there is.
const OVERSCROLL: f32 = 0.15;
/// How much of the pull gets through before the resistance takes over.
const GIVE: f32 = 0.55;
/// How far onto the next page a hand has to set a swipe down for it to take that page.
const COMMIT: f32 = 0.25;
/// How fast a hand still moving as it lifts has to go, in points a second, for the swipe
/// to take the next page however little of it shows.
const FLICK: f32 = 500.;
/// A hand lifting while moving back this fast has changed its mind.
const RECOIL: f32 = 200.;
/// Only the last moments before the hand lifts say how fast it was going.
const SPEED_WINDOW: Duration = Duration::from_millis(80);
/// The glide macOS sends after a landed swipe comes at least this often; a longer gap
/// means it is over.
const GLIDE_GAP: Duration = Duration::from_millis(100);
/// How quickly a landing closes: it settles in about a quarter of a second, without bouncing.
const SPRING_RATE: f32 = 25.;
pub const SPRING_LONGEST: Duration = Duration::from_millis(600);
/// How long a switch made some other way takes to travel one page.
pub const PAN: Duration = Duration::from_millis(280);

/// Whether there is a page on each side of the one a swipe starts on.
#[derive(Clone, Copy)]
pub struct Ends {
    pub previous: bool,
    pub next: bool,
}

/// What a swipe did with one scroll event.
pub enum Swiped {
    /// Not a swipe the pages take, so whatever is under it scrolls.
    Ignored,
    /// The pages follow the hand; they sit `Swipe::offset` pages along.
    Moved,
    /// The glide after a landing, which nothing under the pages may scroll by.
    Gliding,
    /// The hand lifted. The pages land `step` pages along (-1, 0 or 1), starting from
    /// `from` pages away from there at `speed` pages a second.
    Landed { step: i32, from: f32, speed: f32 },
}

#[derive(Default)]
pub struct Swipe {
    /// Whether the pages took the gesture under way, once its first move decided.
    claimed: Option<bool>,
    live: bool,
    raw: f32,
    /// Where the pages sit, in pages from the one the swipe started on.
    pub offset: f32,
    pushes: VecDeque<(Instant, f32)>,
    /// When the last of a landed swipe's glide came.
    glided: Option<Instant>,
}

impl Swipe {
    /// Whether a hand is moving the pages now.
    pub fn held(&self) -> bool {
        self.live && self.claimed == Some(true)
    }

    /// Follows one scroll event over pages `stride` points wide.
    pub fn scroll(&mut self, event: &ScrollWheelEvent, stride: f32, ends: Ends) -> Swiped {
        let ScrollDelta::Pixels(delta) = event.delta else {
            return Swiped::Ignored;
        };
        match event.touch_phase {
            TouchPhase::Started => {
                *self = Self {
                    live: true,
                    ..Self::default()
                };
                Swiped::Ignored
            }
            TouchPhase::Moved if self.live => {
                let (x, y) = (f32::from(delta.x), f32::from(delta.y));
                let claimed = *self
                    .claimed
                    .get_or_insert_with(|| x.abs() > HORIZONTAL_RATIO * y.abs());
                if !claimed || stride <= 0. {
                    return Swiped::Ignored;
                }
                let pushed = -x;
                let now = Instant::now();
                self.pushes
                    .retain(|(at, _)| now.duration_since(*at) <= SPEED_WINDOW);
                self.pushes.push_back((now, pushed));
                self.raw += pushed / stride;
                self.offset = drag_offset(self.raw, ends);
                Swiped::Moved
            }
            TouchPhase::Moved => {
                let now = Instant::now();
                match self.glided {
                    Some(at) if now.duration_since(at) <= GLIDE_GAP => {
                        self.glided = Some(now);
                        Swiped::Gliding
                    }
                    _ => {
                        self.glided = None;
                        Swiped::Ignored
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled if self.live => {
                self.live = false;
                if self.claimed != Some(true) {
                    return Swiped::Ignored;
                }
                self.glided = Some(Instant::now());
                let speed = if event.touch_phase == TouchPhase::Cancelled {
                    0.
                } else {
                    self.speed()
                };
                let step = landing(self.offset, speed, ends);
                let landed = Swiped::Landed {
                    step,
                    from: self.offset - step as f32,
                    speed: speed / stride,
                };
                self.offset = 0.;
                landed
            }
            _ => Swiped::Ignored,
        }
    }

    /// How fast the hand was going as it lifted, in points a second.
    fn speed(&self) -> f32 {
        let (Some((first, _)), Some((last, _))) = (self.pushes.front(), self.pushes.back()) else {
            return 0.;
        };
        let span = last.duration_since(*first).as_secs_f32();
        if self.pushes.len() < 2 || span <= 0. {
            return 0.;
        }
        let moved: f32 = self.pushes.iter().skip(1).map(|(_, pixels)| pixels).sum();
        moved / span
    }
}

/// Gives a little and then less and less, so an end feels like an end.
fn resisted(past: f32) -> f32 {
    (OVERSCROLL * GIVE * past) / (GIVE * past + OVERSCROLL)
}

/// Where the pages sit after a swipe has dragged `raw` pages: one for one up to the next
/// page, resisted past it or past an end, so one swipe never uncovers more than one page.
fn drag_offset(raw: f32, ends: Ends) -> f32 {
    let way = raw.signum();
    let reach = if (way > 0. && ends.next) || (way < 0. && ends.previous) {
        1.
    } else {
        0.
    };
    let past = raw.abs() - reach;
    if past <= 0. {
        raw
    } else {
        way * (reach + resisted(past))
    }
}

/// Which page a swipe lands on, -1, 0 or 1 from where it started, from how far it got and
/// how fast the hand was going, in points a second along the offset.
fn landing(offset: f32, speed: f32, ends: Ends) -> i32 {
    let way = offset.signum();
    let open = (way > 0. && ends.next) || (way < 0. && ends.previous);
    if offset == 0. || !open {
        return 0;
    }
    let along = speed * way;
    if along <= -RECOIL {
        return 0;
    }
    if offset.abs() >= COMMIT || along >= FLICK {
        way as i32
    } else {
        0
    }
}

/// The landing as an animation: a critically damped spring carrying the pages from
/// `from` pages away at `speed` pages a second to rest, as the fraction travelled.
pub fn spring(from: f32, speed: f32) -> Animation {
    let limit = SPRING_RATE * from.abs();
    let speed = speed.clamp(-limit, limit);
    let lean = speed + SPRING_RATE * from;
    let longest = SPRING_LONGEST.as_secs_f32();
    Animation::new(SPRING_LONGEST).with_easing(move |progress| {
        if from == 0. || progress >= 1. {
            return 1.;
        }
        let t = progress * longest;
        let at = (from + lean * t) * (-SPRING_RATE * t).exp();
        1. - at / from
    })
}

/// A switch made some other way, travelling one page and slowing as it arrives.
pub fn pan() -> Animation {
    Animation::new(PAN).with_easing(|progress| 1. - (1. - progress).powi(5))
}
