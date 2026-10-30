// A picture the person chose for their panes, dithered as DitherEffect's ordered looks
// draw it: each dot takes one of three inks by its brightness, or one of the picture's
// own colours, with a glow over the light parts. It holds still.
//
// The picture comes with its colours laid on black and, in place of its opacity, how
// much glow each place gets, blurred ahead of time.
//
// params: device pixels per point, the pane's corner radius in points, how strongly the
// picture shows and whether it covers the whole pane; the dot size in points, the pattern
// (2, 4 or 8 for a Bayer grid, 0 for noise), contrast and brightness; how strong the glow
// is, whether the picture keeps its own colours, and the dark, middle and light inks, each
// as red, green and blue bytes in one number, negative for no ink.

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;

const PICTURE_STEPS: f32 = 4.0;

fn fade_down(y: f32) -> f32 {
    if (y < 0.4) {
        return mix(1.0, 0.45, y / 0.4);
    }
    if (y < 0.76) {
        return mix(0.45, 0.08, (y - 0.4) / 0.36);
    }
    return mix(0.08, 0.0, min((y - 0.76) / 0.24, 1.0));
}

fn ink(packed: f32) -> vec4<f32> {
    if (packed < 0.0) {
        return vec4<f32>(0.0);
    }
    let r = floor(packed / 65536.0);
    let g = floor((packed - r * 65536.0) / 256.0);
    let b = packed - r * 65536.0 - g * 256.0;
    return vec4<f32>(r / 255.0, g / 255.0, b / 255.0, 1.0);
}

fn bayer4(cell: vec2<f32>) -> f32 {
    let at = vec2<u32>(fract(cell / 4.0) * 4.0);
    let a = at.x ^ at.y;
    let b = at.y;
    return f32(((a & 1u) << 3u) | ((b & 1u) << 2u) | (a & 2u) | ((b & 2u) >> 1u)) / 16.0;
}

fn bayer2(cell: vec2<f32>) -> f32 {
    let at = vec2<u32>(fract(cell / 2.0) * 2.0);
    return f32(((at.x ^ at.y) << 1u) | at.y) / 4.0;
}

fn threshold(cell: vec2<f32>, pattern: f32) -> f32 {
    if (pattern > 6.0) {
        return bayer8(cell);
    }
    if (pattern > 3.0) {
        return bayer4(cell);
    }
    if (pattern > 1.0) {
        return bayer2(cell);
    }
    return fract(sin(dot(floor(cell), vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn sample_at(point: vec2<f32>, size: vec2<f32>, texels: vec4<f32>) -> vec4<f32> {
    let aspect = texels.z / texels.w;
    let covered = max(size.x / aspect, size.y) * vec2<f32>(aspect, 1.0);
    let at = point / covered + 0.5;
    let picture_uv = vec2<f32>(at.x, 1.0 - at.y);
    let atlas = vec2<f32>(textureDimensions(image));
    return textureSampleLevel(image, image_sampler, (texels.xy + picture_uv * texels.zw) / atlas, 0.0);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let look = shader.params[1];
    let light = shader.params[2];
    let scale = setup.x;
    let full = setup.w > 0.5;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    // A band's bottom edge runs across the pane, so only its top corners are the pane's.
    var rounded = size;
    if (!full) {
        rounded.y = size.y + 2.0 * setup.y;
    }
    if (outside_corners(local, rounded, setup.y)) {
        discard;
    }
    let point = vec2<f32>(local.x, size.y - local.y) - 0.5 * size;

    let dot_size = look.x;
    let cell = point / dot_size;
    let texel = sample_at((floor(cell) + 0.5) * dot_size, size, shader.image);
    let raw = dot(vec3<f32>(0.2126, 0.7152, 0.0722), texel.rgb);
    let lum = clamp((raw - 0.5) * (1.0 + look.z) + 0.5 + 0.5 * look.w, 0.0, 1.0);
    let edge = threshold(cell, look.y);

    var colour: vec3<f32>;
    var opacity: f32;
    if (light.y > 0.5) {
        let steps = floor(clamp(lum + (edge - 0.5) / PICTURE_STEPS, 0.0, 1.0) * PICTURE_STEPS + 0.5) / PICTURE_STEPS;
        colour = texel.rgb / max(raw, 0.04) * steps;
        opacity = steps;
    } else {
        var inks = array<vec4<f32>, 3>(ink(light.z), ink(light.w), ink(shader.params[3].x));
        let along = lum * 2.0;
        let lower = min(floor(along), 1.0);
        let chosen = u32(lower + step(edge, along - lower));
        colour = inks[chosen].rgb;
        opacity = inks[chosen].a;
    }

    let glow_tint = select(ink(light.w).rgb, texel.rgb / max(raw, 0.04), light.y > 0.5);
    let glow = sample_at(point, size, shader.image).a * light.x;
    var fade = 1.0;
    if (!full) {
        fade = fade_down(local.y / size.y);
    }
    let strength = fade * setup.z;
    let lit = colour * opacity + glow_tint * glow;
    let alpha = clamp(opacity + glow, 0.0, 1.0) * strength;
    if (alpha <= 0.0) {
        discard;
    }
    let premultiplied = min(lit * strength, vec3<f32>(alpha));
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(premultiplied, alpha);
    }
    return vec4<f32>(premultiplied / alpha, alpha);
}
