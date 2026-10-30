//! Frames a window filmed offscreen the way a post shows it: on a wallpaper with a soft shadow,
//! seen through a camera that can move in on any part of it, with a drawn cursor on top and a
//! closing card at the end. Pictures are added one at a time and averaged into a frame, so two
//! pictures a frame give fast moves a little motion blur.

use std::collections::HashMap;
use std::path::Path;

use image::{imageops, RgbaImage};
use resvg::{tiny_skia, usvg};

/// What the camera sees: a rectangle of the stage, in stage points, as wide as `width` and as
/// tall as the stage's shape makes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f32,
    pub y: f32,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    Arrow,
    Hand,
    Beam,
}

/// The drawn cursor, in window points. `press` runs from 0, up, to 1, held down.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub x: f32,
    pub y: f32,
    pub press: f32,
    pub opacity: f32,
    pub shape: Shape,
}

pub struct Setup<'a> {
    /// The stage and the window on it, in points; the window sits in its middle.
    pub stage: (f32, f32),
    pub window: (f32, f32),
    /// Pixels a point in the window's pictures, which the stage is drawn at too.
    pub density: f32,
    /// The size of each finished frame, in pixels.
    pub output: (u32, u32),
    pub wallpaper: Option<&'a Path>,
    /// The picture the closing card shows, such as the brand's lockup.
    pub lockup: Option<&'a Path>,
}

pub struct Picture<'a> {
    pub window: &'a RgbaImage,
    pub view: View,
    pub pointer: Option<Pointer>,
    /// How far the closing card has come in, from 0 to 1.
    pub end: f32,
    /// Draws lines every 50 window points, numbered every 100, for placing a scene's moves.
    pub grid: bool,
}

const RADIUS: f32 = 12.;
/// How much larger than a real pointer the drawn one is.
const POINTER_SCALE: f32 = 1.4;

const ARROW: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -2 22 30" width="22" height="30"><path d="M0 0v22.6l5.3-5.1 3.6 8.4 3.5-1.5-3.6-8.3H16L0 0z" fill="#000" stroke="#fff" stroke-width="1.6" stroke-linejoin="round"/></svg>"##;
const HAND: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-1 -1 26 30" width="26" height="30"><path d="M8.6 1.2c-1.2 0-2.1.9-2.1 2.1v10.3l-1.8-1.9c-.9-.9-2.2-1-3.1-.2-.9.8-1 2-.3 3l4.3 6c1.6 2.2 4 3.6 6.8 3.6h2.4c3.9 0 7-3.1 7-7v-5.2c0-1.2-.9-2.1-2.1-2.1s-2.1.9-2.1 2.1v-1c0-1.2-.9-2.1-2.1-2.1s-2.1.9-2.1 2.1v-.9c0-1.2-.9-2.1-2.1-2.1s-2.1.9-2.1 2.1V3.3c0-1.2-.9-2.1-2.1-2.1z" fill="#fff" stroke="#000" stroke-width="1.4" stroke-linejoin="round"/><path d="M12.8 15.8v3M16.9 15.8v3M21 15.8v2.6" stroke="#000" stroke-width="1.2" stroke-linecap="round"/></svg>"##;
const BEAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-3 -2 14 28" width="14" height="28"><path d="M0 0c2.4 0 3.6.8 4 2.2C4.4.8 5.6 0 8 0M4 2.2v19.6M0 24c2.4 0 3.6-.8 4-2.2.4 1.4 1.6 2.2 4 2.2M1.6 12h4.8" fill="none" stroke="#fff" stroke-width="3.4" stroke-linecap="round"/><path d="M0 0c2.4 0 3.6.8 4 2.2C4.4.8 5.6 0 8 0M4 2.2v19.6M0 24c2.4 0 3.6-.8 4-2.2.4 1.4 1.6 2.2 4 2.2M1.6 12h4.8" fill="none" stroke="#000" stroke-width="1.5" stroke-linecap="round"/></svg>"##;

impl Shape {
    fn art(self) -> (&'static str, (f32, f32)) {
        match self {
            Shape::Arrow => (ARROW, (2., 2.)),
            Shape::Hand => (HAND, (9.6, 2.2)),
            Shape::Beam => (BEAM, (7., 14.)),
        }
    }
}

/// A pointer drawn at one size, with its shadow, premultiplied, and where its hot spot is.
struct Art {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    hot: (f32, f32),
}

