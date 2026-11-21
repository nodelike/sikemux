//! What the window's renderer paints behind panes and overlays with the shaders in
//! `assets/shaders`: the moving grain, the person's own picture dithered in its place,
//! the first-run setup's dithered mark and tiles, and the sky over the release notes. The grain and the picture
//! are on the window's ground, which macOS keeps in a layer of its own at one pixel per
//! point, so the grain moves without anything over it drawn again. The sky is over the
//! shell, so it moves by showing the whole last frame again with the clock moved on.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, point, px, relative, size, AnyElement, App, AppContext, Bounds, Context, Global,
    Hsla, IntoElement, ParentElement, Pixels, RawShaderProgram, RenderImage, Rgba, Styled, Task,
    Window,
};
use sikemux_process::clock::epoch_ms;
use sikemux_view::backdrop::{BackdropKind, GridShape, WornBackdrop};
use sikemux_view::inbox::Celebration;
use sikemux_view::prefs::{
    Motion, PaneBackdrop, PaneColours, PaneCoverage, PanePattern, Preferences,
};
use sikemux_wire::protocol::AgentGroup;

use crate::theme::{self, radius_3, Tokens};

macro_rules! program {
    ($name:literal) => {
        RawShaderProgram {
            name: $name,
            wgsl: include_str!(concat!(env!("OUT_DIR"), "/shaders/", $name, ".wgsl")),
            msl: include_str!(concat!(env!("OUT_DIR"), "/shaders/", $name, ".metal")),
        }
    };
}

static FIELD: RawShaderProgram = program!("field");
static KIND: RawShaderProgram = program!("kind");
static PICTURE: RawShaderProgram = program!("picture");
static SKY: RawShaderProgram = program!("sky");
static POSTER_MARK: RawShaderProgram = program!("poster_mark");
static TILE: RawShaderProgram = program!("tile");
static GLOW: RawShaderProgram = program!("glow");
static EDGE: RawShaderProgram = program!("edge");
static BURST: RawShaderProgram = program!("burst");

static MARK_PNG: &[u8] = include_bytes!("../../../brand/mark/mark-white-256.png");
/// How much of the distance between dots each dot fills, and how solid they are.
const MARK_DOT: f32 = 2. / 3.;
const MARK_OPACITY: f32 = 0.9;
/// At this spacing in device pixels a dot takes less of it, or the mark reads as solid.
const MARK_FINE_PITCH: f32 = 4.;

const FRAMES_PER_SECOND: u64 = 30;
/// Slower on battery, where every frame is paid for out of the charge.
const BATTERY_FRAMES_PER_SECOND: u64 = 20;
const FIELD_SPEED: f32 = 0.5;
/// How far the grid scrolls in a second at full drift, in points.
const GRID_DRIFT: f32 = 8.;
const SKY_SPEED: f32 = 0.35;
const GLOW_SPEED: f32 = 1.;
const TILE_SPEED: f32 = 1.2;
/// The setup's tiles and mark move in seconds.
const BREATHE_SPEED: f32 = 1.;
/// Where the clock stands for a person who asked for less motion, so everything holds still.
const STILL: f32 = 2.5;
/// High, because the grain's dots are the only paint and nothing is drawn between them.
const FIELD_OPACITY: f32 = 0.85;
const PICTURE_OPACITY: f32 = 0.6;
/// How solid the pane picture is behind a new chat's welcome.
const WELCOME_PICTURE: f32 = 0.4;
/// How much of a pane, from its top, the picture covers when it does not cover it all.
const PICTURE_BAND: f32 = 0.6;
/// The picture is drawn in dots of at least one point, so no more of it than that is kept.
const PICTURE_LONGEST: u32 = 2560;
/// The glow is a blur, so it is worked out on a small copy of the picture and stretched back.
const GLOW_LONGEST: u32 = 320;
/// How bright a place must be, from 0 to 1, before it glows.
const GLOW_THRESHOLD: f32 = 0.45;

/// How long the panes' backdrop keeps moving after the last agent stops, and how long it takes
/// to ease to a stop or back up to speed, in that many steps.
const REST_AFTER: Duration = Duration::from_secs(10);
const EASE: Duration = Duration::from_secs(2);
const EASE_STEPS: u32 = 8;

/// How long a waiting pane's edge breathes before it holds still at its brightest.
const BREATHING: Duration = Duration::from_secs(20);

/// A burst of sparks: how long it plays, how far its sparks reach, and where its middle is
/// from the top right of what the pane shows below its header, in points.
const BURST_LASTS: Duration = Duration::from_millis(950);
const SPARK_REACH: f32 = 18.;
const SPARK_FROM_RIGHT: f32 = 34.;
const SPARK_FROM_TOP: f32 = 24.;
/// A show: its bursts start a second apart, each within this many points of the pane's top or
/// sides, at these spots across and down what the pane shows below its header.
const SHOW_ZONE: f32 = 80.;
const SHOW_SPOTS: [Spot; 6] = [
    Spot::Top(0.5),
    Spot::Right(0.3),
    Spot::Top(0.2),
    Spot::Left(0.45),
    Spot::Top(0.8),
    Spot::Right(0.55),
];
/// A moment older than this when it arrives has passed, so it plays nothing.
const MOMENT_FRESH: Duration = Duration::from_secs(10);

/// How fast a backdrop runs: at its own speed while an agent works, held still at rest, so
/// movement behind the panes always means work and an idle window draws nothing.
#[derive(Clone, Copy, Default)]
struct Pace {
    working: bool,
    /// From 0, held still, to 1, the backdrop's own speed.
    now: f32,
    /// Where the backdrop's own clock stood, in seconds at full speed, when `now` last changed,
    /// and the window's shader time then.
    held: f32,
    since: f32,
    /// Counts changes of `working`, so an easing that a newer change overtook stops.
    turn: u64,
}

impl Pace {
    fn set(&mut self, now: f32, time: f32) {
        self.held += (time - self.since) * self.now;
        self.since = time;
        self.now = now;
    }

    /// Where a shader's clock starts and how fast it runs, for a backdrop of `speed`.
    fn clock(&self, speed: f32) -> (f32, f32) {
        (
            speed * (self.held - self.since * self.now),
            speed * self.now,
        )
    }
}

