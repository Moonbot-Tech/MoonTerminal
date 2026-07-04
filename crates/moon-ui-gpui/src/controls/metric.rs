//! Торговые метрики тулбара (TP/SL/Lev): кнопки-триггеры, тогл SL и контент попапов.
//! Вынесено из `controls.rs` точь-в-точь.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonInput, MoonInputState, MoonPalette, MoonSlider, MoonSliderState,
    MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex, v_flex,
};

use moon_core::feed::ClientSettingsEdit;

use super::TP_FINE_MAX;
use crate::shell::Shell;
use crate::{Backend, design};

/// Торговая метрика тулбара с собственным попапом (слайдер + поле ввода).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TradeMetric {
    Tp,
    Sl,
    Lev,
}

impl TradeMetric {
    fn id(self) -> &'static str {
        match self {
            TradeMetric::Tp => "toolbar-tp",
            TradeMetric::Sl => "toolbar-sl",
            TradeMetric::Lev => "toolbar-lev",
        }
    }

    fn label(self) -> &'static str {
        match self {
            TradeMetric::Tp => "TP",
            TradeMetric::Sl => "SL",
            TradeMetric::Lev => "Lev",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            TradeMetric::Lev => "×",
            _ => "%",
        }
    }

    fn title(self) -> String {
        match self {
            TradeMetric::Tp => t!("toolbar.tp_title").to_string(),
            TradeMetric::Sl => t!("toolbar.sl_title").to_string(),
            TradeMetric::Lev => t!("toolbar.lev_title").to_string(),
        }
    }

    /// Текущее значение метрики активного ядра (для сидирования слайдера/инпута при открытии).
    /// Lev зависит ОТ ЯДРА И ТЕКУЩЕЙ МОНЕТЫ: плечо рынка main-чарта из ассетов активного ядра.
    pub fn current(self, b: &Backend, group: &str) -> Option<f32> {
        let core = b.active_trade_core(group)?;
        let cd = b.session.store().core(core)?;
        match self {
            TradeMetric::Tp => cd
                .client_settings
                .as_ref()
                // «Свой» TP кнопки (не эффективный): выбор S-слота не должен подменять seed слайдера.
                .map(|s| s.take_profit_main_pct as f32),
            TradeMetric::Sl => cd.client_settings.as_ref().map(|s| s.stop_loss_pct),
            TradeMetric::Lev => {
                // Плечо монеты main-чарта из per-core карты (любой отслеживаемый рынок, не
                // только с позицией). Нет в карте → плечо неизвестно (покажем «—»).
                let (_, market) = b.main_chart_target(group)?;
                cd.assets.leverage.get(&market).map(|l| *l as f32)
            }
        }
    }
}

/// Кнопка-триггер торговой метрики. Клик открывает/закрывает её попап в `Shell`
/// (overlay со слайдером/полем; закрытие — как у попапа раскладки чарта).
#[allow(clippy::too_many_arguments)]
pub(super) fn metric_button(
    metric: TradeMetric,
    value_str: String,
    color: u32,
    width: f32,
    open: bool,
    engaged: bool,
    show_label: bool,
    enabled: bool,
    shell: Entity<Shell>,
    p: MoonPalette,
) -> impl IntoElement {
    // «Горит» = попап открыт ИЛИ метрика задействована (для TP — fixed_sell выключен). Так TP
    // и S-слоты дают взаимоисключающую подсветку: либо горит TP, либо один S-слот.
    // Неактивная кнопка (SL при выключенном тогле) не горит.
    let lit = enabled && (open || engaged);
    let mut btn = MoonButton::new(metric.id())
        .width(width)
        .variant(if lit {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Neutral
        })
        .size(MoonButtonSize::ToolbarCompact)
        .selected(lit)
        .disabled(!enabled);
    // Подпись метрики («SL»/«TP»/«Lev») — опционально: у SL она вынесена на тогл.
    if show_label {
        btn = btn.segment(
            MoonButtonSegment::new(metric.label())
                .color(p.text_muted)
                .weight(400.0),
        );
    }
    btn.text_segment(value_str, color, 500.0)
        .on_click(move |_, window, app| {
            shell.update(app, |this, cx| this.toggle_metric_popup(metric, window, cx));
        })
        .render()
}

/// Тогл включения стоп-лосса (`panic_if_price_drop`) слева от кнопки SL. Подпись «SL» вынесена
/// сюда из кнопки; выкл → кнопка SL неактивна (значение/попап только при включённом тогле).
pub(super) fn sl_toggle(on: bool, backend: Entity<Backend>, group: String) -> impl IntoElement {
    MoonToggle::new("toolbar-sl-toggle")
        .label("SL")
        .label_side(MoonToggleLabelSide::Left)
        .checked(on)
        .size(MoonToggleSize::Compact)
        .on_change(move |ch: &bool, _w, app| {
            let v = *ch;
            let b = backend.read(app);
            if let Some(core) = b.active_trade_core(&group) {
                if let Err(e) = b
                    .session
                    .edit_client_settings(core, ClientSettingsEdit::PanicIfPriceDrop(v))
                {
                    log::warn!("sl toggle failed: {e:#}");
                }
            }
        })
}

