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
//! - [`core_combo`] provides exchange grouping plus the shared multi-select core picker for Orders,
//!   Report, Assets, Analytics, and Core Status;
//! - [`fmt`] formats size, sell, and field values and computes mouse-wheel steps;
//! - [`manual_strat`] provides the header's manual-strategy toggle and picker;
//! - [`metric`] provides TP/SL/leverage trigger buttons and popup content;
//! - [`strips`] provides size and sell preset strips with native MoonUI interaction;
//! - [`scale`] provides price-scale dropdowns for tabs and AddToChart stacks;
//! - [`date_range`] provides the shared from/to date+time bounds of Report and Analytics;
//! - [`toolbar`] composes the toolbar row.

mod coin_menu;
pub(crate) mod coin_search;
mod core_combo;
pub(crate) mod date_range;
mod exchange_label;
mod fmt;
mod manual_strat;
mod metric;
mod scale;
mod strips;
pub(crate) mod toolbar;

pub use coin_menu::{CoinMenuCtx, CoinMenuOrigin, OrderSide, open_coin_menu};
pub(crate) use core_combo::{
    core_combo, core_menu_sections, core_selection_is_all, normalized_core_filter_ids,
    toggle_all_core_selection, toggle_exchange_cores,
};
pub(crate) use exchange_label::{exchange_display_name, exchange_display_name_with_spot};
pub use fmt::{fmt_adaptive, fmt_field2, fmt_field2_signed};
pub use manual_strat::manual_strategy_controls;
pub(crate) use manual_strat::select_manual_strategy;
pub use metric::{MetricTarget, TradeMetric, metric_popup_content};
pub(crate) use scale::{scale_dropdown_for_add_stack, scale_dropdown_for_tabs, step_scale};
pub use toolbar::toolbar;

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
pub const LEV_BOUNDS: (f32, f32, f32) = (1.0, 125.0, 1.0);