/// The pace of the window's panes together, and of each agent's pane for a theme whose
/// backdrop follows its own agent, with the agents waiting on the person.
#[derive(Clone)]
struct Paces {
    started: Instant,
    window: Pace,
    agents: HashMap<String, Pace>,
    waiting: HashSet<String>,
    /// The shader time the waiting panes' edges stop breathing.
    breathing_until: f32,
    burst: Option<Burst>,
    /// The count of the latest moment taken, so each plays once.
    celebrated: u64,
}

/// A burst playing in the pane of `agent` from the shader time `started`.
#[derive(Clone)]
struct Burst {
    agent: String,
    started: f32,
    /// Varies where its sparks fly from one burst to the next.
    seed: f32,
    show: bool,
}

impl Burst {
    fn lasts(&self) -> Duration {
        match self.show {
            true => Duration::from_secs(SHOW_SPOTS.len() as u64 - 1) + BURST_LASTS,
            false => BURST_LASTS,
        }
    }
}

/// Where one of a show's bursts goes: along the top, across by the share given, or along a
/// side, down by the share given.
#[derive(Clone, Copy)]
enum Spot {
    Top(f32),
    Left(f32),
    Right(f32),
}

impl Default for Paces {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            window: Pace::default(),
            agents: HashMap::new(),
            waiting: HashSet::new(),
            breathing_until: 0.,
            burst: None,
            celebrated: 0,
        }
    }
}

impl Global for Paces {}

impl Paces {
    /// The window's shader time now: seconds since its renderer started, near enough to when
    /// the window began animating.
    fn time(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    fn pace(&mut self, agent: Option<&str>) -> &mut Pace {
        match agent {
            None => &mut self.window,
            Some(agent) => self.agents.entry(agent.to_owned()).or_default(),
        }
    }

    fn moving(&self) -> bool {
        self.window.now > 0. || self.agents.values().any(|pace| pace.now > 0.)
    }

    fn breathing(&self) -> bool {
        !self.waiting.is_empty() && self.time() < self.breathing_until
    }

    fn bursting(&self) -> Option<&Burst> {
        (self.burst.as_ref())
            .filter(|burst| self.time() < burst.started + burst.lasts().as_secs_f32())
    }
}

/// Plays the latest moment the core says the theme celebrates in the pane of the agent `in_pane`,
/// once: when its count is new and it happened under 10 s ago. Less motion plays none.
pub fn celebrate(moment: Option<&Celebration>, in_pane: Option<&str>, cx: &mut App) {
    let Some(moment) = moment else {
        return;
    };
    let mut paces = cx.try_global::<Paces>().cloned().unwrap_or_default();
    if paces.celebrated == moment.count {
        return;
    }
    paces.celebrated = moment.count;
    let fresh = epoch_ms().saturating_sub(moment.at) < MOMENT_FRESH.as_millis() as u64;
    let drawn = (cx.try_global::<Grain>())
        .is_some_and(|grain| grain.worn.spark > 0. && !grain.worn.sparks.is_empty());
    let burst = (in_pane.filter(|_| fresh && drawn && !cx.reduce_motion())).map(|agent| Burst {
        agent: agent.to_owned(),
        started: paces.time(),
        seed: (moment.count % 997) as f32,
        show: moment.show,
    });
    let Some(lasts) = burst.as_ref().map(Burst::lasts) else {
        cx.set_global(paces);
        return;
    };
    paces.burst = burst;
    cx.set_global(paces);
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(lasts).await;
        cx.update(|cx| {
            if let Some(paces) = cx.try_global::<Paces>().cloned() {
                cx.set_global(paces);
                cx.refresh_windows();
            }
        });
    })
    .detach();
}

/// Takes whether any agent on the host in front works, and which of its open agents work and
/// which wait on the person, easing each backdrop up to speed at once when its work starts, or
/// down to still a while after it stops.
pub fn working(any: bool, agents: &[(String, AgentGroup)], cx: &mut App) {
    let mut paces = cx.try_global::<Paces>().cloned().unwrap_or_default();
    let before = paces.agents.len();
    paces
        .agents
        .retain(|agent, _| agents.iter().any(|(id, _)| id == agent));
    let mut changed = paces.agents.len() != before;
    let waiting: HashSet<String> = (agents.iter())
        .filter(|(_, group)| *group == AgentGroup::Blocked)
        .map(|(id, _)| id.clone())
        .collect();
    let newly_waiting = waiting.iter().any(|id| !paces.waiting.contains(id));
    if waiting != paces.waiting {
        paces.waiting = waiting;
        changed = true;
    }
    let mut easing = Vec::new();
    let all = std::iter::once((None, any)).chain(
        (agents.iter()).map(|(id, group)| (Some(id.as_str()), *group == AgentGroup::Working)),
    );
    for (agent, working) in all {
        let pace = paces.pace(agent);
        if pace.working != working {
            pace.working = working;
            pace.turn += 1;
            easing.push((agent.map(str::to_owned), working, pace.turn));
        }
    }
    if !changed && easing.is_empty() {
        return;
    }
    cx.set_global(paces);
    if newly_waiting {
        breathe(cx);
    }
    if mood(cx) {
        cx.refresh_windows();
    }
    for (agent, working, turn) in easing {
        ease(agent, working, turn, cx);
    }
}

/// Eases the backdrop of `agent`, or of the window for none, toward its own speed, or toward
/// still once it has rested for a while, unless a newer change than `turn` overtakes it.
fn ease(agent: Option<String>, working: bool, turn: u64, cx: &mut App) {
    let (wait, target) = match working {
        true => (Duration::ZERO, 1.),
        false => (REST_AFTER, 0.),
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        let by = 1. / EASE_STEPS as f32;
        loop {
            let more = cx.update(|cx| {
                let mut paces = cx.global::<Paces>().clone();
                let time = paces.time();
                let Some(pace) = (match &agent {
                    None => Some(&mut paces.window),
                    Some(agent) => paces.agents.get_mut(agent),
                }) else {
                    return false;
                };
                if pace.turn != turn || pace.now == target {
                    return false;
                }
                let now = match target > pace.now {
                    true => (pace.now + by).min(target),
                    false => (pace.now - by).max(target),
                };
                pace.set(now, time);
                let more = now != target;
                cx.set_global(paces);
                if agent.is_none() || mood(cx) {
                    cx.refresh_windows();
                }
                more
            });
            if !more {
                return;
            }
            cx.background_executor().timer(EASE / EASE_STEPS).await;
        }
    })
    .detach();
}