pub struct Compositor {
    stage: (f32, f32),
    density: f32,
    size: (usize, usize),
    origin: (usize, usize),
    window_size: (usize, usize),
    plain: Vec<u8>,
    shadowed: Vec<u8>,
    canvas: Vec<u8>,
    /// Whether the canvas outside the window still shows the closing card's last picture.
    dirty: bool,
    mask: Vec<f32>,
    rim: Vec<f32>,
    lockup: Option<RgbaImage>,
    output: (usize, usize),
    frame: Vec<u8>,
    sum: Vec<u16>,
    added: u16,
    pointers: HashMap<(Shape, u32), Art>,
}

impl Compositor {
    pub fn new(setup: Setup) -> Self {
        let d = setup.density;
        let size = (
            (setup.stage.0 * d).round() as usize,
            (setup.stage.1 * d).round() as usize,
        );
        let window_size = (
            (setup.window.0 * d).round() as usize,
            (setup.window.1 * d).round() as usize,
        );
        let origin = ((size.0 - window_size.0) / 2, (size.1 - window_size.1) / 2);
        let plain = wallpaper(setup.wallpaper, size);
        let mut shadowed = plain.clone();
        for (offset, blur, strength) in [(30., 80., 0.55), (8., 24., 0.35)] {
            let mut alpha = vec![0f32; size.0 * size.1];
            let rect = (
                origin.0 as f32,
                origin.1 as f32 + offset * d,
                window_size.0 as f32,
                window_size.1 as f32,
            );
            for y in 0..size.1 {
                for x in 0..size.0 {
                    alpha[y * size.0 + x] =
                        coverage(x as f32 + 0.5, y as f32 + 0.5, rect, RADIUS * d);
                }
            }
            gaussian(&mut alpha, size.0, size.1, blur * d / 2.);
            for (pixel, alpha) in shadowed.as_chunks_mut::<4>().0.iter_mut().zip(&alpha) {
                let keep = 1. - alpha * strength;
                for channel in &mut pixel[..3] {
                    *channel = (*channel as f32 * keep).round() as u8;
                }
            }
        }
        let mut mask = vec![0f32; window_size.0 * window_size.1];
        let mut rim = vec![0f32; window_size.0 * window_size.1];
        let rect = (0., 0., window_size.0 as f32, window_size.1 as f32);
        for y in 0..window_size.1 {
            for x in 0..window_size.0 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let at = y * window_size.0 + x;
                mask[at] = coverage(px, py, rect, RADIUS * d);
                let edge = distance(px, py, rect, RADIUS * d);
                rim[at] = (1. - (edge + d * 0.5).abs() / (d * 0.5)).clamp(0., 1.) * 0.09;
            }
        }
        let lockup = setup
            .lockup
            .and_then(|path| image::open(path).ok())
            .map(|picture| {
                let picture = picture.to_rgba8();
                let width = (setup.stage.0 * 0.3 * d) as u32;
                let height =
                    (picture.height() as f32 * width as f32 / picture.width() as f32) as u32;
                imageops::resize(&picture, width, height, imageops::FilterType::Lanczos3)
            });
        let output = (setup.output.0 as usize, setup.output.1 as usize);
        Self {
            stage: setup.stage,
            density: d,
            size,
            origin,
            window_size,
            canvas: shadowed.clone(),
            plain,
            shadowed,
            dirty: false,
            mask,
            rim,
            lockup,
            output,
            frame: vec![0; output.0 * output.1 * 4],
            sum: vec![0; output.0 * output.1 * 4],
            added: 0,
            pointers: HashMap::new(),
        }
    }

    /// Where the window's top left corner is on the stage, in points.
    pub fn window_origin(&self) -> (f32, f32) {
        (
            self.origin.0 as f32 / self.density,
            self.origin.1 as f32 / self.density,
        )
    }

    pub fn add(&mut self, picture: Picture) {
        self.paint_canvas(picture.window, picture.end);
        let d = self.density;
        let height = picture.view.width * self.stage.1 / self.stage.0;
        resample(
            &self.canvas,
            self.size,
            (
                picture.view.x as f64 * d as f64,
                picture.view.y as f64 * d as f64,
                picture.view.width as f64 * d as f64,
                height as f64 * d as f64,
            ),
            &mut self.frame,
            self.output,
        );
        let scale = self.output.0 as f32 / picture.view.width;
        let (ox, oy) = self.window_origin();
        let on_screen = |x: f32, y: f32| {
            (
                (ox + x - picture.view.x) * scale,
                (oy + y - picture.view.y) * scale,
            )
        };
        if picture.grid {
            self.paint_grid(&on_screen, scale);
        }
        if let Some(pointer) = picture.pointer.filter(|pointer| pointer.opacity > 0.) {
            let (x, y) = on_screen(pointer.x, pointer.y);
            let size = POINTER_SCALE * scale * (1. - 0.18 * pointer.press);
            self.paint_pointer(pointer.shape, size, (x, y), pointer.opacity);
        }
        for (sum, value) in self.sum.iter_mut().zip(&self.frame) {
            *sum += *value as u16;
        }
        self.added += 1;
    }

    /// The pictures added since the last frame, averaged.
    pub fn take(&mut self) -> &[u8] {
        let added = self.added.max(1);
        for (value, sum) in self.frame.iter_mut().zip(self.sum.iter_mut()) {
            *value = ((*sum + added / 2) / added) as u8;
            *sum = 0;
        }
        self.added = 0;
        &self.frame
    }

    fn paint_canvas(&mut self, window: &RgbaImage, end: f32) {
        let shown = 1. - smooth((end / 0.6).clamp(0., 1.));
        let card = smooth(((end - 0.35) / 0.65).clamp(0., 1.));
        let window =
            if window.dimensions() == (self.window_size.0 as u32, self.window_size.1 as u32) {
                std::borrow::Cow::Borrowed(window)
            } else {
                std::borrow::Cow::Owned(imageops::resize(
                    window,
                    self.window_size.0 as u32,
                    self.window_size.1 as u32,
                    imageops::FilterType::Triangle,
                ))
            };
        let row = self.size.0 * 4;
        let full = end > 0. || self.dirty;
        let (origin, window_size, plain, shadowed) =
            (self.origin, self.window_size, &self.plain, &self.shadowed);
        let (mask, rim, raw) = (&self.mask, &self.rim, window.as_raw());
        rows(&mut self.canvas, row, |y, line| {
            let inside = y >= origin.1 && y < origin.1 + window_size.1;
            if !full && !inside {
                return;
            }
            let base = y * row;
            if full {
                for (at, value) in line.iter_mut().enumerate() {
                    let (a, b) = (plain[base + at] as f32, shadowed[base + at] as f32);
                    *value = (a + (b - a) * shown).round() as u8;
                }
            } else {
                let span = origin.0 * 4..(origin.0 + window_size.0) * 4;
                line[span.clone()].copy_from_slice(&shadowed[base + span.start..base + span.end]);
            }
            if !inside || shown <= 0. {
                return;
            }
            let wy = y - origin.1;
            for wx in 0..window_size.0 {
                let at = wy * window_size.0 + wx;
                let source = &raw[at * 4..at * 4 + 4];
                let cover = mask[at] * shown * source[3] as f32 / 255.;
                let lit = rim[at] * shown;
                let pixel = &mut line[(origin.0 + wx) * 4..(origin.0 + wx) * 4 + 4];
                for channel in 0..3 {
                    let under = pixel[channel] as f32;
                    let mixed = under + (source[channel] as f32 - under) * cover;
                    pixel[channel] = (mixed + (255. - mixed) * lit).round() as u8;
                }
            }
        });
        if card > 0. {
            if let Some(lockup) = &self.lockup {
                let (w, h) = (lockup.width() as usize, lockup.height() as usize);
                let left = (self.size.0 - w) / 2;
                // High in the frame, in the wallpaper's sky rather than over its subject.
                let top = (self.size.1 as f32 * 0.19 - h as f32 / 2.
                    + (1. - card) * 14. * self.density) as usize;
                for (y, line) in lockup.as_raw().chunks_exact(w * 4).enumerate() {
                    let start = ((top + y) * self.size.0 + left) * 4;
                    let target = &mut self.canvas[start..start + w * 4];
                    for (pixel, source) in target
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(line.as_chunks::<4>().0)
                    {
                        let cover = source[3] as f32 / 255. * card;
                        for channel in 0..3 {
                            let under = pixel[channel] as f32;
                            pixel[channel] =
                                (under + (source[channel] as f32 - under) * cover).round() as u8;
                        }
                    }
                }
            }
        }
        self.dirty = end > 0.;
    }

    fn paint_pointer(&mut self, shape: Shape, size: f32, at: (f32, f32), opacity: f32) {
        let key = (shape, (size * 20.).round() as u32);
        let art = self
            .pointers
            .entry(key)
            .or_insert_with(|| draw_pointer(shape, key.1 as f32 / 20.));
        let left = (at.0 - art.hot.0).round() as isize;
        let top = (at.1 - art.hot.1).round() as isize;
        let (width, height) = (self.output.0 as isize, self.output.1 as isize);
        for y in 0..art.height as isize {
            let ty = top + y;
            if ty < 0 || ty >= height {
                continue;
            }
            for x in 0..art.width as isize {
                let tx = left + x;
                if tx < 0 || tx >= width {
                    continue;
                }
                let source = &art.pixels[(y as usize * art.width + x as usize) * 4..][..4];
                let alpha = source[3] as f32 / 255. * opacity;
                if alpha <= 0. {
                    continue;
                }
                let target = &mut self.frame[((ty * width + tx) * 4) as usize..][..4];
                for channel in 0..3 {
                    target[channel] = (source[channel] as f32 * opacity
                        + target[channel] as f32 * (1. - alpha))
                        .round()
                        .min(255.) as u8;
                }
            }
        }
    }

    fn paint_grid(&mut self, on_screen: &dyn Fn(f32, f32) -> (f32, f32), scale: f32) {
        let (ww, wh) = (
            self.window_size.0 as f32 / self.density,
            self.window_size.1 as f32 / self.density,
        );
        let (width, height) = self.output;
        let line = |frame: &mut [u8], x0: f32, y0: f32, x1: f32, y1: f32, strength: f32| {
            let steps = ((x1 - x0).abs().max((y1 - y0).abs())) as usize;
            for step in 0..=steps {
                let t = step as f32 / steps.max(1) as f32;
                let (x, y) = ((x0 + (x1 - x0) * t) as isize, (y0 + (y1 - y0) * t) as isize);
                if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
                    continue;
                }
                let pixel = &mut frame[(y as usize * width + x as usize) * 4..][..4];
                pixel[0] = (pixel[0] as f32 + (255. - pixel[0] as f32) * strength) as u8;
                pixel[1] = (pixel[1] as f32 * (1. - strength)) as u8;
                pixel[2] = (pixel[2] as f32 + (255. - pixel[2] as f32) * strength) as u8;
            }
        };
        let digit_size = (scale * 1.6).max(2.);
        let mut gx = 0.;
        while gx <= ww {
            let (x, top) = on_screen(gx, 0.);
            let (_, bottom) = on_screen(gx, wh);
            let major = gx % 100. == 0.;
            line(
                &mut self.frame,
                x,
                top,
                x,
                bottom,
                if major { 0.55 } else { 0.25 },
            );
            if major {
                digits(
                    &mut self.frame,
                    width,
                    height,
                    gx as u32,
                    x + 3.,
                    top + 3.,
                    digit_size,
                );
            }
            gx += 50.;
        }
        let mut gy = 0.;
        while gy <= wh {
            let (left, y) = on_screen(0., gy);
            let (right, _) = on_screen(ww, gy);
            let major = gy % 100. == 0.;
            line(
                &mut self.frame,
                left,
                y,
                right,
                y,
                if major { 0.55 } else { 0.25 },
            );
            if major && gy > 0. {
                digits(
                    &mut self.frame,
                    width,
                    height,
                    gy as u32,
                    left + 3.,
                    y + 3.,
                    digit_size,
                );
            }
            gy += 50.;
        }
    }
}

