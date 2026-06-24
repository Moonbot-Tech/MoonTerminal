// Композит замороженного фон-битмапа чарта (combo) и блит надписей/подложки.
// Тяжёлая история (кресты+линии) запечена в offscreen-текстуру шириной W*1.2 (запас
// 20% справа под живой край). Каждый кадр — блит видимого окна текстуры на чарт-область
// backbuffer'а; пан = сдвиг UV (uv_off), без перерисовки. Point-семпл, 1:1.

cbuffer BlitParams : register(b0) {
    float4 bp_dst;        // ox, oy, w, h — целевая область в backbuffer px
    float2 bp_resolution; // w, h backbuffer px
    float2 bp_uv_off;     // u_left, v_top — левый-верхний угол видимого окна в текстуре (0..1)
    float2 bp_uv_scale;   // u_span, v_span — ширина/высота окна в UV
    float2 bp_pad;
};

Texture2D bp_tex : register(t0);
SamplerState bp_samp : register(s0);

struct BlitOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0;
};

// два треугольника (TRIANGLELIST), угол quad'а в [0,1]
static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

BlitOut blit_vertex(uint vid : SV_VertexID) {
    float2 c = CORNERS[vid];
    float2 px = bp_dst.xy + c * bp_dst.zw;
    float2 ndc = float2(px.x / bp_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / bp_resolution.y * 2.0);
    BlitOut o;
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = bp_uv_off + c * bp_uv_scale;
    return o;
}

float4 blit_fragment(BlitOut i) : SV_Target {
    return bp_tex.Sample(bp_samp, i.uv);
}

// OPAQUE-вариант для блита полной базы (base.rs). База — непрозрачный кадр всей сцены;
// блитить её надо как замену (alpha=1, blend off), иначе при alpha<1 сквозь неё
// блендится белый clear backbuffer'а (Opaque-окно форк чистит в [1,1,1,1]) → бледные
// вспышки панелей на каждый UI-present. Combo/orderbook этот fragment НЕ используют —
// им нужна прозрачность поверх фона.
float4 blit_opaque_fragment(BlitOut i) : SV_Target {
    return float4(bp_tex.Sample(bp_samp, i.uv).rgb, 1.0);
}
