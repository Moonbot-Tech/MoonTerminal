//! Сборка полосы тулбара (size/Lev/SL/TP+S-слоты/Live). Вынесено из `controls.rs` точь-в-точь.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSegment, MoonButtonSize, MoonButtonVariant,
    MoonInputState, MoonPalette, h_flex,
};

use moon_core::session::CoreId;

use super::metric::{metric_button, sl_toggle};
use super::strips::{SIZE_SEL_DEFAULT, divider, sell_strip, size_strip};
use super::{TOOLBAR_H, TradeMetric, fmt_field2, fmt_field2_signed};
use crate::shell::Shell;
use crate::{Backend, design};

/// Полоса тулбара: рисуется как обычный child `Shell` (между шапкой и доком), не dock-панель.
/// Читает текущий масштаб/follow из `backend`, клики пишут обратно (+notify → перерисовка).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(CoreId, usize)>,
    size_input: &Entity<MoonInputState>,
    sell_edit: Option<(CoreId, usize)>,
    sell_input: &Entity<MoonInputState>,
    shell: &Entity<Shell>,
    open_metric: Option<TradeMetric>,
    cx: &App,
) -> impl IntoElement {
    let (
        follow,
        focus_core,
        size_values,
        size_sel,
        tp_str,
        tp_engaged,
        sl_str,
        sl_on,
        lev_str,
        sell_pcts,
        sell_slot,
        manual_on,
    ) = {
        let b = backend.read(cx);
        // Активное торговое ядро = выбор в селекторе шапки (sticky-override) ИЛИ ядро
        // открытого фуллскрином Main-чарта. Все торговые контролы (размеры/TP/SL/Lev/sell)
        // читают ЕГО. Нет ядра → дефолтные размеры, прочерки, клики игнор.
        let focus_core = b.active_trade_core(group);
        let (size_values, size_sel) = match focus_core {
            Some(core) => b.manual_order_size_state(core),
            None => (
                moon_core::config::servers::default_order_sizes(""),
                SIZE_SEL_DEFAULT,
            ),
        };
        let core_data = focus_core.and_then(|c| b.session.store().core(c));
        let cs = core_data.and_then(|d| d.client_settings.as_ref());
        // Кнопка TP всегда показывает СВОЙ TP (`take_profit_main_pct`), даже когда задействован
        // S-слот — выбор слота не подменяет отображаемое значение TP.
        let tp_str = cs
            .map(|s| format!("{}%", fmt_field2(s.take_profit_main_pct as f32)))
            .unwrap_or_else(|| "—".to_string());
        // TP «горит», когда fixed-sell выключен. Локальный optimistic-state перекрывает
        // снимок ядра, чтобы клик по S/TP отображался сразу, а не после echo ClientSettings.
        let tp_engaged = match (focus_core, cs) {
            (Some(core), Some(s)) => !b.fixed_sell_mode_with(core, s.fixed_sell_mode),
            _ => true,
        };
        // SL знаковый: «+1,00%» / «-20,00%» (а не «--» из ручного минуса перед отрицательным).
        let sl_str = cs
            .map(|s| format!("{}%", fmt_field2_signed(s.stop_loss_pct)))
            .unwrap_or_else(|| "—".to_string());
        // Включён ли стоп-лосс (`panic_if_price_drop`) — управляется тоглом рядом с кнопкой SL;
        // когда выкл, кнопка SL неактивна.
        let sl_on = cs.map(|s| s.panic_if_price_drop).unwrap_or(false);
        // Накладываем оптимистичный локальный кэш поверх значений ядра (живой sell-дисплей).
        let sell_pcts = focus_core.zip(cs).map(|(core, s)| {
            let arr: [f64; 6] =
                std::array::from_fn(|i| b.fixed_sell_pct_with(core, i, s.fixed_sell_pcts[i]));
            arr
        });
        // S-слот подсвечен ТОЛЬКО когда fixed-sell включён (иначе по умолчанию все S погашены).
        let sell_slot = match (focus_core, cs) {
            (Some(core), Some(s)) => {
                b.fixed_sell_slot_with(core, s.fixed_sell_mode.then_some(s.fixed_sell_slot))
            }
            _ => None,
        };
        // Lev = плечо монеты main-чарта на активном ядре (per-core, per-coin) из ассетов.
        let lev_str = TradeMetric::Lev
            .current(b, group)
            .filter(|l| *l > 0.0)
            .map(|l| format!("×{}", l as i32))
            .unwrap_or_else(|| "—".to_string());
        // Ручная стратегия включена (тогл MS в шапке): sell/стопы новым ордерам ставит ядро
        // из её полей → TP/S-слоты/SL тулбара гасим (значения не применяются).
        let manual_on = focus_core
            .map(|c| b.manual_strat_state(c).0)
            .unwrap_or(false);
        (
            b.follow,
            focus_core,
            size_values,
            size_sel,
            tp_str,
            tp_engaged,
            sl_str,
            sl_on,
            lev_str,
            sell_pcts,
            sell_slot,
            manual_on,
        )
    };
    let p = MoonPalette::active(cx);
    let tp_color = if p.is_light() { p.accent } else { p.blue };
    let sl_color = if p.is_light() { p.red_text } else { p.red };

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(design::fit_h_px(cx, TOOLBAR_H, 13.0, 9.5))
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .bg(rgb(p.shell_high))
        .border_b_1()
        .border_color(rgb(p.border));

    row = row
        // Размер ордера первым, без подписи «size» — s1-s6 говорят сами за себя.
        .child(size_strip(
            size_values,
            size_sel,
            // Редактируем инпутом только если запрос относится к ФОКУСНОМУ ядру тулбара.
            size_edit
                .filter(|(c, _)| Some(*c) == focus_core)
                .map(|(_, i)| i),
            size_input,
            backend.clone(),
            focus_core,
        ))
        .child(divider(p))
        // Плечо.
        .child(metric_button(
            TradeMetric::Lev,
            lev_str,
            p.text,
            61.6,
            open_metric == Some(TradeMetric::Lev),
            false,
            true,
            true,
            shell.clone(),
            p,
        ))
        .child(divider(p))
        // Стоп-лосс: тогл вкл/выкл (`panic_if_price_drop`) + кнопка только со значением+попапом.
        .child(sl_toggle(
            sl_on,
            manual_on,
            backend.clone(),
            group.to_string(),
        ))
        .child(metric_button(
            TradeMetric::Sl,
            sl_str,
            sl_color,
            58.0,
            open_metric == Some(TradeMetric::Sl),
            false,
            false,
            sl_on && !manual_on,
            shell.clone(),
            p,
        ))
        .child(divider(p))
        // TP + полоса S-слотов рядом (без подписи «sell»): это один и тот же sell-таргет, горит
        // что-то одно — либо TP, либо выбранный S-слот.
        .child(metric_button(
            TradeMetric::Tp,
            tp_str,
            tp_color,
            74.6,
            open_metric == Some(TradeMetric::Tp),
            tp_engaged && !manual_on,
            true,
            !manual_on,
            shell.clone(),
            p,
        ))
        .child({
            let strip = sell_strip(
                sell_pcts,
                // При ручной стратегии слоты не применяются — ничего не горит.
                sell_slot.filter(|_| !manual_on),
                // Редактируем S-инпутом только если запрос относится к ФОКУСНОМУ ядру тулбара.
                sell_edit
                    .filter(|(c, _)| Some(*c) == focus_core && !manual_on)
                    .map(|(_, i)| i),
                sell_input,
                backend.clone(),
                // Ручная стратегия: гасим взаимодействие (клик/колесо/дабл) целиком.
                focus_core.filter(|_| !manual_on),
            );
            if manual_on {
                div().opacity(0.55).child(strip).into_any_element()
            } else {
                strip.into_any_element()
            }
        })
        .child(divider(p));
    // Масштаб переехал в полоску чарт-вкладок (рядом с ⚙) и теперь per-вкладочный —
    // см. controls::scale_dropdown_for_tabs / chart_tabs::ChartTabs::pick_active_scale.

    let live_tone = if follow {
        if p.is_light() { p.green_text } else { p.green }
    } else {
        p.text_muted
    };
    let live_label = if follow {
        t!("toolbar.live").to_string()
    } else {
        t!("toolbar.pause").to_string()
    };
    let backend_live = backend.clone();
    row = row.child(
        MoonButton::new("live")
            .width(design::font_w(cx, 62.0))
            .variant(MoonButtonVariant::Soft)
            .size(MoonButtonSize::ToolbarCompact)
            .segment(
                MoonButtonSegment::new("●")
                    .color(live_tone)
                    .font_size(9.0)
                    .weight(700.0),
            )
            .segment(
                MoonButtonSegment::new(live_label)
                    .color(live_tone)
                    .weight(500.0),
            )
            .on_click(move |_, _, cx| {
                backend_live.update(cx, |b, bcx| {
                    b.follow = !b.follow;
                    bcx.notify();
                });
            })
            .render(),
    );
    // Правый край: «Стратегии»/«Скринер» (иконки) и «Настройки» (иконка+надпись) —
    // переехали из шапки, стиль как Live.
    row.child(div().flex_1())
        .child(open_window_button(
            "toolbar-strategies",
            t!("toolbar.strategies").to_string(),
            "icons/bot.svg",
            None,
            backend.clone(),
            crate::strategies::open,
            p,
        ))
        .child(open_window_button(
            "toolbar-screener",
            t!("toolbar.screener").to_string(),
            "icons/chart-pie.svg",
            None,
            backend.clone(),
            crate::screener::open,
            p,
        ))
        .child(open_window_button(
            "toolbar-settings",
            t!("shell.settings_btn").to_string(),
            "icons/settings.svg",
            // С надписью. Фикс-ширина вместо паддинга: pad_x у MoonButton = 0 (FORK_BUGS),
            // контент центрируется внутри заданной ширины.
            Some(design::font_w(cx, 92.0)),
            backend.clone(),
            crate::settings::open,
            p,
        ))
}

