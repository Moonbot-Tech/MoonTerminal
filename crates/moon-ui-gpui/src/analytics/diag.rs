//! Analytics diag extracted from the window module.

use super::*;

pub(super) const ANALYTICS_HEADER_H: f32 = 32.0;

/// Is the coin-table observation channel armed (`MOON_ANALYTICS_PROBE`, any value)?
///
/// A GUI panel has no observation channel by default, so this mirrors the convention
/// `diag.rs` already established for render counters: gated on an env var rather than
/// on `cfg(debug_assertions)` (this workspace builds dev with debug-assertions off),
/// read once, and inert in EVERY build unless it is set. Armed, it makes startup open
/// this window straight on the coin table and makes that table log what it renders —
/// which is the only way to observe the panel without clicking through the UI by hand.
pub(crate) fn probe_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MOON_ANALYTICS_PROBE").is_some())
}

/// `MOON_ANALYTICS_PROBE=select` — additionally adopt the first strategy that actually
/// carries a coin list, once the summary lands.
///
/// The interesting state of the "By coin" panels is the one WITH a strategy chosen, and
/// reaching it otherwise means clicking a row by hand — which is not an observation channel.
/// Inert unless the variable starts with `select`.
pub(crate) fn probe_selects_strategy() -> bool {
    probe_select_spec().is_some()
}

/// The selection spec, so any particular case — an empty list, one with no datable history —
/// can be put on screen without a human hunting for it in the list:
///
/// - `select` — the strategy with the biggest blacklist;
/// - `select:ID@CORE` — that exact strategy, addressed by the row key the page itself uses;
/// - `select:ID` — that strategy id on whichever core carries it first.
pub(crate) fn probe_select_spec() -> Option<&'static str> {
    use std::sync::OnceLock;
    static SPEC: OnceLock<Option<String>> = OnceLock::new();
    SPEC.get_or_init(|| {
        std::env::var("MOON_ANALYTICS_PROBE")
            .ok()
            .filter(|v| v == "select" || v.starts_with("select:"))
            .map(|v| v.strip_prefix("select:").unwrap_or("").to_string())
    })
    .as_deref()
}

/// Delay before showing the busy overlay, so quick recomputations do not flash the dimmer.
pub(super) const BUSY_OVERLAY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// How long typing in the strategy-name mask settles before the tabs are re-read.
///
/// Lives beside [`AnalyticsView::reload`] rather than beside the control that feeds it, because
/// what it protects is the READ: one reload cancels every in-flight axis and restarts a
/// full-period scan, so the cost is set here, not by the toolbar.
pub(super) const MASK_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

// Comparable profit unit for this frame's shared formatters.
//
// Set once from the active metric at the top of `AnalyticsView::render` and read by the shared
// profit formatters (`summary::fmt_signed`, the calendar and tuner cells), so a "%" suffix
// appears in percent mode without threading the metric through every signature. The window
// renders on the UI thread, so a thread-local stays consistent within a frame.
thread_local! {
    static PNL_UNIT: std::cell::Cell<Option<ProfitUnit>> = const { std::cell::Cell::new(None) };
}
/// Record the active profit unit for this frame's formatters.
///
/// Args:
///     unit: Comparable quote or Percent unit, or `None` outside scalar data.
///
/// Returns:
///     Nothing.
pub(in crate::analytics) fn set_pnl_unit(unit: Option<ProfitUnit>) {
    PNL_UNIT.with(|cell| cell.set(unit));
}
/// Is the window rendering percent profit rather than raw quote money?
///
/// Returns:
///     `true` only when the active comparable unit is Percent.
pub(in crate::analytics) fn pnl_is_pct() -> bool {
    PNL_UNIT.with(|cell| matches!(cell.get(), Some(ProfitUnit::Percent)))
}
/// Unit suffix for a profit-metric figure: `%` in percent mode and empty for quote money.
///
/// Returns:
///     Percent suffix or an empty quote-money suffix.
pub(in crate::analytics) fn pnl_suffix() -> &'static str {
    if pnl_is_pct() { "%" } else { "" }
}
/// Standalone unit token for a label or axis caption that stands BESIDE a profit figure rather
/// than riding on it: the exact quote ticker in money mode, `%` in percent mode. A number already
/// carries its own unit via `pnl_suffix`, so this is only for the surrounding label. Tickers are
/// language-neutral (see locales/README.md), so — like `pnl_suffix` — it lives in code, not the
/// dictionary, and slots into a `%{unit}` placeholder.
///
/// Returns:
///     Exact quote ticker, `%`, or an empty label outside comparable scalar data.
pub(in crate::analytics) fn pnl_unit_label() -> &'static str {
    PNL_UNIT.with(|cell| match cell.get() {
        Some(ProfitUnit::Percent) => "%",
        Some(ProfitUnit::Quote(currency)) => currency.ticker(),
        None => "",
    })
}
