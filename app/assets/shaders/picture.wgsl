// The picture the person chose for their panes, as the `picture` kind dithers it.
//
// The picture comes with its colours laid on black and, in place of its opacity, how
// much glow each place gets, blurred ahead of time.
//
// params: device pixels per point, the pane's corner radius in points, how strongly the
// picture shows and whether it covers the whole pane; the dot size in points, the pattern
// (2, 4 or 8 for a Bayer grid, 0 for noise), contrast and brightness; how strong the glow
// is, whether the picture keeps its own colours, and the dark, middle and light inks, each
// as red, green and blue bytes in one number, negative for no ink.

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;

fn ink(packed: f32) -> vec4<f32> {
    if (packed < 0.0) {
        return vec4<f32>(0.0);
    }
    let r = floor(packed / 65536.0);
    let g = floor((packed - r * 65536.0) / 256.0);
    let b = packed - r * 65536.0 - g * 256.0;
    return vec4<f32>(r / 255.0, g / 255.0, b / 255.0, 1.0);
}

fn sample_at(point: vec2<f32>, size: vec2<f32>, texels: vec4<f32>) -> vec4<f32> {
    let picture_uv = picture_cover(point, size, texels.z / texels.w);
    let atlas = vec2<f32>(textureDimensions(image));
    return textureSampleLevel(image, image_sampler, (texels.xy + picture_uv * texels.zw) / atlas, 0.0);
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
    let light = shader.params[2];
    let scale = setup.x;
    let full = setup.w > 0.5;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    // A band's bottom edge runs across the pane, so only its top corners are the pane's.
    var rounded = size;
    if (!full) {
        rounded.y = size.y + 2.0 * setup.y;
    }
    if (outside_corners(local, rounded, setup.y)) {
        discard;
    }
    let point = vec2<f32>(local.x, size.y - local.y) - 0.5 * size;

    var fade = 1.0;
    if (!full) {
        fade = picture_fade(local.y / size.y);
    }
    let dot_size = look.x;
    let coloured = picture(
        point,
        sample_at(picture_dot(point, dot_size), size, shader.image).rgb,
        sample_at(point, size, shader.image).a,
        dot_size,
        look.y,
        look.z,
        look.w,
        light.x,
        light.y,
        ink(light.z),
        ink(light.w),
        ink(shader.params[3].x),
        fade * setup.z,
    );
    let alpha = coloured.a;
    if (alpha <= 0.0) {
        discard;
    }
    if (globals.premultiplied_alpha != 0u) {
        return coloured;
    }
    return vec4<f32>(coloured.rgb / alpha, alpha);
}
