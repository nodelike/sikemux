// Packed instanced circular particle sprites for a new chat's welcome screen,
// matching shared/signin/dots.js FILL with unsnapped subpixel coordinates.
//
// Normal packed instance (up to 4 nearby dots composited back-to-front):
//   params[i] = vec4(center_x_dev, center_y_dev, signed_r_alpha, rgb24)
//     signed_r_alpha: abs = alpha_q10 * 16384 + radius_q8_8; < 0 means bokeh (blur = 1.5 * r)
//     rgb24: r8 * 65536 + g8 * 256 + b8 (always >= 0)
//     empty slot has signed_r_alpha == 0
//
// Corner-clipped single-dot instance (only for the handful of dots straddling a rounded corner):
//   params[0] = vec4(center_x_dev, center_y_dev, signed_r_alpha, rgb24)
//   params[1] = vec4(pane_left_dev, pane_top_dev, pane_width_dev, pane_height_dev)
//   params[2] = vec4(corner_radius_dev, 0, 0, 0)
//   params[3] = vec4(0, 0, 0, -1)

fn wd_unpack_rgb(packed: f32) -> vec3<f32> {
    let r = floor(packed * (1.0 / 65536.0));
    let rem = packed - r * 65536.0;
    let g = floor(rem * (1.0 / 256.0));
    let b = rem - g * 256.0;
    return vec3<f32>(r, g, b) * (1.0 / 255.0);
}

fn wd_eval_dot(device: vec2<f32>, slot: vec4<f32>) -> vec4<f32> {
    let sra = slot.z;
    let packed_ra = abs(sra);
    let alpha_q = floor(packed_ra * (1.0 / 16384.0));
    let r_q = packed_ra - alpha_q * 16384.0;
    let radius = r_q * (1.0 / 256.0);
    let diff = device - slot.xy;
    let d2 = dot(diff, diff);
    let max_r = select(radius + 0.6, radius * 1.75 + 0.6, sra < 0.0);
    if (d2 >= max_r * max_r) {
        return vec4<f32>(0.0);
    }
    let past = sqrt(d2) - radius;
    var cov = 0.0;
    if (sra < 0.0) {
        let b = clamp(1.0 - smoothstep(-radius, radius * 0.75, past), 0.0, 1.0);
        cov = b * sqrt(b);
    } else {
        cov = 1.0 - smoothstep(-0.55, 0.55, past);
    }
    let a = clamp(alpha_q * (1.0 / 1023.0) * cov, 0.0, 1.0);
    if (a <= 0.002) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(wd_unpack_rgb(slot.w) * a, a);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let clip = instances[input.instance].clip;
    let device = input.position.xy;
    if (any(device < clip.xy) || any(device >= clip.xy + clip.zw)) {
        discard;
    }

    let p3 = instances[input.instance].params[3];
    let p0 = instances[input.instance].params[0];
    var acc = vec4<f32>(0.0);
    if (p3.w < 0.0) {
        let pane = instances[input.instance].params[1];
        let corner_radius = instances[input.instance].params[2].x;
        if (corner_radius > 0.0 && outside_corners(device - pane.xy, pane.zw, corner_radius)) {
            discard;
        }
        acc = wd_eval_dot(device, p0);
    } else {
        acc = wd_eval_dot(device, p0);
        let p1 = instances[input.instance].params[1];
        if (p1.z != 0.0) {
            let d1 = wd_eval_dot(device, p1);
            if (d1.a > 0.0) {
                acc = vec4<f32>(d1.rgb + acc.rgb * (1.0 - d1.a), d1.a + acc.a * (1.0 - d1.a));
            }
            let p2 = instances[input.instance].params[2];
            if (p2.z != 0.0) {
                let d2 = wd_eval_dot(device, p2);
                if (d2.a > 0.0) {
                    acc = vec4<f32>(d2.rgb + acc.rgb * (1.0 - d2.a), d2.a + acc.a * (1.0 - d2.a));
                }
                if (p3.z != 0.0) {
                    let d3 = wd_eval_dot(device, p3);
                    if (d3.a > 0.0) {
                        acc = vec4<f32>(d3.rgb + acc.rgb * (1.0 - d3.a), d3.a + acc.a * (1.0 - d3.a));
                    }
                }
            }
        }
    }

    if (acc.a <= 0.002) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return acc;
    }
    return vec4<f32>(clamp(acc.rgb / acc.a, vec3<f32>(0.0), vec3<f32>(1.0)), acc.a);
}
