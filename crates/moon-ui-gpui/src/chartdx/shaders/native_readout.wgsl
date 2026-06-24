struct ReadoutRect {
    dst: vec4<f32>,
    bg: vec4<f32>,
    border: vec4<f32>,
    m: vec4<f32>,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<storage, read> rects: array<ReadoutRect>;

struct RectOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dst: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) border: vec4<f32>,
    @location(4) border_width: f32,
};

@vertex
fn readout_rect_vertex(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> RectOut {
    let r = rects[iid];
    let c = CORNERS_01[vid];
    let px = r.dst.xy + c * r.dst.zw;
    var out: RectOut;
    out.pos = to_clip(px, r.m.yz);
    out.uv = c;
    out.dst = r.dst;
    out.bg = r.bg;
    out.border = r.border;
    out.border_width = max(r.m.x, 0.0);
    return out;
}

@fragment
fn readout_rect_fragment(in: RectOut) -> @location(0) vec4<f32> {
    let px = in.uv * in.dst.zw;
    let edge = min(min(px.x, in.dst.z - px.x), min(px.y, in.dst.w - px.y));
    if edge <= in.border_width {
        return in.border;
    }
    return in.bg;
}
