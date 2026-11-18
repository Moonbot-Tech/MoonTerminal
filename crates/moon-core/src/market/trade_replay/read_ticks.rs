//! Windowed copies and visible price fits from the replay's time-ordered ticks.

use std::ops::Range;
use std::rc::{Rc, Weak};

use crate::feed::Tick;

use super::{CandleReadParams, ChartHistoryBuffers, ChartHistoryRead, TradeReplaySeries};

/// Filter a frozen tick window once per published snapshot and selected slice.
#[derive(Default)]
pub struct ReplayTickWindow {
    source: Weak<TradeReplaySeries>,
    selection: Range<usize>,
    ticks: Vec<Tick>,
}

impl ReplayTickWindow {
    /// Read the current replay window, with trade rows available through [`Self::ticks`].
    /// The other rows go into `out` as in [`TradeReplaySeries::read_with_price_window`].
    pub fn read(
        &mut self,
        source: &Rc<TradeReplaySeries>,
        epoch_ms: f64,
        bounds: (f32, f32),
        price_window: Option<(f32, f32)>,
        candle_params: Option<&CandleReadParams>,
        out: &mut ChartHistoryBuffers,
    ) -> ChartHistoryRead {
        source.read_history(
            epoch_ms,
            bounds,
            price_window,
            candle_params,
            out,
            Some((source, self)),
        )
    }

    /// The filtered rows for the most recent read, in original time order.
    pub fn ticks(&self) -> &[Tick] {
        &self.ticks
    }

    /// Select exactly the requested ticks, reusing the rows while only the visible fit moves.
    pub(super) fn select(&mut self, source: &Rc<TradeReplaySeries>, bounds: (i64, i64)) {
        let selection = tick_indices(&source.ticks, bounds);
        if self.source.as_ptr() != Rc::as_ptr(source) || self.selection != selection {
            filter_ticks(&source.ticks[selection.clone()], &mut self.ticks);
            self.source = Rc::downgrade(source);
            self.selection = selection;
        }
    }
}

/// Copy only the requested slice and fit its visible prices.
pub(super) fn copy_ticks(
    ticks: &[Tick],
    bounds: (i64, i64),
    price_window: Option<(i64, i64)>,
    out: &mut Vec<Tick>,
) -> Option<(f32, f32)> {
    filter_ticks(&ticks[tick_indices(ticks, bounds)], out);
    price_range(out, price_window)
}

/// Find the retained upload slice using inclusive, truncated-millisecond bounds.
fn tick_indices(ticks: &[Tick], bounds: (i64, i64)) -> Range<usize> {
    // Replay builders publish ticks in time order. Integer comparisons deliberately preserve the
    // old inclusive millisecond clipping (a fractional timestamp is truncated, not rounded).
    let lo = ticks.partition_point(|tick| (tick.time_ms as i64) < bounds.0);
    let hi = ticks.partition_point(|tick| (tick.time_ms as i64) <= bounds.1);
    lo..hi
}

/// Materialize only ticks that can actually be drawn.
fn filter_ticks(selected: &[Tick], out: &mut Vec<Tick>) {
    out.clear();
    out.reserve(selected.len());
    out.extend(
        selected
            .iter()
            .copied()
            .filter(|tick| tick.time_ms.is_finite() && tick.price.is_finite() && tick.price > 0.0),
    );
}

/// Fit the visible part of the validated upload slice using exact fractional times.
pub(super) fn price_range(ticks: &[Tick], price_window: Option<(i64, i64)>) -> Option<(f32, f32)> {
    let (from, to) = price_window?;
    let lo = ticks.partition_point(|tick| tick.time_ms < from as f64);
    let hi = ticks.partition_point(|tick| tick.time_ms <= to as f64);
    let (first, rest) = ticks.get(lo..hi)?.split_first()?;
    Some(
        rest.iter()
            .fold((first.price, first.price), |(lo, hi), tick| {
                (lo.min(tick.price), hi.max(tick.price))
            }),
    )
}

#[cfg(test)]
mod tests;
