// What every dithered program shares: GPUI's bindings, the rectangle each instance
// covers, and Paper Shaders' simplex noise and 8x8 Bayer grid. The build puts it in
// front of each program in this folder.

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

fn permute(x: vec3<f32>) -> vec3<f32> {
    return (((x * 34.0) + 1.0) * x) % vec3<f32>(289.0);
}

fn modulo(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn snoise(v: vec2<f32>) -> f32 {
    let c = vec4<f32>(0.211324865405187, 0.366025403784439, -0.577350269189626, 0.024390243902439);
    var i = floor(v + dot(v, c.yy));
    let x0 = v - i + dot(i, c.xx);
    let i1 = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), x0.x > x0.y);
    var x12 = x0.xyxy + c.xxzz;
    x12 = vec4<f32>(x12.xy - i1, x12.zw);
    i = modulo(i, 289.0);
    let p = permute(permute(i.y + vec3<f32>(0.0, i1.y, 1.0)) + i.x + vec3<f32>(0.0, i1.x, 1.0));
    var m = max(vec3<f32>(0.5) - vec3<f32>(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), vec3<f32>(0.0));
    m = m * m;
    m = m * m;
    let x = 2.0 * fract(p * c.www) - 1.0;
    let h = abs(x) - 0.5;
    let ox = floor(x + 0.5);
    let a0 = x - ox;
    m = m * (1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h));
    let g = vec3<f32>(a0.x * x0.x + h.x * x0.y, a0.yz * x12.xz + h.yz * x12.yw);
    return 130.0 * dot(m, g);
}

fn bayer8(cell: vec2<f32>) -> f32 {
    let at = vec2<u32>(fract(cell / 8.0) * 8.0);
    // The 8x8 Bayer matrix's entry, built from the bits of its column and row.
    let a = at.x ^ at.y;
    let b = at.y;
    let value = ((a & 1u) << 5u) | ((b & 1u) << 4u) | ((a & 2u) << 2u) | ((b & 2u) << 1u)
        | ((a & 4u) >> 1u) | ((b & 4u) >> 2u);
    return f32(value) / 64.0;
}

// Whether `local`, in points from a rectangle's corner, falls outside its rounded corners.
fn outside_corners(local: vec2<f32>, size: vec2<f32>, radius: f32) -> bool {
    let corner = abs(local - 0.5 * size) - (0.5 * size - radius);
    return length(max(corner, vec2<f32>(0.0))) > radius;
}
