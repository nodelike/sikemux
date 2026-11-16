// One burst of sparks thrown out from the middle of its square: streaks that slow, sink a
// little and fade out, 0.95 s from the shader time given, drawn on the 1 pt grid.
//
// params: when it starts, device pixels per point, a number that varies where its sparks
// fly and how solid they are at their brightest; then its three inks, the first with how far
// the sparks reach in points.

const BURST_SECONDS: f32 = 0.95;
const SPARKS: f32 = 22.0;
const TURN: f32 = 6.2831853;

fn burst_random(seed: f32) -> f32 {
    return fract(sin(seed) * 43758.5453);
}

@fragment
fn fs_main(input: Varying) -> @location(0) vec4<f32> {
    let shader = instances[input.instance];
    let device = input.position.xy;
    if (any(device < shader.clip.xy) || any(device >= shader.clip.xy + shader.clip.zw)) {
        discard;
    }
    let setup = shader.params[0];
    let age = globals.time - setup.x;
    if (age < 0.0 || age > BURST_SECONDS) {
        discard;
    }
    let scale = setup.y;
    let size = shader.bounds.zw / scale;
    let local = (device - shader.bounds.xy) / scale;
    let from_middle = floor(local) + 0.5 - 0.5 * size;
    let span = shader.params[1].w;
    if (length(from_middle) > 1.15 * span) {
        discard;
    }

    let reach = span * (1.0 - exp(-6.0 * age));
    let slot = floor((atan2(from_middle.y, from_middle.x) + 0.5 * TURN) / TURN * SPARKS);
    var best = 0.0;
    var ink = 0;
    for (var k = -1; k <= 1; k++) {
        let spark = (slot + f32(k) + SPARKS) % SPARKS;
        let seed = spark + setup.z;
        let angle = (spark + 0.5 + 0.6 * (burst_random(seed * 12.9898) - 0.5)) / SPARKS * TURN
            - 0.5 * TURN;
        let speed = 0.7 + 0.3 * burst_random(seed * 78.233);
        let head = vec2<f32>(cos(angle), sin(angle)) * reach * speed + vec2<f32>(0.0, 6.0 * age * age);
        let tail = head * (0.55 + 0.25 * age);
        let along = head - tail;
        let share = clamp(dot(from_middle - tail, along) / max(dot(along, along), 1e-3), 0.0, 1.0);
        let streak = (1.0 - smoothstep(0.3, 1.1, length(from_middle - tail - along * share)))
            * (0.35 + 0.65 * share);
        let tip = 1.0 - smoothstep(0.6, 1.7, length(from_middle - head));
        let lit = max(streak, tip);
        if (lit > best) {
            best = lit;
            ink = i32(spark) % 3;
        }
    }
    let alpha = best * (1.0 - smoothstep(0.45, BURST_SECONDS, age)) * setup.w;
    if (alpha <= 0.0) {
        discard;
    }
    var colour = shader.params[1].rgb;
    if (ink == 1) {
        colour = shader.params[2].rgb;
    } else if (ink == 2) {
        colour = shader.params[3].rgb;
    }
    if (globals.premultiplied_alpha != 0u) {
        return vec4<f32>(colour * alpha, alpha);
    }
    return vec4<f32>(colour, alpha);
}
