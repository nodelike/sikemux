// The gradient: each ink a soft blob drifting slowly over the pane. `point` is in points
// from the pane's top left, `time` the clock run at the kind's speed, `softness` from 0 to 1
// how far a blob spreads, and `dither` from 0 to 1 how much of it is drawn in dots of `dot_size`
// points instead of smoothly. Answers the colour and how solid it is, not premultiplied.

fn gradient_blob(point: vec2<f32>, size: vec2<f32>, time: f32, phase: f32, reach: f32) -> f32 {
    let path = vec2<f32>(sin(0.11 * time + phase * 6.283), cos(0.083 * time + phase * 4.1));
    let centre = size * (vec2<f32>(0.5) + 0.38 * path);
    let apart = length(point - centre) / reach;
    return exp(-apart * apart);
}

fn gradient(
    point: vec2<f32>,
    size: vec2<f32>,
    time: f32,
    seed: f32,
    softness: f32,
    dither: f32,
    dot_size: f32,
    first: vec4<f32>,
    second: vec4<f32>,
    third: vec4<f32>,
) -> vec4<f32> {
    let reach = max(size.x, size.y) * mix(0.22, 0.6, softness);
    let phases = cell_random(vec2<f32>(seed, 1.0), seed);
    let a = gradient_blob(point, size, time, phases.x, reach) * first.a;
    let b = gradient_blob(point, size, time + 40.0, phases.y, reach) * second.a;
    let c = gradient_blob(point, size, time + 80.0, phases.x + phases.y, reach) * third.a;
    let total = a + b + c;
    if (total <= 0.0001) {
        return vec4<f32>(0.0);
    }
    let colour = (first.rgb * a + second.rgb * b + third.rgb * c) / total;
    let smooth_alpha = min(total, 1.0);
    let cell = point / max(dot_size, 1.0);
    let dotted = step(bayer8(cell), smooth_alpha);
    return vec4<f32>(colour, mix(smooth_alpha, dotted * 0.8, dither));
}
