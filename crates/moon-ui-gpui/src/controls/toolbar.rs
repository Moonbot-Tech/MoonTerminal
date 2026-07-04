//! Сборка полосы тулбара (TP/S-слоты/SL/Lev/size/Live). Вынесено из `controls.rs` точь-в-точь.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonInputState,
    MoonPalette, h_flex,
};

use moon_core::session::CoreId;

use super::metric::{metric_button, sl_toggle};
use super::strips::{SIZE_SEL_DEFAULT, divider, sell_strip, size_strip, strip_label};
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
        // TP + полоса S-слотов рядом (без подписи «sell»): это один и тот же sell-таргет, горит
        // что-то одно — либо TP, либо выбранный S-слот.
        .child(metric_button(
            TradeMetric::Tp,
            tp_str,
            tp_color,
            74.6,
            open_metric == Some(TradeMetric::Tp),
            tp_engaged,
            true,
            true,
            shell.clone(),
            p,
        ))
        .child(sell_strip(
            sell_pcts,
            sell_slot,
            // Редактируем S-инпутом только если запрос относится к ФОКУСНОМУ ядру тулбара.
            sell_edit
                .filter(|(c, _)| Some(*c) == focus_core)
                .map(|(_, i)| i),
            sell_input,
            backend.clone(),
            focus_core,
        ))
        .child(divider(p))
        // Стоп-лосс: тогл вкл/выкл (`panic_if_price_drop`) + кнопка только со значением+попапом.
        .child(sl_toggle(sl_on, backend.clone(), group.to_string()))
        .child(metric_button(
            TradeMetric::Sl,
            sl_str,
            sl_color,
            58.0,
            open_metric == Some(TradeMetric::Sl),
            false,
            false,
            sl_on,
            shell.clone(),
            p,
        ))
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
        .child(strip_label("size", p, cx))
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
    let backend = backend.clone();
    row.child(
        MoonButton::new("live")
            .width(62.0)
            .variant(MoonButtonVariant::Soft)
            .size(MoonButtonSize::ToolbarCompact)
            .segment(
                MoonButtonSegment::new("●")
                    .color(live_tone)
                    .font_size(8.0)
                    .weight(700.0),
            )
            .segment(
                MoonButtonSegment::new(live_label)
                    .color(live_tone)
                    .weight(500.0),
            )
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    b.follow = !b.follow;
                    bcx.notify();
                });
            })
            .render(),
    )
}
