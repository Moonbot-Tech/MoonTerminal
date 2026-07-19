//! Карточки детекта по дизайн-макету:
//! нейтральная кнопка (фон/рамка палитры, БЕЗ заливки цветом ядра) + слева цветная
//! полоска сервера (rail) с градиент-фейдом; поля раскладываются ПО СЛОТАМ конфига
//! ([`DetectSizeCfg::slots`]): мини 4 (2 ряда), средний 6 (колонки по бокам графика),
//! крупный 9 (ряды над/под графиком + углы поверх). Габариты/график/rail — per-размер
//! из `detects_view.toml`. Только визуал — интерактив (клик/ПКМ) вешает [`super`].

use gpui::*;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonBadgeVariant, MoonPalette, MoonText, h_flex, rgba_from, v_flex,
};

use moon_core::config::{
    BadgesConfig, DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI, DetectChart,
    DetectField, DetectSizeCfg, DetectSlot, DetectViewCfg, detect_slot_count,
};

use super::DetectItem;
use crate::design;

/// Доля ширины карточки среднего размера под зону графика (текстовые колонки по бокам).
const MEDIUM_CHART_FRAC: f32 = 0.40;
/// Высота ряда полей над/под графиком (крупный) и рядов мини, лог. px.
const BAND_H: f32 = 16.0;

/// Номинальный размер зоны графика (лог. px после масштаба, округлён): высота бокса
/// показа фиксированная, ширина на дисплее тянется на всю зону (графики векторные).
fn zone_dims(size: u8, s: &DetectSizeCfg, cx: &App) -> (f32, f32) {
    let (w, h) = (f32::from(s.w), f32::from(s.h));
    match size {
        // Крупный: зона на всю внутреннюю ширину, высота = карточка минус ряды/отступы.
        DETECT_SIZE_LARGE => (
            design::ui_value(cx, w - 16.0).round(),
            design::ui_value(cx, (h - 12.0 - 2.0 * BAND_H - 6.0).max(12.0)).round(),
        ),
        // Средний: фиксированная доля ширины, высота почти на всю карточку.
        _ => (
            medium_zone_w(s, cx),
            design::ui_value(cx, (h - 10.0).max(12.0)).round(),
        ),
    }
}

/// Собрать карточку активного размера (без интерактива — его вешает вызывающий).
#[allow(clippy::too_many_arguments)]
pub(super) fn card(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    card_sized(
        it,
        secs,
        cfg,
        cfg.size_clamped(),
        theme,
        badges,
        p,
        is_light,
        cx,
    )
}

/// Карточка КОНКРЕТНОГО размера — для ленты (активный) и превью попапа (размер вкладки).
#[allow(clippy::too_many_arguments)]
pub(super) fn card_sized(
    it: &DetectItem,
    secs: u32,
    cfg: &DetectViewCfg,
    size: u8,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let scfg = cfg.size_cfg(size);
    let dec = cfg.delta_decimals_clamped();
    let color = design::rgb_to_u32(it.color);
    let inner = match size {
        DETECT_SIZE_MINI => mini_layout(it, secs, scfg, dec, theme, badges, p, is_light, cx),
        DETECT_SIZE_LARGE => large_layout(it, secs, scfg, dec, theme, badges, p, is_light, cx),
        _ => medium_layout(it, secs, scfg, dec, theme, badges, p, is_light, cx),
    };
    base(scfg, color, p, cx).child(inner)
}

/// Нейтральная кнопка-подложка + rail (полоска цвета сервера и градиент-фейд от неё).
fn base(scfg: &DetectSizeCfg, color: u32, p: MoonPalette, cx: &App) -> Div {
    let w = design::ui_px(cx, f32::from(scfg.w));
    let h = design::ui_px(cx, f32::from(scfg.h));
    let mut card = div()
        .relative()
        .flex()
        .w(w)
        .max_w(w)
        .h(h)
        .flex_none()
        // Радиус 6 — по макету (между button 4 и container 8 moonui).
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.shell_high))
        .overflow_hidden()
        .hover(|s| s.border_color(rgba_from(color, 0.6)).bg(rgb(p.panel_high)));
    for layer in rail_layers(
        color,
        f32::from(scfg.rail_w_clamped()),
        f32::from(scfg.rail_grad_clamped()),
        f32::from(scfg.w),
        cx,
    ) {
        card = card.child(layer);
    }
    card
}

