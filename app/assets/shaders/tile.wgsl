// An agent's dither on the first-run setup's wall: one noise field across the window, gathered
// around an anchor of the agent's own and faded out at every edge and around what the tile
// says. A running tile's anchor rises and falls.
//
// params: where the clock starts, how fast it runs, device pixels per point and the density;
// the colour as three bytes, its opacity, a seed and how softly the dots clear the text; the
// anchor and its reach across and down; the mark's right and bottom with the label's right,
// then the label's height with the pill's width and bottom, in units of 4 points; where the
// wall is brightest, as bytes of the window's width; and how fast it rises and falls.

const CELL: f32 = 2.0;
const POINTS_PER_UNIT: f32 = 4.0;
const EDGE_FADE_ROOMY: f32 = 96.0;
const EDGE_FADE_COMPACT: f32 = 40.0;
const COMPACT_FEATHER: f32 = 100.0;

fn bytes(packed: f32) -> vec3<f32> {
    return vec3<f32>(floor(packed / 65536.0), floor(packed / 256.0) % 256.0, packed % 256.0);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let look = shader.params[1];
    let anchor = shader.params[2];
    let placed = shader.params[3];
    let scale = setup.z;
    let size = shader.bounds.zw / scale;
    let cell = floor((device - shader.bounds.xy) / scale / CELL);
    let at = (cell + 0.5) * CELL;

    let feather = look.w;
    let margin = feather * 0.06;
    let mark = bytes(placed.x) * POINTS_PER_UNIT;
    let others = bytes(placed.y) * POINTS_PER_UNIT;
    var clear = clear_of(at, vec4<f32>(0.0, 0.0, mark.x, mark.y), margin, feather);
    clear = min(
        clear,
        clear_of(at, vec4<f32>(0.0, size.y - others.x, mark.z, size.y), margin, feather),
    );
    if (others.z > 0.0) {
        clear = min(
            clear,
            clear_of(at, vec4<f32>(size.x - others.y, 0.0, size.x, others.z), margin, feather),
        );
    }
    let edge_fade = select(EDGE_FADE_COMPACT, EDGE_FADE_ROOMY, feather > COMPACT_FEATHER);
    let edge = clamp(min(min(at.x, at.y), min(size.x - at.x, size.y - at.y)) / edge_fade, 0.0, 1.0);
    let fade = clear * smoothstep(0.0, 1.0, edge);
    if (fade <= 0.0) {
        discard;
    }

    let width = globals.viewport_size.x / scale;
    let window_at = shader.bounds.xy / scale + at;
    let t = 0.5 * (setup.x + globals.time * setup.y);
    let wall = window_at / width * 2.8 + vec2<f32>(t * 0.04, -t * 0.025);
    let warp = layered_noise(wall * 0.7, 23.0) * 0.9;
    let gathered = smoothstep(
        0.0,
        1.0,
        clamp((layered_noise(wall + vec2<f32>(1.7 + warp, warp), 11.0) - 0.34) / 0.3, 0.0, 1.0),
    );

    let share = at / size;
    let wander = (layered_noise(share * 2.6, look.z) - 0.5) * 0.9;
    let lift = sin(globals.time * placed.w) * step(0.0001, placed.w) * 0.14;
    let reach = vec2<f32>((share.x - anchor.x) / anchor.z, (share.y - anchor.y - lift) / anchor.w);
    let far = length(reach) + wander;
    let amount = select(exp(-far * far * 2.3), 1.0, far <= 0.0);
    let bright = bytes(placed.z) / 255.0;
    let away = length(window_at / width - bright.xy);
    let light = 0.62 + 0.38 * exp(-away * away * 9.0);

    let value = min(0.5, setup.w * amount * (0.3 + 1.05 * gathered) * light * 1.1) * fade;
    if (value <= 0.002 || value + (cell_hash(cell, 3.0) - 0.5) * 0.03 + bayer8(cell) < 1.0) {
        discard;
    }
    let colour = bytes(look.x) / 255.0;
    let alpha = look.y * (0.6 + 0.4 * clamp(value / 0.42, 0.0, 1.0));
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour * alpha, alpha);
    }
    return vec4<f32>(colour, alpha);
}
