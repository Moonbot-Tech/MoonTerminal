//! Timeframe resolution for a row: `TfSource::Auto` from the visible time span, with hysteresis,
//! and where a row's bars come from once its timeframe is known. Decisions only — no I/O.

use moon_core::config::auto_figures::{TfId, TfSource};

/// `Auto`'s table: the visible span, in HOURS, at which each timeframe starts, widest first (the
/// shape of `hvol::AUTO_TF_TABLE`).
///
/// Each step keeps roughly 50–200 bars of the chosen timeframe on screen: few enough that a level
/// is a level of what the reader is looking at, many enough that the detector's default 150–200
/// bar lookback reaches past the screen's left edge. 4 h of 1 m bars is 240 bars, 16 h of 5 m is
/// 192, 2 d of 15 m is 192, 4 d of 30 m is 192, 8 d of 1 h is 192, 32 d of 4 h is 192.
const AUTO_TF_TABLE: [(f64, TfId); 7] = [
    (768.0, TfId::D1),
    (192.0, TfId::H4),
    (96.0, TfId::H1),
    (48.0, TfId::M30),
    (16.0, TfId::M15),
    (4.0, TfId::M5),
    (0.0, TfId::M1),
];

/// How far past a threshold the span must go before `Auto` switches, as a fraction of the span:
/// 25 % — a coarser timeframe takes over at 1.25× its threshold, a finer one at 1/1.25 of the
/// current one's. One line notch of the wheel zooms 2× (`chartdx/input.rs`), and that does NOT
/// always switch: a notch that lands between a threshold and 1.25× of it (2.4 h → 4.8 h stays on
/// 1 m, since 4.8 h < 5 h) waits for the next notch, which always clears the margin. What the
/// margin buys is that trackpad jitter and a span sitting right on a threshold never flip the lines
/// back and forth.
///
/// The spec also asks for a dwell ("switch once the span has STAYED past the threshold"). That needs
/// a clock, and this module has none by design; it is left to the stage-3/6 caller, which owns the
/// frame clock and can hold an [`AutoTf::update`] result for as long as it likes before using it.
pub const HYSTERESIS_MARGIN: f64 = 0.25;

/// The timeframe the table gives a visible span, without hysteresis.
///
/// Args:
///     visible_ms: Time the plot shows edge to edge, milliseconds.
///
/// Returns:
///     The table row the span reaches; the finest one when the span is not a positive number.
pub fn auto_tf(visible_ms: f64) -> TfId {
    let finest = AUTO_TF_TABLE[AUTO_TF_TABLE.len() - 1].1;
    if !visible_ms.is_finite() || visible_ms <= 0.0 {
        return finest;
    }
    let hours = visible_ms / 3_600_000.0;
    AUTO_TF_TABLE
        .iter()
        .find(|(at_least, _)| hours >= *at_least)
        .map_or(finest, |(_, tf)| *tf)
}

/// `Auto`'s memory between two zoom changes: the timeframe it last settled on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoTf {
    current: Option<TfId>,
}

impl AutoTf {
    /// No timeframe chosen yet: the first [`AutoTf::update`] takes the table's answer as is.
    pub fn new() -> Self {
        Self::default()
    }

    /// The timeframe settled on, if any.
    pub fn current(&self) -> Option<TfId> {
        self.current
    }

