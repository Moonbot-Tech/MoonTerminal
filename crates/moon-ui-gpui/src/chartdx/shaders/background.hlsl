// Статичная фото-подложка chart_area. Шейдер сам смешивает картинку с темовым bg,
// поэтому слой остаётся непрозрачной базой, а grid выше рисует только линии.

cbuffer BackgroundParams : register(b0) {
    float4 bp_dst;
    float2 bp_resolution;
    float2 bp_uv_off;
    float2 bp_uv_scale;
    float  bp_opacity;
    float  bp_pad;
    float4 bp_bg;
};

Texture2D bp_tex : register(t0);
SamplerState bp_samp : register(s0);

struct BgOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

BgOut background_vertex(uint vid : SV_VertexID) {
    float2 c = CORNERS[vid];
    float2 px = bp_dst.xy + c * bp_dst.zw;
    BgOut o;
    o.pos = float4(px.x / bp_resolution.x * 2.0 - 1.0,
                   1.0 - px.y / bp_resolution.y * 2.0,
                   0.0,
                   1.0);
    o.uv = bp_uv_off + c * bp_uv_scale;
    return o;
}

float4 background_fragment(BgOut i) : SV_Target {
    float3 photo = bp_tex.Sample(bp_samp, i.uv).rgb;
    float3 rgb = lerp(bp_bg.rgb, photo, saturate(bp_opacity));
    return float4(rgb, 1.0);
}