/// Слои rail: полоска и градиент рисуются ФОНАМИ слоёв на всю карточку (жёсткий стоп
/// градиента на ширине полоски) — фон gpui скругляется по радиусам div'а, поэтому
/// полоска сама изгибается по углу карточки во всю высоту (отдельный брусок торчал бы
/// углом: клип overflow_hidden прямоугольный, а радиус узкого div клампится к половине
/// его ширины). Слои с inset 1px — под рамкой карточки.
pub(super) fn rail_layers(color: u32, rail_w: f32, grad_w: f32, card_w: f32, cx: &App) -> Vec<Div> {
    let mut out = Vec::new();
    let inner_w = (design::ui_value(cx, card_w) - 2.0).max(1.0);
    let r = design::ui_px(cx, 5.0);
    let stripe = design::ui_value(cx, rail_w) / inner_w;
    let layer = || {
        div()
            .absolute()
            .left(px(1.0))
            .right(px(1.0))
            .top(px(1.0))
            .bottom(px(1.0))
            .rounded(r)
    };
    if grad_w > 0.0 {
        let grad_end = ((design::ui_value(cx, rail_w + grad_w)) / inner_w).clamp(0.0, 1.0);
        out.push(layer().bg(linear_gradient(
            90.0,
            linear_color_stop(rgba_from(color, 0.125), stripe.min(1.0)),
            linear_color_stop(rgba_from(color, 0.0), grad_end.max(stripe + 0.001)),
        )));
    }
    if rail_w > 0.0 {
        // Жёсткий стоп (~полпикселя перехода = лёгкое сглаживание кромки).
        out.push(layer().bg(linear_gradient(
            90.0,
            linear_color_stop(rgba_from(color, 1.0), stripe.min(1.0)),
            linear_color_stop(rgba_from(color, 0.0), (stripe + 0.002).min(1.0)),
        )));
    }
    out
}

/// Номинальная ширина зоны графика среднего размера.
fn medium_zone_w(scfg: &DetectSizeCfg, cx: &App) -> f32 {
    design::ui_value(cx, (f32::from(scfg.w) * MEDIUM_CHART_FRAC).max(30.0)).round()
}

/// Левый внутренний отступ контента: за полоску + небольшой зазор.
fn pad_l(scfg: &DetectSizeCfg, base: f32, cx: &App) -> Pixels {
    design::ui_px(cx, base + f32::from(scfg.rail_w_clamped()))
}

// --- Field chips use the shared MoonText, MoonBadge, and delta styles. ---

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

/// Бейдж имени ядра (цвет сервера). Кегль — как у бейджа типа (Tiny).
fn core_badge(it: &DetectItem, color: u32) -> MoonBadge {
    MoonBadge::new(it.core_name.clone())
        .variant(MoonBadgeVariant::Soft)
        .size(MoonBadgeSize::Tiny)
        .bg_color(color)
        .text_color(color)
        .border_color(color)
        .border_alpha(0.4)
        .mono(true)
}

/// Токен монеты (крупная моно-подпись).
fn coin_text(it: &DetectItem, p: MoonPalette, size: f32) -> MoonText {
    MoonText::new(it.base.clone())
        .color(p.text)
        .font_size(size)
        .line_height(size + 3.0)
        .weight(600.0)
        .mono(true)
        .uppercase(false)
}

/// Мелкая приглушённая подпись (время). Кегль — дефолт MoonText (9).
fn muted(text: String, p: MoonPalette) -> MoonText {
    MoonText::new(text)
        .color(p.text_muted)
        .mono(true)
        .uppercase(false)
}

/// Подпись биржи/типа биржи (soft-тон).
fn soft(text: String, p: MoonPalette) -> MoonText {
    MoonText::new(text)
        .color(p.text_soft)
        .mono(true)
        .uppercase(false)
}

/// Цвета роста/падения (как дельты в шапке терминала).
use design::{danger_color as neg_col, positive_color as pos_col};

