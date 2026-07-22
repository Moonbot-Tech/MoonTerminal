// Candle own pass: instanced layer beneath trade crosses (base pass, before the combo blit).
// One instance is one candle: body (quad) + upper/lower wicks (thin quads), 18 vertices.
// Modes (filled / outlined / outlined in the trade zone), zone, wicks, and neutral color are
// CandleStyle constants, so changing the appearance does not touch the vertex buffer.
// The outline uses PS to discard the interior by per-pixel distance from the edge (the quad
// size comes from VS). Write sRGB colors directly (UNORM target, as in grid.hlsl).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — chart area
    float2 cv_resolution; // backbuffer w, h (px)
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half;
    float  cv_instance_offset;
    float  cv_volume_buy_inv;
    float  cv_volume_sell_inv;
    float  cv_volume_alpha;
    float  cv_pad2;
};

cbuffer CandleStyle : register(b1) {
    float4 cs_up;      // rising-candle color
    float4 cs_down;    // falling-candle color
    float4 cs_neutral; // neutral trade-zone color
    float  cs_tf_rel;          // bucket width (rel ms)
    float  cs_zone_start;      // trade-zone start in rel ms (f32::MAX = no zone)
    float  cs_mode;            // 0 filled / 1 outlined / 2 outlined in the zone
    float  cs_outline_px;      // outline width in physical px
    float  cs_wicks_in_zone;   // 0/1 — draw wicks in the zone
    float  cs_neutral_in_zone; // 0/1 — use the neutral color in the zone
    float  cs_fill_alpha;      // body fill opacity
    float  cs_hide_start;      // rel ms: omit candles with t_open ≥ boundary (trades only)
};

struct Candle {
    float t_open; // bucket opening time in rel ms
    float o;
    float h;
    float l;
    float c;
    float vol;
    float tf_rel; // Candle's OWN timeframe (rel ms); 0 = series timeframe. History tail uses higher timeframes.
};

StructuredBuffer<Candle> candles : register(t3);

struct CandleOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0; // 0..1 inside the quad
    nointerpolation float2 size_px : TEXCOORD1; // quad size in px (for the outline)
    nointerpolation float  outline : TEXCOORD2; // 1 = draw as an outline (body only)
    nointerpolation float4 color   : TEXCOORD3;
};

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

float price_y(float price) {
    return cv_bounds.y + cv_bounds.w - (price - cv_view_price0) * cv_price_to_px;
}

CandleOut cull_out() {
    CandleOut o;
    o.pos = float4(2.0, 2.0, 0.0, 1.0);
    o.uv = float2(0.0, 0.0);
    o.size_px = float2(1.0, 1.0);
    o.outline = 0.0;
    o.color = float4(0.0, 0.0, 0.0, 0.0);
    return o;
}

CandleOut candles_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Candle cd = candles[iid];
    uint part = vid / 6u;          // 0 body, 1 upper wick, 2 lower wick
    float2 corner = CORNERS[vid % 6u];

    if (cd.t_open >= cs_hide_start) {
        return cull_out(); // Omit the candle in the "trades only" zone.
    }
    // The history tail is completed with higher timeframes: those candles carry their own
    // tf_rel (width) and muted colors to distinguish them from the selected timeframe.
    bool foreign_tf = cd.tf_rel > 0.0 && abs(cd.tf_rel - cs_tf_rel) > 0.5;
    float tf_rel = (cd.tf_rel > 0.0) ? cd.tf_rel : cs_tf_rel;
    float x0 = cv_bounds.x + (cd.t_open - cv_view_time0) * cv_time_to_px;
    float x1 = x0 + tf_rel * cv_time_to_px;
    if (x1 < cv_bounds.x - 2.0 || x0 > cv_bounds.x + cv_bounds.z + 2.0) {
        return cull_out();
    }
    x0 = round(x0);
    x1 = max(round(x1), x0 + 1.0);

    bool in_zone = cd.t_open >= cs_zone_start;
    bool outline = (cs_mode >= 0.5 && cs_mode < 1.5) || (cs_mode >= 1.5 && in_zone);
    if (part != 0u && in_zone && cs_wicks_in_zone < 0.5) {
        return cull_out();
    }

    float y_top_body = min(price_y(cd.o), price_y(cd.c));
    float y_bot_body = max(price_y(cd.o), price_y(cd.c));
    y_top_body = round(y_top_body);
    y_bot_body = max(round(y_bot_body), y_top_body + 1.0);

    // Leave a gap between candle bodies (10% of the width, at most 4 px). A degenerate narrow
    // body becomes a column at least 1 px wide at the bucket center.
    float gap = clamp((x1 - x0) * 0.10, 0.0, 4.0);
    float bx0 = x0 + gap;
    float bx1 = x1 - gap;
    if (bx1 - bx0 < 1.0) {
        float cx = floor((x0 + x1) * 0.5);
        bx0 = cx;
        bx1 = cx + 1.0;
    }

    float2 p0; // top-left corner of the quad
    float2 sz;
    if (part == 0u) {
        p0 = float2(bx0, y_top_body);
        sz = float2(bx1 - bx0, y_bot_body - y_top_body);
    } else {
        float wick_w = max(1.0, min(cs_outline_px, (bx1 - bx0)));
        float wx = floor((x0 + x1) * 0.5 - wick_w * 0.5);
        float y0;
        float y1;
        if (part == 1u) {
            y0 = round(price_y(cd.h));
            y1 = y_top_body;
        } else {
            y0 = y_bot_body;
            y1 = round(price_y(cd.l));
        }
        if (y1 - y0 < 0.5) {
            return cull_out(); // No wick when high/low lies inside the body.
        }
        p0 = float2(wx, y0);
        sz = float2(wick_w, y1 - y0);
    }

    float2 px = p0 + corner * sz;
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);

    float4 base = (cd.c >= cd.o) ? cs_up : cs_down;
    if (in_zone && cs_neutral_in_zone > 0.5) {
        base = cs_neutral;
    }
    float alpha = 1.0;
    if (part == 0u && !outline) {
        alpha = saturate(cs_fill_alpha); // The body fill is translucent, leaving the grid faintly visible.
    }
    if (foreign_tf) {
        alpha *= 0.55; // The tail from another timeframe is translucent.
    }

    CandleOut o;
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = corner;
    o.size_px = sz;
    o.outline = (part == 0u && outline) ? 1.0 : 0.0;
    o.color = float4(base.rgb, alpha);
    return o;
}

float4 candles_fragment(CandleOut i) : SV_Target {
    if (i.outline > 0.5) {
        float dx = min(i.uv.x, 1.0 - i.uv.x) * i.size_px.x;
        float dy = min(i.uv.y, 1.0 - i.uv.y) * i.size_px.y;
        if (min(dx, dy) > max(cs_outline_px, 1.0)) {
            discard;
        }
        // Alpha comes from VS: normally 1.0 for outlines and muted for another timeframe's tail.
        return i.color;
    }
    return i.color;
}
