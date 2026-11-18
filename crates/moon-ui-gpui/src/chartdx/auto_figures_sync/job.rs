//! The background half of the auto figures: one pane's rows whose key moved, turned into detector
//! outcomes. Runs on the GPUI background executor, never on the UI thread — the cache read waits
//! on the kline-cache worker, and a resample of two weeks of 5-minute bars is real work.
//!
//! ## Where a row's bars come from
//!
//! `moon_chart::auto_figures::tf::bar_source` decides; this module only carries it out, and every
//! source is one the terminal already holds. The chart's own closed candles (snapshotted by the
//! panel), or the local kline cache — the 5-minute rows the background recorder keeps for every
//! market, and the daily rows. NOTHING here asks a core for history: a second candle kind makes the
//! core fetch klines from the exchange, and that is how a user's core got API-banned once.
//!
//! The 5-minute cache is written by a recorder on its own cycle, so its newest rows can lag the
//! chart. When the chart's timeframe divides five minutes its closed bars are folded to 5-minute
//! bars and appended AFTER the cache's last row — the cache wins wherever both have a bar, the
//! chart only fills the tail the recorder has not written yet.
//!
//! ## The recompute key
//!
//! [`RowKey`] is everything a row's answer depends on: its detector parameters, its resolved
//! timeframe, the open time of the last CLOSED bar of that timeframe, and whether the chart had
//! candles at all. Its time part changes only when a bar of the row's own timeframe closes, so an
//! H4 row runs six times a day whatever the chart does in between. Price ticks, the mouse, a pan
//! and a zoom of a `Fixed` row are deliberately not in it.

use std::sync::Arc;

use moon_chart::auto_figures::tf::{self, BarSource};
use moon_chart::auto_figures::{self, DetectCtx, Found};
use moon_core::config::auto_figures::{DetectorParams, TfId};
use moon_core::market::ChartCandle;
use moon_core::market::candles::resample;
use moon_core::market::kline_cache::KlineCache;

/// How long after a bar's close the row waits before treating it as closed, ms.
///
/// The bar's last trades are still in flight when the clock crosses the boundary — the core's
/// candle event and the recorder's write both land a moment later. Two seconds of a five-minute
/// bar is nothing to a reader, and it spares recomputing on a bar that is still being finished.
pub(crate) const SETTLE_MS: i64 = 2_000;

/// The 5-minute bar of the always-recorded local cache, ms.
const CACHE_BASE_MS: i64 = 5 * 60_000;

/// Open time of the last bar of `tf` that has closed and settled by `now_ms`.
pub(crate) fn last_closed_open_ms(now_ms: f64, tf: TfId) -> i64 {
    let tf_ms = tf.ms();
    let settled = now_ms as i64 - SETTLE_MS;
    settled.div_euclid(tf_ms) * tf_ms - tf_ms
}

/// The moment [`last_closed_open_ms`] next moves for `tf`, Unix ms: when the bar forming at
/// `now_ms` has closed and settled.
pub(crate) fn next_key_change_ms(now_ms: f64, tf: TfId) -> i64 {
    last_closed_open_ms(now_ms, tf) + 2 * tf.ms() + SETTLE_MS
}

/// Everything one row's detector answer depends on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RowKey {
    pub detector: DetectorParams,
    /// The resolved timeframe, already capped by the detector's coarsest one.
    pub tf: TfId,
    /// Open time of the last closed bar of `tf`.
    pub last_closed_ms: i64,
    /// Whether the chart held candles of its own when the key was taken. A chart opened before
    /// its history arrived recomputes once when it does, instead of keeping an answer computed on
    /// nothing until the next bar closes.
    pub chart_ready: bool,
}

/// Where the cache rows of one market are read from.
#[derive(Clone)]
pub(crate) struct CacheAddr {
    pub cache: KlineCache,
    pub exchange: String,
    pub market: String,
}

/// A source of cached bars, so the job can be tested without a database.
pub(crate) trait BarCache {
    /// Rows of `kind_min` with an open time in `[from_ms, to_ms]`; `None` when the read did not
    /// happen (worker busy or gone), as opposed to an empty answer.
    fn read(&self, kind_min: u32, from_ms: i64, to_ms: i64) -> Option<Vec<ChartCandle>>;
}

