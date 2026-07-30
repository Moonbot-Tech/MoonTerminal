//! Toolbar trading metrics (TP/SL/Lev): anchored popup triggers, the SL toggle, popup content, and
//! target identities that keep group exits local while leverage stays bound to one core and market.

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

/// Toolbar trading metric with its own slider-and-input popup.
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
    /// fields, so the toolbar's TP and SL would not reach a new order. Leverage requires an active
    /// core; group-local TP and SL always have a complete neutral-or-user-edited generation.
    pub fn available_with(self, has_core: bool, sl_on: bool, manual_on: bool) -> bool {
        match self {
            TradeMetric::Lev => has_core,
            TradeMetric::Tp => !manual_on,
            TradeMetric::Sl => sl_on && !manual_on,
        }
    }

    /// [`Self::available_with`] for `Shell`, using the same hover-aware manual core as the toolbar.
    pub fn available(self, b: &Backend, group: &str, manual_core: Option<CoreId>) -> bool {
        let core = b.active_trade_core(group);
        let exit = b.group_exit_settings(group);
        let manual_on = manual_core
            .map(|core| b.manual_strat_state(core).0)
            .unwrap_or(false);
        self.available_with(core.is_some(), exit.stop_loss_enabled, manual_on)
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
        match self {
            // Leverage is stored per (core, MARKET) and applied per market, so the coin on the Main
            // chart is part of the address, not context.
            TradeMetric::Lev => {
                let core = b.active_trade_core(group)?;
                b.main_chart_target(group).map(|(_, market)| MetricTarget {
                    core: Some(core),
                    market: Some(market),
                })
            }
            TradeMetric::Tp | TradeMetric::Sl => Some(MetricTarget {
                core: None,
                market: None,
            }),
        }
    }

    /// Return the current group or core value for seeding this metric's slider and input.
    ///
    /// Leverage depends on both the core and the Main chart's current market and is read from the
    /// active core's asset state.
    ///
    /// Args:
    ///     b: Backend providing group exits plus the active trading core and its state.
    ///     group: Window group used directly for exits and to resolve the leverage target.
    ///
    /// Returns:
    ///     The current metric value, or `None` when the leverage target is absent.
    pub fn current(self, b: &Backend, group: &str) -> Option<f32> {
        match self {
            TradeMetric::Tp => Some(b.group_exit_settings(group).take_profit_pct as f32),
            TradeMetric::Sl => Some(b.group_exit_settings(group).stop_loss_pct),
            TradeMetric::Lev => {
                let core = b.active_trade_core(group)?;
                // Read the Main chart market's leverage from the per-core map, which includes every
                // tracked market rather than only open positions. Absence means unknown leverage and
                // renders as a dash.
                let (_, market) = b.main_chart_target(group)?;
                b.session
                    .store()
                    .core(core)?
                    .assets
                    .leverage
                    .get(&market)
                    .map(|l| *l as f32)
            }
        }
    }
}

/// Everything an open metric popup's edits are ADDRESSED TO — recorded when it opens, re-checked
/// before every write.
///
/// TP and SL use `(None, None)` because their address is the containing group, which does not change
/// when its active core changes. Leverage records `(core, market)` so a chart switch cannot apply a
/// stale value to the wrong exchange target.
///
/// Checking only at render is not enough on its own: repaints pass three stacked throttles, so a
/// slider drag can fire in the window between the address moving and the popup being taken down.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MetricTarget {
    /// Core that owns leverage; `None` for group-local exit metrics.
    pub core: Option<CoreId>,
    /// Market that owns leverage; `None` for group-local exit metrics.
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
/// Unscaled: `MoonPopover::content_width_font` applies the font scale before it adds the
/// component-owned popup padding and border.
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
    _cx: &App,
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
        .content_width_font(POPUP_CONTENT_W)
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

/// Build the `panic_if_price_drop` toggle to the left of the SL button.
///
/// The toggle owns the `SL` label. When off, the adjacent value and popup button are disabled.
/// `disabled` represents manual-strategy mode, in which toolbar SL does not apply to new orders.
///
/// Args:
///     on: Current `panic_if_price_drop` value.
///     disabled: Whether manual-strategy mode prevents editing toolbar SL.
///     backend: Backend that owns the group-local exit settings.
///     group: Window group whose local SL toggle receives the edit.
///
/// Returns:
///     The configured SL toggle element.
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
            backend.update(app, |b, _| {
                b.edit_group_exit(&group, ClientSettingsEdit::PanicIfPriceDrop(v));
            });
        })
}

/// Build a metric popup's heading, slider, and input; TP also gets its `x_tmode`/`s9` extended-range
/// checkbox. The caller selects the normal or extended TP slider through `extended`.
///
/// This returns content only. `MoonPopover` supplies the background, border, radius, padding, and
/// width; drawing them here would create a second frame inside the anchored popup.
///
/// `target` is the address the popup was seeded from. Every control re-checks that address before
/// writing, so group exits survive a core switch while leverage drops a stale event — see
/// [`MetricTarget`].
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
    // Chrome is MoonPopover's; see `popover_contents_do_not_paint_a_second_surface`.
    let mut content = v_flex()
        .id("metric-popup-content")
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
                    let is_live = {
                        let b = backend.read(app);
                        target.is_live(TradeMetric::Tp, b, &group)
                    };
                    if !is_live {
                        return;
                    }
                    backend.update(app, |b, _| {
                        let cur = b.group_exit_settings(&group).take_profit_pct;
                        b.edit_group_exit(
                            &group,
                            ClientSettingsEdit::TakeProfit {
                                pct: cur,
                                extended: ext,
                            },
                        );
                    });
                }),
        );
        // The fine slider controls scalp TP values from 0 to `TP_FINE_CAP` (1.99) in 0.01 steps.
        // It is enabled only at the coarse slider's 2.0 boundary without the x10 option; 2.0 itself
        // belongs to the coarse/main TP path, and raising the coarse value above it disables fine TP.
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
        // `use_stop_market` sells with a market order when the stop triggers instead of placing a
        // stop-limit order. This control moved here from the core-settings popup.
        let stop_market_on = {
            let b = backend.read(cx);
            b.group_exit_settings(group).use_stop_market
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
                    let is_live = {
                        let b = backend.read(app);
                        target.is_live(TradeMetric::Sl, b, &group)
                    };
                    if !is_live {
                        return;
                    }
                    backend.update(app, |b, _| {
                        b.edit_group_exit(&group, ClientSettingsEdit::UseStopMarket(on));
                    });
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
                        let Some(core) = target.core else {
                            return;
                        };
                        if let Err(error) = b.session.set_hedge_mode(core, on) {
                            log::warn!("set hedge mode failed: {error}");
                        }
                    }
                }),
        );
        // Leverage is an exchange action and is applied only by this button; the slider and field
        // merely choose a value and dragging sends nothing. Read the value from the field, which the
        // slider updates live and which also accepts an exact number. Send per-market Engine
        // `set_leverage` for the Main chart coin whose leverage is displayed, not the global
        // LevManage snapshot that the core does not send and whose edit was silently lost.
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
                    let Some(core) = target.core else {
                        return;
                    };
                    let Some(market) = target.market.clone() else {
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

#[cfg(test)]
mod tests;
