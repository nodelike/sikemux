// Soft atmospheric glow behind a new chat's welcome particles, matching
// shared/signin/dots.js GLOW + GLOW_FILL.
//
// params[0]: clock, pointer_seen, glow_gain, opacity
// params[1]: packed tint, packed deep, packed alt, scale
// params[2]: corner_radius, mark_cx, mark_cy, mark_span
// params[3]: pointer_x, pointer_y, 0, 0

fn wg_unpack_rgb(packed: f32) -> vec3<f32> {
    let b = floor(packed % 256.0);
    let g = floor((packed / 256.0) % 256.0);
    let r = floor(packed / 65536.0);
    return vec3<f32>(r, g, b) / 255.0;
}

fn wg_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q = q + vec2<f32>(dot(q, q + vec2<f32>(45.32)));
    return fract(q.x * q.y);
}

fn wg_lamp(at: vec2<f32>, origin: vec2<f32>, wide: f32) -> f32 {
    let off = at - origin;
    return exp(-dot(off, off) / max(wide * wide, 1.0));
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }

    let p0 = shader.params[0];
    let t = p0.x;
    let seen = p0.y;
    let glow_gain = p0.z;
    let opacity = p0.w;
    if (opacity <= 0.001 || glow_gain <= 0.001) {
        discard;
    }

    let p1 = shader.params[1];
    let p2 = shader.params[2];
    let p3 = shader.params[3];
    let scale = max(p1.w, 1.0);
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    if (outside_corners(local, size, p2.x)) {
        discard;
    }

    let tint = wg_unpack_rgb(p1.x);
    let deep = wg_unpack_rgb(p1.y);
    let alt = wg_unpack_rgb(p1.z);
    let center = p2.yz;
    let span = max(p2.w, 60.0);
    let ptr = p3.xy;

    let diag = length(size);
    let slant = vec2<f32>(0.8 * local.x + 0.6 * local.y, -0.6 * local.x + 0.8 * local.y);
    let s1 = slant / 260.0;
    let cloud = 0.5
        + 0.28 * sin(s1.x + t * 0.08 + 0.7 * cos(s1.y - t * 0.06))
        + 0.22 * cos(s1.y * 1.15 - t * 0.05 + 0.6 * sin(s1.x * 0.85));
    let s2 = slant / 340.0;
    let second = 0.5
        + 0.28 * cos(s2.x - t * 0.06 + 0.7 * sin(s2.y + t * 0.07))
        + 0.22 * sin(s2.y * 1.1 + t * 0.05);

    var light = (deep * 0.62 + tint * 0.16) * wg_lamp(local, center, max(span * 1.6, diag * 0.22)) * (0.5 + cloud);
    light = light + tint * 0.30 * wg_lamp(local, center, span * 0.72) * (0.62 + 0.55 * cloud);
    light = light + alt * 0.10 * wg_lamp(local, size * vec2<f32>(0.12, 1.04), diag * 0.46) * (0.4 + 1.1 * second);
    light = light + deep * 0.22 * wg_lamp(local, size * vec2<f32>(0.98, 1.02), diag * 0.34) * (0.35 + second);
    if (seen > 0.001) {
        light = light + tint * 0.14 * seen * wg_lamp(local, ptr, 230.0);
    }
    light = light * glow_gain;

    let grain = (wg_hash(device + vec2<f32>(fract(t))) - 0.5) * 0.014 * step(0.004, light.r + light.g + light.b);
    let lit = clamp(light + vec3<f32>(grain), vec3<f32>(0.0), vec3<f32>(1.0));
    let out_a = clamp(max(lit.r, max(lit.g, lit.b)) * opacity, 0.0, 1.0);
    if (out_a <= 0.001) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(lit * opacity, out_a);
    }
    return vec4<f32>(clamp((lit * opacity) / out_a, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
}
