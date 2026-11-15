#include <metal_stdlib>
#include <SwiftUI/SwiftUI_Metal.h>
using namespace metal;

// The Mac's poster mark (app/assets/shaders/poster_mark.wgsl): Sikemux's mark as an ordered dither in the accent, dense
// at the top and thinning down, its dots churning inside the mark's shape. The layer supplies that shape.

static float posterBayer(float2 cell) {
    uint2 at = uint2(fract(cell / 8.0) * 8.0);
    uint a = at.x ^ at.y;
    uint b = at.y;
    uint value = ((a & 1u) << 5u) | ((b & 1u) << 4u) | ((a & 2u) << 2u) | ((b & 2u) << 1u) | ((a & 4u) >> 1u)
        | ((b & 4u) >> 2u);
    return float(value) / 64.0;
}

static float posterWander(float2 at) {
    return 0.5 + 0.25 * sin(at.x * 0.021 + 2.0) * cos(at.y * 0.017 - 2.6) + 0.25 * sin((at.x + at.y) * 0.009 + 4.2);
}

static float posterChurn(float2 at, float phase) {
    return 0.5 + 0.25 * sin(at.x * 0.031 + phase) * cos(at.y * 0.027 - phase * 1.3)
        + 0.25 * sin((at.x + at.y) * 0.016 + phase * 2.1);
}

[[stitchable]] half4 posterMark(float2 position, SwiftUI::Layer layer, float2 size, float scale, float time, half4 colour) {
    float pitch = max(round(2.0 * scale), 2.0);
    float dot = max(round(pitch * (pitch <= 4.0 ? 0.5 : 2.0 / 3.0)), 1.0);
    float2 local = position * scale;
    float2 cell = floor(local / pitch);
    if (any(local - cell * pitch >= float2(dot))) { return half4(0); }
    float2 centre = (cell + 0.5) * pitch / scale;
    if (layer.sample(centre).a < 0.5) { return half4(0); }
    float2 share = centre / size;
    float2 point = local / scale;
    float boil = posterChurn(point * 0.75 + float2(16.0, -20.0) * time, time * 0.4) - 0.5;
    float value = 0.82 - 0.5 * share.y + 0.12 * (posterWander(share * 580.0) - 0.5) + 0.26 * boil;
    if (value + posterBayer(cell) < 1.0) { return half4(0); }
    return half4(colour.rgb * colour.a * 0.9, colour.a * 0.9);
}
