// Свечи own-pass: инстансный слой под крестами трейдов (base-проход, до combo-блита).
// Один инстанс = одна свеча: тело (quad) + верхний/нижний фитили (тонкие quad'ы), 18
// вершин. Режимы (заполненные / контуры / контуры в зоне трейдов), зона, тени и
// нейтральный цвет — константами CandleStyle: смена вида не трогает вершинный буфер.
// Контур — PS-дискард внутренности по попиксельной дистанции до края (размер quad'а
// приходит из VS). Цвета sRGB напрямую (таргет UNORM, как grid.hlsl).

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w, h (px) — область чарта
    float2 cv_resolution; // w, h бэкбуфера (px)
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

cbuffer CandleStyle : register(b1) {
    float4 cs_up;      // цвет растущей свечи
    float4 cs_down;    // цвет падающей
    float4 cs_neutral; // нейтральный цвет зоны трейдов
    float  cs_tf_rel;          // ширина бакета (rel ms)
    float  cs_zone_start;      // rel ms начала зоны трейдов (f32::MAX = зоны нет)
    float  cs_mode;            // 0 заполненные / 1 контуры / 2 контуры в зоне
    float  cs_outline_px;      // толщина контура, физ. px
    float  cs_wicks_in_zone;   // 0/1 — рисовать фитили в зоне
    float  cs_neutral_in_zone; // 0/1 — нейтральный цвет в зоне
    float  cs_fill_alpha;      // непрозрачность заливки тела
    float  cs_hide_start;      // rel ms: свечи с t_open ≥ границы не рисуем (только трейды)
};

struct Candle {
    float t_open; // rel ms открытия бакета
    float o;
    float h;
    float l;
    float c;
    float vol;
    float tf_rel; // СВОЙ ТФ свечи (rel ms); 0 = ТФ серии. Хвост истории — старшие ТФ.
};

StructuredBuffer<Candle> candles : register(t3);

struct CandleOut {
    float4 pos : SV_Position;
    float2 uv  : TEXCOORD0; // 0..1 внутри quad'а
    nointerpolation float2 size_px : TEXCOORD1; // размер quad'а, px (для контура)
    nointerpolation float  outline : TEXCOORD2; // 1 = рисуем контуром (только тело)
    nointerpolation float4 color   : TEXCOORD3;
};

static const float2 CORNERS[6] = {
    float2(0, 0), float2(1, 0), float2(0, 1),
    float2(0, 1), float2(1, 0), float2(1, 1)
};

float price_y(float price) {
    return cv_bounds.y + cv_bounds.w - (price - cv_view_price0) * cv_price_to_px;
}

CandleOut cull_out() {
    CandleOut o;
    o.pos = float4(2.0, 2.0, 0.0, 1.0);
    o.uv = float2(0.0, 0.0);
    o.size_px = float2(1.0, 1.0);
    o.outline = 0.0;
    o.color = float4(0.0, 0.0, 0.0, 0.0);
    return o;
}

CandleOut candles_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Candle cd = candles[iid];
    uint part = vid / 6u;          // 0 тело, 1 верхний фитиль, 2 нижний фитиль
    float2 corner = CORNERS[vid % 6u];

    if (cd.t_open >= cs_hide_start) {
        return cull_out(); // зона «только трейды» — свечу не рисуем
    }
    // Хвост истории дорисован старшими ТФ: у таких свечей свой tf_rel (ширина) и
    // приглушённые цвета — визуально отличаются от выбранного ТФ.
    bool foreign_tf = cd.tf_rel > 0.0 && abs(cd.tf_rel - cs_tf_rel) > 0.5;
    float tf_rel = (cd.tf_rel > 0.0) ? cd.tf_rel : cs_tf_rel;
    float x0 = cv_bounds.x + (cd.t_open - cv_view_time0) * cv_time_to_px;
    float x1 = x0 + tf_rel * cv_time_to_px;
    if (x1 < cv_bounds.x - 2.0 || x0 > cv_bounds.x + cv_bounds.z + 2.0) {
        return cull_out();
    }
    x0 = round(x0);
    x1 = max(round(x1), x0 + 1.0);

    bool in_zone = cd.t_open >= cs_zone_start;
    bool outline = (cs_mode >= 0.5 && cs_mode < 1.5) || (cs_mode >= 1.5 && in_zone);
    if (part != 0u && in_zone && cs_wicks_in_zone < 0.5) {
        return cull_out();
    }

    float y_top_body = min(price_y(cd.o), price_y(cd.c));
    float y_bot_body = max(price_y(cd.o), price_y(cd.c));
    y_top_body = round(y_top_body);
    y_bot_body = max(round(y_bot_body), y_top_body + 1.0);

    // Тело с зазором между свечами (10% ширины, максимум 4px); вырожденное узкое —
    // колонка минимум 1px по центру бакета.
    float gap = clamp((x1 - x0) * 0.10, 0.0, 4.0);
    float bx0 = x0 + gap;
    float bx1 = x1 - gap;
    if (bx1 - bx0 < 1.0) {
        float cx = floor((x0 + x1) * 0.5);
        bx0 = cx;
        bx1 = cx + 1.0;
    }

    float2 p0; // левый-верхний угол quad'а
    float2 sz;
    if (part == 0u) {
        p0 = float2(bx0, y_top_body);
        sz = float2(bx1 - bx0, y_bot_body - y_top_body);
    } else {
        float wick_w = max(1.0, min(cs_outline_px, (bx1 - bx0)));
        float wx = floor((x0 + x1) * 0.5 - wick_w * 0.5);
        float y0;
        float y1;
        if (part == 1u) {
            y0 = round(price_y(cd.h));
            y1 = y_top_body;
        } else {
            y0 = y_bot_body;
            y1 = round(price_y(cd.l));
        }
        if (y1 - y0 < 0.5) {
            return cull_out(); // фитиля нет (high/low внутри тела)
        }
        p0 = float2(wx, y0);
        sz = float2(wick_w, y1 - y0);
    }

    float2 px = p0 + corner * sz;
    float2 ndc = float2(px.x / cv_resolution.x * 2.0 - 1.0,
                        1.0 - px.y / cv_resolution.y * 2.0);

    float4 base = (cd.c >= cd.o) ? cs_up : cs_down;
    if (in_zone && cs_neutral_in_zone > 0.5) {
        base = cs_neutral;
    }
    float alpha = 1.0;
    if (part == 0u && !outline) {
        alpha = saturate(cs_fill_alpha); // заливка тела полупрозрачна (сетка чуть видна)
    }
    if (foreign_tf) {
        alpha *= 0.55; // хвост чужого ТФ — полупрозрачный
    }

    CandleOut o;
    o.pos = float4(ndc, 0.0, 1.0);
    o.uv = corner;
    o.size_px = sz;
    o.outline = (part == 0u && outline) ? 1.0 : 0.0;
    o.color = float4(base.rgb, alpha);
    return o;
}

float4 candles_fragment(CandleOut i) : SV_Target {
    if (i.outline > 0.5) {
        float dx = min(i.uv.x, 1.0 - i.uv.x) * i.size_px.x;
        float dy = min(i.uv.y, 1.0 - i.uv.y) * i.size_px.y;
        if (min(dx, dy) > max(cs_outline_px, 1.0)) {
            discard;
        }
        // Альфа из VS: у контуров обычно 1.0, у хвоста чужого ТФ — приглушённая.
        return i.color;
    }
    return i.color;
}
