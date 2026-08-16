// Order layer (UserData) own pass: horizontal lines (entry/stop/liq) + segments (ladder) +
// markers (start/end crosses and nodes). Coordinates are LOGICAL (time_rel/price) and mapped
// in the shader through cv_* using the same transform as crosses. Port of moon-chart
// order_lines/seg/marker.wgsl. GPU structs are 16-byte aligned (float4 fields), allowing
// StructuredBuffer to read them correctly.

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
// Target = B8G8R8A8_UNORM: write the order's sRGB color DIRECTLY (as GPUI/crosses do), without
// converting to linear; otherwise lines/markers are darker than the requested color (see grid.hlsl).

static const float2 CORNERS[6] = {
    float2(-1, -1), float2(1, -1), float2(1, 1),
    float2(-1, -1), float2(1, 1), float2(-1, 1)
};

// ── Zone (ZoneInstance: color, m=(price0,price1,t0_rel,t1_rel)) ─────────────
// A band is bounded in time by m.zw; the +-1e30 sentinel (an order zone) clamps to the plot's
// edges, which is exactly what the fixed bounds used to be. The right one is `cv_pad`, the edge of
// the order-book column rather than of the plot — a full-width band runs under the book, which
// blits over it later in the same pass.
struct Zone { float4 color; float4 m; };
StructuredBuffer<Zone> zones : register(t1);
struct ZOut { float4 pos : SV_Position; float4 color : COLOR0; };

ZOut zone_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Zone z = zones[iid];
    // Rounded like `hline_vertex`/`seg_vertex`: at fractional Y a band's edge sits up to a pixel
    // off the line that bounds it, and the offset walks as view_price0 drifts between bakes.
    float y0 = round(cv_bounds.y + cv_bounds.w - (z.m.x - cv_view_price0) * cv_price_to_px);
    float y1 = round(cv_bounds.y + cv_bounds.w - (z.m.y - cv_view_price0) * cv_price_to_px);
    // `edge` can pan LEFT of the plot, which would make min > max; order the bounds first.
    float lo = cv_bounds.x;
    float hi = max(cv_bounds.x + (cv_pad - cv_view_time0) * cv_time_to_px, lo);
    float left = clamp(cv_bounds.x + (z.m.z - cv_view_time0) * cv_time_to_px, lo, hi);
    float right = clamp(cv_bounds.x + (z.m.w - cv_view_time0) * cv_time_to_px, lo, hi);
    float2 corner = CORNERS[vid];
    float2 px = float2(lerp(left, right, (corner.x + 1.0) * 0.5), lerp(y0, y1, (corner.y + 1.0) * 0.5));
    ZOut o; o.pos = float4(to_clip(px), 0, 1); o.color = z.color; return o;
}
float4 zone_fragment(ZOut i) : SV_Target {
    return float4(i.color.rgb, i.color.a);
}

// ── Line patterns (TPenStyle order: 0 solid · 1 dash · 2 dot · 3 dash-dot · 4 dash-dot-dot) ──
// `d` is distance along the line in physical pixels: X for a full-width line, arc length for a
// segment, so one function serves both and the two cannot drift apart.
//
// The dash and the dot keep the exact runs the two hard-coded patterns had before there were five,
// so nothing already on screen changed when the missing three were added; those three are built
// from the same pieces at the same period.
bool pattern_on(float style, float d) {
    if (style < 0.5) return true;
    if (style < 1.5) return frac(d / 16.0) < (9.0 / 16.0);
    if (style < 2.5) return frac(d / 6.0) < (2.0 / 6.0);
    float x = frac(d / 20.0) * 20.0;
    if (style < 3.5) return x < 9.0 || (x >= 13.0 && x < 15.0);
    return x < 8.0 || (x >= 11.0 && x < 13.0) || (x >= 16.0 && x < 18.0);
}

// ── Horizontal line (LineInstance: color, m=(price,style,thickness,_)) ───────
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
    if (!pattern_on(i.style, i.xpx)) discard;
    return float4(i.color.rgb, i.color.a);
}

// ── Segment (SegInstance: pts=(t0,p0,t1,p1), color, m=(thickness,pattern,_,_)) ─
struct Seg { float4 pts; float4 color; float4 m; };
StructuredBuffer<Seg> segs : register(t1);
struct SOut { float4 pos : SV_Position; float4 color : COLOR0; nointerpolation float pattern : TEXCOORD0; float dist : TEXCOORD1; };

