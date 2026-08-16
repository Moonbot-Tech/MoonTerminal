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

// shape: 0 = cross, 1 = filled knot, 3 = warning badge, 4 = trade arrow UP (a buy), 5 = trade arrow
// DOWN (a sell); the registry lives on `MARKER_SHAPE_CROSS` in moon-chart's layers/order_lines.rs,
// and the warning badge is dispatched OPEN-ENDED as `> 2.5`, so any id above 3 must be tested for
// before it. 2 = news gem (pos.z half height, pos.w half width, m.z/m.w the
// tag-colour wedge). m.y anchor: 0 = price, 1 = plot bottom (pos.y is physical px above the bottom
// edge, plus a horizontal clip to the plot). Keep in sync with order_lines.hlsl / chart_native.metal.
struct MOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) @interpolate(flat) shape: f32,
    @location(3) @interpolate(flat) thick: f32,
    @location(4) @interpolate(flat) sz: f32,
    @location(5) @interpolate(flat) xclip: vec2<f32>,
    @location(6) @interpolate(flat) wedge: vec2<f32>,
};

const GEM_FACET_GAP: f32 = 0.055;
const GEM_LEFT_SHADE: f32 = 0.78;
const GEM_TWO_PI: f32 = 6.28318531;
// Trade-arrow rim: width as a fraction of the glyph's half-height, and how far its colour is
// darkened. Keep in sync with order_lines.hlsl / chart_native.metal.
const ARROW_RIM_FRAC: f32 = 0.22;
const ARROW_RIM_SHADE: f32 = 0.18;
// How much a clustered arrow grows per doubling of its member count, and the doublings after which
// it stops growing. Capped because the count is unbounded — a thousand-trade cluster must still be
// an arrow, not a billboard; the exact number belongs to the hover card, not to the glyph.
const ARROW_CLUSTER_GROW: f32 = 0.25;
const ARROW_CLUSTER_GROW_CAP: f32 = 3.0;

@vertex
fn marker_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> MOut {
    let mk = markers[iid];
    var c = data_to_px(cv, mk.pos.x, mk.pos.y);
    let bottom = mk.m.y > 0.5;
    if bottom {
        c.y = cv.bounds.y + cv.bounds.w - mk.pos.y;
    }
    var half_sz = max(mk.pos.z, 1.0);
    var half_w = max(mk.pos.w, 1.0);
    if mk.m.x > 3.5 && mk.m.x < 5.5 {
        // A CLUSTER of trades draws as one arrow, grown by a capped log of its member count (m.z),
        // so a busy pixel column reads as "several" rather than hiding all but one of them.
        let grow = 1.0 + ARROW_CLUSTER_GROW * min(log2(max(mk.m.z, 1.0)), ARROW_CLUSTER_GROW_CAP);
        half_sz = half_sz * grow;
        half_w = half_w * grow;
        // A trade-history arrow hangs off its price: the APEX stays on the level and the body
        // extends away from it, clear of the candle. Derived from `shape` alone, so no instance
        // field pays for it, and applied in PIXELS so zoom leaves it untouched — a price-space
        // offset would have to be rebuilt on every view change, which is the one thing this own
        // pass exists to avoid. It uses the GROWN half height, or a cluster's apex would drift off
        // its own price as the cluster gets bigger.
        if mk.m.x < 4.5 {
            c.y = c.y + half_sz;
        } else {
            c.y = c.y - half_sz;
        }
    }
    let center = vec2<f32>(round(c.x), round(c.y));
    // The gem is taller than wide; every other marker keeps its square quad.
    var half_ext = vec2<f32>(half_sz, half_sz);
    if mk.m.x >= 1.5 {
        half_ext = vec2<f32>(half_w, half_sz);
    }
    let corner = CORNERS_PM_ALT[vid];
    let px = center + corner * half_ext;
    var out: MOut;
    out.pos = to_clip(px, cv.resolution);
    out.color = mk.color;
    out.local = corner * half_ext;
    out.shape = mk.m.x;
    out.thick = half_w;
    out.sz = half_sz;
    // Price-anchored markers keep their historical reach (order lines extend into the book zone).
    if bottom {
        out.xclip = vec2<f32>(cv.bounds.x, cv.bounds.x + cv.bounds.z);
    } else {
        out.xclip = vec2<f32>(-1e30, 1e30);
    }
    out.wedge = vec2<f32>(mk.m.z, max(mk.m.w, 1.0));
    return out;
}

