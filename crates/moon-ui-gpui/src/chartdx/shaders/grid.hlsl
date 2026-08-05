// Chart grid own pass: STATIC fixed X and Y screen divisions, generated procedurally in one
// fullscreen pass over chart_area (one draw call, no instance buffer).
// Vertical lines use fixed pixel fractions of the width; horizontal lines use fixed fractions
// of the height (Moonbot model). The grid is NOT tied to round time/price labels and does NOT
// move during zoom/pan; only labels move, showing non-round times/prices on fixed lines.
// Rendered BENEATH crosses/data.

cbuffer GridParams : register(b0) {
    float4 g_bounds;       // ox, oy, w, h — chart_area (window px)
    float2 g_resolution;   // w, h backbuffer (px)
    float  g_n_vert;       // fixed number of vertical width divisions
    float  g_n_horiz;      // fixed number of horizontal height divisions
    float  g_pad0;         // reserved (0)
    float  g_pad1;         // reserved (0)
    float  g_grid_alpha;   // grid visibility 0..1 (theme)
    float  g_bg_alpha;     // 1 = grid paints background, 0 = Background layer already painted it
    float4 g_bg;           // chart background (sRGB)
    float4 g_grid_col;     // line color (sRGB)
};

struct GridOut {
    float4 pos : SV_Position;
};

static const float GRID_LINE_HALF_PX = 0.5;

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

GridOut grid_vertex(uint vid : SV_VertexID) {
    float2 c = CORNERS[vid];
    float2 p = g_bounds.xy + c * g_bounds.zw;
    float2 ndc = float2(p.x / g_resolution.x * 2.0 - 1.0,
                        1.0 - p.y / g_resolution.y * 2.0);
    GridOut o;
    o.pos = float4(ndc, 0.0, 1.0);
    return o;
}

float4 grid_fragment(GridOut i) : SV_Target {
    // Chart base: OPAQUE background with grid lines above it. Whenever it supplies that background
    // (g_bg_alpha = 1, i.e. no photo backdrop) this pass ERASES whatever sits under it, which is
    // why the zone layer (order zones, figure fills) is drawn after it and not before; the combo
    // bitmap of crosses is TRANSPARENT above it (the background is not duplicated), so the grid
    // remains visible between crosses. With a photo backdrop (Background layer),
    // that layer supplies the background and this pass leaves only the lines.
    // Backbuffer target = B8G8R8A8_UNORM (NOT sRGB): GPUI and crosses write sRGB values DIRECTLY
    // because the GPU does not encode linear→sRGB. Do the same WITHOUT converting to linear;
    // otherwise #131416 is crushed almost to black and the plot is darker than the panels.
    // Lines are a blend of bg↔grid_col.
    float3 bg = g_bg.rgb;
    float3 grid_col = lerp(bg, g_grid_col.rgb, saturate(g_grid_alpha));
    bool hit = false; // NB: `line` is a reserved HLSL word (geometry primitive).
    // Fragment-stage SV_Position is rasterizer-generated. Using a vertex varying here makes the
    // two triangles disagree at their diagonal seam due to interpolation precision.
    float2 fragment_px = i.pos.xy;

    // Vertical lines: fixed width divisions (static and independent of time).
    float step_x = g_bounds.z / max(g_n_vert, 1.0);
    float local_x = fragment_px.x - g_bounds.x;
    float line_x = g_bounds.x + round(local_x / step_x) * step_x;
    float snapped_x = floor(line_x) + 0.5;
    if (abs(fragment_px.x - snapped_x) < GRID_LINE_HALF_PX) {
        hit = true;
    }

    // Horizontal lines: fixed height divisions (static and independent of price).
    float step_y = g_bounds.w / max(g_n_horiz, 1.0);
    float local_y = fragment_px.y - g_bounds.y;
    float line_y = g_bounds.y + round(local_y / step_y) * step_y;
    float snapped_y = floor(line_y) + 0.5;
    if (abs(fragment_px.y - snapped_y) < GRID_LINE_HALF_PX) {
        hit = true;
    }

    float alpha = hit ? 1.0 : saturate(g_bg_alpha);
    return float4(hit ? grid_col : bg, alpha);
}
