#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

// The Mac's pane backdrop, after paper-design's dithering and image-dithering shaders with the Mac's
// `ambient` and `image` presets fixed in: a moving simplex-noise field, or a picture, through an 8×8 Bayer matrix.

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

/// The picture, covering the band and dithered in its own colours to four steps.
[[ stitchable ]] half4 backdropPicture(float2 position, half4 color, float2 size, float scale, float aspect, texture2d<half> picture) {
    constexpr sampler linear(filter::linear, address::clamp_to_edge);
    float2 resolution = size * scale;
    float2 spot = position * scale;
    float pixel = 2.0 * scale;
    float2 cells = (spot - 0.5 * resolution) / pixel;
    float2 normalized = (floor(cells) + 0.5) * pixel / resolution;
    float boxWidth = max(resolution.x / aspect, resolution.y) * aspect;
    float2 box = float2(boxWidth, boxWidth / aspect);
    float2 uv = normalized * (resolution / box) + 0.5;
    float4 image = float4(picture.sample(linear, uv));
    float lum = dot(float3(0.2126, 0.7152, 0.0722), image.rgb);
    const float steps = 4.0;
    float brightness = clamp(lum + (bayer(cells) - 0.5) / steps, 0.0, 1.0) * image.a;
    float level = floor(brightness * steps + 0.5) / steps;
    float3 tint = image.rgb / max(lum, 0.001) * level;
    float alpha = mix(level, 1.0, floor(image.a * steps + 0.5) / steps);
    return half4(half3(tint), half(alpha));
}
