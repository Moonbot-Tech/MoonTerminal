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

struct GpuLine {
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
@group(0) @binding(1) var<storage, read> hlines: array<GpuLine>;

struct HOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) style: f32,
    @location(2) xpx: f32,
};

@vertex
fn hline_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> HOut {
    let h = hlines[iid];
    let price = h.m.x;
    let style = h.m.y;
    let thickness = h.m.z;
    let base = cv.bounds.y + cv.bounds.w;
    let cy = round(base - (price - cv.view_price0) * cv.price_to_px);
    let left = cv.bounds.x;
    let right = cv.bounds.x + cv.bounds.z;
    let cx = (left + right) * 0.5;
    let half_w = (right - left) * 0.5;
    let half_h = max(thickness, 1.0) * 0.5;
    let corner = CORNERS_PM_ALT[vid];
    let px = vec2<f32>(cx + corner.x * half_w, cy + corner.y * half_h);
    var out: HOut;
    out.pos = to_clip(px, cv.resolution);
    out.color = h.color;
    out.style = style;
    out.xpx = px.x;
    return out;
}

@fragment
fn hline_fragment(in: HOut) -> @location(0) vec4<f32> {
    if in.style >= 0.5 && fract(in.xpx / 16.0) > 9.0 / 16.0 {
        discard;
    }
    return in.color;
}