/// Starts the waiting panes' edges breathing again, and stops them once that is over.
fn breathe(cx: &mut App) {
    let Some(mut paces) = cx.try_global::<Paces>().cloned() else {
        return;
    };
    if paces.waiting.is_empty() || !mood(cx) {
        return;
    }
    paces.breathing_until = paces.time() + BREATHING.as_secs_f32();
    cx.set_global(paces);
    cx.spawn(async move |cx| {
        cx.background_executor().timer(BREATHING).await;
        cx.update(|cx| {
            if let Some(paces) = cx.try_global::<Paces>().cloned() {
                cx.set_global(paces);
                cx.refresh_windows();
            }
        });
    })
    .detach();
}

/// Whether the worn backdrop follows each pane's own agent.
fn mood(cx: &App) -> bool {
    cx.try_global::<Grain>()
        .is_some_and(|grain| grain.worn.look.mood)
}

/// A backdrop's clock: the window's, or for a theme whose backdrop follows its own agent that
/// agent's, still for a pane with none; held where it stands at rest, and still for less motion.
fn paced(speed: f32, agent: Option<&str>, cx: &App) -> (f32, f32) {
    if cx.reduce_motion() {
        return (STILL, 0.);
    }
    let Some(paces) = cx.try_global::<Paces>() else {
        return (0., 0.);
    };
    match (mood(cx), agent) {
        (false, _) => paces.window.clock(speed),
        (true, Some(agent)) => (paces.agents.get(agent)).map_or((0., 0.), |pace| pace.clock(speed)),
        (true, None) => (0., 0.),
    }
}

/// What panes show behind them, as the person's preferences say, and whether a sky is up.
/// Until the preferences are read, panes show nothing.
#[derive(Clone, Default)]
pub struct Grain {
    shown: bool,
    choice: PaneBackdrop,
    /// The theme's backdrop, or the kind the person picked, as the core resolved it.
    worn: WornBackdrop,
    /// From 0 to 2.
    strength: f32,
    look: Look,
    /// The picture the preferences ask for, which may not have been read yet.
    wanted: Option<Baked>,
    picture: Option<(Baked, Arc<RenderImage>)>,
    reading: Option<Baked>,
    skies: usize,
    glowing: bool,
}

impl Global for Grain {}

/// A picture as read, with the glow worked into it for one spread.
#[derive(Clone, PartialEq)]
struct Baked {
    path: String,
    glow_size: f32,
}

/// How the picture is dithered, as the preferences say.
#[derive(Clone, Copy, PartialEq, Default)]
struct Look {
    colours: PaneColours,
    inks: [Option<Rgba>; 3],
    pattern: PanePattern,
    dot_size: f32,
    contrast: f32,
    brightness: f32,
    glow: f32,
    full: bool,
}

impl Look {
    fn of(preferences: &Preferences) -> Self {
        let ink = |at: usize| {
            preferences
                .pane_inks
                .get(at)
                .map(|hex| theme::color(hex).to_rgb())
        };
        Self {
            colours: preferences.pane_colours,
            inks: [ink(0), ink(1), ink(2)],
            pattern: preferences.pane_pattern,
            dot_size: preferences.pane_dot_size as f32,
            contrast: preferences.pane_contrast as f32,
            brightness: preferences.pane_brightness as f32,
            glow: preferences.pane_glow as f32,
            full: preferences.pane_coverage == PaneCoverage::Full,
        }
    }

    /// The shader's params after the first four, which the caller fills.
    fn params(&self, cx: &App) -> [f32; gpui::RAW_SHADER_PARAMS] {
        let pack = |ink: Option<Rgba>| {
            ink.map_or(-1., |ink| {
                let byte = |channel: f32| (channel.clamp(0., 1.) * 255.).round();
                byte(ink.r) * 65536. + byte(ink.g) * 256. + byte(ink.b)
            })
        };
        let inks = match self.colours {
            PaneColours::Custom => self.inks,
            PaneColours::Theme | PaneColours::Picture => {
                let t = Tokens::get(cx);
                [None, Some(t.accent.to_rgb()), Some(t.text_primary.to_rgb())]
            }
        };
        let pattern = match self.pattern {
            PanePattern::Bayer2 => 2.,
            PanePattern::Bayer4 => 4.,
            PanePattern::Bayer8 => 8.,
            PanePattern::Noise => 0.,
        };
        let mut params = [0.; gpui::RAW_SHADER_PARAMS];
        params[3] = if self.full { 1. } else { 0. };
        params[4..13].copy_from_slice(&[
            self.dot_size,
            pattern,
            self.contrast,
            self.brightness,
            self.glow,
            if self.colours == PaneColours::Picture {
                1.
            } else {
                0.
            },
            pack(inks[0]),
            pack(inks[1]),
            pack(inks[2]),
        ]);
        params
    }
}

impl Grain {
    /// Only the grain moves: a picture holds still, and one still being read or a grain at no
    /// strength paints nothing.
    fn moving(&self, paced: bool) -> bool {
        let drawn = match self.choice {
            PaneBackdrop::Picture => self.wanted.is_none(),
            _ => self.worn.look.speed > 0.,
        };
        self.skies > 0 || self.glowing || (paced && self.shown && self.strength() > 0. && drawn)
    }

    /// The person's pane strength laid over what is worn, held where text over it still reads.
    fn strength(&self) -> f32 {
        match self.choice {
            PaneBackdrop::Picture | PaneBackdrop::None => self.strength,
            _ => (self.strength * self.worn.look.strength as f32).min(self.worn.most as f32),
        }
    }
}