    /// Feed a new visible span; returns the timeframe to use.
    ///
    /// Switches coarser only when the span is past the next threshold by [`HYSTERESIS_MARGIN`]
    /// (`auto_tf(span / (1 + m))` is coarser), finer only when it is short of the current one's
    /// threshold by the margin (`auto_tf(span · (1 + m))` is finer). A zoom across several
    /// thresholds at once lands on the timeframe the margin-adjusted span reaches, not one step at a
    /// time. A span that is not a positive number changes nothing. There is no dwell here — see
    /// [`HYSTERESIS_MARGIN`] for why it belongs to the caller.
    pub fn update(&mut self, visible_ms: f64) -> TfId {
        if !visible_ms.is_finite() || visible_ms <= 0.0 {
            return self.current.unwrap_or_else(|| auto_tf(visible_ms));
        }
        let next = match self.current {
            None => auto_tf(visible_ms),
            Some(current) => {
                let coarser = auto_tf(visible_ms / (1.0 + HYSTERESIS_MARGIN));
                let finer = auto_tf(visible_ms * (1.0 + HYSTERESIS_MARGIN));
                if coarser > current {
                    coarser
                } else if finer < current {
                    finer
                } else {
                    current
                }
            }
        };
        self.current = Some(next);
        next
    }
}

/// The timeframe a row runs on: its fixed one, or `Auto`'s pick for the span. A fixed row leaves
/// `auto` untouched, so switching a row back to `Auto` starts from the last automatic choice.
pub fn resolve(source: TfSource, auto: &mut AutoTf, visible_ms: f64) -> TfId {
    match source {
        TfSource::Fixed(tf) => tf,
        TfSource::Auto => auto.update(visible_ms),
    }
}

/// Where a row's bars come from. Every source is one the terminal already has: the chart's own
/// series, or the local kline cache (5-minute rows for ALL markets, daily rows). A row never asks
/// the core for history of its own — that is what makes the core fetch klines from the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarSource {
    /// The chart's series as is: the row runs on the chart's own timeframe.
    ChartSeries,
    /// The chart's series resampled up to the row's timeframe, a whole multiple of the chart's,
    /// when the series is deep enough for the row's lookback.
    ChartResampled,
    /// The local 5-minute cache, resampled when the row's timeframe is wider than 5 minutes.
    Cache5m,
    /// The local daily cache.
    CacheDaily,
    /// Nothing the terminal holds yields these bars — a 1-minute row on a coarser chart. The row
    /// draws nothing and says it lacks history.
    Unavailable,
}

/// Five minutes, ms — the bar of the always-recorded local cache.
const CACHE_BASE_MS: i64 = 5 * 60_000;

/// Decide where a row's bars come from.
///
/// Args:
///     row: The row's resolved timeframe.
///     chart_tf_ms: The chart series' bar length, ms; `<= 0` when there is no chart series.
///     chart_history_ms: How much time the chart series covers, ms.
///     lookback_bars: Bars of `row` the detector reads.
///
/// Returns:
///     In order: the chart series on an equal timeframe; the daily cache for `D1`; the chart series
///     resampled when the row is a multiple of the chart's timeframe and the series covers
///     `lookback_bars + 1` row bars (the first, partial bucket is dropped); the 5-minute cache for any multiple of 5 minutes; otherwise
///     [`BarSource::Unavailable`].
pub fn bar_source(
    row: TfId,
    chart_tf_ms: i64,
    chart_history_ms: f64,
    lookback_bars: u32,
) -> BarSource {
    let row_ms = row.ms();
    if chart_tf_ms > 0 && row_ms == chart_tf_ms {
        return BarSource::ChartSeries;
    }
    if row == TfId::D1 {
        return BarSource::CacheDaily;
    }
    // One bucket more than the lookback: a series that does not start on a bucket boundary yields
    // a PARTIAL first bucket, which the bar assembly drops (a partial high or low would place a
    // level where the market never turned). Asking for exactly `lookback` buckets of history would
    // then deliver `lookback - 1` bars and report a short history the cache could have filled.
    let needed_ms = (f64::from(lookback_bars) + 1.0) * row_ms as f64;
    if chart_tf_ms > 0
        && row_ms > chart_tf_ms
        && row_ms % chart_tf_ms == 0
        && chart_history_ms >= needed_ms
    {
        return BarSource::ChartResampled;
    }
    if row_ms % CACHE_BASE_MS == 0 {
        return BarSource::Cache5m;
    }
    BarSource::Unavailable
}

#[cfg(test)]
mod tests;
