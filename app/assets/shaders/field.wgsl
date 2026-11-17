// A pane's surface: the grain in one colour over nothing, laid out in points from the
// pane's centre with y upward, faded out by the middle of the pane, the band a pane's
// content reads in.
//
// params: where the clock starts, how fast it runs, device pixels per point and the
// pane's corner radius in points, then the dots' colour.

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let scale = setup.z;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    let fade = band_fade(local.y / size.y);
    if (fade <= 0.0) {
        discard;
    }
    if (outside_corners(local, size, setup.w)) {
        discard;
    }
    let lit = grain(vec2<f32>(local.x, size.y - local.y) - 0.5 * size, setup.x + globals.time * setup.y);

    let colour = shader.params[1];
    let alpha = colour.a * fade * lit;
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * alpha, alpha);
    }
    return vec4<f32>(colour.rgb, alpha);
}
