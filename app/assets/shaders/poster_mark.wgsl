// The Sikemux mark on the first-run setup's poster: an ordered dither in the accent, dense at
// the top and thinning down, keeping to the mark's shape. The picture supplies that shape in
// its opacity and nothing else.
//
// params: how far the dots have come in, from 0 to 1, the distance between dots and the size
// of a dot in device pixels; and the colour with its opacity.

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;

// The size the slow wander across the mark is measured against.
const WANDER_SPAN: f32 = 580.0;

fn wander(at: vec2<f32>) -> f32 {
    return 0.5 + 0.25 * sin(at.x * 0.021 + 2.0) * cos(at.y * 0.017 - 2.6)
        + 0.25 * sin((at.x + at.y) * 0.009 + 4.2);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let colour = shader.params[1];
    let local = device - shader.bounds.xy;
    let cell = floor(local / setup.y);
    if (any(local - cell * setup.y >= vec2<f32>(setup.z))) {
        discard;
    }
    let share = (cell + 0.5) * setup.y / shader.bounds.zw;
    let atlas = vec2<f32>(textureDimensions(image));
    let uv = (shader.image.xy + share * shader.image.zw) / atlas;
    if (textureSampleLevel(image, image_sampler, uv, 0.0).a < 0.5) {
        discard;
    }
    let value = (0.82 - 0.5 * share.y + 0.12 * (wander(share * WANDER_SPAN) - 0.5)) * setup.x;
    if (value + bayer8(cell) < 1.0) {
        discard;
    }
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour.rgb * colour.a, colour.a);
    }
    return colour;
}
