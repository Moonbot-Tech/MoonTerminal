// Shared texture blit for the opaque base cache and transparent order-book/combo caches.
// The combo cache contains volume bars and crosses; price lines are rendered separately. Its
// offscreen width is W + max(20% of W, 128 px), leaving padding on the right for the live edge.
// Each frame blits the visible texture window into the chart area of the backbuffer; panning
// shifts UV (uv_off) without redrawing. Point sample, 1:1.

cbuffer BlitParams : register(b0) {
    float4 bp_dst;        // ox, oy, w, h — destination area in backbuffer px
    float2 bp_resolution; // w, h in backbuffer px
    float2 bp_uv_off;     // u_left, v_top — top-left corner of the visible texture window (0..1)
    float2 bp_uv_scale;   // u_span, v_span — window width/height in UV
    float2 bp_pad;
};

Texture2D bp_tex : register(t0);
SamplerState bp_samp : register(s0);

struct BlitOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

// Two triangles (TRIANGLELIST), with quad corners in [0,1].
static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

BlitOut blit_vertex(uint vid : SV_VertexID) {
    float2 c = CORNERS[vid];
    float2 px = bp_dst.xy + c * bp_dst.zw;
    float2 ndc = float2(px.x / bp_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / bp_resolution.y * 2.0);
    BlitOut o;
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = bp_uv_off + c * bp_uv_scale;
    return o;
}

float4 blit_fragment(BlitOut i) : SV_Target {
    return bp_tex.Sample(bp_samp, i.uv);
}

// OPAQUE variant for blitting the complete base (base.rs). The base is an opaque frame of the
// entire scene and must be blitted as a replacement (alpha=1, blending off). Otherwise, alpha<1
// blends in the backbuffer's white clear color (the Opaque-window fork clears to [1,1,1,1]),
// causing pale panel flashes on every UI present. Combo/orderbook do NOT use this fragment;
// they require transparency over the background.
float4 blit_opaque_fragment(BlitOut i) : SV_Target {
    return float4(bp_tex.Sample(bp_samp, i.uv).rgb, 1.0);
}
