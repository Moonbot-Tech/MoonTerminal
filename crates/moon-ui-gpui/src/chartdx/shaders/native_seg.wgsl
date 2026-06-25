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

struct GpuSeg {
    pts: vec4<f32>,
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

fn data_to_px(cv: ChartView, t_rel: f32, price: f32) -> vec2<f32> {
    let x = cv.bounds.x + (t_rel - cv.view_time0) * cv.time_to_px;
    let y = cv.bounds.y + cv.bounds.w - (price - cv.view_price0) * cv.price_to_px;
    return vec2<f32>(x, y);
}

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<storage, read> segs: array<GpuSeg>;

struct SOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) pattern: f32,
    @location(2) dist: f32,
};

@vertex
fn seg_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> SOut {
    let s = segs[iid];
    let a = data_to_px(cv, s.pts.x, s.pts.y);
    let t1 = select(s.pts.z, cv.pad, s.m.z >= 0.5);
    let b = data_to_px(cv, t1, s.pts.w);
    var dir = b - a;
    let len = max(length(dir), 1e-4);
    dir = dir / len;
    let nrm = vec2<f32>(-dir.y, dir.x) * max(s.m.x, 1.0) * 0.5;
    let along = array<f32, 6>(0.0, 1.0, 1.0, 0.0, 1.0, 0.0);
    let side = array<f32, 6>(-1.0, -1.0, 1.0, -1.0, 1.0, 1.0);
    let px = mix(a, b, along[vid]) + nrm * side[vid];
    var out: SOut;
    out.pos = to_clip(px, cv.resolution);
    out.color = s.color;
    out.pattern = s.m.y;
    out.dist = len * along[vid];
    return out;
}

@fragment
fn seg_fragment(in: SOut) -> @location(0) vec4<f32> {
    if in.pattern >= 1.5 {
        if fract(in.dist / 6.0) > 2.0 / 6.0 {
            discard;
        }
    } else if in.pattern >= 0.5 {
        let x = fract(in.dist / 20.0) * 20.0;
        if !(x < 8.0 || (x >= 11.0 && x < 13.0) || (x >= 16.0 && x < 18.0)) {
            discard;
        }
    }
    return in.color;
}