impl BarCache for CacheAddr {
    fn read(&self, kind_min: u32, from_ms: i64, to_ms: i64) -> Option<Vec<ChartCandle>> {
        self.cache
            .read_range(&self.exchange, &self.market, kind_min, from_ms, to_ms)
    }
}

/// One pane's rows to recompute, with everything the background thread needs and nothing it may
/// not touch: plain values and a cache handle, no engine, no session.
pub(crate) struct PaneJob {
    /// `(row index, key)` of every row to run.
    pub rows: Vec<(usize, RowKey)>,
    /// The chart series' bar length, ms.
    pub chart_tf_ms: i64,
    /// The chart's closed candles of its own timeframe, oldest first.
    pub chart: Arc<[ChartCandle]>,
    /// The market's cache address, when the terminal has a cache and knows the exchange.
    pub cache: Option<CacheAddr>,
    /// The moment the keys were taken, Unix ms — the detectors' reference time.
    pub now_ms: f64,
}

/// What one row's run produced. Kept whole for the later stages: the popup reads
/// [`Self::short_history`], captions and hover read the touches, the timeframe and the break.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AutoRowOutcome {
    pub key: RowKey,
    /// Where the bars came from.
    pub source: BarSource,
    /// Closed bars the detector read.
    pub bars: usize,
    /// Bars it needs for a full answer (`DetectorParams::bars_needed`).
    pub needed: u32,
    /// Close of the newest bar read — the reference price when the chart has no last price yet.
    pub last_close: Option<f64>,
    /// The cache read this row depended on did not happen; the row is retried rather than
    /// trusted.
    pub read_failed: bool,
    /// Every figure the detector found, before `take::select`.
    pub found: Vec<Found>,
}

impl AutoRowOutcome {
    /// Whether the row ran on less history than it asks for — "not enough history" in the popup.
    pub fn short_history(&self) -> bool {
        self.bars < self.needed as usize
    }
}

/// Run every row of `job`. Each cache kind is read at most once, wide enough for every row that
/// needs it.
pub(crate) fn run(job: &PaneJob, cache: Option<&dyn BarCache>) -> Vec<(usize, AutoRowOutcome)> {
    let chart_tf_ms = if job.chart.is_empty() {
        0
    } else {
        job.chart_tf_ms
    };
    let chart_history_ms = match (job.chart.first(), job.chart.last()) {
        (Some(first), Some(last)) if chart_tf_ms > 0 => {
            last.t_open_ms + chart_tf_ms as f64 - first.t_open_ms
        }
        _ => 0.0,
    };
    let plans: Vec<_> = job
        .rows
        .iter()
        .map(|&(ix, key)| {
            let needed = key.detector.bars_needed(key.tf);
            let source = tf::bar_source(key.tf, chart_tf_ms, chart_history_ms, needed);
            (ix, key, needed, source)
        })
        .collect();
    // One read per kind, spanning every row that wants it.
    let span = |want: BarSource| {
        plans
            .iter()
            .filter(|p| p.3 == want)
            .map(|&(_, key, needed, _)| read_window(key, needed))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    };
    let read = |kind_min: u32, window: Option<(i64, i64)>| -> Option<Option<Vec<ChartCandle>>> {
        let (from, to) = window?;
        Some(cache.and_then(|c| c.read(kind_min, from, to)))
    };
    let m5 = read(5, span(BarSource::Cache5m));
    let d1 = read(1440, span(BarSource::CacheDaily));

    plans
        .into_iter()
        .map(|(ix, key, needed, source)| {
            let timer = crate::diag::timer();
            let row_ms = key.tf.ms();
            let (bars, read_failed) = match source {
                BarSource::ChartSeries | BarSource::ChartResampled => (
                    closed_bars(&job.chart, chart_tf_ms, row_ms, key.last_closed_ms),
                    false,
                ),
                BarSource::Cache5m => match m5.as_ref().and_then(Option::as_ref) {
                    Some(rows) => {
                        let tail_closed = last_closed_open_ms(job.now_ms, TfId::M5);
                        let rows = with_chart_tail(rows, &job.chart, chart_tf_ms, tail_closed);
                        (
                            closed_bars(&rows, CACHE_BASE_MS, row_ms, key.last_closed_ms),
                            false,
                        )
                    }
                    None => (Vec::new(), true),
                },
                BarSource::CacheDaily => match d1.as_ref().and_then(Option::as_ref) {
                    Some(rows) => (
                        closed_bars(rows, TfId::D1.ms(), row_ms, key.last_closed_ms),
                        false,
                    ),
                    None => (Vec::new(), true),
                },
                BarSource::Unavailable => (Vec::new(), false),
            };
            let bars = tail(&bars, needed);
            let found = if bars.is_empty() {
                Vec::new()
            } else {
                auto_figures::detect(
                    &key.detector,
                    bars,
                    &DetectCtx {
                        tf_ms: row_ms,
                        now_ms: job.now_ms,
                    },
                )
            };
            crate::diag::bump(&crate::diag::AUTOFIGURES_DETECT);
            crate::diag::record_us(&crate::diag::AUTOFIGURES_DETECT_US, timer);
            (
                ix,
                AutoRowOutcome {
                    key,
                    source,
                    bars: bars.len(),
                    needed,
                    last_close: bars.last().map(|b| f64::from(b.close)),
                    read_failed,
                    found,
                },
            )
        })
        .collect()
}

