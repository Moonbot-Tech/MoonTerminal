//! In-scene попап настроек раскладки чарт-вкладки: режим (Fit/Scroll) + высота ТОЛЬКО
//! активного режима. Per-tab. Рендер общий для полоски вкладок главного окна и шапки
//! выносного окна; обработчики (применение к нужному стеку + persist) задаёт вызывающий.
//!
//! Семантика: Fit=0 → растяжение (делят окно); Fit≥20 → COMPRESS (фикс. высота без скролла);
//! Scroll → фикс. высота слота + скролл. Допустимый диапазон высоты — [MIN_H, MAX_H].

use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonInput, MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use crate::chart_persist::StackLayoutMode;
use crate::design;

/// Порядок режимов в сегмент-контроле попапа (два положения).
pub(super) const POPUP_MODES: [StackLayoutMode; 2] =
    [StackLayoutMode::Fit, StackLayoutMode::Scroll];

/// Границы высоты слота (px). Меньше MIN (кроме 0 у Fit = растяжение) и больше MAX вводить нельзя.
pub(super) const MIN_H: u16 = 20;
pub(super) const MAX_H: u16 = 4000;

/// Размер сценового попапа (логич. px), посчитанный из тех же метрик, что и содержимое.
/// Вызывающий ставит контейнер в absolute layer и задаёт этот размер.
pub(super) fn content_size(cx: &App) -> Size<Pixels> {
    let pad = f32::from(design::ui_px(cx, 8.0));
    let gap = f32::from(design::ui_px(cx, 8.0));
    let cap = f32::from(design::t_caption(cx)) + 6.0;
    let title_h = cap.max(f32::from(design::ui_px(cx, 22.0)));
    let seg_h = f32::from(design::ui_px(cx, 30.0));
    let line_h = f32::from(design::ui_px(cx, 30.0));
    let cb_h = f32::from(design::ui_px(cx, 22.0));
    let border = 2.0;
    let h = border
        + 2.0 * pad
        + title_h
        + gap
        + seg_h
        + gap
        + line_h
        + gap
        + 2.0 * cap
        + gap
        + cb_h
        + 6.0;
    let w = 2.0 * 110.0 + 20.0 + 2.0 * pad + border;
    size(px(w), px(h))
}

fn mode_label(m: StackLayoutMode) -> &'static str {
    match m {
        StackLayoutMode::Fit => "FIT",
        StackLayoutMode::Scroll => "SCROLL",
    }
}

/// Маленькое окошко настроек раскладки. Показывает поле высоты ТОЛЬКО для текущего режима.
/// `height_fit_input`/`height_scroll_input` — раздельные поля (подписку на Blur/Enter держит
/// вызывающий). `on_pick_mode` вызывается при выборе режима. Позиционируется вызывающим.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_layout_popup<F, G, H>(
    id: &str,
    current: StackLayoutMode,
    height_fit_input: &Entity<MoonInputState>,
    height_scroll_input: &Entity<MoonInputState>,
    orderbook_enabled: bool,
    p: MoonPalette,
    cx: &App,
    on_pick_mode: F,
    apply_all_label: String,
    on_apply_all: G,
    on_toggle_orderbook: H,
) -> AnyElement
where
    F: Fn(StackLayoutMode, &mut App) + 'static,
    G: Fn(&mut App) + 'static,
    H: Fn(bool, &mut App) + 'static,
{
    let sel = POPUP_MODES.iter().position(|m| *m == current).unwrap_or(0);
    let items: Vec<MoonSegmentItem> = POPUP_MODES
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut it = MoonSegmentItem::new("", mode_label(*m)).width(110.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(format!("{id}-mode"))
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(m) = POPUP_MODES.get(ix) {
                on_pick_mode(*m, cx);
            }
        })
        .render();

    // Поле + примечание — только для активного режима.
    let (input, label, hint) = match current {
        StackLayoutMode::Fit => (
            height_fit_input,
            t!("chart.layout.height_fit").to_string(),
            t!("chart.layout.height_fit_hint").to_string(),
        ),
        StackLayoutMode::Scroll => (
            height_scroll_input,
            t!("chart.layout.height_scroll").to_string(),
            t!("chart.layout.height_scroll_hint").to_string(),
        ),
    };
    // "Высота X  [поле]  px"
    let height_line = h_flex()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .child(div().text_color(rgb(p.text)).child(label))
        .child(
            div().w(px(64.0)).child(
                MoonInput::new(SharedString::from(format!("{id}-input")))
                    .state(input)
                    .small(),
            ),
        )
        .child(div().text_color(rgb(p.text_muted)).child("px"));
    // Примечание под полем (многострочное по '\n').
    let hint_block = v_flex().children(hint.split('\n').map(|line| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(line.to_string())
    }));

    // Чекбокс «Стакан» — вкл/выкл orderbook на графиках вкладки.
    let orderbook_cb = MoonCheckbox::new(SharedString::from(format!("{id}-orderbook")))
        .label(t!("chart.layout.orderbook").to_string())
        .checked(orderbook_enabled)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_orderbook(*ch, app));

    // Иконка «применить ко всем» — справа в строке заголовка, только символ + всплывающая подсказка
    // (текст области: ко всем окнам / только чартам).
    let apply_all_btn = MoonButton::new(SharedString::from(format!("{id}-apply-all")))
        .label("⧉")
        .tooltip(apply_all_label)
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .on_click(move |_, _w, app| on_apply_all(app))
        .render();

    // Контент заполняет сценовый popup-контейнер. Фон непрозрачный: если поверх него виден
    // chart text, это настоящий z-order баг, а не дизайнерская прозрачность.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .size_full()
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            // Заголовок слева + иконка «ко всем» прижата к правому краю окна.
            h_flex()
                .w_full()
                .items_center()
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("chart.layout.title").to_string()),
                )
                .child(div().flex_1())
                .child(apply_all_btn),
        )
        .child(seg)
        .child(height_line)
        .child(hint_block)
        .child(orderbook_cb)
        .into_any_element()
}

/// Клампинг введённой высоты: Fit допускает 0 (растяжение), иначе [MIN_H, MAX_H]; Scroll — всегда
/// [MIN_H, MAX_H].
pub(super) fn clamp_height(mode: StackLayoutMode, raw: u16) -> u16 {
    match mode {
        StackLayoutMode::Fit if raw == 0 => 0,
        _ => raw.clamp(MIN_H, MAX_H),
    }
}
