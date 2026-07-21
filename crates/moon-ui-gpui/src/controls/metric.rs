//! Toolbar trading metrics (TP/SL/Lev): anchored popup triggers, the SL toggle, popup content, and
//! the target identity that keeps edits bound to the core and market from which they were seeded.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonInput, MoonInputState, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonSlider, MoonSliderState, MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex, v_flex,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use super::TP_FINE_MAX;
use crate::shell::Shell;
use crate::{Backend, design};

/// Торговая метрика тулбара с собственным попапом (слайдер + поле ввода).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

    /// Element id of this metric's popover. A `&'static str` like [`Self::id`] rather than a
    /// `format!`, which would allocate three strings on every frame for three constants.
    fn popover_id(self) -> &'static str {
        match self {
            TradeMetric::Tp => "toolbar-tp-popover",
            TradeMetric::Sl => "toolbar-sl-popover",
            TradeMetric::Lev => "toolbar-lev-popover",
        }
    }

    /// Whether the button draws this metric's own label beside its value. SL's label lives on the
    /// toggle standing next to it, so the button would repeat it.
    fn shows_label(self) -> bool {
        !matches!(self, TradeMetric::Sl)
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

    /// Whether this metric can be edited right now, from state the caller already holds.
    ///
    /// The ONE home of the predicate, kept pure so the two callers cannot drift: the toolbar
    /// disables the button with it, and `Shell` drops a popup whose metric went unavailable WHILE
    /// IT WAS OPEN — the SL toggle beside it switched off, or the manual strategy armed. A second
    /// copy would let those disagree, and the disagreement is invisible until it strands an open
    /// popup: `MoonPopover` renders a disabled trigger and nothing else, so the popup vanishes with
    /// no `on_open_change` to tell anyone, and re-enabling the metric would pop it back up
    /// unclicked, holding stale slider values.
    ///
    /// `manual_on` is the manual strategy: the core then takes sell and stop levels from ITS
    /// fields, so the toolbar's TP and SL would not reach a new order. Leverage is not overridden
    /// by the manual strategy, but like the other two it is meaningless without a core: every edit
    /// these popups make is addressed to one, so `has_core` gates all three.
    pub fn available_with(self, has_core: bool, sl_on: bool, manual_on: bool) -> bool {
        has_core
            && match self {
                TradeMetric::Lev => true,
                TradeMetric::Tp => !manual_on,
                TradeMetric::Sl => sl_on && !manual_on,
            }
    }

    /// [`Self::available_with`] for a caller holding only the backend — `Shell`, which keeps no
    /// trading state of its own. The toolbar does NOT go through here: it already read every flag
    /// while building the row, and `active_trade_core` is a session scan it would otherwise repeat
    /// once per metric on every frame.
    pub fn available(self, b: &Backend, group: &str) -> bool {
        let core = b.active_trade_core(group);
        let sl_on = core
            .and_then(|c| b.session.store().core(c))
            .and_then(|d| d.client_settings.as_ref())
            .map(|s| s.panic_if_price_drop)
            .unwrap_or(false);
        let manual_on = core.map(|c| b.manual_strat_state(c).0).unwrap_or(false);
        self.available_with(core.is_some(), sl_on, manual_on)
    }

    /// The value this metric's popup would seed from, or `None` when there is none to show.
    ///
    /// [`Self::current`] with the row's own display rule applied, so that the button, the popup and
    /// the guard against opening one cannot disagree: a leverage of 0 means "not set" and renders as
    /// a dash, and a popup must not open on a value the row refuses to state.
    pub fn seed_value(self, b: &Backend, group: &str) -> Option<f32> {
        let value = self.current(b, group)?;
        match self {
            TradeMetric::Lev => (value > 0.0).then_some(value),
            _ => Some(value),
        }
    }

    /// Where an edit from this metric's popup is addressed right now, or `None` if nowhere.
    pub fn target(self, b: &Backend, group: &str) -> Option<MetricTarget> {
        let core = b.active_trade_core(group)?;
        match self {
            // Leverage is stored per (core, MARKET) and applied per market, so the coin on the Main
            // chart is part of the address, not context.
            TradeMetric::Lev => b.main_chart_target(group).map(|(_, market)| MetricTarget {
                core,
                market: Some(market),
            }),
            _ => Some(MetricTarget { core, market: None }),
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

/// Everything an open metric popup's edits are ADDRESSED TO — recorded when it opens, re-checked
/// before every write.
///
/// A popup seeds its slider and field from one place and its handlers resolve that place again when
/// the event fires. Between those two moments the address can move without a click anyone would
/// read as dismissing the popup: a hotkey or a chart tab changes the Main chart, which is both what
/// `active_trade_core` falls back to and where leverage takes its market. Comparing the seeded
/// address against the live one before each write stops a popup for core A, or BTC leverage, from
/// continuing to modify that old target after the visible trading context has moved elsewhere.
///
/// Checking only at render is not enough on its own: repaints pass three stacked throttles, so a
/// slider drag can fire in the window between the address moving and the popup being taken down.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MetricTarget {
    /// Core that owned the value when the popup was opened.
    pub core: CoreId,
    /// Only leverage is per-market; `None` for the per-core metrics.
    pub market: Option<String>,
}

impl MetricTarget {
    /// Whether this seeded address is still the one `metric` resolves to — the event-time guard.
    pub fn is_live(&self, metric: TradeMetric, b: &Backend, group: &str) -> bool {
        metric.target(b, group).as_ref() == Some(self)
    }
}

/// Base width of a metric popup's content — slider, field, checkboxes.
///
/// Unscaled: it goes through `design::font_w` because the content includes labels and inputs that
/// grow with the Font slider, while `MoonPopover::width` puts its argument into `px(..)` verbatim.
const POPUP_CONTENT_W: f32 = 220.0;

/// A trading-metric button together with its anchored popup.
///
/// The button IS the `MoonPopover` trigger, so the popup sits under its own button by construction
/// and no toolbar layout change can detach it.
///
/// `popup` comes from `Shell` and only for the OPEN metric: the content owns that metric's sliders
/// and input, and `AnyElement` does not clone.
#[allow(clippy::too_many_arguments)]
pub(super) fn metric_button(
    metric: TradeMetric,
    value_str: String,
    color: u32,
    width: f32,
    open: bool,
    engaged: bool,
    enabled: bool,
    popup: Option<AnyElement>,
    shell: Entity<Shell>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    // "Lit" = the popup is open OR the metric is engaged (for TP: fixed-sell is off). That is what
    // makes TP and the S slots mutually exclusive — either TP is lit or exactly one S slot is. A
    // disabled button (SL with its toggle off) never lights up.
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
    if metric.shows_label() {
        btn = btn.segment(
            MoonButtonSegment::new(metric.label())
                .color(p.text_muted)
                .weight(400.0),
        );
    }
    let trigger = btn.text_segment(value_str, color, 500.0).render();
    MoonPopover::new(SharedString::from(metric.popover_id()))
        .placement(MoonPopoverPlacement::BottomStart)
        .width(design::popover_outer_width(
            cx,
            design::font_w(cx, POPUP_CONTENT_W),
        ))
        // A disabled metric (SL with its toggle off) does not open: there is nothing to edit.
        //
        // Guarding `open` with `enabled` here would be inert — `MoonPopover::render` returns the
        // bare trigger on `disabled` BEFORE it consults the controlled `open`. The case it looked
        // like it covered (the metric goes unavailable while its popup is open) is handled where it
        // can actually be handled: `Shell` drops the open state via [`TradeMetric::available`].
        .disabled(!enabled)
        .open(open)
        // A click on the CONTENT must not close it: inside are sliders, checkboxes and an Apply
        // button, and closing on the first click would put all of them out of reach.
        .close_on_content_click(false)
        .on_open_change(move |open, window, cx| {
            shell.update(cx, |s, cx| {
                s.set_metric_popup_open(metric, open, window, cx);
            });
        })
        .trigger(trigger)
        .content(popup.unwrap_or_else(|| div().into_any_element()))
}

/// Тогл включения стоп-лосса (`panic_if_price_drop`) слева от кнопки SL. Подпись «SL» вынесена
/// сюда из кнопки; выкл → кнопка SL неактивна (значение/попап только при включённом тогле).
/// `disabled` — режим ручной стратегии: SL тулбара к новым ордерам не применяется.
pub(super) fn sl_toggle(
    on: bool,
    disabled: bool,
    backend: Entity<Backend>,
    group: String,
) -> impl IntoElement {
    MoonToggle::new("toolbar-sl-toggle")
        .label("SL")
        .label_side(MoonToggleLabelSide::Left)
        .checked(on)
        .size(MoonToggleSize::Compact)
        .disabled(disabled)
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

/// Build a metric popup's heading, slider, and input; TP also gets its `x_tmode`/`s9` extended-range
/// checkbox. The caller selects the normal or extended TP slider through `extended`.
///
/// This returns content only. `MoonPopover` supplies the background, border, radius, padding, and
/// width; drawing them here would create a second frame inside the anchored popup.
///
/// `target` is the address the popup was seeded from. Every control in here writes to a core (and,
/// for leverage, a market) resolved when its event fires, so each one re-checks that address first
/// and drops the event if it has moved — see [`MetricTarget`].
#[allow(clippy::too_many_arguments)]
pub fn metric_popup_content(
    metric: TradeMetric,
    target: &MetricTarget,
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
        .gap(design::ui_px(cx, design::CHROME_GAP))
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
        let target = target.clone();
        content = content.child(
            MoonCheckbox::new("toolbar-tp-ext")
                .label(t!("toolbar.tp_ext").to_string())
                .checked(extended)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let ext = *ch;
                    let b = backend.read(app);
                    if !target.is_live(TradeMetric::Tp, b, &group) {
                        return;
                    }
                    let core = target.core;
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
        let target = target.clone();
        content = content.child(
            MoonCheckbox::new("toolbar-stop-market")
                .label(t!("toolbar.stop_market").to_string())
                .checked(stop_market_on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let on = *ch;
                    let b = backend.read(app);
                    if !target.is_live(TradeMetric::Sl, b, &group) {
                        return;
                    }
                    if let Err(error) = b
                        .session
                        .edit_client_settings(target.core, ClientSettingsEdit::UseStopMarket(on))
                    {
                        log::warn!("stop market toggle failed: {error}");
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
                    let target = target.clone();
                    move |ch: &bool, _w, app| {
                        let on = *ch;
                        let b = backend.read(app);
                        if !target.is_live(TradeMetric::Lev, b, &group) {
                            return;
                        }
                        if let Err(error) = b.session.set_hedge_mode(target.core, on) {
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
        //
        // The market is taken from the SEEDED address, not resolved afresh: this is the one control
        // here whose address includes a coin, and the Main chart can move to another one while the
        // popup stands open — applying the leverage shown for BTC to ETH is a real exchange write.
        let input = input.clone();
        let target = target.clone();
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
                    if !target.is_live(TradeMetric::Lev, b, &group) {
                        return;
                    }
                    let Some(market) = target.market.clone() else {
                        return;
                    };
                    if let Err(error) = b.session.set_leverage(target.core, market, v) {
                        log::warn!("apply leverage failed: {error:#}");
                    }
                })
                .render(),
        );
    }
    content.into_any_element()
}

#[cfg(test)]
mod tests;
