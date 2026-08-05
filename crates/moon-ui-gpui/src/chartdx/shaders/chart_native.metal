#include <metal_stdlib>
using namespace metal;

struct ChartView {
    float4 bounds;
    float2 resolution;
    float time_to_px;
    float view_time0;
    float price_to_px;
    float view_price0;
    float marker_half;
    float pad;
    float volume_buy_inv;
    float volume_sell_inv;
    float volume_alpha;
    float _pad2;
};

struct BackgroundParams {
    float4 dst;
    float2 resolution;
    float2 uv_off;
    float2 uv_scale;
    float opacity;
    float _pad;
    float4 bg;
};

struct GridParams {
    float4 bounds;
    float2 resolution;
    float n_vert;
    float n_horiz;
    float pad0;
    float pad1;
    float grid_alpha;
    float bg_alpha;
    float4 bg;
    float4 grid_col;
};

struct CursorParams {
    float4 bounds;
    float2 resolution;
    float2 cursor;
    float4 color;
    float thickness;
    float enabled;
    float2 _pad;
};

struct ReadoutRect {
    float4 dst;
    float4 bg;
    float4 border;
    float4 m;
};

struct BookStyle {
    float4 book_bg;
    float4 bid;
    float4 ask;
    float4 level;
    float4 bg_ask;
    float4 bg_bid;
    // x = best ask price, y = best bid price, z = whether the book is populated (0/1).
    float4 edges;
};

struct Cross {
    float time_rel;
    float price;
    uint side;
    float qty;
};

struct PricePoint {
    float time_rel_ms;
    float price;
};

struct Level {
    float price;
    float span;
    float len_norm;
    float kind;
};

struct GpuLine { float4 color; float4 m; };
struct GpuZone { float4 color; float4 m; };
struct GpuSeg { float4 pts; float4 color; float4 m; };
struct GpuMarker { float4 color; float4 pos; float4 m; };

constant float2 CORNERS_01[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};
constant float2 CORNERS_PM[6] = {
    float2(-1, -1), float2(1, -1), float2(-1, 1),
    float2(-1, 1), float2(1, -1), float2(1, 1)
};
constant float2 CORNERS_ALT[6] = {
    float2(-1, -1), float2(1, -1), float2(1, 1),
    float2(-1, -1), float2(1, 1), float2(-1, 1)
};

