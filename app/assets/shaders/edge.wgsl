// A pane whose agent waits on the person: warn dots on the 2 pt grid along all four inside
// edges, solid to half the band's reach and thinning out to none at its end, breathing from
// 55% to their peak once in 2.5 s until the shader time given, then holding at the peak.
//
// params: the time the breath ends, device pixels per point and the pane's corner radius in
// points; then the warn colour and how solid its dots are at their peak.

const PX_SIZE: f32 = 2.0;
const REACH: f32 = 12.0;
const BREATH_SECONDS: f32 = 2.5;
const BREATH_LOW: f32 = 0.55;

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let scale = setup.y;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    if (outside_corners(local, size, setup.z)) {
        discard;
    }
    let cell = floor(local / PX_SIZE);
    let at = (cell + 0.5) * PX_SIZE;
    let inside = min(min(at.x, at.y), min(size.x - at.x, size.y - at.y));
    let level = 1.0 - smoothstep(0.5 * REACH, REACH, inside);
    if (level <= bayer8(cell)) {
        discard;
    }

    var breath = 1.0;
    if (globals.time < setup.x) {
        let wave = 0.5 - 0.5 * cos(6.2831853 * globals.time / BREATH_SECONDS);
        breath = mix(BREATH_LOW, 1.0, wave);
    }
    let warn = shader.params[1];
    let alpha = warn.a * breath;
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(warn.rgb * alpha, alpha);
    }
    return vec4<f32>(warn.rgb, alpha);
}
