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

struct GpuMarker {
    color: vec4<f32>,
    pos: vec4<f32>,
    m: vec4<f32>,
};

const CORNERS_PM_ALT: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

fn data_to_px(cv: ChartView, t_rel: f32, price: f32) -> vec2<f32> {
    let x = cv.bounds.x + (t_rel - cv.view_time0) * cv.time_to_px;
    let y = cv.bounds.y + cv.bounds.w - (price - cv.view_price0) * cv.price_to_px;
    return vec2<f32>(x, y);
}

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<storage, read> markers: array<GpuMarker>;

struct MOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) @interpolate(flat) shape: f32,
    @location(3) @interpolate(flat) thick: f32,
    @location(4) @interpolate(flat) sz: f32,
};

@vertex
fn marker_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> MOut {
    let mk = markers[iid];
    let c = data_to_px(cv, mk.pos.x, mk.pos.y);
    let center = vec2<f32>(round(c.x), round(c.y));
    let half_sz = max(mk.pos.z, 1.0);
    let corner = CORNERS_PM_ALT[vid];
    let px = center + corner * half_sz;
    var out: MOut;
    out.pos = to_clip(px, cv.resolution);
    out.color = mk.color;
    out.local = corner * half_sz;
    out.shape = mk.m.x;
    out.thick = mk.pos.w;
    out.sz = half_sz;
    return out;
}

@fragment
fn marker_fragment(in: MOut) -> @location(0) vec4<f32> {
    if in.shape < 0.5 {
        let h = max(in.thick, 1.0) * 0.5;
        let d1 = abs(in.local.x - in.local.y) * 0.70710678;
        let d2 = abs(in.local.x + in.local.y) * 0.70710678;
        if min(d1, d2) > h {
            discard;
        }
    } else if length(in.local) > in.sz {
        discard;
    }
    return in.color;
}
