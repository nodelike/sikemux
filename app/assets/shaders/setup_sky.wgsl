// The first-run setup's sky: an ordered dither in one colour rising from the bottom of its
// area with a ragged top, kept clear of one block of text.
//
// params: device pixels per point, how high the sky reaches, its densest and a seed; the
// colour; how far and how softly it stays clear of the text; the text's rectangle.

const CELL: f32 = 2.0;

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let scale = setup.x;
    let level = setup.y;
    let densest = setup.z;
    let seed = setup.w;
    let size = shader.bounds.zw / scale;
    let cell = floor((device - shader.bounds.xy) / scale / CELL);
    let at = (cell + 0.5) * CELL;
    let across = at.x / size.x;
    let down = at.y / size.y;
    let top = level * (1.0 + (layered_noise(vec2<f32>(across * 1.7 + seed, 0.3), seed) - 0.5) * 0.7);
    let rise = (top - (1.0 - down)) / top;
    if (rise <= 0.0) {
        discard;
    }
    let clear = shader.params[2];
    let value = min(densest, pow(rise, 1.9) * densest * 1.15)
        * (1.0 + 1.5 * (layered_noise(vec2<f32>(across * 2.6, down * 5.2), seed + 4.0) - 0.5))
        * clear_of(at, shader.params[3], clear.x, clear.y);
    if (value <= 0.002 || value + (cell_hash(cell, 3.0) - 0.5) * 0.03 + bayer8(cell) < 1.0) {
        discard;
    }
    let colour = shader.params[1];
    let alpha = colour.a * (0.6 + 0.4 * clamp(value / 0.42, 0.0, 1.0));
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * alpha, alpha);
    }
    return vec4<f32>(colour.rgb, alpha);
}
