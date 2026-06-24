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
    float price_to_px;
    float view_price0;
    float price_interval;
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

struct GridOut { float4 position [[position]]; float2 px; };

vertex GridOut grid_vertex(uint vid [[vertex_id]], constant GridParams& gp [[buffer(0)]]) {
    float2 c = CORNERS_01[vid];
    float2 px = gp.bounds.xy + c * gp.bounds.zw;
    return { to_clip(px, gp.resolution), px };
}

fragment float4 grid_fragment(GridOut in [[stage_in]], constant GridParams& gp [[buffer(0)]]) {
    float3 bg = gp.bg.rgb;
    float3 grid_col = mix(bg, gp.grid_col.rgb, saturate(gp.grid_alpha));
    bool hit = false;
    float step_x = gp.bounds.z / max(gp.n_vert, 1.0);
    float local_x = in.px.x - gp.bounds.x;
    if (abs(local_x - round(local_x / step_x) * step_x) < 1.0) hit = true;
    if (gp.price_interval > 1e-12 && gp.price_to_px > 1e-9) {
        float price = gp.view_price0 + (gp.bounds.y + gp.bounds.w - in.px.y) / gp.price_to_px;
        float k = price / gp.price_interval;
        if (abs(k - round(k)) * gp.price_interval * gp.price_to_px < 1.0) hit = true;
    }
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

struct CrossOut { float4 position [[position]]; float2 uv; uint side [[flat]]; };

vertex CrossOut crosses_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                               constant ChartView& cv [[buffer(0)]],
                               const device Cross* crosses [[buffer(1)]]) {
    Cross c = crosses[uint(max(cv.pad, 0.0)) + iid];
    float sx = round(cv.bounds.x + (c.time_rel - cv.view_time0) * cv.time_to_px);
    float sy = round(cv.bounds.y + cv.bounds.w - (c.price - cv.view_price0) * cv.price_to_px);
    if (sx < cv.bounds.x - 8.0 || sx > cv.bounds.x + cv.bounds.z + 8.0 ||
        sy < cv.bounds.y - 8.0 || sy > cv.bounds.y + cv.bounds.w + 8.0) {
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
    Cross c = crosses[uint(max(cv.pad, 0.0)) + iid];
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

fragment float4 book_bg_fragment(constant BookStyle& bs [[buffer(1)]]) {
    return float4(bs.book_bg.rgb, 1.0);
}

struct ZOut { float4 position [[position]]; float4 color; };

vertex ZOut zone_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                        constant ChartView& cv [[buffer(0)]],
                        const device GpuZone* zones [[buffer(1)]]) {
    GpuZone z = zones[iid];
    float y0 = cv.bounds.y + cv.bounds.w - (z.m.x - cv.view_price0) * cv.price_to_px;
    float y1 = cv.bounds.y + cv.bounds.w - (z.m.y - cv.view_price0) * cv.price_to_px;
    float left = cv.bounds.x;
    float right = cv.bounds.x + (cv.pad - cv.view_time0) * cv.time_to_px;
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

struct SOut { float4 position [[position]]; float4 color; float dashed [[flat]]; float dist; };

vertex SOut seg_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                       constant ChartView& cv [[buffer(0)]],
                       const device GpuSeg* segs [[buffer(1)]]) {
    GpuSeg s = segs[iid];
    float2 a = data_to_px(cv, s.pts.x, s.pts.y);
    float t1 = s.m.z >= 0.5 ? cv.pad : s.pts.z;
    float2 b = data_to_px(cv, t1, s.pts.w);
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
    if (in.dashed >= 0.5 && fract(in.dist / 16.0) > 9.0 / 16.0) discard_fragment();
    return in.color;
}

struct MOut { float4 position [[position]]; float4 color; float2 local; float shape [[flat]]; float thick [[flat]]; float sz [[flat]]; };

vertex MOut marker_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                          constant ChartView& cv [[buffer(0)]],
                          const device GpuMarker* markers [[buffer(1)]]) {
    GpuMarker mk = markers[iid];
    float2 center = round(data_to_px(cv, mk.pos.x, mk.pos.y));
    float half_sz = max(mk.pos.z, 1.0);
    float2 local = CORNERS_ALT[vid] * half_sz;
    return { to_clip(center + local, cv.resolution), mk.color, local, mk.m.x, mk.pos.w, half_sz };
}

fragment float4 marker_fragment(MOut in [[stage_in]]) {
    if (in.shape < 0.5) {
        float h = max(in.thick, 1.0) * 0.5;
        float d1 = abs(in.local.x - in.local.y) * 0.70710678;
        float d2 = abs(in.local.x + in.local.y) * 0.70710678;
        if (min(d1, d2) > h) discard_fragment();
    } else if (length(in.local) > in.sz) {
        discard_fragment();
    }
    return in.color;
}