static inline float4 to_clip(float2 px, float2 resolution) {
    return float4(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

static inline float2 data_to_px(constant ChartView& cv, float t_rel, float price) {
    float x = cv.bounds.x + (t_rel - cv.view_time0) * cv.time_to_px;
    float y = cv.bounds.y + cv.bounds.w - (price - cv.view_price0) * cv.price_to_px;
    return float2(x, y);
}

struct BgOut { float4 position [[position]]; float2 uv; };

vertex BgOut background_vertex(uint vid [[vertex_id]], constant BackgroundParams& bp [[buffer(0)]]) {
    float2 c = CORNERS_01[vid];
    float2 px = bp.dst.xy + c * bp.dst.zw;
    return { to_clip(px, bp.resolution), bp.uv_off + c * bp.uv_scale };
}

fragment float4 background_fragment(BgOut in [[stage_in]],
                                    constant BackgroundParams& bp [[buffer(0)]],
                                    texture2d<float> tex [[texture(0)]],
                                    sampler samp [[sampler(0)]]) {
    float3 photo = tex.sample(samp, in.uv).rgb;
    return float4(mix(bp.bg.rgb, photo, saturate(bp.opacity)), 1.0);
}

fragment float4 blit_fragment(BgOut in [[stage_in]],
                              texture2d<float> tex [[texture(0)]],
                              sampler samp [[sampler(0)]]) {
    return tex.sample(samp, in.uv);
}

struct GridOut { float4 position [[position]]; };

vertex GridOut grid_vertex(uint vid [[vertex_id]], constant GridParams& gp [[buffer(0)]]) {
    float2 c = CORNERS_01[vid];
    float2 px = gp.bounds.xy + c * gp.bounds.zw;
    return { to_clip(px, gp.resolution) };
}

fragment float4 grid_fragment(GridOut in [[stage_in]], constant GridParams& gp [[buffer(0)]]) {
    const float GRID_LINE_HALF_PX = 0.5;
    float3 bg = gp.bg.rgb;
    float3 grid_col = mix(bg, gp.grid_col.rgb, saturate(gp.grid_alpha));
    bool hit = false;
    // Fragment-stage position is rasterizer-generated, so both quad triangles use identical
    // pixel coordinates at their diagonal seam.
    float2 fragment_px = in.position.xy;
    float step_x = gp.bounds.z / max(gp.n_vert, 1.0);
    float local_x = fragment_px.x - gp.bounds.x;
    float line_x = gp.bounds.x + round(local_x / step_x) * step_x;
    float snapped_x = floor(line_x) + 0.5;
    if (abs(fragment_px.x - snapped_x) < GRID_LINE_HALF_PX) hit = true;
    float step_y = gp.bounds.w / max(gp.n_horiz, 1.0);
    float local_y = fragment_px.y - gp.bounds.y;
    float line_y = gp.bounds.y + round(local_y / step_y) * step_y;
    float snapped_y = floor(line_y) + 0.5;
    if (abs(fragment_px.y - snapped_y) < GRID_LINE_HALF_PX) hit = true;
    float alpha = hit ? 1.0 : saturate(gp.bg_alpha);
    return float4(hit ? grid_col : bg, alpha);
}

struct CursorOut { float4 position [[position]]; float4 color; };

vertex CursorOut cursor_vertex(uint vid [[vertex_id]], constant CursorParams& cp [[buffer(0)]]) {
    uint which = vid / 6u;
    uint corner_id = vid - which * 6u;
    float x01 = (corner_id == 1u || corner_id == 4u || corner_id == 5u) ? 1.0 : 0.0;
    float y01 = (corner_id == 2u || corner_id == 3u || corner_id == 5u) ? 1.0 : 0.0;
    float thickness = max(cp.thickness, 1.0);
    float half_t = thickness * 0.5;
    float right = cp.bounds.x + cp.bounds.z;
    float bottom = cp.bounds.y + cp.bounds.w;
    bool vertical_ok = cp.enabled > 0.5 && cp.cursor.x >= cp.bounds.x && cp.cursor.x <= right;
    bool horizontal_ok = cp.enabled > 0.5 && cp.cursor.y >= cp.bounds.y && cp.cursor.y <= bottom;
    float4 dst;
    if (which == 0u) {
        dst = float4(round(cp.cursor.x) - half_t, cp.bounds.y, thickness, cp.bounds.w);
        if (!vertical_ok) dst = float4(-10000.0, -10000.0, 1.0, 1.0);
    } else {
        dst = float4(cp.bounds.x, round(cp.cursor.y) - half_t, cp.bounds.z, thickness);
        if (!horizontal_ok) dst = float4(-10000.0, -10000.0, 1.0, 1.0);
    }
    float2 px = dst.xy + float2(x01, y01) * dst.zw;
    return { to_clip(px, cp.resolution), cp.color };
}

fragment float4 cursor_fragment(CursorOut in [[stage_in]]) {
    return in.color;
}

struct ReadoutRectOut {
    float4 position [[position]];
    float2 uv;
    float4 dst;
    float4 bg;
    float4 border;
    float border_width;
};

vertex ReadoutRectOut readout_rect_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                                          const device ReadoutRect* rects [[buffer(1)]]) {
    ReadoutRect r = rects[iid];
    float2 c = CORNERS_01[vid];
    float2 px = r.dst.xy + c * r.dst.zw;
    return { to_clip(px, r.m.yz), c, r.dst, r.bg, r.border, max(r.m.x, 0.0) };
}

fragment float4 readout_rect_fragment(ReadoutRectOut in [[stage_in]]) {
    float2 px = in.uv * in.dst.zw;
    float edge = min(min(px.x, in.dst.z - px.x), min(px.y, in.dst.w - px.y));
    return edge <= in.border_width ? in.border : in.bg;
}

// ── Candles (mirrors candles.hlsl): body + upper/lower wicks, 18 vertices per instance ──

struct CandleStyle {
    float4 up;
    float4 down;
    float4 neutral;
    float tf_rel;
    float zone_start;
    float mode;
    float outline_px;
    float wicks_in_zone;
    float neutral_in_zone;
    float fill_alpha;
    float hide_start; // rel ms: omit candles with t_open >= boundary (trades only)
};

struct Candle {
    float t_open;
    float o;
    float h;
    float l;
    float c;
    float vol;
    // Candle's OWN timeframe (rel ms); 0 = series timeframe. History tail uses wider, muted higher timeframes.
    float tf_rel;
};

struct CandleOut {
    float4 position [[position]];
    float2 uv;
    float2 size_px [[flat]];
    float outline [[flat]];
    float4 color [[flat]];
};

static inline float candle_price_y(constant ChartView& cv, float price) {
    return cv.bounds.y + cv.bounds.w - (price - cv.view_price0) * cv.price_to_px;
}

static inline CandleOut candle_cull_out() {
    return { float4(2.0, 2.0, 0.0, 1.0), float2(0.0), float2(1.0), 0.0, float4(0.0) };
}

vertex CandleOut candles_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                                constant ChartView& cv [[buffer(0)]],
                                constant CandleStyle& cs [[buffer(1)]],
                                const device Candle* candles [[buffer(2)]]) {
    Candle cd = candles[iid];
    uint part = vid / 6u; // 0 body, 1 upper wick, 2 lower wick
    float2 corner = CORNERS_01[vid % 6u];

    if (cd.t_open >= cs.hide_start) {
        return candle_cull_out(); // Omit the candle in the "trades only" zone.
    }
    bool foreign_tf = cd.tf_rel > 0.0 && fabs(cd.tf_rel - cs.tf_rel) > 0.5;
    float tf_rel = (cd.tf_rel > 0.0) ? cd.tf_rel : cs.tf_rel;
    float x0 = cv.bounds.x + (cd.t_open - cv.view_time0) * cv.time_to_px;
    float x1 = x0 + tf_rel * cv.time_to_px;
    if (x1 < cv.bounds.x - 2.0 || x0 > cv.bounds.x + cv.bounds.z + 2.0) {
        return candle_cull_out();
    }
    x0 = round(x0);
    x1 = max(round(x1), x0 + 1.0);

    bool in_zone = cd.t_open >= cs.zone_start;
    bool outline = (cs.mode >= 0.5 && cs.mode < 1.5) || (cs.mode >= 1.5 && in_zone);
    if (part != 0u && in_zone && cs.wicks_in_zone < 0.5) {
        return candle_cull_out();
    }

    float y_top_body = round(min(candle_price_y(cv, cd.o), candle_price_y(cv, cd.c)));
    float y_bot_body = max(round(max(candle_price_y(cv, cd.o), candle_price_y(cv, cd.c))),
                           y_top_body + 1.0);

    float gap = clamp((x1 - x0) * 0.10, 0.0, 4.0);
    float bx0 = x0 + gap;
    float bx1 = x1 - gap;
    if (bx1 - bx0 < 1.0) {
        float cx = floor((x0 + x1) * 0.5);
        bx0 = cx;
        bx1 = cx + 1.0;
    }

    float2 p0;
    float2 sz;
    if (part == 0u) {
        p0 = float2(bx0, y_top_body);
        sz = float2(bx1 - bx0, y_bot_body - y_top_body);
    } else {
        float wick_w = max(1.0, min(cs.outline_px, bx1 - bx0));
        float wx = floor((x0 + x1) * 0.5 - wick_w * 0.5);
        float y0;
        float y1;
        if (part == 1u) {
            y0 = round(candle_price_y(cv, cd.h));
            y1 = y_top_body;
        } else {
            y0 = y_bot_body;
            y1 = round(candle_price_y(cv, cd.l));
        }
        if (y1 - y0 < 0.5) {
            return candle_cull_out();
        }
        p0 = float2(wx, y0);
        sz = float2(wick_w, y1 - y0);
    }

    float2 px = p0 + corner * sz;
    float4 base = (cd.c >= cd.o) ? cs.up : cs.down;
    if (in_zone && cs.neutral_in_zone > 0.5) {
        base = cs.neutral;
    }
    float alpha = (part == 0u && !outline) ? saturate(cs.fill_alpha) : 1.0;
    if (foreign_tf) {
        alpha *= 0.55; // The tail from another timeframe is translucent.
    }

    CandleOut out;
    out.position = to_clip(px, cv.resolution);
    out.uv = corner;
    out.size_px = sz;
    out.outline = (part == 0u && outline) ? 1.0 : 0.0;
    out.color = float4(base.rgb, alpha);
    return out;
}