/// Takes the preferences for what panes show, reading a newly chosen picture or spread of glow.
pub fn set(preferences: &Preferences, cx: &mut App) {
    let old = cx.try_global::<Grain>().cloned().unwrap_or_default();
    let wanted = (preferences.pane_image.clone())
        .filter(|_| preferences.pane_backdrop == PaneBackdrop::Picture)
        .map(|path| Baked {
            path,
            glow_size: preferences.pane_glow_size as f32,
        });
    let mut picture = old.picture.clone();
    if wanted.is_none() {
        if let Some((_, image)) = picture.take() {
            cx.drop_image(image, None);
        }
    }
    let mut reading = old.reading.clone();
    if let Some(wanted) = &wanted {
        let done = picture.as_ref().is_some_and(|(baked, _)| baked == wanted);
        if !done && reading.is_none() {
            read(wanted.clone(), cx);
            reading = Some(wanted.clone());
        }
    }
    let grain = Grain {
        shown: preferences.pane_backdrop != PaneBackdrop::None,
        choice: preferences.pane_backdrop,
        worn: old.worn.clone(),
        strength: preferences.pane_strength as f32,
        look: Look::of(preferences),
        wanted,
        picture,
        reading,
        skies: old.skies,
        glowing: old.glowing,
    };
    let changed = cx.try_global::<Grain>().is_none()
        || grain.shown != old.shown
        || grain.choice != old.choice
        || grain.strength != old.strength
        || grain.look != old.look
        || grain.wanted != old.wanted
        || grain.picture.as_ref().map(|(baked, _)| baked)
            != old.picture.as_ref().map(|(baked, _)| baked);
    if !changed && grain.reading == old.reading {
        return;
    }
    cx.set_global(grain);
    if changed {
        cx.refresh_windows();
    }
}

/// The kind the worn theme draws, for Settings to name.
pub fn theme_kind(cx: &App) -> BackdropKind {
    cx.try_global::<Grain>()
        .map_or(BackdropKind::Grain, |grain| grain.worn.look.kind)
}

/// Takes what the theme draws behind panes, or the kind the person picked, from the palette.
pub fn wear(worn: &WornBackdrop, cx: &mut App) {
    let Some(grain) = cx.try_global::<Grain>() else {
        cx.set_global(Grain {
            worn: worn.clone(),
            ..Grain::default()
        });
        return;
    };
    if grain.worn == *worn {
        return;
    }
    let mut grain = grain.clone();
    grain.worn = worn.clone();
    cx.set_global(grain);
    cx.refresh_windows();
}

fn read(wanted: Baked, cx: &mut App) {
    cx.spawn(async move |cx| {
        let baking = wanted.clone();
        let image = cx
            .background_spawn(async move { bake(Path::new(&baking.path), baking.glow_size) })
            .await;
        cx.update(|cx| {
            let Some(mut grain) = cx.try_global::<Grain>().cloned() else {
                return;
            };
            grain.reading = None;
            if grain.wanted.as_ref() != Some(&wanted) {
                if let Some(next) = grain.wanted.clone() {
                    read(next.clone(), cx);
                    grain.reading = Some(next);
                }
                cx.set_global(grain);
                return;
            }
            if let Some((_, old)) = grain.picture.take() {
                cx.drop_image(old, None);
            }
            grain.picture = image.map(|image| (wanted, image));
            cx.set_global(grain);
            cx.refresh_windows();
        })
    })
    .detach();
}

/// The picture at `path` as the shader takes it, no larger than its dots can show: its colours
/// laid on black, and in place of its opacity how much its bright parts glow, spread by `glow_size`.
fn bake(path: &Path, glow_size: f32) -> Option<Arc<RenderImage>> {
    use image::imageops::{self, FilterType};

    let decoded = image::open(path)
        .inspect_err(|error| eprintln!("sikemux: the pane picture does not open: {error}"))
        .ok()?;
    let fitted = match decoded.width().max(decoded.height()) > PICTURE_LONGEST {
        true => decoded.thumbnail(PICTURE_LONGEST, PICTURE_LONGEST),
        false => decoded,
    };
    let mut pixels = fitted.into_rgba8();
    let (width, height) = pixels.dimensions();
    let lum = |pixel: &image::Rgba<u8>| {
        let [r, g, b, a] = pixel.0.map(|channel| channel as f32 / 255.);
        (0.2126 * r + 0.7152 * g + 0.0722 * b) * a
    };
    let shrink = GLOW_LONGEST as f32 / width.max(height) as f32;
    let small = imageops::thumbnail(
        &pixels,
        ((width as f32 * shrink).round() as u32).max(1),
        ((height as f32 * shrink).round() as u32).max(1),
    );
    let bright = image::GrayImage::from_fn(small.width(), small.height(), |x, y| {
        let over = (lum(small.get_pixel(x, y)) - GLOW_THRESHOLD) / (1. - GLOW_THRESHOLD);
        image::Luma([(over.max(0.) * 255.) as u8])
    });
    let spread = 1. + glow_size * 10.;
    let near = imageops::fast_blur(&bright, spread);
    let far = imageops::fast_blur(&bright, spread * 3.);
    let glow = image::GrayImage::from_fn(bright.width(), bright.height(), |x, y| {
        let sum = near.get_pixel(x, y).0[0] as f32 * 1.2 + far.get_pixel(x, y).0[0] as f32 * 0.8;
        image::Luma([sum.min(255.) as u8])
    });
    let glow = imageops::resize(&glow, width, height, FilterType::Triangle);
    for (pixel, glow) in pixels.pixels_mut().zip(glow.pixels()) {
        let [r, g, b, a] = pixel.0;
        let on_black = |channel: u8| ((channel as u16 * a as u16) / 255) as u8;
        pixel.0 = [on_black(b), on_black(g), on_black(r), glow.0[0]];
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
}

/// What fills a pane's frame behind everything the pane shows.
pub fn field(cx: &App) -> Option<AnyElement> {
    field_at(None, cx).map(|draw| draw(1.))
}

/// What fills the frame of `agent`'s pane: its usual field, crossing over to a new chat's
/// welcome as `glow` runs from 0 to 1, with its edge lit while the agent waits on the person.
/// The welcome is the pane picture, strongly dithered, when one is set, and the glow when
/// none is.
pub fn pane_ground(agent: Option<&str>, cx: &App) -> impl Fn(f32) -> AnyElement + 'static {
    let field = field_at(agent, cx);
    let edge = agent.and_then(|agent| waiting_edge(agent, cx));
    let welcome_picture = welcome_picture(cx);
    let accent = Tokens::get(cx).accent.to_rgb();
    let (start, speed) = clock(GLOW_SPEED, cx);
    move |glow| {
        div()
            .absolute()
            .inset_0()
            .children(
                field
                    .as_ref()
                    .filter(|_| glow < 1.)
                    .map(|draw| draw(1. - glow)),
            )
            .children(
                welcome_picture
                    .as_ref()
                    .filter(|_| glow > 0.)
                    .map(|draw| draw(glow)),
            )
            .children((glow > 0. && welcome_picture.is_none()).then(|| {
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| {
                        let mut params = [0.; gpui::RAW_SHADER_PARAMS];
                        params[..4].copy_from_slice(&[
                            start,
                            speed,
                            window.ground_scale(),
                            inner_radius(),
                        ]);
                        params[4..8].copy_from_slice(&[accent.r, accent.g, accent.b, glow]);
                        let _ = window.paint_ground_shader(bounds, &GLOW, params, None);
                    },
                )
                .absolute()
                .inset_0()
            }))
            .children(edge.as_ref().map(|draw| draw()))
            .into_any_element()
    }
}

