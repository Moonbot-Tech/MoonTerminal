//! Раскладки карточек детекта по размеру (`DetectViewCfg::size`): мини (плитка
//! монета+бейдж+время), средний (строка как раньше + мини-чарт справа), крупный
//! (квадратная плитка со всеми полями). Мини/крупный раскладываются сеткой (flex-wrap),
//! средний — вертикальным списком (см. [`super`]). Только визуал — интерактив (клик/ПКМ)
//! навешивает [`super`] снаружи. Мини-чарт — замороженный тумбнейл (см.
//! [`crate::detect_thumb`]); поля гейтятся галками, физически невлезающие в мини — опущены.

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonText, h_flex, rgba_from, v_flex,
};
use moon_core::config::detect_view::{DETECT_SIZE_LARGE, DETECT_SIZE_MINI};
use moon_core::config::{BadgesConfig, DetectViewCfg};

use super::DetectItem;
use crate::design;

/// Высота ячейки графика среднего размера (лог. px): карточка минус отступы (3+3) и рамка.
pub(super) fn medium_cell_h(cx: &App) -> f32 {
    (design::fit_h_value(cx, 40.0, 14.0, 10.0) - 8.0).max(12.0)
}

/// Размер холста тумбнейла = ТОЧНЫЙ размер ячейки показа (лог. px, округлённый) — печём 1:1,
/// БЕЗ растяжений (просьба пользователя; заодно интринсик картинки = ячейке, и никакие
/// auto-размеры gpui не могут её раздуть). Зависит от режима размера карточки.
pub(super) fn thumb_px(cfg: &DetectViewCfg, cx: &App) -> (u32, u32) {
    if cfg.size_clamped() == DETECT_SIZE_LARGE {
        // Крупный: ширина = внутренняя ширина плитки (150 - 2×8 паддинг - 2 рамка), высота 84.
        let w = (design::ui_value(cx, 150.0) - 2.0 * design::ui_value(cx, 8.0) - 2.0).max(20.0);
        (w.round() as u32, design::ui_value(cx, 84.0).round() as u32)
    } else {
        // Средний (и мини — чарт там не показывается): ячейка 110 × (карточка-8).
        (
            design::ui_value(cx, 110.0).round() as u32,
            medium_cell_h(cx).round() as u32,
        )
    }
}

/// Сетка плиток (flex-wrap) для этого размера? Мини/крупный — да, средний — список.
pub(super) fn size_is_grid(size: u8) -> bool {
    size == DETECT_SIZE_MINI || size == DETECT_SIZE_LARGE
}

/// Собрать карточку нужного размера (без интерактива — его вешает вызывающий).
pub(super) fn card(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    match cfg.size_clamped() {
        DETECT_SIZE_MINI => mini_card(it, secs, cfg, badges, p, is_light, cx),
        DETECT_SIZE_LARGE => large_card(it, secs, cfg, badges, p, is_light, cx),
        _ => medium_card(it, secs, cfg, badges, p, is_light, cx),
    }
}

/// Тинт-подложка карточки цветом ядра + hover (как исходная лента).
fn base(color: u32, cx: &App) -> Div {
    div()
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 4.0))
        .rounded(design::r_button(cx))
        .border_1()
        .border_color(rgba_from(color, 0.32))
        .bg(rgba_from(color, 0.12))
        // Клип содержимого по рамке карточки — тумбнейл/тексты не вылезают за границу.
        .overflow_hidden()
        .hover(|s| {
            s.border_color(rgba_from(color, 0.6))
                .bg(rgba_from(color, 0.2))
        })
}

/// Бейдж типа детекта (код long/short + цвет темы + опц. обводка). None — тип неактивен.
fn type_badge(it: &DetectItem, badges: &BadgesConfig, is_light: bool) -> Option<MoonBadge> {
    badges.active(it.kind).then(|| {
        let code = badges.code(it.kind, it.is_short).to_string();
        let bcol = design::rgb_to_u32(badges.color(it.kind, is_light));
        let mut badge = MoonBadge::new(code)
            .variant(MoonBadgeVariant::Soft)
            .size(MoonBadgeSize::Tiny)
            .bg_color(bcol)
            .text_color(bcol)
            .mono(true);
        if let Some(oc) = badges.outline_color(it.kind, it.is_short, is_light) {
            let ocol = design::rgb_to_u32(oc);
            badge = badge.border_color(ocol).border_alpha(0.9);
        }
        badge
    })
}

