//! Раскладки карточек детекта по размеру (`DetectViewCfg::size`): мини (плитка
//! монета+бейдж+время), средний (строка как раньше + мини-чарт справа), крупный
//! (квадратная плитка со всеми полями). Мини/крупный раскладываются сеткой (flex-wrap),
//! средний — вертикальным списком (см. [`super`]). Только визуал — интерактив (клик/ПКМ)
//! навешивает [`super`] снаружи. Мини-чарт — замороженный тумбнейл (см.
//! [`crate::detect_thumb`]); поля гейтятся галками, физически невлезающие в мини — опущены.

use std::sync::Arc;

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonText, h_flex, rgba_from, v_flex,
};
use moon_core::config::detect_view::{DETECT_SIZE_LARGE, DETECT_SIZE_MINI};
use moon_core::config::{BadgesConfig, DetectViewCfg};

use super::DetectItem;
use crate::design;

/// Размер, в котором пекутся тумбнейлы (физ. px, аспект 5:3 под ленты карточек). Один бейк
/// на карточку, gpui масштабирует под место показа; 160×96×4 ≈ 61КБ на карточку (≤48 → ≤3МБ).
pub(super) const THUMB_BAKE_W: u32 = 160;
pub(super) const THUMB_BAKE_H: u32 = 96;

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
        .rounded(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgba_from(color, 0.32))
        .bg(rgba_from(color, 0.12))
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
    MoonText::new(text)
        .color(p.text_muted)
        .font_size(9.0)
        .line_height(11.0)
        .mono(true)
        .uppercase(false)
}

/// Элемент тумбнейла заданного размера (обрезка по углам).
fn thumb_el(tex: &Arc<RenderImage>, w: Pixels, h: Pixels, cx: &App) -> Div {
    div()
        .w(w)
        .h(h)
        .flex_none()
        .rounded(design::ui_px(cx, 3.0))
        .overflow_hidden()
        .child(img(tex.clone()).size_full())
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
        if let Some(tex) = &it.thumb {
            row = row.child(thumb_el(
                tex,
                design::ui_px(cx, 60.0),
                design::ui_px(cx, 34.0),
                cx,
            ));
        }
    }
    base(color, cx)
        .w_full()
        .h(design::fit_h_px(cx, 40.0, 14.0, 10.0))
        .child(row)
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
    // Картинка СВЕРХУ, тянется на всё свободное место квадрата над текстом.
    if cfg.show_chart {
        if let Some(tex) = &it.thumb {
            col = col.child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .overflow_hidden()
                    .child(img(tex.clone()).size_full()),
            );
        }
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
