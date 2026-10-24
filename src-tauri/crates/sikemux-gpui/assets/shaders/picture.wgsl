// A picture the person chose for their panes, dithered in its own colours on the 8x8
// Bayer grid as Paper Shaders' image dithering draws it, covering the band at the top
// of a pane and fading out down it. It holds still.
//
// params: device pixels per point, the pane's corner radius in points and how strongly
// the picture shows.

@group(2) @binding(0) var image: texture_2d<f32>;
@group(2) @binding(1) var image_sampler: sampler;

const PX_SIZE: f32 = 2.0;
const COLOR_STEPS: f32 = 4.0;

fn fade_down(y: f32) -> f32 {
    if (y < 0.4) {
        return mix(1.0, 0.45, y / 0.4);
    }
    if (y < 0.76) {
        return mix(0.45, 0.08, (y - 0.4) / 0.36);
    }
    return mix(0.08, 0.0, min((y - 0.76) / 0.24, 1.0));
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let scale = setup.x;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    // Only the band's top corners are the pane's; its bottom edge runs across the pane.
    let rounded = vec2<f32>(size.x, size.y + 2.0 * setup.y);
    if (outside_corners(local, rounded, setup.y)) {
        discard;
    }
    let point = vec2<f32>(local.x, size.y - local.y);

    let cell = (point - 0.5 * size) / PX_SIZE;
    let pixelized = (floor(cell) + 0.5) * PX_SIZE;
    let texels = shader.image.zw;
    let aspect = texels.x / texels.y;
    let covered = max(size.x / aspect, size.y) * vec2<f32>(aspect, 1.0);
    let at = pixelized / covered + 0.5;
    let picture_uv = vec2<f32>(at.x, 1.0 - at.y);
    let atlas = vec2<f32>(textureDimensions(image));
    let texel = textureSampleLevel(
        image,
        image_sampler,
        (shader.image.xy + picture_uv * texels) / atlas,
        0.0,
    );

    let lum = dot(vec3<f32>(0.2126, 0.7152, 0.0722), texel.rgb);
    let brightness = clamp(lum + (bayer8(cell) - 0.5) / COLOR_STEPS, 0.0, 1.0) * texel.a;
    let steps = floor(brightness * COLOR_STEPS + 0.5) / COLOR_STEPS;
    let quantized_alpha = floor(texel.a * COLOR_STEPS + 0.5) / COLOR_STEPS;
    let opacity = mix(steps, 1.0, quantized_alpha);
    let colour = texel.rgb / max(lum, 0.001) * steps / max(opacity, 0.001);

    let alpha = opacity * fade_down(local.y / size.y) * setup.z;
    if (alpha <= 0.0) {
        discard;
    }
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour * alpha, alpha);
    }
    return vec4<f32>(colour, alpha);
}
