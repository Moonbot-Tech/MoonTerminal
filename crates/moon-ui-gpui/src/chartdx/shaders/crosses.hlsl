// Tick crosses own pass: resident instanced layer in the GPUI pass.
// Crosses reside in a GPU StructuredBuffer as semantic data (time_rel, price, side).
// Pan/zoom updates the ChartView cbuffer (uniform) without touching the CPU array.
// The 7×7 "Normal Trade X" shape uses ROW_MASK + discard. Off-screen → outside NDC (hardware clip).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — chart area (anchor)
    float2 cv_resolution; // backbuffer w, h (px)
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half; // 3.5 for 7×7
    float  cv_instance_offset; // combo bake: first item in resident ring buffer
    float  cv_volume_buy_inv;
    float  cv_volume_sell_inv;
    float  cv_volume_alpha;
    float  cv_pad2;
};

struct Cross {
    float time_rel;
    float price;
    uint  side; // 0 buy / 1 sell / 2 liquidation
    float qty;
};

// Price-line appearance. b1 is free in this file: only ChartView occupies b0, and FXC strips a
// cbuffer no entry point references, so the crosses and volume PSOs do not acquire a b1
// requirement from this declaration.
cbuffer PriceStyle : register(b1) {
    float4 ps_last; // last-price line rgb + alpha
    float4 ps_mark; // mark-price line rgb + alpha
    float4 ps_m;    // x = line half-width in physical px
};

// b1 belongs to PriceStyle; ticks use the free b2 slot in both stages.
cbuffer TickStyle : register(b2) {
    float4 ts_buy;
    float4 ts_sell;
    float4 ts_liq;
    float4 ts_shape;  // x = kind (0 crosses, 1 bubbles); y = cross half as a share of cv_marker_half
    float4 ts_bubble; // x = smallest disc / largest; y, z = log10 notional of the smallest/largest
};

// Set on `side` for a trade drawn as a disc; the low byte keeps the side itself.
static const uint BUBBLE_FLAG = 0x100u;

// A disc's radius as a share of the largest (`cv_marker_half` in bubble mode), from the trade's
// quote notional on a log scale. Mirrors `moon_chart::bubbles::radius_frac`, which the text pass
// sizes its labels by.
float bubble_frac(Cross c) {
    float n = c.price * c.qty;
    float t = (n > 0.0) ? saturate((log10(n) - ts_bubble.y) / (ts_bubble.z - ts_bubble.y)) : 0.0;
    return ts_bubble.x + (1.0 - ts_bubble.x) * t;
}

StructuredBuffer<Cross> crosses : register(t1);

Cross combo_cross(uint iid) {
    return crosses[(uint)round(cv_instance_offset) + iid];
}

struct CrossOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
    nointerpolation uint side : TEXCOORD1;
};

static const float2 CORNERS[6] = {
    float2(-1, -1), float2(1, -1), float2(-1, 1),
    float2(-1,  1), float2(1, -1), float2( 1, 1)
};

CrossOut crosses_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Cross c = combo_cross(iid);
    CrossOut o;

    // Semantic data → screen pixels (anchored to the chart area's left/bottom edges).
    float sx = cv_bounds.x + (c.time_rel - cv_view_time0) * cv_time_to_px;
    float sy = cv_bounds.y + cv_bounds.w - (c.price - cv_view_price0) * cv_price_to_px;
    sx = round(sx);
    sy = round(sy);

    // Move off-screen X/Y outside NDC so the rasterizer clips it for free.
    float cull_margin = max(8.0, cv_marker_half + 1.0);
    if (sx < cv_bounds.x - cull_margin || sx > cv_bounds.x + cv_bounds.z + cull_margin ||
        sy < cv_bounds.y - cull_margin || sy > cv_bounds.y + cv_bounds.w + cull_margin) {
        o.pos = float4(2.0, 2.0, 0.0, 1.0);
        o.uv = float2(0.0, 0.0);
        o.side = 0u;
        return o;
    }

    // Liquidations keep their cross in bubble mode; `ts_shape.y` shrinks it back from the largest
    // disc's extent to the cross's own.
    bool bubble = ts_shape.x >= 0.5 && c.side < 2u;
    float half_px = cv_marker_half * (bubble ? bubble_frac(c) : ts_shape.y);
    float2 corner = CORNERS[vid];
    float2 px = float2(sx, sy) + corner * half_px;
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = corner;
    o.side = c.side | (bubble ? BUBBLE_FLAG : 0u);
    return o;
}

