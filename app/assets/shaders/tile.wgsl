// An agent's dither on the first-run setup's wall: a cloud gathered around an anchor of the
// agent's own, dense at its centre and thinning through every step of the ordered pattern to
// nothing, kept clear of what the tile says and faded out at every edge. It swells and settles
// on a phase of its own, and a running tile's cloud rises and falls as well.
//
// params: where the clock starts, how fast it runs, device pixels per point and the peak
// density; the colour as three bytes, its opacity, a seed and how softly the dots clear the
// text; the anchor and its reach across and down; the mark's right and bottom with the label's
// right, then the label's height with the pill's width and bottom, in units of 4 points, and
// how fast a running tile rises and falls.

const CELL: f32 = 1.5;
const CELL_COMPACT: f32 = 1.0;
const POINTS_PER_UNIT: f32 = 4.0;
// How much of a tile's shorter side its field takes to fade in from each edge.
const EDGE_FADE: f32 = 0.26;
// A tile shorter than this has its dots closer together.
const SMALL_TILE: f32 = 150.0;
// The room kept around a tile's words rounds and wanders, so a field has no straight edge.
const CUTOUT_BLEND: f32 = 40.0;
const CUTOUT_WOBBLE: f32 = 30.0;
const CUTOUT_WOBBLE_SIZE: f32 = 46.0;
// How the cloud swells: its least and its range, and how fast in radians a second.
const BREATH_LEAST: f32 = 0.72;
const BREATH_RANGE: f32 = 0.42;
const BREATH_RATE: f32 = 1.05;
// How far the cloud's shape strays from round, at about this many points across.
const SHAPE_SIZE: f32 = 70.0;

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
    let feather = look.w;
    let pitch = select(CELL_COMPACT, CELL, min(size.x, size.y) > SMALL_TILE);
    let cell = floor((device - shader.bounds.xy) / scale / pitch);
    let at = (cell + 0.5) * pitch;

    let margin = feather * 0.29;
    let mark = bytes(placed.x) * POINTS_PER_UNIT;
    let others = bytes(placed.y) * POINTS_PER_UNIT;
    var nearest = blended_nearest(
        rect_distance(at, vec4<f32>(0.0, 0.0, mark.x, mark.y)),
        rect_distance(at, vec4<f32>(0.0, size.y - others.x, mark.z, size.y)),
        CUTOUT_BLEND,
    );
    if (others.z > 0.0) {
        nearest = min(nearest, rect_distance(at, vec4<f32>(size.x - others.y, 0.0, size.x, others.z)));
    }
    nearest += (layered_noise(at / CUTOUT_WOBBLE_SIZE, look.z + 5.0) - 0.5) * CUTOUT_WOBBLE;
    let clear = smoothstep(0.0, 1.0, clamp((nearest - margin) / feather, 0.0, 1.0));
    let edge = clamp(
        min(min(at.x, at.y), min(size.x - at.x, size.y - at.y)) / (min(size.x, size.y) * EDGE_FADE),
        0.0,
        1.0,
    );
    let fade = clear * smoothstep(0.0, 1.0, edge);
    if (fade <= 0.0) {
        discard;
    }

    let t = setup.x + globals.time * setup.y;
    let moving = step(0.0001, setup.y);
    let breath = mix(1.0, BREATH_LEAST + BREATH_RANGE * sin(t * BREATH_RATE + look.z), moving);
    let share = at / size;
    let lift = sin(globals.time * placed.w) * step(0.0001, placed.w) * 0.14;
    let reach = vec2<f32>((share.x - anchor.x) / anchor.z, (share.y - anchor.y - lift) / anchor.w);
    let shape = layered_noise(at / SHAPE_SIZE, look.z);
    let amount = exp(-dot(reach, reach) * 1.5) * (0.72 + 0.56 * shape);

    let value = clamp(setup.w * breath * amount, 0.0, 1.0) * fade;
    if (value + bayer8(cell) < 1.0) {
        discard;
    }
    let colour = bytes(look.x) / 255.0;
    let alpha = look.y * (0.6 + 0.4 * clamp(value / 0.6, 0.0, 1.0));
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour * alpha, alpha);
    }
    return vec4<f32>(colour, alpha);
}
