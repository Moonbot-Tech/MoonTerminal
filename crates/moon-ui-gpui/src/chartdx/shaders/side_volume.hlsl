// Sides half of the bottom band (`candle_volume_sides`): bought and sold turnover as ROLLING sums over the band's
// interval, sampled along the time axis and drawn as two filled series in the base pass, under
// the candle bodies and beside the candle half that `candles.hlsl` draws where the history does
// not reach.
// One instance is one sample and carries BOTH sides: vertices 0-5 are the buy column, 6-11 the
// sell column, so the sell column is always rasterised after the buy one and the overlaid kind
// blends deterministically. Neighbouring samples abut, which is what makes the series a hill.
//
// Kinds (`vs_m3.x`): 1 = overlaid — both columns rise from the band floor and the taller side
// shows above the other; 2 = stacked — the sell column sits on top of the buy column. Heights
// are LINEAR in the value, unlike the square-root the candle band uses: the scale labels read
// `max` at the top and `max / 2` half-way, which is only true of a proportional height.
//
// Shares the ChartView (b0) and VolumeStyle (b2) constant layouts with `candles.hlsl`: the Rust
// side uploads the very same structs to both pipelines. Write sRGB colours directly (UNORM
// target, as in grid.hlsl).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — chart area
    float2 cv_resolution; // backbuffer w, h (px)
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half;
    float  cv_instance_offset;
    float  cv_volume_buy_inv;
    float  cv_volume_sell_inv;
    float  cv_volume_alpha;
    float  cv_pad2;
};

cbuffer VolumeStyle : register(b2) {
    float4 vs_up;    // buy column rgb + band opacity
    float4 vs_down;  // sell column rgb + band opacity
    float4 vs_scale; // scale-bracket rgb + alpha
    float4 vs_m;     // x candle-band style, y height fraction, z 1/max, w half-line ratio
    float4 vs_m2;    // x unused, y unused here, z bracket line px, w bracket stem signed inset px
    float4 vs_m3;    // x sides switch (0 off / 1 overlaid / 2 stacked), y split boundary rel ms, z interval rel ms, w bracket tick px
};

struct SideBucket {
    float t_open; // bucket opening time in rel ms
    float tf_rel; // bucket width in rel ms
    float buy;    // bought over the bucket, quote currency
    float sell;   // sold over the bucket, quote currency
};

StructuredBuffer<SideBucket> buckets : register(t3);

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

struct SideOut {
    float4 pos : SV_Position;
    nointerpolation uint sell : TEXCOORD0;
};

float side_band_h() {
    return cv_bounds.w * vs_m.y;
}

// Linear, not sqrt: see the header.
float side_height_px(float value) {
    return saturate(value * vs_m.z) * side_band_h();
}

SideOut side_cull() {
    SideOut o;
    o.pos = float4(2.0, 2.0, 0.0, 1.0);
    o.sell = 0u;
    return o;
}

SideOut side_band_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    if (vs_m3.x < 0.5) {
        return side_cull(); // the switch is off
    }
    SideBucket b = buckets[iid];
    uint sell = (vid >= 6u) ? 1u : 0u;
    float value = sell ? b.sell : b.buy;
    if (value <= 0.0) {
        return side_cull();
    }
    float x0 = cv_bounds.x + (b.t_open - cv_view_time0) * cv_time_to_px;
    float w = max(b.tf_rel * cv_time_to_px, 1.0);
    // Cull off-plot buckets before any arithmetic on their corners.
    if (x0 > cv_bounds.x + cv_bounds.z || x0 + w < cv_bounds.x) {
        return side_cull();
    }
    float base = cv_bounds.y + cv_bounds.w - 1.0;
    float h = side_height_px(value);
    // Stacked: the sell column starts where the buy column ends.
    float lift = (sell && vs_m3.x >= 1.5) ? side_height_px(b.buy) : 0.0;
    float2 corner = CORNERS[vid % 6u];
    // Samples of a rolling sum abut: each spans exactly from its own rounded left edge to the
    // next sample's, so a filled series has no seams and no gaps at any zoom.
    float x1 = round(x0 + w);
    float2 p0 = float2(round(x0), base - lift - h);
    float2 sz = float2(max(x1 - round(x0), 1.0), h);
    float2 px = p0 + corner * sz;
    // Clamp into the plot: the base pass scissor reaches across the price gutter and the order
    // book, so an unclamped band spills into both.
    px.x = clamp(px.x, cv_bounds.x, cv_bounds.x + cv_bounds.z);
    SideOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0,
                   1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    o.sell = sell;
    return o;
}

float4 side_band_fragment(SideOut i) : SV_Target {
    return (i.sell == 1u) ? vs_down : vs_up;
}

// The scale is Moonbot's BRACKET, not a pair of full-width lines: a stem from the band floor up to
// the visible maximum, and three ticks to its right — at the maximum, at the second reference level
// and on the floor. Instance 0 is the stem, 1..3 the ticks top-down (VOLUME_SCALE_INSTANCES). Where
// the stem stands is vs_m2.w, the signed inset in physical px — from the plot's left edge when
// non-negative, from its right edge when negative — the very rule the text pass places the labels
// from (`moon_chart::volume_bars::scale_bracket_offset`); vs_m3.w is the tick length.
float4 scale_bracket_quad(uint vid, uint iid, float band, float second_frac) {
    float base = cv_bounds.y + cv_bounds.w - 1.0;
    float th = max(vs_m2.z, 1.0);
    float tick = max(vs_m3.w, th);
    float off = (vs_m2.w >= 0.0) ? vs_m2.w : cv_bounds.z + vs_m2.w;
    float bx = cv_bounds.x + clamp(off, 0.0, cv_bounds.z);
    float top = round(base - band);
    float2 origin;
    float2 size;
    if (iid == 0u) {
        origin = float2(bx, top);
        size = float2(th, base - top + 1.0);
    } else {
        float frac = (iid == 1u) ? 1.0 : ((iid == 2u) ? second_frac : 0.0);
        float y = min(round(base - band * frac), base - th + 1.0);
        origin = float2(bx, y);
        size = float2(tick, th);
    }
    float2 corner = CORNERS[vid % 6u];
    float2 px = origin + corner * size;
    return float4(px.x / cv_resolution.x * 2.0 - 1.0,
                  1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
}

// The sides band's scale bracket: its second tick sits at the half-way level the `max / 2` label
// names. Linear, so no square root here.
struct SideScaleOut {
    float4 pos : SV_Position;
};

SideScaleOut side_scale_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    SideScaleOut o;
    if (vs_m3.x < 0.5) {
        o.pos = float4(2.0, 2.0, 0.0, 1.0);
        return o;
    }
    o.pos = scale_bracket_quad(vid, iid, side_band_h(), saturate(vs_m.w));
    return o;
}

float4 side_scale_fragment(SideScaleOut i) : SV_Target {
    return vs_scale;
}