fn smooth(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

/// How far a point is outside a rounded rectangle, negative inside.
fn distance(x: f32, y: f32, rect: (f32, f32, f32, f32), radius: f32) -> f32 {
    let (half_w, half_h) = (rect.2 / 2., rect.3 / 2.);
    let (cx, cy) = (rect.0 + half_w, rect.1 + half_h);
    let qx = (x - cx).abs() - (half_w - radius);
    let qy = (y - cy).abs() - (half_h - radius);
    let outside = (qx.max(0.).powi(2) + qy.max(0.).powi(2)).sqrt();
    outside + qx.max(qy).min(0.) - radius
}

fn coverage(x: f32, y: f32, rect: (f32, f32, f32, f32), radius: f32) -> f32 {
    (0.5 - distance(x, y, rect, radius)).clamp(0., 1.)
}

/// Blurs `values` about as a Gaussian of `sigma` would, with three box blurs each way.
fn gaussian(values: &mut [f32], width: usize, height: usize, sigma: f32) {
    let ideal = (12. * sigma * sigma / 3. + 1.).sqrt();
    let radius = ((ideal - 1.) / 2.).round().max(1.) as usize;
    let mut scratch = vec![0f32; values.len()];
    for _ in 0..3 {
        box_pass(values, &mut scratch, width, height, radius, 1, width);
        box_pass(&scratch, values, height, width, radius, width, 1);
    }
}

/// One box blur along lines `count` long, `stride` apart within a line and `step` between lines.
fn box_pass(
    from: &[f32],
    into: &mut [f32],
    count: usize,
    lines: usize,
    radius: usize,
    stride: usize,
    step: usize,
) {
    let span = (2 * radius + 1) as f32;
    for line in 0..lines {
        let at = |i: isize| -> f32 {
            let i = i.clamp(0, count as isize - 1) as usize;
            from[line * step + i * stride]
        };
        let mut sum: f32 = (-(radius as isize)..=radius as isize).map(at).sum();
        for i in 0..count {
            into[line * step + i * stride] = sum / span;
            sum += at(i as isize + radius as isize + 1) - at(i as isize - radius as isize);
        }
    }
}

fn wallpaper(path: Option<&Path>, size: (usize, usize)) -> Vec<u8> {
    if let Some(picture) = path.and_then(|path| image::open(path).ok()) {
        let picture = picture.to_rgba8();
        let scale =
            (size.0 as f32 / picture.width() as f32).max(size.1 as f32 / picture.height() as f32);
        let (w, h) = (
            (picture.width() as f32 * scale).ceil() as u32,
            (picture.height() as f32 * scale).ceil() as u32,
        );
        let resized = imageops::resize(&picture, w, h, imageops::FilterType::Triangle);
        let (left, top) = ((w - size.0 as u32) / 2, (h - size.1 as u32) / 2);
        return imageops::crop_imm(&resized, left, top, size.0 as u32, size.1 as u32)
            .to_image()
            .into_raw();
    }
    let mut pixels = vec![255u8; size.0 * size.1 * 4];
    for (y, line) in pixels.chunks_exact_mut(size.0 * 4).enumerate() {
        let t = y as f32 / size.1 as f32;
        let colour = [11. + 18. * t, 10. + 12. * t, 16. + 30. * t];
        for pixel in line.as_chunks_mut::<4>().0 {
            for channel in 0..3 {
                pixel[channel] = colour[channel] as u8;
            }
        }
    }
    pixels
}

fn draw_pointer(shape: Shape, size: f32) -> Art {
    let (svg, hot) = shape.art();
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("pointer art");
    let pad = (4. * size).ceil();
    let width = (tree.size().width() * size + pad * 2.).ceil() as u32;
    let height = (tree.size().height() * size + pad * 2.).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("pointer size");
    resvg::render(
        &tree,
        tiny_skia::Transform::from_row(size, 0., 0., size, pad, pad),
        &mut pixmap.as_mut(),
    );
    let (w, h) = (width as usize, height as usize);
    let art = pixmap.data();
    let mut shadow: Vec<f32> = art
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| pixel[3] as f32 / 255.)
        .collect();
    gaussian(&mut shadow, w, h, size);
    let drop = (1.5 * size).round() as usize;
    let mut pixels = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let at = y * w + x;
            let under = if y >= drop {
                shadow[at - drop * w] * 0.45
            } else {
                0.
            };
            let source = &art[at * 4..at * 4 + 4];
            let alpha = source[3] as f32 / 255.;
            let out = &mut pixels[at * 4..at * 4 + 4];
            out[..3].copy_from_slice(&source[..3]);
            out[3] = ((alpha + under * (1. - alpha)) * 255.).round() as u8;
        }
    }
    Art {
        pixels,
        width: w,
        height: h,
        hot: (pad + hot.0 * size, pad + hot.1 * size),
    }
}

