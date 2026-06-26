// Тиковые кресты own-pass: резидентный instanced-слой в проходе GPUI.
// Кресты лежат в GPU StructuredBuffer как семантика (time_rel, price, side);
// пан/зум — смена cbuffer ChartView (юнформ), CPU массив не трогает.
// Форма 7×7 «Normal Trade X» через ROW_MASK + discard. Off-screen → вне NDC (hw clip).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — область чарта (привязка)
    float2 cv_resolution; // w, h бэкбуфера (px)
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half; // 3.5 для 7×7
    float  cv_pad;
    float  cv_volume_buy_inv;
    float  cv_volume_sell_inv;
    float  cv_volume_alpha;
    float  cv_pad2;
};

struct Cross {
    float time_rel;
    float price;
    uint  side; // 0 buy / 1 sell
    float qty;
};

StructuredBuffer<Cross> crosses : register(t1);

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
    Cross c = crosses[iid];
    CrossOut o;

    // семантика → экранные пиксели (привязка к левому/нижнему краю области чарта)
    float sx = cv_bounds.x + (c.time_rel - cv_view_time0) * cv_time_to_px;
    float sy = cv_bounds.y + cv_bounds.w - (c.price - cv_view_price0) * cv_price_to_px;
    sx = round(sx);
    sy = round(sy);

    // off-screen по X/Y → выкинуть из NDC, растеризатор отсечёт бесплатно
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
    // uv в [-1,1] → ячейка 0..6 матрицы 7×7
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
    // Канон-палитра приложения: --long (GREEN) / --short (ORANGE) — те же, что bid/ask стакана,
    // чтобы buy-трейд и bid-книга были одного зелёного. sRGB напрямую (таргет UNORM, см. grid.hlsl).
    float3 buy  = float3(0.18431, 0.65882, 0.36078); // #2FA85C palette GREEN
    float3 sell = float3(1.0,     0.55686, 0.35294); // #FF8E5A palette ORANGE
    float3 rgb = (i.side == 0u) ? buy : sell;
    return float4(rgb, 1.0);
}

struct VolumeOut {
    float4 pos : SV_Position;
    nointerpolation uint side : TEXCOORD0;
};

VolumeOut volume_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Cross c = crosses[iid];
    VolumeOut o;

    float sx = cv_bounds.x + (c.time_rel - cv_view_time0) * cv_time_to_px;
    if (sx < cv_bounds.x - 2.0 || sx > cv_bounds.x + cv_bounds.z + 2.0 || c.qty <= 0.0) {
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
    float3 buy  = float3(0.18431, 0.65882, 0.36078);
    float3 sell = float3(1.0,     0.55686, 0.35294);
    float3 rgb = (i.side == 0u) ? buy : sell;
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
    PricePoint p0 = price_points[iid];
    PricePoint p1 = price_points[iid + 1];
    float2 a = price_point_px(p0);
    float2 b = price_point_px(p1);
    float2 dir = b - a;
    float len = max(length(dir), 1e-4);
    dir /= len;
    float2 nrm = float2(-dir.y, dir.x) * 0.85;
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
    return float4(0.82, 0.60, 0.36, 0.82);
}

float4 price_mark_fragment(PriceLineOut i) : SV_Target {
    return float4(0.42, 0.72, 1.00, 0.78);
}
