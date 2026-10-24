//! What the window's renderer paints behind panes and overlays with the shaders in
//! `assets/shaders`: the moving grain, the person's own picture dithered in its place,
//! and the sky over the welcome screen and the release notes. The grain and the picture
//! are on the window's ground, which macOS keeps in a layer of its own at one pixel per
//! point, so the grain moves without anything over it drawn again. The sky is over the
//! shell, so it moves by showing the whole last frame again with the clock moved on.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    canvas, relative, App, AppContext, Context, Global, Hsla, IntoElement, RenderImage,
    ShaderProgram, Styled, Task, Window,
};
use sikemux_wire::protocol::Preferences;

use crate::theme::{self, Tokens, RADIUS_3};
use crate::workbench::Workbench;

macro_rules! program {
    ($name:literal) => {
        ShaderProgram {
            name: $name,
            wgsl: include_str!(concat!(env!("OUT_DIR"), "/shaders/", $name, ".wgsl")),
            msl: include_str!(concat!(env!("OUT_DIR"), "/shaders/", $name, ".metal")),
        }
    };
}

static FIELD: ShaderProgram = program!("field");
static PICTURE: ShaderProgram = program!("picture");
static SKY: ShaderProgram = program!("sky");

const FRAMES_PER_SECOND: u64 = 30;
/// Slower on battery, where every frame is paid for out of the charge.
const BATTERY_FRAMES_PER_SECOND: u64 = 20;
const FIELD_SPEED: f32 = 0.5;
const SKY_SPEED: f32 = 0.35;
/// Where the clock stands for a person who asked for less motion, so everything holds still.
const STILL: f32 = 2.5;
/// High, because the grain's dots are the only paint and nothing is drawn between them.
const FIELD_OPACITY: f32 = 0.85;
const PICTURE_OPACITY: f32 = 0.38;
/// How much of a pane, from its top, the picture covers.
const PICTURE_BAND: f32 = 0.6;
/// The picture is drawn in two-pixel dots, so no more of it than one texel per dot is kept.
const PICTURE_LONGEST: u32 = 2560;

/// What panes show behind them, as the person's preferences say, and whether a sky is up.
/// Until the preferences are read, panes show nothing.
#[derive(Clone)]
pub struct Grain {
    shown: bool,
    /// From 0 to 2.
    strength: f32,
    picture_path: Option<String>,
    picture: Picture,
    skies: usize,
}

#[derive(Clone)]
enum Picture {
    None,
    Loading,
    Loaded(Arc<RenderImage>),
}

impl Global for Grain {}

impl Default for Grain {
    fn default() -> Self {
        Self {
            shown: false,
            strength: 0.,
            picture_path: None,
            picture: Picture::None,
            skies: 0,
        }
    }
}

impl Grain {
    /// Only the grain moves: a picture holds still, and one still loading or a grain at no
    /// strength paints nothing.
    fn moving(&self) -> bool {
        self.skies > 0
            || (self.shown && self.strength > 0. && matches!(self.picture, Picture::None))
    }
}

/// Takes the preferences for what panes show, loading a newly chosen picture.
pub fn set(preferences: &Preferences, cx: &mut App) {
    let path = preferences.pane_image.clone();
    let known = cx.try_global::<Grain>().map(|grain| {
        let kept = (grain.picture_path == path).then(|| grain.picture.clone());
        (kept, grain.skies)
    });
    let skies = known.as_ref().map_or(0, |(_, skies)| *skies);
    let picture = match known.and_then(|(kept, _)| kept) {
        Some(kept) => kept,
        None => {
            if let Some(path) = &path {
                load(path.clone(), cx);
            }
            path.as_ref().map_or(Picture::None, |_| Picture::Loading)
        }
    };
    let changed = cx.try_global::<Grain>().is_none_or(|grain| {
        grain.shown != preferences.pane_shader
            || grain.strength != preferences.pane_strength as f32
            || grain.picture_path != path
    });
    if !changed {
        return;
    }
    cx.set_global(Grain {
        shown: preferences.pane_shader,
        strength: preferences.pane_strength as f32,
        picture_path: path,
        picture,
        skies,
    });
    cx.refresh_windows();
}

fn load(path: String, cx: &mut App) {
    cx.spawn(async move |cx| {
        let read = path.clone();
        let picture = cx
            .background_spawn(async move { read_picture(Path::new(&read)) })
            .await;
        cx.update(|cx| {
            if let Some(grain) = cx.try_global::<Grain>() {
                if grain.picture_path.as_deref() == Some(path.as_str()) {
                    let mut grain = grain.clone();
                    grain.picture = match picture {
                        Some(picture) => Picture::Loaded(picture),
                        None => Picture::None,
                    };
                    cx.set_global(grain);
                    cx.refresh_windows();
                }
            }
        })
    })
    .detach();
}

