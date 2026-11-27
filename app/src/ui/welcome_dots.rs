use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(feature = "drive")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{
    canvas, point, px, size, App, AppContext, Bounds, Context, DispatchPhase, Entity, Global,
    IntoElement, MouseMoveEvent, ParentElement, Pixels, Point, RawShaderProgram, Render, Rgba,
    Styled, Task, Window,
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
const MIN_STEP_INTERVAL: f32 = 0.004;
const GRID_UPDATE_STEP: f32 = 0.015;
const CURL_GRID_W: usize = 32;
const CURL_GRID_H: usize = 24;
const FIELD_GRID_W: usize = 48;
const FIELD_GRID_H: usize = 36;
const MARK_CURL_W: usize = 10;
const MARK_CURL_H: usize = 10;
const MARK_SHINE_W: usize = 16;
const MARK_SHINE_H: usize = 16;
const SPATIAL_W: usize = 32;
const SPATIAL_H: usize = 24;
const SPATIAL_CELLS: usize = SPATIAL_W * SPATIAL_H;
const PACK_MAX_SIDE: f32 = 22.0;
const PACK_MAX_AREA: f32 = 260.0;
const GLOW_CELL_TARGET: f32 = 52.0;
const GLOW_MAX_COLS: usize = 36;
const GLOW_MAX_ROWS: usize = 24;
const GLOW_UPDATE_STEP: f32 = 0.05;
const AMBIENT_FRAME_INTERVAL: Duration = Duration::from_millis(33);

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
fn len2(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

#[inline]
fn fast_sin(x: f32) -> f32 {
    const INV_TAU: f32 = std::f32::consts::FRAC_1_PI * 0.5;
    let shifted = x * INV_TAU;
    let k = (shifted + 0.5).floor();
    let t = (shifted - k) * 2.0;
    let y = 4.0 * t * (1.0 - t.abs());
    y * (0.775 + 0.225 * y.abs())
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

#[inline]
fn pack_radius_alpha(radius_dev: f32, alpha: f32, is_bokeh: bool) -> f32 {
    let alpha_q = (alpha.clamp(0.0, 1.0) * 1023.0).round();
    let r_q = (radius_dev.clamp(0.0, 63.99) * 256.0).round().max(1.0);
    let packed = alpha_q * 16384.0 + r_q;
    if is_bokeh {
        -packed
    } else {
        packed
    }
}

#[inline]
fn outside_rounded_corner(px: f32, py: f32, w: f32, h: f32, radius: f32) -> f32 {
    let cx = (px - 0.5 * w).abs() - (0.5 * w - radius);
    let cy = (py - 0.5 * h).abs() - (0.5 * h - radius);
    len2(cx.max(0.0), cy.max(0.0)) - radius
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

#[derive(Clone, Copy)]
struct FieldDrawSpec {
    hx: f32,
    hy: f32,
    cx: f32,
    cy: f32,
    cut: f32,
    base_radius: f32,
    bokeh_radius: f32,
    bokeh_alpha: f32,
    sparkle_step: f32,
    shade_mul: f32,
    float_sin_x: f32,
    float_cos_x: f32,
    float_sin_y: f32,
    float_cos_y: f32,
    is_bokeh: bool,
}

#[derive(Clone, Copy)]
struct FieldPhysSpec {
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
}

struct FieldSpecs {
    draw: Vec<FieldDrawSpec>,
    phys: Vec<FieldPhysSpec>,
}

fn field_specs() -> &'static FieldSpecs {
    static SPECS: OnceLock<FieldSpecs> = OnceLock::new();
    SPECS.get_or_init(|| {
        let mut roll_home = Rolls::new(3);
        let mut roll_chance = Rolls::new(3);
        let mut draw = Vec::with_capacity(MOST_IN_FIELD);
        let mut phys = Vec::with_capacity(MOST_IN_FIELD);
        for at in 0..MOST_IN_FIELD {
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
            draw.push(FieldDrawSpec {
                hx,
                hy,
                cx,
                cy,
                cut,
                base_radius: 0.48 + 0.75 * cy5,
                bokeh_radius: 2.4 + (8.5 - 2.4) * cy2,
                bokeh_alpha: 0.12 + (0.05 - 0.12) * cy,
                sparkle_step: if cx >= 0.965 { 0.5 } else { 0.0 },
                shade_mul: 0.78 + 0.4 * cx,
                float_sin_x,
                float_cos_x,
                float_sin_y,
                float_cos_y,
                is_bokeh: cx > 0.982,
            });
            phys.push(FieldPhysSpec {
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
            });
        }
        FieldSpecs { draw, phys }
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
        let mut grains: Vec<MarkGrain> = raw
            .into_iter()
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
            .collect();

        // Spatial 2D tile ordering keeps consecutive mark grains within ~4pt so 4-dot packing merges them cleanly.
        grains.sort_by_key(|g| {
            let gx = ((g.u + 0.5) * 28.0).clamp(0.0, 27.0) as u32;
            let gy = ((g.v + 0.5) * 28.0).clamp(0.0, 27.0) as u32;
            (gy, gx)
        });
        grains
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
            let speed = len2(vx, vy).max(1.0);
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
    mark_curl: Vec<(f32, f32)>,
    mark_shine: Vec<f32>,
    last_field_t: f32,
    last_field_w: f32,
    last_field_h: f32,
    last_mark_t: f32,
    last_mark_span: f32,
}

impl SimGrids {
    fn new() -> Self {
        Self {
            curl: vec![(0.0, 0.0); CURL_GRID_W * CURL_GRID_H],
            field: vec![FieldSample::default(); FIELD_GRID_W * FIELD_GRID_H],
            mark_curl: vec![(0.0, 0.0); MARK_CURL_W * MARK_CURL_H],
            mark_shine: vec![0.0; MARK_SHINE_W * MARK_SHINE_H],
            last_field_t: f32::NAN,
            last_field_w: 0.0,
            last_field_h: 0.0,
            last_mark_t: f32::NAN,
            last_mark_span: 0.0,
        }
    }

    fn update_field(&mut self, width: f32, height: f32, t: f32) {
        let same_size =
            (self.last_field_w - width).abs() <= 0.5 && (self.last_field_h - height).abs() <= 0.5;
        if same_size && (self.last_field_t - t).abs() < GRID_UPDATE_STEP {
            return;
        }
        self.last_field_t = t;
        self.last_field_w = width;
        self.last_field_h = height;

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
            let dy = py / 150.0;
            let wy_py = py / 330.0 - t * 0.008;
            let oy_py = py / 210.0 + t * 0.01 + 17.0;
            for (gx, cell) in row.iter_mut().enumerate() {
                let px = gx as f32 * fw_inv;
                let dx = px / 150.0;
                let wx = noise2(dx * 0.7 + t * 0.02, dy * 0.7 + t * 0.02);
                let wy = noise2(dx * 0.7 + 9.0 - t * 0.02, dy * 0.7 + 9.0 - t * 0.02);
                *cell = FieldSample {
                    cloud: noise2(dx + t * 0.025 + 2.0 * wx, dy - t * 0.015 + 2.0 * wy),
                    warmth: noise2(px / 330.0 + t * 0.011, wy_py),
                    other: noise2(px / 210.0 - t * 0.008 + 17.0, oy_py),
                };
            }
        }
    }

    fn update_mark(&mut self, span: f32, t: f32) {
        if (self.last_mark_span - span).abs() <= 0.5
            && (self.last_mark_t - t).abs() < GRID_UPDATE_STEP
        {
            return;
        }
        self.last_mark_t = t;
        self.last_mark_span = span;

        let t_curl = t * 0.05;
        let scale = span / 60.0;
        for (gy, row) in self.mark_curl.chunks_mut(MARK_CURL_W).enumerate() {
            let v = gy as f32 / (MARK_CURL_H - 1) as f32 - 0.5;
            let ay = v * scale;
            for (gx, cell) in row.iter_mut().enumerate() {
                let u = gx as f32 / (MARK_CURL_W - 1) as f32 - 0.5;
                let ax = u * scale;
                let (dx, dy) = curl(ax, ay, t_curl);
                *cell = (dx * 0.5, dy * 0.5);
            }
        }

        let t_shine = t * 0.12;
        for (gy, row) in self.mark_shine.chunks_mut(MARK_SHINE_W).enumerate() {
            let v = gy as f32 / (MARK_SHINE_H - 1) as f32 - 0.5;
            let nv = v * 6.0 + t_shine;
            for (gx, cell) in row.iter_mut().enumerate() {
                let u = gx as f32 / (MARK_SHINE_W - 1) as f32 - 0.5;
                *cell = noise2(u * 6.0 + t_shine, nv) * 4.0;
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
        let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
            self.curl.get(i00),
            self.curl.get(i00 + 1),
            self.curl.get(i00 + CURL_GRID_W),
            self.curl.get(i00 + CURL_GRID_W + 1),
        ) else {
            return (0.0, 0.0);
        };
        let cx0 = a.0 + (b.0 - a.0) * tx;
        let cx1 = c.0 + (d.0 - c.0) * tx;
        let cy0 = a.1 + (b.1 - a.1) * tx;
        let cy1 = c.1 + (d.1 - c.1) * tx;
        (cx0 + (cx1 - cx0) * ty, cy0 + (cy1 - cy0) * ty)
    }

    #[inline]
    fn sample_field_cloud(&self, u: f32, v: f32) -> (f32, usize, f32, f32) {
        let fx = (u * (FIELD_GRID_W - 1) as f32).clamp(0.0, (FIELD_GRID_W - 1) as f32);
        let fy = (v * (FIELD_GRID_H - 1) as f32).clamp(0.0, (FIELD_GRID_H - 1) as f32);
        let x0 = (fx as usize).min(FIELD_GRID_W - 2);
        let y0 = (fy as usize).min(FIELD_GRID_H - 2);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let i00 = y0 * FIELD_GRID_W + x0;
        let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
            self.field.get(i00),
            self.field.get(i00 + 1),
            self.field.get(i00 + FIELD_GRID_W),
            self.field.get(i00 + FIELD_GRID_W + 1),
        ) else {
            return (0.0, 0, 0.0, 0.0);
        };
        let top = a.cloud + (b.cloud - a.cloud) * tx;
        let bot = c.cloud + (d.cloud - c.cloud) * tx;
        (top + (bot - top) * ty, i00, tx, ty)
    }

    #[inline]
    fn sample_field_colors(&self, i00: usize, tx: f32, ty: f32) -> (f32, f32) {
        let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
            self.field.get(i00),
            self.field.get(i00 + 1),
            self.field.get(i00 + FIELD_GRID_W),
            self.field.get(i00 + FIELD_GRID_W + 1),
        ) else {
            return (0.0, 0.0);
        };
        let wt = a.warmth + (b.warmth - a.warmth) * tx;
        let wb = c.warmth + (d.warmth - c.warmth) * tx;
        let ot = a.other + (b.other - a.other) * tx;
        let ob = c.other + (d.other - c.other) * tx;
        (wt + (wb - wt) * ty, ot + (ob - ot) * ty)
    }

    #[inline]
    fn sample_mark_curl(&self, u: f32, v: f32) -> (f32, f32) {
        let fx = ((u + 0.5) * (MARK_CURL_W - 1) as f32).clamp(0.0, (MARK_CURL_W - 1) as f32);
        let fy = ((v + 0.5) * (MARK_CURL_H - 1) as f32).clamp(0.0, (MARK_CURL_H - 1) as f32);
        let x0 = (fx as usize).min(MARK_CURL_W - 2);
        let y0 = (fy as usize).min(MARK_CURL_H - 2);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let i00 = y0 * MARK_CURL_W + x0;
        let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
            self.mark_curl.get(i00),
            self.mark_curl.get(i00 + 1),
            self.mark_curl.get(i00 + MARK_CURL_W),
            self.mark_curl.get(i00 + MARK_CURL_W + 1),
        ) else {
            return (0.0, 0.0);
        };
        let cx0 = a.0 + (b.0 - a.0) * tx;
        let cx1 = c.0 + (d.0 - c.0) * tx;
        let cy0 = a.1 + (b.1 - a.1) * tx;
        let cy1 = c.1 + (d.1 - c.1) * tx;
        (cx0 + (cx1 - cx0) * ty, cy0 + (cy1 - cy0) * ty)
    }

    #[inline]
    fn sample_mark_shine(&self, u: f32, v: f32) -> f32 {
        let fx = ((u + 0.5) * (MARK_SHINE_W - 1) as f32).clamp(0.0, (MARK_SHINE_W - 1) as f32);
        let fy = ((v + 0.5) * (MARK_SHINE_H - 1) as f32).clamp(0.0, (MARK_SHINE_H - 1) as f32);
        let x0 = (fx as usize).min(MARK_SHINE_W - 2);
        let y0 = (fy as usize).min(MARK_SHINE_H - 2);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let i00 = y0 * MARK_SHINE_W + x0;
        let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
            self.mark_shine.get(i00),
            self.mark_shine.get(i00 + 1),
            self.mark_shine.get(i00 + MARK_SHINE_W),
            self.mark_shine.get(i00 + MARK_SHINE_W + 1),
        ) else {
            return 0.0;
        };
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        top + (bot - top) * ty
    }
}

struct FieldSpatialGrid {
    built_used: usize,
    offsets: Vec<usize>,
    indices: Vec<u32>,
    cell_in_reach: Vec<u8>,
}

impl FieldSpatialGrid {
    fn new() -> Self {
        Self {
            built_used: 0,
            offsets: vec![0; SPATIAL_CELLS + 1],
            indices: Vec::new(),
            cell_in_reach: vec![1; SPATIAL_CELLS],
        }
    }

    fn rebuild(&mut self, field_used: usize) {
        if self.built_used == field_used {
            return;
        }
        self.built_used = field_used;
        let draw_specs = &field_specs().draw;
        let mut counts = vec![0usize; SPATIAL_CELLS];
        for spec in draw_specs.iter().take(field_used) {
            let cx = ((spec.hx * SPATIAL_W as f32) as usize).min(SPATIAL_W - 1);
            let cy = ((spec.hy * SPATIAL_H as f32) as usize).min(SPATIAL_H - 1);
            if let Some(slot) = counts.get_mut(cy * SPATIAL_W + cx) {
                *slot += 1;
            }
        }
        let mut sum = 0usize;
        for (cell, &count) in counts.iter().enumerate() {
            if let Some(slot) = self.offsets.get_mut(cell) {
                *slot = sum;
            }
            sum += count;
        }
        if let Some(last) = self.offsets.get_mut(SPATIAL_CELLS) {
            *last = sum;
        }
        self.indices.resize(sum, 0);
        let mut cursor = self.offsets.clone();
        for (idx, spec) in draw_specs.iter().take(field_used).enumerate() {
            let cx = ((spec.hx * SPATIAL_W as f32) as usize).min(SPATIAL_W - 1);
            let cy = ((spec.hy * SPATIAL_H as f32) as usize).min(SPATIAL_H - 1);
            let cell = cy * SPATIAL_W + cx;
            if let Some(pos) = cursor.get_mut(cell) {
                if let Some(dst) = self.indices.get_mut(*pos) {
                    *dst = idx as u32;
                }
                *pos += 1;
            }
        }
        for cell in 0..SPATIAL_CELLS {
            let (Some(&start), Some(&end)) = (self.offsets.get(cell), self.offsets.get(cell + 1))
            else {
                continue;
            };
            if let Some(slice) = self.indices.get_mut(start..end) {
                slice.sort_unstable_by_key(|&idx| {
                    let Some(spec) = draw_specs.get(idx as usize) else {
                        return (0u32, 0u32);
                    };
                    let fx = (spec.hx * (SPATIAL_W * 4) as f32) as u32;
                    let fy = (spec.hy * (SPATIAL_H * 4) as f32) as u32;
                    (fy, fx)
                });
            }
        }
    }

    fn update_visibility(&mut self, w: f32, h: f32, center: [f32; 2], span: f32) {
        let diag = len2(w, h);
        let reach = diag * 0.62;
        // Include max pointer leash (200pt) + ambient drift (25pt) so no visible particle is culled.
        let max_dist = reach.max(span * 1.6) + 225.0;
        let max_dist_sq = max_dist * max_dist;
        let cell_w = w / SPATIAL_W as f32;
        let cell_h = h / SPATIAL_H as f32;
        for gy in 0..SPATIAL_H {
            let y0 = gy as f32 * cell_h;
            let y1 = y0 + cell_h;
            let nearest_y = center[1].clamp(y0, y1);
            let dy = nearest_y - center[1];
            for gx in 0..SPATIAL_W {
                let x0 = gx as f32 * cell_w;
                let x1 = x0 + cell_w;
                let nearest_x = center[0].clamp(x0, x1);
                let dx = nearest_x - center[0];
                let visible = u8::from(dx * dx + dy * dy <= max_dist_sq);
                if let Some(slot) = self.cell_in_reach.get_mut(gy * SPATIAL_W + gx) {
                    *slot = visible;
                }
            }
        }
    }

    #[inline]
    fn cell_slice(&self, cell: usize) -> &[u32] {
        let (Some(&start), Some(&end)) = (self.offsets.get(cell), self.offsets.get(cell + 1))
        else {
            return &[];
        };
        self.indices.get(start..end).unwrap_or(&[])
    }
}

#[derive(Clone, Copy)]
struct DotDraw {
    cx: f32,
    cy: f32,
    radius: f32,
    is_bokeh: bool,
    rgb: [f32; 3],
    alpha: f32,
}

#[derive(Clone, Copy)]
struct PackedQuad {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    params: [f32; 16],
}

#[derive(Clone, Copy)]
struct GlowQuad {
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    params: [f32; 16],
}

#[derive(Clone, Copy)]
struct GlowCacheKey {
    t_bucket: i32,
    seen_q: u16,
    ptr_x: i32,
    ptr_y: i32,
    opacity_q: u16,
    w_bits: u32,
    h_bits: u32,
    origin_x_bits: u32,
    origin_y_bits: u32,
    scale_bits: u32,
    corner_bits: u32,
    tint_packed: u32,
    deep_packed: u32,
    alt_packed: u32,
}

impl PartialEq for GlowCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.t_bucket == other.t_bucket
            && self.seen_q == other.seen_q
            && self.ptr_x == other.ptr_x
            && self.ptr_y == other.ptr_y
            && self.opacity_q == other.opacity_q
            && self.w_bits == other.w_bits
            && self.h_bits == other.h_bits
            && self.origin_x_bits == other.origin_x_bits
            && self.origin_y_bits == other.origin_y_bits
            && self.scale_bits == other.scale_bits
            && self.corner_bits == other.corner_bits
            && self.tint_packed == other.tint_packed
            && self.deep_packed == other.deep_packed
            && self.alt_packed == other.alt_packed
    }
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
    frame_scheduled: bool,
    ambient_task: Option<Task<()>>,

    pane_w: f32,
    pane_h: f32,
    mark_center: [f32; 2],
    mark_span: f32,
    quiet_rect: Option<[f32; 4]>,

    field_used: usize,
    field_fresh: bool,
    field: Vec<ParticleState>,
    field_is_perturbed: Vec<u8>,
    field_perturbed: Vec<u32>,
    spatial: FieldSpatialGrid,

    mark_fresh: bool,
    mark_active: bool,
    mark: Vec<ParticleState>,

    grids: SimGrids,
    draws: Vec<DotDraw>,
    packed: Vec<PackedQuad>,
    glow_corners: Vec<[f32; 3]>,
    glow_quads: Vec<GlowQuad>,
    glow_cache_key: Option<GlowCacheKey>,
    draws_gen: u64,
    packed_key: (u64, u32, u32, u32, u32),
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
            frame_scheduled: false,
            ambient_task: None,
            pane_w: 0.0,
            pane_h: 0.0,
            mark_center: [0.0, 0.0],
            mark_span: 116.0,
            quiet_rect: None,
            field_used: 0,
            field_fresh: true,
            field: Vec::new(),
            field_is_perturbed: Vec::new(),
            field_perturbed: Vec::with_capacity(2048),
            spatial: FieldSpatialGrid::new(),
            mark_fresh: true,
            mark_active: false,
            mark: vec![ParticleState::default(); mark_len],
            grids: SimGrids::new(),
            draws: Vec::with_capacity(6144),
            packed: Vec::with_capacity(2048),
            glow_corners: Vec::with_capacity((GLOW_MAX_COLS + 1) * (GLOW_MAX_ROWS + 1)),
            glow_quads: Vec::with_capacity(512),
            glow_cache_key: None,
            draws_gen: 0,
            packed_key: (0, 0, 0, 0, 0),
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
        let radius_changed = (self.props.corner_radius - next.corner_radius).abs() > 0.01;
        self.props = next;
        if next.reduce_motion {
            self.awake = false;
            self.last_frame = None;
            self.dirty = true;
            return palette_changed
                || motion_changed
                || glow_changed
                || field_changed
                || radius_changed;
        }
        if palette_changed || motion_changed || glow_changed || field_changed || radius_changed {
            self.dirty = true;
        }
        if palette_changed || motion_changed || field_changed {
            self.wake_for(PLAYS_FOR);
        }
        palette_changed || motion_changed || glow_changed || field_changed || radius_changed
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
        let was_ambient_paced = self.ambient_task.is_some();
        let woke = if bounds.contains(&pos) {
            let local = [
                f32::from(pos.x - bounds.origin.x),
                f32::from(pos.y - bounds.origin.y),
            ];
            self.pointer.move_to(local, now);
            self.ambient_task = None;
            self.wake_for(PLAYS_ON_AFTER_POINTER)
        } else if self.pointer.here {
            self.pointer.here = false;
            self.ambient_task = None;
            self.wake_for(PLAYS_ON_AFTER_POINTER)
        } else {
            false
        };
        (woke || was_ambient_paced) && !self.frame_scheduled
    }

    fn wants_full_vsync(&self) -> bool {
        self.pointer.seen > 0.005
            || self.pointer.here && (self.now() - self.pointer.moved) < POINTER_FELT_FOR
            || !self.field_perturbed.is_empty()
            || self.mark_active
            || self.props.glow < 0.999
    }

    fn sync_geometry(&mut self, bounds: Bounds<Pixels>) {
        let width = f32::from(bounds.size.width).max(1.0);
        let height = f32::from(bounds.size.height).max(1.0);
        let mut geom_changed = false;
        if (self.pane_w - width).abs() > 1.0 || (self.pane_h - height).abs() > 1.0 {
            self.pane_w = width;
            self.pane_h = height;
            self.field_used = ((width * height) / FIELD_AREA_EACH)
                .round()
                .clamp(0.0, MOST_IN_FIELD as f32) as usize;
            if self.field.len() < self.field_used {
                self.field.resize(self.field_used, ParticleState::default());
                self.field_is_perturbed.resize(self.field_used, 0);
            }
            self.spatial.rebuild(self.field_used);
            self.field_fresh = true;
            self.dirty = true;
            geom_changed = true;
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
                geom_changed = true;
            }
        } else if self.mark_center == [0.0, 0.0] {
            self.mark_center = [width * 0.5, height * 0.34];
            self.mark_span = 116.0;
            self.dirty = true;
            geom_changed = true;
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

        if geom_changed {
            self.spatial.update_visibility(
                self.pane_w,
                self.pane_h,
                self.mark_center,
                self.mark_span,
            );
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

        let now = self.now();
        if !still
            && !self.dirty
            && !self.draws.is_empty()
            && self
                .last_frame
                .is_some_and(|prev| (now - prev) < MIN_STEP_INTERVAL)
        {
            let mood = self
                .mood
                .unwrap_or_else(|| Mood::from_palette(self.props.palette));
            return (self.clock, mood);
        }
        self.dirty = false;

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
            self.grids.update_field(self.pane_w, self.pane_h, t);
        }
        if !still {
            self.grids.update_mark(self.mark_span, t);
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
        self.draws_gen = self.draws_gen.wrapping_add(1);
        self.last_collect_us = t1.elapsed().as_micros() as u64;

        (t, mood)
    }

    fn clear_field_perturbed(&mut self) {
        for &idx in &self.field_perturbed {
            if let Some(slot) = self.field_is_perturbed.get_mut(idx as usize) {
                *slot = 0;
            }
        }
        self.field_perturbed.clear();
    }

    fn step_field(&mut self, t: f32, still: bool, seconds: f32) -> f32 {
        let reset = still || self.field_fresh;
        if reset {
            self.clear_field_perturbed();
            return 0.0;
        }

        let specs = field_specs();
        let w = self.pane_w;
        let h = self.pane_h;
        let ptr = self.pointer.at;
        let path_x = self.pointer.before[0] - ptr[0];
        let path_y = self.pointer.before[1] - ptr[1];
        let path_len2 = (path_x * path_x + path_y * path_y).max(1.0);
        let inv_path_len2 = 1.0 / path_len2;
        let sweep_x = self.pointer.sweep[0];
        let sweep_y = self.pointer.sweep[1];
        let quick = len2(sweep_x, sweep_y);
        let seen = self.pointer.seen;
        let ptr_active = seen > 0.0005;

        if !ptr_active && self.field_perturbed.is_empty() {
            return 0.0;
        }

        let min_px = ptr[0].min(self.pointer.before[0]) - 420.0;
        let max_px = ptr[0].max(self.pointer.before[0]) + 420.0;
        let min_py = ptr[1].min(self.pointer.before[1]) - 420.0;
        let max_py = ptr[1].max(self.pointer.before[1]) + 420.0;
        let quick_scale = (0.3 + quick / 950.0).min(1.0);
        let (sin_f1, cos_f1) = (t * 0.2).sin_cos();
        let (sin_f2, cos_f2) = (t * 0.17).sin_cos();
        let t_spin = t * 1.7;

        // Only wake unperturbed field particles whose home cells overlap the pointer's reach.
        if ptr_active && self.field_used > 0 {
            let cell_w = w / SPATIAL_W as f32;
            let cell_h = h / SPATIAL_H as f32;
            let gx0 = (((min_px - 32.0) / cell_w).floor() as isize).clamp(0, SPATIAL_W as isize - 1)
                as usize;
            let gx1 = (((max_px + 32.0) / cell_w).floor() as isize).clamp(0, SPATIAL_W as isize - 1)
                as usize;
            let gy0 = (((min_py - 32.0) / cell_h).floor() as isize).clamp(0, SPATIAL_H as isize - 1)
                as usize;
            let gy1 = (((max_py + 32.0) / cell_h).floor() as isize).clamp(0, SPATIAL_H as isize - 1)
                as usize;

            for gy in gy0..=gy1 {
                for gx in gx0..=gx1 {
                    let cell = gy * SPATIAL_W + gx;
                    if self.spatial.cell_in_reach.get(cell).copied().unwrap_or(0) == 0 {
                        continue;
                    }
                    let slice = self.spatial.cell_slice(cell);
                    for &idx_u32 in slice {
                        let idx = idx_u32 as usize;
                        if self.field_is_perturbed.get(idx).copied().unwrap_or(1) != 0 {
                            continue;
                        }
                        let (Some(dspec), Some(pspec)) = (specs.draw.get(idx), specs.phys.get(idx))
                        else {
                            continue;
                        };
                        let (curl_x, curl_y) = self.grids.sample_curl(dspec.hx, dspec.hy);
                        let float_x =
                            (sin_f1 * dspec.float_cos_x + cos_f1 * dspec.float_sin_x) * 4.0;
                        let float_y =
                            (cos_f2 * dspec.float_cos_y - sin_f2 * dspec.float_sin_y) * 4.0;
                        let bx = dspec.hx * w + curl_x + float_x;
                        let by = dspec.hy * h + curl_y + float_y;
                        if bx < min_px || bx > max_px || by < min_py || by > max_py {
                            continue;
                        }
                        let rel_x = bx - ptr[0];
                        let rel_y = by - ptr[1];
                        let along =
                            ((rel_x * path_x + rel_y * path_y) * inv_path_len2).clamp(0.0, 1.0);
                        let off_x = rel_x - path_x * along;
                        let off_y = rel_y - path_y * along;
                        if (off_x * off_x + off_y * off_y) * pspec.inv_felt2 < 8.5 {
                            if let Some(p) = self.field.get_mut(idx) {
                                *p = ParticleState {
                                    x: bx,
                                    y: by,
                                    vx: 0.0,
                                    vy: 0.0,
                                };
                            }
                            if let Some(flag) = self.field_is_perturbed.get_mut(idx) {
                                *flag = 1;
                            }
                            self.field_perturbed.push(idx_u32);
                        }
                    }
                }
            }
        }

        let mut max_motion = 0.0f32;
        let mut cursor = 0usize;
        while cursor < self.field_perturbed.len() {
            let Some(&idx_u32) = self.field_perturbed.get(cursor) else {
                break;
            };
            let idx = idx_u32 as usize;
            let (Some(dspec), Some(pspec), Some(p)) = (
                specs.draw.get(idx),
                specs.phys.get(idx),
                self.field.get_mut(idx),
            ) else {
                self.field_perturbed.swap_remove(cursor);
                continue;
            };

            let (curl_x, curl_y) = self.grids.sample_curl(dspec.hx, dspec.hy);
            let float_x = (sin_f1 * dspec.float_cos_x + cos_f1 * dspec.float_sin_x) * 4.0;
            let float_y = (cos_f2 * dspec.float_cos_y - sin_f2 * dspec.float_sin_y) * 4.0;
            let mut bx = dspec.hx * w + curl_x + float_x;
            let mut by = dspec.hy * h + curl_y + float_y;

            let mut x = p.x;
            let mut y = p.y;
            let mut vx = p.vx;
            let mut vy = p.vy;

            let mut touched = false;
            if ptr_active && x >= min_px && x <= max_px && y >= min_py && y <= max_py {
                let rel_x = x - ptr[0];
                let rel_y = y - ptr[1];
                let along = ((rel_x * path_x + rel_y * path_y) * inv_path_len2).clamp(0.0, 1.0);
                let off_x = rel_x - path_x * along;
                let off_y = rel_y - path_y * along;
                let far2 = off_x * off_x + off_y * off_y;
                let exp_arg = far2 * pspec.inv_felt2;
                if exp_arg < 8.5 {
                    touched = true;
                    let near = seen * (-exp_arg).exp();
                    let far = far2.sqrt() + 0.01;
                    let inv_far = 1.0 / far;
                    let dir_x = off_x * inv_far;
                    let dir_y = off_y * inv_far;
                    let rot_sx = sweep_x * pspec.turn_cos - sweep_y * pspec.turn_sin;
                    let rot_sy = sweep_x * pspec.turn_sin + sweep_y * pspec.turn_cos;
                    let fling_x = rot_sx * pspec.fling_mul + dir_x * quick * pspec.radial_mul;
                    let fling_y = rot_sy * pspec.fling_mul + dir_y * quick * pspec.radial_mul;

                    let was = len2(vx, vy);
                    let cap = was.max(pspec.cap_own * quick_scale);
                    let disp = len2(x - bx, y - by);
                    let leash_mul = (1.0 - disp * (1.0 / 200.0)).max(0.0);
                    let impulse = near * seconds * leash_mul;
                    vx += fling_x * impulse;
                    vy += fling_y * impulse;
                    let new_speed = len2(vx, vy);
                    if new_speed > cap && new_speed > 1.0 {
                        let s = cap / new_speed;
                        vx *= s;
                        vy *= s;
                    }
                    let push = near * pspec.felt * 0.38;
                    bx += dir_x * push;
                    by += dir_y * push;
                }
            }

            let dx = bx - x;
            let dy = by - y;
            let v2 = vx * vx + vy * vy;
            if !touched && v2 < 0.01 && dx * dx + dy * dy < 0.01 {
                *p = ParticleState {
                    x: bx,
                    y: by,
                    vx: 0.0,
                    vy: 0.0,
                };
                if let Some(flag) = self.field_is_perturbed.get_mut(idx) {
                    *flag = 0;
                }
                self.field_perturbed.swap_remove(cursor);
                continue;
            }

            if v2 > 0.04 {
                let spin = seconds
                    * (pspec.spin_base
                        + 5.0 * fast_sin(x * (1.0 / 37.0) + y * (1.0 / 53.0) + t_spin));
                let (ss, sc) = spin.sin_cos();
                let nvx = vx * sc - vy * ss;
                let nvy = vx * ss + vy * sc;
                vx = nvx;
                vy = nvy;
            }

            let spring_dt = pspec.spring * seconds;
            let inv_drag = 1.0 / (1.0 + pspec.drag * seconds);
            vx = (vx + dx * spring_dt) * inv_drag;
            vy = (vy + dy * spring_dt) * inv_drag;
            x += vx * seconds;
            y += vy * seconds;

            *p = ParticleState { x, y, vx, vy };

            let motion = (x - bx).abs() + (y - by).abs() + (vx.abs() + vy.abs()) * 0.08;
            if motion > max_motion {
                max_motion = motion;
            }
            cursor += 1;
        }

        max_motion
    }

    fn step_mark(&mut self, t: f32, still: bool, seconds: f32) -> f32 {
        let grains = mark_grains();
        if grains.is_empty() {
            return 0.0;
        }
        let reset = still || self.mark_fresh;
        if reset {
            self.mark_active = false;
            return 0.0;
        }

        let span = self.mark_span;
        let center = self.mark_center;
        let ptr = self.pointer.at;
        let seen = self.pointer.seen;
        let ptr_active = seen > 0.0005;
        let disperse_amt = 1.0 - self.props.glow;
        let felt_base = (span * 0.42).clamp(36.0, 98.0);
        let mark_reach = span * 0.6 + felt_base * 3.3 + 20.0;
        let mark_near_ptr = ptr_active
            && (ptr[0].min(self.pointer.before[0]) - center[0]) <= mark_reach
            && (ptr[0].max(self.pointer.before[0]) - center[0]) >= -mark_reach
            && (ptr[1].min(self.pointer.before[1]) - center[1]) <= mark_reach
            && (ptr[1].max(self.pointer.before[1]) - center[1]) >= -mark_reach;

        if !self.mark_active && !mark_near_ptr && disperse_amt <= 0.001 {
            return 0.0;
        }

        if !self.mark_active {
            for (g, p) in grains.iter().zip(self.mark.iter_mut()) {
                let (cx_off, cy_off) = self.grids.sample_mark_curl(g.u, g.v);
                *p = ParticleState {
                    x: g.u * span + cx_off,
                    y: g.v * span + cy_off,
                    vx: 0.0,
                    vy: 0.0,
                };
            }
            self.mark_active = true;
        }

        let path_x = self.pointer.before[0] - ptr[0];
        let path_y = self.pointer.before[1] - ptr[1];
        let inv_path_len2 = 1.0 / (path_x * path_x + path_y * path_y).max(1.0);
        let sweep_x = self.pointer.sweep[0];
        let sweep_y = self.pointer.sweep[1];
        let quick = len2(sweep_x, sweep_y);
        let most = (span * 3.8).clamp(320.0, 640.0);
        let inv_leash = 1.0 / (span * 0.56).clamp(50.0, 148.0);
        let quick_scale = (0.3 + quick / 950.0).min(1.0);
        let t_spin = t * 1.7;

        let mut max_motion = 0.0f32;
        let mut any_touched = false;

        for (g, p) in grains.iter().zip(self.mark.iter_mut()) {
            let ax = g.u * span;
            let ay = g.v * span;
            let (cx_off, cy_off) = self.grids.sample_mark_curl(g.u, g.v);
            let mut bx = ax + cx_off;
            let mut by = ay + cy_off;
            if disperse_amt > 0.001 {
                let inv_len = 1.0 / len2(ax, ay).max(1.0);
                let fling = disperse_amt * (16.0 + 28.0 * g.cx);
                bx += ax * inv_len * fling;
                by += ay * inv_len * fling;
            }

            let mut x = p.x;
            let mut y = p.y;
            let mut vx = p.vx;
            let mut vy = p.vy;
            let page_x = center[0] + x;
            let page_y = center[1] + y;

            let mut touched = false;
            if mark_near_ptr {
                let rel_x = page_x - ptr[0];
                let rel_y = page_y - ptr[1];
                let along = ((rel_x * path_x + rel_y * path_y) * inv_path_len2).clamp(0.0, 1.0);
                let off_x = rel_x - path_x * along;
                let off_y = rel_y - path_y * along;
                let felt = felt_base * (0.8 + 0.3 * g.cy);
                let far2 = off_x * off_x + off_y * off_y;
                let exp_arg = far2 / (felt * felt);
                if exp_arg < 8.5 {
                    touched = true;
                    any_touched = true;
                    let near = seen * (-exp_arg).exp();
                    let inv_far = 1.0 / (far2.sqrt() + 0.01);
                    let dir_x = off_x * inv_far;
                    let dir_y = off_y * inv_far;
                    let rot_sx = sweep_x * g.turn_cos - sweep_y * g.turn_sin;
                    let rot_sy = sweep_x * g.turn_sin + sweep_y * g.turn_cos;
                    let fling_x = rot_sx * g.fling_mul + dir_x * quick * g.radial_mul;
                    let fling_y = rot_sy * g.fling_mul + dir_y * quick * g.radial_mul;

                    let was = len2(vx, vy);
                    let cap = was.max(most * (0.2 + 0.8 * g.own) * quick_scale);
                    let disp = len2(x - bx, y - by);
                    let leash_mul = (1.0 - disp * inv_leash).max(0.0);
                    let impulse = near * seconds * leash_mul;
                    vx += fling_x * impulse;
                    vy += fling_y * impulse;
                    let new_speed = len2(vx, vy);
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
                let spin = seconds
                    * (g.spin_base
                        + 5.0 * fast_sin(page_x * (1.0 / 37.0) + page_y * (1.0 / 53.0) + t_spin));
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

        if !any_touched && max_motion < 0.02 && disperse_amt <= 0.001 {
            self.mark_active = false;
        }

        max_motion
    }

    fn collect_field_draws(&mut self, t: f32, still: bool, mood: &Mood) {
        let draw_specs = &field_specs().draw;
        if self.field_used == 0 {
            return;
        }
        let w = self.pane_w;
        let h = self.pane_h;
        let inv_w = 1.0 / w;
        let inv_h = 1.0 / h;
        let center = self.mark_center;
        let span = self.mark_span;
        let diag = len2(w, h);
        let reach = diag * 0.62;
        let reach_sq = reach * reach;
        let core_cutoff_sq = span * span * 2.5;
        let inv_reach = 1.0 / reach.max(1.0);
        let clearing = span * 0.50;
        let inv_core_denom = 1.0 / (span * span * 0.55).max(1.0);
        let rich = if self.props.palette.dark { 0.90 } else { 0.88 };
        let opacity = self.props.glow;
        let ptr = self.pointer.at;
        let seen = self.pointer.seen;
        let ptr_lit = seen > 0.001;
        let quiet = self.quiet_rect;
        let twinkle_mul = if still { 0.0 } else { 0.16 };
        let bokeh_light_mul = if self.props.palette.dark { 1.0 } else { 0.72 };
        let (sin_f1, cos_f1) = (t * 0.2).sin_cos();
        let (sin_f2, cos_f2) = (t * 0.17).sin_cos();
        let has_perturbed = !self.field_perturbed.is_empty();
        let cell_w = w / SPATIAL_W as f32;
        let cell_h = h / SPATIAL_H as f32;

        for cell in 0..SPATIAL_CELLS {
            if self.spatial.cell_in_reach.get(cell).copied().unwrap_or(0) == 0 {
                continue;
            }
            if !has_perturbed {
                if let Some([ql, qt, qr, qb]) = quiet {
                    let gx = cell % SPATIAL_W;
                    let gy = cell / SPATIAL_W;
                    let x0 = gx as f32 * cell_w - 22.0;
                    let x1 = (gx + 1) as f32 * cell_w + 22.0;
                    let y0 = gy as f32 * cell_h - 22.0;
                    let y1 = (gy + 1) as f32 * cell_h + 22.0;
                    if x0 >= ql && x1 <= qr && y0 >= qt && y1 <= qb {
                        continue;
                    }
                }
            }

            let slice = self.spatial.cell_slice(cell);
            for &idx_u32 in slice {
                let idx = idx_u32 as usize;
                let Some(spec) = draw_specs.get(idx) else {
                    continue;
                };
                let (px, py, vx, vy) = if has_perturbed
                    && self.field_is_perturbed.get(idx).copied().unwrap_or(0) != 0
                {
                    let Some(p) = self.field.get(idx) else {
                        continue;
                    };
                    (p.x, p.y, p.vx, p.vy)
                } else {
                    let (curl_x, curl_y) = self.grids.sample_curl(spec.hx, spec.hy);
                    let float_x = (sin_f1 * spec.float_cos_x + cos_f1 * spec.float_sin_x) * 4.0;
                    let float_y = (cos_f2 * spec.float_cos_y - sin_f2 * spec.float_sin_y) * 4.0;
                    (
                        spec.hx * w + curl_x + float_x,
                        spec.hy * h + curl_y + float_y,
                        0.0,
                        0.0,
                    )
                };

                if px < -16.0 || py < -16.0 || px > w + 16.0 || py > h + 16.0 {
                    continue;
                }

                let dx_c = px - center[0];
                let dy_c = py - center[1];
                let away2 = dx_c * dx_c + dy_c * dy_c;
                if away2 >= reach_sq && away2 > core_cutoff_sq {
                    continue;
                }

                let calm = match quiet {
                    Some([ql, qt, qr, qb]) => {
                        if px >= ql && px <= qr && py >= qt && py <= qb {
                            continue;
                        }
                        let dx_q = px - px.clamp(ql, qr);
                        let dy_q = py - py.clamp(qt, qb);
                        let qd2 = dx_q * dx_q + dy_q * dy_q;
                        if qd2 >= 12_100.0 {
                            1.0
                        } else {
                            let c = smoothstep(0.0, 110.0, qd2.sqrt());
                            if c <= 0.005 {
                                continue;
                            }
                            c
                        }
                    }
                    None => 1.0,
                };

                let away = away2.sqrt();
                let fade = (1.0 - away * inv_reach).max(0.0)
                    * smoothstep(clearing - 34.0, clearing + 30.0, away);
                let core_arg = away2 * inv_core_denom;
                let core = if core_arg < 8.5 {
                    (-core_arg).exp()
                } else {
                    0.0
                };
                if fade <= 0.002 && core <= 0.015 {
                    continue;
                }

                let (cloud, i00, tx, ty) = self.grids.sample_field_cloud(px * inv_w, py * inv_h);
                let mut tone = fade * smoothstep(0.12, 0.86, cloud) * 1.45 * rich;
                if away >= clearing {
                    tone += core * 0.5 * rich;
                }

                let cx = spec.cx;
                let cy = spec.cy;
                let (radius, alpha, lit) = if spec.is_bokeh {
                    let r = spec.bokeh_radius;
                    let a = fade
                        * spec.bokeh_alpha
                        * smoothstep(0.1, 0.6, tone + 0.2)
                        * calm
                        * bokeh_light_mul
                        * opacity;
                    if r < 0.05 || a < 0.012 {
                        continue;
                    }
                    let lit = if ptr_lit {
                        let pdx = px - ptr[0];
                        let pdy = py - ptr[1];
                        let pd2 = pdx * pdx + pdy * pdy;
                        if pd2 < 230_000.0 {
                            seen * (-pd2 * (1.0 / 27_000.0)).exp()
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    (r, a, lit)
                } else {
                    if tone <= spec.cut - 0.1 {
                        continue;
                    }
                    let shown = smoothstep(spec.cut - 0.1, spec.cut + 0.1, tone);
                    if shown <= 0.01 {
                        continue;
                    }
                    let lit = if ptr_lit {
                        let pdx = px - ptr[0];
                        let pdy = py - ptr[1];
                        let pd2 = pdx * pdx + pdy * pdy;
                        if pd2 < 230_000.0 {
                            seen * (-pd2 * (1.0 / 27_000.0)).exp()
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    let speed_boost = if vx != 0.0 || vy != 0.0 {
                        0.5 * (len2(vx, vy) * (1.0 / 160.0)).min(1.0)
                    } else {
                        0.0
                    };
                    let clamped_tone = tone.clamp(0.0, 1.0);
                    let r = spec.base_radius + 0.28 * clamped_tone;
                    let mut a = shown * (0.24 + 0.46 * clamped_tone) * (1.0 + lit + speed_boost);
                    if twinkle_mul > 0.0 {
                        a *= 0.84 + twinkle_mul * fast_sin(t * (0.2 + cx * 0.5) + cy * 60.0);
                    }
                    a *= calm * opacity;
                    if r < 0.05 || a < 0.012 {
                        continue;
                    }
                    (r, a, lit)
                };

                let (warmth, other) = self.grids.sample_field_colors(i00, tx, ty);
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
                    is_bokeh: spec.is_bokeh,
                    rgb: shade,
                    alpha: alpha.min(1.0),
                });
            }
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
        let ptr_lit = seen > 0.001;
        let radius_scale = (span / 430.0).max(0.76) * 0.96;
        let w = self.pane_w;
        let h = self.pane_h;
        let mark_active = self.mark_active;
        let t_half = t * 0.5;

        for (idx, g) in grains.iter().enumerate() {
            let (px, py) = if mark_active {
                let Some(p) = self.mark.get(idx) else {
                    continue;
                };
                (center[0] + p.x, center[1] + p.y)
            } else if still {
                (center[0] + g.u * span, center[1] + g.v * span)
            } else {
                let (cx_off, cy_off) = self.grids.sample_mark_curl(g.u, g.v);
                (
                    center[0] + g.u * span + cx_off,
                    center[1] + g.v * span + cy_off,
                )
            };
            if px < -8.0 || py < -8.0 || px > w + 8.0 || py > h + 8.0 {
                continue;
            }
            let shine = if still {
                0.0
            } else {
                fast_sin(t_half + g.u * 7.0 + g.v * 9.0 + self.grids.sample_mark_shine(g.u, g.v))
            };
            let radius = g.radius * radius_scale * (1.0 + 0.03 * shine);
            let fade = smoothstep(g.cy * 0.36, g.cy * 0.36 + 0.64, shown);
            let alpha = (g.light * (0.95 + 0.05 * shine) * fade).clamp(0.0, 1.0);
            if radius < 0.05 || alpha < 0.012 {
                continue;
            }

            let lit_ptr = if ptr_lit {
                let pdx = px - ptr[0];
                let pdy = py - ptr[1];
                let pd2 = pdx * pdx + pdy * pdy;
                if pd2 < 230_000.0 {
                    seen * (-pd2 * (1.0 / 27_000.0)).exp()
                } else {
                    0.0
                }
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
                is_bokeh: false,
                rgb: shade,
                alpha,
            });
        }
    }

    fn ensure_packed(&mut self, bounds: Bounds<Pixels>, scale: f32) {
        let origin_x = f32::from(bounds.origin.x);
        let origin_y = f32::from(bounds.origin.y);
        let corner_radius = self.props.corner_radius;
        let key = (
            self.draws_gen,
            origin_x.to_bits(),
            origin_y.to_bits(),
            scale.to_bits(),
            corner_radius.to_bits(),
        );
        if self.packed_key == key {
            return;
        }
        self.packed_key = key;
        self.packed.clear();

        let pane_w = self.pane_w;
        let pane_h = self.pane_h;
        let pane_left_dev = origin_x * scale;
        let pane_top_dev = origin_y * scale;
        let pane_w_dev = f32::from(bounds.size.width) * scale;
        let pane_h_dev = f32::from(bounds.size.height) * scale;
        let corner_radius_dev = corner_radius * scale;
        let pane_corner = corner_radius + 12.0;

        let mut cur_min_x = 0.0f32;
        let mut cur_min_y = 0.0f32;
        let mut cur_max_x = 0.0f32;
        let mut cur_max_y = 0.0f32;
        let mut cur_params = [0.0f32; 16];
        let mut cur_count = 0usize;

        let flush = |packed: &mut Vec<PackedQuad>,
                     count: &mut usize,
                     min_x: f32,
                     min_y: f32,
                     max_x: f32,
                     max_y: f32,
                     params: &mut [f32; 16]| {
            if *count > 0 {
                packed.push(PackedQuad {
                    min_x,
                    min_y,
                    max_x,
                    max_y,
                    params: *params,
                });
                *count = 0;
                *params = [0.0; 16];
            }
        };

        for dot in &self.draws {
            let half = if dot.is_bokeh {
                dot.radius * 1.75 + 1.0
            } else {
                dot.radius + 1.0
            };
            let x0 = dot.cx - half;
            let y0 = dot.cy - half;
            let x1 = dot.cx + half;
            let y1 = dot.cy + half;

            let near_corner = corner_radius > 0.0
                && (dot.cx < pane_corner || dot.cx > pane_w - pane_corner)
                && (dot.cy < pane_corner || dot.cy > pane_h - pane_corner);

            if near_corner {
                let dist = outside_rounded_corner(dot.cx, dot.cy, pane_w, pane_h, corner_radius);
                if dist > half {
                    continue;
                }
                if dist > -half {
                    flush(
                        &mut self.packed,
                        &mut cur_count,
                        cur_min_x,
                        cur_min_y,
                        cur_max_x,
                        cur_max_y,
                        &mut cur_params,
                    );
                    self.packed.push(PackedQuad {
                        min_x: x0,
                        min_y: y0,
                        max_x: x1,
                        max_y: y1,
                        params: [
                            (origin_x + dot.cx) * scale,
                            (origin_y + dot.cy) * scale,
                            pack_radius_alpha(dot.radius * scale, dot.alpha, dot.is_bokeh),
                            pack_rgb(dot.rgb),
                            pane_left_dev,
                            pane_top_dev,
                            pane_w_dev,
                            pane_h_dev,
                            corner_radius_dev,
                            0.0,
                            0.0,
                            0.0,
                            0.0,
                            0.0,
                            0.0,
                            -1.0,
                        ],
                    });
                    continue;
                }
            }

            let slot = [
                (origin_x + dot.cx) * scale,
                (origin_y + dot.cy) * scale,
                pack_radius_alpha(dot.radius * scale, dot.alpha, dot.is_bokeh),
                pack_rgb(dot.rgb),
            ];

            if dot.is_bokeh {
                flush(
                    &mut self.packed,
                    &mut cur_count,
                    cur_min_x,
                    cur_min_y,
                    cur_max_x,
                    cur_max_y,
                    &mut cur_params,
                );
                let mut params = [0.0f32; 16];
                params[0] = slot[0];
                params[1] = slot[1];
                params[2] = slot[2];
                params[3] = slot[3];
                self.packed.push(PackedQuad {
                    min_x: x0,
                    min_y: y0,
                    max_x: x1,
                    max_y: y1,
                    params,
                });
                continue;
            }

            if cur_count > 0 && cur_count < 4 {
                let next_min_x = cur_min_x.min(x0);
                let next_min_y = cur_min_y.min(y0);
                let next_max_x = cur_max_x.max(x1);
                let next_max_y = cur_max_y.max(y1);
                let mw = next_max_x - next_min_x;
                let mh = next_max_y - next_min_y;
                if mw <= PACK_MAX_SIDE && mh <= PACK_MAX_SIDE && mw * mh <= PACK_MAX_AREA {
                    cur_min_x = next_min_x;
                    cur_min_y = next_min_y;
                    cur_max_x = next_max_x;
                    cur_max_y = next_max_y;
                    let base = cur_count * 4;
                    if let Some(dst) = cur_params.get_mut(base..base + 4) {
                        dst.copy_from_slice(&slot);
                    }
                    cur_count += 1;
                    if cur_count == 4 {
                        flush(
                            &mut self.packed,
                            &mut cur_count,
                            cur_min_x,
                            cur_min_y,
                            cur_max_x,
                            cur_max_y,
                            &mut cur_params,
                        );
                    }
                    continue;
                }
                flush(
                    &mut self.packed,
                    &mut cur_count,
                    cur_min_x,
                    cur_min_y,
                    cur_max_x,
                    cur_max_y,
                    &mut cur_params,
                );
            }

            cur_min_x = x0;
            cur_min_y = y0;
            cur_max_x = x1;
            cur_max_y = y1;
            cur_params[0] = slot[0];
            cur_params[1] = slot[1];
            cur_params[2] = slot[2];
            cur_params[3] = slot[3];
            cur_count = 1;
        }

        flush(
            &mut self.packed,
            &mut cur_count,
            cur_min_x,
            cur_min_y,
            cur_max_x,
            cur_max_y,
            &mut cur_params,
        );
    }

    fn paint_glow(&mut self, bounds: Bounds<Pixels>, t: f32, mood: &Mood, window: &mut Window) {
        let w = self.pane_w;
        let h = self.pane_h;
        let origin_x = f32::from(bounds.origin.x);
        let origin_y = f32::from(bounds.origin.y);
        let ground_scale = window.ground_scale().max(1.0);
        let opacity = self.props.glow;
        let seen = self.pointer.seen;
        let ptr = self.pointer.at;

        // Quantize slow glow updates to ~20Hz so macOS's ground-layer cache skips redrawing on 5/6 frames at 120Hz.
        let key = GlowCacheKey {
            t_bucket: (t / GLOW_UPDATE_STEP).floor() as i32,
            seen_q: (seen.clamp(0.0, 1.0) * 64.0).round() as u16,
            ptr_x: if seen > 0.001 {
                (ptr[0] * 0.5).round() as i32
            } else {
                0
            },
            ptr_y: if seen > 0.001 {
                (ptr[1] * 0.5).round() as i32
            } else {
                0
            },
            opacity_q: (opacity.clamp(0.0, 1.0) * 255.0).round() as u16,
            w_bits: w.to_bits(),
            h_bits: h.to_bits(),
            origin_x_bits: origin_x.to_bits(),
            origin_y_bits: origin_y.to_bits(),
            scale_bits: ground_scale.to_bits(),
            corner_bits: self.props.corner_radius.to_bits(),
            tint_packed: pack_rgb(mood.tint) as u32,
            deep_packed: pack_rgb(mood.deep) as u32,
            alt_packed: pack_rgb(mood.alt) as u32,
        };

        if self.glow_cache_key != Some(key) {
            self.glow_cache_key = Some(key);
            self.glow_quads.clear();

            let cols = ((w / GLOW_CELL_TARGET).ceil() as usize).clamp(12, GLOW_MAX_COLS);
            let rows = ((h / GLOW_CELL_TARGET).ceil() as usize).clamp(8, GLOW_MAX_ROWS);
            let stride = cols + 1;
            let total = stride * (rows + 1);
            self.glow_corners.resize(total, [0.0; 3]);

            let glow_t = key.t_bucket as f32 * GLOW_UPDATE_STEP;
            let diag = len2(w, h);
            let center = self.mark_center;
            let span = self.mark_span.max(60.0);
            let glow_gain = 0.16_f32;

            let w0 = (span * 1.6).max(diag * 0.22);
            let w1 = span * 0.72;
            let w2 = (diag * 0.46).max(1.0);
            let w3 = (diag * 0.34).max(1.0);
            let inv_w0_sq = 1.0 / (w0 * w0);
            let inv_w1_sq = 1.0 / (w1 * w1);
            let inv_w2_sq = 1.0 / (w2 * w2);
            let inv_w3_sq = 1.0 / (w3 * w3);
            let inv_ptr_sq = 1.0 / (230.0 * 230.0);
            let p2_origin = [w * 0.12, h * 1.04];
            let p3_origin = [w * 0.98, h * 1.02];

            let deep_tint = [
                mood.deep[0] * 0.62 + mood.tint[0] * 0.16,
                mood.deep[1] * 0.62 + mood.tint[1] * 0.16,
                mood.deep[2] * 0.62 + mood.tint[2] * 0.16,
            ];
            let cell_w = w / cols as f32;
            let cell_h = h / rows as f32;

            for gy in 0..=rows {
                let ly = gy as f32 * cell_h;
                let dy_c = ly - center[1];
                let dy_c2 = dy_c * dy_c;
                let dy_2 = ly - p2_origin[1];
                let dy_22 = dy_2 * dy_2;
                let dy_3 = ly - p3_origin[1];
                let dy_32 = dy_3 * dy_3;
                let dy_p = ly - ptr[1];
                let dy_p2 = dy_p * dy_p;
                let row_base = gy * stride;
                for gx in 0..=cols {
                    let lx = gx as f32 * cell_w;
                    let slant_x = 0.8 * lx + 0.6 * ly;
                    let slant_y = -0.6 * lx + 0.8 * ly;
                    let s1x = slant_x * (1.0 / 260.0);
                    let s1y = slant_y * (1.0 / 260.0);
                    let cloud = 0.5
                        + 0.28 * (s1x + glow_t * 0.08 + 0.7 * (s1y - glow_t * 0.06).cos()).sin()
                        + 0.22 * (s1y * 1.15 - glow_t * 0.05 + 0.6 * (s1x * 0.85).sin()).cos();
                    let s2x = slant_x * (1.0 / 340.0);
                    let s2y = slant_y * (1.0 / 340.0);
                    let second = 0.5
                        + 0.28 * (s2x - glow_t * 0.06 + 0.7 * (s2y + glow_t * 0.07).sin()).cos()
                        + 0.22 * (s2y * 1.1 + glow_t * 0.05).sin();

                    let dx_c = lx - center[0];
                    let d_c2 = dx_c * dx_c + dy_c2;
                    let l0 = (-d_c2 * inv_w0_sq).exp() * (0.5 + cloud);
                    let l1 = (-d_c2 * inv_w1_sq).exp() * (0.62 + 0.55 * cloud) * 0.30;

                    let dx_2 = lx - p2_origin[0];
                    let l2 =
                        (-(dx_2 * dx_2 + dy_22) * inv_w2_sq).exp() * (0.4 + 1.1 * second) * 0.10;

                    let dx_3 = lx - p3_origin[0];
                    let l3 = (-(dx_3 * dx_3 + dy_32) * inv_w3_sq).exp() * (0.35 + second) * 0.22;

                    let lp = if seen > 0.001 {
                        let dx_p = lx - ptr[0];
                        let d_p2 = dx_p * dx_p + dy_p2;
                        if d_p2 < 450_000.0 {
                            0.14 * seen * (-d_p2 * inv_ptr_sq).exp()
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                    let light = [
                        (deep_tint[0] * l0
                            + mood.tint[0] * (l1 + lp)
                            + mood.alt[0] * l2
                            + mood.deep[0] * l3)
                            * glow_gain,
                        (deep_tint[1] * l0
                            + mood.tint[1] * (l1 + lp)
                            + mood.alt[1] * l2
                            + mood.deep[1] * l3)
                            * glow_gain,
                        (deep_tint[2] * l0
                            + mood.tint[2] * (l1 + lp)
                            + mood.alt[2] * l2
                            + mood.deep[2] * l3)
                            * glow_gain,
                    ];
                    if let Some(slot) = self.glow_corners.get_mut(row_base + gx) {
                        *slot = light;
                    }
                }
            }

            let corner_radius_dev = self.props.corner_radius * ground_scale;
            let t_fract = glow_t.fract();
            let inv_scale = 1.0 / ground_scale;
            let snap_x = |gx: usize| -> f32 {
                if gx == cols {
                    w
                } else {
                    ((origin_x + gx as f32 * cell_w) * ground_scale).round() * inv_scale - origin_x
                }
            };
            let snap_y = |gy: usize| -> f32 {
                if gy == rows {
                    h
                } else {
                    ((origin_y + gy as f32 * cell_h) * ground_scale).round() * inv_scale - origin_y
                }
            };

            for gy in 0..rows {
                let y0 = snap_y(gy);
                let y1 = snap_y(gy + 1);
                let r0 = gy * stride;
                let r1 = (gy + 1) * stride;
                for gx in 0..cols {
                    let (Some(&c00), Some(&c10), Some(&c01), Some(&c11)) = (
                        self.glow_corners.get(r0 + gx),
                        self.glow_corners.get(r0 + gx + 1),
                        self.glow_corners.get(r1 + gx),
                        self.glow_corners.get(r1 + gx + 1),
                    ) else {
                        continue;
                    };
                    let peak = c00[0]
                        .max(c00[1])
                        .max(c00[2])
                        .max(c10[0])
                        .max(c10[1])
                        .max(c10[2])
                        .max(c01[0])
                        .max(c01[1])
                        .max(c01[2])
                        .max(c11[0])
                        .max(c11[1])
                        .max(c11[2]);
                    if peak * opacity <= 0.0015 {
                        continue;
                    }
                    let x0 = snap_x(gx);
                    let x1 = snap_x(gx + 1);
                    let (cr, cid) = if corner_radius_dev > 0.0 {
                        match (gx == 0, gx + 1 == cols, gy == 0, gy + 1 == rows) {
                            (true, _, true, _) => (corner_radius_dev, 1.0),
                            (_, true, true, _) => (corner_radius_dev, 2.0),
                            (true, _, _, true) => (corner_radius_dev, 3.0),
                            (_, true, _, true) => (corner_radius_dev, 4.0),
                            _ => (0.0, 0.0),
                        }
                    } else {
                        (0.0, 0.0)
                    };
                    self.glow_quads.push(GlowQuad {
                        x0,
                        y0,
                        w: x1 - x0,
                        h: y1 - y0,
                        params: [
                            c00[0], c00[1], c00[2], opacity, c10[0], c10[1], c10[2], t_fract,
                            c01[0], c01[1], c01[2], cr, c11[0], c11[1], c11[2], cid,
                        ],
                    });
                }
            }
        }

        if self.glow_quads.is_empty() {
            return;
        }
        window.paint_layer(bounds, |window| {
            for quad in &self.glow_quads {
                let cell_bounds = Bounds::new(
                    point(px(origin_x + quad.x0), px(origin_y + quad.y0)),
                    size(px(quad.w), px(quad.h)),
                );
                let _ = window.paint_ground_shader(cell_bounds, &WELCOME_GLOW, quad.params, None);
            }
        });
    }

    fn paint_all(&mut self, bounds: Bounds<Pixels>, t: f32, mood: &Mood, window: &mut Window) {
        let scale = window.scale_factor().max(1.0);
        let origin_x = f32::from(bounds.origin.x);
        let origin_y = f32::from(bounds.origin.y);

        if self.props.show_field && self.props.palette.dark && self.props.glow > 0.001 {
            self.paint_glow(bounds, t, mood, window);
        }

        self.ensure_packed(bounds, scale);
        if self.packed.is_empty() {
            return;
        }

        // One layer assigns every dot quad the same draw order: 1 BoundsTree insert and 1 Metal instanced draw call.
        window.paint_layer(bounds, |window| {
            for quad in &self.packed {
                let dot_bounds = Bounds::new(
                    point(px(origin_x + quad.min_x), px(origin_y + quad.min_y)),
                    size(px(quad.max_x - quad.min_x), px(quad.max_y - quad.min_y)),
                );
                let _ = window.paint_raw_shader(dot_bounds, &WELCOME_DOT, quad.params, None);
            }
        });
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
                    let should_repaint =
                        view.update(cx, |dots, _| dots.handle_mouse_move(event.position, bounds));
                    if should_repaint {
                        cx.repaint(view.entity_id());
                    }
                });

                let active = window.is_window_active() && window.is_visible();
                let (t, mood, keep_going) = strong.update(cx, |dots, _| {
                    let (t, mood) = dots.step_and_collect(bounds, active);
                    (t, mood, dots.awake)
                });

                let t_emit = Instant::now();
                strong.update(cx, |dots, _| dots.paint_all(bounds, t, &mood, window));
                let emit_us = t_emit.elapsed().as_micros() as u64;
                enum NextPace {
                    None,
                    Vsync,
                    Ambient,
                }
                let pace = strong.update(cx, |dots, _| {
                    dots.last_emit_us = emit_us;
                    if keep_going && active && !dots.props.reduce_motion {
                        if dots.wants_full_vsync() {
                            dots.ambient_task = None;
                            if !dots.frame_scheduled {
                                dots.frame_scheduled = true;
                                NextPace::Vsync
                            } else {
                                NextPace::None
                            }
                        } else if !dots.frame_scheduled && dots.ambient_task.is_none() {
                            NextPace::Ambient
                        } else {
                            NextPace::None
                        }
                    } else {
                        dots.ambient_task = None;
                        NextPace::None
                    }
                });

                #[cfg(feature = "drive")]
                if keep_going {
                    WELCOME_PRESENTS.fetch_add(1, Ordering::Relaxed);
                }

                match pace {
                    NextPace::Vsync => {
                        let next_weak = entity.clone();
                        let id = strong.entity_id();
                        window.on_next_frame(move |_, cx| {
                            if let Some(view) = next_weak.upgrade() {
                                view.update(cx, |dots, _| dots.frame_scheduled = false);
                                cx.repaint(id);
                            }
                        });
                    }
                    NextPace::Ambient => {
                        let next_weak = entity.clone();
                        let id = strong.entity_id();
                        let task = cx.spawn(async move |cx| {
                            cx.background_executor().timer(AMBIENT_FRAME_INTERVAL).await;
                            cx.update(|cx| {
                                if let Some(view) = next_weak.upgrade() {
                                    let still_awake = view.update(cx, |dots, _| {
                                        dots.ambient_task = None;
                                        dots.awake && !dots.props.reduce_motion
                                    });
                                    if still_awake {
                                        cx.repaint(id);
                                    }
                                }
                            });
                        });
                        strong.update(cx, |dots, _| dots.ambient_task = Some(task));
                    }
                    NextPace::None => {
                        if !(keep_going && active && !cx.reduce_motion()) {
                            sync_awake_flag(cx);
                        }
                    }
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
            cx.repaint(view.entity_id());
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
            cx.repaint(view.entity_id());
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
        let should_repaint = view.update(cx, |dots, _| {
            if !dots.props.reduce_motion && dots.now() < dots.until {
                dots.awake = true;
                dots.last_frame = None;
                !dots.frame_scheduled
            } else {
                false
            }
        });
        if should_repaint {
            cx.repaint(view.entity_id());
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
            dots.dirty = true;
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

pub fn bench_packed_count(cx: &App) -> usize {
    cx.try_global::<WelcomeRegistry>()
        .map(|reg| reg.views.values().map(|v| v.read(cx).packed.len()).sum())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn test_palette() -> Palette {
        Palette {
            tint: [0.635, 0.467, 1.0],
            deep: [0.23, 0.17, 0.52],
            alt: [0.4, 0.85, 0.72],
            bright: [0.95, 0.92, 1.0],
            dark: true,
        }
    }

    fn test_props() -> Props {
        Props {
            palette: test_palette(),
            glow: 1.0,
            show_field: true,
            reduce_motion: false,
            corner_radius: 10.0,
        }
    }

    #[test]
    fn fast_sin_matches_std_within_tolerance() {
        for i in -1000..=1000 {
            let x = i as f32 * 0.037;
            let diff = (fast_sin(x) - x.sin()).abs();
            assert!(diff < 0.0015, "fast_sin({x}) diverged by {diff}");
        }
    }

    #[test]
    fn packed_dot_roundtrips_radius_alpha_and_rgb() {
        for &(r, a, bokeh) in &[
            (1.25_f32, 0.75_f32, false),
            (0.48_f32, 0.02_f32, false),
            (16.5_f32, 0.12_f32, true),
        ] {
            let packed = pack_radius_alpha(r, a, bokeh);
            assert_eq!(packed < 0.0, bokeh);
            let abs = packed.abs();
            let alpha_q = (abs / 16384.0).floor();
            let r_q = abs - alpha_q * 16384.0;
            let got_a = alpha_q / 1023.0;
            let got_r = r_q / 256.0;
            assert!((got_a - a).abs() < 0.0015, "alpha {a} -> {got_a}");
            assert!((got_r - r).abs() < 0.005, "radius {r} -> {got_r}");
        }

        let rgb = [0.635_f32, 0.467_f32, 1.0_f32];
        let packed = pack_rgb(rgb);
        let r = (packed / 65536.0).floor();
        let rem = packed - r * 65536.0;
        let g = (rem / 256.0).floor();
        let b = rem - g * 256.0;
        assert!((r / 255.0 - rgb[0]).abs() < 0.005);
        assert!((g / 255.0 - rgb[1]).abs() < 0.005);
        assert!((b / 255.0 - rgb[2]).abs() < 0.005);
    }

    #[test]
    fn retina_4k_pointer_coalescing_packing_and_idle_sleep() {
        let mut dots = WelcomeDots::new(test_props());
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(2560.0), px(1440.0)));

        // Coalesce 200 high-rate pointer events before stepping once: only perturbed particles step.
        dots.step_and_collect(bounds, true);
        assert_eq!(dots.field_used, MOST_IN_FIELD);
        assert!(dots.field_perturbed.is_empty());
        assert!(
            !dots.wants_full_vsync(),
            "calm ambient drift should pace at 30Hz instead of full 120Hz VSync"
        );

        let base_t = dots.now();
        for i in 0..200 {
            let pos = point(px(1200.0 + i as f32 * 0.5), px(520.0 + i as f32 * 0.2));
            let _ = dots.handle_mouse_move(pos, bounds);
        }
        assert!(
            dots.wants_full_vsync(),
            "pointer interaction must immediately switch to full native VSync"
        );
        dots.last_frame = Some(base_t - 0.016);
        dots.step_and_collect(bounds, true);
        assert!(
            !dots.field_perturbed.is_empty(),
            "pointer sweep must perturb nearby field particles"
        );
        assert!(
            dots.field_perturbed.len() < dots.field_used / 2,
            "spatial perturbation must stay local ({} of {})",
            dots.field_perturbed.len(),
            dots.field_used
        );

        dots.ensure_packed(bounds, 2.0);
        assert!(
            !dots.draws.is_empty() && !dots.packed.is_empty(),
            "welcome screen must emit draws and packed quads"
        );
        assert!(
            dots.packed.len() < dots.draws.len(),
            "tight spatial packing should merge nearby dots ({} quads for {} dots)",
            dots.packed.len(),
            dots.draws.len()
        );

        // Once past until + coast, animation must go completely to sleep.
        dots.until = dots.now() - MAX_COAST_AFTER_UNTIL - 1.0;
        dots.pointer.here = false;
        dots.pointer.seen = 0.0;
        dots.last_frame = Some(dots.now() - 0.016);
        dots.step_and_collect(bounds, true);
        assert!(!dots.awake, "welcome particles must sleep once settled");
    }

    #[gpui::test]
    fn repaint_stays_isolated_without_rerendering_stage(cx: &mut gpui::TestAppContext) {
        struct StageHost {
            dots: Entity<WelcomeDots>,
            renders: Rc<Cell<usize>>,
        }

        impl Render for StageHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                self.renders.set(self.renders.get() + 1);
                gpui::div()
                    .relative()
                    .size_full()
                    .child(
                        gpui::div()
                            .absolute()
                            .inset_0()
                            .child(self.dots.clone().into_element().isolated()),
                    )
                    .child(mark_placeholder(116.0))
            }
        }

        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            let renders = Rc::new(Cell::new(0));
            let (host, wcx) = cx.add_window_view(|window, cx| {
                window.set_scale_factor(scale);
                window.resize(size(px(1440.0), px(900.0)));
                let dots = cx.new(|_| WelcomeDots::new(test_props()));
                StageHost {
                    dots,
                    renders: renders.clone(),
                }
            });
            wcx.run_until_parked();
            let baseline_renders = renders.get();
            assert!(baseline_renders >= 1);

            let dots = host.read_with(wcx, |host, _| host.dots.clone());
            for frame in 0..8 {
                dots.update(wcx, |d, _| {
                    let now = d.now();
                    d.pointer.move_to([600.0 + frame as f32 * 18.0, 320.0], now);
                    d.pointer.seen = 1.0;
                    d.last_frame = Some(now - 0.016);
                    d.awake = true;
                });
                let id = dots.entity_id();
                wcx.update(|_, cx| cx.repaint(id));
                wcx.run_until_parked();
                assert_eq!(
                    renders.get(),
                    baseline_renders,
                    "WelcomeDots animation frame must repaint in isolation without re-rendering parent stage (scale={scale}, frame={frame})"
                );
            }
        }
    }
}
