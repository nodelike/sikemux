// The stars: a field of points in up to three depths, each nearer depth larger, brighter
// and drifting faster. `point` is in points from the pane's top left and `time` the clock
// run at the kind's speed. `density` from 0 to 1 is how many, `twinkle` from 0 to 1 how much
// each one breathes, `drift` from 0 to 1 how fast they cross the pane, `depth` from 1 to 3
// how many depths, and `scale` the size of everything. Answers the colour and how solid it is.

fn star_layer(
    point: vec2<f32>,
    time: f32,
    seed: f32,
    layer: f32,
    density: f32,
    twinkle: f32,
    drift: f32,
    scale: f32,
    first: vec4<f32>,
    second: vec4<f32>,
    third: vec4<f32>,
) -> vec4<f32> {
    let near = 1.0 - layer / 3.0;
    let cell_size = mix(46.0, 18.0, density) * scale * mix(1.6, 1.0, near);
    let moved = point + vec2<f32>(time * drift * mix(1.5, 6.0, near), 0.0);
    let cell = floor(moved / cell_size);
    let chance = cell_random(cell, seed + layer * 17.0);
    if (chance.x > mix(0.35, 0.8, density)) {
        return vec4<f32>(0.0);
    }
    let radius = mix(0.55, 1.1, near) * scale;
    let room = cell_size - 4.0 * radius;
    let spot = (cell + 0.0) * cell_size + 2.0 * radius + cell_random(cell + 7.0, seed + layer) * room;
    let apart = length(moved - spot);
    let lit = 1.0 - smoothstep(radius * 0.5, radius, apart);
    if (lit <= 0.0) {
        return vec4<f32>(0.0);
    }
    let breathe = 1.0 - twinkle * (0.5 + 0.5 * sin(time * mix(0.9, 2.3, chance.y) + chance.x * 40.0));
    let ink = pick_ink(chance.y, first, second, third);
    return vec4<f32>(ink.rgb, ink.a * lit * breathe * mix(0.55, 1.0, near));
}

fn stars(
    point: vec2<f32>,
    time: f32,
    seed: f32,
    density: f32,
    twinkle: f32,
    drift: f32,
    depth: f32,
    scale: f32,
    first: vec4<f32>,
    second: vec4<f32>,
    third: vec4<f32>,
) -> vec4<f32> {
    var brightest = vec4<f32>(0.0);
    for (var layer = 0.0; layer < 3.0; layer = layer + 1.0) {
        if (layer >= depth) {
            break;
        }
        let star = star_layer(point, time, seed, layer, density, twinkle, drift, scale, first, second, third);
        if (star.a > brightest.a) {
            brightest = star;
        }
    }
    return brightest;
}
