// GPU-driven ambient mark grains for a new chat's welcome screen.
// Up to 3 spatially adjacent mark grains are packed per RawShaderInstance.
// The vertex shader evaluates analytical mark curl + shine once at the group anchor
// at globals.time and expands the quad to the tight clamped bounding box of the 3 grains.
//
// instance.bounds: pane_left_dev, pane_top_dev, pane_w_dev, pane_h_dev
// params[0]: center_x_pt, center_y_pt, packed_span_scale_cr_still, bright_rgb24
// params[1]: clock_base, gpu_t0_or_ptr, clock_cruise_or_neg_seen, g0_uv
// params[2]: g0_rad_alpha_cy, g0_rgb24, g1_uv, g1_rad_alpha_cy
// params[3]: g1_rgb24, g2_uv, g2_rad_alpha_cy, g2_rgb24

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
    @location(1) @interpolate(flat) d0: vec4<f32>,
    @location(2) @interpolate(flat) d1: vec4<f32>,
    @location(3) @interpolate(flat) d2: vec4<f32>,
    @location(4) @interpolate(flat) corner_radius_dev: f32,
}

fn wm_hash2(px: f32, py: f32) -> f32 {
    var x = fract(px * 123.34);
    var y = fract(py * 456.21);
    let d = x * (x + 45.32) + y * (y + 45.32);
    x += d;
    y += d;
    return fract(x * y);
}

fn wm_noise2(px: f32, py: f32) -> f32 {
    let ix = floor(px);
    let iy = floor(py);
    let fx0 = px - ix;
    let fy0 = py - iy;
    let fx = fx0 * fx0 * (3.0 - 2.0 * fx0);
    let fy = fy0 * fy0 * (3.0 - 2.0 * fy0);
    let a = wm_hash2(ix, iy);
    let b = wm_hash2(ix + 1.0, iy);
    let c = wm_hash2(ix, iy + 1.0);
    let d = wm_hash2(ix + 1.0, iy + 1.0);
    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    return ab + (cd - ab) * fy;
}

fn wm_noise2_grad(px: f32, py: f32) -> vec2<f32> {
    let ix = floor(px);
    let iy = floor(py);
    let fx0 = px - ix;
    let fy0 = py - iy;
    let fx = fx0 * fx0 * (3.0 - 2.0 * fx0);
    let fy = fy0 * fy0 * (3.0 - 2.0 * fy0);
    let dfx = 6.0 * fx0 * (1.0 - fx0);
    let dfy = 6.0 * fy0 * (1.0 - fy0);
    let a = wm_hash2(ix, iy);
    let b = wm_hash2(ix + 1.0, iy);
    let c = wm_hash2(ix, iy + 1.0);
    let d = wm_hash2(ix + 1.0, iy + 1.0);
    let k = a - b - c + d;
    return vec2<f32>(
        ((b - a) + k * fy) * dfx,
        ((c - a) + k * fx) * dfy,
    );
}

fn wm_curl(px: f32, py: f32, t: f32) -> vec2<f32> {
    let g1 = wm_noise2_grad(px + t, py - t * 0.7);
    let g2 = wm_noise2_grad(px * 2.1 - t * 1.3, py * 2.1 - t * 0.4);
    let g = g1 + 1.05 * g2;
    return vec2<f32>(g.y, -g.x);
}

fn wm_unpack_rgb(packed: f32) -> vec3<f32> {
    let r = floor(packed * (1.0 / 65536.0));
    let rem = packed - r * 65536.0;
    let g = floor(rem * (1.0 / 256.0));
    let b = rem - g * 256.0;
    return vec3<f32>(r, g, b) * (1.0 / 255.0);
}

fn wm_pack_rgb(rgb: vec3<f32>) -> f32 {
    let q = round(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0);
    return q.x * 65536.0 + q.y * 256.0 + q.z;
}

fn wm_pack_radius_alpha(radius_dev: f32, alpha: f32) -> f32 {
    let alpha_q = round(clamp(alpha, 0.0, 1.0) * 1023.0);
    let r_q = max(round(clamp(radius_dev, 0.0, 63.99) * 256.0), 1.0);
    return alpha_q * 16384.0 + r_q;
}

fn wm_eval_clock(base: f32, cruise: f32, elapsed: f32) -> f32 {
    if (elapsed <= cruise) {
        return base + elapsed;
    }
    let u = min(elapsed - cruise, 20.0);
    return base + cruise + (1.0 - exp(-2.2 * u)) * (1.0 / 2.2);
}

fn wm_unpack_uv(uv_packed: f32) -> vec2<f32> {
    let uv_bits = bitcast<u32>(uv_packed);
    let u_q = uv_bits & 0x7FFFu;
    let v_q = (uv_bits >> 15u) - 256u;
    return vec2<f32>(
        f32(u_q) * (1.0 / 32767.0) - 0.5,
        f32(v_q) * (1.0 / 32767.0) - 0.5,
    );
}