fragment float4 candles_fragment(CandleOut in [[stage_in]],
                                 constant CandleStyle& cs [[buffer(1)]]) {
    if (in.outline > 0.5) {
        float dx = min(in.uv.x, 1.0 - in.uv.x) * in.size_px.x;
        float dy = min(in.uv.y, 1.0 - in.uv.y) * in.size_px.y;
        if (min(dx, dy) > max(cs.outline_px, 1.0)) {
            discard_fragment();
        }
        // Alpha comes from VS: normally 1.0 for outlines and muted for another timeframe's tail.
        return in.color;
    }
    return in.color;
}

struct CrossOut { float4 position [[position]]; float2 uv; uint side [[flat]]; };

vertex CrossOut crosses_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                               constant ChartView& cv [[buffer(0)]],
                               const device Cross* crosses [[buffer(1)]]) {
    Cross c = crosses[iid];
    float sx = round(cv.bounds.x + (c.time_rel - cv.view_time0) * cv.time_to_px);
    float sy = round(cv.bounds.y + cv.bounds.w - (c.price - cv.view_price0) * cv.price_to_px);
    float cull_margin = max(8.0, cv.marker_half + 1.0);
    if (sx < cv.bounds.x - cull_margin || sx > cv.bounds.x + cv.bounds.z + cull_margin ||
        sy < cv.bounds.y - cull_margin || sy > cv.bounds.y + cv.bounds.w + cull_margin) {
        return { float4(2.0, 2.0, 0.0, 1.0), float2(0.0), 0 };
    }
    float2 corner = CORNERS_PM[vid];
    float2 px = float2(sx, sy) + corner * cv.marker_half;
    return { to_clip(px, cv.resolution), corner, c.side };
}

