use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(feature = "drive")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use gpui::{
    canvas, point, px, size, App, AppContext, Bounds, Context, DispatchPhase, Entity, Global,
    IntoElement, MouseMoveEvent, ParentElement, Pixels, Point, RawShaderProgram, Render, Rgba,
    Styled, Window,
};

use crate::theme::{self, Tokens};

macro_rules! program {
    ($name:literal) => {
        RawShaderProgram {
            name: $name,
            wgsl: include_str!(concat!(env!("OUT_DIR"), "/shaders/", $name, ".wgsl")),
            msl: include_str!(concat!(env!("OUT_DIR"), "/shaders/", $name, ".metal")),
        }
    };
}

static WELCOME_GLOW: RawShaderProgram = program!("welcome_glow");
static WELCOME_DOT: RawShaderProgram = program!("welcome_dot");
static MARK_PNG: &[u8] = include_bytes!("../../../brand/mark/mark-white-256.png");

const MOST_IN_MARK: usize = 18_000;
const MOST_IN_FIELD: usize = 24_000;
const FIELD_AREA_EACH: f32 = 96.0;
const PLAYS_FOR: f32 = 12.0;
const PLAYS_ON_AFTER_POINTER: f32 = 5.0;
const POINTER_FELT_FOR: f32 = 1.2;
const SETTLES_FOR: f32 = 1.8;
const MAX_COAST_AFTER_UNTIL: f32 = 3.0;
const STILL_TIME: f32 = 9.0;
const CURL_GRID_W: usize = 32;
const CURL_GRID_H: usize = 24;
const FIELD_GRID_W: usize = 48;
const FIELD_GRID_H: usize = 36;

#[cfg(feature = "drive")]
static WELCOME_PRESENTS: AtomicU64 = AtomicU64::new(0);
static WELCOME_AWAKE: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "drive")]
pub fn drive_presents() -> u64 {
    WELCOME_PRESENTS.load(Ordering::Relaxed)
}

#[cfg(feature = "drive")]
pub fn drive_awake() -> bool {
    WELCOME_AWAKE.load(Ordering::Relaxed)
}

#[derive(Default)]
struct WelcomeZones {
    marks: Vec<Bounds<Pixels>>,
    quiets: Vec<Bounds<Pixels>>,
}

thread_local! {
    static WELCOME_ZONES: RefCell<WelcomeZones> = RefCell::new(WelcomeZones::default());
}

pub fn stage_begin() {
    WELCOME_ZONES.with_borrow_mut(|zones| {
        zones.marks.clear();
        zones.quiets.clear();
    });
}

pub fn note_mark(bounds: Bounds<Pixels>) {
    WELCOME_ZONES.with_borrow_mut(|zones| {
        if zones.marks.len() < 8 {
            zones.marks.push(bounds);
        }
    });
}

pub fn note_quiet(bounds: Bounds<Pixels>) {
    WELCOME_ZONES.with_borrow_mut(|zones| {
        if zones.quiets.len() < 8 {
            zones.quiets.push(bounds);
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub tint: [f32; 3],
    pub deep: [f32; 3],
    pub alt: [f32; 3],
    pub bright: [f32; 3],
    pub dark: bool,
}

impl Palette {
    pub fn from_app(cx: &App) -> Self {
        let t = Tokens::get(cx);
        let mut tint = t.accent.to_rgb();
        let low = tint.r.min(tint.g).min(tint.b);
        let high = tint.r.max(tint.g).max(tint.b);
        let mut alt = theme::color(&t.highlight.function).to_rgb();
        let alt_diff = (alt.r - tint.r).abs() + (alt.g - tint.g).abs() + (alt.b - tint.b).abs();
        if alt_diff < 0.14 {
            let turned = mix_rgb(
                tint,
                Rgba {
                    r: tint.b,
                    g: tint.r,
                    b: tint.g,
                    a: 1.0,
                },
                0.38,
            );
            let t_low = turned.r.min(turned.g).min(turned.b);
            let t_high = turned.r.max(turned.g).max(turned.b);
            if t_high - t_low > 0.02 {
                let remap = |c: f32| low + ((c - t_low) / (t_high - t_low)) * (high - low);
                alt = Rgba {
                    r: remap(turned.r),
                    g: remap(turned.g),
                    b: remap(turned.b),
                    a: 1.0,
                };
            } else {
                alt = theme::color(&t.highlight.string).to_rgb();
            }
        }
        let (deep, bright) = if t.dark {
            if high - low < 0.14 {
                tint = mix_rgb(
                    tint,
                    Rgba {
                        r: 0.635,
                        g: 0.467,
                        b: 1.0,
                        a: 1.0,
                    },
                    0.4,
                );
            }
            let deep = Rgba {
                r: tint.r * tint.r * 0.6,
                g: tint.g * tint.g * 0.6,
                b: tint.b * tint.b * 0.6,
                a: 1.0,
            };
            let bright = mix_rgb(
                tint,
                Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                },
                0.86,
            );
            (deep, bright)
        } else {
            let ink = t.text_primary.to_rgb();
            let deep = mix_rgb(tint, ink, 0.42);
            let bright = mix_rgb(
                tint,
                Rgba {
                    r: tint.r * 0.85,
                    g: tint.g * 0.85,
                    b: tint.b * 0.85,
                    a: 1.0,
                },
                0.25,
            );
            (deep, bright)
        };
        Self {
            tint: [tint.r, tint.g, tint.b],
            deep: [deep.r, deep.g, deep.b],
            alt: [alt.r, alt.g, alt.b],
            bright: [bright.r, bright.g, bright.b],
            dark: t.dark,
        }
    }
}

fn mix_rgb(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn toward(value: f32, target: f32, rate: f32, seconds: f32) -> f32 {
    value + (target - value) * (1.0 - (-seconds * rate).exp())
}

#[inline]
fn toward3(value: [f32; 3], target: [f32; 3], rate: f32, seconds: f32) -> [f32; 3] {
    let k = 1.0 - (-seconds * rate).exp();
    [
        value[0] + (target[0] - value[0]) * k,
        value[1] + (target[1] - value[1]) * k,
        value[2] + (target[2] - value[2]) * k,
    ]
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn hash2(px: f32, py: f32) -> f32 {
    let mut x = (px * 123.34).fract();
    let mut y = (py * 456.21).fract();
    if x < 0.0 {
        x += 1.0;
    }
    if y < 0.0 {
        y += 1.0;
    }
    let d = x * (x + 45.32) + y * (y + 45.32);
    x += d;
    y += d;
    let p = (x * y).fract();
    if p < 0.0 {
        p + 1.0
    } else {
        p
    }
}

#[inline]
fn noise2(px: f32, py: f32) -> f32 {
    let ix = px.floor();
    let iy = py.floor();
    let fx0 = px - ix;
    let fy0 = py - iy;
    let fx = fx0 * fx0 * (3.0 - 2.0 * fx0);
    let fy = fy0 * fy0 * (3.0 - 2.0 * fy0);
    let a = hash2(ix, iy);
    let b = hash2(ix + 1.0, iy);
    let c = hash2(ix, iy + 1.0);
    let d = hash2(ix + 1.0, iy + 1.0);
    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    ab + (cd - ab) * fy
}

#[inline]
fn stream(px: f32, py: f32, t: f32) -> f32 {
    noise2(px + t, py - t * 0.7) + 0.5 * noise2(px * 2.1 - t * 1.3, py * 2.1 - t * 0.4)
}

#[inline]
fn curl(px: f32, py: f32, t: f32) -> (f32, f32) {
    const E: f32 = 0.08;
    let inv = 1.0 / (2.0 * E);
    let dx = (stream(px, py + E, t) - stream(px, py - E, t)) * inv;
    let dy = (stream(px - E, py, t) - stream(px + E, py, t)) * inv;
    (dx, dy)
}

#[inline]
fn pack_rgb(rgb: [f32; 3]) -> f32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round();
    byte(rgb[0]) * 65536.0 + byte(rgb[1]) * 256.0 + byte(rgb[2])
}

struct Rolls(u32);

impl Rolls {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut value = (self.0 ^ (self.0 >> 15)).wrapping_mul(1 | self.0);
        value = value.wrapping_add((value ^ (value >> 7)).wrapping_mul(61 | value)) ^ value;
        (value ^ (value >> 14)) as f32 / 4_294_967_296.0
    }
}

#[derive(Clone, Copy, Default)]
struct ParticleState {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

struct FieldSpec {
    hx: f32,
    hy: f32,
    cx: f32,
    cy: f32,
    turn_cos: f32,
    turn_sin: f32,
    felt: f32,
    inv_felt2: f32,
    spring: f32,
    drag: f32,
    fling_mul: f32,
    radial_mul: f32,
    cap_own: f32,
    spin_base: f32,
    cut: f32,
    base_radius: f32,
    is_bokeh: bool,
    bokeh_radius: f32,
    bokeh_alpha: f32,
    sparkle_step: f32,
    shade_mul: f32,
    float_sin_x: f32,
    float_cos_x: f32,
    float_sin_y: f32,
    float_cos_y: f32,
}

fn field_specs() -> &'static [FieldSpec] {
    static SPECS: OnceLock<Vec<FieldSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let mut roll_home = Rolls::new(3);
        let mut roll_chance = Rolls::new(3);
        (0..MOST_IN_FIELD)
            .map(|at| {
                let f = at as f32;
                let hx = (0.5 + f * 0.754_877_7 + roll_home.next() * 0.004) % 1.0;
                let hy = (0.5 + f * 0.569_840_3 + roll_home.next() * 0.004) % 1.0;
                let cx = roll_chance.next();
                let cy = roll_chance.next();
                let own = hash2(cx + 0.71, cy + 0.71);
                let turn = (hash2(cx * 3.1 + 0.13, cy * 3.1 + 0.13) - 0.5) * 3.0;
                let felt = 64.0 + (134.0 - 64.0) * cy;
                let spring = 5.0 + (11.0 - 5.0) * cx;
                let drag = 1.2 * spring.sqrt();
                let cut = 0.06 + 0.9 * hash2(cx * 13.7, cy * 13.7);
                let cy2 = cy * cy;
                let cy5 = cy2 * cy2 * cy;
                let (float_sin_x, float_cos_x) = (cx * 44.0).sin_cos();
                let (float_sin_y, float_cos_y) = (cy * 51.0).sin_cos();
                FieldSpec {
                    hx,
                    hy,
                    cx,
                    cy,
                    turn_cos: turn.cos(),
                    turn_sin: turn.sin(),
                    felt,
                    inv_felt2: 1.0 / (felt * felt),
                    spring,
                    drag,
                    fling_mul: 1.3 + (11.5 - 1.3) * own * own,
                    radial_mul: -1.0 + 6.0 * cx,
                    cap_own: 620.0 * (0.2 + 0.8 * own),
                    spin_base: (cx - 0.5) * 14.0,
                    cut,
                    base_radius: 0.48 + 0.75 * cy5,
                    is_bokeh: cx > 0.982,
                    bokeh_radius: 2.4 + (8.5 - 2.4) * cy2,
                    bokeh_alpha: 0.12 + (0.05 - 0.12) * cy,
                    sparkle_step: if cx >= 0.965 { 0.5 } else { 0.0 },
                    shade_mul: 0.78 + 0.4 * cx,
                    float_sin_x,
                    float_cos_x,
                    float_sin_y,
                    float_cos_y,
                }
            })
            .collect()
    })
}

