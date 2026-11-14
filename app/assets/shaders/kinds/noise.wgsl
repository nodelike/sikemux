// What the kinds share: Paper Shaders' simplex noise and the Bayer grids dots are dithered through.

fn permute(x: vec3<f32>) -> vec3<f32> {
    return (((x * 34.0) + 1.0) * x) % vec3<f32>(289.0);
}

fn modulo(x: vec2<f32>, y: f32) -> vec2<f32> {
    return x - y * floor(x / y);
}

fn snoise(v: vec2<f32>) -> f32 {
    let c = vec4<f32>(0.211324865405187, 0.366025403784439, -0.577350269189626, 0.024390243902439);
    var i = floor(v + dot(v, c.yy));
    let x0 = v - i + dot(i, c.xx);
    let i1 = select(vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), x0.x > x0.y);
    var x12 = x0.xyxy + c.xxzz;
    x12 = vec4<f32>(x12.xy - i1, x12.zw);
    i = modulo(i, 289.0);
    let p = permute(permute(i.y + vec3<f32>(0.0, i1.y, 1.0)) + i.x + vec3<f32>(0.0, i1.x, 1.0));
    var m = max(vec3<f32>(0.5) - vec3<f32>(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), vec3<f32>(0.0));
    m = m * m;
    m = m * m;
    let x = 2.0 * fract(p * c.www) - 1.0;
    let h = abs(x) - 0.5;
    let ox = floor(x + 0.5);
    let a0 = x - ox;
    m = m * (1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h));
    let g = vec3<f32>(a0.x * x0.x + h.x * x0.y, a0.yz * x12.xz + h.yz * x12.yw);
    return 130.0 * dot(m, g);
}

// The Bayer matrices' entries, built from the bits of the column and row.
fn bayer8(cell: vec2<f32>) -> f32 {
    let at = vec2<u32>(fract(cell / 8.0) * 8.0);
    let a = at.x ^ at.y;
    let b = at.y;
    let value = ((a & 1u) << 5u) | ((b & 1u) << 4u) | ((a & 2u) << 2u) | ((b & 2u) << 1u)
        | ((a & 4u) >> 1u) | ((b & 4u) >> 2u);
    return f32(value) / 64.0;
}

fn bayer4(cell: vec2<f32>) -> f32 {
    let at = vec2<u32>(fract(cell / 4.0) * 4.0);
    let a = at.x ^ at.y;
    let b = at.y;
    return f32(((a & 1u) << 3u) | ((b & 1u) << 2u) | (a & 2u) | ((b & 2u) >> 1u)) / 16.0;
}

fn bayer2(cell: vec2<f32>) -> f32 {
    let at = vec2<u32>(fract(cell / 2.0) * 2.0);
    return f32(((at.x ^ at.y) << 1u) | at.y) / 4.0;
}

// A dot's threshold: `pattern` 2, 4 or 8 for a Bayer grid that size, 0 for noise.
fn threshold(cell: vec2<f32>, pattern: f32) -> f32 {
    if (pattern > 6.0) {
        return bayer8(cell);
    }
    if (pattern > 3.0) {
        return bayer4(cell);
    }
    if (pattern > 1.0) {
        return bayer2(cell);
    }
    return fract(sin(dot(floor(cell), vec2<f32>(12.9898, 78.233))) * 43758.5453);
}
