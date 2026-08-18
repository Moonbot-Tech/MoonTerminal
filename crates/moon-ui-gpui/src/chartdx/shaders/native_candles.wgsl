// Candle layer (wgpu): mirrors candles.hlsl. One instance = body + upper/lower wicks
// (18 vertices); modes, zone, and outline are CandleStyle constants.

struct ChartView {
    bounds: vec4<f32>,
    resolution: vec2<f32>,
    time_to_px: f32,
    view_time0: f32,
    price_to_px: f32,
    view_price0: f32,
    marker_half: f32,
    pad: f32,
    volume_buy_inv: f32,
    volume_sell_inv: f32,
    volume_alpha: f32,
    _pad2: f32,
};

struct CandleStyle {
    up: vec4<f32>,
    down: vec4<f32>,
    neutral: vec4<f32>,
    tf_rel: f32,
    zone_start: f32,
    mode: f32,
    outline_px: f32,
    wicks_in_zone: f32,
    neutral_in_zone: f32,
    fill_alpha: f32,
    // rel ms: omit candles with t_open >= boundary (trades only).
    hide_start: f32,
};

struct Candle {
    t_open: f32,
    o: f32,
    h: f32,
    l: f32,
    c: f32,
    vol: f32,
    // Candle's OWN timeframe (rel ms); 0 = series timeframe. History tail uses wider, muted higher timeframes.
    tf_rel: f32,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<uniform> cs: CandleStyle;
@group(0) @binding(2) var<storage, read> candles: array<Candle>;

struct CandleOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) size_px: vec2<f32>,
    @location(2) @interpolate(flat) outline: f32,
    @location(3) @interpolate(flat) color: vec4<f32>,
};

fn price_y(price: f32) -> f32 {
    return cv.bounds.y + cv.bounds.w - (price - cv.view_price0) * cv.price_to_px;
}

fn cull_out() -> CandleOut {
    var o: CandleOut;
    o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(0.0, 0.0);
    o.size_px = vec2<f32>(1.0, 1.0);
    o.outline = 0.0;
    o.color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    return o;
}

@vertex
fn candles_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> CandleOut {
    let cd = candles[iid];
    let part = vid / 6u; // 0 body, 1 upper wick, 2 lower wick
    let corner = CORNERS_01[vid % 6u];

    if cd.t_open >= cs.hide_start {
        return cull_out(); // Omit the candle in the "trades only" zone.
    }
    let foreign_tf = cd.tf_rel > 0.0 && abs(cd.tf_rel - cs.tf_rel) > 0.5;
    var tf_rel = cs.tf_rel;
    if cd.tf_rel > 0.0 {
        tf_rel = cd.tf_rel;
    }
    var x0 = cv.bounds.x + (cd.t_open - cv.view_time0) * cv.time_to_px;
    var x1 = x0 + tf_rel * cv.time_to_px;
    if x1 < cv.bounds.x - 2.0 || x0 > cv.bounds.x + cv.bounds.z + 2.0 {
        return cull_out();
    }
    x0 = round(x0);
    x1 = max(round(x1), x0 + 1.0);

    let in_zone = cd.t_open >= cs.zone_start;
    let outline = (cs.mode >= 0.5 && cs.mode < 1.5) || (cs.mode >= 1.5 && in_zone);
    if part != 0u && in_zone && cs.wicks_in_zone < 0.5 {
        return cull_out();
    }

    var y_top_body = min(price_y(cd.o), price_y(cd.c));
    var y_bot_body = max(price_y(cd.o), price_y(cd.c));
    y_top_body = round(y_top_body);
    y_bot_body = max(round(y_bot_body), y_top_body + 1.0);

    let gap = clamp((x1 - x0) * 0.10, 0.0, 4.0);
    var bx0 = x0 + gap;
    var bx1 = x1 - gap;
    if bx1 - bx0 < 1.0 {
        let cx = floor((x0 + x1) * 0.5);
        bx0 = cx;
        bx1 = cx + 1.0;
    }

    var p0: vec2<f32>;
    var sz: vec2<f32>;
    if part == 0u {
        p0 = vec2<f32>(bx0, y_top_body);
        sz = vec2<f32>(bx1 - bx0, y_bot_body - y_top_body);
    } else {
        let wick_w = max(1.0, min(cs.outline_px, bx1 - bx0));
        let wx = floor((x0 + x1) * 0.5 - wick_w * 0.5);
        var y0: f32;
        var y1: f32;
        if part == 1u {
            y0 = round(price_y(cd.h));
            y1 = y_top_body;
        } else {
            y0 = y_bot_body;
            y1 = round(price_y(cd.l));
        }
        if y1 - y0 < 0.5 {
            return cull_out();
        }
        p0 = vec2<f32>(wx, y0);
        sz = vec2<f32>(wick_w, y1 - y0);
    }

    let px = p0 + corner * sz;
    let ndc = vec2<f32>(px.x / cv.resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv.resolution.y * 2.0);

    var base = cs.down;
    if cd.c >= cd.o {
        base = cs.up;
    }
    if in_zone && cs.neutral_in_zone > 0.5 {
        base = cs.neutral;
    }
    var alpha = 1.0;
    if part == 0u && !outline {
        alpha = saturate(cs.fill_alpha);
    }
    if foreign_tf {
        alpha *= 0.55; // The tail from another timeframe is translucent.
    }

    var o: CandleOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = corner;
    o.size_px = sz;
    if part == 0u && outline {
        o.outline = 1.0;
    } else {
        o.outline = 0.0;
    }
    o.color = vec4<f32>(base.rgb, alpha);
    return o;
}