fn wm_eval_grain(
    uv_packed: f32,
    rac_packed: f32,
    rgb_packed: f32,
    t: f32,
    curl_off: vec2<f32>,
    shine_noise: f32,
    origin_dev: vec2<f32>,
    pane_size: vec2<f32>,
    center: vec2<f32>,
    span: f32,
    scale: f32,
    still: bool,
    bright: vec3<f32>,
    ptr: vec2<f32>,
    seen: f32,
) -> vec4<f32> {
    if (rac_packed == 0.0) {
        return vec4<f32>(0.0);
    }

    let uv = wm_unpack_uv(uv_packed);
    let rac_bits = bitcast<u32>(rac_packed);
    let rad_q = rac_bits & 0x3FFu;
    let alpha_q = (rac_bits >> 10u) & 0x3FFu;
    let cy_q = (rac_bits >> 20u) - 8u;
    let base_radius = f32(rad_q) * (4.0 / 1023.0);
    let base_alpha = f32(alpha_q) * (1.5 / 1023.0);
    let cy = f32(cy_q) * (1.0 / 255.0);

    var px = center.x + uv.x * span;
    var py = center.y + uv.y * span;
    var shine = 0.0;
    if (!still) {
        px += curl_off.x;
        py += curl_off.y;
        shine = sin(t * 0.5 + uv.x * 7.0 + uv.y * 9.0 + shine_noise);
    }

    if (px < -8.0 || py < -8.0 || px > pane_size.x + 8.0 || py > pane_size.y + 8.0) {
        return vec4<f32>(0.0);
    }

    let radius = base_radius * (1.0 + 0.03 * shine);
    let alpha = clamp(base_alpha * (0.95 + 0.05 * shine), 0.0, 1.0);
    if (radius < 0.05 || alpha < 0.012) {
        return vec4<f32>(0.0);
    }

    var shade = wm_unpack_rgb(rgb_packed);
    let shine_boost = max(shine, 0.0) * 0.6;
    if (shine_boost > 0.0) {
        shade = min(shade * (1.0 + 0.08 * shine_boost), vec3<f32>(1.0));
    }
    if (seen > 0.001) {
        let dp = vec2<f32>(px, py) - ptr;
        let pd2 = dot(dp, dp);
        if (pd2 < 230000.0) {
            let lit_ptr = seen * exp(-pd2 * (1.0 / 27000.0));
            if (lit_ptr > 0.001) {
                shade = mix(shade, bright, 0.35 * lit_ptr * cy);
            }
        }
    }

    return vec4<f32>(
        origin_dev.x + px * scale,
        origin_dev.y + py * scale,
        wm_pack_radius_alpha(radius * scale, alpha),
        wm_pack_rgb(shade),
    );
}