/// Бейдж имени ядра.
fn core_badge(it: &DetectItem, color: u32) -> MoonBadge {
    MoonBadge::new(it.core_name.clone())
        .variant(MoonBadgeVariant::Soft)
        .size(MoonBadgeSize::Status)
        .bg_color(color)
        .text_color(color)
        .border_color(color)
        .border_alpha(0.4)
        .mono(true)
}

/// Токен монеты (крупная моно-подпись, ужимается и обрезается).
fn coin_text(it: &DetectItem, p: MoonPalette, size: f32) -> MoonText {
    MoonText::new(it.base.clone())
        .color(p.text)
        .font_size(size)
        .line_height(size + 3.0)
        .weight(600.0)
        .mono(true)
        .uppercase(false)
}

/// Мелкая приглушённая подпись (время/биржа).
fn muted(text: String, p: MoonPalette) -> MoonText {
    // Кегль/интерлиньяж НЕ задаём: дефолт MoonText (9/11) — ровно то, что нужно.
    MoonText::new(text)
        .color(p.text_muted)
        .mono(true)
        .uppercase(false)
}

/// Цвета роста/падения (как дельты в шапке терминала).
fn pos_col(p: MoonPalette) -> u32 {
    if p.is_light() {
        p.green_text
    } else {
        p.green
    }
}
fn neg_col(p: MoonPalette) -> u32 {
    if p.is_light() {
        p.red_text
    } else {
        p.red
    }
}

/// Подпись дельты («+1.23%» / «-0.45%») с ПОДЛОЖКОЙ (полупрозрачный фон темы — не сливается с
/// линией). Зелёная при ≥0, красная при <0, со знаком; мелкий жирный моно (стандартный MoonText).
fn delta_text(val: f32, p: MoonPalette, cx: &App) -> Div {
    let col = if val < 0.0 { neg_col(p) } else { pos_col(p) };
    div()
        .px(px(2.0))
        .rounded(design::ui_px(cx, 2.0))
        .bg(rgba_from(p.surface, 0.72))
        .child(
            MoonText::new(format!("{val:+.2}%"))
                .color(col)
                .weight(700.0)
                .mono(true)
                .uppercase(false),
        )
}

/// Ячейка графика (свечи ИЛИ линия по `cfg.line_mode`) + дельты 24ч (сверху-слева) и 1ч
/// (снизу-слева) поверх — гейтятся галками, показываются в ОБОИХ режимах. Размер задаёт
/// родитель (definite w×h; холст 1:1). `ObjectFit::Fill` — картинка точно по ячейке.
fn chart_cell(it: &DetectItem, cfg: &DetectViewCfg, p: MoonPalette, cx: &App) -> Div {
    let mut cell = div()
        .relative()
        .size_full()
        .rounded(design::ui_px(cx, 3.0))
        .overflow_hidden();
    // img — ОБЫЧНЫЙ (in-flow) ребёнок, НЕ absolute-обёртка: пары инсетов (inset_0) в gpui не
    // резолвят размер → обёртка брала интринсик картинки и, будучи absolute, НЕ клипалась
    // overflow_hidden. In-flow + size_full от ячейки с definite-размером = точные границы.
    let tex = if cfg.line_mode {
        &it.line_thumb
    } else {
        &it.thumb
    };
    if let Some(tex) = tex {
        cell = cell.child(img(tex.clone()).size_full().object_fit(ObjectFit::Fill));
    }
    cell.children(cfg.show_delta_24h.then(|| {
        div()
            .absolute()
            .top(px(2.0))
            .left(px(3.0))
            .child(delta_text(it.delta_24h, p, cx))
    }))
    .children(cfg.show_delta_1h.then(|| {
        div()
            .absolute()
            .bottom(px(2.0))
            .left(px(3.0))
            .child(delta_text(it.delta_1h, p, cx))
    }))
}

/// Строка «биржа · тип» (то, что включено галками). Пусто → None.
fn exchange_line(it: &DetectItem, cfg: &DetectViewCfg, p: MoonPalette) -> Option<MoonText> {
    let mut parts: Vec<&str> = Vec::new();
    if cfg.show_exchange && !it.exchange_name.is_empty() {
        parts.push(it.exchange_name.as_str());
    }
    if cfg.show_exchange_kind && !it.exchange_kind.is_empty() {
        parts.push(it.exchange_kind.as_str());
    }
    (!parts.is_empty()).then(|| muted(parts.join(" · "), p))
}