SOut seg_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    // extend: 0 = as given, 1 = to the right edge at the SAME price (an order line), 2 = ray.
    // A ray keeps its DIRECTION, so its far end is pushed along a->b past the plot and left to the
    // clip — solving against an edge would divide by a direction component that is zero for a
    // vertical ray. Two details matter: the direction is taken from the UNSNAPPED points, because a
    // one-pixel round over a long reach visibly tilts the line, and the far end is not snapped at
    // all for the same reason. Only the start keeps the snap that stops a thin line from flickering.
    Seg s = segs[iid];
    bool ray = s.m.z >= 1.5;
    float2 a_raw = data_to_px(s.pts.x, s.pts.y);
    float t1 = (s.m.z >= 0.5 && !ray) ? cv_pad : s.pts.z;
    float2 b_raw = data_to_px(t1, s.pts.w);
    // Snap endpoint Y coordinates to whole pixels. An order line (entry/stop/trailing) is a
    // constant-price segment; at fractional Y, a thin line flickers in thickness/brightness as
    // view_price0 drifts by subpixels between base rebakes and lands on a different row than
    // rounded crosses/hlines. Preserve X so the horizontal extent remains smooth.
    float2 a = float2(a_raw.x, round(a_raw.y));
    float2 b = float2(b_raw.x, round(b_raw.y));
    if (ray) {
        float2 d = b_raw - a_raw;
        float reach = length(cv_bounds.zw) + length(d) + 1.0;
        b = a + normalize(d + float2(1e-6, 0.0)) * reach;
    }
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
    if (!pattern_on(i.pattern, i.dist)) discard;
    return float4(i.color.rgb, i.color.a);
}

// ── Marker (MarkerInstance: color, pos=(t_rel,price,size,thickness), m=(shape,anchor,sector,sectors)) ─
// shape: 0 = cross, 1 = filled knot, 2 = news gem, 3 = warning badge, 4 = trade arrow UP (a buy),
// 5 = trade arrow DOWN (a sell). The registry lives on `MARKER_SHAPE_CROSS` in moon-chart's
// layers/order_lines.rs; note the warning badge is dispatched OPEN-ENDED as `> 2.5`, so any id above
// 3 must be tested for before it. anchor: 0 = price (Y through the chart transform),
// 1 = plot bottom (pos.y is physical px ABOVE the bottom edge, and the mark is clipped to the plot
// horizontally so it cannot spill into the axis gutter or the order book as it scrolls off).
// The gem reads pos.z as half HEIGHT and pos.w as half WIDTH, and paints only wedge m.z of m.w —
// one instance per tag colour, which is how a news mark carries several colours at once.
struct Marker { float4 color; float4 pos; float4 m; };
StructuredBuffer<Marker> markers : register(t1);
struct MOut { float4 pos : SV_Position; float4 color : COLOR0; float2 local : TEXCOORD0; nointerpolation float shape : TEXCOORD1; nointerpolation float thick : TEXCOORD2; nointerpolation float sz : TEXCOORD3; nointerpolation float2 xclip : TEXCOORD4; nointerpolation float2 wedge : TEXCOORD5; };

// Gap between wedges, as a fraction of one wedge, and the shading of the gem's left face. Together
// they are what makes a multi-colour mark read as a cut gem rather than as a flat blob.
static const float GEM_FACET_GAP = 0.055;
static const float GEM_LEFT_SHADE = 0.78;
static const float GEM_TWO_PI = 6.28318531;
// Trade-arrow rim: width as a fraction of the glyph's half-height, and how far its colour is
// darkened. Keep in sync with native_marker.wgsl / chart_native.metal.
static const float ARROW_RIM_FRAC = 0.22;
static const float ARROW_RIM_SHADE = 0.18;
// How much a clustered arrow grows per doubling of its member count, and the doublings after which
// it stops growing. Capped because the count is unbounded — a thousand-trade cluster must still be
// an arrow, not a billboard; the exact number belongs to the hover card, not to the glyph.
static const float ARROW_CLUSTER_GROW = 0.25;
static const float ARROW_CLUSTER_GROW_CAP = 3.0;

