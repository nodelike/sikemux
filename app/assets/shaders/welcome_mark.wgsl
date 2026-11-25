// The Sikemux mark on a new chat's welcome screen: crisp individual luminous particles
// tracing the mark's outline and sculptural studio lighting, gently shimmering and reacting
// to the pointer, matching app.sikemux.com's dots.js.
//
// params[0]: clock_offset, anchor_t, until, shown_signed (sign < 0 for light themes)
// params[1]: packed tint, packed deep, packed alt, packed bright
// params[2]: scale, mark_pad, mark_span, 0
// params[3]: pointer_x, pointer_y, moved_at, packed (sweep_x, sweep_y, strength)

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;

const WM_PI: f32 = 3.14159265;
const WM_SETTLES_FOR: f32 = 1.8;

fn wm_unpack_rgb(packed: f32) -> vec3<f32> {
    let b = floor(packed % 256.0);
    let g = floor((packed / 256.0) % 256.0);
    let r = floor(packed / 65536.0);
    return vec3<f32>(r, g, b) / 255.0;
}

fn wm_sim_time(clock_offset: f32, anchor_t: f32, until: f32, now: f32) -> f32 {
    if (until <= anchor_t) {
        return clock_offset;
    }
    let u = clamp(now, anchor_t, until);
    let settle_start = max(anchor_t, until - WM_SETTLES_FOR);
    let d = max(until - settle_start, 0.001);
    if (u <= settle_start) {
        return clock_offset + (u - anchor_t);
    }
    let s = u - settle_start;
    let eased = 0.5 * (s + (d / WM_PI) * sin(WM_PI * s / d));
    return clock_offset + (settle_start - anchor_t) + eased;
}

fn wm_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q = q + vec2<f32>(dot(q, q + vec2<f32>(45.32)));
    return fract(q.x * q.y);
}

fn wm_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f0 = fract(p);
    let f = f0 * f0 * (3.0 - 2.0 * f0);
    let a = wm_hash(i);
    let b = wm_hash(i + vec2<f32>(1.0, 0.0));
    let c = wm_hash(i + vec2<f32>(0.0, 1.0));
    let d = wm_hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn wm_pointer_disp(
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
    if (strength <= 0.001 || age > 4.8) {
        return vec2<f32>(0.0);
    }
    let spring_decay = (1.0 + spring * 0.55 * age) * exp(-spring * age);
    let wake_pulse = exp(-spring * 0.72 * age);
    let path = trail - ptr;
    let path_len2 = max(dot(path, path), 1.0);
    let along = clamp(dot(p - ptr, path) / path_len2, 0.0, 1.0);
    let off = (p - ptr) - path * along;
    let far2 = dot(off, off);
    let f2 = felt * felt;
    if (far2 > f2 * 7.5) {
        return vec2<f32>(0.0);
    }
    let far = sqrt(far2) + 0.5;
    let dir = off / far;
    let near = strength * exp(-far2 / f2);
    let radial = dir * (near * felt * give * smoothstep(0.0, felt * 0.28, far)) * spring_decay;
    let speed = length(sweep);
    let wake_amt = min(speed / 580.0, 1.0) * near * wake_pulse;
    let ca = cos(swirl);
    let sa = sin(swirl);
    let swirled = vec2<f32>(sweep.x * ca - sweep.y * sa, sweep.x * sa + sweep.y * ca);
    let wake = (swirled / max(speed, 1.0)) * (wake_amt * felt * 0.44);
    return radial + wake;
}

fn wm_sample_mark(uv: vec2<f32>, img_rect: vec4<f32>, atlas: vec2<f32>) -> f32 {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return 0.0;
    }
    let sample_uv = (img_rect.xy + uv * img_rect.zw) / atlas;
    return textureSampleLevel(image, image_sampler, sample_uv, 0.0).a;
}

