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

struct Cross {
    time_rel: f32,
    price: f32,
    side: u32,
    qty: f32,
};

const CORNERS_PM: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
    vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0)
);
const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<uniform> cv: ChartView;
@group(0) @binding(1) var<storage, read> crosses: array<Cross>;

struct CrossOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) side: u32,
};

@vertex
fn crosses_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> CrossOut {
    let c = crosses[iid];
    var sx = cv.bounds.x + (c.time_rel - cv.view_time0) * cv.time_to_px;
    var sy = cv.bounds.y + cv.bounds.w - (c.price - cv.view_price0) * cv.price_to_px;
    sx = round(sx);
    sy = round(sy);
    var out: CrossOut;
    let cull_margin = max(8.0, cv.marker_half + 1.0);
    if sx < cv.bounds.x - cull_margin || sx > cv.bounds.x + cv.bounds.z + cull_margin ||
       sy < cv.bounds.y - cull_margin || sy > cv.bounds.y + cv.bounds.w + cull_margin {
        out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.uv = vec2<f32>(0.0, 0.0);
        out.side = 0u;
        return out;
    }
    let corner = CORNERS_PM[vid];
    let px = vec2<f32>(sx, sy) + corner * cv.marker_half;
    out.pos = to_clip(px, cv.resolution);
    out.uv = corner;
    out.side = c.side;
    return out;
}

@fragment
fn crosses_fragment(in: CrossOut) -> @location(0) vec4<f32> {
    let col = clamp(i32(floor((in.uv.x * 0.5 + 0.5) * 7.0)), 0, 6);
    let row = clamp(i32(floor((in.uv.y * 0.5 + 0.5) * 7.0)), 0, 6);
    var mask: u32;
    if row == 0 || row == 6 {
        mask = 0x77u;
    } else if row == 1 || row == 5 {
        mask = 0x7Fu;
    } else {
        mask = 0x3Eu;
    }
    if ((mask >> u32(col)) & 1u) == 0u {
        discard;
    }
    let buy = vec3<f32>(0.18431, 0.65882, 0.36078);
    let sell = vec3<f32>(1.0, 0.55686, 0.35294);
    return vec4<f32>(select(buy, sell, in.side != 0u), 1.0);
}

struct VolumeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) side: u32,
};

@vertex
fn volume_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> VolumeOut {
    let c = crosses[iid];
    var out: VolumeOut;
    let sx = cv.bounds.x + (c.time_rel - cv.view_time0) * cv.time_to_px;
    if sx < cv.bounds.x - 2.0 || sx > cv.bounds.x + cv.bounds.z + 2.0 || c.qty <= 0.0 {
        out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.side = 0u;
        return out;
    }
    let inv = select(cv.volume_buy_inv, cv.volume_sell_inv, c.side != 0u);
    let norm = clamp(c.qty * inv, 0.0, 1.0);
    let band_h = min(cv.bounds.w * 0.18, 72.0);
    let h = max(1.0, sqrt(norm) * band_h);
    let base = cv.bounds.y + cv.bounds.w - 1.0;
    let bar_w = clamp(cv.time_to_px * 0.35, 1.0, 3.0);
    let corner = CORNERS_01[vid];
    let px = vec2<f32>(round(sx) - bar_w * 0.5, base - h) + corner * vec2<f32>(bar_w, h);
    out.pos = to_clip(px, cv.resolution);
    out.side = c.side;
    return out;
}

@fragment
fn volume_fragment(in: VolumeOut) -> @location(0) vec4<f32> {
    let buy = vec3<f32>(0.18431, 0.65882, 0.36078);
    let sell = vec3<f32>(1.0, 0.55686, 0.35294);
    return vec4<f32>(select(buy, sell, in.side != 0u), clamp(cv.volume_alpha, 0.0, 1.0));
}
