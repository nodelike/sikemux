// GPU-driven ambient field particles for a new chat's welcome screen.
// Up to 4 spatially adjacent field particles are packed per RawShaderInstance.
// The vertex shader evaluates analytical curl + domain warp once at the quad center,
// reconstructs each particle's deterministic spec from its index, compacts visible dots,
// and expands the quad to the tight clamped bounding box of only the visible dots.
//
// instance.bounds: pane_left_dev, pane_top_dev, pane_w_dev, pane_h_dev
// params[0]: center_x_pt, center_y_pt, span_pt, scale
// params[1]: deep_rgb24, tint_rgb24, alt_rgb24, bright_rgb24
// params[2]:
//   ambient GPU mode (y >= 0, z >= 0): clock_base, gpu_t0, clock_cruise, meta
//   static still mode (y < 0):         t_exact, -1, 0, meta
//   interactive mode (y >= 0, z < 0):  t_exact, pack_u12(ptr_x, ptr_y), -seen, meta
//   meta = flags * 262144 + cr_q4 * 1024 + opacity_q10  (flags: bit0=dark, bit1=still)
// params[3]: pack_u12(ql, qr) or -1, pack_u12(qt, qb), pack_u15(i0, i1), pack_u15(i2, i3)
//   (empty slot index = 24001)

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
    @location(4) @interpolate(flat) d3: vec4<f32>,
    @location(5) @interpolate(flat) corner_radius_dev: f32,
}

const WF_EMPTY_SLOT: u32 = 24001u;

fn wf_roll(state: u32) -> f32 {
    var v = (state ^ (state >> 15u)) * (1u | state);
    v = (v + ((v ^ (v >> 7u)) * (61u | v))) ^ v;
    return f32(v ^ (v >> 14u)) * (1.0 / 4294967296.0);
}

fn wf_hash2(px: f32, py: f32) -> f32 {
    var x = fract(px * 123.34);
    var y = fract(py * 456.21);
    let d = x * (x + 45.32) + y * (y + 45.32);
    x += d;
    y += d;
    return fract(x * y);
}

fn wf_noise2(px: f32, py: f32) -> f32 {
    let ix = floor(px);
    let iy = floor(py);
    let fx0 = px - ix;
    let fy0 = py - iy;
    let fx = fx0 * fx0 * (3.0 - 2.0 * fx0);
    let fy = fy0 * fy0 * (3.0 - 2.0 * fy0);
    let a = wf_hash2(ix, iy);
    let b = wf_hash2(ix + 1.0, iy);
    let c = wf_hash2(ix, iy + 1.0);
    let d = wf_hash2(ix + 1.0, iy + 1.0);
    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    return ab + (cd - ab) * fy;
}

fn wf_noise2_grad(px: f32, py: f32) -> vec2<f32> {
    let ix = floor(px);
    let iy = floor(py);
    let fx0 = px - ix;
    let fy0 = py - iy;
    let fx = fx0 * fx0 * (3.0 - 2.0 * fx0);
    let fy = fy0 * fy0 * (3.0 - 2.0 * fy0);
    let dfx = 6.0 * fx0 * (1.0 - fx0);
    let dfy = 6.0 * fy0 * (1.0 - fy0);
    let a = wf_hash2(ix, iy);
    let b = wf_hash2(ix + 1.0, iy);
    let c = wf_hash2(ix, iy + 1.0);
    let d = wf_hash2(ix + 1.0, iy + 1.0);
    let k = a - b - c + d;
    return vec2<f32>(
        ((b - a) + k * fy) * dfx,
        ((c - a) + k * fx) * dfy,
    );
}

fn wf_curl(px: f32, py: f32, t: f32) -> vec2<f32> {
    let g1 = wf_noise2_grad(px + t, py - t * 0.7);
    let g2 = wf_noise2_grad(px * 2.1 - t * 1.3, py * 2.1 - t * 0.4);
    let g = g1 + 1.05 * g2;
    return vec2<f32>(g.y, -g.x);
}

fn wf_unpack_rgb(packed: f32) -> vec3<f32> {
    let r = floor(packed * (1.0 / 65536.0));
    let rem = packed - r * 65536.0;
    let g = floor(rem * (1.0 / 256.0));
    let b = rem - g * 256.0;
    return vec3<f32>(r, g, b) * (1.0 / 255.0);
}

fn wf_pack_rgb(rgb: vec3<f32>) -> f32 {
    let q = round(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0);
    return q.x * 65536.0 + q.y * 256.0 + q.z;
}

