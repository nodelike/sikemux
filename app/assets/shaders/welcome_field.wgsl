// A new chat's calm, pointer-reactive particle field and subtle ambient glow, matching
// app.sikemux.com's dots.js on the GPU with zero CPU per-particle work and full idle sleep.
//
// params[0]: clock_offset, anchor_t, until, opacity_signed (sign < 0 for light themes)
// params[1]: packed tint, packed deep, packed alt, packed bright
// params[2]: scale, corner_radius, packed (mark_cx, mark_cy), packed (mark_span, quiet_bottom)
// params[3]: pointer_x, pointer_y, moved_at, packed (sweep_x, sweep_y, strength)

const WF_PI: f32 = 3.14159265;
const WF_SETTLES_FOR: f32 = 1.8;

fn wf_unpack_rgb(packed: f32) -> vec3<f32> {
    let b = floor(packed % 256.0);
    let g = floor((packed / 256.0) % 256.0);
    let r = floor(packed / 65536.0);
    return vec3<f32>(r, g, b) / 255.0;
}

fn wf_unpack_pair(packed: f32) -> vec2<f32> {
    let hi = floor(packed / 4096.0);
    let lo = packed - hi * 4096.0;
    return vec2<f32>(hi, lo);
}

fn wf_sim_time(clock_offset: f32, anchor_t: f32, until: f32, now: f32) -> f32 {
    if (until <= anchor_t) {
        return clock_offset;
    }
    let u = clamp(now, anchor_t, until);
    let settle_start = max(anchor_t, until - WF_SETTLES_FOR);
    let d = max(until - settle_start, 0.001);
    if (u <= settle_start) {
        return clock_offset + (u - anchor_t);
    }
    let s = u - settle_start;
    let eased = 0.5 * (s + (d / WF_PI) * sin(WF_PI * s / d));
    return clock_offset + (settle_start - anchor_t) + eased;
}

fn wf_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q = q + vec2<f32>(dot(q, q + vec2<f32>(45.32)));
    return fract(q.x * q.y);
}

fn wf_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f0 = fract(p);
    let f = f0 * f0 * (3.0 - 2.0 * f0);
    let a = wf_hash(i);
    let b = wf_hash(i + vec2<f32>(1.0, 0.0));
    let c = wf_hash(i + vec2<f32>(0.0, 1.0));
    let d = wf_hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Analytical value-noise gradient in one 4-corner lookup; curl is (grad.y, -grad.x).
fn wf_curl(p: vec2<f32>, t: f32) -> vec2<f32> {
    let q = p + vec2<f32>(t, -t * 0.7);
    let i = floor(q);
    let f = fract(q);
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let a = wf_hash(i);
    let b = wf_hash(i + vec2<f32>(1.0, 0.0));
    let c = wf_hash(i + vec2<f32>(0.0, 1.0));
    let d = wf_hash(i + vec2<f32>(1.0, 1.0));
    let k1 = b - a;
    let k2 = c - a;
    let k4 = a - b - c + d;
    let grad = du * vec2<f32>(k1 + k4 * u.y, k2 + k4 * u.x);
    return vec2<f32>(grad.y, -grad.x);
}

fn wf_lamp(at: vec2<f32>, origin: vec2<f32>, wide: f32) -> f32 {
    let off = at - origin;
    return exp(-dot(off, off) / max(wide * wide, 1.0));
}

fn wf_pointer_disp(
    p: vec2<f32>,
    ptr: vec2<f32>,
    trail: vec2<f32>,
    sweep: vec2<f32>,
    age: f32,
    strength: f32,
    felt: f32,
    give: f32,
    swirl: f32,
    spring: f32,
) -> vec2<f32> {
    let path = trail - ptr;
    let path_len2 = max(dot(path, path), 1.0);
    let along = clamp(dot(p - ptr, path) / path_len2, 0.0, 1.0);
    let off = (p - ptr) - path * along;
    let far2 = dot(off, off);
    let f2 = felt * felt;
    if (far2 > f2 * 6.5) {
        return vec2<f32>(0.0);
    }
    let spring_decay = (1.0 + spring * 0.6 * age) * exp(-spring * age);
    let wake_pulse = exp(-spring * 0.72 * age);
    let far = sqrt(far2) + 0.5;
    let dir = off / far;
    let near = strength * exp(-far2 / f2);
    let radial = dir * (near * felt * give * smoothstep(0.0, felt * 0.3, far)) * spring_decay;
    let speed = length(sweep);
    let wake_amt = min(speed / 620.0, 1.0) * near * wake_pulse;
    let ca = cos(swirl);
    let sa = sin(swirl);
    let swirled = vec2<f32>(sweep.x * ca - sweep.y * sa, sweep.x * sa + sweep.y * ca);
    let wake = (swirled / max(speed, 1.0)) * (wake_amt * felt * 0.42);
    return radial + wake;
}

