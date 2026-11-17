// The grid: lines, or dots where they would cross, on a flat sheet `cell` points apart,
// `weight` points thick, scrolling up and to the left at `drift` points a second. `point`
// is in points from the pane's top left and `time` the clock run at the kind's speed;
// `pixel` is a device pixel in points, which the edges are smoothed over. `dots` above
// one half draws dots. Answers the colour and how solid it is.

fn grid(
    point: vec2<f32>,
    time: f32,
    dots: f32,
    cell: f32,
    weight: f32,
    drift: f32,
    pixel: f32,
    first: vec4<f32>,
) -> vec4<f32> {
    let moved = point + vec2<f32>(time * drift);
    let within = moved - cell * floor(moved / cell);
    let apart = min(within, vec2<f32>(cell) - within);
    var edge: f32;
    if (dots > 0.5) {
        edge = length(apart) - weight;
    } else {
        edge = min(apart.x, apart.y) - 0.5 * weight;
    }
    let lit = 1.0 - smoothstep(-0.5 * pixel, 0.5 * pixel, edge);
    return vec4<f32>(first.rgb, first.a * lit);
}