fn wf_pack_radius_alpha(radius_dev: f32, alpha: f32, is_bokeh: bool) -> f32 {
    let alpha_q = round(clamp(alpha, 0.0, 1.0) * 1023.0);
    let r_q = max(round(clamp(radius_dev, 0.0, 63.99) * 256.0), 1.0);
    let packed = alpha_q * 16384.0 + r_q;
    return select(packed, -packed, is_bokeh);
}

fn wf_eval_clock(base: f32, cruise: f32, elapsed: f32) -> f32 {
    if (elapsed <= cruise) {
        return base + elapsed;
    }
    let u = min(elapsed - cruise, 20.0);
    return base + cruise + (1.0 - exp(-2.2 * u)) * (1.0 / 2.2);
}

fn wf_home_pos(idx: u32, pane_size: vec2<f32>) -> vec2<f32> {
    let s1 = 3u + (2u * idx + 1u) * 0x6d2b79f5u;
    let s2 = s1 + 0x6d2b79f5u;
    let cx = wf_roll(s1);
    let cy = wf_roll(s2);
    let f = f32(idx);
    let hx = fract(0.5 + f * 0.7548777 + cx * 0.004);
    let hy = fract(0.5 + f * 0.5698403 + cy * 0.004);
    return vec2<f32>(hx * pane_size.x, hy * pane_size.y);
}

