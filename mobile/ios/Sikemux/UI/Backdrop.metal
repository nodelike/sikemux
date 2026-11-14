#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
#include "BackdropKinds.generated.h"
using namespace metal;

// The Mac's pane backdrop kinds as SwiftUI shaders. `size` and `position` are in points.

static float2 fromMiddle(float2 position, float2 size) {
    return float2(position.x, size.y - position.y) - 0.5 * size;
}

/// The grain, `time` in seconds already run at its speed, in the colour `ink`.
[[ stitchable ]] half4 backdropGrain(float2 position, half4 color, float2 size, float time, half4 ink) {
    half lit = half(grain(fromMiddle(position, size), time));
    return half4(ink.rgb * ink.a * lit, ink.a * lit);
}

/// Keeps a place on the picture off the edge of its half of the atlas, so it never reads the other half.
static float2 inHalf(float2 at) {
    return clamp(at, 0.002, 0.998);
}

/// `atlas` holds the picture in its top half and how much each place glows in its bottom half.
[[ stitchable ]] half4 backdropPicture(
    float2 position, half4 color, float2 size, float aspect, float dotSize, float pattern, float contrast,
    float brightness, float glow, float ownColours, float full, float strength, half4 dark, half4 middle,
    half4 light, texture2d<half> atlas
) {
    constexpr sampler linear(filter::linear, address::clamp_to_edge);
    float2 point = fromMiddle(position, size);
    float2 uv = inHalf(picture_cover(picture_dot(point, dotSize), size, aspect));
    float3 texel = float3(atlas.sample(linear, float2(uv.x, uv.y * 0.5)).rgb);
    float2 glowAt = inHalf(picture_cover(point, size, aspect));
    float glowing = float(atlas.sample(linear, float2(glowAt.x, 0.5 + glowAt.y * 0.5)).r);
    float shown = (full > 0.5 ? 1.0 : picture_fade(position.y / size.y)) * strength;
    return half4(picture(
        point, texel, glowing, dotSize, pattern, contrast, brightness, glow, ownColours, float4(dark), float4(middle),
        float4(light), shown));
}