fragment float4 crosses_fragment(CrossOut in [[stage_in]]) {
    int col = clamp((int)floor((in.uv.x * 0.5 + 0.5) * 7.0), 0, 6);
    int row = clamp((int)floor((in.uv.y * 0.5 + 0.5) * 7.0), 0, 6);
    uint mask = (row == 0 || row == 6) ? 0x77u : ((row == 1 || row == 5) ? 0x7Fu : 0x3Eu);
    if (((mask >> (uint)col) & 1u) == 0u) discard_fragment();
    float3 buy = float3(0.18431, 0.65882, 0.36078);
    float3 sell = float3(1.0, 0.55686, 0.35294);
    return float4(in.side == 0 ? buy : sell, 1.0);
}

struct VolumeOut { float4 position [[position]]; uint side [[flat]]; };

vertex VolumeOut volume_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                               constant ChartView& cv [[buffer(0)]],
                               const device Cross* crosses [[buffer(1)]]) {
    Cross c = crosses[iid];
    float sx = cv.bounds.x + (c.time_rel - cv.view_time0) * cv.time_to_px;
    if (sx < cv.bounds.x - 2.0 || sx > cv.bounds.x + cv.bounds.z + 2.0 || c.qty <= 0.0) {
        return { float4(2.0, 2.0, 0.0, 1.0), 0 };
    }
    float inv = c.side == 0 ? cv.volume_buy_inv : cv.volume_sell_inv;
    float h = max(1.0, sqrt(saturate(c.qty * inv)) * min(cv.bounds.w * 0.18, 72.0));
    float base = cv.bounds.y + cv.bounds.w - 1.0;
    float bar_w = clamp(cv.time_to_px * 0.35, 1.0, 3.0);
    float2 px = float2(round(sx) - bar_w * 0.5, base - h) + CORNERS_01[vid] * float2(bar_w, h);
    return { to_clip(px, cv.resolution), c.side };
}

fragment float4 volume_fragment(VolumeOut in [[stage_in]], constant ChartView& cv [[buffer(0)]]) {
    float3 buy = float3(0.18431, 0.65882, 0.36078);
    float3 sell = float3(1.0, 0.55686, 0.35294);
    return float4(in.side == 0 ? buy : sell, saturate(cv.volume_alpha));
}

struct PriceOut { float4 position [[position]]; };

static inline float2 price_point_px(constant ChartView& cv, PricePoint p) {
    return float2(cv.bounds.x + (p.time_rel_ms - cv.view_time0) * cv.time_to_px,
                  cv.bounds.y + cv.bounds.w - (p.price - cv.view_price0) * cv.price_to_px);
}

