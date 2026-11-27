// Coarse-grid atmospheric glow cell behind a new chat's welcome particles,
// evaluated in the vertex shader so ambient frames animate on the GPU via globals.time
// with zero CPU stepping and zero per-pixel trig/exp.
//
// params[0]: pane_left_dev, pane_top_dev, pane_w_pt, pane_h_pt
// params[1]: center_x_pt, center_y_pt, span_pt, inv_ground_scale
// params[2]: tint_rgb24, deep_rgb24, alt_rgb24, packed_meta (cid * 262144 + cr_q4 * 1024 + opacity_q10)
// params[3]:
//   ambient GPU mode (w == 0, y >= 0): clock_base, gpu_t0, clock_cruise, 0
//   static still mode (w == 0, y < 0): t_exact, -1, 0, 0
//   interactive mode (w > 0):          t_exact, ptr_x_pt, ptr_y_pt, seen

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
    @location(1) light: vec3<f32>,
    @location(2) @interpolate(flat) glow_info: vec4<f32>,
}

fn wg_unpack_rgb(packed: f32) -> vec3<f32> {
    let r = floor(packed * (1.0 / 65536.0));
    let rem = packed - r * 65536.0;
    let g = floor(rem * (1.0 / 256.0));
    let b = rem - g * 256.0;
    return vec3<f32>(r, g, b) * (1.0 / 255.0);
}

fn wg_eval_clock(base: f32, cruise: f32, elapsed: f32) -> f32 {
    if (elapsed <= cruise) {
        return base + elapsed;
    }
    let u = min(elapsed - cruise, 20.0);
    return base + cruise + (1.0 - exp(-2.2 * u)) * (1.0 / 2.2);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Varying {
    let inst = instances[instance];
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    let device = inst.bounds.xy + corner * inst.bounds.zw;
    let clip = device / globals.viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);

    let p0 = inst.params[0];
    let p1 = inst.params[1];
    let p2 = inst.params[2];
    let p3 = inst.params[3];

    let w = p0.z;
    let h = p0.w;
    let center = p1.xy;
    let span = max(p1.z, 60.0);
    let local = (device - p0.xy) * p1.w;

    var glow_t = p3.x;
    var seen = 0.0;
    var ptr = vec2<f32>(0.0);
    if (p3.w > 0.0) {
        seen = p3.w;
        ptr = p3.yz;
    } else if (p3.y >= 0.0) {
        let elapsed = max(globals.time - p3.y, 0.0);
        glow_t = wg_eval_clock(p3.x, p3.z, elapsed);
    }

    let diag = sqrt(w * w + h * h);
    let w0 = max(span * 1.6, diag * 0.22);
    let w1 = span * 0.72;
    let w2 = max(diag * 0.46, 1.0);
    let w3 = max(diag * 0.34, 1.0);
    let inv_w0_sq = 1.0 / (w0 * w0);
    let inv_w1_sq = 1.0 / (w1 * w1);
    let inv_w2_sq = 1.0 / (w2 * w2);
    let inv_w3_sq = 1.0 / (w3 * w3);
    let inv_ptr_sq = 1.0 / (230.0 * 230.0);

    let slant_x = 0.8 * local.x + 0.6 * local.y;
    let slant_y = -0.6 * local.x + 0.8 * local.y;
    let s1x = slant_x * (1.0 / 260.0);
    let s1y = slant_y * (1.0 / 260.0);
    let cloud = 0.5
        + 0.28 * sin(s1x + glow_t * 0.08 + 0.7 * cos(s1y - glow_t * 0.06))
        + 0.22 * cos(s1y * 1.15 - glow_t * 0.05 + 0.6 * sin(s1x * 0.85));
    let s2x = slant_x * (1.0 / 340.0);
    let s2y = slant_y * (1.0 / 340.0);
    let second = 0.5
        + 0.28 * cos(s2x - glow_t * 0.06 + 0.7 * sin(s2y + glow_t * 0.07))
        + 0.22 * sin(s2y * 1.1 + glow_t * 0.05);

    let dc = local - center;
    let d_c2 = dot(dc, dc);
    let l0 = exp(-d_c2 * inv_w0_sq) * (0.5 + cloud);
    let l1 = exp(-d_c2 * inv_w1_sq) * (0.62 + 0.55 * cloud) * 0.30;

    let d2 = local - vec2<f32>(w * 0.12, h * 1.04);
    let l2 = exp(-dot(d2, d2) * inv_w2_sq) * (0.4 + 1.1 * second) * 0.10;

    let d3 = local - vec2<f32>(w * 0.98, h * 1.02);
    let l3 = exp(-dot(d3, d3) * inv_w3_sq) * (0.35 + second) * 0.22;

    var lp = 0.0;
    if (seen > 0.001) {
        let dp = local - ptr;
        let d_p2 = dot(dp, dp);
        if (d_p2 < 450000.0) {
            lp = 0.14 * seen * exp(-d_p2 * inv_ptr_sq);
        }
    }

    let tint = wg_unpack_rgb(p2.x);
    let deep = wg_unpack_rgb(p2.y);
    let alt = wg_unpack_rgb(p2.z);
    let deep_tint = deep * 0.62 + tint * 0.16;
    let light = (deep_tint * l0 + tint * (l1 + lp) + alt * l2 + deep * l3) * 0.16;

    let packed_meta = p2.w;
    let corner_id = floor(packed_meta * (1.0 / 262144.0));
    let rem_meta = packed_meta - corner_id * 262144.0;
    let cr_q = floor(rem_meta * (1.0 / 1024.0));
    let opacity_q = rem_meta - cr_q * 1024.0;

    var out: Varying;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.instance = instance;
    out.light = light;
    out.glow_info = vec4<f32>(opacity_q * (1.0 / 1023.0), fract(glow_t), cr_q * 0.25, corner_id);
    return out;
}

fn wg_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q = q + vec2<f32>(dot(q, q + vec2<f32>(45.32)));
    return fract(q.x * q.y);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let inst = instances[input.instance];
    let clip = inst.clip;
    let device = input.position.xy;
    if (any(device < clip.xy) || any(device >= clip.xy + clip.zw)) {
        discard;
    }

    let corner_radius = input.glow_info.z;
    if (corner_radius > 0.0) {
        let bounds = inst.bounds;
        let corner_id = input.glow_info.w;
        var local = device - bounds.xy;
        if (corner_id == 2.0) {
            local = vec2<f32>(bounds.x + bounds.z - device.x, device.y - bounds.y);
        } else if (corner_id == 3.0) {
            local = vec2<f32>(device.x - bounds.x, bounds.y + bounds.w - device.y);
        } else if (corner_id == 4.0) {
            local = bounds.xy + bounds.zw - device;
        }
        if (local.x < corner_radius && local.y < corner_radius) {
            let d = vec2<f32>(corner_radius) - local;
            if (dot(d, d) > corner_radius * corner_radius) {
                discard;
            }
        }
    }

    let light = input.light;
    let grain = (wg_hash(device + vec2<f32>(input.glow_info.y)) - 0.5)
        * 0.014
        * step(0.004, light.r + light.g + light.b);
    let lit = clamp(light + vec3<f32>(grain), vec3<f32>(0.0), vec3<f32>(1.0));
    let opacity = input.glow_info.x;
    let out_a = clamp(max(lit.r, max(lit.g, lit.b)) * opacity, 0.0, 1.0);
    if (out_a <= 0.001) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(lit * opacity, out_a);
    }
    return vec4<f32>(clamp((lit * opacity) / out_a, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
}