fn wm_dot_half_dev(sra: f32, scale: f32) -> f32 {
    let alpha_q = floor(sra * (1.0 / 16384.0));
    let r_dev = (sra - alpha_q * 16384.0) * (1.0 / 256.0);
    return r_dev + scale;
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Varying {
    let inst = instances[instance];
    let p0 = inst.params[0];
    let p1 = inst.params[1];
    let p2 = inst.params[2];
    let p3 = inst.params[3];

    let meta_bits = bitcast<u32>(p0.z);
    let span = f32(meta_bits & 0xFFFu) * 0.25;
    let scale = max(f32((meta_bits >> 12u) & 0x1FFu) * (1.0 / 32.0), 1.0);
    let still = ((meta_bits >> 21u) & 1u) != 0u;
    let cr_q = (meta_bits >> 22u) - 4u;
    let corner_radius_dev = f32(cr_q) * 0.5 * scale;

    let origin_dev = inst.bounds.xy;
    let pane_size = inst.bounds.zw / scale;
    let center = p0.xy;
    let bright = wm_unpack_rgb(p0.w);

    var t = p1.x;
    var seen = 0.0;
    var ptr = vec2<f32>(0.0);
    if (p1.y < 0.0) {
        // Static still frame.
    } else if (p1.z < 0.0) {
        seen = -p1.z;
        let px_q = floor(p1.y * (1.0 / 4096.0));
        let py_q = p1.y - px_q * 4096.0;
        ptr = vec2<f32>(px_q, py_q);
    } else {
        let elapsed = max(globals.time - p1.y, 0.0);
        t = wm_eval_clock(p1.x, p1.z, elapsed);
    }

    var curl_off = vec2<f32>(0.0);
    var shine_noise = 0.0;
    if (!still) {
        let uv0 = wm_unpack_uv(p1.w);
        curl_off = wm_curl(uv0.x * span * (1.0 / 60.0), uv0.y * span * (1.0 / 60.0), t * 0.05) * 0.5;
        shine_noise = wm_noise2(uv0.x * 6.0 + t * 0.12, uv0.y * 6.0 + t * 0.12) * 4.0;
    }

    let d0 = wm_eval_grain(p1.w, p2.x, p2.y, t, curl_off, shine_noise, origin_dev, pane_size, center, span, scale, still, bright, ptr, seen);
    let d1 = wm_eval_grain(p2.z, p2.w, p3.x, t, curl_off, shine_noise, origin_dev, pane_size, center, span, scale, still, bright, ptr, seen);
    let d2 = wm_eval_grain(p3.y, p3.z, p3.w, t, curl_off, shine_noise, origin_dev, pane_size, center, span, scale, still, bright, ptr, seen);

    var min_xy = vec2<f32>(1e9);
    var max_xy = vec2<f32>(-1e9);
    var any_vis = false;
    if (d0.z > 0.0) {
        let half = wm_dot_half_dev(d0.z, scale);
        min_xy = min(min_xy, d0.xy - vec2<f32>(half));
        max_xy = max(max_xy, d0.xy + vec2<f32>(half));
        any_vis = true;
    }
    if (d1.z > 0.0) {
        let half = wm_dot_half_dev(d1.z, scale);
        min_xy = min(min_xy, d1.xy - vec2<f32>(half));
        max_xy = max(max_xy, d1.xy + vec2<f32>(half));
        any_vis = true;
    }
    if (d2.z > 0.0) {
        let half = wm_dot_half_dev(d2.z, scale);
        min_xy = min(min_xy, d2.xy - vec2<f32>(half));
        max_xy = max(max_xy, d2.xy + vec2<f32>(half));
        any_vis = true;
    }

    var out: Varying;
    out.instance = instance;
    out.d0 = d0;
    out.d1 = d1;
    out.d2 = d2;

    min_xy = max(min_xy, inst.clip.xy);
    max_xy = min(max_xy, inst.clip.xy + inst.clip.zw);
    if (!any_vis || min_xy.x >= max_xy.x || min_xy.y >= max_xy.y) {
        out.corner_radius_dev = 0.0;
        out.position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        return out;
    }

    let rel_min = min_xy - origin_dev;
    let rel_max = max_xy - origin_dev;
    let near_x = rel_min.x < corner_radius_dev || rel_max.x > inst.bounds.z - corner_radius_dev;
    let near_y = rel_min.y < corner_radius_dev || rel_max.y > inst.bounds.w - corner_radius_dev;
    out.corner_radius_dev = select(0.0, corner_radius_dev, corner_radius_dev > 0.0 && near_x && near_y);

    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    let device = mix(min_xy, max_xy, corner);
    let ndc = device / globals.viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    return out;
}

fn wm_outside_corners(local: vec2<f32>, size: vec2<f32>, radius: f32) -> bool {
    let corner = abs(local - 0.5 * size) - (0.5 * size - radius);
    return length(max(corner, vec2<f32>(0.0))) > radius;
}

fn wm_eval_dot(device: vec2<f32>, slot: vec4<f32>) -> vec4<f32> {
    let sra = slot.z;
    let alpha_q = floor(sra * (1.0 / 16384.0));
    let r_q = sra - alpha_q * 16384.0;
    let radius = r_q * (1.0 / 256.0);
    let diff = device - slot.xy;
    let d2 = dot(diff, diff);
    let max_r = radius + 0.6;
    if (d2 >= max_r * max_r) {
        return vec4<f32>(0.0);
    }
    let past = sqrt(d2) - radius;
    let cov = 1.0 - smoothstep(-0.55, 0.55, past);
    let a = clamp(alpha_q * (1.0 / 1023.0) * cov, 0.0, 1.0);
    if (a <= 0.002) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(wm_unpack_rgb(slot.w) * a, a);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let inst = instances[input.instance];
    let device = input.position.xy;
    if (any(device < inst.clip.xy) || any(device >= inst.clip.xy + inst.clip.zw)) {
        discard;
    }
    if (input.corner_radius_dev > 0.0 && wm_outside_corners(device - inst.bounds.xy, inst.bounds.zw, input.corner_radius_dev)) {
        discard;
    }

    var acc = vec4<f32>(0.0);
    if (input.d0.z > 0.0) {
        acc = wm_eval_dot(device, input.d0);
    }
    if (input.d1.z > 0.0) {
        let c1 = wm_eval_dot(device, input.d1);
        if (c1.a > 0.0) {
            acc = vec4<f32>(c1.rgb + acc.rgb * (1.0 - c1.a), c1.a + acc.a * (1.0 - c1.a));
        }
    }
    if (input.d2.z > 0.0) {
        let c2 = wm_eval_dot(device, input.d2);
        if (c2.a > 0.0) {
            acc = vec4<f32>(c2.rgb + acc.rgb * (1.0 - c2.a), c2.a + acc.a * (1.0 - c2.a));
        }
    }

    if (acc.a <= 0.002) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return acc;
    }
    return vec4<f32>(clamp(acc.rgb / acc.a, vec3<f32>(0.0), vec3<f32>(1.0)), acc.a);
}
