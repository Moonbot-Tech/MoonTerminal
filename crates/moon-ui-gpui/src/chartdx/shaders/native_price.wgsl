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

struct PricePoint {
    time_rel_ms: f32,
    price: f32,
};

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

struct PriceStyle {
    last: vec4<f32>,
    mark: vec4<f32>,
    m: vec4<f32>, // x = line half-width in physical px
};

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<storage, read> price_points: array<PricePoint>;
@group(0) @binding(2) var<uniform> ps: PriceStyle;

struct PriceLineOut {
    @builtin(position) pos: vec4<f32>,
};

fn price_point_px(p: PricePoint) -> vec2<f32> {
    let x = cv.bounds.x + (p.time_rel_ms - cv.view_time0) * cv.time_to_px;
    let y = cv.bounds.y + cv.bounds.w - (p.price - cv.view_price0) * cv.price_to_px;
    return vec2<f32>(x, y);
}

@vertex
fn price_line_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> PriceLineOut {
    let p0 = price_points[iid];
    let p1 = price_points[iid + 1u];
    var out: PriceLineOut;
    // A negative price is a gap (`PRICE_LINE_GAP`): no segment reaches it. Every vertex of the
    // segment lands on one point outside the clip volume, so nothing is rasterized.
    // Keep in sync with crosses.hlsl / chart_native.metal.
    if (p0.price < 0.0 || p1.price < 0.0) {
        out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        return out;
    }
    let a = price_point_px(p0);
    let b = price_point_px(p1);
    var dir = b - a;
    let len = max(length(dir), 1e-4);
    dir = dir / len;
    let nrm = vec2<f32>(-dir.y, dir.x) * max(ps.m.x, 0.25);
    let along = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    let side = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let px = mix(a, b, along[vid]) + nrm * side[vid];
    out.pos = to_clip(px, cv.resolution);
    return out;
}

@fragment
fn price_last_fragment(_in: PriceLineOut) -> @location(0) vec4<f32> {
    return ps.last;
}

@fragment
fn price_mark_fragment(_in: PriceLineOut) -> @location(0) vec4<f32> {
    return ps.mark;
}
