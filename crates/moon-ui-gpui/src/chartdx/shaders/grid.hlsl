// Сетка чарта own-pass: СТАТИЧНЫЕ вертикали (фикс. X-деления) + горизонтали по цене.
// Процедурно в одном fullscreen-проходе над chart_area (1 drawcall, без instance-буфера).
// Вертикали — фикс. пиксельные доли ширины (модель MoonBot: НЕ привязаны к круглым меткам
// времени, едут только подписи). Горизонтали — на кратных `price_interval` (совпадают с
// подписями цены). Рисуется ПОД крестами/данными в нашем own-pass.

cbuffer GridParams : register(b0) {
    float4 g_bounds;       // ox, oy, w, h — chart_area (px окна)
    float2 g_resolution;   // w, h backbuffer (px)
    float  g_n_vert;       // число вертикальных делений (фикс.)
    float  g_price_to_px;  // пикселей на единицу цены
    float  g_view_price0;  // цена у НИЗА bounds
    float  g_price_interval; // шаг цены для горизонталей (== nice_interval подписей)
    float  g_grid_alpha;   // видимость сетки 0..1 (тема)
    float  g_bg_alpha;     // 1 = grid сам красит фон, 0 = фон уже нарисован Background-слоем
    float4 g_bg;           // фон чарта (sRGB)
    float4 g_grid_col;     // цвет линий (sRGB)
};

struct GridOut {
    float4 pos : SV_Position;
    float2 px  : TEXCOORD0; // экранные пиксели
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
    o.px = p;
    return o;
}

float4 grid_fragment(GridOut i) : SV_Target {
    // База чарта: фон (НЕПРОЗРАЧНО) + линии сетки поверх. Это нижний слой own-pass;
    // combo-битмап крестов идёт ПРОЗРАЧНЫМ поверх (фон не дублируем) → сетка видна между крестами.
    // (Когда добавим фото-подложку (Background-слой) — фон будет от неё, тут оставим только линии.)
    // Таргет backbuffer = B8G8R8A8_UNORM (НЕ sRGB): GPUI и кресты пишут sRGB-значения
    // НАПРЯМУЮ (GPU не кодирует linear→sRGB). Пишем так же — БЕЗ конверсии в linear, иначе
    // #131416 раздавливается в почти чёрный и плот темнее панелей. Линии = смесь bg↔grid_col.
    float3 bg = g_bg.rgb;
    float3 grid_col = lerp(bg, g_grid_col.rgb, saturate(g_grid_alpha));
    bool hit = false; // NB: `line` — зарезервированное слово HLSL (geometry primitive), нельзя

    // Вертикали: фикс. деления ширины (статичны — НЕ зависят от времени).
    float step_x = g_bounds.z / max(g_n_vert, 1.0);
    float local_x = i.px.x - g_bounds.x;
    if (abs(local_x - round(local_x / step_x) * step_x) < GRID_LINE_HALF_PX) {
        hit = true;
    }

    // Горизонтали: на кратных price_interval (совпадают с подписями цены). Цена в пикселе:
    // price = view_price0 + (низ bounds - y)/price_to_px.
    if (g_price_interval > 1e-12 && g_price_to_px > 1e-9) {
        float price = g_view_price0 + (g_bounds.y + g_bounds.w - i.px.y) / g_price_to_px;
        float k = price / g_price_interval;
        if (abs(k - round(k)) * g_price_interval * g_price_to_px < GRID_LINE_HALF_PX) {
            hit = true;
        }
    }

    float alpha = hit ? 1.0 : saturate(g_bg_alpha);
    return float4(hit ? grid_col : bg, alpha);
}
