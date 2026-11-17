// How much of a kind shows at `y`, the share of the way down a pane: fading out by the
// middle, the band a pane's content reads in.

fn band_fade(y: f32) -> f32 {
    if (y < 0.2) {
        return mix(1.0, 0.45, y / 0.2);
    }
    if (y < 0.38) {
        return mix(0.45, 0.08, (y - 0.2) / 0.18);
    }
    return mix(0.08, 0.0, min((y - 0.38) / 0.12, 1.0));
}

// A repeatable pair of random values from 0 to 1 for a cell, for one seed.
fn cell_random(cell: vec2<f32>, seed: f32) -> vec2<f32> {
    let p = vec2<f32>(dot(cell, vec2<f32>(127.1, 311.7)), dot(cell, vec2<f32>(269.5, 183.3))) + seed * 0.618;
    return fract(sin(p) * 43758.5453);
}

// One of up to three inks, picked by `choice` from 0 to 1 among those with any opacity.
fn pick_ink(choice: f32, first: vec4<f32>, second: vec4<f32>, third: vec4<f32>) -> vec4<f32> {
    let count = 1.0 + step(0.001, second.a) + step(0.001, third.a);
    let at = floor(choice * count);
    if (at < 0.5) {
        return first;
    }
    if (at < 1.5 && second.a > 0.001) {
        return second;
    }
    return select(second, third, third.a > 0.001);
}
