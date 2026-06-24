struct BackgroundParams {
    dst: vec4<f32>,
    resolution: vec2<f32>,
    uv_off: vec2<f32>,
    uv_scale: vec2<f32>,
    opacity: f32,
    _pad: f32,
    bg: vec4<f32>,
};

const CORNERS_01: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0)
);

fn to_clip(px: vec2<f32>, resolution: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / resolution.x * 2.0 - 1.0, 1.0 - px.y / resolution.y * 2.0, 0.0, 1.0);
}

@group(0) @binding(0) var<uniform> bp: BackgroundParams;
@group(0) @binding(1) var bg_tex: texture_2d<f32>;
@group(0) @binding(2) var bg_samp: sampler;

struct BgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn background_vertex(@builtin(vertex_index) vid: u32) -> BgOut {
    let c = CORNERS_01[vid];
    let px = bp.dst.xy + c * bp.dst.zw;
    var out: BgOut;
    out.pos = to_clip(px, bp.resolution);
    out.uv = bp.uv_off + c * bp.uv_scale;
    return out;
}

@fragment
fn background_fragment(in: BgOut) -> @location(0) vec4<f32> {
    let photo = textureSample(bg_tex, bg_samp, in.uv).rgb;
    let rgb = mix(bp.bg.rgb, photo, clamp(bp.opacity, 0.0, 1.0));
    return vec4<f32>(rgb, 1.0);
}

@fragment
fn blit_fragment(in: BgOut) -> @location(0) vec4<f32> {
    return textureSample(bg_tex, bg_samp, in.uv);
}
