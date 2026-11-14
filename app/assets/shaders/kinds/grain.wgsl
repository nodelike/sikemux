// The grain: dots on an 8x8 Bayer grid over drifting simplex noise, as Paper Shaders'
// dithering draws it. `point` is in points from the middle with y up, and `time` is the
// clock already run at the grain's speed. It answers whether the dot there is lit.

const GRAIN_DOT: f32 = 3.0;
const GRAIN_SCALE: f32 = 2.4;

fn grain(point: vec2<f32>, time: f32) -> f32 {
    let t = 0.5 * time;
    let cell = point / GRAIN_DOT;
    let pixelized = (floor(cell) + 0.5) * GRAIN_DOT;
    let uv = (pixelized / GRAIN_SCALE + 0.5) * 0.001;
    var noise = 0.5 * snoise(uv - vec2<f32>(0.0, 0.3 * t));
    noise = noise + 0.5 * snoise(2.0 * uv + vec2<f32>(0.0, 0.32 * t));
    let shape = smoothstep(0.3, 0.9, 0.5 + 0.5 * noise);
    return step(0.5, shape + bayer8(cell) - 0.5);
}
