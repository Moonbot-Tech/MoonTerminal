//! Toolbar trading metrics (TP/SL/Lev): anchored popup triggers, the SL toggle, popup content, and
//! target identities that keep group exits local while leverage, when the scope names one core,
//! stays bound to that core and market.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonInput, MoonInputState, MoonPalette, MoonPopover, MoonPopoverPlacement,
    MoonSlider, MoonSliderState, MoonToggle, MoonToggleLabelSide, MoonToggleSize, h_flex, v_flex,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::market::MarketLimits;
use moon_core::session::CoreId;

use super::{DASH, LEV_PRESETS, MaxOrderReadout, TP_FINE_MAX, lev_preset_available};
use crate::shell::Shell;
use crate::{Backend, design};
use moon_core::util::fmt;

/// The core a leverage read or edit is addressed to, or `None` while the visible scope names none.
///
/// [`Backend::active_trade_core`] deliberately never answers `None` for a group with live cores —
/// order placement and the gear popover need a concrete address — so in the Auto workspace
/// Overview it falls through to the group's FIRST core. Every leverage surface reads through HERE
/// instead, so the button, the popup's open guard and `Shell::reconcile_metric_popup` all reach
/// the same conclusion; a copy of this gate at only some of them is how a popup opened over one
/// core survives a switch into Overview and then Applies to a server the user never chose.
///
/// Args:
///     b: Backend providing the workspace scope and the active trade core.
///     group: Group whose leverage control is being resolved.
///
/// Returns:
///     The active trade core, or `None` in Auto Overview.
fn scoped_lev_core(b: &Backend, group: &str) -> Option<CoreId> {
    if b.is_auto_overview_scope(group) {
        None
    } else {
        b.active_trade_core(group)
    }
}

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
    /// fields, so the toolbar's TP and SL would not reach a new order. Leverage requires a core
    /// named by the visible scope; group-local TP and SL always have a complete
    /// neutral-or-user-edited generation.
    /// Manual-strategy mode closes the TP popup ALONE.
    ///
    /// TP there would be a free-form take profit with nowhere to go: the strategy sells at one
    /// value, and the S presets are the control that changes it. The STOP is different — it exists
    /// in the strategy as a level plus an on/off flag, exactly the two things this button and its
    /// toggle edit, so in manual mode they keep working and write to the strategy instead of to the
    /// manual-strategy overlay (`Backend::manual_exit_overlay`), which is also what the order
    /// carries.
    pub fn available_with(self, has_core: bool, sl_on: bool, manual_on: bool) -> bool {
        match self {
            TradeMetric::Lev => has_core,
            TradeMetric::Tp => !manual_on,
            TradeMetric::Sl => sl_on,
        }
    }

    /// [`Self::available_with`] for `Shell`, using the scope-gated leverage core and the same
    /// hover-aware manual core as the toolbar.
    pub fn available(self, b: &Backend, group: &str, manual_core: Option<CoreId>) -> bool {
        let core = scoped_lev_core(b, group);
        let manual_on = manual_core
            .map(|core| b.manual_strat_active(core).is_some())
            .unwrap_or(false);
        // The same source the toolbar's own SL button renders from: with a manual strategy in force
        // that is the overlay, and reading the saved generation here made the row and the button
        // beside it disagree about whether the stop is even on.
        let sl_on = manual_core
            .and_then(|core| b.manual_exit_overlay(core))
            .map(|ms| ms.stop_on)
            .unwrap_or_else(|| b.group_exit_settings(group).stop_loss_enabled);
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
    ///
    /// Leverage uses the scope-gated core, so Auto Overview has no editable per-core target.
    pub fn target(self, b: &Backend, group: &str) -> Option<MetricTarget> {
        match self {
            // Leverage is stored per (core, MARKET) and applied per market, so the coin on the Main
            // chart is part of the address, not context.
            TradeMetric::Lev => {
                let core = scoped_lev_core(b, group)?;
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
    /// Leverage depends on both the scope-gated core and the Main chart's current market and is
    /// read from that core's asset state. TP and SL read through
    /// [`Backend::write_aligned_group_exit`] rather than the plain group-local getter: this seed
    /// feeds a popup whose edits go out through `edit_group_exit`, so it must be resolved against
    /// the SAME source that write will target (goal A2 FIX-2) — a core opted into the per-core
    /// route must not have its popup seeded from the group's generation.
    ///
    /// Args:
    ///     b: Backend providing group exits plus the scope-gated leverage core and its state.
    ///     group: Window group used directly for exits and to resolve the leverage target.
    ///
    /// Returns:
    ///     The current metric value, or `None` when the leverage target is absent.
    pub fn current(self, b: &Backend, group: &str) -> Option<f32> {
        // While a manual strategy owns the exits, the popup must open on the value the button next
        // to it shows and the order will use — the overlay — not on the saved generation sitting
        // underneath it.
        let manual = b
            .active_trade_core(group)
            .and_then(|core| b.manual_exit_overlay(core));
        match self {
            TradeMetric::Tp => Some(
                manual
                    .map(|ms| ms.take_profit_pct)
                    .unwrap_or_else(|| b.write_aligned_group_exit(group).take_profit_pct)
                    as f32,
            ),
            TradeMetric::Sl => Some(
                manual
                    .map(|ms| ms.stop_pct)
                    .unwrap_or_else(|| b.write_aligned_group_exit(group).stop_loss_pct),
            ),
            TradeMetric::Lev => {
                let core = scoped_lev_core(b, group)?;
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

/// The open metric popup: which metric, where its edits are addressed, and what the leverage
/// slider's range was seeded from.
///
/// ONE struct rather than the metric and its address in separate fields, because the seeded slider
/// range must die at exactly the moment the popup does. Three related values cleared together in three
/// different places is precisely the drift [`MetricTarget`]'s own comment warns about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenMetricPopup {
    /// The metric whose editor is on screen.
    pub metric: TradeMetric,
    /// Where this popup's edits are addressed — re-checked before every write.
    pub target: MetricTarget,
    /// The coin maximum leverage the slider range was seeded from; `0` when it was unknown.
    ///
    /// Recorded because the range is seeded ONCE, at open: rewriting a slider's bounds underneath a
    /// live drag is worse than leaving them, so when the exchange revises this coin's cap while the
    /// popup stands open the popup SAYS the range is stale instead of silently moving. Everything
    /// else in the popup — the printed limits and which presets are offered — is read per render
    /// and stays current.
    pub lev_coin_max: i32,
}

/// One `label ... value` line of the limits readout, with the reason on hover.
///
/// Composed from the popup's own `h_flex` + `div` idiom rather than a list widget, so the two lines
/// sit in the same visual system as the title and unit rows already beside them.
///
/// Args:
///     label: Localized label naming the limit.
///     value: Formatted limit value displayed opposite the label.
///     tooltip: Localized explanation of the value's provenance.
///     p: Active palette supplying caption and value colors.
///     cx: Application context supplying the caption size and UI scale.
///
/// Returns:
///     One flex row with a hover explanation for the displayed limit.
fn limit_row(
    label: String,
    value: String,
    tooltip: String,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .id(SharedString::from(format!("metric-limit-{label}")))
        .justify_between()
        .gap(design::ui_px(cx, 6.0))
        .text_size(design::t_caption(cx))
        .child(div().text_color(rgb(p.text_muted)).child(label))
        .child(div().text_color(rgb(p.text)).child(value))
        .tooltip(crate::panels::common::text_tooltip(tooltip))
}

/// A wrapped caption stating something the numbers above cannot say for themselves.
///
/// Args:
///     text: Localized explanation of the limit state.
///     p: Active palette supplying the muted caption color.
///     cx: Application context supplying the caption size and UI scale.
///
/// Returns:
///     A muted caption element for the limits readout.
fn limit_note(text: String, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .text_size(design::t_caption(cx))
        .text_color(rgb(p.text_muted))
        .child(text)
}

/// The leverage currently in the popup's field, which is what Apply would send.
///
/// Read from the FIELD rather than from account state because that is the value at risk: the
/// over-limit caption must describe what pressing Apply would do, not what the exchange last said.
///
/// Args:
///     input: Popup field containing the leverage value Apply would send.
///     cx: Application context used to read the field state.
///
/// Returns:
///     The parsed leverage, or zero when the field is not a valid number.
fn current_lev(input: &Entity<MoonInputState>, cx: &App) -> f32 {
    input.read(cx).value().trim().parse::<f32>().unwrap_or(0.0)
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
/// `open` carries the address the popup was seeded from. Every control re-checks that address before
/// writing, so group exits survive a core switch while leverage drops a stale event — see
/// [`MetricTarget`].
///
/// Args:
///     open: Recorded metric, edit address, and seeded leverage slider cap.
///     limits: Fresh exchange limits for the popup's seeded leverage address.
///     quote: Quote token used beside a displayed maximum order size.
///     slider: Main slider state for the selected metric.
///     fine_slider: Fine TP slider state used only by TP.
///     input: Numeric field state whose leverage value Apply sends.
///     extended: Whether TP uses its extended range.
///     hedge_on: Current hedge-mode state for the leverage popup.
///     backend: Shared terminal state used by popup controls.
///     group: Window group receiving group-local metric edits.
///     p: Active palette for popup elements.
///     cx: Application context used to render and read state.
///
/// Returns:
///     The configured popup content for the recorded metric.
#[allow(clippy::too_many_arguments)]
pub fn metric_popup_content(
    open: &OpenMetricPopup,
    limits: Option<MarketLimits>,
    quote: &str,
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
    let metric = open.metric;
    let target = &open.target;
    let is_lev = matches!(metric, TradeMetric::Lev);
    // The coin's own maximum, read fresh every render. Only the SLIDER RANGE is frozen at open
    // (`OpenMetricPopup::lev_coin_max`); what the popup STATES and which presets it offers stay
    // current, so a revised exchange limit is never presented as still applying.
    let coin_max = limits.map(|l| l.max_leverage).unwrap_or(0);

    // Exchange limits for the coin on screen: what it may be traded at, above the control that
    // chooses it. Placed before the slider on purpose — a cap read AFTER moving the slider is a
    // cap read too late.
    let lev_limits = is_lev.then(|| {
        let readout = MaxOrderReadout::of(limits);
        let max_order_text = readout.format(fmt::usd_grouped, quote);
        let (lev_text, lev_tip) = if coin_max > 0 {
            (format!("×{coin_max}"), "toolbar.lev_max_tip")
        } else if limits.is_some() {
            // The row states the FACT — the exchange named no maximum. The note below states the
            // CONSEQUENCE, that the slider therefore falls back to a default range. Two different
            // sentences on purpose: repeating one of them in both places reads as a stutter.
            (DASH.to_string(), "toolbar.lev_max_none")
        } else {
            (DASH.to_string(), "toolbar.limits_unknown")
        };
        v_flex()
            .gap(design::ui_px(cx, 2.0))
            .child(limit_row(
                t!("toolbar.max_order").to_string(),
                max_order_text,
                t!(readout.tooltip_key()).to_string(),
                p,
                cx,
            ))
            .child(limit_row(
                t!("toolbar.lev_max").to_string(),
                lev_text,
                t!(lev_tip).to_string(),
                p,
                cx,
            ))
            // Said out loud rather than left to be inferred from a slider that happens to end at
            // 125: without a stated maximum the range is a TERMINAL DEFAULT, not this coin's limit.
            //
            // Keyed on the SEEDED maximum, not the live one, because this sentence describes the
            // SLIDER — and the slider's range is whatever it was seeded with.
            .children(
                (open.lev_coin_max <= 0)
                    .then(|| limit_note(t!("toolbar.lev_max_unknown").to_string(), p, cx)),
            )
            // The value about to be applied is above the coin's stated cap. NOT an error and NOT
            // corrected here: clamping the display would misstate live position risk, and clamping
            // the value would make Apply send a leverage change nobody requested.
            //
            // Deliberately does NOT require the popup to have been SEEDED with a known cap. A popup
            // opened before the limits loaded carries a 1..125 fallback slider, and if the cap then
            // arrives at 20 a drag to 50 is exactly the case that needs saying — gating this on the
            // seeded value silenced the warning in the one situation it exists for.
            .children(
                (coin_max > 0 && current_lev(input, cx) > coin_max as f32).then(|| {
                    limit_note(
                        t!("toolbar.lev_max_exceeded", max = coin_max.to_string()).to_string(),
                        p,
                        cx,
                    )
                }),
            )
            // The coin's cap is no longer the one the slider was seeded with — the exchange revised
            // it, or it simply loaded after the popup opened. The slider keeps its seeded range,
            // because rewriting bounds under a live drag is worse than leaving them, so the popup
            // states the divergence instead of moving silently.
            //
            // A plain inequality on the two CAPS, so it also catches the transitions through the
            // unknown sentinel — 0 -> 20 when the limits load after the popup opened, and 20 -> 0
            // when the exchange withdraws a cap. Requiring both sides to be known excluded exactly
            // those, which are the common cases.
            //
            // Gated on the read having SUCCEEDED, though: `market_limits` returns `None` while the
            // provider client, its snapshot or the market lookup is momentarily unavailable — a
            // brief reconnect is enough — and `coin_max` then drops to 0 without anything having
            // been revised. Without this gate the popup accuses the exchange of changing a limit
            // every time the connection blinks.
            .children(
                (limits.is_some() && coin_max != open.lev_coin_max)
                    .then(|| limit_note(t!("toolbar.lev_range_stale").to_string(), p, cx)),
            )
    });

    // One-click presets. They choose a VALUE exactly as a slider drag does and send nothing: this
    // popup's contract is that only Apply reaches the exchange, and a one-click unconfirmed
    // leverage write would be a new and worse money surface.
    let lev_presets = is_lev.then(|| {
        let mut row = h_flex().gap(design::ui_px(cx, 4.0));
        for preset in LEV_PRESETS {
            let available = lev_preset_available(preset, coin_max);
            let tip = if available {
                t!("toolbar.lev_preset_tip", x = preset.to_string()).to_string()
            } else {
                t!(
                    "toolbar.lev_preset_blocked",
                    x = preset.to_string(),
                    max = coin_max.to_string()
                )
                .to_string()
            };
            let backend = backend.clone();
            let group = group.to_string();
            let target = target.clone();
            let slider = slider.clone();
            let input = input.clone();
            row = row.child(
                MoonButton::new(SharedString::from(format!("toolbar-lev-x{preset}")))
                    .label(format!("×{preset}"))
                    .variant(MoonButtonVariant::Neutral)
                    .size(MoonButtonSize::ToolbarCompact)
                    .disabled(!available)
                    .tooltip(tip)
                    .on_click(move |_, window, app| {
                        let b = backend.read(app);
                        if !target.is_live(TradeMetric::Lev, b, &group) {
                            return;
                        }
                        slider.update(app, |st, c| st.set_value(preset as f32, window, c));
                        input.update(app, |st, c| st.set_value(format!("{preset}"), window, c));
                    })
                    .render(),
            );
        }
        row
    });
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
        .children(lev_limits)
        .child(
            MoonSlider::new(slider)
                .id(format!("{}-slider", metric.id()))
                .height(18.0),
        )
        .children(lev_presets)
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
                        // Preserve the pct the write is about to target, not the group-local one:
                        // toggling Extended must not carry the group's percentage into the core
                        // alongside the requested mode (goal A2 FIX-2).
                        let cur = b.write_aligned_group_exit(&group).take_profit_pct;
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
        // stop-limit order. This control moved here from the core-settings popup. Read through
        // `write_aligned_group_exit` so the checkbox shows the value the toggle below is actually
        // about to overwrite, not the group-local one (goal A2 FIX-2).
        let stop_market_on = {
            let b = backend.read(cx);
            b.write_aligned_group_exit(group).use_stop_market
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