struct MarkGrain {
    u: f32,
    v: f32,
    radius: f32,
    light: f32,
    lit: f32,
    rim: f32,
    cx: f32,
    cy: f32,
    own: f32,
    turn_cos: f32,
    turn_sin: f32,
    spring: f32,
    drag: f32,
    fling_mul: f32,
    radial_mul: f32,
    spin_base: f32,
}

fn mark_grains() -> &'static [MarkGrain] {
    static GRAINS: OnceLock<Vec<MarkGrain>> = OnceLock::new();
    GRAINS.get_or_init(|| {
        let Ok(decoded) = image::load_from_memory(MARK_PNG) else {
            return Vec::new();
        };
        let rgba = decoded.into_rgba8();
        let (src_w, src_h) = rgba.dimensions();
        let (mut left, mut top, mut right, mut bottom) = (src_w, src_h, 0u32, 0u32);
        for y in 0..src_h {
            for x in 0..src_w {
                if rgba.get_pixel(x, y).0[3] >= 128 {
                    left = left.min(x);
                    right = right.max(x + 1);
                    top = top.min(y);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        if right <= left || bottom <= top {
            return Vec::new();
        }

        // Sized for the 116pt welcome mark so each individual particle reads crisply.
        let size_ref = 136.0f32;
        let side = (size_ref * 1.6).round() as usize;
        let scale = (side as f32 * 0.8) / (right - left).max(bottom - top) as f32;
        let cx_src = (left + right) as f32 * 0.5;
        let cy_src = (top + bottom) as f32 * 0.5;
        let half = side as f32 * 0.5;

        let mut alpha = vec![0u8; side * side];
        for (y, row) in alpha.chunks_mut(side).enumerate() {
            let sy = ((y as f32 + 0.5 - half) / scale + cy_src).floor() as i32;
            if sy < 0 || sy >= src_h as i32 {
                continue;
            }
            for (x, slot) in row.iter_mut().enumerate() {
                let sx = ((x as f32 + 0.5 - half) / scale + cx_src).floor() as i32;
                if sx < 0 || sx >= src_w as i32 {
                    continue;
                }
                *slot = rgba.get_pixel(sx as u32, sy as u32).0[3];
            }
        }

        let covered = |x: i32, y: i32| -> bool {
            if x < 0 || y < 0 || (x as usize) >= side || (y as usize) >= side {
                return false;
            }
            alpha
                .get(y as usize * side + x as usize)
                .is_some_and(|&a| a >= 128)
        };

        let mut outline = Vec::new();
        let mut inside = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let ix = x as i32;
                let iy = y as i32;
                if !covered(ix, iy) {
                    continue;
                }
                let idx = y * side + x;
                inside.push(idx);
                if !covered(ix - 1, iy)
                    || !covered(ix + 1, iy)
                    || !covered(ix, iy - 1)
                    || !covered(ix, iy + 1)
                {
                    outline.push(idx);
                }
            }
        }
        if inside.is_empty() {
            return Vec::new();
        }

        let mut roll = Rolls::new(7);
        for last in (1..outline.len()).rev() {
            let other = (roll.next() * (last + 1) as f32) as usize;
            outline.swap(last, other.min(last));
        }

        let mut gap = (1.42f32.max(size_ref / 300.0) * side as f32) / size_ref;
        let wanted = (inside.len() as f32 / (gap * gap)) * 0.62;
        let room = MOST_IN_MARK as f32 * 0.92;
        if wanted > room {
            gap *= (wanted / room).sqrt();
        }
        let cell = gap * 0.8;
        let across = (side as f32 / cell).ceil() as usize;
        let mut taken = vec![-1i32; across * across];

        struct RawGrain {
            left: f32,
            top: f32,
            u: f32,
            v: f32,
            radius: f32,
            light: f32,
            lit: f32,
            rim: f32,
        }
        let mut raw: Vec<RawGrain> = Vec::with_capacity(4096);

        let mut drop_at = |at: usize, apart: f32, edge: bool, roll: &mut Rolls| {
            let x = (at % side) as f32 + if edge { 0.5 } else { roll.next() };
            let y = (at / side) as f32 + if edge { 0.5 } else { roll.next() };
            let col = ((x / cell) as usize).min(across.saturating_sub(1));
            let row = ((y / cell) as usize).min(across.saturating_sub(1));
            if taken.get(row * across + col).is_none_or(|&t| t >= 0) {
                return;
            }
            let reach = (apart / cell).ceil() as usize;
            let r0 = row.saturating_sub(reach);
            let r1 = (row + reach).min(across.saturating_sub(1));
            let c0 = col.saturating_sub(reach);
            let c1 = (col + reach).min(across.saturating_sub(1));
            let apart2 = apart * apart;
            for b in r0..=r1 {
                for a in c0..=c1 {
                    if let Some(&idx) = taken.get(b * across + a) {
                        if let Some(other) = usize::try_from(idx).ok().and_then(|u| raw.get(u)) {
                            let dx = other.left - x;
                            let dy = other.top - y;
                            if dx * dx + dy * dy < apart2 {
                                return;
                            }
                        }
                    }
                }
            }
            if let Some(slot) = taken.get_mut(row * across + col) {
                *slot = raw.len() as i32;
            }
            let u = x / side as f32 - 0.5;
            let v = y / side as f32 - 0.5;
            let light = (1.16 - 0.5 * (0.42 + u * 0.55 + v * 0.95).max(0.0)
                + (roll.next() - 0.5) * 0.3)
                .max(0.55);
            let r_roll = roll.next();
            let radius = (if edge { 0.86 } else { 1.0 }) * (0.62 + 0.5 * r_roll * r_roll);
            let lit = (0.52 - u * 0.62 - v * 1.02 + (roll.next() - 0.5) * 0.26).clamp(0.0, 1.0);
            let rim = ((u + v) * 0.9).max(0.0) * if edge { 1.25 } else { 0.45 };
            raw.push(RawGrain {
                left: x,
                top: y,
                u,
                v,
                radius,
                light,
                lit,
                rim,
            });
        };

        for &at in &outline {
            drop_at(at, gap * 0.82, true, &mut roll);
        }
        let tries = (inside.len() as f32 * 2.2) as usize;
        for _ in 0..tries {
            let pick = (roll.next() * inside.len() as f32) as usize % inside.len();
            let Some(&at) = inside.get(pick) else {
                continue;
            };
            let u = (at % side) as f32 / side as f32 - 0.5;
            let v = (at / side) as f32 / side as f32 - 0.5;
            let shade = (0.42 + u * 0.55 + v * 0.95).clamp(0.0, 1.0);
            let jitter = roll.next();
            drop_at(
                at,
                gap * (0.74 + 1.2 * shade * shade + 0.22 * jitter),
                false,
                &mut roll,
            );
        }

        raw.sort_by(|a, b| {
            let key = |g: &RawGrain| g.v.atan2(g.u) + (g.u * 91.0 + g.v * 57.0).sin() * 0.5;
            key(a)
                .partial_cmp(&key(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        raw.truncate(MOST_IN_MARK);

        let mut chance_roll = Rolls::new(3);
        raw.into_iter()
            .map(|g| {
                let cx = chance_roll.next();
                let cy = chance_roll.next();
                let own = hash2(cx + 0.71, cy + 0.71);
                let turn = (hash2(cx * 3.1 + 0.13, cy * 3.1 + 0.13) - 0.5) * 3.0;
                let spring = 16.0 + (34.0 - 16.0) * cx;
                let drag = 1.2 * spring.sqrt();
                MarkGrain {
                    u: g.u,
                    v: g.v,
                    radius: g.radius,
                    light: g.light,
                    lit: g.lit,
                    rim: g.rim,
                    cx,
                    cy,
                    own,
                    turn_cos: turn.cos(),
                    turn_sin: turn.sin(),
                    spring,
                    drag,
                    fling_mul: 1.3 + (11.5 - 1.3) * own * own,
                    radial_mul: -1.0 + 6.0 * cx,
                    spin_base: (cx - 0.5) * 14.0,
                }
            })
            .collect()
    })
}

#[inline]
fn ink_of(lit: f32, rim: f32, mood: &Mood) -> [f32; 3] {
    let shade = if lit > 0.45 {
        mix3(mood.tint, mood.bright, (lit - 0.45) / 0.55)
    } else {
        mix3(mix3(mood.deep, mood.tint, 0.3), mood.tint, lit / 0.45)
    };
    mix3(shade, mood.alt, rim.min(0.85))
}

#[derive(Clone, Copy)]
struct Mood {
    tint: [f32; 3],
    deep: [f32; 3],
    alt: [f32; 3],
    bright: [f32; 3],
}

impl Mood {
    fn from_palette(p: Palette) -> Self {
        Self {
            tint: p.tint,
            deep: p.deep,
            alt: p.alt,
            bright: p.bright,
        }
    }

    fn ease_toward(&mut self, goal: Palette, dt: f32) {
        self.tint = toward3(self.tint, goal.tint, 3.2, dt);
        self.deep = toward3(self.deep, goal.deep, 3.2, dt);
        self.alt = toward3(self.alt, goal.alt, 3.2, dt);
        self.bright = toward3(self.bright, goal.bright, 3.2, dt);
    }
}

struct PointerState {
    at: [f32; 2],
    before: [f32; 2],
    sweep: [f32; 2],
    seen: f32,
    here: bool,
    moved: f32,
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            at: [0.0, 0.0],
            before: [0.0, 0.0],
            sweep: [0.0, 0.0],
            seen: 0.0,
            here: false,
            moved: -1e6,
        }
    }
}

impl PointerState {
    fn move_to(&mut self, pos: [f32; 2], now: f32) {
        if self.here && (now - self.moved) < 0.2 {
            let seconds = (now - self.moved).max(0.004);
            let vx = (pos[0] - self.at[0]) / seconds;
            let vy = (pos[1] - self.at[1]) / seconds;
            let speed = vx.hypot(vy).max(1.0);
            let most = (1500.0 / speed).min(1.0);
            self.sweep[0] = (self.sweep[0] + vx * most) * 0.5;
            self.sweep[1] = (self.sweep[1] + vy * most) * 0.5;
        } else {
            self.before = pos;
        }
        self.at = pos;
        self.here = true;
        self.moved = now;
    }

    fn pass(&mut self, now: f32, seconds: f32) {
        let felt = self.here && (now - self.moved) < POINTER_FELT_FOR;
        let target = if felt { 1.0 } else { 0.0 };
        let rate = if felt { 24.0 } else { 4.0 };
        self.seen = toward(self.seen, target, rate, seconds);
        let decay = (-seconds * 9.0).exp();
        self.sweep[0] *= decay;
        self.sweep[1] *= decay;
    }
}

#[derive(Clone, Copy, Default)]
struct FieldSample {
    cloud: f32,
    warmth: f32,
    other: f32,
}

struct SimGrids {
    curl: Vec<(f32, f32)>,
    field: Vec<FieldSample>,
}

impl SimGrids {
    fn new() -> Self {
        Self {
            curl: vec![(0.0, 0.0); CURL_GRID_W * CURL_GRID_H],
            field: vec![FieldSample::default(); FIELD_GRID_W * FIELD_GRID_H],
        }
    }

    fn update(&mut self, width: f32, height: f32, t: f32) {
        let cw_inv = width / (CURL_GRID_W - 1) as f32;
        let ch_inv = height / (CURL_GRID_H - 1) as f32;
        let ct = t * 0.035;
        for (gy, row) in self.curl.chunks_mut(CURL_GRID_W).enumerate() {
            let py = gy as f32 * ch_inv / 230.0;
            for (gx, cell) in row.iter_mut().enumerate() {
                let px = gx as f32 * cw_inv / 230.0;
                let (dx, dy) = curl(px, py, ct);
                *cell = (dx * 14.0, dy * 14.0);
            }
        }

        let fw_inv = width / (FIELD_GRID_W - 1) as f32;
        let fh_inv = height / (FIELD_GRID_H - 1) as f32;
        for (gy, row) in self.field.chunks_mut(FIELD_GRID_W).enumerate() {
            let py = gy as f32 * fh_inv;
            for (gx, cell) in row.iter_mut().enumerate() {
                let px = gx as f32 * fw_inv;
                let dx = px / 150.0;
                let dy = py / 150.0;
                let wx = noise2(dx * 0.7 + t * 0.02, dy * 0.7 + t * 0.02);
                let wy = noise2(dx * 0.7 + 9.0 - t * 0.02, dy * 0.7 + 9.0 - t * 0.02);
                *cell = FieldSample {
                    cloud: noise2(dx + t * 0.025 + 2.0 * wx, dy - t * 0.015 + 2.0 * wy),
                    warmth: noise2(px / 330.0 + t * 0.011, py / 330.0 - t * 0.008),
                    other: noise2(px / 210.0 - t * 0.008 + 17.0, py / 210.0 + t * 0.01 + 17.0),
                };
            }
        }
    }

    #[inline]
    fn sample_curl(&self, u: f32, v: f32) -> (f32, f32) {
        let fx = (u * (CURL_GRID_W - 1) as f32).clamp(0.0, (CURL_GRID_W - 1) as f32);
        let fy = (v * (CURL_GRID_H - 1) as f32).clamp(0.0, (CURL_GRID_H - 1) as f32);
        let x0 = (fx as usize).min(CURL_GRID_W - 2);
        let y0 = (fy as usize).min(CURL_GRID_H - 2);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let i00 = y0 * CURL_GRID_W + x0;
        let a = self.curl.get(i00).copied().unwrap_or((0.0, 0.0));
        let b = self.curl.get(i00 + 1).copied().unwrap_or(a);
        let c = self.curl.get(i00 + CURL_GRID_W).copied().unwrap_or(a);
        let d = self.curl.get(i00 + CURL_GRID_W + 1).copied().unwrap_or(c);
        let cx0 = a.0 + (b.0 - a.0) * tx;
        let cx1 = c.0 + (d.0 - c.0) * tx;
        let cy0 = a.1 + (b.1 - a.1) * tx;
        let cy1 = c.1 + (d.1 - c.1) * tx;
        (cx0 + (cx1 - cx0) * ty, cy0 + (cy1 - cy0) * ty)
    }

    #[inline]
    fn sample_field(&self, u: f32, v: f32) -> (f32, f32, f32) {
        let fx = (u * (FIELD_GRID_W - 1) as f32).clamp(0.0, (FIELD_GRID_W - 1) as f32);
        let fy = (v * (FIELD_GRID_H - 1) as f32).clamp(0.0, (FIELD_GRID_H - 1) as f32);
        let x0 = (fx as usize).min(FIELD_GRID_W - 2);
        let y0 = (fy as usize).min(FIELD_GRID_H - 2);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let i00 = y0 * FIELD_GRID_W + x0;
        let a = self.field.get(i00).copied().unwrap_or_default();
        let b = self.field.get(i00 + 1).copied().unwrap_or(a);
        let c = self.field.get(i00 + FIELD_GRID_W).copied().unwrap_or(a);
        let d = self.field.get(i00 + FIELD_GRID_W + 1).copied().unwrap_or(c);
        let bilerp = |va: f32, vb: f32, vc: f32, vd: f32| {
            let top = va + (vb - va) * tx;
            let bot = vc + (vd - vc) * tx;
            top + (bot - top) * ty
        };
        (
            bilerp(a.cloud, b.cloud, c.cloud, d.cloud),
            bilerp(a.warmth, b.warmth, c.warmth, d.warmth),
            bilerp(a.other, b.other, c.other, d.other),
        )
    }
}

#[derive(Clone, Copy)]
struct DotDraw {
    cx: f32,
    cy: f32,
    radius: f32,
    blur: f32,
    rgb: [f32; 3],
    alpha: f32,
}

#[derive(Clone, Copy)]
struct Props {
    palette: Palette,
    glow: f32,
    show_field: bool,
    reduce_motion: bool,
    corner_radius: f32,
}

pub struct WelcomeDots {
    started: Instant,
    props: Props,
    mood: Option<Mood>,
    pointer: PointerState,
    clock: f32,
    life: f32,
    until: f32,
    last_frame: Option<f32>,
    awake: bool,
    dirty: bool,

    pane_w: f32,
    pane_h: f32,
    mark_center: [f32; 2],
    mark_span: f32,
    quiet_rect: Option<[f32; 4]>,

    field_used: usize,
    field_fresh: bool,
    field: Vec<ParticleState>,

    mark_fresh: bool,
    mark: Vec<ParticleState>,

    grids: SimGrids,
    draws: Vec<DotDraw>,
    last_step_us: u64,
    last_collect_us: u64,
    last_emit_us: u64,
}

impl WelcomeDots {
    fn new(props: Props) -> Self {
        let mark_len = mark_grains().len();
        Self {
            started: Instant::now(),
            props,
            mood: None,
            pointer: PointerState::default(),
            clock: 4.0,
            life: 1.0,
            until: if props.reduce_motion { 0.0 } else { PLAYS_FOR },
            last_frame: None,
            awake: !props.reduce_motion,
            dirty: true,
            pane_w: 0.0,
            pane_h: 0.0,
            mark_center: [0.0, 0.0],
            mark_span: 116.0,
            quiet_rect: None,
            field_used: 0,
            field_fresh: true,
            field: Vec::new(),
            mark_fresh: true,
            mark: vec![ParticleState::default(); mark_len],
            grids: SimGrids::new(),
            draws: Vec::with_capacity(6144),
            last_step_us: 0,
            last_collect_us: 0,
            last_emit_us: 0,
        }
    }

    fn now(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    fn sync_props(&mut self, next: Props) -> bool {
        let palette_changed = self.props.palette != next.palette;
        let motion_changed = self.props.reduce_motion != next.reduce_motion;
        let glow_changed = (self.props.glow - next.glow).abs() > 0.001;
        let field_changed = self.props.show_field != next.show_field;
        self.props = next;
        if next.reduce_motion {
            self.awake = false;
            self.last_frame = None;
            self.dirty = true;
            return palette_changed || motion_changed || glow_changed || field_changed;
        }
        if palette_changed || motion_changed || glow_changed || field_changed {
            self.dirty = true;
        }
        if palette_changed || motion_changed || field_changed {
            self.wake_for(PLAYS_FOR);
        }
        palette_changed || motion_changed || glow_changed || field_changed
    }

    fn wake_for(&mut self, seconds: f32) -> bool {
        if self.props.reduce_motion {
            return false;
        }
        let now = self.now();
        self.until = self.until.max(now + seconds);
        let was_asleep = !self.awake;
        if was_asleep {
            self.awake = true;
            self.last_frame = None;
        }
        WELCOME_AWAKE.store(true, Ordering::Relaxed);
        was_asleep
    }

    fn handle_mouse_move(&mut self, pos: Point<Pixels>, bounds: Bounds<Pixels>) -> bool {
        if self.props.reduce_motion || self.props.glow <= 0.001 {
            return false;
        }
        let now = self.now();
        if bounds.contains(&pos) {
            let local = [
                f32::from(pos.x - bounds.origin.x),
                f32::from(pos.y - bounds.origin.y),
            ];
            self.pointer.move_to(local, now);
            self.wake_for(PLAYS_ON_AFTER_POINTER)
        } else if self.pointer.here {
            self.pointer.here = false;
            self.wake_for(PLAYS_ON_AFTER_POINTER)
        } else {
            false
        }
    }

    fn sync_geometry(&mut self, bounds: Bounds<Pixels>) {
        let width = f32::from(bounds.size.width).max(1.0);
        let height = f32::from(bounds.size.height).max(1.0);
        if (self.pane_w - width).abs() > 1.0 || (self.pane_h - height).abs() > 1.0 {
            self.pane_w = width;
            self.pane_h = height;
            self.field_used = ((width * height) / FIELD_AREA_EACH)
                .round()
                .clamp(0.0, MOST_IN_FIELD as f32) as usize;
            if self.field.len() < self.field_used {
                self.field.resize(self.field_used, ParticleState::default());
            }
            self.field_fresh = true;
            self.dirty = true;
        }

        let (mark, quiet) = WELCOME_ZONES.with_borrow(|zones| {
            let mark = zones
                .marks
                .iter()
                .find(|m| bounds.contains(&m.center()))
                .copied();
            let quiet = zones
                .quiets
                .iter()
                .find(|q| bounds.contains(&q.center()))
                .copied();
            (mark, quiet)
        });

        if let Some(mark) = mark {
            let next_center = [
                f32::from(mark.center().x - bounds.origin.x),
                f32::from(mark.center().y - bounds.origin.y),
            ];
            let next_span = f32::from(mark.size.width).max(48.0);
            if (self.mark_center[0] - next_center[0]).abs() > 0.5
                || (self.mark_center[1] - next_center[1]).abs() > 0.5
                || (self.mark_span - next_span).abs() > 0.5
            {
                self.mark_center = next_center;
                self.mark_span = next_span;
                self.dirty = true;
            }
        } else if self.mark_center == [0.0, 0.0] {
            self.mark_center = [width * 0.5, height * 0.34];
            self.mark_span = 116.0;
            self.dirty = true;
        }

        if let Some(quiet) = quiet {
            let quiet_half_w = (width * 0.42).min(330.0);
            let quiet_top = (self.mark_center[1] + self.mark_span * 0.5 + 8.0)
                .min(f32::from(quiet.top() - bounds.origin.y));
            let quiet_bottom = (f32::from(quiet.bottom() - bounds.origin.y) + 152.0).min(height);
            let next_quiet = Some([
                self.mark_center[0] - quiet_half_w,
                quiet_top,
                self.mark_center[0] + quiet_half_w,
                quiet_bottom.max(quiet_top + 48.0),
            ]);
            if self.quiet_rect != next_quiet {
                self.quiet_rect = next_quiet;
                self.dirty = true;
            }
        } else if self.quiet_rect.is_none() {
            let quiet_half_w = (width * 0.42).min(330.0);
            let quiet_top = self.mark_center[1] + self.mark_span * 0.5 + 10.0;
            self.quiet_rect = Some([
                self.mark_center[0] - quiet_half_w,
                quiet_top,
                self.mark_center[0] + quiet_half_w,
                (height * 0.72).max(quiet_top + 48.0),
            ]);
            self.dirty = true;
        }
    }

    fn step_and_collect(&mut self, bounds: Bounds<Pixels>, active: bool) -> (f32, Mood) {
        self.sync_geometry(bounds);

        let still = self.props.reduce_motion;
        if !self.awake && !self.dirty && !self.draws.is_empty() {
            let t = if still { STILL_TIME } else { self.clock };
            let mood = self
                .mood
                .unwrap_or_else(|| Mood::from_palette(self.props.palette));
            self.last_step_us = 0;
            self.last_collect_us = 0;
            return (t, mood);
        }
        self.dirty = false;

        let now = self.now();
        let dt = if still {
            0.001
        } else {
            let raw = match self.last_frame {
                Some(prev) => now - prev,
                None => 1.0 / 120.0,
            };
            self.last_frame = Some(now);
            raw.clamp(0.001, 0.05)
        };

        if !still {
            let want_life = if (self.until - now) > SETTLES_FOR {
                1.0
            } else {
                0.0
            };
            self.life = toward(self.life, want_life, 2.2, dt);
            self.clock += dt * self.life;
            self.pointer.pass(now, dt);
        } else {
            self.pointer.seen = 0.0;
            self.pointer.sweep = [0.0, 0.0];
        }

        let mood = match self.mood.as_mut() {
            Some(mood) => {
                if still {
                    *mood = Mood::from_palette(self.props.palette);
                } else {
                    mood.ease_toward(self.props.palette, dt);
                }
                *mood
            }
            None => {
                let mood = Mood::from_palette(self.props.palette);
                self.mood = Some(mood);
                mood
            }
        };

        let t = if still { STILL_TIME } else { self.clock };
        let steps = if !still && dt > 0.022 { 2 } else { 1 };
        let step_dt = dt / steps as f32;

        let t0 = Instant::now();
        if self.props.show_field {
            self.grids.update(self.pane_w, self.pane_h, t);
        }

        let mut max_motion = 0.0f32;
        if self.props.show_field {
            for _ in 0..steps {
                let m = self.step_field(t, still, step_dt);
                max_motion = max_motion.max(m);
            }
            self.field_fresh = false;
        }
        for _ in 0..steps {
            let m = self.step_mark(t, still, step_dt);
            max_motion = max_motion.max(m);
        }
        self.mark_fresh = false;
        self.pointer.before = self.pointer.at;
        self.last_step_us = t0.elapsed().as_micros() as u64;

        if !still {
            let past_until = now >= self.until;
            let settled = (self.pointer.seen < 0.002 && max_motion < 0.08)
                || now >= self.until + MAX_COAST_AFTER_UNTIL;
            if !active || (past_until && settled) {
                self.awake = false;
                self.last_frame = None;
            }
        }

        let t1 = Instant::now();
        self.draws.clear();
        if self.props.show_field {
            self.collect_field_draws(t, still, &mood);
        }
        self.collect_mark_draws(t, still, &mood);
        self.last_collect_us = t1.elapsed().as_micros() as u64;

        (t, mood)
    }

    fn step_field(&mut self, t: f32, still: bool, seconds: f32) -> f32 {
        let specs = field_specs();
        let n = self.field_used;
        let w = self.pane_w;
        let h = self.pane_h;
        let reset = still || self.field_fresh;

        let ptr = self.pointer.at;
        let path_x = self.pointer.before[0] - ptr[0];
        let path_y = self.pointer.before[1] - ptr[1];
        let path_len2 = (path_x * path_x + path_y * path_y).max(1.0);
        let sweep_x = self.pointer.sweep[0];
        let sweep_y = self.pointer.sweep[1];
        let quick = sweep_x.hypot(sweep_y);
        let seen = self.pointer.seen;
        let ptr_active = !reset && seen > 0.0005;
        let min_px = ptr[0].min(self.pointer.before[0]) - 420.0;
        let max_px = ptr[0].max(self.pointer.before[0]) + 420.0;
        let min_py = ptr[1].min(self.pointer.before[1]) - 420.0;
        let max_py = ptr[1].max(self.pointer.before[1]) + 420.0;
        let quick_scale = (0.3 + quick / 950.0).min(1.0);
        let (sin_f1, cos_f1) = (t * 0.2).sin_cos();
        let (sin_f2, cos_f2) = (t * 0.17).sin_cos();
        let t_spin = t * 1.7;

        let mut max_motion = 0.0f32;

        for (spec, p) in specs.iter().zip(self.field.iter_mut()).take(n) {
            let (curl_x, curl_y) = self.grids.sample_curl(spec.hx, spec.hy);
            let float_x = (sin_f1 * spec.float_cos_x + cos_f1 * spec.float_sin_x) * 4.0;
            let float_y = (cos_f2 * spec.float_cos_y - sin_f2 * spec.float_sin_y) * 4.0;
            let mut bx = spec.hx * w + curl_x + float_x;
            let mut by = spec.hy * h + curl_y + float_y;

            if reset {
                *p = ParticleState {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                };
                continue;
            }

            let mut x = p.x;
            let mut y = p.y;
            let mut vx = p.vx;
            let mut vy = p.vy;

            let mut touched = false;
            if ptr_active && x >= min_px && x <= max_px && y >= min_py && y <= max_py {
                let rel_x = x - ptr[0];
                let rel_y = y - ptr[1];
                let along = ((rel_x * path_x + rel_y * path_y) / path_len2).clamp(0.0, 1.0);
                let off_x = rel_x - path_x * along;
                let off_y = rel_y - path_y * along;
                let far2 = off_x * off_x + off_y * off_y;
                let exp_arg = far2 * spec.inv_felt2;
                if exp_arg < 8.5 {
                    touched = true;
                    let near = seen * (-exp_arg).exp();
                    let far = far2.sqrt() + 0.01;
                    let dir_x = off_x / far;
                    let dir_y = off_y / far;
                    let rot_sx = sweep_x * spec.turn_cos - sweep_y * spec.turn_sin;
                    let rot_sy = sweep_x * spec.turn_sin + sweep_y * spec.turn_cos;
                    let fling_x = rot_sx * spec.fling_mul + dir_x * quick * spec.radial_mul;
                    let fling_y = rot_sy * spec.fling_mul + dir_y * quick * spec.radial_mul;

                    let was = vx.hypot(vy);
                    let cap = was.max(spec.cap_own * quick_scale);
                    let disp = (x - bx).hypot(y - by);
                    let leash_mul = (1.0 - disp / 200.0).max(0.0);
                    let impulse = near * seconds * leash_mul;
                    vx += fling_x * impulse;
                    vy += fling_y * impulse;
                    let new_speed = vx.hypot(vy);
                    if new_speed > cap && new_speed > 1.0 {
                        let s = cap / new_speed;
                        vx *= s;
                        vy *= s;
                    }
                    let push = near * spec.felt * 0.38;
                    bx += dir_x * push;
                    by += dir_y * push;
                }
            }

            let dx = bx - x;
            let dy = by - y;
            let v2 = vx * vx + vy * vy;
            if !touched && v2 < 1e-3 && dx * dx + dy * dy < 0.002 {
                *p = ParticleState {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                };
                continue;
            }

            if v2 > 0.04 {
                let spin = seconds * (spec.spin_base + 5.0 * (x / 37.0 + y / 53.0 + t_spin).sin());
                let (ss, sc) = spin.sin_cos();
                let nvx = vx * sc - vy * ss;
                let nvy = vx * ss + vy * sc;
                vx = nvx;
                vy = nvy;
            }

            let spring_dt = spec.spring * seconds;
            let inv_drag = 1.0 / (1.0 + spec.drag * seconds);
            vx = (vx + dx * spring_dt) * inv_drag;
            vy = (vy + dy * spring_dt) * inv_drag;
            x += vx * seconds;
            y += vy * seconds;

            *p = ParticleState { x, y, vx, vy };

            let motion = (x - bx).abs() + (y - by).abs() + (vx.abs() + vy.abs()) * 0.08;
            if motion > max_motion {
                max_motion = motion;
            }
        }

        max_motion
    }

    fn step_mark(&mut self, t: f32, still: bool, seconds: f32) -> f32 {
        let grains = mark_grains();
        if grains.is_empty() {
            return 0.0;
        }
        let span = self.mark_span;
        let center = self.mark_center;
        let reset = still || self.mark_fresh;

        let ptr = self.pointer.at;
        let path_x = self.pointer.before[0] - ptr[0];
        let path_y = self.pointer.before[1] - ptr[1];
        let path_len2 = (path_x * path_x + path_y * path_y).max(1.0);
        let sweep_x = self.pointer.sweep[0];
        let sweep_y = self.pointer.sweep[1];
        let quick = sweep_x.hypot(sweep_y);
        let seen = self.pointer.seen;
        let ptr_active = !reset && seen > 0.0005;
        let felt_base = (span * 0.42).clamp(36.0, 98.0);
        let most = (span * 3.8).clamp(320.0, 640.0);
        let leash = (span * 0.56).clamp(50.0, 148.0);
        let quick_scale = (0.3 + quick / 950.0).min(1.0);
        let breathe_mul = if still { 0.0 } else { 0.5 };
        let t_curl = t * 0.05;
        let t_spin = t * 1.7;
        let disperse_amt = 1.0 - self.props.glow;

        let mut max_motion = 0.0f32;

        for (g, p) in grains.iter().zip(self.mark.iter_mut()) {
            let ax = g.u * span;
            let ay = g.v * span;
            let (cx_off, cy_off) = if breathe_mul > 0.0 {
                let (dx, dy) = curl(ax / 60.0, ay / 60.0, t_curl);
                (dx * breathe_mul, dy * breathe_mul)
            } else {
                (0.0, 0.0)
            };
            let mut bx = ax + cx_off;
            let mut by = ay + cy_off;
            if disperse_amt > 0.001 {
                let len = ax.hypot(ay).max(1.0);
                let fling = disperse_amt * (16.0 + 28.0 * g.cx);
                bx += (ax / len) * fling;
                by += (ay / len) * fling;
            }

            if reset {
                *p = ParticleState {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                };
                continue;
            }

            let mut x = p.x;
            let mut y = p.y;
            let mut vx = p.vx;
            let mut vy = p.vy;
            let page_x = center[0] + x;
            let page_y = center[1] + y;

            let mut touched = false;
            if ptr_active {
                let rel_x = page_x - ptr[0];
                let rel_y = page_y - ptr[1];
                let along = ((rel_x * path_x + rel_y * path_y) / path_len2).clamp(0.0, 1.0);
                let off_x = rel_x - path_x * along;
                let off_y = rel_y - path_y * along;
                let felt = felt_base * (0.8 + 0.3 * g.cy);
                let far2 = off_x * off_x + off_y * off_y;
                let exp_arg = far2 / (felt * felt);
                if exp_arg < 8.5 {
                    touched = true;
                    let near = seen * (-exp_arg).exp();
                    let far = far2.sqrt() + 0.01;
                    let dir_x = off_x / far;
                    let dir_y = off_y / far;
                    let rot_sx = sweep_x * g.turn_cos - sweep_y * g.turn_sin;
                    let rot_sy = sweep_x * g.turn_sin + sweep_y * g.turn_cos;
                    let fling_x = rot_sx * g.fling_mul + dir_x * quick * g.radial_mul;
                    let fling_y = rot_sy * g.fling_mul + dir_y * quick * g.radial_mul;

                    let was = vx.hypot(vy);
                    let cap = was.max(most * (0.2 + 0.8 * g.own) * quick_scale);
                    let disp = (x - bx).hypot(y - by);
                    let leash_mul = (1.0 - disp / leash).max(0.0);
                    let impulse = near * seconds * leash_mul;
                    vx += fling_x * impulse;
                    vy += fling_y * impulse;
                    let new_speed = vx.hypot(vy);
                    if new_speed > cap && new_speed > 1.0 {
                        let s = cap / new_speed;
                        vx *= s;
                        vy *= s;
                    }
                    let push = near * felt * 0.26;
                    bx += dir_x * push;
                    by += dir_y * push;
                }
            }

            let dx = bx - x;
            let dy = by - y;
            let v2 = vx * vx + vy * vy;
            if !touched && v2 < 1e-3 && dx * dx + dy * dy < 0.002 {
                *p = ParticleState {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                };
                continue;
            }

            if v2 > 0.04 {
                let spin =
                    seconds * (g.spin_base + 5.0 * (page_x / 37.0 + page_y / 53.0 + t_spin).sin());
                let (ss, sc) = spin.sin_cos();
                let nvx = vx * sc - vy * ss;
                let nvy = vx * ss + vy * sc;
                vx = nvx;
                vy = nvy;
            }

            let spring_dt = g.spring * seconds;
            let inv_drag = 1.0 / (1.0 + g.drag * seconds);
            vx = (vx + dx * spring_dt) * inv_drag;
            vy = (vy + dy * spring_dt) * inv_drag;
            x += vx * seconds;
            y += vy * seconds;

            *p = ParticleState { x, y, vx, vy };

            let motion = (x - bx).abs() + (y - by).abs() + (vx.abs() + vy.abs()) * 0.08;
            if motion > max_motion {
                max_motion = motion;
            }
        }

        max_motion
    }

    fn collect_field_draws(&mut self, t: f32, still: bool, mood: &Mood) {
        let specs = field_specs();
        let n = self.field_used;
        let w = self.pane_w;
        let h = self.pane_h;
        let inv_w = 1.0 / w;
        let inv_h = 1.0 / h;
        let center = self.mark_center;
        let span = self.mark_span;
        let diag = w.hypot(h);
        let reach = diag * 0.62;
        let inv_reach = 1.0 / reach.max(1.0);
        let clearing = span * 0.50;
        let inv_core_denom = 1.0 / (span * span * 0.55).max(1.0);
        let rich = if self.props.palette.dark { 0.90 } else { 0.88 };
        let opacity = self.props.glow;
        let ptr = self.pointer.at;
        let seen = self.pointer.seen;
        let quiet = self.quiet_rect;
        let twinkle_mul = if still { 0.0 } else { 0.16 };
        let bokeh_light_mul = if self.props.palette.dark { 1.0 } else { 0.72 };

        for (spec, p) in specs.iter().zip(self.field.iter()).take(n) {
            let px = p.x;
            let py = p.y;
            if px < -16.0 || py < -16.0 || px > w + 16.0 || py > h + 16.0 {
                continue;
            }

            let dx_c = px - center[0];
            let dy_c = py - center[1];
            let away2 = dx_c * dx_c + dy_c * dy_c;
            let away = away2.sqrt();
            if away >= reach && away2 > span * span * 2.5 {
                continue;
            }

            let fade = (1.0 - away * inv_reach).max(0.0)
                * smoothstep(clearing - 34.0, clearing + 30.0, away);
            let core = (-away2 * inv_core_denom).exp();
            if fade <= 0.002 && core <= 0.015 {
                continue;
            }

            let calm = match quiet {
                Some([ql, qt, qr, qb]) => {
                    let qx = px.clamp(ql, qr);
                    let qy = py.clamp(qt, qb);
                    let qdist = (px - qx).hypot(py - qy);
                    smoothstep(0.0, 110.0, qdist)
                }
                None => 1.0,
            };
            if calm <= 0.005 {
                continue;
            }

            let (cloud, warmth, other) = self.grids.sample_field(px * inv_w, py * inv_h);
            let mut tone = fade * smoothstep(0.12, 0.86, cloud) * 1.45 * rich;
            if away >= clearing {
                tone += core * 0.5 * rich;
            }

            let cx = spec.cx;
            let cy = spec.cy;
            let pdx = px - ptr[0];
            let pdy = py - ptr[1];
            let lit = if seen > 0.001 {
                seen * (-(pdx * pdx + pdy * pdy) / 27_000.0).exp()
            } else {
                0.0
            };

            let (radius, blur, alpha) = if spec.is_bokeh {
                let r = spec.bokeh_radius;
                let a = fade
                    * spec.bokeh_alpha
                    * smoothstep(0.1, 0.6, tone + 0.2)
                    * calm
                    * bokeh_light_mul
                    * opacity;
                (r, r * 1.5, a)
            } else {
                let shown = smoothstep(spec.cut - 0.1, spec.cut + 0.1, tone);
                if shown <= 0.01 {
                    continue;
                }
                let speed = p.vx.hypot(p.vy);
                let clamped_tone = tone.clamp(0.0, 1.0);
                let r = spec.base_radius + 0.28 * clamped_tone;
                let mut a = shown
                    * (0.24 + 0.46 * clamped_tone)
                    * (1.0 + lit + 0.5 * (speed / 160.0).min(1.0));
                if twinkle_mul > 0.0 {
                    a *= 0.84 + twinkle_mul * (t * (0.2 + cx * 0.5) + cy * 60.0).sin();
                }
                a *= calm * opacity;
                (r, 0.0, a)
            };

            if radius < 0.05 || alpha < 0.012 {
                continue;
            }

            let mut shade = mix3(
                mood.deep,
                mood.tint,
                smoothstep(0.18, 0.62, warmth + 0.5 * core + 0.22 * (cx - 0.5)),
            );
            shade = mix3(
                shade,
                mood.alt,
                smoothstep(0.5, 0.74, other + 0.18 * (cy - 0.5)) * (1.0 - 0.6 * core),
            );
            let bright_mix =
                (core * 1.2 * cx * cx + spec.sparkle_step + 0.45 * lit * cy).clamp(0.0, 0.92);
            shade = mix3(shade, mood.bright, bright_mix);
            let mul = spec.shade_mul;
            shade = [
                (shade[0] * mul).clamp(0.0, 1.0),
                (shade[1] * mul).clamp(0.0, 1.0),
                (shade[2] * mul).clamp(0.0, 1.0),
            ];

            self.draws.push(DotDraw {
                cx: px,
                cy: py,
                radius,
                blur,
                rgb: shade,
                alpha: alpha.min(1.0),
            });
        }
    }

    fn collect_mark_draws(&mut self, t: f32, still: bool, mood: &Mood) {
        let grains = mark_grains();
        if grains.is_empty() {
            return;
        }
        let center = self.mark_center;
        let span = self.mark_span;
        let shown = self.props.glow;
        if shown <= 0.002 {
            return;
        }
        let ptr = self.pointer.at;
        let seen = self.pointer.seen;
        let radius_scale = (span / 430.0).max(0.76) * 0.96;
        let w = self.pane_w;
        let h = self.pane_h;

        for (g, p) in grains.iter().zip(self.mark.iter()) {
            let px = center[0] + p.x;
            let py = center[1] + p.y;
            if px < -8.0 || py < -8.0 || px > w + 8.0 || py > h + 8.0 {
                continue;
            }
            let shine = if still {
                0.0
            } else {
                (t * 0.5
                    + g.u * 7.0
                    + g.v * 9.0
                    + noise2(g.u * 6.0 + t * 0.12, g.v * 6.0 + t * 0.12) * 4.0)
                    .sin()
            };
            let radius = g.radius * radius_scale * (1.0 + 0.03 * shine);
            let fade = smoothstep(g.cy * 0.36, g.cy * 0.36 + 0.64, shown);
            let alpha = (g.light * (0.95 + 0.05 * shine) * fade).clamp(0.0, 1.0);
            if radius < 0.05 || alpha < 0.012 {
                continue;
            }

            let pdx = px - ptr[0];
            let pdy = py - ptr[1];
            let lit_ptr = if seen > 0.001 {
                seen * (-(pdx * pdx + pdy * pdy) / 27_000.0).exp()
            } else {
                0.0
            };

            let mut shade = ink_of(g.lit, g.rim, mood);
            let shine_boost = shine.max(0.0) * 0.6;
            if shine_boost > 0.0 {
                let s = 1.0 + 0.08 * shine_boost;
                shade = [
                    (shade[0] * s).min(1.0),
                    (shade[1] * s).min(1.0),
                    (shade[2] * s).min(1.0),
                ];
            }
            if lit_ptr > 0.001 {
                shade = mix3(shade, mood.bright, 0.35 * lit_ptr * g.cy);
            }

            self.draws.push(DotDraw {
                cx: px,
                cy: py,
                radius,
                blur: 0.0,
                rgb: shade,
                alpha,
            });
        }
    }

    fn paint_all(&self, bounds: Bounds<Pixels>, t: f32, mood: &Mood, window: &mut Window) {
        let scale = window.scale_factor().max(1.0);
        let origin_x = f32::from(bounds.origin.x);
        let origin_y = f32::from(bounds.origin.y);
        let pane_left_dev = origin_x * scale;
        let pane_top_dev = origin_y * scale;
        let pane_w_dev = f32::from(bounds.size.width) * scale;
        let pane_h_dev = f32::from(bounds.size.height) * scale;
        let corner_radius = self.props.corner_radius;
        let corner_radius_dev = corner_radius * scale;

        if self.props.show_field && self.props.palette.dark && self.props.glow > 0.001 {
            let params = [
                t,
                self.pointer.seen,
                0.16,
                self.props.glow,
                pack_rgb(mood.tint),
                pack_rgb(mood.deep),
                pack_rgb(mood.alt),
                scale,
                corner_radius,
                self.mark_center[0],
                self.mark_center[1],
                self.mark_span,
                self.pointer.at[0],
                self.pointer.at[1],
                0.0,
                0.0,
            ];
            let _ = window.paint_raw_shader(bounds, &WELCOME_GLOW, params, None);
        }

        let pane_corner = corner_radius + 12.0;
        let pane_w = self.pane_w;
        let pane_h = self.pane_h;
        for dot in &self.draws {
            let half = dot.radius + dot.blur * 0.5 + 1.0;
            let dot_bounds = Bounds::new(
                point(px(origin_x + dot.cx - half), px(origin_y + dot.cy - half)),
                size(px(half * 2.0), px(half * 2.0)),
            );
            let near_corner = corner_radius_dev > 0.0
                && (dot.cx < pane_corner || dot.cx > pane_w - pane_corner)
                && (dot.cy < pane_corner || dot.cy > pane_h - pane_corner);
            let params = if near_corner {
                [
                    (origin_x + dot.cx) * scale,
                    (origin_y + dot.cy) * scale,
                    dot.radius * scale,
                    dot.blur * scale,
                    dot.rgb[0],
                    dot.rgb[1],
                    dot.rgb[2],
                    dot.alpha,
                    pane_left_dev,
                    pane_top_dev,
                    pane_w_dev,
                    pane_h_dev,
                    corner_radius_dev,
                    0.0,
                    0.0,
                    0.0,
                ]
            } else {
                [
                    (origin_x + dot.cx) * scale,
                    (origin_y + dot.cy) * scale,
                    dot.radius * scale,
                    dot.blur * scale,
                    dot.rgb[0],
                    dot.rgb[1],
                    dot.rgb[2],
                    dot.alpha,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ]
            };
            let _ = window.paint_raw_shader(dot_bounds, &WELCOME_DOT, params, None);
        }
    }
}

impl Render for WelcomeDots {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let Some(strong) = entity.upgrade() else {
                    return;
                };

                let weak = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let Some(view) = weak.upgrade() else {
                        return;
                    };
                    let woke =
                        view.update(cx, |dots, _| dots.handle_mouse_move(event.position, bounds));
                    if woke {
                        cx.notify(view.entity_id());
                    }
                });

                let active = window.is_window_active() && window.is_visible();
                let (t, mood, keep_going) = strong.update(cx, |dots, _| {
                    let (t, mood) = dots.step_and_collect(bounds, active);
                    (t, mood, dots.awake)
                });

                let t_emit = Instant::now();
                strong.read(cx).paint_all(bounds, t, &mood, window);
                let emit_us = t_emit.elapsed().as_micros() as u64;
                strong.update(cx, |dots, _| dots.last_emit_us = emit_us);

                #[cfg(feature = "drive")]
                if keep_going {
                    WELCOME_PRESENTS.fetch_add(1, Ordering::Relaxed);
                }

                if keep_going && active && !cx.reduce_motion() {
                    let id = strong.entity_id();
                    window.on_next_frame(move |_, cx| cx.notify(id));
                } else {
                    sync_awake_flag(cx);
                }
            },
        )
        .size_full()
    }
}

