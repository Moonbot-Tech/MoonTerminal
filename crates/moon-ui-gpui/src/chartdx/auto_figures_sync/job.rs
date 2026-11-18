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
//! chart only fills the tail the recorder has not written yet. Right after a bucket of the row's
//! timeframe closes, the cache may still hold only part of it; that newest bucket is then left out
//! and the outcome marked lagging, so the panel rechecks it on a bounded timer instead of keeping a
//! bar with a wrong high and low until the next one closes ([`LAG_CAP_MS`]).
//!
//! A market with no cache address at all gets a definitive empty answer for its cache rows
//! ([`without_cache`]), which is never retried; only a read that did not happen is.
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

/// How long past a bucket's close a resampled row keeps waiting for the 5-minute cache to fill
/// that bucket, ms.
///
/// The recorder writes on its own cycle, so right after an hour closes the cache can hold fewer
/// than twelve of its 5-minute rows; a bucket built from them has a wrong high and low, and as the
/// row's key does not move again until the NEXT hour, that wrong bar would stand for an hour.
/// Inside this window the incomplete newest bucket is left out and the row is marked
/// [`AutoRowOutcome::lagging`], which the panel rechecks on a bounded timer. Past it, a bucket
/// that is still short is taken as it is — an illiquid market may simply have no row for a quiet
/// five minutes, and waiting for it forever would be a loop with no exit.
pub(crate) const LAG_CAP_MS: i64 = 5 * 60_000;

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
    /// The cache read this row depended on did not happen (worker busy or gone): transient, so the
    /// row is retried rather than trusted. A market with NO cache address is not this — that is a
    /// definitive "no cache" answer ([`without_cache`]).
    pub read_failed: bool,
    /// The newest closed bucket was left out because the 5-minute cache has not filled it yet
    /// (see [`LAG_CAP_MS`]). The answer is usable but not final for its key.
    pub lagging: bool,
    /// Every figure the detector found, before `take::select`.
    pub found: Vec<Found>,
}

impl AutoRowOutcome {
    /// Whether the row ran on less history than it asks for — "not enough history" in the popup.
    pub fn short_history(&self) -> bool {
        self.bars < self.needed as usize
    }
}

/// The chart series as `bar_source` reads it: its bar length (`0` without candles) and the time it
/// covers, ms.
pub(crate) fn chart_extent(chart: &[ChartCandle], chart_tf_ms: i64) -> (i64, f64) {
    let tf = if chart.is_empty() { 0 } else { chart_tf_ms };
    let history = match (chart.first(), chart.last()) {
        (Some(first), Some(last)) if tf > 0 => last.t_open_ms + tf as f64 - first.t_open_ms,
        _ => 0.0,
    };
    (tf, history)
}

/// Where a row's bars come from, and how many it needs.
pub(crate) fn plan_row(key: &RowKey, chart_tf_ms: i64, chart_history_ms: f64) -> (BarSource, u32) {
    let needed = key.detector.bars_needed(key.tf);
    (
        tf::bar_source(key.tf, chart_tf_ms, chart_history_ms, needed),
        needed,
    )
}

/// Whether a source reads the local kline cache.
pub(crate) fn reads_cache(source: BarSource) -> bool {
    matches!(source, BarSource::Cache5m | BarSource::CacheDaily)
}

/// The definitive answer for a row whose bars would come from the cache when the market has none
/// (no cache open, or no exchange key for it): no bars, nothing found, nothing to retry. The row
/// runs again only when its key moves.
pub(crate) fn without_cache(key: RowKey, source: BarSource, needed: u32) -> AutoRowOutcome {
    AutoRowOutcome {
        key,
        source,
        bars: 0,
        needed,
        last_close: None,
        read_failed: false,
        lagging: false,
        found: Vec::new(),
    }
}

/// Run every row of `job`. Each cache kind is read at most once, wide enough for every row that
/// needs it.
///
/// Counters: `autofigures_detect` counts detector RUNS only — a row with no bars (unavailable,
/// failed read, no history) runs nothing and counts nothing; `autofigures_detect_us` is the whole
/// job's time, the shared cache reads included, recorded once per job.
pub(crate) fn run(job: &PaneJob, cache: Option<&dyn BarCache>) -> Vec<(usize, AutoRowOutcome)> {
    let timer = crate::diag::timer();
    let out = run_rows(job, cache);
    crate::diag::record_us(&crate::diag::AUTOFIGURES_DETECT_US, timer);
    out
}