vertex PriceOut price_line_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                                  constant ChartView& cv [[buffer(0)]],
                                  const device PricePoint* points [[buffer(1)]]) {
    float2 a = price_point_px(cv, points[iid]);
    float2 b = price_point_px(cv, points[iid + 1]);
    float2 dir = b - a;
    float len = max(length(dir), 1e-4);
    dir /= len;
    float2 nrm = float2(-dir.y, dir.x) * 0.85;
    float along = (vid == 1 || vid == 2 || vid == 4) ? 1.0 : 0.0;
    float side = (vid == 2 || vid == 4 || vid == 5) ? 1.0 : -1.0;
    float2 px = mix(a, b, along) + nrm * side;
    return { to_clip(px, cv.resolution) };
}

fragment float4 price_last_fragment() { return float4(0.82, 0.60, 0.36, 0.82); }
fragment float4 price_mark_fragment() { return float4(0.42, 0.72, 1.00, 0.78); }

struct BookOut { float4 position [[position]]; float kind [[flat]]; };

vertex BookOut book_bars_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                                constant ChartView& cv [[buffer(0)]],
                                const device BookStyle& bs [[buffer(1)]],
                                const device Level* levels [[buffer(2)]]) {
    (void)bs;
    Level lv = levels[iid];
    float zone = cv.bounds.z;
    float right = cv.bounds.x + zone;
    float seg_len = max(lv.len_norm * zone, 1.0);
    float cx = right - seg_len * 0.5;
    float base = cv.bounds.y + cv.bounds.w;
    float y_price = base - (lv.price - cv.view_price0) * cv.price_to_px;
    float y_inner = base - (lv.price + lv.span - cv.view_price0) * cv.price_to_px;
    float top = round(min(y_price, y_inner));
    float bot = round(max(y_price, y_inner));
    if (bot - top < 1.0) bot = top + 1.0;
    float cy = (top + bot) * 0.5;
    float hh = bot - top;
    if (lv.kind >= 2.0) { cy = round(y_price); hh = max(bs.level.y, 1.0); }
    float2 px = float2(cx + CORNERS_PM[vid].x * seg_len * 0.5, cy + CORNERS_PM[vid].y * hh * 0.5);
    return { to_clip(px, cv.resolution), lv.kind };
}

fragment float4 book_bars_fragment(BookOut in [[stage_in]], constant BookStyle& bs [[buffer(1)]]) {
    if (in.kind < 0.5) return float4(bs.bid.rgb, 1.0);
    if (in.kind < 1.5) return float4(bs.ask.rgb, 1.0);
    if (in.kind < 2.5) return float4(min(bs.bid.rgb * 1.25, float3(1.0)), bs.level.x);
    return float4(min(bs.ask.rgb * 1.25, float3(1.0)), bs.level.x);
}

vertex PriceOut book_bg_vertex(uint vid [[vertex_id]], constant ChartView& cv [[buffer(0)]]) {
    float2 px = cv.bounds.xy + CORNERS_01[vid] * cv.bounds.zw;
    return { to_clip(px, cv.resolution) };
}

fragment float4 book_bg_fragment(PriceOut in [[stage_in]],
                                 constant ChartView& cv [[buffer(0)]],
                                 constant BookStyle& bs [[buffer(1)]]) {
    // Three-color background: above best ask / spread gap / below best bid.
    if (bs.edges.z > 0.5) {
        float base = cv.bounds.y + cv.bounds.w;
        float ask_y = base - (bs.edges.x - cv.view_price0) * cv.price_to_px;
        float bid_y = base - (bs.edges.y - cv.view_price0) * cv.price_to_px;
        if (in.position.y < ask_y) return float4(bs.bg_ask.rgb, 1.0);
        if (in.position.y > bid_y) return float4(bs.bg_bid.rgb, 1.0);
    }
    return float4(bs.book_bg.rgb, 1.0);
}

struct ZOut { float4 position [[position]]; float4 color; };