/// Runs `paint` on each `row`-byte line of `buffer`, the lines shared among the machine's cores.
fn rows(buffer: &mut [u8], row: usize, paint: impl Fn(usize, &mut [u8]) + Sync) {
    let lines = buffer.len() / row;
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let per = lines.div_ceil(threads).max(1);
    let paint = &paint;
    std::thread::scope(|scope| {
        for (chunk, part) in buffer.chunks_mut(per * row).enumerate() {
            scope.spawn(move || {
                for (line, bytes) in part.chunks_mut(row).enumerate() {
                    paint(chunk * per + line, bytes);
                }
            });
        }
    });
}

struct Taps {
    first: Vec<usize>,
    weights: Vec<Vec<f32>>,
}

/// Which source pixels each output pixel along one side takes, and how much of each, for a
/// source span that may start and end between pixels.
fn taps(source: usize, start: f64, extent: f64, out: usize) -> Taps {
    let step = extent / out as f64;
    let reach = step.max(1.);
    let mut first = Vec::with_capacity(out);
    let mut weights = Vec::with_capacity(out);
    for i in 0..out {
        let centre = start + (i as f64 + 0.5) * step;
        let lo = ((centre - reach).floor().max(0.)) as usize;
        let hi = ((centre + reach).ceil() as usize).min(source).max(lo + 1);
        let mut taken: Vec<f32> = (lo..hi)
            .map(|j| (1. - ((j as f64 + 0.5 - centre) / reach).abs()).max(0.) as f32)
            .collect();
        let total: f32 = taken.iter().sum();
        if total > 0. {
            taken.iter_mut().for_each(|weight| *weight /= total);
        } else {
            taken = vec![0.; hi - lo];
            taken[0] = 1.;
        }
        first.push(lo.min(source - 1));
        weights.push(taken);
    }
    Taps { first, weights }
}

