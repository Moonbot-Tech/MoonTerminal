//! Trading toolbar: the terminal's own composition on top of MoonPalette.
//!
//! The row is built from five semantic sections — size, leverage, risk, exit, session — plus the
//! window launchers at the trailing edge; `design::chrome_divider` draws the section boundaries.
//! Edits go where they belong: size/TP/SL/sell into group-local config, leverage to the exchange,
//! manual strategy to its core, and `follow` into `Backend`.
//!
//! Organized into submodules, while this module owns slider bounds and re-exports:
//! - [`coin_menu`] provides the shared coin context menu;
//! - [`coin_search`] provides coin search and the shared `COIN - Server` dropdown for chart tabs,
//!   the header ticker, and the Report coin filter;
//! - [`core_broadcast`] resolves the cross-window core filter the Profit Monitor publishes;
//! - [`core_combo`] provides exchange grouping plus the shared multi-select core picker for Orders,
//!   Report, Assets, Alerts, Analytics, and Core Status;
//! - [`core_quick`] holds that picker's pure decisions — row toggling and the per-exchange
//!   selection state;
//! - [`core_run`] is the shared run control: whether a core (or a whole group of them) is up and
//!   trading, plus the restart and start/stop buttons that change it;
//! - [`core_groups`] holds the pure decisions behind its saved core groups;
//! - [`core_group_dialogs`] owns the two modals that create and manage those groups;
//! - [`core_host`] is the adapter each of its six consumers implements, plus the extras assembly
//!   they all share;
//! - [`fmt`] formats size, sell, and field values and computes mouse-wheel steps;
//! - [`manual_strat`] provides the header's manual-strategy toggle and picker;
//! - [`metric`] provides TP/SL/leverage trigger buttons and popup content;
//! - [`strips`] provides size and sell preset strips with native MoonUI interaction;
//! - [`scale`] provides price-scale dropdowns for tabs, AddToChart stacks, and trade windows;
//! - [`date_range`] provides the shared from/to date+time bounds of Report and Analytics;
//! - [`toolbar`] composes the toolbar row.

mod coin_menu;
pub(crate) mod coin_search;
mod core_broadcast;
mod core_combo;
mod core_group_dialogs;
mod core_groups;
mod core_host;
pub(crate) mod core_run;
mod core_quick;
pub(crate) mod date_range;
mod fmt;
mod label_fields;
mod manual_strat;
mod metric;
mod scale;
mod strips;
pub(crate) mod toolbar;
mod venue_label;

pub use coin_menu::{CoinMenuCtx, CoinMenuOrigin, OrderSide, open_coin_menu};
pub(crate) use core_broadcast::{apply_core_broadcast, next_core_filter};
pub(crate) use core_combo::{
    CoreAllRowMode, core_combo, core_menu_sections, toggle_exchange_cores,
};
pub(crate) use core_groups::group_is_applied;
pub(crate) use core_host::{CoreComboHost, core_combo_extras};
pub(crate) use core_quick::toggle_core_selection;
pub use fmt::{fmt_adaptive, fmt_field2, fmt_field2_signed};
pub(crate) use label_fields::{field_picker, row_display_name, row_title};
pub use manual_strat::manual_strategy_controls;
pub(crate) use manual_strat::select_manual_strategy;
pub use metric::{MetricTarget, OpenMetricPopup, TradeMetric, metric_popup_content};

use moon_core::market::{MarketLimits, MaxOrderSource};
pub(crate) use scale::{
    scale_dropdown_for_add_stack, scale_dropdown_for_tabs, scale_dropdown_for_trade_window,
    step_scale,
};
pub use toolbar::toolbar;
pub(crate) use venue_label::{venue_id_label, venue_label, venue_section_label};

/// Unscaled width of the shared core-selector trigger.
///
/// Consumers that reserve responsive toolbar space use the same value as [`core_combo`], so the
/// layout cannot drift from the MoonUI dropdown it contains.
///
/// Narrowed by 10% from 118 px: the trigger only ever shows "All cores" or "Cores: N", and the
/// bars that carry it (Orders, Report, Assets, Core Status, Analytics, and the Screener's own
/// core source selector, which uses it as its lower bound) win that space for their controls. The
/// longest label is the Spanish "Todos los núcleos", which truncates here as it already did at 118.
pub(crate) const CORE_COMBO_TRIGGER_W: f32 = 106.0;