vertex ZOut zone_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                        constant ChartView& cv [[buffer(0)]],
                        const device GpuZone* zones [[buffer(1)]]) {
    GpuZone z = zones[iid];
    // Rounded like the line shaders: at fractional Y a band's edge sits up to a pixel off the
    // line that bounds it, and the offset walks as view_price0 drifts between bakes.
    float y0 = round(cv.bounds.y + cv.bounds.w - (z.m.x - cv.view_price0) * cv.price_to_px);
    float y1 = round(cv.bounds.y + cv.bounds.w - (z.m.y - cv.view_price0) * cv.price_to_px);
    // Bounded in time by m.zw; the ±1e30 sentinel (an order zone) clamps to the plot's edges.
    // `edge` can pan LEFT of the plot, which would make min > max; order the bounds first.
    float lo = cv.bounds.x;
    float hi = max(cv.bounds.x + (cv.pad - cv.view_time0) * cv.time_to_px, lo);
    float left = clamp(cv.bounds.x + (z.m.z - cv.view_time0) * cv.time_to_px, lo, hi);
    float right = clamp(cv.bounds.x + (z.m.w - cv.view_time0) * cv.time_to_px, lo, hi);
    float2 c = CORNERS_ALT[vid];
    float2 px = float2(mix(left, right, (c.x + 1.0) * 0.5), mix(y0, y1, (c.y + 1.0) * 0.5));
    return { to_clip(px, cv.resolution), z.color };
}

fragment float4 zone_fragment(ZOut in [[stage_in]]) { return in.color; }

struct HOut { float4 position [[position]]; float4 color; float style [[flat]]; float xpx; };

vertex HOut hline_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                         constant ChartView& cv [[buffer(0)]],
                         const device GpuLine* lines [[buffer(1)]]) {
    GpuLine h = lines[iid];
    float cy = round(cv.bounds.y + cv.bounds.w - (h.m.x - cv.view_price0) * cv.price_to_px);
    float left = cv.bounds.x, right = cv.bounds.x + cv.bounds.z;
    float2 px = float2((left + right) * 0.5 + CORNERS_ALT[vid].x * (right - left) * 0.5,
                       cy + CORNERS_ALT[vid].y * max(h.m.z, 1.0) * 0.5);
    return { to_clip(px, cv.resolution), h.color, h.m.y, px.x };
}

fragment float4 hline_fragment(HOut in [[stage_in]]) {
    if (in.style >= 0.5 && fract(in.xpx / 16.0) > 9.0 / 16.0) discard_fragment();
    return in.color;
}

struct SOut { float4 position [[position]]; float4 color; float pattern [[flat]]; float dist; };

vertex SOut seg_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                       constant ChartView& cv [[buffer(0)]],
                       const device GpuSeg* segs [[buffer(1)]]) {
    GpuSeg s = segs[iid];
    float2 a = data_to_px(cv, s.pts.x, s.pts.y);
    float t1 = s.m.z >= 0.5 ? cv.pad : s.pts.z;
    float2 b = data_to_px(cv, t1, s.pts.w);
    // Snap endpoint Y coordinates to whole pixels; otherwise a horizontal order line flickers
    // in thickness/brightness as view_price0 drifts by subpixels (matching hline's round()).
    a.y = round(a.y);
    b.y = round(b.y);
    float2 dir = b - a;
    float len = max(length(dir), 1e-4);
    dir /= len;
    float2 nrm = float2(-dir.y, dir.x) * max(s.m.x, 1.0) * 0.5;
    float along = (vid == 1 || vid == 2 || vid == 4) ? 1.0 : 0.0;
    float side = (vid == 2 || vid == 4 || vid == 5) ? 1.0 : -1.0;
    float2 px = mix(a, b, along) + nrm * side;
    return { to_clip(px, cv.resolution), s.color, s.m.y, len * along };
}

fragment float4 seg_fragment(SOut in [[stage_in]]) {
    if (in.pattern >= 1.5) {
        if (fract(in.dist / 6.0) > 2.0 / 6.0) discard_fragment();
    } else if (in.pattern >= 0.5) {
        float x = fract(in.dist / 20.0) * 20.0;
        if (!(x < 8.0 || (x >= 11.0 && x < 13.0) || (x >= 16.0 && x < 18.0))) discard_fragment();
    }
    return in.color;
}

