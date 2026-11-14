// What every program in this folder shares: GPUI's bindings and the rectangle each
// instance covers. The build puts it in front of each, with the kinds in `kinds`.

struct GlobalParams {
    viewport_size: vec2<f32>,
    premultiplied_alpha: u32,
    time: f32,
}

struct ShaderInstance {
    bounds: vec4<f32>,
    clip: vec4<f32>,
    image: vec4<f32>,
    params: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> globals: GlobalParams;
@group(1) @binding(0) var<storage, read> instances: array<ShaderInstance>;

struct Varying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) instance: u32,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Varying {
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    let bounds = instances[instance].bounds;
    let device = bounds.xy + corner * bounds.zw;
    let clip = device / globals.viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    var out: Varying;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.instance = instance;
    return out;
}

// Whether `local`, in points from a rectangle's corner, falls outside its rounded corners.
fn outside_corners(local: vec2<f32>, size: vec2<f32>, radius: f32) -> bool {
    let corner = abs(local - 0.5 * size) - (0.5 * size - radius);
    return length(max(corner, vec2<f32>(0.0))) > radius;
}

// A repeatable random value for a cell, for one seed.
fn cell_hash(cell: vec2<f32>, seed: f32) -> f32 {
    return fract(sin(cell.x * 127.1 + cell.y * 311.7 + seed * 74.7) * 43758.5453);
}

// Smooth value noise.
fn value_noise(at: vec2<f32>, seed: f32) -> f32 {
    let whole = floor(at);
    let part = smoothstep(vec2<f32>(0.0), vec2<f32>(1.0), at - whole);
    let p = cell_hash(whole, seed);
    let q = cell_hash(whole + vec2<f32>(1.0, 0.0), seed);
    let r = cell_hash(whole + vec2<f32>(0.0, 1.0), seed);
    let s = cell_hash(whole + vec2<f32>(1.0, 1.0), seed);
    return p + (q - p) * part.x + (r - p) * part.y + (p - q - r + s) * part.x * part.y;
}

// Three octaves of value noise.
fn layered_noise(at: vec2<f32>, seed: f32) -> f32 {
    return value_noise(at, seed) * 0.57
        + value_noise(at * 2.03, seed + 1.0) * 0.29
        + value_noise(at * 4.1, seed + 2.0) * 0.14;
}

// How far `at` is from `rect`, 0 inside it.
fn rect_distance(at: vec2<f32>, rect: vec4<f32>) -> f32 {
    let dx = max(max(rect.x - at.x, 0.0), at.x - rect.z);
    let dy = max(max(rect.y - at.y, 0.0), at.y - rect.w);
    return length(vec2<f32>(dx, dy));
}

// The nearer of two distances, rounding off the corner where the two shapes meet.
fn blended_nearest(a: f32, b: f32, blend: f32) -> f32 {
    let share = clamp(0.5 + 0.5 * (b - a) / blend, 0.0, 1.0);
    return mix(b, a, share) - blend * share * (1.0 - share);
}
