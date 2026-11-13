// A new chat's glow: light coming in from two corners in drifting smoky streaks, drawn as
// dots on an 8x8 Bayer grid whose number thins out with the light, each dot taking its colour
// from a ramp of the accent, so it never fills solid. With a pane picture, the picture's
// brightness decides how many dots the light lights, so the picture shows in the glow's dots.
//
// params: where the clock starts, how fast it runs, device pixels per point and the
// pane's corner radius in points; then the accent and how solid the dots are; then whether
// the picture given is the pane's.

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;

const PX_SIZE: f32 = 2.0;
const REACH: f32 = 0.92;

fn light(at: vec2<f32>, source: vec2<f32>, strength: f32, size: vec2<f32>) -> f32 {
    let apart = (at - source) * size / max(size.x, size.y);
    return strength * max(0.0, 1.0 - length(apart) / REACH);
}

// The light a little fainter toward the middle of the pane, where the welcome sits.
fn dim_middle(at: vec2<f32>, size: vec2<f32>) -> f32 {
    let from_middle = (at - 0.5) * size / max(size.x, size.y);
    return mix(0.55, 1.0, smoothstep(0.05, 0.35, length(from_middle)));
}

// The picture's brightness at `point`, measured from the pane's middle with y up, as the
// picture covers the pane.
fn brightness(point: vec2<f32>, size: vec2<f32>, texels: vec4<f32>) -> f32 {
    let aspect = texels.z / texels.w;
    let covered = max(size.x / aspect, size.y) * vec2<f32>(aspect, 1.0);
    let at = point / covered + 0.5;
    let atlas = vec2<f32>(textureDimensions(image));
    let uv = (texels.xy + vec2<f32>(at.x, 1.0 - at.y) * texels.zw) / atlas;
    let texel = textureSampleLevel(image, image_sampler, uv, 0.0);
    return dot(vec3<f32>(0.2126, 0.7152, 0.0722), texel.rgb);
}

fn ramp(level: f32, accent: vec3<f32>) -> vec3<f32> {
    let deep = accent * 0.3;
    let mid = accent * 0.7;
    let high = mix(accent, vec3<f32>(1.0), 0.3);
    if (level < 0.45) {
        return mix(deep, mid, level / 0.45);
    }
    if (level < 0.8) {
        return mix(mid, accent, (level - 0.45) / 0.35);
    }
    return mix(accent, high, (level - 0.8) / 0.2);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let t = 0.5 * (setup.x + globals.time * setup.y);
    let scale = setup.z;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    if (outside_corners(local, size, setup.w)) {
        discard;
    }
    let cell = floor(local / PX_SIZE);
    let at = (cell + 0.5) * PX_SIZE / size;

    let breath = 1.0 + 0.08 * sin(0.6 * t);
    let energy = breath * (light(at, vec2<f32>(1.05, 1.05), 1.05, size)
        + light(at, vec2<f32>(-0.05, -0.05), 0.65, size)) * dim_middle(at, size);
    if (energy <= 0.0) {
        discard;
    }
    let p = at * size / max(size.x, size.y);
    let streak = vec2<f32>(p.x + p.y, p.x - p.y) * vec2<f32>(1.3, 4.5);
    let drift = vec2<f32>(0.12 * t, 0.04 * t);
    let smoke = 0.6 * snoise(1.5 * streak + drift) + 0.3 * snoise(3.1 * streak + 7.0 - 1.6 * drift)
        + 0.1 * snoise(7.0 * streak + 3.0 + 2.5 * drift);
    var lit = energy * (0.8 + 0.45 * smoke);
    if (shader.params[2].x > 0.5) {
        let centre = (cell + 0.5) * PX_SIZE;
        let point = vec2<f32>(centre.x, size.y - centre.y) - 0.5 * size;
        lit = lit * (0.15 + 1.7 * brightness(point, size, shader.image));
    }
    let level = clamp(lit - 0.12, 0.0, 1.0);
    if (level <= bayer8(cell)) {
        discard;
    }

    let accent = shader.params[1];
    let colour = ramp(level, accent.rgb);
    let alpha = accent.a * (0.6 + 0.4 * level);
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour * alpha, alpha);
    }
    return vec4<f32>(colour, alpha);
}
