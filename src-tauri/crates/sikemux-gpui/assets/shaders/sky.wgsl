// The sky over the welcome screen and the release notes: the pane grain squeezed into
// a band, so it neither fills solid nor empties out, with each dot's colour taken from
// a gradient running across the sky, and fading out down it.
//
// params: where the clock starts, how fast it runs, device pixels per point and how far
// down the fade begins, then the gradient's three colours.

const PX_SIZE: f32 = 3.0;
const SCALE: f32 = 1.4;
const DENSITY_FLOOR: f32 = 0.12;
const DENSITY_PEAK: f32 = 0.42;

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
    let fade = 1.0 - clamp((local.y / size.y - setup.w) / (1.0 - setup.w), 0.0, 1.0);
    let point = vec2<f32>(local.x, size.y - local.y);

    let cell = (point - 0.5 * size) / PX_SIZE;
    let pixelized = (floor(cell) + 0.5) * PX_SIZE;
    let uv = (pixelized / SCALE + 0.5) * 0.001;
    var noise = 0.5 * snoise(uv - vec2<f32>(0.0, 0.3 * t));
    noise = noise + 0.5 * snoise(2.0 * uv + vec2<f32>(0.0, 0.32 * t));
    let shape = smoothstep(0.3, 0.9, 0.5 + 0.5 * noise);
    let lit = step(0.5, mix(DENSITY_FLOOR, DENSITY_PEAK, shape) + bayer8(cell) - 0.5);

    let across = pixelized / size;
    let at = clamp(across.x + 0.5 + 0.12 * sin(t + across.y * 4.0), 0.0, 1.0);
    var colour = mix(shader.params[2], shader.params[3], at * 2.0 - 1.0);
    if (at < 0.5) {
        colour = mix(shader.params[1], shader.params[2], at * 2.0);
    }
    let alpha = colour.a * fade * lit;
    if (fade <= 0.0 || alpha <= 0.0) {
        discard;
    }
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * alpha, alpha);
    }
    return vec4<f32>(colour.rgb, alpha);
}
