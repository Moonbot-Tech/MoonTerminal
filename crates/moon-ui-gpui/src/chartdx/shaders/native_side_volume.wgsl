// Sides volume band (wgpu): mirrors side_volume.hlsl. One instance = one bucket, twelve vertices —
// the buy column (0-5) then the sell column (6-11) over it. Kinds: vs.m3.x 1 overlaid / 2 stacked.
// Heights are LINEAR in the value; the two reference lines sit at the maximum and at vs.m.w.

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
    m2: vec4<f32>, // x unused, y unused here, z line px
    m3: vec4<f32>, // x sides switch (0 off / 1 overlaid / 2 stacked), y split boundary rel ms, z interval rel ms
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
    let band = side_band_h();
    var frac = clamp(vs.m.w, 0.0, 1.0);
    if iid == 0u {
        frac = 1.0;
    }
    let base = cv.bounds.y + cv.bounds.w - 1.0;
    let y = round(base - band * frac);
    let th = max(vs.m2.z, 1.0);
    let corner = CORNERS_01[vid % 6u];
    let px = vec2<f32>(cv.bounds.x, y) + corner * vec2<f32>(cv.bounds.z, th);
    o.pos = to_clip(px, cv.resolution);
    return o;
}

@fragment
fn side_scale_fragment(_in: SideScaleOut) -> @location(0) vec4<f32> {
    return vs.scale;
}