#[derive(Default)]
struct WelcomeRegistry {
    views: HashMap<Option<String>, Entity<WelcomeDots>>,
}

impl Global for WelcomeRegistry {}

fn sync_awake_flag(cx: &App) {
    let any = cx
        .try_global::<WelcomeRegistry>()
        .is_some_and(|reg| reg.views.values().any(|view| view.read(cx).awake));
    WELCOME_AWAKE.store(any, Ordering::Relaxed);
}

pub fn view_for(
    agent: Option<&str>,
    glow: f32,
    show_field: bool,
    corner_radius: f32,
    cx: &mut App,
) -> Entity<WelcomeDots> {
    let props = Props {
        palette: Palette::from_app(cx),
        glow,
        show_field,
        reduce_motion: cx.reduce_motion(),
        corner_radius,
    };
    let key = agent.map(str::to_owned);
    if !cx.has_global::<WelcomeRegistry>() {
        cx.set_global(WelcomeRegistry::default());
    }
    let existing = cx.global::<WelcomeRegistry>().views.get(&key).cloned();
    if let Some(view) = existing {
        let changed = view.update(cx, |dots, _| dots.sync_props(props));
        if changed {
            cx.notify(view.entity_id());
        }
        sync_awake_flag(cx);
        return view;
    }
    let view = cx.new(|_| WelcomeDots::new(props));
    cx.global_mut::<WelcomeRegistry>()
        .views
        .insert(key, view.clone());
    sync_awake_flag(cx);
    view
}

