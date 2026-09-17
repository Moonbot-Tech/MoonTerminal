// Horizontal volumes (wgpu): mirrors hvol.hlsl. One instance = one row, twelve vertices — the
// bought length (0-5), then the sold one (6-11) over it, as the sides band draws its columns.
// Kinds: hs.m.y 0 overlaid / 1 stacked (sold continues from where bought ends). Rows are placed
// against the PANE view's price mapping and grow from the zone's right edge, the plot side,
// leftward — or, laid over the plot (hs.m.x >= 0.5), from its left edge rightward, with no
// backdrop. A backdrop pass draws the zone's fill and frame before the rows.

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

struct HvolStyle {
    zone: vec4<f32>,    // zone x, y, w, h (px)
    buy: vec4<f32>,     // bought rgb + opacity
    sell: vec4<f32>,    // sold rgb + opacity
    bg: vec4<f32>,      // backdrop rgb + opacity
    border: vec4<f32>,  // border rgb + opacity
    m: vec4<f32>,       // x over the plot (1) / carved out (0), y stacked (1) / overlaid (0), z 1/max, w border px
};

struct HvolRow {
    price_lo: f32,
    price_hi: f32,
    buy: f32,
    sell: f32,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<uniform> hs: HvolStyle;
@group(0) @binding(2) var<storage, read> rows: array<HvolRow>;

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

struct HvolOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) sell: u32,
};

fn hvol_cull() -> HvolOut {
    var o: HvolOut;
    o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    o.sell = 0u;
    return o;
}

// Row length in px, LINEAR in the value.
fn hvol_len_px(value: f32) -> f32 {
    return clamp(value * hs.m.z, 0.0, 1.0) * hs.zone.z;
}

@vertex
fn hvol_row_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> HvolOut {
    if hs.zone.z < 1.0 {
        return hvol_cull();
    }
    let r = rows[iid];
    var sell = 0u;
    if vid >= 6u {
        sell = 1u;
    }
    var value = r.buy;
    if sell == 1u {
        value = r.sell;
    }
    let len = hvol_len_px(value);
    // Stacked: the sold length starts where the bought one ends.
    var start = 0.0;
    if sell == 1u && hs.m.y >= 0.5 {
        start = hvol_len_px(r.buy);
    }
    if len <= 0.0 {
        return hvol_cull();
    }
    // Whole pixels along the price axis, each edge rounded — never expanded outward, which
    // would blend the translucent fills twice at every seam; see hvol.hlsl.
    let base = cv.bounds.y + cv.bounds.w;
    let y_top = base - (r.price_hi - cv.view_price0) * cv.price_to_px;
    let y_bot = base - (r.price_lo - cv.view_price0) * cv.price_to_px;
    let top = round(min(y_top, y_bot));
    let bot = max(round(max(y_top, y_bot)), top + 1.0);
    if bot < hs.zone.y || top > hs.zone.y + hs.zone.w {
        return hvol_cull();
    }
    // Carved out: from the zone's right edge, the plot side, leftward. Over the plot: from its
    // left edge rightward. At least one pixel of colour either way.
    var x0: f32;
    var x1: f32;
    if hs.m.x >= 0.5 {
        let left = hs.zone.x;
        x0 = round(left + start);
        x1 = max(round(x0 + len), x0 + 1.0);
    } else {
        let right = hs.zone.x + hs.zone.z;
        x1 = round(right - start);
        x0 = min(round(x1 - len), x1 - 1.0);
    }
    let corner = CORNERS_01[vid % 6u];
    var px = vec2<f32>(x0, top) + corner * vec2<f32>(x1 - x0, bot - top);
    px.x = clamp(px.x, hs.zone.x, hs.zone.x + hs.zone.z);
    var o: HvolOut;
    o.pos = to_clip(px, cv.resolution);
    o.sell = sell;
    return o;
}

@fragment
fn hvol_row_fragment(i: HvolOut) -> @location(0) vec4<f32> {
    if i.sell == 1u {
        return hs.sell;
    }
    return hs.buy;
}

// The zone's backdrop (instance 0) and its border (instances 1..4: top, bottom, left, right).
struct HvolBgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) border: u32,
};

@vertex
fn hvol_bg_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> HvolBgOut {
    var o: HvolBgOut;
    // No zone, or a zone laid over the plot, which has no backdrop or frame of its own.
    if hs.zone.z < 1.0 || hs.m.x >= 0.5 {
        o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        o.border = 0u;
        return o;
    }
    let th = max(hs.m.w, 1.0);
    var origin = hs.zone.xy;
    var size = hs.zone.zw;
    if iid == 1u {
        size = vec2<f32>(hs.zone.z, th);
    } else if iid == 2u {
        origin = vec2<f32>(hs.zone.x, hs.zone.y + hs.zone.w - th);
        size = vec2<f32>(hs.zone.z, th);
    } else if iid == 3u {
        size = vec2<f32>(th, hs.zone.w);
    } else if iid == 4u {
        origin = vec2<f32>(hs.zone.x + hs.zone.z - th, hs.zone.y);
        size = vec2<f32>(th, hs.zone.w);
    }
    let corner = CORNERS_01[vid % 6u];
    let px = origin + corner * size;
    o.pos = to_clip(px, cv.resolution);
    if iid == 0u {
        o.border = 0u;
    } else {
        o.border = 1u;
    }
    return o;
}

@fragment
fn hvol_bg_fragment(i: HvolBgOut) -> @location(0) vec4<f32> {
    if i.border == 1u {
        return hs.border;
    }
    return hs.bg;
}
