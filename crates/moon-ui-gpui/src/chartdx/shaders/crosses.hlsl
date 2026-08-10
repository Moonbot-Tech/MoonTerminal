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
    float  cv_volume_height_frac;
    float4 cv_price_line;
    float4 cv_mark_price_line;
    float  cv_price_line_width;
    float  cv_volume_style;
    float2 cv_pad3;
};

struct Cross {
    float time_rel;
    float price;
    uint  side; // 0 buy / 1 sell / 2 liquidation
    float qty;
};

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

    float2 corner = CORNERS[vid];
    float2 px = float2(sx, sy) + corner * cv_marker_half;
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = corner;
    o.side = c.side;
    return o;
}

float4 crosses_fragment(CrossOut i) : SV_Target {
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
    // Canonical application palette: --long (GREEN) / --short (ORANGE), matching order-book
    // bid/ask so buy trades and book bids use the same green. Direct sRGB (UNORM target; see grid.hlsl).
    float3 buy  = float3(0.18431, 0.65882, 0.36078); // #2FA85C palette GREEN
    float3 sell = float3(1.0,     0.55686, 0.35294); // #FF8E5A palette ORANGE
    float3 liq  = float3(1.0,     1.0,     0.0);     // #FFFF00 liquidation (bright yellow)
    float3 rgb = (i.side == 0u) ? buy : ((i.side == 1u) ? sell : liq);
    return float4(rgb, 1.0);
}

struct VolumeOut {
    float4 pos : SV_Position;
    nointerpolation uint side : TEXCOORD0;
    float2 local : TEXCOORD1;
};

VolumeOut volume_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Cross c = combo_cross(iid);
    VolumeOut o;

    float sx = cv_bounds.x + (c.time_rel - cv_view_time0) * cv_time_to_px;
    // side>=2 (liquidations) do not draw a volume bar, so cull them from this pass.
    if (sx < cv_bounds.x - 2.0 || sx > cv_bounds.x + cv_bounds.z + 2.0 || c.qty <= 0.0 || c.side >= 2u) {
        o.pos = float4(2.0, 2.0, 0.0, 1.0);
        o.side = 0u;
        o.local = float2(0.0, 0.0);
        return o;
    }

    float inv = (c.side == 0u) ? cv_volume_buy_inv : cv_volume_sell_inv;
    float norm = saturate(c.qty * inv);
    float style = clamp(cv_volume_style, 0.0, 2.0);
    float band_h = cv_bounds.w * clamp(cv_volume_height_frac, 0.02, 0.45);
    float h = max(1.0, sqrt(norm) * band_h);
    float base = cv_bounds.y + cv_bounds.w - 1.0;
    float bar_w = (style < 0.5)
        ? clamp(cv_time_to_px * 0.35, 1.0, 3.0)
        : ((style < 1.5)
            ? clamp(cv_time_to_px * 1.05, 2.0, 10.0)
            : clamp(cv_time_to_px * 24.0, 24.0, 84.0));
    float2 corner = CORNERS[vid] * 0.5 + 0.5;
    float2 px = float2(round(sx) - bar_w * 0.5, base - h) + corner * float2(bar_w, h);
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);
    o.pos = float4(ndc, 0.0, 1.0);
    o.side = c.side;
    o.local = corner;
    return o;
}

float4 volume_fragment(VolumeOut i) : SV_Target {
    float3 buy  = float3(0.18431, 0.65882, 0.36078);
    float3 sell = float3(1.0,     0.55686, 0.35294);
    float3 rgb = (i.side == 0u) ? buy : sell;
    float alpha = saturate(cv_volume_alpha);
    if (cv_volume_style >= 1.5) {
        alpha = max(alpha, 0.74);
    }
    return float4(rgb, alpha);
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
    PricePoint p0 = price_points[iid];
    PricePoint p1 = price_points[iid + 1];
    float2 a = price_point_px(p0);
    float2 b = price_point_px(p1);
    float2 dir = b - a;
    float len = max(length(dir), 1e-4);
    dir /= len;
    float2 nrm = float2(-dir.y, dir.x) * max(cv_price_line_width, 0.5) * 0.5;
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
    return cv_price_line;
}

float4 price_mark_fragment(PriceLineOut i) : SV_Target {
    return cv_mark_price_line;
}