/// Контент попапа метрики (overlay-бокс со своим фоном/рамкой): заголовок + слайдер + поле;
/// для TP — ещё чекбокс расширенного диапазона `x_tmode`/«s9». Рисуется `Shell` поверх дока
/// на абсолютной позиции под кнопкой. `slider` уже выбран вызывающим (для TP — обычный/
/// расширенный по `extended`).
#[allow(clippy::too_many_arguments)]
pub fn metric_popup_content(
    metric: TradeMetric,
    slider: &Entity<MoonSliderState>,
    fine_slider: &Entity<MoonSliderState>,
    input: &Entity<MoonInputState>,
    extended: bool,
    hedge_on: bool,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut content = v_flex()
        .w(px(220.0))
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(metric.title()),
        )
        .child(
            MoonSlider::new(slider)
                .id(format!("{}-slider", metric.id()))
                .height(18.0),
        )
        .child(
            h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(
                    div().w(px(72.0)).child(
                        MoonInput::new(SharedString::from(format!("{}-input", metric.id())))
                            .state(input)
                            .small(),
                    ),
                )
                .child(div().text_color(rgb(p.text_muted)).child(metric.unit())),
        );

    if matches!(metric, TradeMetric::Tp) {
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-tp-ext")
                .label(t!("toolbar.tp_ext").to_string())
                .checked(extended)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let ext = *ch;
                    let b = backend.read(app);
                    let Some(core) = b.active_trade_core(&group) else {
                        return;
                    };
                    let cur = b
                        .session
                        .store()
                        .core(core)
                        .and_then(|d| d.client_settings.as_ref())
                        .map(|s| s.take_profit_main_pct)
                        .unwrap_or(0.0);
                    if let Err(error) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::TakeProfit {
                            pct: cur,
                            extended: ext,
                        },
                    ) {
                        log::warn!("tp extended toggle failed: {error}");
                    }
                }),
        );
        // Файн-слайдер: суб-процентный TP (0..2, шаг 0.01) через scalp. Активен ТОЛЬКО когда
        // верхний TP на минимуме (=2, без галки ×10); поднял верхний выше 2 — нижний disabled.
        let coarse_tp = slider.read(cx).value().end();
        let fine_enabled = !extended && coarse_tp <= TP_FINE_MAX + 0.001;
        content = content
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .opacity(if fine_enabled { 1.0 } else { 0.4 })
                    .child(t!("toolbar.tp_fine").to_string()),
            )
            .child(
                MoonSlider::new(fine_slider)
                    .id("toolbar-tp-fine-slider")
                    .disabled(!fine_enabled)
                    .height(18.0),
            );
    }

    if matches!(metric, TradeMetric::Sl) {
        // Стоп-маркет: при срабатывании стопа продавать РЫНОЧНЫМ ордером, а не стоп-лимитом
        // (`use_stop_market`). Перенесён сюда из попапа настроек ядра.
        let stop_market_on = {
            let b = backend.read(cx);
            b.active_trade_core(group)
                .and_then(|c| b.session.store().core(c))
                .and_then(|d| d.client_settings.as_ref())
                .map(|s| s.use_stop_market)
                .unwrap_or(false)
        };
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-stop-market")
                .label(t!("toolbar.stop_market").to_string())
                .checked(stop_market_on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let on = *ch;
                    let b = backend.read(app);
                    if let Some(core) = b.active_trade_core(&group) {
                        if let Err(error) = b
                            .session
                            .edit_client_settings(core, ClientSettingsEdit::UseStopMarket(on))
                        {
                            log::warn!("stop market toggle failed: {error}");
                        }
                    }
                }),
        );
    }

    if matches!(metric, TradeMetric::Lev) {
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-hedge")
                .label(t!("toolbar.hedge").to_string())
                .checked(hedge_on)
                .size(MoonCheckboxSize::Compact)
                .on_change({
                    let backend = backend.clone();
                    let group = group.clone();
                    move |ch: &bool, _w, app| {
                        let on = *ch;
                        let b = backend.read(app);
                        let Some(core) = b.active_trade_core(&group) else {
                            return;
                        };
                        if let Err(error) = b.session.set_hedge_mode(core, on) {
                            log::warn!("set hedge mode failed: {error}");
                        }
                    }
                }),
        );
        // Плечо — биржевое действие: применяем ТОЛЬКО по этой кнопке (слайдер/поле лишь
        // выбирают значение, на драг ничего не шлётся). Значение берём из поля (его живо
        // обновляет драг слайдера, и в него можно ввести точное число). Шлём per-market
        // Engine `set_leverage` для монеты main-чарта (той, чьё плечо и показываем) — НЕ
        // глобальный LevManage-снапшот: его ядро не присылает, и правка молча терялась.
        let input = input.clone();
        content = content.child(
            MoonButton::new("toolbar-lev-apply")
                .label(t!("toolbar.apply").to_string())
                .variant(MoonButtonVariant::Blue)
                .size(MoonButtonSize::ToolbarCompact)
                .full_width()
                .on_click(move |_, _w, app| {
                    let Ok(v) = input.read(app).value().trim().parse::<i32>() else {
                        return;
                    };
                    let b = backend.read(app);
                    let Some(core) = b.active_trade_core(&group) else {
                        return;
                    };
                    let Some((_, market)) = b.main_chart_target(&group) else {
                        return;
                    };
                    if let Err(error) = b.session.set_leverage(core, market, v) {
                        log::warn!("apply leverage failed: {error:#}");
                    }
                })
                .render(),
        );
    }
    content.into_any_element()
}
