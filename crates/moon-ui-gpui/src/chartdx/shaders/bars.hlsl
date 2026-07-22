// Order-book own pass: zone background + cumulative depth rectangles
// + separate level lines above the fill.
// Uses its own zone on the RIGHT (NOT a time series, without combo). Bars extend LEFT from
// the right edge; length = len_norm·zone. Normalization and geometry are computed on the CPU
// (book.build_instances). Port of moon-chart/shaders/glass.wgsl. The ChartView cbuffer is the
// same as for crosses (b0), but its viewport is the order-book zone.

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w (= order-book zone width), h (window px)
    float2 cv_resolution; // backbuffer w, h
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half;
    float  cv_pad;
};

// Order-book colors (sRGB rgb + pad). Theme-aware; currently close to crosses (green bid/red ask).
cbuffer BookStyle : register(b1) {
    float4 bs_book_bg; // SPREAD GAP background (between best bid/ask); whole zone when book is empty
    float4 bs_bid;     // bid rgb
    float4 bs_ask;     // ask rgb
    float4 bs_level;   // x = level-line opacity, y = level-line height in px
    float4 bs_bg_ask;  // ask-half background (above best ask)
    float4 bs_bg_bid;  // bid-half background (below best bid)
    float4 bs_edges;   // x = best ask price, y = best bid price, z = whether the book is populated
};

struct Level {
    float price;
    float span;     // signed price delta to the other edge of the fill strip
    float len_norm; // 0..1 fraction of the zone width
    float kind;     // 0 bid fill / 1 ask fill / 2 bid level / 3 ask level
};
StructuredBuffer<Level> levels : register(t1);

static const float2 CORNERS[6] = {
    float2(-1, -1), float2(1, -1), float2(-1, 1),
    float2(-1,  1), float2(1, -1), float2( 1, 1)
};

// Target = B8G8R8A8_UNORM (NOT sRGB): write sRGB values DIRECTLY, as GPUI/crosses do.
// There is NO conversion to linear; otherwise the colors are crushed into darkness (see grid.hlsl).

// ── Fill rectangles and separate level lines (instanced) ────────────────────
struct BarOut {
    float4 pos : SV_Position;
    nointerpolation float kind : TEXCOORD0;
};

BarOut bars_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Level lv = levels[iid];
    float zone = cv_bounds.z;
    float right = cv_bounds.x + zone;
    float seg_len = max(lv.len_norm * zone, 1.0);
    float cx = right - seg_len * 0.5;

    float base = cv_bounds.y + cv_bounds.w;
    float y_price = base - (lv.price - cv_view_price0) * cv_price_to_px;
    // Fill: one edge is the level's own price, the other is the neighbor's price (price+span).
    // A neighboring strip computes the shared seam from a DIFFERENT f32 expression (its price
    // versus our price+span). With large prices (micro-tokens) and zoom, round() can map these
    // slightly different f32 values to DIFFERENT pixels, leaving a 1 px black seam. Expand each
    // strip to WHOLE pixels (floor/ceil) so neighboring strips overlap at the join and eliminate
    // the seam. The fill is opaque, so the overlap is invisible. Ported from the same fix in
    // moon-chart/shaders/glass.wgsl.
    float inner = lv.price + lv.span;
    float y_inner = base - (inner - cv_view_price0) * cv_price_to_px;
    float top = floor(min(y_price, y_inner));
    float bot = ceil(max(y_price, y_inner));
    if (bot - top < 1.0) {
        bot = top + 1.0;
    }
    float cy = (top + bot) * 0.5;
    float hh = bot - top;
    if (lv.kind >= 2.0) {
        cy = round(y_price);
        hh = max(bs_level.y, 1.0);
    }

    float2 corner = CORNERS[vid];
    float2 px = float2(cx + corner.x * seg_len * 0.5, cy + corner.y * hh * 0.5);
    BarOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0, 1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    o.kind = lv.kind;
    return o;
}

float4 bars_fragment(BarOut i) : SV_Target {
    if (i.kind < 0.5) {
        return float4(bs_bid.rgb, 1.0);
    } else if (i.kind < 1.5) {
        return float4(bs_ask.rgb, 1.0);
    } else if (i.kind < 2.5) {
        return float4(min(bs_bid.rgb * 1.25, 1.0.xxx), bs_level.x);
    }
    return float4(min(bs_ask.rgb * 1.25, 1.0.xxx), bs_level.x);
}

// ── Order-book zone background (fullscreen quad over the zone) ──────────────
struct BgOut {
    float4 pos : SV_Position;
};

BgOut bg_vertex(uint vid : SV_VertexID) {
    float2 c = CORNERS[vid] * 0.5 + 0.5; // [0,1]
    float2 px = cv_bounds.xy + c * cv_bounds.zw;
    BgOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0, 1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    return o;
}

float4 bg_fragment(BgOut i) : SV_Target {
    // Three-color background: bg_ask above the best ask, bg_bid below the best bid, and book_bg
    // between them (the spread gap). Y uses the same transform as the bars.
    if (bs_edges.z > 0.5) {
        float base = cv_bounds.y + cv_bounds.w;
        float ask_y = base - (bs_edges.x - cv_view_price0) * cv_price_to_px;
        float bid_y = base - (bs_edges.y - cv_view_price0) * cv_price_to_px;
        if (i.pos.y < ask_y) {
            return float4(bs_bg_ask.rgb, 1.0);
        }
        if (i.pos.y > bid_y) {
            return float4(bs_bg_bid.rgb, 1.0);
        }
    }
    return float4(bs_book_bg.rgb, 1.0);
}
