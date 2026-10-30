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
    canvas, relative, App, AppContext, Context, Global, Hsla, IntoElement, RenderImage, Rgba,
    ShaderProgram, Styled, Task, Window,
};
use sikemux_wire::protocol::{PaneColours, PaneCoverage, PanePattern, Preferences};

use crate::theme::{self, Tokens, RADIUS_3};

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
const PICTURE_OPACITY: f32 = 0.6;
/// How much of a pane, from its top, the picture covers when it does not cover it all.
const PICTURE_BAND: f32 = 0.6;
/// The picture is drawn in dots of at least one point, so no more of it than that is kept.
const PICTURE_LONGEST: u32 = 2560;
/// The glow is a blur, so it is worked out on a small copy of the picture and stretched back.
const GLOW_LONGEST: u32 = 320;
/// How bright a place must be, from 0 to 1, before it glows.
const GLOW_THRESHOLD: f32 = 0.45;

/// What panes show behind them, as the person's preferences say, and whether a sky is up.
/// Until the preferences are read, panes show nothing.
#[derive(Clone, Default)]
pub struct Grain {
    shown: bool,
    /// From 0 to 2.
    strength: f32,
    look: Look,
    /// The picture the preferences ask for, which may not have been read yet.
    wanted: Option<Baked>,
    picture: Option<(Baked, Arc<RenderImage>)>,
    reading: Option<Baked>,
    skies: usize,
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
    fn params(&self, cx: &App) -> [f32; gpui::SHADER_PARAMS] {
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
        let mut params = [0.; gpui::SHADER_PARAMS];
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
    fn moving(&self) -> bool {
        self.skies > 0 || (self.shown && self.strength > 0. && self.wanted.is_none())
    }
}

/// Takes the preferences for what panes show, reading a newly chosen picture or spread of glow.
pub fn set(preferences: &Preferences, cx: &mut App) {
    let old = cx.try_global::<Grain>().cloned().unwrap_or_default();
    let wanted = preferences.pane_image.clone().map(|path| Baked {
        path,
        glow_size: preferences.pane_glow_size as f32,
    });
    let mut picture = old.picture.clone();
    if let Some((baked, image)) = &picture {
        if wanted
            .as_ref()
            .is_none_or(|wanted| wanted.path != baked.path)
        {
            cx.drop_image(image.clone(), None);
            picture = None;
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
        shown: preferences.pane_shader,
        strength: preferences.pane_strength as f32,
        look: Look::of(preferences),
        wanted,
        picture,
        reading,
        skies: old.skies,
    };
    let changed = cx.try_global::<Grain>().is_none()
        || grain.shown != old.shown
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
pub fn field(cx: &App) -> Option<impl IntoElement> {
    let grain = cx
        .try_global::<Grain>()
        .filter(|grain| grain.shown && grain.strength > 0.)?;
    let inner_radius = f32::from(RADIUS_3) - 1.;
    if let Some((_, picture)) = &grain.picture {
        let band = match grain.look.full {
            true => relative(1.),
            false => relative(PICTURE_BAND),
        };
        return Some(
            picture_canvas(grain, picture.clone(), inner_radius, true, cx)
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(band)
                .into_any_element(),
        );
    }
    if grain.wanted.is_some() {
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

/// The pane picture as the preferences draw it now, for Settings to show beside its controls,
/// or nothing while no picture is read.
pub fn picture_preview(cx: &App) -> Option<impl IntoElement + Styled> {
    let grain = cx.try_global::<Grain>()?;
    let (_, picture) = grain.picture.as_ref()?;
    Some(picture_canvas(
        grain,
        picture.clone(),
        f32::from(RADIUS_3),
        false,
        cx,
    ))
}

fn picture_canvas(
    grain: &Grain,
    picture: Arc<RenderImage>,
    radius: f32,
    ground: bool,
    cx: &App,
) -> impl IntoElement + Styled {
    let mut params = grain.look.params(cx);
    params[2] = (PICTURE_OPACITY * grain.strength).min(1.);
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
                    window.paint_shader(bounds, &PICTURE, params, Some(&picture))
                }
            };
            if let Err(error) = painted {
                eprintln!("sikemux: the pane picture does not draw: {error}");
            }
        },
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
/// the power allows, which `on_battery` answers from the view.
pub fn animate<V: 'static>(window: &mut Window, cx: &mut Context<V>, on_battery: fn(&V) -> bool) {
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

fn tick<V: 'static>(
    window: &Window,
    cx: &mut Context<V>,
    on_battery: fn(&V) -> bool,
) -> Option<Task<()>> {
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

/// Takes the system's ask for less motion again. macOS changes it in System Settings, so reading
/// it each time the window comes back to the front follows it; Linux also says when it changes.
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

#[cfg(target_os = "linux")]
fn system_reduces_motion() -> bool {
    linux::reduces_motion()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_reduces_motion() -> bool {
    false
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
