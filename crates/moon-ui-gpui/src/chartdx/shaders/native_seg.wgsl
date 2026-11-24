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

// One axis of the Liang-Barsky clip in `seg_vertex`: narrows the visible parameter range `t` of
// `p + d * t` to [lo, hi] on this axis; an axis-parallel segment outside the band empties it.
fn seg_clip_axis(p: f32, d: f32, lo: f32, hi: f32, t: vec2<f32>) -> vec2<f32> {
    if (d != 0.0) {
        let ta = (lo - p) / d;
        let tb = (hi - p) / d;
        return vec2<f32>(max(t.x, min(ta, tb)), min(t.y, max(ta, tb)));
    }
    if (p < lo || p > hi) {
        return vec2<f32>(t.x, -1.0);
    }
    return t;
}

// Period of every line pattern in `pattern_on` (lcm of 16, 6 and 20): a dash phase carried modulo
// this stays exact however far the visible part starts from the segment's origin.
const SEG_PATTERN_PERIOD: f32 = 240.0;

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
    // clip. Parallel axes are skipped when finding the plot exit. The direction is taken from
    // the UNSNAPPED points, because a one-pixel round over a long reach visibly tilts the line.
    // The far end is not snapped at
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
    let unit = normalize(d + vec2<f32>(1e-6, 0.0));
    // The aim gives direction only: a day of pixels must not lengthen the quad at narrow zooms.
    // Expand by half the stroke so a thick ray grazing an edge keeps its visible span.
    // Use the first forward exit, even for origins far outside the plot, plus one pixel.
    let half_width = max(s.m.x, 1.0) * 0.5;
    let lo = cv.bounds.xy - vec2<f32>(half_width);
    let hi = cv.bounds.xy + cv.bounds.zw + vec2<f32>(half_width);
    let edge = select(lo, hi, unit > vec2<f32>(0.0));
    var exit = vec2<f32>(1e30, 1e30);
    if (unit.x != 0.0) { exit.x = (edge.x - a.x) / unit.x; }
    if (unit.y != 0.0) { exit.y = (edge.y - a.y) / unit.y; }
    let reach = max(min(exit.x, exit.y), 0.0) + 1.0;
    var b = select(b_snapped, a + unit * reach, ray);
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
    // Clip BOTH ends to the render target, grown by the stroke and a margin that scales with the
    // coordinates' f32 error. Kept identical to the DX11 copy (order_lines.hlsl), which says why:
    // a far-off origin made a quad long enough to drop out of whole frames.
    let reach_err = (max(abs(a.x), abs(a.y)) + max(abs(b.x), abs(b.y))) * 1e-6;
    let margin = max(s.m.x, 1.0) * 0.5 + 2.0 + reach_err;
    let full = b - a;
    var vis = seg_clip_axis(a.x, full.x, -margin, cv.resolution.x + margin, vec2<f32>(0.0, 1.0));
    vis = seg_clip_axis(a.y, full.y, -margin, cv.resolution.y + margin, vis);
    var dist0 = 0.0;
    if (vis.y < vis.x) {
        b = a; // wholly outside: a zero-area quad
    } else {
        dist0 = (length(full) * vis.x) % SEG_PATTERN_PERIOD;
        // Only a clipped end moves (see order_lines.hlsl).
        if (vis.y < 1.0) { b = a + full * vis.y; }
        if (vis.x > 0.0) { a = a + full * vis.x; }
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
    out.dist = dist0 + len * along[vid];
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