/// The picture at `path` as the atlas takes it, no larger than its dots can show.
fn read_picture(path: &Path) -> Option<Arc<RenderImage>> {
    let decoded = image::open(path)
        .inspect_err(|error| eprintln!("sikemux: the pane picture does not open: {error}"))
        .ok()?;
    let fitted = match decoded.width().max(decoded.height()) > PICTURE_LONGEST {
        true => decoded.thumbnail(PICTURE_LONGEST, PICTURE_LONGEST),
        false => decoded,
    };
    let mut pixels = fitted.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
}

/// What fills a pane's frame behind everything the pane shows.
pub fn field(cx: &App) -> Option<impl IntoElement> {
    let grain = cx
        .try_global::<Grain>()
        .filter(|grain| grain.shown && grain.strength > 0.)?;
    let inner_radius = f32::from(RADIUS_3) - 1.;
    if let Picture::Loaded(picture) = &grain.picture {
        let picture = picture.clone();
        let strength = (PICTURE_OPACITY * grain.strength).min(1.);
        return Some(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    let mut params = [0.; gpui::SHADER_PARAMS];
                    params[..3].copy_from_slice(&[window.ground_scale(), inner_radius, strength]);
                    if let Err(error) =
                        window.paint_ground_shader(bounds, &PICTURE, params, Some(&picture))
                    {
                        eprintln!("sikemux: the pane picture does not draw: {error}");
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(relative(PICTURE_BAND))
            .into_any_element(),
        );
    }
    if matches!(grain.picture, Picture::Loading) {
        return None;
    }
    let dot = Tokens::get(cx).shader_dot.to_rgb();
    let (start, speed) = clock(FIELD_SPEED, cx);
    let alpha = (dot.a * FIELD_OPACITY * grain.strength).min(1.);
    Some(
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let mut params = [0.; gpui::SHADER_PARAMS];
                params[..8].copy_from_slice(&[
                    start,
                    speed,
                    window.ground_scale(),
                    inner_radius,
                    dot.r,
                    dot.g,
                    dot.b,
                    alpha,
                ]);
                let _ = window.paint_ground_shader(bounds, &FIELD, params, None);
            },
        )
        .absolute()
        .inset_0()
        .into_any_element(),
    )
}

/// The release sky, in the accent running through the syntax pink to the syntax green,
/// fading out from `fade_from` of the way down to nothing at the bottom.
pub fn sky(fade_from: f32, opacity: f32, cx: &App) -> impl IntoElement + Styled {
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
            let mut params = [0.; gpui::SHADER_PARAMS];
            params[..4].copy_from_slice(&[start, speed, window.scale_factor(), fade_from]);
            for (slot, hue) in params[4..].as_chunks_mut::<4>().0.iter_mut().zip(hues) {
                slot.copy_from_slice(&[hue.r, hue.g, hue.b, hue.a * opacity]);
            }
            let _ = window.paint_shader(bounds, &SKY, params, None);
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
/// the power allows.
pub fn animate(window: &mut Window, cx: &mut Context<Workbench>) {
    follow_reduced_motion(cx);
    let ticking = Rc::new(RefCell::new(None));
    let restart = |ticking: Rc<RefCell<Option<Task<()>>>>| {
        move |_: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>| {
            if window.is_window_active() {
                follow_reduced_motion(cx);
            }
            *ticking.borrow_mut() = tick(window, cx);
        }
    };
    cx.observe_window_activation(window, restart(ticking.clone()))
        .detach();
    let on_visibility = restart(ticking.clone());
    cx.observe_window_visibility(window, move |bench, _, window, cx| {
        on_visibility(bench, window, cx)
    })
    .detach();
    cx.observe_global_in::<Grain>(window, restart(ticking))
        .detach();
}

fn tick(window: &Window, cx: &mut Context<Workbench>) -> Option<Task<()>> {
    let moving = window.is_window_active()
        && window.is_visible()
        && !cx.reduce_motion()
        && cx.try_global::<Grain>().is_some_and(Grain::moving);
    moving.then(|| {
        cx.spawn_in(window, async move |bench, cx| loop {
            let Ok(wait) = bench.update_in(cx, |bench, window, cx| {
                match cx
                    .try_global::<Grain>()
                    .is_some_and(|grain| grain.skies > 0)
                {
                    true => {
                        window.present_shaders();
                        #[cfg(feature = "drive")]
                        SKY_PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    false => window.present_ground(),
                }
                let on_battery = bench.battery.is_some_and(|battery| !battery.charging);
                let rate = match on_battery {
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

/// Takes macOS's ask for less motion again. It is changed in System Settings, so reading it
/// each time the window comes back to the front follows it.
fn follow_reduced_motion(cx: &mut App) {
    let reduce = test_motion().unwrap_or_else(system_reduces_motion);
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

#[cfg(not(target_os = "macos"))]
fn system_reduces_motion() -> bool {
    false
}