/// Дельта («+1.23%») жирным моно; `over` — с подложкой (читаемость поверх графика).
/// `decimals` — знаков после запятой (настройка «Точность» попапа, одна на все размеры).
fn delta_chip(val: f32, over: bool, decimals: usize, p: MoonPalette, cx: &App) -> Div {
    // Same percentage contract as the header deltas: classify by the ROUNDED value, so a small
    // negative cannot print a minus while being coloured positive.
    let (label, col) = match moon_core::util::fmt::signed_pct(f64::from(val), decimals) {
        Some((text, sign)) => (text, sign.pick(pos_col(p), neg_col(p), p.text_soft)),
        None => ("—".to_string(), p.text_muted),
    };
    let text = MoonText::new(label)
        .color(col)
        .weight(700.0)
        .mono(true)
        .uppercase(false);
    let mut chip = div().child(text);
    if over {
        chip = chip
            .px(px(2.0))
            .rounded(design::ui_px(cx, 2.0))
            .bg(rgba_from(p.surface, 0.72));
    }
    chip
}

/// Чип одного поля слота. None — поле пустое/неактивное (бейдж типа может быть выключен).
#[allow(clippy::too_many_arguments)]
fn chip(
    field: DetectField,
    over: bool,
    it: &DetectItem,
    secs: u32,
    coin_px: f32,
    decimals: usize,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Option<AnyElement> {
    let el: AnyElement = match field {
        DetectField::None => return None,
        DetectField::Coin => coin_text(it, p, coin_px).render().into_any_element(),
        DetectField::Time => muted(format!("{secs}s"), p).render().into_any_element(),
        DetectField::Badge => type_badge(it, badges, is_light)?
            .render()
            .into_any_element(),
        DetectField::Core => core_badge(it, design::rgb_to_u32(it.color))
            .render()
            .into_any_element(),
        DetectField::Delta24h => delta_chip(it.delta_24h, over, decimals, p, cx).into_any_element(),
        DetectField::Delta1h => delta_chip(it.delta_1h, over, decimals, p, cx).into_any_element(),
        DetectField::Exchange => {
            if it.exchange_name.is_empty() {
                return None;
            }
            soft(it.exchange_name.clone(), p)
                .render()
                .into_any_element()
        }
        DetectField::ExchangeKind => {
            if it.exchange_kind.is_empty() {
                return None;
            }
            soft(it.exchange_kind.clone(), p)
                .render()
                .into_any_element()
        }
    };
    // Не-дельтовые чипы поверх графика тоже получают подложку (как в макете).
    if over && !matches!(field, DetectField::Delta24h | DetectField::Delta1h) {
        return Some(
            div()
                .px(px(2.0))
                .rounded(design::ui_px(cx, 2.0))
                .bg(rgba_from(p.surface, 0.72))
                .child(el)
                .into_any_element(),
        );
    }
    Some(el)
}

/// Кластер чипов слотов, прошедших фильтр (порядок слотов сохраняется). None — пусто.
#[allow(clippy::too_many_arguments)]
fn cluster<'a>(
    slots: impl Iterator<Item = &'a DetectSlot>,
    over: bool,
    it: &DetectItem,
    secs: u32,
    coin_px: f32,
    decimals: usize,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Option<Div> {
    let chips: Vec<AnyElement> = slots
        .filter_map(|s| {
            chip(
                s.field, over, it, secs, coin_px, decimals, badges, p, is_light, cx,
            )
        })
        .collect();
    if chips.is_empty() {
        return None;
    }
    Some(
        h_flex()
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .whitespace_nowrap()
            .children(chips),
    )
}

/// Элемент графика по типу из конфига — ОБА векторные, рисуются по фактическим
/// границам зоны в paint (битмап-тумбнейлы «плыли» от растяжения и требовали
/// бейка/инвалидации). None — нет графика/данных.
fn chart_el(
    it: &DetectItem,
    scfg: &DetectSizeCfg,
    theme: &moon_core::config::ChartTheme,
) -> Option<AnyElement> {
    match scfg.chart {
        DetectChart::None => None,
        DetectChart::Candles => candle_canvas(&it.bars, theme),
        DetectChart::Line => line_canvas(&it.line, theme),
    }
}

/// Draw hollow vector candles as quads in the element's actual bounds.
///
/// The high-low wick has segments above and below the body. Rising and doji bodies are outlined;
/// falling bodies are filled. Scaling uses the high-low range with a 1px inset.
fn candle_canvas(
    bars: &[(f32, f32, f32, f32)],
    theme: &moon_core::config::ChartTheme,
) -> Option<AnyElement> {
    if bars.is_empty() {
        return None;
    }
    let up = rgba_from(design::rgb_to_u32(theme.candle_up), 1.0);
    let down = rgba_from(design::rgb_to_u32(theme.candle_down), 1.0);
    let neutral = rgba_from(design::rgb_to_u32(theme.candle_neutral), 1.0);
    let bars: Vec<(f32, f32, f32, f32)> = bars.to_vec();
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w < 2.0 || h < 2.0 {
                    return;
                }
                let (mut hi, mut lo) = (f32::NEG_INFINITY, f32::INFINITY);
                for &(_, bh, bl, _) in &bars {
                    hi = hi.max(bh);
                    lo = lo.min(bl);
                }
                if !hi.is_finite() || !lo.is_finite() {
                    return;
                }
                let span = (hi - lo).max(1e-9);
                let pad = if h > 6.0 { 1.0 } else { 0.0 };
                let usable = (h - 2.0 * pad).max(1.0);
                let yof = |price: f32| pad + (hi - price) / span * usable;
                let n = bars.len() as f32;
                let col_w = (w / n).max(1.0);
                let wick_w = (col_w * 0.22).clamp(1.0, 3.0);
                let body_w = (col_w * 0.68).max(wick_w);
                let (ox, oy) = (bounds.origin.x, bounds.origin.y);
                let mut quad = |x0: f32, x1: f32, y0: f32, y1: f32, c: Hsla| {
                    let (y0, y1) = (y0.min(y1), (y0.max(y1)).max(y0.min(y1) + 1.0));
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            gpui::point(ox + px(x0), oy + px(y0)),
                            gpui::point(ox + px(x1.max(x0 + 1.0)), oy + px(y1)),
                        ),
                        c,
                    ));
                };
                for (i, &(o, bh, bl, c)) in bars.iter().enumerate() {
                    let xc = (i as f32 + 0.5) * col_w;
                    let color = if c > o {
                        up
                    } else if c < o {
                        down
                    } else {
                        neutral
                    };
                    let (x0, x1) = (xc - body_w * 0.5, xc + body_w * 0.5);
                    let (yt, yb) = (yof(o).min(yof(c)), yof(o).max(yof(c)));
                    let (wx0, wx1) = (xc - wick_w * 0.5, xc + wick_w * 0.5);
                    if yt - yof(bh) > 0.5 {
                        quad(wx0, wx1, yof(bh), yt, color);
                    }
                    if yof(bl) - yb > 0.5 {
                        quad(wx0, wx1, yb, yof(bl), color);
                    }
                    if c < o {
                        quad(x0, x1, yt, yb, color);
                    } else {
                        // Полое тело: контур 1px.
                        quad(x0, x1, yt, yt + 1.0, color);
                        quad(x0, x1, yb - 1.0, yb, color);
                        quad(x0, x0 + 1.0, yt, yb, color);
                        quad(x1 - 1.0, x1, yt, yb, color);
                    }
                }
            },
        )
        .size_full()
        .into_any_element(),
    )
}

