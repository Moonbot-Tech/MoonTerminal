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

struct GpuZone {
    color: vec4<f32>,
    m: vec4<f32>,
};

const CORNERS_PM_ALT: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<storage, read> zones: array<GpuZone>;

struct ZOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn zone_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> ZOut {
    let z = zones[iid];
    let y0 = cv.bounds.y + cv.bounds.w - (z.m.x - cv.view_price0) * cv.price_to_px;
    let y1 = cv.bounds.y + cv.bounds.w - (z.m.y - cv.view_price0) * cv.price_to_px;
    // Bounded in time by m.zw; the ±1e30 sentinel (an order zone) clamps to the plot's edges.
    // `edge` can pan LEFT of the plot, which would make min > max; order the bounds first.
    let lo = cv.bounds.x;
    let hi = max(cv.bounds.x + (cv.pad - cv.view_time0) * cv.time_to_px, lo);
    let left = clamp(cv.bounds.x + (z.m.z - cv.view_time0) * cv.time_to_px, lo, hi);
    let right = clamp(cv.bounds.x + (z.m.w - cv.view_time0) * cv.time_to_px, lo, hi);
    let corner = CORNERS_PM_ALT[vid];
    let px = vec2<f32>(mix(left, right, (corner.x + 1.0) * 0.5), mix(y0, y1, (corner.y + 1.0) * 0.5));
    var out: ZOut;
    out.pos = to_clip(px, cv.resolution);
    out.color = z.color;
    return out;
}

@fragment
fn zone_fragment(in: ZOut) -> @location(0) vec4<f32> {
    return in.color;
}