@fragment
fn marker_fragment(in: MOut) -> @location(0) vec4<f32> {
    if in.pos.x < in.xclip.x || in.pos.x > in.xclip.y {
        discard;
    }
    if in.shape < 0.5 {
        let h = max(in.thick, 1.0) * 0.5;
        let d1 = abs(in.local.x - in.local.y) * 0.70710678;
        let d2 = abs(in.local.x + in.local.y) * 0.70710678;
        if min(d1, d2) > h {
            discard;
        }
        return in.color;
    }
    if in.shape < 1.5 {
        if length(in.local) > in.sz {
            discard;
        }
        return in.color;
    }
    if in.shape > 3.5 && in.shape < 5.5 {
        // Trade-history arrow: a solid triangle, apex UP for a buy and DOWN for a sell. Bounded on
        // BOTH sides deliberately — the warning badge below is an open-ended `> 2.5`, which is
        // exactly how these ids would otherwise have rendered as warning triangles.
        var dir = 1.0;
        if in.shape >= 4.5 {
            dir = -1.0;
        }
        let anx = in.local.x / max(in.thick, 1.0);
        let any = in.local.y / max(in.sz, 1.0) * dir;
        let inside = any - (2.0 * abs(anx) - 1.0);
        if inside < 0.0 {
            discard;
        }
        // A dark rim, because a trade arrow is drawn in the SAME palette as the candles: a green
        // buy landing on a green candle has no edge at all without one. Derived from the glyph's
        // own half-height so it holds its weight at any DPI instead of thinning out.
        let rim = max(1.0, in.sz * ARROW_RIM_FRAC) / max(in.sz, 1.0);
        if inside < rim || (1.0 - any) < rim {
            return vec4<f32>(in.color.rgb * ARROW_RIM_SHADE, in.color.a);
        }
        return in.color;
    }
    if in.shape > 2.5 {
        // Warning badge: an upward triangle (apex at top, base on the axis) with a dark
        // exclamation mark cut into it. local.y is +down, so the base sits at +sz.
        let tw = max(in.thick, 1.0);
        let nx = in.local.x / tw;
        let ny = in.local.y / max(in.sz, 1.0);
        if ny < 2.0 * abs(nx) - 1.0 {
            discard;
        }
        let bar = abs(nx) < 0.13 && ny > -0.30 && ny < 0.34;
        let dot = abs(nx) < 0.15 && ny > 0.50 && ny < 0.74;
        if bar || dot {
            return vec4<f32>(in.color.rgb * 0.14, in.color.a);
        }
        return in.color;
    }
    // News gem: a vertically elongated diamond, optionally cut into wedges by tag colour.
    let hw = max(in.thick, 1.0);
    if abs(in.local.x) / hw + abs(in.local.y) / max(in.sz, 1.0) > 1.0 {
        discard;
    }
    if in.wedge.y > 1.5 {
        // Wedge index runs clockwise from the top tip, so a two-colour gem splits left/right.
        var ang = atan2(in.local.x, -in.local.y);
        if ang < 0.0 {
            ang = ang + GEM_TWO_PI;
        }
        let f = ang / GEM_TWO_PI * in.wedge.y;
        let idx = floor(f);
        if abs(idx - in.wedge.x) > 0.5 {
            discard;
        }
        // Facet gaps only ABOVE the center: with an even wedge count one boundary lands exactly on
        // the bottom tip, and cutting there would lift the gem off the axis it marks.
        let frac = f - idx;
        if in.local.y < 0.0 && (frac < GEM_FACET_GAP || frac > 1.0 - GEM_FACET_GAP) {
            discard;
        }
    }
    var shade = 1.0;
    if in.local.x < 0.0 {
        shade = GEM_LEFT_SHADE;
    }
    return vec4<f32>(in.color.rgb * shade, in.color.a);
}