/// Draw a vector sparkline against the element's actual bounds with a fixed-width stroke.
///
/// Binning keeps roughly one point per 3px; the high/low range uses the smoothed series with 18%
/// vertical padding.
fn line_canvas(line: &[f32], theme: &moon_core::config::ChartTheme) -> Option<AnyElement> {
    if line.len() < 2 {
        return None;
    }
    let rgb3 = if line[line.len() - 1] >= line[0] {
        theme.candle_up
    } else {
        theme.candle_down
    };
    let color = rgba_from(design::rgb_to_u32(rgb3), 1.0);
    let prices: Vec<f32> = line.to_vec();
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w < 2.0 || h < 2.0 {
                    return;
                }
                let target = ((w / 3.0) as usize).clamp(2, prices.len());
                let pts: Vec<f32> = (0..target)
                    .map(|k| {
                        let a = k * prices.len() / target;
                        let b = (((k + 1) * prices.len() / target).max(a + 1)).min(prices.len());
                        let sl = &prices[a..b];
                        sl.iter().copied().filter(|v| v.is_finite()).sum::<f32>()
                            / (sl.iter().filter(|v| v.is_finite()).count().max(1) as f32)
                    })
                    .collect();
                let (mut hi, mut lo) = (f32::NEG_INFINITY, f32::INFINITY);
                for &v in &pts {
                    if v.is_finite() {
                        hi = hi.max(v);
                        lo = lo.min(v);
                    }
                }
                if !hi.is_finite() || !lo.is_finite() {
                    return;
                }
                let span = (hi - lo).max(1e-9);
                let pad = (h * 0.18).max(2.0);
                let usable = (h - 2.0 * pad).max(1.0);
                let n = pts.len();
                let mut pb = PathBuilder::stroke(px(2.0));
                for (k, &v) in pts.iter().enumerate() {
                    let x = bounds.origin.x + px(k as f32 / ((n - 1).max(1) as f32) * w);
                    let y = bounds.origin.y + px(pad + (hi - v) / span * usable);
                    if k == 0 {
                        pb.move_to(gpui::point(x, y));
                    } else {
                        pb.line_to(gpui::point(x, y));
                    }
                }
                if let Ok(path) = pb.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full()
        .into_any_element(),
    )
}

