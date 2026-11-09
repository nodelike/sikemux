// The dither low in a first-run setup tile: the sky's noise through the Bayer grid in the
// agent's colour, so dots gather in soft clouds, densest along the bottom edge and thinning to
// nothing up the tile. A running tile's top also rises and falls.
//
// params: where the clock starts, how fast it runs, device pixels per point and the density at
// the bottom edge; the colour; then how far down the dots start, how far a running tile's top
// lifts, the tile's corner radius and how fast it rises and falls.

const CELL: f32 = 2.0;
const CLOUD: f32 = 70.0;
const DENSITY_FLOOR: f32 = 0.4;

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let tile = shader.params[2];
    let scale = setup.z;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;

    let corner = abs(local - 0.5 * size) - (0.5 * size - vec2<f32>(tile.z));
    if (length(max(corner, vec2<f32>(0.0))) > tile.z) {
        discard;
    }

    let top = tile.x - tile.y * (0.5 + 0.5 * sin(globals.time * tile.w));
    let fade = smoothstep(top, 1.0, local.y / size.y);
    if (fade <= 0.0) {
        discard;
    }

    let t = 0.5 * (setup.x + globals.time * setup.y);
    let cell = floor(device / scale / CELL);
    let uv = (cell + 0.5) * CELL / CLOUD;
    var noise = 0.5 * snoise(uv - vec2<f32>(0.0, 0.3 * t));
    noise = noise + 0.5 * snoise(2.0 * uv + vec2<f32>(0.0, 0.32 * t));
    let cloud = smoothstep(0.3, 0.9, 0.5 + 0.5 * noise);
    if (setup.w * mix(DENSITY_FLOOR, 1.0, cloud) * fade + bayer8(cell) < 1.0) {
        discard;
    }

    let colour = shader.params[1];
    let alpha = colour.a * min(1.0, fade * 1.6);
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * alpha, alpha);
    }
    return vec4<f32>(colour.rgb, alpha);
}
