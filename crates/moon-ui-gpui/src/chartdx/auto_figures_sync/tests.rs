use std::cell::RefCell;
use std::sync::Arc;

use moon_chart::auto_figures::tf::BarSource;
use moon_core::config::ChartTheme;
use moon_core::config::LabelColor;
use moon_core::config::auto_figures::{DetectorParams, SwingParams, TfId};
use moon_core::market::ChartCandle;

use super::job::{
    BarCache, PaneJob, RowKey, closed_bars, last_closed_open_ms, next_key_change_ms, run,
    with_chart_tail,
};
use super::{native_closed_candles, resolve_rgb};
use crate::chartdx::types::CandleGpu;

const MIN: i64 = 60_000;
/// A UTC midnight (day 20 780 since the epoch), so every grid below — up to D1 — is aligned by
/// construction.
const DAY0: i64 = 1_795_392_000_000;

fn bar(t_ms: i64, low: f32, high: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms: t_ms as f64,
        open: low,
        high,
        low,
        close: high,
        volume: 1.0,
        quote_volume: 1.0,
    }
}

/// `n` consecutive bars of `tf_ms` from `from`, each a little higher than the last.
fn series(from: i64, tf_ms: i64, n: usize) -> Vec<ChartCandle> {
    (0..n)
        .map(|i| bar(from + i as i64 * tf_ms, 100.0 + i as f32, 101.0 + i as f32))
        .collect()
}

#[test]
fn the_last_closed_bar_moves_only_after_its_close_has_settled() {
    let t = |h: i64, m: i64, s: i64| (DAY0 + h * 3_600_000 + m * MIN + s * 1_000) as f64;
    // 10:04:59 — the 10:00 bar is still forming; the newest closed one opened at 09:55.
    assert_eq!(
        last_closed_open_ms(t(10, 4, 59), TfId::M5),
        DAY0 + 9 * 3_600_000 + 55 * MIN
    );
    // One second after the close it has not settled yet...
    assert_eq!(
        last_closed_open_ms(t(10, 5, 1), TfId::M5),
        DAY0 + 9 * 3_600_000 + 55 * MIN
    );
    // ...three seconds after, it has.
    assert_eq!(
        last_closed_open_ms(t(10, 5, 3), TfId::M5),
        DAY0 + 10 * 3_600_000
    );
    // An H4 bar: 08:00–12:00 closes at noon, so at 11:59 the last closed one is 04:00.
    assert_eq!(
        last_closed_open_ms(t(11, 59, 0), TfId::H4),
        DAY0 + 4 * 3_600_000
    );
    // And the wake for it lands exactly on the settle moment after noon.
    assert_eq!(
        next_key_change_ms(t(11, 59, 0), TfId::H4),
        DAY0 + 12 * 3_600_000 + 2_000
    );
}

#[test]
fn resampling_drops_a_partial_first_bucket_and_the_forming_one() {
    // 24 five-minute bars, 10:05 through 12:00: the 10:00 hour misses its first bar.
    let rows = series(DAY0 + 10 * 3_600_000 + 5 * MIN, 5 * MIN, 24);
    // Last closed hour is 11:00, so 11:xx is kept and 10:xx is dropped as partial.
    let hours = closed_bars(&rows, 5 * MIN, 60 * MIN, DAY0 + 11 * 3_600_000);
    assert_eq!(hours.len(), 1);
    assert_eq!(hours[0].t_open_ms, (DAY0 + 11 * 3_600_000) as f64);
    // Its high and low span its twelve bars: bars 11..=22 of the series.
    assert_eq!((hours[0].low, hours[0].high), (111.0, 123.0));
    // With the last closed hour at 10:00, 11:xx is still forming and nothing is left.
    assert!(closed_bars(&rows, 5 * MIN, 60 * MIN, DAY0 + 10 * 3_600_000).is_empty());
}

#[test]
fn the_chart_fills_only_the_tail_the_cache_has_not_written() {
    // Cache up to 10:10; the chart (1-minute bars) runs 10:05..10:24.
    let cache = series(DAY0 + 10 * 3_600_000, 5 * MIN, 3);
    let chart = series(DAY0 + 10 * 3_600_000 + 5 * MIN, MIN, 20);
    let merged = with_chart_tail(&cache, &chart, MIN, DAY0 + 10 * 3_600_000 + 15 * MIN);
    let opens: Vec<i64> = merged.iter().map(|b| b.t_open_ms as i64).collect();
    // The cache's three rows untouched, then the chart's 10:15 bucket; 10:20 is not closed yet.
    assert_eq!(
        opens,
        vec![
            DAY0 + 10 * 3_600_000,
            DAY0 + 10 * 3_600_000 + 5 * MIN,
            DAY0 + 10 * 3_600_000 + 10 * MIN,
            DAY0 + 10 * 3_600_000 + 15 * MIN,
        ]
    );
    // The cache's own 10:05 row wins over the chart's.
    assert_eq!(merged[1], cache[1]);
    // A chart whose timeframe does not divide five minutes adds nothing.
    assert_eq!(with_chart_tail(&cache, &chart, 30 * MIN, i64::MAX).len(), 3);
}

/// A cache that counts its reads and answers from fixed rows.
struct FakeCache {
    m5: Option<Vec<ChartCandle>>,
    reads: RefCell<Vec<(u32, i64, i64)>>,
}