/// The warn dots along the edge of the pane of `agent` while it waits on the person, for a
/// theme whose backdrop follows its agents.
fn waiting_edge(agent: &str, cx: &App) -> Option<impl Fn() -> AnyElement + 'static> {
    let worn = &cx.try_global::<Grain>()?.worn;
    let paces = cx.try_global::<Paces>()?;
    if !worn.look.mood || worn.glow <= 0. || !paces.waiting.contains(agent) {
        return None;
    }
    let warn = theme::color(&worn.warn).to_rgb();
    let alpha = worn.glow as f32;
    let until = match cx.reduce_motion() {
        true => 0.,
        false => paces.breathing_until,
    };
    Some(move || {
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let mut params = [0.; gpui::RAW_SHADER_PARAMS];
                params[..8].copy_from_slice(&[
                    until,
                    window.ground_scale(),
                    inner_radius(),
                    0.,
                    warn.r,
                    warn.g,
                    warn.b,
                    alpha,
                ]);
                let _ = window.paint_ground_shader(bounds, &EDGE, params, None);
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    })
}

/// The burst of sparks near the top right of what the pane of `agent` shows below its header,
/// or a show's six along its top and sides, while one plays there.
pub fn burst_in(agent: Option<&str>, cx: &App) -> Option<AnyElement> {
    let burst = cx.try_global::<Paces>()?.bursting()?;
    if Some(burst.agent.as_str()) != agent {
        return None;
    }
    let worn = &cx.try_global::<Grain>()?.worn;
    let inks: Vec<Rgba> = (worn.sparks.iter())
        .map(|ink| theme::color(ink).to_rgb())
        .collect();
    let mut params = [0.; gpui::RAW_SHADER_PARAMS];
    params[..4].copy_from_slice(&[burst.started, 0., burst.seed, worn.spark as f32]);
    for (slot, ink) in params[4..].chunks_mut(4).zip(inks.iter().cycle().take(3)) {
        if let [r, g, b, ..] = slot {
            (*r, *g, *b) = (ink.r, ink.g, ink.b);
        }
    }
    let show = burst.show;
    Some(
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let inset = px(0.5 * SHOW_ZONE);
                let spots: Vec<_> = match show {
                    false => vec![(
                        point(
                            bounds.right() - px(SPARK_FROM_RIGHT),
                            bounds.top() + px(SPARK_FROM_TOP),
                        ),
                        SPARK_REACH,
                    )],
                    true => (SHOW_SPOTS.into_iter())
                        .map(|spot| {
                            let middle = match spot {
                                Spot::Top(across) => point(
                                    bounds.left() + bounds.size.width * across,
                                    bounds.top() + inset,
                                ),
                                Spot::Left(down) => point(
                                    bounds.left() + inset,
                                    bounds.top() + bounds.size.height * down,
                                ),
                                Spot::Right(down) => point(
                                    bounds.right() - inset,
                                    bounds.top() + bounds.size.height * down,
                                ),
                            };
                            (middle, 0.42 * SHOW_ZONE)
                        })
                        .collect(),
                };
                for (after, (middle, reach)) in spots.into_iter().enumerate() {
                    let mut one = params;
                    one[0] += after as f32;
                    one[1] = window.ground_scale();
                    one[2] += 31. * after as f32;
                    one[7] = reach;
                    let side = px(2.4 * reach);
                    let square = Bounds::centered_at(middle, size(side, side));
                    let _ = window.paint_ground_shader(square, &BURST, one, None);
                }
            },
        )
        .absolute()
        .inset_0()
        .into_any_element(),
    )
}

/// The pane picture as a new chat's welcome shows it: as solid as `WELCOME_PICTURE`, whatever
/// strength the panes take, faded by the number given.
fn welcome_picture(cx: &App) -> Option<Rc<dyn Fn(f32) -> AnyElement>> {
    let grain = cx.try_global::<Grain>().filter(|grain| grain.shown)?;
    let (_, picture) = grain.picture.as_ref()?;
    let mut params = picture_params(grain, cx);
    params[2] = WELCOME_PICTURE;
    Some(picture_field(grain, params, picture.clone()))
}

/// The pane picture over the part of the pane its coverage asks for.
fn picture_field(
    grain: &Grain,
    params: [f32; gpui::RAW_SHADER_PARAMS],
    picture: Arc<RenderImage>,
) -> Rc<dyn Fn(f32) -> AnyElement> {
    let band = match grain.look.full {
        true => relative(1.),
        false => relative(PICTURE_BAND),
    };
    Rc::new(move |solid| {
        let mut params = params;
        params[2] *= solid;
        picture_canvas(params, picture.clone(), inner_radius(), true)
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(band)
            .into_any_element()
    })
}

fn inner_radius() -> f32 {
    f32::from(radius_3()) - 1.
}

