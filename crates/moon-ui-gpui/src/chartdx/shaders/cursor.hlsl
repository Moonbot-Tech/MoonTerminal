// Native chart cursor/crosshair overlay. It draws two axis-aligned quads in
// window device pixels, after chart data layers and before GPUI axes.

cbuffer CursorParams : register(b0) {
    float4 c_bounds;     // x, y, w, h of chart+book area
    float2 c_resolution; // backbuffer pixels
    float2 c_cursor;     // cursor in backbuffer pixels
    float4 c_color;      // sRGB + alpha
    float  c_thickness;  // px
    float  c_enabled;
    float2 c_pad;
};

struct CursorOut {
    float4 pos : SV_Position;
    float4 color : COLOR0;
};

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

CursorOut cursor_vertex(uint vid : SV_VertexID) {
    uint which = vid / 6;
    uint corner_id = vid - which * 6;
    float2 corner = CORNERS[corner_id];
    float half_t = max(c_thickness, 1.0) * 0.5;

    float right = c_bounds.x + c_bounds.z;
    float bottom = c_bounds.y + c_bounds.w;
    bool vertical_ok = c_enabled > 0.5 && c_cursor.x >= c_bounds.x && c_cursor.x <= right;
    bool horizontal_ok = c_enabled > 0.5 && c_cursor.y >= c_bounds.y && c_cursor.y <= bottom;

    float4 dst = (which == 0)
        ? float4(round(c_cursor.x) - half_t, c_bounds.y, max(c_thickness, 1.0), c_bounds.w)
        : float4(c_bounds.x, round(c_cursor.y) - half_t, c_bounds.z, max(c_thickness, 1.0));

    if ((which == 0 && !vertical_ok) || (which == 1 && !horizontal_ok)) {
        dst = float4(-10000.0, -10000.0, 1.0, 1.0);
    }

    float2 p = dst.xy + corner * dst.zw;
    float2 ndc = float2(p.x / c_resolution.x * 2.0 - 1.0,
                        1.0 - p.y / c_resolution.y * 2.0);
    CursorOut o;
    o.pos = float4(ndc, 0.0, 1.0);
    o.color = c_color;
    return o;
}

float4 cursor_fragment(CursorOut i) : SV_Target {
    return i.color;
}
