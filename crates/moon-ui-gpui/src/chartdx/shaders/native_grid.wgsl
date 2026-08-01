struct GridParams {
    bounds: vec4<f32>,
    resolution: vec2<f32>,
    n_vert: f32,
    n_horiz: f32,
    pad0: f32,
    pad1: f32,
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

const GRID_LINE_HALF_PX: f32 = 0.5;

struct GridOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn grid_vertex(@builtin(vertex_index) vid: u32) -> GridOut {
    let c = CORNERS_01[vid];
    let px = gp.bounds.xy + c * gp.bounds.zw;
    var out: GridOut;
    out.pos = to_clip(px, gp.resolution);
    return out;
}

@fragment
fn grid_fragment(in: GridOut) -> @location(0) vec4<f32> {
    let bg = gp.bg.rgb;
    let grid_col = mix(bg, gp.grid_col.rgb, clamp(gp.grid_alpha, 0.0, 1.0));
    var hit = false;
    // Fragment-stage position is rasterizer-generated, so both quad triangles use identical
    // pixel coordinates at their diagonal seam.
    let fragment_px = in.pos.xy;
    let step_x = gp.bounds.z / max(gp.n_vert, 1.0);
    let local_x = fragment_px.x - gp.bounds.x;
    let line_x = gp.bounds.x + round(local_x / step_x) * step_x;
    let snapped_x = floor(line_x) + 0.5;
    if abs(fragment_px.x - snapped_x) < GRID_LINE_HALF_PX {
        hit = true;
    }
    let step_y = gp.bounds.w / max(gp.n_horiz, 1.0);
    let local_y = fragment_px.y - gp.bounds.y;
    let line_y = gp.bounds.y + round(local_y / step_y) * step_y;
    let snapped_y = floor(line_y) + 0.5;
    if abs(fragment_px.y - snapped_y) < GRID_LINE_HALF_PX {
        hit = true;
    }
    let alpha = select(clamp(gp.bg_alpha, 0.0, 1.0), 1.0, hit);
    return vec4<f32>(select(bg, grid_col, hit), alpha);
}
