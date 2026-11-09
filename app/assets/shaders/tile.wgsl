// The dither low in a first-run setup tile: fine dots in the agent's colour, thinning upward.
// It holds still unless the tile says it is running, when its top rises and falls.
//
// params: where the clock starts, how fast it runs, device pixels per point and the dots'
// density; the colour; then how far down the dots start, how far a running tile's top lifts
// and the tile's corner radius.

const CELL: f32 = 2.0;

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let shape = shader.params[2];
    let scale = setup.z;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;

    let corner = abs(local - 0.5 * size) - (0.5 * size - vec2<f32>(shape.z));
    if (length(max(corner, vec2<f32>(0.0))) > shape.z) {
        discard;
    }

    let t = setup.x + globals.time * setup.y;
    let top = shape.x - shape.y * (0.5 + 0.5 * sin(t));
    let fade = smoothstep(top, 1.0, local.y / size.y);
    let cell = floor(local / CELL);
    if (fade <= 0.0 || any(local - cell * CELL >= vec2<f32>(1.0))) {
        discard;
    }
    if (setup.w * fade + bayer8(cell) < 1.0) {
        discard;
    }

    let colour = shader.params[1];
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * colour.a, colour.a);
    }
    return colour;
}