/// Trading-metric slider bounds `(min, max, step)` matching core semantics.
///
/// `Shell` also uses these bounds when it creates slider state.
pub const TP_NORMAL: (f32, f32, f32) = (2.0, 100.0, 1.0); // x_tmode off: 2..100% (minimum = 2)
/// Boundary at or below which the fine slider controls sub-percent TP through scalp.
///
/// A coarse TP of 2 is its minimum, so the lower 0..2 slider remains enabled. Raising coarse TP
/// above 2 disables the lower slider. This boundary also corresponds to the shared `2%`
/// label/junction.
pub const TP_FINE_MAX: f32 = 2.0;
/// Caps the fine-slider (scalp) VALUE below the main TP boundary.
///
/// Exactly 2% must go through the main TP (`x_sell`) on the upper slider; sending it through scalp
/// instead puts the bot into scalping mode and sells near its minimum (~0.4%). Therefore the scalp
/// value stops at 1.99%, while the scale and label still end at `2%`.
pub const TP_FINE_CAP: f32 = 1.99;
pub const TP_EXT: (f32, f32, f32) = (100.0, 900.0, 10.0); // x_tmode on ("s9"): 100..900%
pub const SL_BOUNDS: (f32, f32, f32) = (-20.0, 1.0, 0.01); // signed: -20..+1%
/// FALLBACK leverage slider bounds, used ONLY while the coin's real maximum is unknown.
///
/// This is not "the leverage range": 125 is the highest any supported exchange offers, not what the
/// market on screen allows, and most coins stop far below it. [`lev_bounds_for`] narrows the slider
/// to the exchange's own figure as soon as one is available, and the popup says out loud when it is
/// falling back to this range instead.
pub const LEV_BOUNDS: (f32, f32, f32) = (1.0, 125.0, 1.0);

/// One-click leverage presets offered beside the slider.
pub const LEV_PRESETS: [i32; 3] = [5, 10, 50];

/// Slider bounds `(min, max, step)` for one coin's leverage.
///
/// `coin_max <= 0` means the exchange has not stated a maximum (or the market is spot), and the
/// slider keeps [`LEV_BOUNDS`] — today's behaviour — while the popup states out loud that the range
/// is a terminal default rather than this coin's limit.
///
/// Otherwise the slider ENDS AT THE COIN'S OWN MAXIMUM, so no drag can select above it. The
/// account's current leverage is deliberately NOT folded into the upper bound, even when it is
/// already higher: stretching the range to reach it would make the control offer above-cap values
/// the exchange will reject, which is the settable-then-rejected behaviour this work exists to
/// remove.
///
/// An already-over-cap account is still displayed HONESTLY rather than clamped, and that costs
/// nothing here: `MoonSliderState` clamps only the thumb POSITION, never the stored value, so
/// seeding 50 into a 1..20 slider pins the thumb at 20 while the field — which is what Apply
/// actually sends — keeps saying 50. So the live figure is not misstated, pressing Apply does not
/// silently write a leverage nobody chose, and the popup explains the mismatch in words. Clamping
/// the value instead would turn "open the popup, press Apply" into an unrequested 50 -> 20 write.
///
/// Args:
///     coin_max: Exchange-stated maximum leverage, or a non-positive unknown/spot sentinel.
///
/// Returns:
///     The minimum, maximum, and step for the popup's leverage slider.
pub fn lev_bounds_for(coin_max: i32) -> (f32, f32, f32) {
    if coin_max <= 0 {
        return LEV_BOUNDS;
    }
    let (min, _, step) = LEV_BOUNDS;
    (min, coin_max as f32, step)
}

/// What an unknown figure renders as, on every surface that shows one.
///
/// Never a zero and never a plausible substitute: these figures size real orders, so a wrong number
/// costs more than an absent one. Shared by the toolbar readout and the leverage popup so the two
/// cannot drift into different marks for the same absence.
pub const DASH: &str = "—";

/// What a max-order readout is actually stating, once its three absent-or-unknown states are
/// separated.
///
/// The classification lives here, pure and in ONE place, because two surfaces render this figure at
/// different precisions — the toolbar compactly, the popup in full — and only the PRECISION may
/// differ between them. If each decided on its own which unknown it was looking at, the row and the
/// popup could show a dash for different reasons and explain the wrong one on hover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaxOrderReadout {
    /// The exchange stated a fixed notional cap. It does not move with the price.
    Stated(f64),
    /// No notional cap was stated, so this is the quantity cap converted at the current ask — which
    /// is why it drifts on a volatile market while a stated cap sits still.
    Derived(f64),
    /// The exchange stated no cap at all for this market.
    NoCap,
    /// Nothing has loaded yet: no provider, snapshot, or market.
    NoData,
    /// The visible scope names no single account, so no ONE exchange's cap applies to this row.
    ///
    /// Distinct from [`Self::NoData`] on purpose: there the figure is on its way, here there is no
    /// question to answer until the user picks a server. Saying "not loaded yet" in the Auto
    /// workspace Overview would explain the dash with a fact that is not true, on a readout that
    /// sizes real orders.
    OutOfScope,
}

impl MaxOrderReadout {
    /// Classify a market-limits read. `None` is NOT the same fact as a stated absence of any cap.
    ///
    /// [`MaxOrderSource::Pending`] joins `None` rather than `NoCap`: a cap that exists but cannot
    /// be converted yet is data that has not loaded, and saying "this market has no maximum" there
    /// would be the one wrong answer on a figure that sizes real orders.
    ///
    /// Args:
    ///     limits: Loaded market limits, or `None` before the provider data is available.
    ///
    /// Returns:
    ///     The readout state that preserves the difference between no cap and no data.
    pub fn of(limits: Option<MarketLimits>) -> Self {
        let Some(limits) = limits else {
            return Self::NoData;
        };
        match limits.max_order.source {
            MaxOrderSource::Stated => Self::Stated(limits.max_order.value),
            MaxOrderSource::Derived => Self::Derived(limits.max_order.value),
            MaxOrderSource::Pending => Self::NoData,
            MaxOrderSource::Absent => Self::NoCap,
        }
    }