/// The open-time window a row reads from the cache: its `needed` bars ending with the last closed
/// one, plus one bar of slack on the left for a first bucket the resample has to drop as partial.
fn read_window(key: RowKey, needed: u32) -> (i64, i64) {
    let row_ms = key.tf.ms();
    let to = key.last_closed_ms + row_ms - 1;
    let from = key.last_closed_ms - i64::from(needed) * row_ms;
    (from, to)
}

/// The newest `n` bars of `bars`.
fn tail(bars: &[ChartCandle], n: u32) -> &[ChartCandle] {
    let n = usize::try_from(n).unwrap_or(usize::MAX).min(bars.len());
    &bars[bars.len() - n..]
}

/// `rows` (bars of `src_ms`, oldest first) as CLOSED bars of `row_ms`, up to and including the one
/// opening at `last_closed_ms`.
///
/// Equal timeframes pass through. A wider one is resampled, and a bucket the source does not
/// cover from its start is dropped — the first one, when the source begins mid-bucket — because a
/// partial high or low would place a level where the market never turned. A bucket is closed when
/// it opens at or before `last_closed_ms`; the one still forming is dropped.
pub(crate) fn closed_bars(
    rows: &[ChartCandle],
    src_ms: i64,
    row_ms: i64,
    last_closed_ms: i64,
) -> Vec<ChartCandle> {
    if rows.is_empty() || src_ms <= 0 || row_ms <= 0 {
        return Vec::new();
    }
    let last = last_closed_ms as f64;
    if row_ms == src_ms {
        return rows
            .iter()
            .filter(|b| b.t_open_ms <= last)
            .copied()
            .collect();
    }
    let mut out = Vec::new();
    resample(rows, row_ms, &mut out);
    if let (Some(first_bucket), Some(first_row)) = (out.first(), rows.first())
        && first_row.t_open_ms > first_bucket.t_open_ms
    {
        out.remove(0);
    }
    out.retain(|b| b.t_open_ms <= last);
    out
}

/// The cache's 5-minute rows with the chart's newer closed bars appended, folded to five minutes.
///
/// Only when the chart's timeframe divides five minutes; otherwise the cache alone. A chart bar
/// never replaces a cache row: the recorder's row covers the whole bucket, while the chart's
/// series may start in the middle of one.
pub(crate) fn with_chart_tail(
    cache_rows: &[ChartCandle],
    chart: &[ChartCandle],
    chart_tf_ms: i64,
    last_closed_5m: i64,
) -> Vec<ChartCandle> {
    let mut rows = cache_rows.to_vec();
    if chart_tf_ms <= 0 || CACHE_BASE_MS % chart_tf_ms != 0 || chart.is_empty() {
        return rows;
    }
    let after = rows.last().map_or(f64::NEG_INFINITY, |r| r.t_open_ms);
    let folded = closed_bars(chart, chart_tf_ms, CACHE_BASE_MS, last_closed_5m);
    rows.extend(folded.into_iter().filter(|b| b.t_open_ms > after));
    rows
}