fn wf_rot(v: vec2<f32>, cs: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(v.x * cs.x - v.y * cs.y, v.x * cs.y + v.y * cs.x);
}

fn wf_rot_inv(v: vec2<f32>, cs: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(v.x * cs.x + v.y * cs.y, -v.x * cs.y + v.y * cs.x);
}

fn wf_layer(
    local: vec2<f32>,
    t: f32,
    still: f32,
    soft: f32,
    is_dark: f32,
    layer_id: f32,
    cell_size: f32,
    margin: f32,
    float_amp: f32,
    rot_cs: vec2<f32>,
    offset: vec2<f32>,
    curl_vec: vec2<f32>,
    ptr_active: bool,
    ptr: vec2<f32>,
    trail: vec2<f32>,
    sweep: vec2<f32>,
    age: f32,
    strength: f32,
    felt: f32,
    give: f32,
    swirl: f32,
    spring: f32,
    fade: f32,
    core: f32,
    tone: f32,
    calm: f32,
    lit: f32,
    tint: vec3<f32>,
    deep: vec3<f32>,
    alt: vec3<f32>,
    bright: vec3<f32>,
    allow_bokeh: bool,
) -> vec4<f32> {
    var disp = curl_vec;
    if (ptr_active) {
        let p0 = wf_pointer_disp(local, ptr, trail, sweep, age, strength, felt, give, swirl, spring);
        let p1 = wf_pointer_disp(local - p0, ptr, trail, sweep, age, strength, felt, give, swirl, spring);
        disp = curl_vec + p1;
    }

    let rest = wf_rot(local - disp, rot_cs) + offset;
    let cell = floor(rest / cell_size);
    let rnd = cell_random(cell, 11.3 + layer_id * 37.7);
    let chance = vec2<f32>(
        wf_hash(cell * 1.17 + vec2<f32>(layer_id * 19.3, 3.7)),
        wf_hash(cell * 2.31 + vec2<f32>(7.1, layer_id * 23.9)),
    );

    let span_in = 1.0 - 2.0 * margin;
    let home_cell = (cell + margin + rnd * span_in) * cell_size;
    let float_off = vec2<f32>(
        sin(t * 0.2 + chance.x * 44.0),
        cos(t * 0.17 + chance.y * 51.0),
    ) * float_amp;
    var dist = length(rest - (home_cell + float_off));
    var dot_ptr_disp = disp - curl_vec;
    if (ptr_active) {
        let h_local = wf_rot_inv(home_cell + float_off - offset, rot_cs);
        dot_ptr_disp = wf_pointer_disp(h_local, ptr, trail, sweep, age, strength, felt, give, swirl, spring);
        dist = length(local - (h_local + curl_vec + dot_ptr_disp));
    }

    let y2 = chance.y * chance.y;
    let y5 = y2 * y2 * chance.y;
    var radius = 0.46 + 0.72 * y5 + 0.25 * clamp(tone, 0.0, 1.0);
    var blur = 0.0;
    var is_bokeh = false;

    if (allow_bokeh && chance.x > 0.74) {
        is_bokeh = true;
        radius = mix(2.0, 5.4, y2);
        blur = radius * 1.25;
    }

    let reach_px = radius + blur * 0.5 + soft + 0.5;
    if (dist > reach_px) {
        return vec4<f32>(0.0);
    }

    var alpha = 0.0;
    if (is_bokeh) {
        alpha = fade * mix(0.11, 0.045, chance.y) * smoothstep(0.08, 0.6, tone + 0.2) * calm;
        if (is_dark < 0.5) {
            alpha = alpha * 0.72;
        }
    } else {
        let cut = 0.05 + 0.88 * wf_hash(chance * 13.7 + vec2<f32>(layer_id));
        let shown = smoothstep(cut - 0.1, cut + 0.1, tone);
        if (shown <= 0.005) {
            return vec4<f32>(0.0);
        }
        let wake_boost = min(length(dot_ptr_disp) / 22.0, 1.0);
        alpha = shown * (0.24 + 0.48 * clamp(tone, 0.0, 1.0))
            * (1.0 + 0.85 * lit + 0.45 * wake_boost) * calm;
        alpha = alpha * (0.84 + 0.16 * sin(t * (0.2 + chance.x * 0.5) + chance.y * 60.0) * (1.0 - still));
    }

    if (alpha < 0.008) {
        return vec4<f32>(0.0);
    }

    let past = dist - radius;
    var cov = 0.0;
    if (is_bokeh) {
        let b = 1.0 - smoothstep(-radius, blur * 0.5, past);
        cov = b * sqrt(max(b, 0.0));
    } else {
        cov = 1.0 - smoothstep(-soft, soft, past);
    }
    let a = clamp(alpha * cov, 0.0, 1.0);
    if (a <= 0.002) {
        return vec4<f32>(0.0);
    }

    // Color lookups run only for pixels that actually hit a dot (~5% of screen area).
    let warmth = wf_noise(local / 330.0 + vec2<f32>(t * 0.011, -t * 0.008));
    let other = wf_noise(local / 210.0 - vec2<f32>(t * 0.008, t * 0.01) + vec2<f32>(17.0));
    var shade = mix(deep, tint, smoothstep(0.18, 0.62, warmth + 0.5 * core + 0.22 * (chance.x - 0.5)));
    shade = mix(shade, alt, smoothstep(0.5, 0.74, other + 0.18 * (chance.y - 0.5)) * (1.0 - 0.6 * core));
    let sparkle = clamp(core * 1.15 * chance.x * chance.x + 0.48 * step(0.955, chance.x) + 0.38 * lit * chance.y, 0.0, 0.92);
    shade = mix(shade, bright, sparkle);
    shade = shade * (0.8 + 0.38 * chance.x);

    return vec4<f32>(shade * a, a);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }

    let clock = shader.params[0];
    let opacity = abs(clock.w);
    if (opacity <= 0.001) {
        discard;
    }
    let is_dark = select(0.0, 1.0, clock.w >= 0.0);

    let geom = shader.params[2];
    let scale = max(geom.x, 1.0);
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    if (outside_corners(local, size, geom.y)) {
        discard;
    }

    let t = wf_sim_time(clock.x, clock.y, clock.z, globals.time);
    let still = select(0.0, 1.0, clock.z <= clock.y);

    let palette = shader.params[1];
    let tint = wf_unpack_rgb(palette.x);
    let deep = wf_unpack_rgb(palette.y);
    let alt = wf_unpack_rgb(palette.z);
    let bright = wf_unpack_rgb(palette.w);

    let mark_center = wf_unpack_pair(geom.z);
    let span_quiet = wf_unpack_pair(geom.w);
    let span = max(span_quiet.x, 60.0);
    let quiet_bottom = span_quiet.y;

    let ptr_raw = shader.params[3];
    let ptr = ptr_raw.xy;
    let age = max(0.0, globals.time - ptr_raw.z);
    let vel_strength = wf_unpack_rgb(ptr_raw.w);
    let sweep = (vel_strength.xy * 2.0 - 1.0) * 1500.0;
    let strength = vel_strength.z;
    let ptr_alive = strength > 0.001 && age < 4.8;
    let seen = select(0.0, strength * exp(-age * 2.0), ptr_alive);
    let raw_trail_off = sweep * 0.14;
    let trail_len = length(raw_trail_off);
    let trail = ptr - raw_trail_off * min(1.0, 145.0 / max(trail_len, 1.0));
    let ptr_dist2 = dot(local - ptr, local - ptr);
    let ptr_active = ptr_alive && (ptr_dist2 < 160000.0 || dot(local - trail, local - trail) < 160000.0);

    // Soft atmospheric glow (matching dots.js GLOW with lean 2-lookup noise).
    let diag = length(size);
    let slant = vec2<f32>(0.8 * local.x + 0.6 * local.y, -0.6 * local.x + 0.8 * local.y);
    let g_warp = wf_noise(slant / 350.0 - vec2<f32>(t * 0.01));
    let g_cloud = wf_noise(slant / 300.0 + vec2<f32>(t * 0.015, -t * 0.01) + vec2<f32>(1.1 * g_warp));
    let g_second = wf_noise(slant / 420.0 - vec2<f32>(t * 0.01, t * 0.015) + vec2<f32>(31.0 + g_warp));
    var light = (deep * 0.62 + tint * 0.16) * wf_lamp(local, mark_center, max(span * 1.65, diag * 0.24)) * (0.5 + g_cloud);
    light = light + tint * 0.30 * wf_lamp(local, mark_center, span * 0.78) * (0.62 + 0.55 * g_cloud);
    light = light + alt * 0.10 * wf_lamp(local, size * vec2<f32>(0.12, 1.04), diag * 0.46) * (0.4 + 1.1 * g_second);
    light = light + deep * 0.22 * wf_lamp(local, size * vec2<f32>(0.98, 1.02), diag * 0.34) * (0.35 + g_second);
    if (ptr_active) {
        light = light + tint * 0.13 * seen * wf_lamp(local, ptr, 215.0);
    }
    let glow_gain = select(0.07, 0.16, is_dark > 0.5);
    light = light * glow_gain;
    let grain_dither = (wf_hash(device + vec2<f32>(fract(t))) - 0.5) * 0.012 * step(0.004, light.r + light.g + light.b);
    let glow_rgb = clamp(light + vec3<f32>(grain_dither), vec3<f32>(0.0), vec3<f32>(1.0));
    let glow_a = max(glow_rgb.r, max(glow_rgb.g, glow_rgb.b));

    // Ambient field envelope, organic cloud modulation, clearing around mark, and quiet zone.
    let away = length(local - mark_center);
    let reach = diag * 0.64;
    let clearing = span * 0.50;
    let fade = max(0.0, 1.0 - away / reach) * smoothstep(clearing - 26.0, clearing + 34.0, away);
    let core = exp(-(away * away) / max(span * span * 0.62, 1.0));

    var dots_rgb = vec3<f32>(0.0);
    var dots_a = 0.0;

    if (fade > 0.004 || core > 0.02) {
        let quiet_half_w = min(size.x * 0.42, 330.0);
        let quiet_top = mark_center.y + span * 0.5 + 10.0;
        let quiet_rect = vec4<f32>(
            mark_center.x - quiet_half_w,
            quiet_top,
            mark_center.x + quiet_half_w,
            max(quiet_top + 48.0, quiet_bottom),
        );
        let calm = mix(0.14, 1.0, smoothstep(0.0, 108.0, rect_distance(local, quiet_rect)));

        let drift = local / 155.0;
        let cloud = wf_noise(drift + vec2<f32>(t * 0.025, -t * 0.015) + vec2<f32>(1.6 * g_cloud, 1.6 * g_second));
        let rich = select(0.88, 0.92, is_dark > 0.5);
        var tone = fade * smoothstep(0.12, 0.86, cloud) * 1.42 * rich;
        tone = tone + core * 0.48 * step(clearing * 0.85, away) * rich;

        let lit = select(0.0, seen * exp(-ptr_dist2 / 26000.0), ptr_active);
        let soft = 0.55 / scale;

        let base_curl = wf_curl(local / 220.0, t * 0.035);
        let c0 = base_curl * 16.0;
        let c1 = vec2<f32>(-base_curl.y, base_curl.x) * 13.0;
        let c2 = base_curl * 19.0;

        let l0 = wf_layer(
            local, t, still, soft, is_dark, 0.0, 9.8, 0.27, 0.95,
            vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 0.0), c0,
            ptr_active, ptr, trail, sweep, age, strength, 84.0, 0.42, 0.55, 4.4,
            fade, core, tone, calm, lit, tint, deep, alt, bright, false,
        );
        let l1 = wf_layer(
            local, t, still, soft, is_dark, 1.0, 12.2, 0.24, 1.2,
            vec2<f32>(0.9205, 0.3907), vec2<f32>(37.3, 91.7), c1,
            ptr_active, ptr, trail, sweep, age, strength, 116.0, 0.46, -0.65, 3.0,
            fade, core, tone, calm, lit, tint, deep, alt, bright, false,
        );
        let l2 = wf_layer(
            local, t, still, soft, is_dark, 2.0, 28.0, 0.37, 1.4,
            vec2<f32>(0.8572, -0.5150), vec2<f32>(113.1, 59.4), c2,
            ptr_active, ptr, trail, sweep, age, strength, 138.0, 0.36, 0.35, 2.2,
            fade, core, tone, calm, lit, tint, deep, alt, bright, true,
        );

        dots_rgb = l0.rgb + l1.rgb * (1.0 - l0.a);
        dots_a = l0.a + l1.a * (1.0 - l0.a);
        dots_rgb = dots_rgb + l2.rgb * (1.0 - dots_a);
        dots_a = dots_a + l2.a * (1.0 - dots_a);
    }

    let out_premul_rgb = (dots_rgb + glow_rgb * (1.0 - dots_a)) * opacity;
    let out_a = clamp((dots_a + glow_a * (1.0 - dots_a)) * opacity, 0.0, 1.0);
    if (out_a <= 0.001) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(out_premul_rgb, out_a);
    }
    return vec4<f32>(clamp(out_premul_rgb / out_a, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
}