/// Эффективный «поверх графика»: только при включённом графике.
fn eff_over(scfg: &DetectSizeCfg, slot: &DetectSlot) -> bool {
    scfg.chart != DetectChart::None && slot.over
}

// --- Мини: 2 ряда × лево/право, без графика ---

fn mini_layout(
    it: &DetectItem,
    secs: u32,
    scfg: &DetectSizeCfg,
    decimals: usize,
    _theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let n = detect_slot_count(DETECT_SIZE_MINI);
    let slots = &scfg.slots[..n];
    let row = |range: std::ops::Range<usize>| {
        let l = cluster(
            slots[range.clone()].iter().filter(|s| !s.right),
            false,
            it,
            secs,
            12.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        let r = cluster(
            slots[range].iter().filter(|s| s.right),
            false,
            it,
            secs,
            12.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .children(l)
            .children(r)
    };
    v_flex()
        .size_full()
        .pl(pad_l(scfg, 7.0, cx))
        .pr(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .justify_between()
        .child(row(0..2))
        .child(row(2..4))
}

// --- Средний: текст-колонки по бокам, график в середине, over-чипы по краям графика ---

fn medium_layout(
    it: &DetectItem,
    secs: u32,
    scfg: &DetectSizeCfg,
    decimals: usize,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let n = detect_slot_count(DETECT_SIZE_MEDIUM);
    let slots = &scfg.slots[..n];
    let half = n / 2;
    // Текстовая колонка сбоку: верхний ряд из первых 3 слотов, нижний — из вторых.
    let column = |right: bool| -> Div {
        let top = cluster(
            slots[..half]
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.right == right),
            false,
            it,
            secs,
            13.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        let bot = cluster(
            slots[half..]
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.right == right),
            false,
            it,
            secs,
            13.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        // Колонка по КОНТЕНТУ (flex_none): остаток ширины забирает график (как в макете);
        // пустая колонка не занимает место и не оставляет дыру сбоку.
        let mut col = v_flex()
            .h_full()
            .flex_none()
            .min_w(px(0.0))
            .overflow_hidden()
            .justify_between()
            .py(design::ui_px(cx, 5.0));
        if right {
            col = col.items_end();
        }
        col.children(top).children(bot)
    };
    // Чип(ы) поверх графика — по углам зоны (якорь одной стороной, без пар-инсетов:
    // gpui не резолвит размер абсолюта из пары, контент уезжал за рамку).
    let corner = |top_row: bool, right: bool| -> Option<Div> {
        let range = if top_row { 0..half } else { half..n };
        let c = cluster(
            slots[range]
                .iter()
                .filter(|s| eff_over(scfg, s) && s.right == right),
            true,
            it,
            secs,
            13.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        )?;
        let mut host = div().absolute();
        host = if right {
            host.right(px(3.0))
        } else {
            host.left(px(3.0))
        };
        host = if top_row {
            host.top(px(4.0))
        } else {
            host.bottom(px(4.0))
        };
        Some(host.child(c))
    };

    let chart_on = scfg.chart != DetectChart::None;
    let mid = if chart_on {
        // Зона забирает остаток между колонками. Ширина картинки тянется/сжимается
        // на ВСЮ зону (w_full + Fill — иначе фикс-бокс прилипал к контенту слева и
        // обгрызался справа), высота — ТОЧНО бейк (вертикальный стретч резал линию).
        let (_zw, zh) = zone_dims(DETECT_SIZE_MEDIUM, scfg, cx);
        let mut zone = div()
            .relative()
            .flex_1()
            .min_w(design::ui_px(cx, 20.0))
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden();
        if let Some(el) = chart_el(it, scfg, theme) {
            zone = zone.child(div().w_full().h(px(zh)).flex_none().child(el));
        }
        zone = zone
            .children(corner(true, false))
            .children(corner(true, true))
            .children(corner(false, false))
            .children(corner(false, true));
        zone
    } else {
        div().flex_1()
    };

    h_flex()
        .size_full()
        .pl(pad_l(scfg, 8.0, cx))
        .pr(design::ui_px(cx, 6.0))
        .items_stretch()
        .gap(design::ui_px(cx, 6.0))
        .child(column(false))
        .child(mid)
        .child(column(true))
}

// --- Крупный: ряды над/под графиком + углы поверх ---

fn large_layout(
    it: &DetectItem,
    secs: u32,
    scfg: &DetectSizeCfg,
    decimals: usize,
    theme: &moon_core::config::ChartTheme,
    badges: &BadgesConfig,
    p: MoonPalette,
    is_light: bool,
    cx: &App,
) -> Div {
    let n = detect_slot_count(DETECT_SIZE_LARGE);
    let slots = &scfg.slots[..n];
    // Ряд НЕ-поверх для своей стороны (над/под графиком): лево/право по краям.
    let band = |below: bool| -> Div {
        let l = cluster(
            slots
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.below == below && !s.right),
            false,
            it,
            secs,
            14.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        let r = cluster(
            slots
                .iter()
                .filter(|s| !eff_over(scfg, s) && s.below == below && s.right),
            false,
            it,
            secs,
            14.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        );
        // Пустой ряд держит высоту — график не прыгает от настроек полей (как в макете).
        h_flex()
            .w_full()
            .h(design::ui_px(cx, BAND_H))
            .justify_between()
            .items_center()
            .gap(design::ui_px(cx, 6.0))
            .children(l)
            .children(r)
    };
    // Угол поверх графика (verh/niz × левый/правый).
    let corner = |below: bool, right: bool| -> Option<Div> {
        let c = cluster(
            slots
                .iter()
                .filter(|s| eff_over(scfg, s) && s.below == below && s.right == right),
            true,
            it,
            secs,
            14.0,
            decimals,
            badges,
            p,
            is_light,
            cx,
        )?;
        let mut host = div().absolute();
        host = if right {
            host.right(px(4.0))
        } else {
            host.left(px(4.0))
        };
        host = if below {
            host.bottom(px(2.0))
        } else {
            host.top(px(2.0))
        };
        Some(host.child(c))
    };

    let chart_on = scfg.chart != DetectChart::None;
    let mid = if chart_on {
        // Зона между рядами: ширина картинки — на всю зону (Fill по X), высота —
        // ТОЧНО бейк (вертикальный стретч резал линию). Углы — абсолюты с якорем
        // одной стороной.
        let (_zw, zh) = zone_dims(DETECT_SIZE_LARGE, scfg, cx);
        let mut zone = div()
            .relative()
            .flex_1()
            .min_h(design::ui_px(cx, 12.0))
            .my(px(3.0))
            .flex()
            .items_center()
            .overflow_hidden();
        if let Some(el) = chart_el(it, scfg, theme) {
            zone = zone.child(div().w_full().h(px(zh)).flex_none().child(el));
        }
        zone = zone
            .children(corner(false, false))
            .children(corner(false, true))
            .children(corner(true, false))
            .children(corner(true, true));
        zone
    } else {
        div().flex_1()
    };

    v_flex()
        .size_full()
        .pl(pad_l(scfg, 8.0, cx))
        .pr(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 6.0))
        .child(band(false))
        .child(mid)
        .child(band(true))
}