/// [`run`] without the job-level stopwatch.
fn run_rows(job: &PaneJob, cache: Option<&dyn BarCache>) -> Vec<(usize, AutoRowOutcome)> {
    let (chart_tf_ms, chart_history_ms) = chart_extent(&job.chart, job.chart_tf_ms);
    let plans: Vec<_> = job
        .rows
        .iter()
        .map(|&(ix, key)| {
            let (source, needed) = plan_row(&key, chart_tf_ms, chart_history_ms);
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
            if reads_cache(source) && cache.is_none() {
                return (ix, without_cache(key, source, needed));
            }
            let row_ms = key.tf.ms();
            let (bars, read_failed, lagging) = match source {
                BarSource::ChartSeries | BarSource::ChartResampled => (
                    closed_bars(&job.chart, chart_tf_ms, row_ms, key.last_closed_ms),
                    false,
                    false,
                ),
                BarSource::Cache5m => match m5.as_ref().and_then(Option::as_ref) {
                    Some(rows) => {
                        let tail_closed = last_closed_open_ms(job.now_ms, TfId::M5);
                        let rows = with_chart_tail(rows, &job.chart, chart_tf_ms, tail_closed);
                        let (bars, lagging) =
                            cache_bars(&rows, row_ms, key.last_closed_ms, job.now_ms);
                        (bars, false, lagging)
                    }
                    None => (Vec::new(), true, false),
                },
                BarSource::CacheDaily => match d1.as_ref().and_then(Option::as_ref) {
                    Some(rows) => (
                        closed_bars(rows, TfId::D1.ms(), row_ms, key.last_closed_ms),
                        false,
                        false,
                    ),
                    None => (Vec::new(), true, false),
                },
                BarSource::Unavailable => (Vec::new(), false, false),
            };
            let bars = tail(&bars, needed);
            let found = if bars.is_empty() {
                Vec::new()
            } else {
                crate::diag::bump(&crate::diag::AUTOFIGURES_DETECT);
                auto_figures::detect(
                    &key.detector,
                    bars,
                    &DetectCtx {
                        tf_ms: row_ms,
                        now_ms: job.now_ms,
                    },
                )
            };
            (
                ix,
                AutoRowOutcome {
                    key,
                    source,
                    bars: bars.len(),
                    needed,
                    last_close: bars.last().map(|b| f64::from(b.close)),
                    read_failed,
                    lagging,
                    found,
                },
            )
        })
        .collect()
}

/// The 5-minute rows (cache plus chart tail) as closed bars of `row_ms`, and whether the newest
/// closed bucket had to be left out because the cache has not filled it yet.
///
/// The newest bucket counts its 5-minute rows against the number it holds (`row_ms / 5 min`); one
/// short, inside [`LAG_CAP_MS`] past the bucket's close, is dropped from the input and reported as
/// lagging. Past the cap it is used as it is. Only the NEWEST bucket is checked: an older gap is
/// a market that did not trade, which no wait will fill.
pub(crate) fn cache_bars(
    rows: &[ChartCandle],
    row_ms: i64,
    last_closed_ms: i64,
    now_ms: f64,
) -> (Vec<ChartCandle>, bool) {
    let expected = (row_ms / CACHE_BASE_MS).max(1) as usize;
    let (open, close) = (last_closed_ms as f64, (last_closed_ms + row_ms) as f64);
    let have = rows
        .iter()
        .filter(|r| r.t_open_ms >= open && r.t_open_ms < close)
        .count();
    let lagging = have < expected && now_ms < (last_closed_ms + row_ms + LAG_CAP_MS) as f64;
    let through = if lagging {
        last_closed_ms - row_ms
    } else {
        last_closed_ms
    };
    (closed_bars(rows, CACHE_BASE_MS, row_ms, through), lagging)
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
