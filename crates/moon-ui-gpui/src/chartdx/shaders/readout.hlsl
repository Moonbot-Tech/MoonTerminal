struct ReadoutRect {
    float4 dst;
    float4 bg;
    float4 border;
    float4 m;
};

StructuredBuffer<ReadoutRect> rects : register(t1);

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

float4 to_clip(float2 px, float2 resolution) {
    return float4(px.x / resolution.x * 2.0 - 1.0,
                  1.0 - px.y / resolution.y * 2.0,
                  0.0,
                  1.0);
}

struct RectOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
    float4 dst : TEXCOORD1;
    float4 bg : COLOR0;
    float4 border : COLOR1;
    float border_width : TEXCOORD2;
};

RectOut readout_rect_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    ReadoutRect r = rects[iid];
    float2 c = CORNERS[vid];
    float2 px = r.dst.xy + c * r.dst.zw;
    RectOut o;
    o.pos = to_clip(px, r.m.yz);
    o.uv = c;
    o.dst = r.dst;
    o.bg = r.bg;
    o.border = r.border;
    o.border_width = max(r.m.x, 0.0);
    return o;
}

float4 readout_rect_fragment(RectOut i) : SV_Target {
    float2 px = i.uv * i.dst.zw;
    float edge = min(min(px.x, i.dst.z - px.x), min(px.y, i.dst.w - px.y));
    return edge <= i.border_width ? i.border : i.bg;
}