pub fn theme_changed(cx: &mut App) {
    let Some(views) = cx
        .try_global::<WelcomeRegistry>()
        .map(|reg| reg.views.values().cloned().collect::<Vec<_>>())
    else {
        return;
    };
    let palette = Palette::from_app(cx);
    for view in views {
        let woke = view.update(cx, |dots, _| {
            dots.props.palette = palette;
            dots.dirty = true;
            dots.wake_for(PLAYS_FOR);
            true
        });
        if woke {
            cx.notify(view.entity_id());
        }
    }
}

pub fn window_activated(cx: &mut App) {
    let Some(views) = cx
        .try_global::<WelcomeRegistry>()
        .map(|reg| reg.views.values().cloned().collect::<Vec<_>>())
    else {
        return;
    };
    for view in views {
        let should_notify = view.update(cx, |dots, _| {
            if !dots.props.reduce_motion && dots.now() < dots.until {
                dots.awake = true;
                dots.last_frame = None;
                true
            } else {
                false
            }
        });
        if should_notify {
            cx.notify(view.entity_id());
        }
    }
    sync_awake_flag(cx);
}

pub fn clear_all(cx: &mut App) {
    if cx.has_global::<WelcomeRegistry>() {
        cx.global_mut::<WelcomeRegistry>().views.clear();
    }
    WELCOME_AWAKE.store(false, Ordering::Relaxed);
}

