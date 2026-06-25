// Слой ордеров (UserData) own-pass: горизонтали (вход/стоп/liq) + отрезки (лестница) +
// маркеры (крест начала/конца, узелки). Координаты ЛОГИЧЕСКИЕ (time_rel/price), маппинг в
// шейдере по cv_* (тот же трансформ, что кресты). Порт moon-chart order_lines/seg/marker.wgsl.
// GPU-структы 16-байт-выровнены (float4-поля) — StructuredBuffer читает их корректно.

cbuffer ChartView : register(b0) {
    float4 cv_bounds;
    float2 cv_resolution;
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half;
    float  cv_pad;
};

float2 to_clip(float2 px) {
    return float2(px.x / cv_resolution.x * 2.0 - 1.0, 1.0 - px.y / cv_resolution.y * 2.0);
}
float2 data_to_px(float t_rel, float price) {
    float x = cv_bounds.x + (t_rel - cv_view_time0) * cv_time_to_px;
    float y = cv_bounds.y + cv_bounds.w - (price - cv_view_price0) * cv_price_to_px;
    return float2(x, y);
}
// Таргет = B8G8R8A8_UNORM: пишем sRGB-цвет ордера НАПРЯМУЮ (как GPUI/кресты), без
// конверсии в linear — иначе линии/маркеры темнее, чем заданный цвет (см. grid.hlsl).

static const float2 CORNERS[6] = {
    float2(-1, -1), float2(1, -1), float2(1, 1),
    float2(-1, -1), float2(1, 1), float2(-1, 1)
};

// ── Зона (ZoneInstance: color, m=(price0,price1,_,_)) ───────────────────────
struct Zone { float4 color; float4 m; };
StructuredBuffer<Zone> zones : register(t1);
struct ZOut { float4 pos : SV_Position; float4 color : COLOR0; };

ZOut zone_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Zone z = zones[iid];
    float y0 = cv_bounds.y + cv_bounds.w - (z.m.x - cv_view_price0) * cv_price_to_px;
    float y1 = cv_bounds.y + cv_bounds.w - (z.m.y - cv_view_price0) * cv_price_to_px;
    float left = cv_bounds.x;
    float right = cv_bounds.x + (cv_pad - cv_view_time0) * cv_time_to_px;
    float2 corner = CORNERS[vid];
    float2 px = float2(lerp(left, right, (corner.x + 1.0) * 0.5), lerp(y0, y1, (corner.y + 1.0) * 0.5));
    ZOut o; o.pos = float4(to_clip(px), 0, 1); o.color = z.color; return o;
}
float4 zone_fragment(ZOut i) : SV_Target {
    return float4(i.color.rgb, i.color.a);
}

// ── Горизонталь (LineInstance: color, m=(price,style,thickness,_)) ───────────
struct HLine { float4 color; float4 m; };
StructuredBuffer<HLine> hlines : register(t1);
struct HOut { float4 pos : SV_Position; float4 color : COLOR0; nointerpolation float style : TEXCOORD0; float xpx : TEXCOORD1; };

HOut hline_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    HLine h = hlines[iid];
    float price = h.m.x, style = h.m.y, thickness = h.m.z;
    float base = cv_bounds.y + cv_bounds.w;
    float cy = round(base - (price - cv_view_price0) * cv_price_to_px);
    float left = cv_bounds.x, right = cv_bounds.x + cv_bounds.z;
    float cx = (left + right) * 0.5, half_w = (right - left) * 0.5, half_h = max(thickness, 1.0) * 0.5;
    float2 corner = CORNERS[vid];
    float2 px = float2(cx + corner.x * half_w, cy + corner.y * half_h);
    HOut o; o.pos = float4(to_clip(px), 0, 1); o.color = h.color; o.style = style; o.xpx = px.x; return o;
}
float4 hline_fragment(HOut i) : SV_Target {
    if (i.style >= 0.5 && frac(i.xpx / 16.0) > (9.0 / 16.0)) discard;
    return float4(i.color.rgb, i.color.a);
}

// ── Отрезок (SegInstance: pts=(t0,p0,t1,p1), color, m=(thickness,pattern,_,_)) ─
struct Seg { float4 pts; float4 color; float4 m; };
StructuredBuffer<Seg> segs : register(t1);
struct SOut { float4 pos : SV_Position; float4 color : COLOR0; nointerpolation float pattern : TEXCOORD0; float dist : TEXCOORD1; };

SOut seg_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Seg s = segs[iid];
    float2 a = data_to_px(s.pts.x, s.pts.y);
    float t1 = s.m.z >= 0.5 ? cv_pad : s.pts.z;
    float2 b = data_to_px(t1, s.pts.w);
    float2 dir = b - a;
    float len = max(length(dir), 1e-4);
    dir /= len;
    float2 nrm = float2(-dir.y, dir.x) * max(s.m.x, 1.0) * 0.5;
    float along[6] = { 0, 1, 1, 0, 1, 0 };
    float side[6]  = { -1, -1, 1, -1, 1, 1 };
    float2 px = lerp(a, b, along[vid]) + nrm * side[vid];
    SOut o; o.pos = float4(to_clip(px), 0, 1); o.color = s.color; o.pattern = s.m.y; o.dist = len * along[vid]; return o;
}
float4 seg_fragment(SOut i) : SV_Target {
    if (i.pattern >= 1.5) {
        if (frac(i.dist / 6.0) > (2.0 / 6.0)) discard;
    } else if (i.pattern >= 0.5) {
        float x = frac(i.dist / 20.0) * 20.0;
        if (!(x < 8.0 || (x >= 11.0 && x < 13.0) || (x >= 16.0 && x < 18.0))) discard;
    }
    return float4(i.color.rgb, i.color.a);
}

// ── Маркер (MarkerInstance: color, pos=(t_rel,price,size,thickness), m=(shape,_,_,_)) ─
struct Marker { float4 color; float4 pos; float4 m; };
StructuredBuffer<Marker> markers : register(t1);
struct MOut { float4 pos : SV_Position; float4 color : COLOR0; float2 local : TEXCOORD0; nointerpolation float shape : TEXCOORD1; nointerpolation float thick : TEXCOORD2; nointerpolation float sz : TEXCOORD3; };

MOut marker_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Marker mk = markers[iid];
    float2 c = data_to_px(mk.pos.x, mk.pos.y);
    float2 center = float2(round(c.x), round(c.y));
    float half_sz = max(mk.pos.z, 1.0);
    float2 corner = CORNERS[vid];
    float2 px = center + corner * half_sz;
    MOut o; o.pos = float4(to_clip(px), 0, 1); o.color = mk.color; o.local = corner * half_sz;
    o.shape = mk.m.x; o.thick = mk.pos.w; o.sz = half_sz; return o;
}
float4 marker_fragment(MOut i) : SV_Target {
    if (i.shape < 0.5) {
        float h = max(i.thick, 1.0) * 0.5;
        float d1 = abs(i.local.x - i.local.y) * 0.70710678;
        float d2 = abs(i.local.x + i.local.y) * 0.70710678;
        if (min(d1, d2) > h) discard;
    } else {
        if (length(i.local) > i.sz) discard;
    }
    return float4(i.color.rgb, i.color.a);
}
