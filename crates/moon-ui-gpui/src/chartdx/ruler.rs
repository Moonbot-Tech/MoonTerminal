//! The percent ruler: a measurement held while the user drags across the plot, gone on release.
//!
//! Never a figure. Nothing here reaches the figure store, `figures.json` or a core, and nothing is
//! drawn through the figure layer: a draft figure with a fill re-bakes the chart's base cache on
//! every accepted move, while the ruler's band rides the readout batch the cursor plates already
//! use and its text rides the text pass beside the cursor badge — both redrawn per present anyway.
//!
//! The text is formatted HERE, once per change of the measured span, so the per-frame passes only
//! place it.

use moon_core::market::{MarketDataSource, VolumeAt, VolumeSpan, VolumeSpanReadout};
use rust_i18n::t;

use super::ChartEngine;
use super::types::ChartViewGpu;

/// Opacity of the band's fill against the book colour it borrows: enough to read as a span, faint
/// enough that the candles under it stay legible.
pub(super) const FILL_ALPHA: f32 = 0.16;
/// Opacity of the band's one-pixel edge, which is what the eye aims the ends with.
pub(super) const BORDER_ALPHA: f32 = 0.75;

/// The two ends the user is measuring between, in the pane's data coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerSpan {
    pub pane: usize,
    /// Where the drag started, Unix milliseconds and price.
    pub t0_ms: f64,
    pub p0: f64,
    /// Where the pointer is now.
    pub t1_ms: f64,
    pub p1: f64,
}

impl RulerSpan {
    /// Whether the move from the start to the pointer is upward; a flat move counts as up.
    pub(super) fn up(&self) -> bool {
        self.p1 >= self.p0
    }

    /// The measured period, earlier end first.
    fn period_ms(&self) -> (f64, f64) {
        (self.t0_ms.min(self.t1_ms), self.t0_ms.max(self.t1_ms))
    }
}

/// What the ruler draws: the span and its two lines of text.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RulerReadout {
    pub(super) span: RulerSpan,
    /// `+1.25% · 12м 30с`.
    pub(super) move_line: String,
    /// `Bv 30.1 k$ · Sv 18.1 k$`, or the candles' total when the trade history does not reach.
    pub(super) volume_line: Option<String>,
    /// The quantized period the volume line was read over, so a pointer that stays inside the same
    /// quantum reuses it instead of asking the market source again.
    volume_period: Option<(i64, i64)>,
}

impl ChartEngine {
    /// Show, move or clear the percent ruler.
    ///
    /// Formats the readout only when the span changed, and reads the traded volume only when the
    /// quantized period did: a pointer drag delivers far more moves than the figures can change.
    ///
    /// Args:
    ///     span: The measured span, or `None` to remove the ruler.
    ///     source: Shared market source the volume is read from.
    ///
    /// Returns:
    ///     Whether anything the chart draws changed.
    pub fn set_ruler(&mut self, span: Option<RulerSpan>, source: &MarketDataSource) -> bool {
        let live = self.data.borrow().draws_live_market();
        let mut st = self.state.borrow_mut();
        let Some(span) = span else {
            let had = st.ruler.take().is_some();
            if had {
                st.needs_present = true;
            }
            return had;
        };
        if st.ruler.as_ref().is_some_and(|held| held.span == span) {
            return false;
        }
        let period = quantized_period(span.period_ms());
        let held_volume = st
            .ruler
            .as_ref()
            .filter(|held| held.span.pane == span.pane && held.volume_period == period)
            .map(|held| held.volume_line.clone());
        let volume_line = match held_volume {
            Some(line) => line,
            None => st.panes.get(span.pane).and_then(|pr| {
                let (from, to) = period?;
                // A frozen replay reads no live history: its rings hold the present, not the trade
                // on screen. The candles are the chart's own and answer for any period.
                let readout = match (live, pr.core) {
                    (true, Some(core)) => {
                        let started = std::time::Instant::now();
                        let readout = source.market_volume_span(
                            core,
                            &pr.market,
                            VolumeSpan::Millis(to - from),
                            // `Around` centres the window: half before, the rest after, so this
                            // is exactly `[from, to]`.
                            VolumeAt::Around(from + (to - from) / 2),
                        );
                        // Counted with the measuring captions' reads: both run on the pointer
                        // path, and `render_diag.log` is where "does dragging the ruler cost
                        // anything" is answered. Only a read that happened is counted.
                        crate::diag::bump(&crate::diag::CHART_VOLUME_READS);
                        crate::diag::bump_by(
                            &crate::diag::CHART_VOLUME_READ_US,
                            started.elapsed().as_micros() as u64,
                        );
                        readout
                    }
                    _ => None,
                };
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(f64::INFINITY, |d| d.as_millis() as f64);
                let candles = moon_chart::volume_bars::quote_turnover_between(
                    &pr.volume_samples,
                    pr.volume_samples_max_tf,
                    from as f64,
                    to as f64,
                    now_ms,
                );
                volume_text(readout, candles, &pr.quote)
            }),
        };
        st.ruler = Some(RulerReadout {
            span,
            move_line: move_text(span),
            volume_line,
            volume_period: period,
        });
        st.needs_present = true;
        true
    }
}

