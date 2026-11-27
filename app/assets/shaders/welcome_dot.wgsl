// Instanced circular particle sprite for a new chat's welcome screen, matching
// shared/signin/dots.js FILL with unsnapped subpixel coordinates.
//
// params[0]: center_x_dev, center_y_dev, radius_dev, blur_dev
// params[1]: r, g, b, alpha
// params[2]: pane_left_dev, pane_top_dev, pane_width_dev, pane_height_dev
// params[3]: corner_radius_dev, 0, 0, 0

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }

    let corner_radius = shader.params[3].x;
    if (corner_radius > 0.0) {
        let pane = shader.params[2];
        if (outside_corners(device - pane.xy, pane.zw, corner_radius)) {
            discard;
        }
    }

    let shape = shader.params[0];
    let past = length(device - shape.xy) - shape.z;
    let blur = shape.w;
    var cov = 0.0;
    if (blur > 0.0) {
        let b = clamp(1.0 - smoothstep(-shape.z, blur * 0.5, past), 0.0, 1.0);
        cov = pow(b, 1.6);
    } else {
        cov = 1.0 - smoothstep(-0.55, 0.55, past);
    }

    let color = shader.params[1];
    let a = clamp(color.a * cov, 0.0, 1.0);
    if (a <= 0.002) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(color.rgb * a, a);
    }
    return vec4<f32>(color.rgb, a);
}
