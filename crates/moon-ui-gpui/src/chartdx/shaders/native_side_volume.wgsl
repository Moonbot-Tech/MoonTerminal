// Sides volume band (wgpu): mirrors side_volume.hlsl. One instance = one bucket, twelve vertices —
// the buy column (0-5) then the sell column (6-11) over it. Kinds: vs.m3.x 1 overlaid / 2 stacked.
// Heights are LINEAR in the value; the scale bracket ticks at the maximum and at vs.m.w.

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

struct VolumeStyle {
    up: vec4<f32>,
    down: vec4<f32>,
    scale: vec4<f32>,
    m: vec4<f32>,  // x candle-band style, y height fraction, z 1/max, w half-line ratio
    m2: vec4<f32>, // x unused, y unused here, z bracket line px, w bracket stem signed inset px
    m3: vec4<f32>, // x sides switch (0 off / 1 overlaid / 2 stacked), y split boundary rel ms, z interval rel ms, w bracket tick px
};

struct SideBucket {
    t_open: f32,
    tf_rel: f32,
    buy: f32,
    sell: f32,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<uniform> vs: VolumeStyle;
@group(0) @binding(2) var<storage, read> buckets: array<SideBucket>;

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

struct SideOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) sell: u32,
};

fn side_band_h() -> f32 {
    return cv.bounds.w * vs.m.y;
}

fn side_height_px(value: f32) -> f32 {
    return clamp(value * vs.m.z, 0.0, 1.0) * side_band_h();
}

fn side_cull() -> SideOut {
    var o: SideOut;
    o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    o.sell = 0u;
    return o;
}

@vertex
fn side_band_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> SideOut {
    if vs.m3.x < 0.5 {
        return side_cull();
    }
    let b = buckets[iid];
    var sell = 0u;
    if vid >= 6u {
        sell = 1u;
    }
    var value = b.buy;
    if sell == 1u {
        value = b.sell;
    }
    if value <= 0.0 {
        return side_cull();
    }
    let x0 = cv.bounds.x + (b.t_open - cv.view_time0) * cv.time_to_px;
    let w = max(b.tf_rel * cv.time_to_px, 1.0);
    if x0 > cv.bounds.x + cv.bounds.z || x0 + w < cv.bounds.x {
        return side_cull();
    }
    let base = cv.bounds.y + cv.bounds.w - 1.0;
    let h = side_height_px(value);
    var lift = 0.0;
    if sell == 1u && vs.m3.x >= 1.5 {
        lift = side_height_px(b.buy);
    }
    let corner = CORNERS_01[vid % 6u];
    // Samples abut: from this sample's rounded left edge to the next one's, no seams, no gaps.
    let x1 = round(x0 + w);
    let p0 = vec2<f32>(round(x0), base - lift - h);
    let sz = vec2<f32>(max(x1 - round(x0), 1.0), h);
    var px = p0 + corner * sz;
    px.x = clamp(px.x, cv.bounds.x, cv.bounds.x + cv.bounds.z);
    var o: SideOut;
    o.pos = to_clip(px, cv.resolution);
    o.sell = sell;
    return o;
}

@fragment
fn side_band_fragment(i: SideOut) -> @location(0) vec4<f32> {
    if i.sell == 1u {
        return vs.down;
    }
    return vs.up;
}

// The scale is Moonbot's BRACKET, not a pair of full-width lines: a stem from the band floor up to
// the visible maximum, and three ticks to its right — at the maximum, at the second reference level
// and on the floor. Instance 0 is the stem, 1..3 the ticks top-down (VOLUME_SCALE_INSTANCES). Where
// the stem stands is vs.m2.w, the signed inset in physical px — from the plot's left edge when
// non-negative, from its right edge when negative — the very rule the text pass places the labels
// from (`moon_chart::volume_bars::scale_bracket_offset`); vs.m3.w is the tick length.
fn scale_bracket_quad(vid: u32, iid: u32, band: f32, second_frac: f32) -> vec4<f32> {
    let base = cv.bounds.y + cv.bounds.w - 1.0;
    let th = max(vs.m2.z, 1.0);
    let tick = max(vs.m3.w, th);
    var off = vs.m2.w;
    if off < 0.0 {
        off = cv.bounds.z + vs.m2.w;
    }
    let bx = cv.bounds.x + clamp(off, 0.0, cv.bounds.z);
    let top = round(base - band);
    var origin: vec2<f32>;
    var size: vec2<f32>;
    if iid == 0u {
        origin = vec2<f32>(bx, top);
        size = vec2<f32>(th, base - top + 1.0);
    } else {
        var frac = 0.0;
        if iid == 1u {
            frac = 1.0;
        } else if iid == 2u {
            frac = second_frac;
        }
        let y = min(round(base - band * frac), base - th + 1.0);
        origin = vec2<f32>(bx, y);
        size = vec2<f32>(tick, th);
    }
    let corner = CORNERS_01[vid % 6u];
    let px = origin + corner * size;
    return to_clip(px, cv.resolution);
}

struct SideScaleOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn side_scale_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> SideScaleOut {
    var o: SideScaleOut;
    if vs.m3.x < 0.5 {
        o.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        return o;
    }
    o.pos = scale_bracket_quad(vid, iid, side_band_h(), clamp(vs.m.w, 0.0, 1.0));
    return o;
}

@fragment
fn side_scale_fragment(_in: SideScaleOut) -> @location(0) vec4<f32> {
    return vs.scale;
}
