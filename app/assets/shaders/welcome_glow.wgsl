// Coarse-grid atmospheric glow cell behind a new chat's welcome particles,
// matching shared/signin/dots.js GLOW + GLOW_FILL without per-pixel trig/exp.
//
// params[0]: c00.r, c00.g, c00.b, opacity
// params[1]: c10.r, c10.g, c10.b, t_fract
// params[2]: c01.r, c01.g, c01.b, corner_radius_dev (0 on interior cells)
// params[3]: c11.r, c11.g, c11.b, corner_id (1=TL, 2=TR, 3=BL, 4=BR, 0=none)

fn wg_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q = q + vec2<f32>(dot(q, q + vec2<f32>(45.32)));
    return fract(q.x * q.y);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let clip = instances[input.instance].clip;
    let device = input.position.xy;
    if (any(device < clip.xy) || any(device >= clip.xy + clip.zw)) {
        discard;
    }

    let bounds = instances[input.instance].bounds;
    let p2 = instances[input.instance].params[2];
    let corner_radius = p2.w;
    if (corner_radius > 0.0) {
        let corner_id = instances[input.instance].params[3].w;
        var local = device - bounds.xy;
        if (corner_id == 2.0) {
            local = vec2<f32>(bounds.x + bounds.z - device.x, device.y - bounds.y);
        } else if (corner_id == 3.0) {
            local = vec2<f32>(device.x - bounds.x, bounds.y + bounds.w - device.y);
        } else if (corner_id == 4.0) {
            local = bounds.xy + bounds.zw - device;
        }
        if (local.x < corner_radius && local.y < corner_radius) {
            let d = vec2<f32>(corner_radius) - local;
            if (dot(d, d) > corner_radius * corner_radius) {
                discard;
            }
        }
    }

    let p0 = instances[input.instance].params[0];
    let p1 = instances[input.instance].params[1];
    let p3 = instances[input.instance].params[3];
    let uv = clamp((device - bounds.xy) / max(bounds.zw, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let top = mix(p0.rgb, p1.rgb, uv.x);
    let bot = mix(p2.rgb, p3.rgb, uv.x);
    let light = mix(top, bot, uv.y);

    let grain = (wg_hash(device + vec2<f32>(p1.w)) - 0.5)
        * 0.014
        * step(0.004, light.r + light.g + light.b);
    let lit = clamp(light + vec3<f32>(grain), vec3<f32>(0.0), vec3<f32>(1.0));
    let opacity = p0.w;
    let out_a = clamp(max(lit.r, max(lit.g, lit.b)) * opacity, 0.0, 1.0);
    if (out_a <= 0.001) {
        discard;
    }

    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(lit * opacity, out_a);
    }
    return vec4<f32>(clamp((lit * opacity) / out_a, vec3<f32>(0.0), vec3<f32>(1.0)), out_a);
}
