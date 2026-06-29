// Стакан (orderbook) own-pass: фон зоны + кумулятивные прямоугольники глубины
// + отдельные линии уровней поверх fill.
// Своя зона СПРАВА (НЕ временной ряд, без combo). Бары тянутся ВЛЕВО от правого края,
// длина = len_norm·zone; нормировка/геометрия считаются на CPU (book.build_instances).
// Порт moon-chart/shaders/glass.wgsl. cbuffer ChartView — тот же, что у крестов (b0),
// но viewport = зона стакана.

cbuffer ChartView : register(b0) {
    float4 cv_bounds;     // ox, oy, w(=ширина зоны стакана), h (px окна)
    float2 cv_resolution; // backbuffer w, h
    float  cv_time_to_px;
    float  cv_view_time0;
    float  cv_price_to_px;
    float  cv_view_price0;
    float  cv_marker_half;
    float  cv_pad;
};

// Цвета стакана (sRGB rgb + pad). Темовые; сейчас близко к крестам (bid green/ask red).
cbuffer BookStyle : register(b1) {
    float4 bs_book_bg; // фон зоны
    float4 bs_bid;     // bid rgb
    float4 bs_ask;     // ask rgb
    float4 bs_level;   // x = level-line opacity, y = level-line height px
};

struct Level {
    float price;
    float span;     // signed-delta цены до второго края fill-полосы
    float len_norm; // 0..1 доля ширины зоны
    float kind;     // 0 bid fill / 1 ask fill / 2 bid level / 3 ask level
};
StructuredBuffer<Level> levels : register(t1);

static const float2 CORNERS[6] = {
    float2(-1, -1), float2(1, -1), float2(-1, 1),
    float2(-1,  1), float2(1, -1), float2( 1, 1)
};

// Таргет = B8G8R8A8_UNORM (НЕ sRGB): пишем sRGB-значения НАПРЯМУЮ, как GPUI/кресты.
// Конверсии в linear НЕТ — иначе цвета раздавливаются в тёмное (см. grid.hlsl).

// ── Fill-прямоугольники и отдельные level-lines (instanced) ─────────────────
struct BarOut {
    float4 pos : SV_Position;
    nointerpolation float kind : TEXCOORD0;
};

BarOut bars_vertex(uint vid : SV_VertexID, uint iid : SV_InstanceID) {
    Level lv = levels[iid];
    float zone = cv_bounds.z;
    float right = cv_bounds.x + zone;
    float seg_len = max(lv.len_norm * zone, 1.0);
    float cx = right - seg_len * 0.5;

    float base = cv_bounds.y + cv_bounds.w;
    float y_price = base - (lv.price - cv_view_price0) * cv_price_to_px;
    // fill: один край — собственная цена уровня, второй — цена соседа (price+span). У соседней
    // полосы общий шов считается из ДРУГОГО f32-выражения (его price vs наш price+span); при
    // больших ценах (микро-токены) + зуме round() этих чуть разных f32 даёт РАЗНЫЙ пиксель →
    // 1px чёрная щель на шве. Поэтому расширяем каждую полосу до ЦЕЛЫХ пикселей (floor/ceil):
    // соседние полосы тогда перекрываются на стыке, шов исчезает (заливка непрозрачна — overlap
    // невиден). Порт moon-chart/shaders/glass.wgsl — там та же правка.
    float inner = lv.price + lv.span;
    float y_inner = base - (inner - cv_view_price0) * cv_price_to_px;
    float top = floor(min(y_price, y_inner));
    float bot = ceil(max(y_price, y_inner));
    if (bot - top < 1.0) {
        bot = top + 1.0;
    }
    float cy = (top + bot) * 0.5;
    float hh = bot - top;
    if (lv.kind >= 2.0) {
        cy = round(y_price);
        hh = max(bs_level.y, 1.0);
    }

    float2 corner = CORNERS[vid];
    float2 px = float2(cx + corner.x * seg_len * 0.5, cy + corner.y * hh * 0.5);
    BarOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0, 1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    o.kind = lv.kind;
    return o;
}

float4 bars_fragment(BarOut i) : SV_Target {
    if (i.kind < 0.5) {
        return float4(bs_bid.rgb, 1.0);
    } else if (i.kind < 1.5) {
        return float4(bs_ask.rgb, 1.0);
    } else if (i.kind < 2.5) {
        return float4(min(bs_bid.rgb * 1.25, 1.0.xxx), bs_level.x);
    }
    return float4(min(bs_ask.rgb * 1.25, 1.0.xxx), bs_level.x);
}

// ── Фон зоны стакана (fullscreen quad над зоной) ────────────────────────────
struct BgOut {
    float4 pos : SV_Position;
};

BgOut bg_vertex(uint vid : SV_VertexID) {
    float2 c = CORNERS[vid] * 0.5 + 0.5; // [0,1]
    float2 px = cv_bounds.xy + c * cv_bounds.zw;
    BgOut o;
    o.pos = float4(px.x / cv_resolution.x * 2.0 - 1.0, 1.0 - px.y / cv_resolution.y * 2.0, 0.0, 1.0);
    return o;
}

float4 bg_fragment(BgOut i) : SV_Target {
    return float4(bs_book_bg.rgb, 1.0);
}
