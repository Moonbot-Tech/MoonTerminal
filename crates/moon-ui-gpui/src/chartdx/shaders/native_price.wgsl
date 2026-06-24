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

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<storage, read> price_points: array<PricePoint>;

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
    let a = price_point_px(price_points[iid]);
    let b = price_point_px(price_points[iid + 1u]);
    var dir = b - a;
    let len = max(length(dir), 1e-4);
    dir = dir / len;
    let nrm = vec2<f32>(-dir.y, dir.x) * 0.85;
    let along = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    let side = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let px = mix(a, b, along[vid]) + nrm * side[vid];
    var out: PriceLineOut;
    out.pos = to_clip(px, cv.resolution);
    return out;
}

@fragment
fn price_last_fragment(_in: PriceLineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.82, 0.60, 0.36, 0.82);
}

@fragment
fn price_mark_fragment(_in: PriceLineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.42, 0.72, 1.00, 0.78);
}