impl BarCache for FakeCache {
    fn read(&self, kind_min: u32, from_ms: i64, to_ms: i64) -> Option<Vec<ChartCandle>> {
        self.reads.borrow_mut().push((kind_min, from_ms, to_ms));
        match kind_min {
            5 => self.m5.clone(),
            _ => Some(Vec::new()),
        }
    }
}

fn key(tf: TfId, now_ms: f64, lookback: u32) -> RowKey {
    RowKey {
        detector: DetectorParams::Swing(SwingParams {
            lookback_bars: lookback,
            ..SwingParams::default()
        }),
        tf,
        last_closed_ms: last_closed_open_ms(now_ms, tf),
        chart_ready: true,
    }
}

#[test]
fn rows_take_their_bars_from_the_chart_or_the_cache_and_read_it_once() {
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    // Two days of 5-minute cache, half a day of 5-minute chart.
    let cache = FakeCache {
        m5: Some(series(DAY0 - 2 * 86_400_000, 5 * MIN, 2 * 288 + 144)),
        reads: RefCell::new(Vec::new()),
    };
    let chart: Arc<[ChartCandle]> = series(DAY0, 5 * MIN, 144).into();
    let job = PaneJob {
        rows: vec![
            // Same timeframe as the chart: the chart series as is.
            (0, key(TfId::M5, now, 100)),
            // 1 h × 200 is past the chart's twelve hours: the cache.
            (1, key(TfId::H1, now, 200)),
            // 4 h × 200: the cache again — read ONCE for both.
            (2, key(TfId::H4, now, 200)),
        ],
        chart_tf_ms: 5 * MIN,
        chart,
        cache: None,
        now_ms: now,
    };
    let out = run(&job, Some(&cache));
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].1.source, BarSource::ChartSeries);
    assert_eq!(out[0].1.bars, 100);
    assert!(!out[0].1.short_history());
    assert_eq!(out[1].1.source, BarSource::Cache5m);
    assert_eq!(out[2].1.source, BarSource::Cache5m);
    // Two and a half days of hours is sixty: short of the two hundred asked for, and it says so.
    assert_eq!(out[1].1.bars, 60);
    assert!(out[1].1.short_history());
    assert_eq!(
        cache.reads.borrow().len(),
        1,
        "one read serves every 5-minute row"
    );
    assert!(out.iter().all(|(_, o)| !o.read_failed));
}

#[test]
fn a_cache_read_that_did_not_happen_is_a_failure_not_an_empty_answer() {
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    let cache = FakeCache {
        m5: None,
        reads: RefCell::new(Vec::new()),
    };
    let job = PaneJob {
        rows: vec![(0, key(TfId::H4, now, 50))],
        chart_tf_ms: 5 * MIN,
        chart: Vec::new().into(),
        cache: None,
        now_ms: now,
    };
    let out = run(&job, Some(&cache));
    assert!(out[0].1.read_failed);
    assert!(out[0].1.found.is_empty());
    // A 1-minute row on a 5-minute chart has no source at all: nothing, and no failure to retry.
    let job = PaneJob {
        rows: vec![(0, key(TfId::M1, now, 50))],
        chart_tf_ms: 5 * MIN,
        chart: series(DAY0, 5 * MIN, 10).into(),
        cache: None,
        now_ms: now,
    };
    let out = run(&job, Some(&cache));
    assert_eq!(out[0].1.source, BarSource::Unavailable);
    assert!(!out[0].1.read_failed && out[0].1.short_history());
}

#[test]
fn gpu_rows_come_back_on_their_grid_and_only_when_closed() {
    let tf = 5 * MIN;
    // An epoch far from the rows, so the f32 relative times carry a visible rounding error.
    let epoch = (DAY0 + 14 * 86_400_000) as f64 + 123.0;
    let row = |t_ms: i64, tf_rel: f32| CandleGpu {
        t_open_rel: (t_ms as f64 - epoch) as f32,
        open: 1.0,
        high: 2.0,
        low: 0.5,
        close: 1.5,
        volume: 7.0,
        tf_rel,
    };
    let rows = [
        // A coarser prefix row: not the chart's own timeframe.
        row(DAY0 - 86_400_000, 86_400_000.0),
        row(DAY0, 0.0),
        row(DAY0 + tf, tf as f32),
        // Still forming at `now`.
        row(DAY0 + 2 * tf, 0.0),
    ];
    let now = (DAY0 + 2 * tf + 1_000) as f64;
    let out = native_closed_candles(&rows, epoch, tf, now);
    let opens: Vec<f64> = out.iter().map(|c| c.t_open_ms).collect();
    assert_eq!(opens, vec![DAY0 as f64, (DAY0 + tf) as f64]);
    assert_eq!(out[0].quote_volume, 7.0);
}

#[test]
fn line_colours_come_from_the_theme_or_the_users_pick() {
    let theme = ChartTheme::default();
    assert_eq!(
        resolve_rgb(LabelColor::Theme, true, &theme),
        theme.caption_label
    );
    // Support (below price) in the positive colour, resistance (above) in the negative one.
    assert_eq!(
        resolve_rgb(LabelColor::BySign, false, &theme),
        theme.label_positive
    );
    assert_eq!(
        resolve_rgb(LabelColor::BySign, true, &theme),
        theme.label_negative
    );
    let picked = theme.label_positive;
    let packed = (u32::from(picked[0]) << 16) | (u32::from(picked[1]) << 8) | u32::from(picked[2]);
    assert_eq!(resolve_rgb(LabelColor::Fixed(packed), true, &theme), picked);
}
