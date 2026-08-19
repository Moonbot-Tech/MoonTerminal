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
    // extend: 0 = as given, 1 = to the right edge at the SAME price (an order line), 2 = ray.
    // A ray keeps its DIRECTION, so its far end is pushed along a->b past the plot and left to the
    // clip — solving against an edge would divide by a direction component that is zero for a
    // vertical ray. Two details matter: the direction is taken from the UNSNAPPED points, because a
    // one-pixel round over a long reach visibly tilts the line, and the far end is not snapped at
    // all for the same reason. Only the start keeps the snap that stops a thin line from flickering.
    let s = segs[iid];
    let ray = s.m.z >= 1.5;
    let a_raw = data_to_px(cv, s.pts.x, s.pts.y);
    let t1 = select(s.pts.z, cv.pad, s.m.z >= 0.5 && !ray);
    let b_raw = data_to_px(cv, t1, s.pts.w);
    // Snap endpoint Y coordinates to whole pixels; otherwise a horizontal order line flickers
    // in thickness/brightness as view_price0 drifts by subpixels (matching hline's round()).
    var a = vec2<f32>(a_raw.x, round(a_raw.y));
    let b_snapped = vec2<f32>(b_raw.x, round(b_raw.y));
    let d = b_raw - a_raw;
    let reach = length(cv.bounds.zw) + length(d) + 1.0;
    var b = select(b_snapped, a + normalize(d + vec2<f32>(1e-6, 0.0)) * reach, ray);
    // m.w = SEG_CLAMP_PLOT pins the segment to the plot once its price leaves the visible band, so
    // an exit line a few percent away stays visible and grabbable at any zoom instead of being
    // clipped away. Only Y moves; the instance still carries the order's real price. Why there is no
    // inset, and what mirrors this on the CPU, is on `SEG_CLAMP_PLOT` in moon-chart's
    // layers/order_lines.rs — stated once, there, for all three backends.
    if (s.m.w >= 0.5) {
        let lo = cv.bounds.y;
        let hi = max(cv.bounds.y + cv.bounds.w, lo);
        a.y = clamp(a.y, lo, hi);
        b.y = clamp(b.y, lo, hi);
    }
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

// ── Line patterns (TPenStyle order: 0 solid · 1 dash · 2 dot · 3 dash-dot · 4 dash-dot-dot) ──
// `d` is distance along the line in physical pixels: X for a full-width line, arc length for a
// segment. Kept identical to the DX11 and Metal copies — three backends draw the same five styles.
fn pattern_on(style: f32, d: f32) -> bool {
    if style < 0.5 { return true; }
    if style < 1.5 { return fract(d / 16.0) < 9.0 / 16.0; }
    if style < 2.5 { return fract(d / 6.0) < 2.0 / 6.0; }
    let x = fract(d / 20.0) * 20.0;
    if style < 3.5 { return x < 9.0 || (x >= 13.0 && x < 15.0); }
    return x < 8.0 || (x >= 11.0 && x < 13.0) || (x >= 16.0 && x < 18.0);
}

@fragment
fn seg_fragment(in: SOut) -> @location(0) vec4<f32> {
    if !pattern_on(in.pattern, in.dist) {
        discard;
    }
    return in.color;
}