// ---- Bottom volume band (mirrors candles.hlsl) -----------------------------
struct VolumeStyle {
    up: vec4<f32>,
    down: vec4<f32>,
    scale: vec4<f32>,
    m: vec4<f32>,  // x style, y height fraction, z 1/max, w avg/max
    m2: vec4<f32>, // x band cap px, y bar width px, z line px
};

@group(0) @binding(3) var<uniform> vs: VolumeStyle;

struct VolumeBarOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) up: u32,
};

fn vol_center_px(cd: Candle) -> vec2<f32> {
    var tf_rel = cs.tf_rel;
    if cd.tf_rel > 0.0 {
        tf_rel = cd.tf_rel;
    }
    let x0 = cv.bounds.x + (cd.t_open - cv.view_time0) * cv.time_to_px;
    return vec2<f32>(x0 + tf_rel * cv.time_to_px * 0.5, tf_rel * cv.time_to_px);
}

fn vol_band_h() -> f32 {
    return min(cv.bounds.w * vs.m.y, vs.m2.x);
}

fn vol_height_px(cd: Candle) -> f32 {
    let norm = clamp(cd.vol * vs.m.z, 0.0, 1.0);
    return sqrt(norm) * vol_band_h();
}

fn vol_cull() -> VolumeBarOut {
    var o: VolumeBarOut;
    o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    o.up = 0u;
    return o;
}

@vertex
fn volume_bars_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> VolumeBarOut {
    if vs.m.x < 0.5 {
        return vol_cull();
    }
    let cd = candles[iid];
    let c0 = vol_center_px(cd);
    let base = cv.bounds.y + cv.bounds.w - 1.0;
    let h0 = vol_height_px(cd);
    let corner = CORNERS_01[vid % 6u];

    var o: VolumeBarOut;
    o.up = 0u;
    if cd.c >= cd.o {
        o.up = 1u;
    }

    if vs.m.x < 1.5 {
        let bw = clamp(c0.y * 0.5, 1.0, vs.m2.y);
        let p0 = vec2<f32>(round(c0.x - bw * 0.5), base - h0);
        var px = p0 + corner * vec2<f32>(bw, h0);
        px.x = clamp(px.x, cv.bounds.x, cv.bounds.x + cv.bounds.z);
        o.pos = to_clip(px, cv.resolution);
        return o;
    }

    let cd1 = candles[iid + 1u];
    let c1 = vol_center_px(cd1);
    let h1 = vol_height_px(cd1);
    let x = mix(c0.x, c1.x, corner.x);
    let h = mix(h0, h1, corner.x);
    var pxh = vec2<f32>(x, base - h * (1.0 - corner.y));
    pxh.x = clamp(pxh.x, cv.bounds.x, cv.bounds.x + cv.bounds.z);
    o.pos = to_clip(pxh, cv.resolution);
    return o;
}

@fragment
fn volume_bars_fragment(in: VolumeBarOut) -> @location(0) vec4<f32> {
    return select(vs.down, vs.up, in.up == 1u);
}

struct VolumeScaleOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn volume_scale_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> VolumeScaleOut {
    var o: VolumeScaleOut;
    if vs.m.x < 0.5 {
        o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        return o;
    }
    let band = vol_band_h();
    var frac = sqrt(clamp(vs.m.w, 0.0, 1.0));
    if iid == 0u {
        frac = 1.0;
    }
    let base = cv.bounds.y + cv.bounds.w - 1.0;
    let y = round(base - band * frac);
    let th = max(vs.m2.z, 1.0);
    let corner = CORNERS_01[vid % 6u];
    let px = vec2<f32>(cv.bounds.x, y) + corner * vec2<f32>(cv.bounds.z, th);
    o.pos = to_clip(px, cv.resolution);
    return o;
}

@fragment
fn volume_scale_fragment(_in: VolumeScaleOut) -> @location(0) vec4<f32> {
    return vs.scale;
}

@fragment
fn candles_fragment(in: CandleOut) -> @location(0) vec4<f32> {
    if in.outline > 0.5 {
        let dx = min(in.uv.x, 1.0 - in.uv.x) * in.size_px.x;
        let dy = min(in.uv.y, 1.0 - in.uv.y) * in.size_px.y;
        if min(dx, dy) > max(cs.outline_px, 1.0) {
            discard;
        }
        // Alpha comes from VS: normally 1.0 for outlines and muted for another timeframe's tail.
        return in.color;
    }
    return in.color;
}