/// The pane's field drawn as solid as the number it is given, from 0 to 1.
fn field_at(agent: Option<&str>, cx: &App) -> Option<Rc<dyn Fn(f32) -> AnyElement>> {
    let grain = cx
        .try_global::<Grain>()
        .filter(|grain| grain.shown && grain.strength() > 0.)?;
    if let Some((_, picture)) = &grain.picture {
        return Some(picture_field(
            grain,
            picture_params(grain, cx),
            picture.clone(),
        ));
    }
    if grain.wanted.is_some() {
        return None;
    }
    let worn = (grain.choice != PaneBackdrop::Picture).then_some(&grain.worn);
    match worn.map(|worn| worn.look.kind) {
        None | Some(BackdropKind::Grain) => Some(grain_field(grain, worn, agent, cx)),
        Some(_) => worn.map(|worn| kind_field(worn, grain.strength(), agent, cx)),
    }
}

/// The grain in the theme's grain colour, or in the first ink the theme gives it.
fn grain_field(
    grain: &Grain,
    worn: Option<&WornBackdrop>,
    agent: Option<&str>,
    cx: &App,
) -> Rc<dyn Fn(f32) -> AnyElement> {
    let ink = worn
        .and_then(|worn| worn.inks.first())
        .map(|ink| theme::color(ink).to_rgb());
    let dot = ink.unwrap_or_else(|| Tokens::get(cx).shader_dot.to_rgb());
    let speed = FIELD_SPEED * worn.map_or(1., |worn| worn.look.speed as f32);
    let (start, speed) = paced(speed, agent, cx);
    let alpha = (dot.a * FIELD_OPACITY * grain.strength()).min(1.);
    Rc::new(move |solid| {
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let mut params = [0.; gpui::RAW_SHADER_PARAMS];
                params[..8].copy_from_slice(&[
                    start,
                    speed,
                    window.ground_scale(),
                    inner_radius(),
                    dot.r,
                    dot.g,
                    dot.b,
                    alpha * solid,
                ]);
                let _ = window.paint_ground_shader(bounds, &FIELD, params, None);
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    })
}

/// The gradient, the stars or the grid, as `kind.wgsl` takes them.
fn kind_field(
    worn: &WornBackdrop,
    strength: f32,
    agent: Option<&str>,
    cx: &App,
) -> Rc<dyn Fn(f32) -> AnyElement> {
    let look = &worn.look;
    let ink = |at: usize| {
        worn.inks.get(at).map_or(-1., |ink| {
            let ink = theme::color(ink).to_rgb();
            packed(ink.r * 255., ink.g * 255., ink.b * 255.)
        })
    };
    let (start, speed) = paced(look.speed as f32, agent, cx);
    let alpha = (look.kind.solid() as f32 * strength).min(1.);
    let (kind, own) = match look.kind {
        BackdropKind::Gradient => (1., [look.softness as f32, look.dither as f32, 0., 0.]),
        BackdropKind::Stars => (
            2.,
            [
                look.density as f32,
                look.twinkle as f32,
                look.drift as f32,
                f32::from(look.depth),
            ],
        ),
        BackdropKind::Grid | BackdropKind::Grain => (
            3.,
            [
                if look.shape == GridShape::Dots {
                    1.
                } else {
                    0.
                },
                look.cell as f32,
                look.weight as f32,
                look.drift as f32 * GRID_DRIFT,
            ],
        ),
    };
    let setup = [
        ink(0),
        ink(1),
        ink(2),
        alpha,
        kind,
        if look.coverage == PaneCoverage::Full {
            1.
        } else {
            0.
        },
        look.seed as f32,
        look.scale as f32,
    ];
    Rc::new(move |solid| {
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let mut params = [0.; gpui::RAW_SHADER_PARAMS];
                params[..4].copy_from_slice(&[start, speed, window.ground_scale(), inner_radius()]);
                params[4..12].copy_from_slice(&setup);
                params[7] *= solid;
                params[12..].copy_from_slice(&own);
                let _ = window.paint_ground_shader(bounds, &KIND, params, None);
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    })
}

/// Keeps the ground moving while a new chat's glow is on screen.
pub fn glow_shown(shown: bool, cx: &mut App) {
    let mut grain = cx.try_global::<Grain>().cloned().unwrap_or_default();
    if grain.glowing != shown {
        grain.glowing = shown;
        cx.set_global(grain);
    }
}

/// The pane picture as the preferences draw it now, for Settings to show beside its controls,
/// or nothing while no picture is read.
pub fn picture_preview(cx: &App) -> Option<impl IntoElement + Styled> {
    let grain = cx.try_global::<Grain>()?;
    let (_, picture) = grain.picture.as_ref()?;
    Some(picture_canvas(
        picture_params(grain, cx),
        picture.clone(),
        f32::from(radius_3()),
        false,
    ))
}

fn picture_params(grain: &Grain, cx: &App) -> [f32; gpui::RAW_SHADER_PARAMS] {
    let mut params = grain.look.params(cx);
    params[2] = (PICTURE_OPACITY * grain.strength).min(1.);
    params
}

fn picture_canvas(
    params: [f32; gpui::RAW_SHADER_PARAMS],
    picture: Arc<RenderImage>,
    radius: f32,
    ground: bool,
) -> impl IntoElement + Styled {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let mut params = params;
            params[1] = radius;
            let painted = match ground {
                true => {
                    params[0] = window.ground_scale();
                    window.paint_ground_shader(bounds, &PICTURE, params, Some(&picture))
                }
                false => {
                    params[0] = window.scale_factor();
                    window.paint_raw_shader(bounds, &PICTURE, params, Some(&picture))
                }
            };
            if let Err(error) = painted {
                eprintln!("sikemux: the pane picture does not draw: {error}");
            }
        },
    )
}

/// The release sky, in the accent running through the syntax pink to the syntax green,
/// fading out down it, with its top-left corner rounded to `corner`.
pub fn sky(corner: Pixels, opacity: f32, cx: &App) -> impl IntoElement + Styled {
    let t = Tokens::get(cx);
    let hues = [
        t.accent,
        theme::color(&t.highlight.function),
        theme::color(&t.highlight.string),
    ]
    .map(|hue: Hsla| hue.to_rgb());
    let (start, speed) = clock(SKY_SPEED, cx);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let mut params = [0.; gpui::RAW_SHADER_PARAMS];
            params[..4].copy_from_slice(&[start, speed, window.scale_factor(), f32::from(corner)]);
            for (slot, hue) in params[4..].as_chunks_mut::<4>().0.iter_mut().zip(hues) {
                slot.copy_from_slice(&[hue.r, hue.g, hue.b, hue.a * opacity]);
            }
            let _ = window.paint_raw_shader(bounds, &SKY, params, None);
        },
    )
}

