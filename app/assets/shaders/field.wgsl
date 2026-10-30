// A pane's surface: dots on an 8x8 Bayer grid over drifting simplex noise, in one
// colour over nothing, as Paper Shaders' dithering draws it. The pattern is laid out
// in points from the pane's centre with y upward, as the WebGL original sees its canvas,
// and fades out by the middle of the pane, the band a pane's content reads in.
//
// params: where the clock starts, how fast it runs, device pixels per point and the
// pane's corner radius in points, then the dots' colour.

const PX_SIZE: f32 = 3.0;
const SCALE: f32 = 2.4;

fn fade_down(y: f32) -> f32 {
    if (y < 0.2) {
        return mix(1.0, 0.45, y / 0.2);
    }
    if (y < 0.38) {
        return mix(0.45, 0.08, (y - 0.2) / 0.18);
    }
    return mix(0.08, 0.0, min((y - 0.38) / 0.12, 1.0));
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
    let fade = fade_down(local.y / size.y);
    if (fade <= 0.0) {
        discard;
    }
    if (outside_corners(local, size, setup.w)) {
        discard;
    }
    let point = vec2<f32>(local.x, size.y - local.y);

    let cell = (point - 0.5 * size) / PX_SIZE;
    let pixelized = (floor(cell) + 0.5) * PX_SIZE;
    let uv = (pixelized / SCALE + 0.5) * 0.001;
    var noise = 0.5 * snoise(uv - vec2<f32>(0.0, 0.3 * t));
    noise = noise + 0.5 * snoise(2.0 * uv + vec2<f32>(0.0, 0.32 * t));
    let shape = smoothstep(0.3, 0.9, 0.5 + 0.5 * noise);
    let lit = step(0.5, shape + bayer8(cell) - 0.5);

    let colour = shader.params[1];
    let alpha = colour.a * fade * lit;
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * alpha, alpha);
    }
    return vec4<f32>(colour.rgb, alpha);
}