float4 crosses_fragment(CrossOut i) : SV_Target {
    uint side = i.side & 0xFFu;
    float3 rgb = (side == 0u) ? ts_buy.rgb : ((side == 1u) ? ts_sell.rgb : ts_liq.rgb);
    if ((i.side & BUBBLE_FLAG) != 0u) {
        float d2 = dot(i.uv, i.uv);
        if (d2 > 1.0) {
            discard;
        }
        // A darker rim keeps overlapping discs apart.
        return float4(d2 > 0.72 ? rgb * 0.6 : rgb, 1.0);
    }
    // Map UV in [-1,1] to cell 0..6 of the 7×7 matrix.
    int col = clamp((int)floor((i.uv.x * 0.5 + 0.5) * 7.0), 0, 6);
    int row = clamp((int)floor((i.uv.y * 0.5 + 0.5) * 7.0), 0, 6);
    // r0/r6 = c!=3 (0x77), r1/r5 = all (0x7F), r2..4 = c1..5 (0x3E)
    uint mask;
    if (row == 0 || row == 6) {
        mask = 0x77u;
    } else if (row == 1 || row == 5) {
        mask = 0x7Fu;
    } else {
        mask = 0x3Eu;
    }
    if (((mask >> (uint)col) & 1u) == 0u) {
        discard;
    }
    // Theme colours preserve the historical palette defaults. Direct sRGB (UNORM target).
    return float4(rgb, 1.0);
}

struct VolumeOut {
    float4 pos : SV_Position;
    nointerpolation uint side : TEXCOORD0;
};

VolumeOut volume_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Cross c = combo_cross(iid);
    VolumeOut o;

    float sx = cv_bounds.x + (c.time_rel - cv_view_time0) * cv_time_to_px;
    // side>=2 (liquidations) do not draw a volume bar, so cull them from this pass.
    if (sx < cv_bounds.x - 2.0 || sx > cv_bounds.x + cv_bounds.z + 2.0 || c.qty <= 0.0 || c.side >= 2u) {
        o.pos = float4(2.0, 2.0, 0.0, 1.0);
        o.side = 0u;
        return o;
    }

    float inv = (c.side == 0u) ? cv_volume_buy_inv : cv_volume_sell_inv;
    float norm = saturate(c.qty * inv);
    float band_h = min(cv_bounds.w * 0.18, 72.0);
    float h = max(1.0, sqrt(norm) * band_h);
    float base = cv_bounds.y + cv_bounds.w - 1.0;
    float bar_w = clamp(cv_time_to_px * 0.35, 1.0, 3.0);
    float2 corner = CORNERS[vid] * 0.5 + 0.5;
    float2 px = float2(round(sx) - bar_w * 0.5, base - h) + corner * float2(bar_w, h);
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);
    o.pos = float4(ndc, 0.0, 1.0);
    o.side = c.side;
    return o;
}

float4 volume_fragment(VolumeOut i) : SV_Target {
    float3 rgb = (i.side == 0u) ? ts_buy.rgb : ts_sell.rgb;
    return float4(rgb, saturate(cv_volume_alpha));
}

struct PricePoint {
    float time_rel;
    float price;
};

StructuredBuffer<PricePoint> price_points : register(t2);

struct PriceLineOut {
    float4 pos : SV_Position;
};

float2 price_point_px(PricePoint p) {
    float x = cv_bounds.x + (p.time_rel - cv_view_time0) * cv_time_to_px;
    float y = cv_bounds.y + cv_bounds.w - (p.price - cv_view_price0) * cv_price_to_px;
    return float2(x, y);
}

PriceLineOut price_line_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    // Buffer length from the view (`cv_pad2`): vs_4_1 cannot query a structured buffer's size.
    uint n = max((uint)round(cv_pad2), 1u);
    uint i0 = ((uint)round(cv_instance_offset) + iid) % n; // ring slot of the segment's start
    PricePoint p0 = price_points[i0];
    PricePoint p1 = price_points[(i0 + 1u) % n];
    float2 a = price_point_px(p0);
    float2 b = price_point_px(p1);
    float2 dir = b - a;
    float len = max(length(dir), 1e-4);
    dir /= len;
    float2 nrm = float2(-dir.y, dir.x) * max(ps_m.x, 0.25);
    float along[6] = { 0, 1, 1, 0, 1, 0 };
    float side[6]  = { -1, -1, 1, -1, 1, 1 };
    float2 px = lerp(a, b, along[vid]) + nrm * side[vid];
    PriceLineOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0,
                   1.0 - px.y / cv_resolution.y * 2.0,
                   0.0,
                   1.0);
    return o;
}

float4 price_last_fragment(PriceLineOut i) : SV_Target {
    return ps_last;
}

float4 price_mark_fragment(PriceLineOut i) : SV_Target {
    return ps_mark;
}
