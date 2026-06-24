struct CursorParams {
    bounds: vec4<f32>,
    resolution: vec2<f32>,
    cursor: vec2<f32>,
    color: vec4<f32>,
    thickness: f32,
    enabled: f32,
    _pad: vec2<f32>,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<uniform> cp: CursorParams;

struct CursorOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn cursor_vertex(@builtin(vertex_index) vid: u32) -> CursorOut {
    let which = vid / 6u;
    let corner = CORNERS_01[vid - which * 6u];
    let thickness = max(cp.thickness, 1.0);
    let half_t = thickness * 0.5;
    let right = cp.bounds.x + cp.bounds.z;
    let bottom = cp.bounds.y + cp.bounds.w;
    let vertical_ok = cp.enabled > 0.5 && cp.cursor.x >= cp.bounds.x && cp.cursor.x <= right;
    let horizontal_ok = cp.enabled > 0.5 && cp.cursor.y >= cp.bounds.y && cp.cursor.y <= bottom;

    var dst: vec4<f32>;
    if which == 0u {
        dst = vec4<f32>(round(cp.cursor.x) - half_t, cp.bounds.y, thickness, cp.bounds.w);
        if !vertical_ok {
            dst = vec4<f32>(-10000.0, -10000.0, 1.0, 1.0);
        }
    } else {
        dst = vec4<f32>(cp.bounds.x, round(cp.cursor.y) - half_t, cp.bounds.z, thickness);
        if !horizontal_ok {
            dst = vec4<f32>(-10000.0, -10000.0, 1.0, 1.0);
        }
    }

    let px = dst.xy + corner * dst.zw;
    var out: CursorOut;
    out.pos = to_clip(px, cp.resolution);
    out.color = cp.color;
    return out;
}

@fragment
fn cursor_fragment(in: CursorOut) -> @location(0) vec4<f32> {
    return in.color;
}