/// Кнопка тулбара, открывающая singleton-окно (Стратегии/Скринер/Настройки): визуал как
/// Live (Soft/ToolbarCompact). `labeled_width = None` — только иконка (имя тултипом),
/// `Some(w)` — иконка + надпись в фикс-ширине `w`. `open` — `strategies::open`/
/// `screener::open`/`settings::open` (одинаковая сигнатура: дедуп/фокус внутри).
/// Иконка icon-only чуть левее центра — баг MoonButton (см. FORK_BUGS), чинится в форке.
#[allow(clippy::too_many_arguments)]
fn open_window_button(
    id: &'static str,
    label: String,
    icon: &'static str,
    labeled_width: Option<f32>,
    backend: Entity<Backend>,
    open: fn(Entity<Backend>, Option<AnyWindowHandle>, Option<DisplayId>, &mut App),
    p: MoonPalette,
) -> impl IntoElement {
    let mut btn = MoonButton::new(id)
        .width(labeled_width.unwrap_or(30.0))
        .variant(MoonButtonVariant::Soft)
        .size(MoonButtonSize::ToolbarCompact)
        .leading_icon(MoonButtonIconSlot::new(icon).color(p.text_soft));
    btn = if labeled_width.is_some() {
        btn.text_segment(label, p.text, 500.0)
    } else {
        btn.tooltip(label)
    };
    btn.on_click(move |_, window, cx| {
        let owner_display = window.display(cx).map(|d| d.id());
        open(
            backend.clone(),
            Some(window.window_handle()),
            owner_display,
            cx,
        );
    })
    .render()
}