/// The brand's mark, only its shape read: it is decoded once, on the first welcome shown.
fn mark_shape() -> Option<Arc<RenderImage>> {
    static SHAPE: OnceLock<Option<Arc<RenderImage>>> = OnceLock::new();
    SHAPE
        .get_or_init(|| {
            let pixels = image::load_from_memory(MARK_PNG)
                .inspect_err(|error| eprintln!("sikemux: the mark does not decode: {error}"))
                .ok()?
                .into_rgba8();
            Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
        })
        .clone()
}

/// The mark as a dither in the accent, come in as far as `resolved` says, with its dots
/// `spacing` points apart.
pub fn poster_mark(resolved: f32, spacing: f32, cx: &App) -> Option<impl IntoElement + Styled> {
    let shape = mark_shape()?;
    let accent = Tokens::get(cx).accent.to_rgb();
    let (start, speed) = clock(BREATHE_SPEED, cx);
    Some(canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let pitch = (spacing * window.scale_factor()).round().max(2.);
            let fill = if pitch <= MARK_FINE_PITCH {
                0.5
            } else {
                MARK_DOT
            };
            let dot = (pitch * fill).round().max(1.);
            let mut params = [0.; gpui::RAW_SHADER_PARAMS];
            params[..4].copy_from_slice(&[resolved, pitch, dot, 0.]);
            params[4..8].copy_from_slice(&[accent.r, accent.g, accent.b, MARK_OPACITY]);
            params[8..12].copy_from_slice(&[start, speed, window.scale_factor(), 0.]);
            if let Err(error) = window.paint_raw_shader(bounds, &POSTER_MARK, params, Some(&shape))
            {
                eprintln!("sikemux: the mark does not draw: {error}");
            }
        },
    ))
}

/// What one tile's dither on the setup's wall looks like. Sizes are in points, and clearances
/// run from the tile's own corners, so they hold whatever size the tile is drawn.
#[derive(Clone, Copy)]
pub struct WallDots {
    pub colour: Hsla,
    pub density: f32,
    pub opacity: f32,
    pub seed: f32,
    pub feather: f32,
    pub anchor: [f32; 4],
    pub mark_right: f32,
    pub mark_bottom: f32,
    pub label_right: f32,
    pub label_height: f32,
    pub pill_width: f32,
    pub pill_bottom: f32,
    pub running: bool,
}

/// Three whole numbers to 255 as one float, which holds them exactly.
fn packed(first: f32, second: f32, third: f32) -> f32 {
    let byte = |value: f32| value.clamp(0., 255.).round();
    byte(first) * 65536. + byte(second) * 256. + byte(third)
}

/// A tile's dither on the setup's wall, swelling and settling on its own clock.
pub fn wall_dots(dots: WallDots, cx: &App) -> impl IntoElement + Styled {
    let colour = dots.colour.to_rgb();
    let (start, speed) = clock(BREATHE_SPEED, cx);
    let rise = if dots.running {
        clock(TILE_SPEED, cx).1
    } else {
        0.
    };
    let unit = |points: f32| (points / 4.).ceil();
    let layout = [
        packed(
            unit(dots.mark_right),
            unit(dots.mark_bottom),
            unit(dots.label_right),
        ),
        packed(
            unit(dots.label_height),
            unit(dots.pill_width),
            unit(dots.pill_bottom),
        ),
        0.,
        rise,
    ];
    let colour = packed(colour.r * 255., colour.g * 255., colour.b * 255.);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let mut params = [0.; gpui::RAW_SHADER_PARAMS];
            params[..4].copy_from_slice(&[start, speed, window.scale_factor(), dots.density]);
            params[4..8].copy_from_slice(&[colour, dots.opacity, dots.seed, dots.feather]);
            params[8..12].copy_from_slice(&dots.anchor);
            params[12..16].copy_from_slice(&layout);
            let _ = window.paint_raw_shader(bounds, &TILE, params, None);
        },
    )
}

/// Keeps the sky moving while a screen that shows one is up, even with panes' grain off, and
/// even when the screen opens before the person's preferences are read.
pub fn sky_shown(shown: bool, cx: &mut App) {
    let mut grain = cx.try_global::<Grain>().cloned().unwrap_or_default();
    grain.skies = match shown {
        true => grain.skies + 1,
        false => grain.skies.saturating_sub(1),
    };
    cx.set_global(grain);
}

/// Where a shader's clock starts and how fast it runs, holding still for less motion.
fn clock(speed: f32, cx: &App) -> (f32, f32) {
    match cx.reduce_motion() {
        true => (STILL, 0.),
        false => (0., speed),
    }
}