pub fn mark_placeholder(span: f32) -> impl IntoElement + Styled {
    gpui::div().relative().size(px(span)).child(
        canvas(|bounds, _, _| note_mark(bounds), |_, (), _, _| {})
            .absolute()
            .inset_0(),
    )
}

pub fn bench_pointer_move(pos: [f32; 2], cx: &mut App) {
    let Some(views) = cx
        .try_global::<WelcomeRegistry>()
        .map(|reg| reg.views.values().cloned().collect::<Vec<_>>())
    else {
        return;
    };
    for view in views {
        view.update(cx, |dots, _| {
            let now = dots.now();
            dots.pointer.move_to(pos, now);
            dots.pointer.seen = 1.0;
            dots.wake_for(PLAYS_ON_AFTER_POINTER);
        });
    }
}

pub fn bench_stats(cx: &App) -> (usize, usize, bool, u64, u64, u64) {
    let Some(reg) = cx.try_global::<WelcomeRegistry>() else {
        return (0, 0, false, 0, 0, 0);
    };
    let mut field = 0;
    let mut draws = 0;
    let mut awake = false;
    let mut step_us = 0;
    let mut collect_us = 0;
    let mut emit_us = 0;
    for view in reg.views.values() {
        let d = view.read(cx);
        field += d.field_used;
        draws += d.draws.len();
        awake |= d.awake;
        step_us += d.last_step_us;
        collect_us += d.last_collect_us;
        emit_us += d.last_emit_us;
    }
    (field, draws, awake, step_us, collect_us, emit_us)
}