/// Draws the part of `source` inside `rect` (in source pixels) into all of `out`.
fn resample(
    source: &[u8],
    size: (usize, usize),
    rect: (f64, f64, f64, f64),
    out: &mut [u8],
    out_size: (usize, usize),
) {
    let across = taps(size.0, rect.0, rect.2, out_size.0);
    let down = taps(size.1, rect.1, rect.3, out_size.1);
    let low = down.first[0];
    let high = down
        .first
        .iter()
        .zip(&down.weights)
        .map(|(first, weights)| first + weights.len())
        .max()
        .unwrap_or(low + 1)
        .min(size.1);
    let width = out_size.0;
    let mut middle = vec![0f32; (high - low) * width * 4];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let per = (high - low).div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (chunk, part) in middle.chunks_mut(per * width * 4).enumerate() {
            let across = &across;
            scope.spawn(move || {
                for (line, values) in part.chunks_mut(width * 4).enumerate() {
                    let y = low + chunk * per + line;
                    let row = &source[y * size.0 * 4..(y + 1) * size.0 * 4];
                    for x in 0..width {
                        let mut sum = [0f32; 4];
                        for (k, weight) in across.weights[x].iter().enumerate() {
                            let pixel = &row[(across.first[x] + k) * 4..][..4];
                            for c in 0..4 {
                                sum[c] += pixel[c] as f32 * weight;
                            }
                        }
                        values[x * 4..x * 4 + 4].copy_from_slice(&sum);
                    }
                }
            });
        }
    });
    let middle = &middle;
    let down = &down;
    rows(out, width * 4, |y, line| {
        let first = down.first[y] - low;
        for (x, value) in line.iter_mut().enumerate() {
            let mut sum = 0.;
            for (k, weight) in down.weights[y].iter().enumerate() {
                sum += middle[(first + k) * width * 4 + x] * weight;
            }
            *value = sum.round().clamp(0., 255.) as u8;
        }
    });
}

const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn digits(frame: &mut [u8], width: usize, height: usize, number: u32, x: f32, y: f32, size: f32) {
    for (place, digit) in number.to_string().bytes().enumerate() {
        let glyph = DIGITS[(digit - b'0') as usize];
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..3 {
                if bits & (4 >> column) == 0 {
                    continue;
                }
                let left = x + (place as f32 * 4. + column as f32) * size;
                let top = y + row as f32 * size;
                for py in top as usize..(top + size) as usize {
                    for px in left as usize..(left + size) as usize {
                        if px < width && py < height {
                            let pixel = &mut frame[(py * width + px) * 4..][..4];
                            pixel[..3].copy_from_slice(&[255, 220, 80]);
                        }
                    }
                }
            }
        }
    }
}
