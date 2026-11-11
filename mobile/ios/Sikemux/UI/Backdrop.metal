#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

// The Mac's pane backdrop: paper-design's dithering shader with the Mac's
// `ambient` preset fixed in, a moving simplex-noise field through an 8×8 Bayer matrix, and the Mac's own picture shader.

constant int bayer8x8[64] = {
    0, 32, 8, 40, 2, 34, 10, 42,
    48, 16, 56, 24, 50, 18, 58, 26,
    12, 44, 4, 36, 14, 46, 6, 38,
    60, 28, 52, 20, 62, 30, 54, 22,
    3, 35, 11, 43, 1, 33, 9, 41,
    51, 19, 59, 27, 49, 17, 57, 25,
    15, 47, 7, 39, 13, 45, 5, 37,
    63, 31, 55, 23, 61, 29, 53, 21,
};

static float bayer(float2 uv) {
    int2 cell = int2(fract(uv / 8.0) * 8.0);
    return float(bayer8x8[cell.y * 8 + cell.x]) / 64.0;
}

static float3 mod289(float3 x) { return x - floor(x / 289.0) * 289.0; }
static float2 mod289(float2 x) { return x - floor(x / 289.0) * 289.0; }

static float snoise(float2 v) {
    const float4 C = float4(0.211324865405187, 0.366025403784439, -0.577350269189626, 0.024390243902439);
    float2 i = floor(v + dot(v, C.yy));
    float2 x0 = v - i + dot(i, C.xx);
    float2 i1 = (x0.x > x0.y) ? float2(1.0, 0.0) : float2(0.0, 1.0);
    float4 x12 = x0.xyxy + C.xxzz;
    x12.xy -= i1;
    i = mod289(i);
    float3 p = mod289(((i.y + float3(0.0, i1.y, 1.0)) * 34.0 + 1.0) * (i.y + float3(0.0, i1.y, 1.0)));
    p = p + i.x + float3(0.0, i1.x, 1.0);
    p = mod289((p * 34.0 + 1.0) * p);
    float3 m = max(0.5 - float3(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), 0.0);
    m = m * m;
    m = m * m;
    float3 x = 2.0 * fract(p * C.www) - 1.0;
    float3 h = abs(x) - 0.5;
    float3 ox = floor(x + 0.5);
    float3 a0 = x - ox;
    m *= 1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h);
    float3 g;
    g.x = a0.x * x0.x + h.x * x0.y;
    g.yz = a0.yz * x12.xz + h.yz * x12.yw;
    return 130.0 * dot(m, g);
}

/// The grain: `size` and `position` in points, `scale` pixels per point, `time` in seconds, `dot` the dots' colour.
[[ stitchable ]] half4 backdropGrain(float2 position, half4 color, float2 size, float scale, float time, half4 ink) {
    float2 resolution = size * scale;
    float2 spot = float2(position.x, size.y - position.y) * scale;
    float pixel = 3.0 * scale;
    float2 cells = (spot - 0.5 * resolution) / pixel;
    float2 pixelated = (floor(cells) + 0.5) * pixel;
    float2 uv = (pixelated / scale / 2.4 + 0.5) * 0.001;
    float t = 0.5 * time;
    float noise = 0.5 * snoise(uv - float2(0.0, 0.3 * t)) + 0.5 * snoise(2.0 * uv + float2(0.0, 0.32 * t));
    float shape = smoothstep(0.3, 0.9, 0.5 + 0.5 * noise);
    float lit = step(0.5, shape + bayer(cells) - 0.5);
    return half4(ink.rgb * ink.a * lit, ink.a * lit);
}

// The picture as the Mac's `picture` shader draws it: each dot takes one of three inks by its brightness, or the
// picture's own colour in four steps, through the host's pattern, with a glow over the light parts.

static float bayerBits8(float2 cell) {
    uint2 at = uint2(fract(cell / 8.0) * 8.0);
    uint a = at.x ^ at.y;
    uint b = at.y;
    uint value = ((a & 1u) << 5u) | ((b & 1u) << 4u) | ((a & 2u) << 2u) | ((b & 2u) << 1u) | ((a & 4u) >> 1u)
        | ((b & 4u) >> 2u);
    return float(value) / 64.0;
}