/// Snap a period's ends to the nearest step of about half a percent of its length, at least 0.1 s.
///
/// Every distinct period is a fresh read and a fresh cache entry in the market source. A drag moves
/// the far end by a few milliseconds per pixel at a close zoom and by minutes at a far one; a step
/// proportional to the period keeps the reads to a couple of hundred per full sweep at either. To
/// the NEAREST step rather than outward: an outward snap widened a two-second ruler's window by up
/// to a second at each end, and the volume printed beside its duration then described a longer
/// period. Off by at most a quarter of a percent each way for a period over twenty seconds; a
/// shorter one snaps to the 0.1 s floor, at most 50 ms each way.
fn quantized_period((from, to): (f64, f64)) -> Option<(i64, i64)> {
    if !(from.is_finite() && to.is_finite() && to > from) {
        return None;
    }
    let step = (((to - from) / 200.0 / 100.0).floor() as i64).max(1) * 100;
    let snap = |v: f64| (v / step as f64).round() as i64 * step;
    let (from, to) = (snap(from), snap(to));
    (to > from).then_some((from, to))
}

/// The ruler's first line: the move and how long it took, `+1.25% · 12м 30с`.
pub(super) fn move_text(span: RulerSpan) -> String {
    let duration = duration_text((span.t1_ms - span.t0_ms).abs());
    match pct_text(span.p0, span.p1) {
        Some(pct) => format!("{pct} · {duration}"),
        None => duration,
    }
}

/// Signed percentage from `from` to `to`, two decimals like every other readout on the chart.
fn pct_text(from: f64, to: f64) -> Option<String> {
    let pct = (to / from - 1.0) * 100.0;
    (from > 0.0 && pct.is_finite()).then(|| format!("{pct:+.2}%"))
}

/// A measured duration: `42с`, `12м 30с`, `3ч 05м`, `2д 03ч`.
///
/// Two units at most — the larger and the next — because the ruler is read at a glance and a
/// third unit adds nothing at that scale. Rounded down: a measurement states what elapsed.
pub(super) fn duration_text(ms: f64) -> String {
    let secs = if ms.is_finite() {
        (ms / 1000.0) as i64
    } else {
        0
    }
    .max(0);
    // The hour range is the countdowns' own spelling, shared; days are the ruler's alone.
    let (day, hour, min, sec) = (
        t!("chart_labels.unit_day"),
        t!("chart_labels.unit_hour"),
        t!("chart_labels.unit_minute"),
        t!("chart_labels.unit_second"),
    );
    match secs {
        s if s >= 86_400 => format!("{}{day} {:02}{hour}", s / 86_400, s % 86_400 / 3_600),
        s if s >= 3_600 => super::text::hours_and_minutes(s / 3_600, s % 3_600 / 60),
        s if s >= 60 => format!("{}{min} {:02}{sec}", s / 60, s % 60),
        s => format!("{s}{sec}"),
    }
}

/// The ruler's second line: who traded over the period.
///
/// Buying and selling apart when the trade history covers the whole period — that split is what
/// the ruler is asked for. Otherwise the candles' turnover when they reach the period's start: it
/// carries no split and is cut proportionally at the edges, so it is always marked `~`. Then a
/// partial split, marked the same way the volume captions mark one, and only then candles that
/// cover just the period's tail — the weakest answer of the four.
///
/// Args:
///     readout: The trade history's answer over the period, when there is one.
///     candles: The candles' turnover over it and whether they reach its start.
///     quote: The market's quote asset; empty means the unit is unknown and nothing is printed.
pub(super) fn volume_text(
    readout: Option<VolumeSpanReadout>,
    candles: Option<(f64, bool)>,
    quote: &str,
) -> Option<String> {
    let money = |v: f64| super::text::quote_turnover_label(v as f32, quote);
    let split = |r: VolumeSpanReadout, mark: &str| -> Option<String> {
        Some(format!(
            "{}{mark}{} · {}{mark}{}",
            t!("chart_labels.short.window_buy_volume"),
            money(r.buy_quote)?,
            t!("chart_labels.short.window_sell_volume"),
            money(r.sell_quote)?,
        ))
    };
    let total = |v: f64| -> Option<String> {
        Some(format!(
            "{}~{}",
            t!("chart_labels.short.window_volume"),
            money(v)?
        ))
    };
    match (readout, candles) {
        (Some(r), _) if r.complete => split(r, ""),
        (_, Some((v, true))) => total(v),
        (Some(r), _) if r.total_quote() > 0.0 => split(r, "~"),
        (_, Some((v, false))) => total(v),
        _ => None,
    }
}

/// The ruler's band in physical pixels, `[x, y, w, h]`, clipped to the pane's plot.
///
/// `None` when the pane has no usable mapping or the band lies wholly outside the plot.
pub(super) fn band_rect_px(
    view: &ChartViewGpu,
    epoch_ms: f64,
    span: &RulerSpan,
) -> Option<[f32; 4]> {
    let (x0, y0) = point_px(view, epoch_ms, span.t0_ms, span.p0)?;
    let (x1, y1) = point_px(view, epoch_ms, span.t1_ms, span.p1)?;
    let [bx, by, bw, bh] = view.bounds;
    let left = x0.min(x1).max(bx);
    let right = x0.max(x1).min(bx + bw);
    let top = y0.min(y1).max(by);
    let bottom = y0.max(y1).min(by + bh);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

/// One data point in physical pixels, the same mapping the order and figure layers use.
pub(super) fn point_px(
    view: &ChartViewGpu,
    epoch_ms: f64,
    t_ms: f64,
    price: f64,
) -> Option<(f32, f32)> {
    if !(view.time_to_px > 0.0 && view.price_to_px > 0.0) {
        return None;
    }
    let x = view.bounds[0] + ((t_ms - epoch_ms) as f32 - view.view_time0) * view.time_to_px;
    let y = view.bounds[1] + view.bounds[3] - (price as f32 - view.view_price0) * view.price_to_px;
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

#[cfg(test)]
mod tests;
