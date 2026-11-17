// A pane's backdrop when its theme names a kind other than the grain: the gradient, the
// stars or the grid, laid out in points from the pane's top left, faded out by the middle
// of the pane unless it covers it all.
//
// params: where the clock starts, how fast it runs, device pixels per point and the pane's
// corner radius in points; the three inks, each as red, green and blue bytes in one number
// and negative for none, then how solid the whole is; the kind (1 gradient, 2 stars, 3 grid),
// whether it covers the whole pane, its seed and its scale; then the kind's own four numbers:
// the gradient's softness and dither, the stars' density, twinkle, drift and depth, the
// grid's dots, cell, weight and drift.

fn kind_ink(packed: f32) -> vec4<f32> {
    if (packed < 0.0) {
        return vec4<f32>(0.0);
    }
    let r = floor(packed / 65536.0);
    let g = floor((packed - r * 65536.0) / 256.0);
    let b = packed - r * 65536.0 - g * 256.0;
    return vec4<f32>(r / 255.0, g / 255.0, b / 255.0, 1.0);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let inks = shader.params[1];
    let kind = shader.params[2];
    let own = shader.params[3];
    let pixels = setup.z;
    let size = shader.bounds.zw / pixels;
    let local = (device - shader.bounds.xy) / pixels;
    var fade = 1.0;
    if (kind.y < 0.5) {
        fade = band_fade(local.y / size.y);
    }
    if (fade <= 0.0 || outside_corners(local, size, setup.w)) {
        discard;
    }
    let time = setup.x + globals.time * setup.y;
    let first = kind_ink(inks.x);
    let second = kind_ink(inks.y);
    let third = kind_ink(inks.z);
    var drawn = vec4<f32>(0.0);
    if (kind.x < 1.5) {
        drawn = gradient(local, size, time, kind.z, own.x, own.y, 2.0 * kind.w, first, second, third);
    } else if (kind.x < 2.5) {
        drawn = stars(local, time, kind.z, own.x, own.y, own.z, own.w, kind.w, first, second, third);
    } else {
        drawn = grid(local, time, own.x, own.y * kind.w, own.z, own.w, 1.0 / pixels, first);
    }
    let alpha = drawn.a * inks.w * fade;
    if (alpha <= 0.0) {
        discard;
    }
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(drawn.rgb * alpha, alpha);
    }
    return vec4<f32>(drawn.rgb, alpha);
}