    /// Format the figure with its quote token, or the dash when there is none to print.
    ///
    /// The unit is OMITTED when `symbol::resolve_quote` could not read one out of the market name,
    /// and substituting a plausible one was considered and REJECTED. The tempting substitute is
    /// USDC — `quote_usd_rate` already treats an empty quote that way, and the market families that
    /// obviously lack a quote token (Hyperliquid perps, HIP-3 `xyz:BIRD`, Bybit `AAVEPERP`) really
    /// are USDC-quoted. But the parser returns an empty quote for a strictly WIDER set than those:
    /// `concat_in` yields one for any suffix outside its nine-entry table, so a market quoted in an
    /// unlisted currency would be labelled USDC while trading in something else.
    ///
    /// On a figure that sizes a real order a WRONG unit is worse than a missing one — a missing one
    /// is recoverable from the hover text, which names the quote currency, while a wrong one is
    /// read and believed.
    ///
    /// Args:
    ///     value_text: Formatter for a present cap, so each surface picks its own precision.
    ///     quote: Quote token to append, or empty when the market name carried none.
    ///
    /// Returns:
    ///     The formatted cap with its unit, or the shared dash when there is no cap to show.
    pub fn format(self, value_text: impl FnOnce(f64) -> String, quote: &str) -> String {
        match self.value() {
            Some(v) if quote.is_empty() => value_text(v),
            Some(v) => format!("{} {quote}", value_text(v)),
            None => DASH.to_string(),
        }
    }

    /// Format the figure for a WIDTH-CONSTRAINED surface: `$` for a dollar quote, the token otherwise.
    ///
    /// The toolbar row competes for width with every trading control, and `496K USDT` is the longest
    /// readout on it. `$` buys that width back — but ONLY where the substitution is factually true.
    /// [`Self::format`]'s doc records why this surface must never print a plausible-but-wrong unit:
    /// a figure that sizes a real order is read and believed. So the dollar sign is gated on
    /// [`moon_core::symbol::is_usd_stable`] rather than applied blanket, and a BTC-quoted market
    /// still reads `0.8 BTC` here. That gate is deliberately the SHARED one: a quote-currency list
    /// owned by a panel is banned outright by `tests/theme_contract/naming.rs`, because two copies
    /// of the rule are how one surface starts disagreeing with another about the same order. The
    /// space is dropped with the token because `496K$` reads as one figure while `496K $` reads as
    /// two.
    ///
    /// Args:
    ///     value_text: Formatter for a present cap, so each surface picks its own precision.
    ///     quote: Quote token to render, or empty when the market name carried none.
    ///
    /// Returns:
    ///     The formatted cap with its compact unit, or the shared dash when there is no cap.
    pub fn format_compact(self, value_text: impl FnOnce(f64) -> String, quote: &str) -> String {
        match self.value() {
            Some(v) if moon_core::symbol::is_usd_stable(quote) => format!("{}$", value_text(v)),
            Some(v) if quote.is_empty() => value_text(v),
            Some(v) => format!("{} {quote}", value_text(v)),
            None => DASH.to_string(),
        }
    }

    /// The figure to print, or `None` when the surface must render a dash instead.
    ///
    /// Returns:
    ///     The stated or derived cap, if one is ready to display.
    pub fn value(self) -> Option<f64> {
        match self {
            Self::Stated(v) | Self::Derived(v) => Some(v),
            Self::NoCap | Self::NoData | Self::OutOfScope => None,
        }
    }

    /// The locale key explaining this readout on hover — including when there IS a number, because
    /// a derived figure that moves on its own needs saying so more than an absent one does.
    ///
    /// Returns:
    ///     The translation key describing this readout's source or absence.
    pub fn tooltip_key(self) -> &'static str {
        match self {
            Self::Stated(_) => "toolbar.max_order_tip",
            Self::Derived(_) => "toolbar.max_order_derived",
            Self::NoCap => "toolbar.max_order_none",
            Self::NoData => "toolbar.limits_unknown",
            Self::OutOfScope => "toolbar.max_order_no_core",
        }
    }
}

/// Whether a preset is offerable for a coin whose maximum leverage is `coin_max`.
///
/// An UNKNOWN maximum (`coin_max <= 0`) leaves every preset available on purpose: disabling on a
/// guess is its own false statement, and an impossible value is rejected by the exchange anyway.
/// A KNOWN maximum disables anything above it, so the control is unavailable rather than
/// settable-then-rejected.
///
/// Args:
///     preset: Candidate leverage preset.
///     coin_max: Exchange-stated maximum leverage, or a non-positive unknown sentinel.
///
/// Returns:
///     Whether the preset may be selected without exceeding a known cap.
pub fn lev_preset_available(preset: i32, coin_max: i32) -> bool {
    coin_max <= 0 || preset <= coin_max
}