fn wm_layer(
    local: vec2<f32>,
    t: f32,
    still: f32,
    shown: f32,
    soft: f32,
    pad: f32,
    span: f32,
    layer_id: f32,
    cell_size: f32,
    offset: vec2<f32>,
    ptr: vec2<f32>,
    trail: vec2<f32>,
    sweep: vec2<f32>,
    age: f32,
    strength: f32,
    felt: f32,
    give: f32,
    swirl: f32,
    spring: f32,
    lit_ptr: f32,
    tint: vec3<f32>,
    deep: vec3<f32>,
    alt: vec3<f32>,
    bright: vec3<f32>,
    img_rect: vec4<f32>,
    atlas: vec2<f32>,
    draw_outline: bool,
) -> vec4<f32> {
    let center = vec2<f32>(pad + span * 0.5);
    let p_off = local + offset;
    let ptr_active = strength > 0.001 && age <= 4.8;
    var disp = vec2<f32>(0.0);
    if (ptr_active) {
        let d0 = wm_pointer_disp(local, ptr, trail, sweep, age, strength, felt, give, swirl, spring);
        disp = wm_pointer_disp(local - d0, ptr, trail, sweep, age, strength, felt, give, swirl, spring);
    }
    let rest = p_off - disp;

    let base_cell = floor(rest / cell_size - 0.5);
    let edge_step = 1.65 / span;
    var acc_rgb = vec3<f32>(0.0);
    var acc_a = 0.0;

    for (var dy = 0; dy <= 1; dy = dy + 1) {
        for (var dx = 0; dx <= 1; dx = dx + 1) {
            let cell = base_cell + vec2<f32>(f32(dx), f32(dy));
            let rnd = cell_random(cell, 5.1 + layer_id * 29.3);
            let chance = vec2<f32>(
                wm_hash(cell * 1.37 + vec2<f32>(layer_id * 13.7, 2.3)),
                wm_hash(cell * 2.51 + vec2<f32>(5.9, layer_id * 17.1)),
            );

            let home_local = (cell + 0.14 + rnd * 0.72) * cell_size - offset;
            let breathe = vec2<f32>(
                sin(t * 0.45 + chance.x * 31.0),
                cos(t * 0.38 + chance.y * 37.0),
            ) * (0.45 * (1.0 - still));
            let away_vec = home_local - center;
            let away_dir = away_vec / max(length(away_vec), 1.0);
            let disperse = away_dir * ((1.0 - shown) * (14.0 + 22.0 * chance.x));
            var dot_disp = vec2<f32>(0.0);
            if (ptr_active) {
                dot_disp = wm_pointer_disp(home_local, ptr, trail, sweep, age, strength, felt, give, swirl, spring);
            }
            let dot_pos = home_local + breathe + disperse + dot_disp;
            let dist = length(local - dot_pos);
            if (dist > 1.55 * max(0.86, span / 118.0) + soft + 0.5) {
                continue;
            }

            let uv = (home_local - vec2<f32>(pad)) / span;
            let a_c = wm_sample_mark(uv, img_rect, atlas);
            if (a_c < 0.42) {
                continue;
            }

            let u = uv.x - 0.5;
            let v = uv.y - 0.5;
            let a_l = wm_sample_mark(uv - vec2<f32>(edge_step, 0.0), img_rect, atlas);
            let a_r = wm_sample_mark(uv + vec2<f32>(edge_step, 0.0), img_rect, atlas);
            let a_u = wm_sample_mark(uv - vec2<f32>(0.0, edge_step), img_rect, atlas);
            let a_d = wm_sample_mark(uv + vec2<f32>(0.0, edge_step), img_rect, atlas);
            let is_edge = min(min(a_l, a_r), min(a_u, a_d)) < 0.52;

            let shade_factor = clamp(0.42 + u * 0.55 + v * 0.95, 0.0, 1.0);
            if (draw_outline) {
                if (!is_edge && chance.x < shade_factor * shade_factor * 0.48) {
                    continue;
                }
            } else {
                if (is_edge || chance.x > (1.0 - shade_factor * 0.72)) {
                    continue;
                }
            }

            let shine = sin(t * 0.5 + u * 7.0 + v * 9.0 + wm_noise(vec2<f32>(u, v) * 6.0 + vec2<f32>(t * 0.12)) * 4.0) * (1.0 - still);
            let base_r = select(0.56 + 0.46 * chance.y * chance.y, 0.74 + 0.24 * chance.y, is_edge);
            let radius = base_r * max(0.86, span / 118.0) * (1.0 + 0.03 * shine);

            if (dist > radius + soft + 0.5) {
                continue;
            }

            let fade_in = smoothstep(chance.y * 0.38, chance.y * 0.38 + 0.62, shown);
            let light = max(0.58, 1.14 - 0.46 * shade_factor + (chance.y - 0.5) * 0.26);
            let cov = 1.0 - smoothstep(-soft, soft, dist - radius);
            let a = clamp(light * (0.94 + 0.06 * shine) * fade_in * cov, 0.0, 1.0);
            if (a <= 0.003) {
                continue;
            }

            let lit = clamp(0.52 - u * 0.62 - v * 1.02 + (chance.x - 0.5) * 0.26, 0.0, 1.0);
            let rim = max(0.0, (u + v) * 0.9) * select(0.45, 1.25, is_edge);
            var shade = mix(mix(deep, tint, 0.3), tint, clamp(lit / 0.45, 0.0, 1.0));
            if (lit > 0.45) {
                shade = mix(tint, bright, (lit - 0.45) / 0.55);
            }
            shade = mix(shade, alt, min(0.85, rim));
            shade = mix(shade, shade * 1.08, max(shine, 0.0) * 0.6);
            shade = mix(shade, bright, 0.30 * lit_ptr * chance.y);
            shade = clamp(shade, vec3<f32>(0.0), vec3<f32>(1.0));

            acc_rgb = acc_rgb + shade * a * (1.0 - acc_a);
            acc_a = acc_a + a * (1.0 - acc_a);
        }
    }

    return vec4<f32>(acc_rgb, acc_a);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }

    let clock = shader.params[0];
    let shown = abs(clock.w);
    if (shown <= 0.001) {
        discard;
    }

    let geom = shader.params[2];
    let scale = max(geom.x, 1.0);
    let pad = geom.y;
    let span = max(geom.z, 32.0);
    let local = (device - shader.bounds.xy) / scale;
    let center = vec2<f32>(pad + span * 0.5);
    if (length(local - center) > span * 0.5 + pad) {
        discard;
    }

    let t = wm_sim_time(clock.x, clock.y, clock.z, globals.time);
    let still = select(0.0, 1.0, clock.z <= clock.y);

    let palette = shader.params[1];
    let tint = wm_unpack_rgb(palette.x);
    let deep = wm_unpack_rgb(palette.y);
    let alt = wm_unpack_rgb(palette.z);
    let bright = wm_unpack_rgb(palette.w);

    let ptr_raw = shader.params[3];
    let ptr = ptr_raw.xy;
    let age = max(0.0, globals.time - ptr_raw.z);
    let vel_strength = wm_unpack_rgb(ptr_raw.w);
    let sweep = (vel_strength.xy * 2.0 - 1.0) * 1500.0;
    let strength = vel_strength.z;
    let seen = select(0.0, strength * exp(-age * 2.0), strength > 0.001 && age < 4.8);
    let raw_trail_off = sweep * 0.14;
    let trail_len = length(raw_trail_off);
    let trail = ptr - raw_trail_off * min(1.0, 120.0 / max(trail_len, 1.0));
    let lit_ptr = seen * exp(-dot(local - ptr, local - ptr) / 14000.0);

    let atlas = vec2<f32>(textureDimensions(image));
    let soft = 0.55 / scale;
    let felt0 = clamp(span * 0.42, 36.0, 72.0);
    let felt1 = clamp(span * 0.56, 48.0, 92.0);

    let l0 = wm_layer(
        local, t, still, shown, soft, pad, span, 0.0, 3.1, vec2<f32>(0.0, 0.0),
        ptr, trail, sweep, age, strength, felt0, 0.30, 0.50, 5.4,
        lit_ptr, tint, deep, alt, bright, shader.image, atlas, true,
    );
    let l1 = wm_layer(
        local, t, still, shown, soft, pad, span, 1.0, 4.3, vec2<f32>(17.3, 29.1),
        ptr, trail, sweep, age, strength, felt1, 0.34, -0.60, 3.8,
        lit_ptr, tint, deep, alt, bright, shader.image, atlas, false,
    );

    let out_premul_rgb = l0.rgb + l1.rgb * (1.0 - l0.a);
    let out_a = clamp(l0.a + l1.a * (1.0 - l0.a), 0.0, 1.0);
    if (out_a <= 0.002) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(out_premul_rgb, out_a);
    }
    return vec4<f32>(clamp(out_premul_rgb / out_a, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
}
