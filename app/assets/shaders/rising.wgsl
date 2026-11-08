// The sky in the first-run setup's column: the pane grain in one colour, rising from the bottom
// and thinning out upward and across, so the dots grow sparse rather than only dim.
//
// params: where the clock starts, how fast it runs, device pixels per point and the densest the
// dots get; the colour; then where the rise starts and is full, and where the fade across starts
// and ends, as shares of the column.

const DOT_SIZE: f32 = 2.0;
const NOISE_STEP: f32 = 3.0;
const SCALE: f32 = 1.4;
const DENSITY_FLOOR: f32 = 0.12;

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
    let reach = shader.params[2];
    let fade = smoothstep(reach.x, reach.y, local.y / size.y)
        * (1.0 - smoothstep(reach.z, reach.w, local.x / size.x));
    if (fade <= 0.0) {
        discard;
    }
    let point = vec2<f32>(local.x, size.y - local.y);

    let cell = (point - 0.5 * size) / DOT_SIZE;
    let spot = (floor(cell) + 0.5) * NOISE_STEP;
    let uv = (spot / SCALE + 0.5) * 0.001;
    var noise = 0.5 * snoise(uv - vec2<f32>(0.0, 0.3 * t));
    noise = noise + 0.5 * snoise(2.0 * uv + vec2<f32>(0.0, 0.32 * t));
    let shape = smoothstep(0.3, 0.9, 0.5 + 0.5 * noise);
    let density = mix(DENSITY_FLOOR, setup.w, shape) * fade;
    let lit = step(0.5, density + bayer8(cell) - 0.5);

    let colour = shader.params[1];
    let alpha = colour.a * min(1.0, fade * 1.6) * lit;
    if (alpha <= 0.0) {
        discard;
    }
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * alpha, alpha);
    }
    return vec4<f32>(colour.rgb, alpha);
}
