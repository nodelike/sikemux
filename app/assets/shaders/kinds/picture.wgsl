// A picture dithered as DitherEffect's ordered looks draw it: each dot takes one of three
// inks by its brightness, or one of the picture's own colours in four steps, with a glow
// over the light parts. It holds still.
//
// Each client reads the picture itself. It takes the colour at `picture_cover` of
// `picture_dot(point)` and the glow at `picture_cover(point)`, then `picture` dithers them.

const PICTURE_STEPS: f32 = 4.0;

// Where `point`, in points from the middle with y up, falls on a picture of `aspect`
// covering `size`, from the picture's top-left corner as 0 to 1.
fn picture_cover(point: vec2<f32>, size: vec2<f32>, aspect: f32) -> vec2<f32> {
    let covered = max(size.x / aspect, size.y) * vec2<f32>(aspect, 1.0);
    let at = point / covered + 0.5;
    return vec2<f32>(at.x, 1.0 - at.y);
}

// The middle of the dot `point` falls in.
fn picture_dot(point: vec2<f32>, dot_size: f32) -> vec2<f32> {
    return (floor(point / dot_size) + 0.5) * dot_size;
}

// How much of the picture shows `down` the band it covers, from 0 at its top to 1 at its bottom.
fn picture_fade(down: f32) -> f32 {
    if (down < 0.4) {
        return mix(1.0, 0.45, down / 0.4);
    }
    if (down < 0.76) {
        return mix(0.45, 0.08, (down - 0.4) / 0.36);
    }
    return mix(0.08, 0.0, min((down - 0.76) / 0.24, 1.0));
}

// The dot at `point`, premultiplied. Inks with no opacity are none and leave their dots
// clear; `shown` scales the whole.
fn picture(
    point: vec2<f32>,
    texel: vec3<f32>,
    glowing: f32,
    dot_size: f32,
    pattern: f32,
    contrast: f32,
    brightness: f32,
    glow: f32,
    own_colours: f32,
    dark: vec4<f32>,
    middle: vec4<f32>,
    light: vec4<f32>,
    shown: f32,
) -> vec4<f32> {
    let cell = point / dot_size;
    let raw = dot(vec3<f32>(0.2126, 0.7152, 0.0722), texel);
    let lum = clamp((raw - 0.5) * (1.0 + contrast) + 0.5 + 0.5 * brightness, 0.0, 1.0);
    let edge = threshold(cell, pattern);
    let own = texel / max(raw, 0.04);

    var colour: vec3<f32>;
    var opacity: f32;
    if (own_colours > 0.5) {
        let steps = floor(clamp(lum + (edge - 0.5) / PICTURE_STEPS, 0.0, 1.0) * PICTURE_STEPS + 0.5) / PICTURE_STEPS;
        colour = own * steps;
        opacity = steps;
    } else {
        let along = lum * 2.0;
        let lower = min(floor(along), 1.0);
        let chosen = lower + step(edge, along - lower);
        var picked = light;
        if (chosen < 0.5) {
            picked = dark;
        } else if (chosen < 1.5) {
            picked = middle;
        }
        colour = picked.rgb;
        opacity = select(0.0, 1.0, picked.a > 0.0);
    }

    let tint = select(middle.rgb, own, own_colours > 0.5);
    let glowed = glowing * glow;
    let lit = colour * opacity + tint * glowed;
    let alpha = clamp(opacity + glowed, 0.0, 1.0) * shown;
    return vec4<f32>(min(lit * shown, vec3<f32>(alpha)), alpha);
}