fn wf_eval_particle(
    idx: u32,
    t: f32,
    curl_off: vec2<f32>,
    warp: vec2<f32>,
    warmth_shared: f32,
    other_shared: f32,
    origin_dev: vec2<f32>,
    pane_size: vec2<f32>,
    scale: f32,
    center: vec2<f32>,
    span: f32,
    deep: vec3<f32>,
    tint: vec3<f32>,
    alt: vec3<f32>,
    bright: vec3<f32>,
    opacity: f32,
    dark: bool,
    still: bool,
    has_quiet: bool,
    quiet: vec4<f32>,
    ptr: vec2<f32>,
    seen: f32,
) -> vec4<f32> {
    if (idx >= WF_EMPTY_SLOT) {
        return vec4<f32>(0.0);
    }

    let s1 = 3u + (2u * idx + 1u) * 0x6d2b79f5u;
    let s2 = s1 + 0x6d2b79f5u;
    let cx = wf_roll(s1);
    let cy = wf_roll(s2);
    let f = f32(idx);
    let hx = fract(0.5 + f * 0.7548777 + cx * 0.004);
    let hy = fract(0.5 + f * 0.5698403 + cy * 0.004);

    let w = pane_size.x;
    let h = pane_size.y;
    let px = hx * w + curl_off.x + sin(t * 0.2 + cx * 44.0) * 4.0;
    let py = hy * h + curl_off.y + cos(t * 0.17 + cy * 51.0) * 4.0;

    if (px < -16.0 || py < -16.0 || px > w + 16.0 || py > h + 16.0) {
        return vec4<f32>(0.0);
    }

    let diag = sqrt(w * w + h * h);
    let reach = diag * 0.62;
    let dc = vec2<f32>(px, py) - center;
    let away2 = dot(dc, dc);
    if (away2 >= reach * reach && away2 > span * span * 2.5) {
        return vec4<f32>(0.0);
    }

    var calm = 1.0;
    if (has_quiet) {
        if (px >= quiet.x && px <= quiet.z && py >= quiet.y && py <= quiet.w) {
            return vec4<f32>(0.0);
        }
        let dq = vec2<f32>(
            px - clamp(px, quiet.x, quiet.z),
            py - clamp(py, quiet.y, quiet.w),
        );
        let qd2 = dot(dq, dq);
        if (qd2 < 12100.0) {
            calm = smoothstep(0.0, 110.0, sqrt(qd2));
            if (calm <= 0.005) {
                return vec4<f32>(0.0);
            }
        }
    }

    let clearing = span * 0.50;
    let away = sqrt(away2);
    let fade = max(1.0 - away / max(reach, 1.0), 0.0)
        * smoothstep(clearing - 34.0, clearing + 30.0, away);
    let core_arg = away2 / max(span * span * 0.55, 1.0);
    let core = select(0.0, exp(-core_arg), core_arg < 8.5);
    if (fade <= 0.002 && core <= 0.015) {
        return vec4<f32>(0.0);
    }

    let dx = px * (1.0 / 150.0);
    let dy = py * (1.0 / 150.0);
    let cloud = wf_noise2(dx + t * 0.025 + 2.0 * warp.x, dy - t * 0.015 + 2.0 * warp.y);

    let rich = select(0.88, 0.90, dark);
    var tone = fade * smoothstep(0.12, 0.86, cloud) * 1.45 * rich;
    if (away >= clearing) {
        tone += core * 0.5 * rich;
    }

    let is_bokeh = cx > 0.982;
    let cy2 = cy * cy;
    var radius = 0.0;
    var alpha = 0.0;
    var lit = 0.0;

    if (is_bokeh) {
        radius = 2.4 + (8.5 - 2.4) * cy2;
        let bokeh_alpha = 0.12 + (0.05 - 0.12) * cy;
        let bokeh_light_mul = select(0.72, 1.0, dark);
        alpha = fade * bokeh_alpha * smoothstep(0.1, 0.6, tone + 0.2) * calm * bokeh_light_mul * opacity;
        if (radius < 0.05 || alpha < 0.012) {
            return vec4<f32>(0.0);
        }
    } else {
        let cut = 0.06 + 0.9 * wf_hash2(cx * 13.7, cy * 13.7);
        if (tone <= cut - 0.1) {
            return vec4<f32>(0.0);
        }
        let shown = smoothstep(cut - 0.1, cut + 0.1, tone);
        if (shown <= 0.01) {
            return vec4<f32>(0.0);
        }
        if (seen > 0.001) {
            let dp = vec2<f32>(px, py) - ptr;
            let pd2 = dot(dp, dp);
            if (pd2 < 230000.0) {
                lit = seen * exp(-pd2 * (1.0 / 27000.0));
            }
        }
        let clamped_tone = clamp(tone, 0.0, 1.0);
        let cy5 = cy2 * cy2 * cy;
        radius = (0.48 + 0.75 * cy5) + 0.28 * clamped_tone;
        alpha = shown * (0.24 + 0.46 * clamped_tone) * (1.0 + lit);
        if (!still) {
            alpha *= 0.84 + 0.16 * sin(t * (0.2 + cx * 0.5) + cy * 60.0);
        }
        alpha *= calm * opacity;
        if (radius < 0.05 || alpha < 0.012) {
            return vec4<f32>(0.0);
        }
    }

    if (is_bokeh && seen > 0.001) {
        let dp = vec2<f32>(px, py) - ptr;
        let pd2 = dot(dp, dp);
        if (pd2 < 230000.0) {
            lit = seen * exp(-pd2 * (1.0 / 27000.0));
        }
    }

    var shade = mix(deep, tint, smoothstep(0.18, 0.62, warmth_shared + 0.5 * core + 0.22 * (cx - 0.5)));
    shade = mix(shade, alt, smoothstep(0.5, 0.74, other_shared + 0.18 * (cy - 0.5)) * (1.0 - 0.6 * core));
    let sparkle_step = select(0.0, 0.5, cx >= 0.965);
    let bright_mix = clamp(core * 1.2 * cx * cx + sparkle_step + 0.45 * lit * cy, 0.0, 0.92);
    shade = mix(shade, bright, bright_mix);
    shade = clamp(shade * (0.78 + 0.4 * cx), vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(
        origin_dev.x + px * scale,
        origin_dev.y + py * scale,
        wf_pack_radius_alpha(radius * scale, min(alpha, 1.0), is_bokeh),
        wf_pack_rgb(shade),
    );
}

fn wf_dot_half_dev(sra: f32, scale: f32) -> f32 {
    let packed_ra = abs(sra);
    let alpha_q = floor(packed_ra * (1.0 / 16384.0));
    let r_dev = (packed_ra - alpha_q * 16384.0) * (1.0 / 256.0);
    return select(r_dev + scale, r_dev * 1.75 + scale, sra < 0.0);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Varying {
    let inst = instances[instance];
    let p0 = inst.params[0];
    let p1 = inst.params[1];
    let p2 = inst.params[2];
    let p3 = inst.params[3];

    let scale = max(p0.w, 1.0);
    let pane_size = inst.bounds.zw / scale;
    let origin_dev = inst.bounds.xy;

    var t = p2.x;
    var seen = 0.0;
    var ptr = vec2<f32>(0.0);
    if (p2.y < 0.0) {
        // Static still frame.
    } else if (p2.z < 0.0) {
        seen = -p2.z;
        let px_q = floor(p2.y * (1.0 / 4096.0));
        let py_q = p2.y - px_q * 4096.0;
        ptr = vec2<f32>(px_q, py_q);
    } else {
        let elapsed = max(globals.time - p2.y, 0.0);
        t = wf_eval_clock(p2.x, p2.z, elapsed);
    }

    let packed_meta = p2.w;
    let flags = u32(floor(packed_meta * (1.0 / 262144.0)));
    let rem_meta = packed_meta - f32(flags) * 262144.0;
    let cr_q = floor(rem_meta * (1.0 / 1024.0));
    let opacity_q = rem_meta - cr_q * 1024.0;
    let dark = (flags & 1u) != 0u;
    let still = (flags & 2u) != 0u;
    let opacity = opacity_q * (1.0 / 1023.0);
    let corner_radius_dev = cr_q * 0.25 * scale;

    let deep = wf_unpack_rgb(p1.x);
    let tint = wf_unpack_rgb(p1.y);
    let alt = wf_unpack_rgb(p1.z);
    let bright = wf_unpack_rgb(p1.w);

    let has_quiet = p3.x >= 0.0;
    var quiet = vec4<f32>(0.0);
    if (has_quiet) {
        let ql = floor(p3.x * (1.0 / 4096.0));
        let qr = p3.x - ql * 4096.0;
        let qt = floor(p3.y * (1.0 / 4096.0));
        let qb = p3.y - qt * 4096.0;
        quiet = vec4<f32>(ql, qt, qr, qb);
    }

    let pair0 = bitcast<u32>(p3.z);
    let pair1 = bitcast<u32>(p3.w);
    let i0 = pair0 & 0x7FFFu;
    let i1 = (pair0 >> 15u) - 256u;
    let i2 = pair1 & 0x7FFFu;
    let i3 = (pair1 >> 15u) - 256u;

    // Evaluate slowly varying fields (wavelength 210..330pt) once at the quad's anchor.
    var anchor = vec2<f32>(0.0);
    if (i0 < WF_EMPTY_SLOT) {
        anchor = wf_home_pos(i0, pane_size);
    } else if (i1 < WF_EMPTY_SLOT) {
        anchor = wf_home_pos(i1, pane_size);
    } else if (i2 < WF_EMPTY_SLOT) {
        anchor = wf_home_pos(i2, pane_size);
    } else if (i3 < WF_EMPTY_SLOT) {
        anchor = wf_home_pos(i3, pane_size);
    }
    let curl_off = wf_curl(anchor.x * (1.0 / 230.0), anchor.y * (1.0 / 230.0), t * 0.035) * 14.0;
    let drifted = anchor + curl_off;
    let adx = drifted.x * (1.0 / 150.0);
    let ady = drifted.y * (1.0 / 150.0);
    let warp = vec2<f32>(
        wf_noise2(adx * 0.7 + t * 0.02, ady * 0.7 + t * 0.02),
        wf_noise2(adx * 0.7 + 9.0 - t * 0.02, ady * 0.7 + 9.0 - t * 0.02),
    );
    let warmth_shared = wf_noise2(drifted.x * (1.0 / 330.0) + t * 0.011, drifted.y * (1.0 / 330.0) - t * 0.008);
    let other_shared = wf_noise2(drifted.x * (1.0 / 210.0) - t * 0.008 + 17.0, drifted.y * (1.0 / 210.0) + t * 0.01 + 17.0);

    let r0 = wf_eval_particle(i0, t, curl_off, warp, warmth_shared, other_shared, origin_dev, pane_size, scale, p0.xy, p0.z, deep, tint, alt, bright, opacity, dark, still, has_quiet, quiet, ptr, seen);
    let r1 = wf_eval_particle(i1, t, curl_off, warp, warmth_shared, other_shared, origin_dev, pane_size, scale, p0.xy, p0.z, deep, tint, alt, bright, opacity, dark, still, has_quiet, quiet, ptr, seen);
    let r2 = wf_eval_particle(i2, t, curl_off, warp, warmth_shared, other_shared, origin_dev, pane_size, scale, p0.xy, p0.z, deep, tint, alt, bright, opacity, dark, still, has_quiet, quiet, ptr, seen);
    let r3 = wf_eval_particle(i3, t, curl_off, warp, warmth_shared, other_shared, origin_dev, pane_size, scale, p0.xy, p0.z, deep, tint, alt, bright, opacity, dark, still, has_quiet, quiet, ptr, seen);

    // Compact visible dots to the front so fs_main short-circuits empty slots immediately.
    var slots = array<vec4<f32>, 4>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var count = 0u;
    var min_xy = vec2<f32>(1e9);
    var max_xy = vec2<f32>(-1e9);
    if (r0.z != 0.0) {
        let half = wf_dot_half_dev(r0.z, scale);
        min_xy = min(min_xy, r0.xy - vec2<f32>(half));
        max_xy = max(max_xy, r0.xy + vec2<f32>(half));
        slots[count] = r0;
        count += 1u;
    }
    if (r1.z != 0.0) {
        let half = wf_dot_half_dev(r1.z, scale);
        min_xy = min(min_xy, r1.xy - vec2<f32>(half));
        max_xy = max(max_xy, r1.xy + vec2<f32>(half));
        slots[count] = r1;
        count += 1u;
    }
    if (r2.z != 0.0) {
        let half = wf_dot_half_dev(r2.z, scale);
        min_xy = min(min_xy, r2.xy - vec2<f32>(half));
        max_xy = max(max_xy, r2.xy + vec2<f32>(half));
        slots[count] = r2;
        count += 1u;
    }
    if (r3.z != 0.0) {
        let half = wf_dot_half_dev(r3.z, scale);
        min_xy = min(min_xy, r3.xy - vec2<f32>(half));
        max_xy = max(max_xy, r3.xy + vec2<f32>(half));
        slots[count] = r3;
        count += 1u;
    }

    var out: Varying;
    out.instance = instance;
    out.d0 = slots[0];
    out.d1 = slots[1];
    out.d2 = slots[2];
    out.d3 = slots[3];

    let clip_min = inst.clip.xy;
    let clip_max = inst.clip.xy + inst.clip.zw;
    min_xy = max(min_xy, clip_min);
    max_xy = min(max_xy, clip_max);

    if (count == 0u || min_xy.x >= max_xy.x || min_xy.y >= max_xy.y) {
        out.corner_radius_dev = 0.0;
        out.position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        return out;
    }

    // Only enable rounded-corner distance checks in fs_main when this quad actually overlaps a corner.
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

fn wf_outside_corners(local: vec2<f32>, size: vec2<f32>, radius: f32) -> bool {
    let corner = abs(local - 0.5 * size) - (0.5 * size - radius);
    return length(max(corner, vec2<f32>(0.0))) > radius;
}

fn wf_eval_dot(device: vec2<f32>, slot: vec4<f32>) -> vec4<f32> {
    let sra = slot.z;
    let packed_ra = abs(sra);
    let alpha_q = floor(packed_ra * (1.0 / 16384.0));
    let r_q = packed_ra - alpha_q * 16384.0;
    let radius = r_q * (1.0 / 256.0);
    let diff = device - slot.xy;
    let d2 = dot(diff, diff);
    let max_r = select(radius + 0.6, radius * 1.75 + 0.6, sra < 0.0);
    if (d2 >= max_r * max_r) {
        return vec4<f32>(0.0);
    }
    let past = sqrt(d2) - radius;
    var cov = 0.0;
    if (sra < 0.0) {
        let b = clamp(1.0 - smoothstep(-radius, radius * 0.75, past), 0.0, 1.0);
        cov = b * sqrt(b);
    } else {
        cov = 1.0 - smoothstep(-0.55, 0.55, past);
    }
    let a = clamp(alpha_q * (1.0 / 1023.0) * cov, 0.0, 1.0);
    if (a <= 0.002) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(wf_unpack_rgb(slot.w) * a, a);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let inst = instances[input.instance];
    let device = input.position.xy;
    if (any(device < inst.clip.xy) || any(device >= inst.clip.xy + inst.clip.zw)) {
        discard;
    }
    if (input.corner_radius_dev > 0.0 && wf_outside_corners(device - inst.bounds.xy, inst.bounds.zw, input.corner_radius_dev)) {
        discard;
    }

    var acc = wf_eval_dot(device, input.d0);
    if (input.d1.z != 0.0) {
        let c1 = wf_eval_dot(device, input.d1);
        if (c1.a > 0.0) {
            acc = vec4<f32>(c1.rgb + acc.rgb * (1.0 - c1.a), c1.a + acc.a * (1.0 - c1.a));
        }
        if (input.d2.z != 0.0) {
            let c2 = wf_eval_dot(device, input.d2);
            if (c2.a > 0.0) {
                acc = vec4<f32>(c2.rgb + acc.rgb * (1.0 - c2.a), c2.a + acc.a * (1.0 - c2.a));
            }
            if (input.d3.z != 0.0) {
                let c3 = wf_eval_dot(device, input.d3);
                if (c3.a > 0.0) {
                    acc = vec4<f32>(c3.rgb + acc.rgb * (1.0 - c3.a), c3.a + acc.a * (1.0 - c3.a));
                }
            }
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
