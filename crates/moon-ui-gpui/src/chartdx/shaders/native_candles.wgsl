// Слой свечей (wgpu): зеркало candles.hlsl. Один инстанс = тело + верхний/нижний
// фитили (18 вершин); режимы/зона/контур — константами CandleStyle.

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
    // rel ms: свечи с t_open >= границы не рисуем (только трейды).
    hide_start: f32,
};

struct Candle {
    t_open: f32,
    o: f32,
    h: f32,
    l: f32,
    c: f32,
    vol: f32,
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
    let part = vid / 6u; // 0 тело, 1 верхний фитиль, 2 нижний фитиль
    let corner = CORNERS_01[vid % 6u];

    if cd.t_open >= cs.hide_start {
        return cull_out(); // зона «только трейды» — свечу не рисуем
    }
    var x0 = cv.bounds.x + (cd.t_open - cv.view_time0) * cv.time_to_px;
    var x1 = x0 + cs.tf_rel * cv.time_to_px;
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

@fragment
fn candles_fragment(in: CandleOut) -> @location(0) vec4<f32> {
    if in.outline > 0.5 {
        let dx = min(in.uv.x, 1.0 - in.uv.x) * in.size_px.x;
        let dy = min(in.uv.y, 1.0 - in.uv.y) * in.size_px.y;
        if min(dx, dy) > max(cs.outline_px, 1.0) {
            discard;
        }
        return vec4<f32>(in.color.rgb, 1.0);
    }
    return in.color;
}