// --- Средний размер (строка как раньше + опц. мини-чарт справа) ---
fn medium_card(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let color = design::rgb_to_u32(it.color);
    let card_hv = design::fit_h_value(cx, 40.0, 14.0, 10.0);
    let text_col = v_flex()
        .flex_1()
        .min_w(px(0.0))
        .size_full()
        .justify_between()
        // Верх: токен + бейдж типа.
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(coin_text(it, p, 13.0)),
                )
                .child(
                    div()
                        .flex_none()
                        .children(cfg.show_badge.then(|| type_badge(it, badges, is_light)).flatten()),
                ),
        )
        // Низ: время + биржа слева, бейдж ядра справа.
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_end()
                .gap_1()
                .child(
                    h_flex()
                        .items_end()
                        .gap_1()
                        .mb(px(2.0))
                        .children(cfg.show_time.then(|| muted(format!("{secs}s"), p)))
                        .children(exchange_line(it, cfg, p)),
                )
                .children(cfg.show_core.then(|| core_badge(it, color))),
        );

    let mut row = h_flex().size_full().gap_2().items_stretch().child(text_col);
    if cfg.show_chart {
        // Резервируем ширину под график ПУСТЫМ спейсером — сам график рисуем АБСОЛЮТНЫМ
        // оверлеем с прижимом top+bottom (ниже): flex-высота строки резолвилась больше
        // внутренней области карточки (дебаг-рамка показала: ячейка доходила до низа карточки),
        // поэтому любой h_full/явный h в потоке вылезал. Абсолют с top+bottom не может.
        row = row.child(div().w(design::ui_px(cx, 110.0)).flex_none());
    }
    let mut card = base(color, cx)
        .relative()
        .w_full()
        .h(px(card_hv))
        .child(row);
    if cfg.show_chart {
        // ЯВНАЯ высота оверлея (НЕ пара top+bottom инсетов!): gpui не резолвит высоту
        // абсолюта из двух инсетов. Размер оверлея = размеру холста бейка (thumb_px) —
        // картинка кладётся 1:1, без растяжений.
        let overlay = div()
            .absolute()
            .top(px(3.0))
            .right(px(4.0))
            .w(px(design::ui_value(cx, 110.0).round()))
            .h(px(medium_cell_h(cx).round()))
            .child(chart_cell(it, cfg, p, cx));
        card = card.child(overlay);
    }
    card
}

// --- Крупный размер: КВАДРАТНАЯ кнопка, картинка сверху, текст ПОД ней ---
fn large_card(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let color = design::rgb_to_u32(it.color);
    let side = design::ui_px(cx, 150.0);
    // КВАДРАТ, растягивается под ширину колонки: max_w = потолок 150, но в узкой панели плитка
    // УЖИМАЕТСЯ (flex-shrink по умолчанию), а высота идёт за шириной через aspect_ratio(1.0) —
    // квадрат мельчает целиком, не режется. Картинка внутри flex_1 → масштабируется с плиткой.
    let mut col = base(color, cx)
        .w(side)
        .max_w(side)
        .aspect_ratio(1.0)
        .overflow_hidden()
        .gap(design::ui_px(cx, 3.0));
    // Картинка СВЕРХУ (свечи/линия + дельты) — ФИКС-размер контейнера = размеру холста (1:1).
    if cfg.show_chart {
        let (tw, th) = thumb_px(cfg, cx);
        col = col.child(
            div()
                .w(px(tw as f32))
                .h(px(th as f32))
                .flex_none()
                .child(chart_cell(it, cfg, p, cx)),
        );
    }
    // Текст ПОД картинкой (flex_none — держит высоту, картинка сверху забирает остаток).
    col.child(
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(coin_text(it, p, 14.0)),
            )
            .child(
                div()
                    .flex_none()
                    .children(cfg.show_badge.then(|| type_badge(it, badges, is_light)).flatten()),
            ),
    )
    .child(
        h_flex()
            .w_full()
            .flex_none()
            .justify_between()
            .items_end()
            .gap_1()
            .children(cfg.show_time.then(|| muted(format!("{secs}s"), p)))
            .children(cfg.show_core.then(|| core_badge(it, color))),
    )
    .children(exchange_line(it, cfg, p).map(|t| div().w_full().flex_none().child(t)))
}

// --- Мини размер (компактная плитка: монета+бейдж+время) ---
fn mini_card(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let color = design::rgb_to_u32(it.color);
    let w = design::ui_px(cx, 86.0);
    // Растягивается под колонку: потолок 86, но в узкой панели ужимается (flex-shrink).
    base(color, cx)
        .w(w)
        .max_w(w)
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .child(coin_text(it, p, 12.0)),
                )
                .child(
                    div()
                        .flex_none()
                        .children(cfg.show_badge.then(|| type_badge(it, badges, is_light)).flatten()),
                ),
        )
        .children(cfg.show_time.then(|| muted(format!("{secs}s"), p)))
}