MOut marker_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Marker mk = markers[iid];
    float2 c = data_to_px(mk.pos.x, mk.pos.y);
    bool bottom = mk.m.y > 0.5;
    if (bottom) c.y = cv_bounds.y + cv_bounds.w - mk.pos.y;
    float half_sz = max(mk.pos.z, 1.0);
    float half_w = max(mk.pos.w, 1.0);
    if (mk.m.x > 3.5 && mk.m.x < 5.5) {
        // A CLUSTER of trades draws as one arrow, grown by a capped log of its member count (m.z),
        // so a busy pixel column reads as "several" rather than hiding all but one of them.
        float grow = 1.0 + ARROW_CLUSTER_GROW * min(log2(max(mk.m.z, 1.0)), ARROW_CLUSTER_GROW_CAP);
        half_sz *= grow;
        half_w *= grow;
        // A trade-history arrow hangs off its price: the APEX stays on the level and the body
        // extends away from it, clear of the candle. Derived from `shape` alone, so no instance
        // field pays for it, and applied in PIXELS so zoom leaves it untouched — a price-space
        // offset would have to be rebuilt on every view change, which is the one thing this own
        // pass exists to avoid. It uses the GROWN half height, or a cluster's apex would drift off
        // its own price as the cluster gets bigger.
        c.y += (mk.m.x < 4.5) ? half_sz : -half_sz;
    }
    float2 center = float2(round(c.x), round(c.y));
    // The gem is taller than wide; every other marker keeps its square quad.
    float2 half_ext = (mk.m.x >= 1.5) ? float2(half_w, half_sz) : float2(half_sz, half_sz);
    float2 corner = CORNERS[vid];
    float2 px = center + corner * half_ext;
    MOut o; o.pos = float4(to_clip(px), 0, 1); o.color = mk.color; o.local = corner * half_ext;
    o.shape = mk.m.x; o.thick = half_w; o.sz = half_sz;
    // Price-anchored markers keep their historical reach (order lines extend into the book zone).
    o.xclip = bottom ? float2(cv_bounds.x, cv_bounds.x + cv_bounds.z) : float2(-1e30, 1e30);
    o.wedge = float2(mk.m.z, max(mk.m.w, 1.0));
    return o;
}
float4 marker_fragment(MOut i) : SV_Target {
    if (i.pos.x < i.xclip.x || i.pos.x > i.xclip.y) discard;
    if (i.shape < 0.5) {
        float h = max(i.thick, 1.0) * 0.5;
        float d1 = abs(i.local.x - i.local.y) * 0.70710678;
        float d2 = abs(i.local.x + i.local.y) * 0.70710678;
        if (min(d1, d2) > h) discard;
        return float4(i.color.rgb, i.color.a);
    }
    if (i.shape < 1.5) {
        if (length(i.local) > i.sz) discard;
        return float4(i.color.rgb, i.color.a);
    }
    if (i.shape > 3.5 && i.shape < 5.5) {
        // Trade-history arrow: a solid triangle, apex UP for a buy and DOWN for a sell. Bounded on
        // BOTH sides deliberately — the warning badge below is an open-ended `> 2.5`, which is
        // exactly how these ids would otherwise have rendered as warning triangles.
        float dir = (i.shape < 4.5) ? 1.0 : -1.0;
        float nx = i.local.x / max(i.thick, 1.0);
        float ny = i.local.y / max(i.sz, 1.0) * dir;
        float inside = ny - (2.0 * abs(nx) - 1.0);
        if (inside < 0.0) discard;
        // A dark rim, because a trade arrow is drawn in the SAME palette as the candles: a green
        // buy landing on a green candle has no edge at all without one. Derived from the glyph's
        // own half-height so it holds its weight at any DPI instead of thinning out.
        float rim = max(1.0, i.sz * ARROW_RIM_FRAC) / max(i.sz, 1.0);
        if (inside < rim || (1.0 - ny) < rim) return float4(i.color.rgb * ARROW_RIM_SHADE, i.color.a);
        return float4(i.color.rgb, i.color.a);
    }
    if (i.shape > 2.5) {
        // Warning badge: an upward triangle (apex at top, base on the axis) with a dark
        // exclamation mark cut into it. local.y is +down, so the base sits at +sz.
        float tw = max(i.thick, 1.0);
        float nx = i.local.x / tw;
        float ny = i.local.y / max(i.sz, 1.0);
        if (ny < 2.0 * abs(nx) - 1.0) discard;
        bool bar = abs(nx) < 0.13 && ny > -0.30 && ny < 0.34;
        bool dot = abs(nx) < 0.15 && ny > 0.50 && ny < 0.74;
        if (bar || dot) return float4(i.color.rgb * 0.14, i.color.a);
        return float4(i.color.rgb, i.color.a);
    }
    // News gem: a vertically elongated diamond, optionally cut into wedges by tag colour.
    float hw = max(i.thick, 1.0);
    if (abs(i.local.x) / hw + abs(i.local.y) / max(i.sz, 1.0) > 1.0) discard;
    if (i.wedge.y > 1.5) {
        // Wedge index runs clockwise from the top tip, so a two-colour gem splits left/right.
        float ang = atan2(i.local.x, -i.local.y);
        if (ang < 0.0) ang += GEM_TWO_PI;
        float f = ang / GEM_TWO_PI * i.wedge.y;
        float idx = floor(f);
        if (abs(idx - i.wedge.x) > 0.5) discard;
        // Facet gaps only ABOVE the center: with an even wedge count one boundary lands exactly on
        // the bottom tip, and cutting there would lift the gem off the axis it marks.
        float frac = f - idx;
        if (i.local.y < 0.0 && (frac < GEM_FACET_GAP || frac > 1.0 - GEM_FACET_GAP)) discard;
    }
    float shade = (i.local.x < 0.0) ? GEM_LEFT_SHADE : 1.0;
    return float4(i.color.rgb * shade, i.color.a);
}
