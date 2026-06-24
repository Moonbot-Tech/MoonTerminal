struct GridParams {
    bounds: vec4<f32>,
    resolution: vec2<f32>,
    n_vert: f32,
    price_to_px: f32,
    view_price0: f32,
    price_interval: f32,
    grid_alpha: f32,
    bg_alpha: f32,
    bg: vec4<f32>,
    grid_col: vec4<f32>,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<uniform> gp: GridParams;

struct GridOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,
};

@vertex
fn grid_vertex(@builtin(vertex_index) vid: u32) -> GridOut {
    let c = CORNERS_01[vid];
    let px = gp.bounds.xy + c * gp.bounds.zw;
    var out: GridOut;
    out.pos = to_clip(px, gp.resolution);
    out.px = px;
    return out;
}

@fragment
fn grid_fragment(in: GridOut) -> @location(0) vec4<f32> {
    let bg = gp.bg.rgb;
    let grid_col = mix(bg, gp.grid_col.rgb, clamp(gp.grid_alpha, 0.0, 1.0));
    var hit = false;
    let step_x = gp.bounds.z / max(gp.n_vert, 1.0);
    let local_x = in.px.x - gp.bounds.x;
    if abs(local_x - round(local_x / step_x) * step_x) < 1.0 {
        hit = true;
    }
    if gp.price_interval > 1e-12 && gp.price_to_px > 1e-9 {
        let price = gp.view_price0 + (gp.bounds.y + gp.bounds.w - in.px.y) / gp.price_to_px;
        let k = price / gp.price_interval;
        if abs(k - round(k)) * gp.price_interval * gp.price_to_px < 1.0 {
            hit = true;
        }
    }
    let alpha = select(clamp(gp.bg_alpha, 0.0, 1.0), 1.0, hit);
    return vec4<f32>(select(bg, grid_col, hit), alpha);
}