static float bayerBits4(float2 cell) {
    uint2 at = uint2(fract(cell / 4.0) * 4.0);
    uint a = at.x ^ at.y;
    uint b = at.y;
    return float(((a & 1u) << 3u) | ((b & 1u) << 2u) | (a & 2u) | ((b & 2u) >> 1u)) / 16.0;
}

static float bayerBits2(float2 cell) {
    uint2 at = uint2(fract(cell / 2.0) * 2.0);
    return float(((at.x ^ at.y) << 1u) | at.y) / 4.0;
}

static float threshold(float2 cell, float pattern) {
    if (pattern > 6.0) return bayerBits8(cell);
    if (pattern > 3.0) return bayerBits4(cell);
    if (pattern > 1.0) return bayerBits2(cell);
    return fract(sin(dot(floor(cell), float2(12.9898, 78.233))) * 43758.5453);
}

static float fadeDown(float y) {
    if (y < 0.4) return mix(1.0, 0.45, y / 0.4);
    if (y < 0.76) return mix(0.45, 0.08, (y - 0.4) / 0.36);
    return mix(0.08, 0.0, min((y - 0.76) / 0.24, 1.0));
}

/// Where `point`, in points from the middle with y up, falls on a picture of `aspect` covering `size`.
static float2 pictureAt(float2 point, float2 size, float aspect) {
    float2 covered = max(size.x / aspect, size.y) * float2(aspect, 1.0);
    float2 at = point / covered + 0.5;
    return clamp(float2(at.x, 1.0 - at.y), 0.002, 0.998);
}

/// `atlas` holds the picture in its top half and how much each place glows in its bottom half. Inks with no opacity
/// are none, and leave their dots clear.
[[ stitchable ]] half4 backdropPicture(
    float2 position, half4 color, float2 size, float aspect, float dotSize, float pattern, float contrast,
    float brightness, float glow, float ownColours, float full, float strength, half4 dark, half4 middle,
    half4 light, texture2d<half> atlas
) {
    constexpr sampler linear(filter::linear, address::clamp_to_edge);
    float2 point = float2(position.x, size.y - position.y) - 0.5 * size;
    float2 cell = point / dotSize;
    float2 uv = pictureAt((floor(cell) + 0.5) * dotSize, size, aspect);
    float3 texel = float3(atlas.sample(linear, float2(uv.x, uv.y * 0.5)).rgb);
    float raw = dot(float3(0.2126, 0.7152, 0.0722), texel);
    float lum = clamp((raw - 0.5) * (1.0 + contrast) + 0.5 + 0.5 * brightness, 0.0, 1.0);
    float edge = threshold(cell, pattern);

    float3 ink;
    float opacity;
    if (ownColours > 0.5) {
        const float steps = 4.0;
        float level = floor(clamp(lum + (edge - 0.5) / steps, 0.0, 1.0) * steps + 0.5) / steps;
        ink = texel / max(raw, 0.04) * level;
        opacity = level;
    } else {
        half4 inks[3] = { dark, middle, light };
        float along = lum * 2.0;
        float lower = min(floor(along), 1.0);
        int chosen = int(lower + step(edge, along - lower));
        ink = float3(inks[chosen].rgb);
        opacity = inks[chosen].a > 0.0h ? 1.0 : 0.0;
    }

    float3 glowTint = ownColours > 0.5 ? texel / max(raw, 0.04) : float3(middle.rgb);
    float2 glowAt = pictureAt(point, size, aspect);
    float glowing = float(atlas.sample(linear, float2(glowAt.x, 0.5 + glowAt.y * 0.5)).r) * glow;
    float fade = full > 0.5 ? 1.0 : fadeDown(position.y / size.y);
    float shown = fade * strength;
    float3 lit = ink * opacity + glowTint * glowing;
    float alpha = clamp(opacity + glowing, 0.0, 1.0) * shown;
    return half4(half3(min(lit * shown, float3(alpha))), half(alpha));
}
