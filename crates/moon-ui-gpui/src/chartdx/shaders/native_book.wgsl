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

struct BookStyle {
    book_bg: vec4<f32>,
    bid: vec4<f32>,
    ask: vec4<f32>,
    level: vec4<f32>,
};

struct Level {
    price: f32,
    span: f32,
    len_norm: f32,
    kind: f32, // 0 = bid fill, 1 = ask fill, 2 = bid level, 3 = ask level
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);
const CORNERS_PM: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
    vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<uniform> bs: BookStyle;
@group(0) @binding(2) var<storage, read> levels: array<Level>;

struct BookOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) kind: f32,
};

@vertex
fn book_bars_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> BookOut {
    let lv = levels[iid];
    let zone = cv.bounds.z;
    let right = cv.bounds.x + zone;
    let seg_len = max(lv.len_norm * zone, 1.0);
    let cx = right - seg_len * 0.5;
    let base = cv.bounds.y + cv.bounds.w;
    let y_price = base - (lv.price - cv.view_price0) * cv.price_to_px;
    let inner = lv.price + lv.span;
    let y_inner = base - (inner - cv.view_price0) * cv.price_to_px;
    var top = round(min(y_price, y_inner));
    var bot = round(max(y_price, y_inner));
    if bot - top < 1.0 {
        bot = top + 1.0;
    }
    var cy = (top + bot) * 0.5;
    var hh = bot - top;
    if lv.kind >= 2.0 {
        cy = round(y_price);
        hh = max(bs.level.y, 1.0);
    }
    let corner = CORNERS_PM[vid];
    let px = vec2<f32>(cx + corner.x * seg_len * 0.5, cy + corner.y * hh * 0.5);
    var out: BookOut;
    out.pos = to_clip(px, cv.resolution);
    out.kind = lv.kind;
    return out;
}

@fragment
fn book_bars_fragment(in: BookOut) -> @location(0) vec4<f32> {
    if in.kind < 0.5 {
        return vec4<f32>(bs.bid.rgb, 1.0);
    } else if in.kind < 1.5 {
        return vec4<f32>(bs.ask.rgb, 1.0);
    } else if in.kind < 2.5 {
        return vec4<f32>(min(bs.bid.rgb * 1.25, vec3<f32>(1.0)), bs.level.x);
    }
    return vec4<f32>(min(bs.ask.rgb * 1.25, vec3<f32>(1.0)), bs.level.x);
}

struct PlainOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn book_bg_vertex(@builtin(vertex_index) vid: u32) -> PlainOut {
    let c = CORNERS_01[vid];
    let px = cv.bounds.xy + c * cv.bounds.zw;
    var out: PlainOut;
    out.pos = to_clip(px, cv.resolution);
    return out;
}

@fragment
fn book_bg_fragment(_in: PlainOut) -> @location(0) vec4<f32> {
    return vec4<f32>(bs.book_bg.rgb, 1.0);
}