// shape: 0 = cross, 1 = filled knot, 2 = news gem (pos.z half height, pos.w half width, m.z/m.w the
// tag-colour wedge). m.y anchor: 0 = price, 1 = plot bottom (pos.y is physical px above the bottom
// edge, plus a horizontal clip to the plot). Keep in sync with order_lines.hlsl / native_marker.wgsl.
struct MOut { float4 position [[position]]; float4 color; float2 local; float shape [[flat]]; float thick [[flat]]; float sz [[flat]]; float2 xclip [[flat]]; float2 wedge [[flat]]; };

constant float GEM_FACET_GAP = 0.055;
constant float GEM_LEFT_SHADE = 0.78;
constant float GEM_TWO_PI = 6.28318531;

vertex MOut marker_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                          constant ChartView& cv [[buffer(0)]],
                          const device GpuMarker* markers [[buffer(1)]]) {
    GpuMarker mk = markers[iid];
    float2 c = data_to_px(cv, mk.pos.x, mk.pos.y);
    bool bottom = mk.m.y > 0.5;
    if (bottom) c.y = cv.bounds.y + cv.bounds.w - mk.pos.y;
    float2 center = round(c);
    float half_sz = max(mk.pos.z, 1.0);
    // The gem is taller than wide; every other marker keeps its square quad.
    float2 half_ext = (mk.m.x >= 1.5) ? float2(max(mk.pos.w, 1.0), half_sz) : float2(half_sz, half_sz);
    float2 local = CORNERS_ALT[vid] * half_ext;
    // Price-anchored markers keep their historical reach (order lines extend into the book zone).
    float2 xclip = bottom ? float2(cv.bounds.x, cv.bounds.x + cv.bounds.z) : float2(-1e30, 1e30);
    float2 wedge = float2(mk.m.z, max(mk.m.w, 1.0));
    return { to_clip(center + local, cv.resolution), mk.color, local, mk.m.x, mk.pos.w, half_sz, xclip, wedge };
}

fragment float4 marker_fragment(MOut in [[stage_in]]) {
    if (in.position.x < in.xclip.x || in.position.x > in.xclip.y) discard_fragment();
    if (in.shape < 0.5) {
        float h = max(in.thick, 1.0) * 0.5;
        float d1 = abs(in.local.x - in.local.y) * 0.70710678;
        float d2 = abs(in.local.x + in.local.y) * 0.70710678;
        if (min(d1, d2) > h) discard_fragment();
        return in.color;
    }
    if (in.shape < 1.5) {
        if (length(in.local) > in.sz) discard_fragment();
        return in.color;
    }
    if (in.shape > 2.5) {
        // Warning badge: an upward triangle (apex at top, base on the axis) with a dark
        // exclamation mark cut into it. local.y is +down, so the base sits at +sz.
        float tw = max(in.thick, 1.0);
        float nx = in.local.x / tw;
        float ny = in.local.y / max(in.sz, 1.0);
        if (ny < 2.0 * abs(nx) - 1.0) discard_fragment();
        bool bar = abs(nx) < 0.13 && ny > -0.30 && ny < 0.34;
        bool dot = abs(nx) < 0.15 && ny > 0.50 && ny < 0.74;
        if (bar || dot) return float4(in.color.rgb * 0.14, in.color.a);
        return in.color;
    }
    // News gem: a vertically elongated diamond, optionally cut into wedges by tag colour.
    float hw = max(in.thick, 1.0);
    if (abs(in.local.x) / hw + abs(in.local.y) / max(in.sz, 1.0) > 1.0) discard_fragment();
    if (in.wedge.y > 1.5) {
        // Wedge index runs clockwise from the top tip, so a two-colour gem splits left/right.
        float ang = atan2(in.local.x, -in.local.y);
        if (ang < 0.0) ang += GEM_TWO_PI;
        float f = ang / GEM_TWO_PI * in.wedge.y;
        float idx = floor(f);
        if (abs(idx - in.wedge.x) > 0.5) discard_fragment();
        // Facet gaps only ABOVE the center: with an even wedge count one boundary lands exactly on
        // the bottom tip, and cutting there would lift the gem off the axis it marks.
        float frac = f - idx;
        if (in.local.y < 0.0 && (frac < GEM_FACET_GAP || frac > 1.0 - GEM_FACET_GAP)) discard_fragment();
    }
    float shade = (in.local.x < 0.0) ? GEM_LEFT_SHADE : 1.0;
    return float4(in.color.rgb * shade, in.color.a);
}
