// Horizontal volumes (Moonbot's `HVol`): bought and sold turnover by PRICE over a trailing window,
// drawn as rows in a zone of their own left of the plot, in the base pass. One instance is one
// row and carries two segments of six vertices each — the bought length, then the sold one over
// it — drawn exactly as the bottom band's sides layer (`side_volume.hlsl`) draws its columns, with
// the same colours and opacity, so the two indicators read alike:
//
//   overlaid (`hs_m.y` < 0.5, Moonbot's `Smooth graph`): both grow from the zone's plot-side
//   edge and the longer side shows past the other through the translucent fills.
//   stacked (`hs_m.y` >= 0.5, `Stacked graph`): the sold length continues from where the bought
//   one ends.
//
// Rows are placed against the PANE view's price mapping (b0), whose `bounds` is the plot: the zone
// shares the plot's top and height, and only the horizontal extent is the zone's own (`hs_zone`).
// Rows grow from the zone's RIGHT edge — the plot side — leftward, as the reference draws them,
// and their length is LINEAR in the value against the visible maximum. Laid over the plot
// (`hs_m.x` >= 0.5) the zone IS the plot's left strip: the rows grow from its LEFT edge rightward
// instead, anchored to the plot's edge, and the backdrop pass draws nothing.
//
// A separate pass draws the zone's backdrop and frame before the rows. Write sRGB colours
// directly (UNORM target, as in grid.hlsl).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — the PLOT
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

cbuffer HvolStyle : register(b1) {
    float4 hs_zone;    // zone x, y, w, h (px)
    float4 hs_buy;     // bought rgb + opacity
    float4 hs_sell;    // sold rgb + opacity
    float4 hs_bg;      // backdrop rgb + opacity
    float4 hs_border;  // border rgb + opacity
    float4 hs_m;       // x over the plot (1) / carved out (0), y stacked (1) / overlaid (0), z 1/max, w border px
};

struct HvolRow {
    float price_lo; // lower price edge
    float price_hi; // upper price edge
    float buy;      // bought inside the row, quote currency
    float sell;     // sold inside the row, quote currency
};

StructuredBuffer<HvolRow> rows : register(t3);

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

struct HvolOut {
    float4 pos : SV_Position;
    nointerpolation uint sell : TEXCOORD0;
};

HvolOut hvol_cull() {
    HvolOut o;
    o.pos = float4(2.0, 2.0, 0.0, 1.0);
    o.sell = 0u;
    return o;
}

// Row length in px, LINEAR in the value; see the header.
float hvol_len_px(float value) {
    return saturate(value * hs_m.z) * hs_zone.z;
}

HvolOut hvol_row_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    if (hs_zone.z < 1.0) {
        return hvol_cull(); // no zone
    }
    HvolRow r = rows[iid];
    uint sell = (vid >= 6u) ? 1u : 0u;
    float len = hvol_len_px(sell ? r.sell : r.buy);
    // Stacked: the sold length starts where the bought one ends.
    float start = (sell && hs_m.y >= 0.5) ? hvol_len_px(r.buy) : 0.0;
    if (len <= 0.0) {
        return hvol_cull();
    }
    // Whole pixels along the price axis, each edge ROUNDED rather than expanded outward: the
    // fills are translucent, and an overlap at the seam would blend twice and draw a light line
    // between every two rows. Neighbours share a seam exactly because one row's `price_hi` IS the
    // next row's `price_lo`, the same f32 from the same expression on the CPU.
    float base = cv_bounds.y + cv_bounds.w;
    float y_top = base - (r.price_hi - cv_view_price0) * cv_price_to_px;
    float y_bot = base - (r.price_lo - cv_view_price0) * cv_price_to_px;
    float top = round(min(y_top, y_bot));
    float bot = max(round(max(y_top, y_bot)), top + 1.0);
    // Cull rows outside the zone vertically before any further arithmetic.
    if (bot < hs_zone.y || top > hs_zone.y + hs_zone.w) {
        return hvol_cull();
    }
    // Carved out: from the zone's right edge, the plot side, leftward. Over the plot: from its
    // left edge rightward. At least one pixel of colour for a row that has anything in it, so a
    // thin market still shows where it traded.
    float x0;
    float x1;
    if (hs_m.x >= 0.5) {
        float left = hs_zone.x;
        x0 = round(left + start);
        x1 = max(round(x0 + len), x0 + 1.0);
    } else {
        float right = hs_zone.x + hs_zone.z;
        x1 = round(right - start);
        x0 = min(round(x1 - len), x1 - 1.0);
    }
    float2 corner = CORNERS[vid % 6u];
    float2 px = float2(x0, top) + corner * float2(x1 - x0, bot - top);
    px.x = clamp(px.x, hs_zone.x, hs_zone.x + hs_zone.z);
    HvolOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0,
                   1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    o.sell = sell;
    return o;
}

float4 hvol_row_fragment(HvolOut i) : SV_Target {
    return (i.sell == 1u) ? hs_sell : hs_buy;
}

// The zone's backdrop (instance 0) and its frame (instances 1..4: top, bottom, left, right), each
// one quad.
struct HvolBgOut {
    float4 pos : SV_Position;
    nointerpolation uint border : TEXCOORD0;
};

HvolBgOut hvol_bg_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    HvolBgOut o;
    // No zone, or a zone laid over the plot, which has no backdrop or frame of its own.
    if (hs_zone.z < 1.0 || hs_m.x >= 0.5) {
        o.pos = float4(2.0, 2.0, 0.0, 1.0);
        o.border = 0u;
        return o;
    }
    float th = max(hs_m.w, 1.0);
    float2 origin = hs_zone.xy;
    float2 size = hs_zone.zw;
    if (iid == 1u) {
        size = float2(hs_zone.z, th);
    } else if (iid == 2u) {
        origin = float2(hs_zone.x, hs_zone.y + hs_zone.w - th);
        size = float2(hs_zone.z, th);
    } else if (iid == 3u) {
        size = float2(th, hs_zone.w);
    } else if (iid == 4u) {
        origin = float2(hs_zone.x + hs_zone.z - th, hs_zone.y);
        size = float2(th, hs_zone.w);
    }
    float2 corner = CORNERS[vid % 6u];
    float2 px = origin + corner * size;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0,
                   1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    o.border = (iid == 0u) ? 0u : 1u;
    return o;
}

float4 hvol_bg_fragment(HvolBgOut i) : SV_Target {
    return (i.border == 1u) ? hs_border : hs_bg;
}