/// Moves what the shaders show on while the window is in front and can be seen, at the rate
/// the power allows, which `on_battery` answers from the view.
pub fn animate<V: 'static>(window: &mut Window, cx: &mut Context<V>, on_battery: fn(&V) -> bool) {
    cx.default_global::<Paces>();
    follow_reduced_motion(cx);
    #[cfg(target_os = "linux")]
    cx.spawn(async move |bench, cx| {
        let mut changes = linux::watch_motion();
        while changes.recv().await.is_some() {
            if bench.update(cx, |_, cx| follow_reduced_motion(cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    let ticking = Rc::new(RefCell::new(None));
    let restart = |ticking: Rc<RefCell<Option<Task<()>>>>| {
        move |_: &mut V, window: &mut Window, cx: &mut Context<V>| {
            if window.is_window_active() {
                follow_reduced_motion(cx);
            }
            *ticking.borrow_mut() = tick(window, cx, on_battery);
        }
    };
    let on_activation = restart(ticking.clone());
    cx.observe_window_activation(window, move |bench, window, cx| {
        if window.is_window_active() {
            breathe(cx);
        }
        on_activation(bench, window, cx)
    })
    .detach();
    let on_visibility = restart(ticking.clone());
    cx.observe_window_visibility(window, move |bench, _, window, cx| {
        on_visibility(bench, window, cx)
    })
    .detach();
    cx.observe_global_in::<Grain>(window, restart(ticking.clone()))
        .detach();
    cx.observe_global_in::<Paces>(window, restart(ticking))
        .detach();
}

fn tick<V: 'static>(
    window: &Window,
    cx: &mut Context<V>,
    on_battery: fn(&V) -> bool,
) -> Option<Task<()>> {
    let moving = window.is_window_active()
        && window.is_visible()
        && !cx.reduce_motion()
        && cx.try_global::<Grain>().is_some_and(|grain| {
            let paces = cx.try_global::<Paces>();
            grain.moving(paces.is_some_and(Paces::moving))
                || (grain.worn.look.mood && paces.is_some_and(Paces::breathing))
                || paces.is_some_and(|paces| paces.bursting().is_some())
        });
    moving.then(|| {
        cx.spawn_in(window, async move |bench, cx| loop {
            let Ok(wait) = bench.update_in(cx, |bench, window, cx| {
                match cx
                    .try_global::<Grain>()
                    .is_some_and(|grain| grain.skies > 0 || grain.glowing)
                {
                    true => {
                        window.present_shaders();
                        #[cfg(feature = "drive")]
                        SKY_PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    false => window.present_ground(),
                }
                let rate = match on_battery(bench) {
                    true => BATTERY_FRAMES_PER_SECOND,
                    false => FRAMES_PER_SECOND,
                };
                Duration::from_millis(1000 / rate)
            }) else {
                break;
            };
            cx.background_executor().timer(wait).await;
        })
    })
}

/// How many frames the tick has shown a sky moving on, for the test bundle's driver.
#[cfg(feature = "drive")]
static SKY_PRESENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many skies are up, how many frames have moved them on, and what the tick waits on, for
/// the test bundle's driver.
#[cfg(feature = "drive")]
pub fn drive_state(window: &Window, cx: &App) -> serde_json::Value {
    serde_json::json!({
        "skies": cx.try_global::<Grain>().map(|grain| grain.skies),
        "presents": SKY_PRESENTS.load(std::sync::atomic::Ordering::Relaxed),
        "active": window.is_window_active(),
        "visible": window.is_visible(),
        "reduceMotion": cx.reduce_motion(),
    })
}

/// Takes the system's ask for less motion again, or the person's Motion choice over it. macOS changes it in System Settings, so reading
/// it each time the window comes back to the front follows it; Linux also says when it changes.
pub(crate) fn follow_reduced_motion(cx: &mut App) {
    let reduce = test_motion().unwrap_or_else(|| match crate::motion::chosen(cx) {
        Motion::System => system_reduces_motion(),
        Motion::Reduced | Motion::Off => true,
    });
    if cx.reduce_motion() != reduce {
        cx.set_reduce_motion(reduce);
        cx.refresh_windows();
    }
}

/// The test bundle's own say on motion over macOS's, from `SIKEMUX_GPUI_DRIVE_MOTION` "on" or
/// "off", so a check can watch the sky move on a machine that asks for less, and hold still.
#[cfg(feature = "drive")]
fn test_motion() -> Option<bool> {
    std::env::var("SIKEMUX_GPUI_DRIVE_MOTION")
        .ok()
        .map(|motion| motion == "off")
}

#[cfg(not(feature = "drive"))]
fn test_motion() -> Option<bool> {
    None
}

#[cfg(target_os = "macos")]
fn system_reduces_motion() -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: NSWorkspace's shared workspace always exists, and the setting is a plain BOOL.
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        msg_send![workspace, accessibilityDisplayShouldReduceMotion]
    }
}

#[cfg(target_os = "linux")]
fn system_reduces_motion() -> bool {
    linux::reduces_motion()
}

/// Windows' "Animation effects" switch, off when the person asks for less motion.
#[cfg(windows)]
fn system_reduces_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION,
    };
    let mut animates = 1i32;
    // SAFETY: this setting writes one BOOL into the integer it is handed.
    let read = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut animates).cast(), 0)
    };
    read != 0 && animates == 0
}

/// The desktop's animation setting, read through the desktop portal: GNOME's "enable
/// animations" switch, or KDE's animation speed set to instant.
#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::sync::mpsc;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    const GNOME: (&str, &str) = ("org.gnome.desktop.interface", "enable-animations");
    const KDE: (&str, &str) = ("org.kde.kdeglobals.KDE", "AnimationDurationFactor");

    static REDUCES: AtomicBool = AtomicBool::new(false);

    pub(super) fn reduces_motion() -> bool {
        REDUCES.load(Ordering::Relaxed)
    }

    /// Says each time the setting has been read, once at the start and then on every change.
    pub(super) fn watch_motion() -> mpsc::UnboundedReceiver<()> {
        let (changed, changes) = mpsc::unbounded_channel();
        let _ = std::thread::Builder::new()
            .name("reduce-motion".into())
            .spawn(move || {
                let _ = watch(changed);
            });
        changes
    }

    /// KDE's portal hands its settings over as the text they are kept in.
    fn number(value: &Value) -> Option<f64> {
        f64::try_from(value)
            .ok()
            .or_else(|| <&str>::try_from(value).ok()?.trim().parse().ok())
    }

    fn watch(changed: mpsc::UnboundedSender<()>) -> zbus::Result<()> {
        let bus = Connection::session()?;
        let portal = Proxy::new(
            &bus,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        let mut changes = portal.receive_signal("SettingChanged")?;
        let read = |(namespace, key): (&str, &str)| {
            portal
                .call::<_, _, OwnedValue>("ReadOne", &(namespace, key))
                .ok()
        };
        let mut gnome = read(GNOME);
        let mut kde = read(KDE);
        loop {
            let still = gnome.as_deref().and_then(|on| bool::try_from(on).ok()) == Some(false)
                || kde.as_deref().and_then(number) == Some(0.);
            REDUCES.store(still, Ordering::Relaxed);
            if changed.send(()).is_err() {
                return Ok(());
            }
            let Some(message) = changes.next() else {
                return Ok(());
            };
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            match (namespace.as_str(), key.as_str()) {
                GNOME => gnome = Some(value),
                KDE => kde = Some(value),
                _ => {}
            }
        }
    }
}
