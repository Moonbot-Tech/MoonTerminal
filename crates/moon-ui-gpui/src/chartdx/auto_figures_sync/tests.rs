use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use moon_chart::auto_figures::tf::BarSource;
use moon_chart::auto_figures::{Found, FoundShape, Touch};
use moon_chart::figures::FigureBuffers;
use moon_core::config::ChartTheme;
use moon_core::config::LabelColor;
use moon_core::config::auto_figures::{
    AutoEvent, BrokenPolicy, CaptionPart, CaptionPlace, CaptionShow, DetectorParams, EventMarks,
    LineCaption, LineStyle, SwingParams, Take, TakeOrder, TakeSide, TfId,
};
use moon_core::market::ChartCandle;

use super::caption::price_quantum;
use super::job::{
    AutoRowOutcome, BarCache, PaneJob, RowKey, cache_bars, closed_bars, last_closed_open_ms,
    next_key_change_ms, run, with_chart_tail, without_cache,
};
use super::pane::{AutoPaneDrawn, SegmentBase, append_auto_geometry, refresh_auto_captions};
use super::select::{self, KeptLine, RowRules};
use super::{
    AutoPaneFigures, AutoRowDrawn, LineKey, PriceStep, RowTitle, native_closed_candles, price_step,
    resegment, resolve_rgb,
};
use crate::chartdx::types::CandleGpu;

const MIN: i64 = 60_000;

/// One row's rules on its own: the pane's hidden lines and the row's last twin marks, no formation
/// hidden through another row.
fn rules<'a>(row: &'a AutoRowDrawn, hidden: &'a [LineKey]) -> RowRules<'a> {
    RowRules {
        row_ix: row.row_ix,
        take: &row.take,
        marks: &row.marks,
        policy: row.policy,
        hidden,
        hidden_twins: &[],
        twins: &row.twins,
    }
}

/// One row selected on its own — no other row, so no twin — the way the pane's pass selects each
/// row (`select::pick`, then `select::kept_lines`).
fn select_kept(
    outcome: &AutoRowOutcome,
    rules: &RowRules<'_>,
    ref_price: f64,
    at_ms: f64,
    scratch: &mut Vec<moon_chart::auto_figures::events::EventSet>,
) -> Vec<KeptLine> {
    let picked = select::pick(outcome, rules, ref_price, at_ms, &[]);
    select::kept_lines(outcome, rules, &picked, ref_price, at_ms, scratch)
}
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

/// An hour the cache has written only nine of its twelve 5-minute rows for is left out — and the
/// row reported lagging — until five minutes past its close; after that it is used as it is.
///
/// Breakage: building the newest bucket from a half-written cache (a wrong high/low kept for the
/// whole next hour), or waiting for the missing rows with no cap.
#[test]
fn a_half_written_newest_bucket_waits_for_the_cache_but_not_forever() {
    let ten = DAY0 + 10 * 3_600_000;
    // 09:00 through 10:40: a whole 09:00 hour, then nine rows of the 10:00 one.
    let rows = series(DAY0 + 9 * 3_600_000, 5 * MIN, 21);
    let one_minute_after_close = (ten + 3_600_000 + MIN) as f64;
    let (bars, lagging) = cache_bars(&rows, 60 * MIN, ten, one_minute_after_close);
    assert!(lagging);
    let opens: Vec<i64> = bars.iter().map(|b| b.t_open_ms as i64).collect();
    assert_eq!(opens, vec![DAY0 + 9 * 3_600_000]);
    let past_the_cap = (ten + 3_600_000 + 6 * MIN) as f64;
    let (bars, lagging) = cache_bars(&rows, 60 * MIN, ten, past_the_cap);
    assert!(!lagging);
    assert_eq!(bars.len(), 2);
    // A complete newest hour is never lagging, however early.
    let full = series(DAY0 + 9 * 3_600_000, 5 * MIN, 24);
    assert!(!cache_bars(&full, 60 * MIN, ten, one_minute_after_close).1);
}

/// A bucket of the cache with empty 5-minute slots is still a bar: the cache holds a row only where
/// something traded, so a quiet 09:00 hour of five rows (09:00, 09:15, 09:30, 09:40, 09:55) is the
/// bar of those five trades — kept between two whole hours, its low and high the lowest and highest
/// of its rows, 98 and 105 by hand. A quiet day — a row every 50 minutes — is that day's bar as
/// well, while a day of one row at 12:00 is a hole: twelve hours of nothing on each side of it are
/// a terminal and a core that were not running, not a market that did not trade.
///
/// Breakage: a share-of-rows rule dropping the quiet buckets (a quiet coin's honest 1h and daily
/// bars thrown away, and the chart seeing other bars than the screener's scan), a bucket of a few
/// recorded minutes read as a whole day, or a bucket's range not folding all of its rows.
#[test]
fn a_bucket_with_empty_slots_is_still_a_bar() {
    let hour = |h: i64| DAY0 + h * 3_600_000;
    let mut rows = series(hour(8), 5 * MIN, 12);
    let quiet = [
        (0, 100.0, 103.0),
        (15, 98.0, 104.0),
        (30, 102.0, 105.0),
        (40, 99.0, 101.0),
        (55, 101.0, 102.0),
    ];
    rows.extend(
        quiet
            .iter()
            .map(|&(m, low, high)| bar(hour(9) + m * MIN, low, high)),
    );
    rows.extend(series(hour(10), 5 * MIN, 12));
    let past_the_cap = (hour(11) + 6 * MIN) as f64;
    let (bars, lagging) = cache_bars(&rows, 60 * MIN, hour(10), past_the_cap);
    assert!(!lagging);
    let opens: Vec<i64> = bars.iter().map(|b| b.t_open_ms as i64).collect();
    assert_eq!(opens, vec![hour(8), hour(9), hour(10)]);
    assert_eq!((bars[1].low, bars[1].high), (98.0, 105.0));

    let day = 24 * 3_600_000;
    let mut rows = series(DAY0, 5 * MIN, 288);
    rows.push(bar(DAY0 + day + 12 * 3_600_000, 90.0, 95.0));
    rows.extend(series(DAY0 + 2 * day, 50 * MIN, 29));
    let days = closed_bars(&rows, 5 * MIN, day, DAY0 + 2 * day);
    let opens: Vec<i64> = days.iter().map(|b| b.t_open_ms as i64).collect();
    assert_eq!(opens, vec![DAY0, DAY0 + 2 * day]);
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
        price_tick: None,
        diag: None,
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
    // One read per kind — the real candles and the core's ring beside them — serves every
    // 5-minute row.
    assert_eq!(
        cache.reads.borrow().iter().map(|r| r.0).collect::<Vec<_>>(),
        [5, moon_core::market::kline_cache::RING_5M_KIND],
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
        price_tick: None,
        diag: None,
    };
    let out = run(&job, Some(&cache));
    assert!(out[0].1.read_failed);
    assert!(out[0].1.found.is_empty());
    // A 1-minute row on a 5-minute chart reads the cache's 1-minute rows: this market has none,
    // which is an empty answer — no bars, and no failure to retry.
    let job = PaneJob {
        rows: vec![(0, key(TfId::M1, now, 50))],
        chart_tf_ms: 5 * MIN,
        chart: series(DAY0, 5 * MIN, 10).into(),
        cache: None,
        now_ms: now,
        price_tick: None,
        diag: None,
    };
    let out = run(&job, Some(&cache));
    assert_eq!(out[0].1.source, BarSource::Cache1m);
    assert!(!out[0].1.read_failed && out[0].1.bars == 0 && out[0].1.short_history());
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

/// A published row of horizontal levels at `levels`, keeping the nearest one per side.
fn level_row(levels: &[f64]) -> AutoRowDrawn {
    let key = key(TfId::H1, (DAY0 + 12 * 3_600_000) as f64, 200);
    let outcome = AutoRowOutcome {
        key,
        source: BarSource::ChartSeries,
        bars: 200,
        needed: 200,
        last_close: Some(100.0),
        read_failed: false,
        lagging: false,
        found: levels
            .iter()
            .enumerate()
            .map(|(i, &price)| Found {
                origin: None,
                shape: FoundShape::Horizontal { price },
                touches: vec![Touch { t_ms: 1.0, price }; 3],
                strength: 1.0,
                // Distinct first anchors: what tells two levels of one row apart across runs.
                first_ms: (DAY0 + i as i64 * 3_600_000) as f64,
                last_ms: (DAY0 + 10 * 3_600_000) as f64,
                broken: None,
                touch_volume: 0.0,
                tolerance_frac: 0.0,
                tag: None,
            })
            .collect(),
        recent: Vec::new(),
    };
    AutoRowDrawn {
        row_ix: 0,
        title: RowTitle::Named("Levels".into()),
        style: LineStyle::default(),
        caption: LineCaption::default(),
        take: Take {
            count: 1,
            side: TakeSide::Both,
            order: TakeOrder::Nearest,
            ..Take::default()
        },
        marks: EventMarks::default(),
        policy: BrokenPolicy::default(),
        outcome: Arc::new(outcome),
        kept: Vec::new(),
        twins: Default::default(),
        ref_ms: 0.0,
    }
}

/// A set of one level row selected against `price`, the way the panel publishes it.
fn published(levels: &[f64], hidden: Vec<LineKey>, price: f64) -> AutoPaneFigures {
    let mut row = level_row(levels);
    row.kept = select_kept(
        &row.outcome,
        &rules(&row, &hidden),
        price,
        0.0,
        &mut Vec::new(),
    );
    AutoPaneFigures::new(vec![row], hidden)
}

fn sides(set: &AutoPaneFigures) -> Vec<(usize, bool)> {
    set.rows[0].kept.iter().map(|k| (k.ix, k.above)).collect()
}

/// Breakage: selecting only at publication (review finding C) — after price crosses a level its
/// side (and `BySign` colour) and the kept set would stay stale until the next bar close.
#[test]
fn a_price_crossing_a_level_reselects_the_set_and_flips_its_side() {
    let set = published(&[90.0, 95.0, 105.0, 112.0], Vec::new(), 100.0);
    // The nearest above (105, index 2) and the nearest below (95, index 1).
    assert_eq!(sides(&set), vec![(2, true), (1, false)]);
    // A tick that moves nothing drawn replaces nothing.
    assert!(set.reselect(100.5, 0.0).is_none());
    // Price crosses 105: it becomes the support, 112 the resistance.
    let next = set
        .reselect(106.0, 0.0)
        .expect("the drawn lines changed sides");
    assert_eq!(sides(&next), vec![(3, true), (2, false)]);
}

/// Breakage: a hidden line pulled back in by the next selection, or replaced by a line the user
/// never saw.
#[test]
fn a_hidden_line_stays_hidden_and_leaves_its_slot_empty() {
    let row = level_row(&[95.0, 105.0]);
    let hidden = vec![LineKey::of(0, &row.outcome.found[1])];
    let set = published(&[95.0, 105.0], hidden, 100.0);
    assert_eq!(sides(&set), vec![(0, false)]);
    // Still hidden after a price change re-selects.
    let next = set.reselect(96.0, 0.0);
    assert!(next.is_none_or(|n| sides(&n) == vec![(0, false)]));
}

/// Breakage: events computed at publication only, so an approach never lights up as price moves.
#[test]
fn a_kept_line_shows_an_approach_once_price_comes_near() {
    let set = published(&[95.0, 105.0], Vec::new(), 100.0);
    assert!(set.rows[0].kept.iter().all(|k| k.events.is_empty()));
    // 0.1 % below the 105 level: inside the default 0.3 % approach distance.
    let next = set.reselect(104.9, 0.0).expect("an event appeared");
    let at_105 = next.rows[0].kept.iter().find(|k| k.ix == 1).unwrap();
    assert!(at_105.events.contains(AutoEvent::Approach));
}

/// Breakage: a caption rebuilt on every price tick, or never rebuilt when its printed distance
/// moves.
#[test]
fn captions_are_rebuilt_only_when_a_printed_value_moves() {
    let mut row = level_row(&[105.0]);
    row.caption = LineCaption {
        parts: vec![CaptionPart::RowName, CaptionPart::DistancePct],
        place: CaptionPlace::RightEnd,
        show: CaptionShow::Always,
    };
    row.kept = select_kept(&row.outcome, &rules(&row, &[]), 100.0, 0.0, &mut Vec::new());
    let set = Rc::new(AutoPaneFigures::new(vec![row], Vec::new()));
    let (mut zones, mut hlines, mut segs, mut markers, mut labels) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut drawn = AutoPaneDrawn::default();
    append_auto_geometry(
        Some(&set),
        &ChartTheme::default(),
        1.0,
        0.0,
        &mut FigureBuffers {
            zones: &mut zones,
            hlines: &mut hlines,
            segs: &mut segs,
            markers: &mut markers,
            labels: &mut labels,
        },
        &mut drawn,
    );
    assert!(refresh_auto_captions(&mut drawn, Some(100.0), 0.0, None));
    assert_eq!(drawn.lines[0].text.as_deref(), Some("Levels · +5.00%"));
    // 100.0 → 100.000_1: the distance still prints +5.00 %, nothing is rebuilt.
    assert!(!refresh_auto_captions(
        &mut drawn,
        Some(100.000_1),
        0.0,
        None
    ));
    // 100 → 101: +3.96 %.
    assert!(refresh_auto_captions(&mut drawn, Some(101.0), 0.0, None));
    assert_eq!(drawn.lines[0].text.as_deref(), Some("Levels · +3.96%"));
}

/// Breakage: keying a line by its first anchor alone — a period's high and low share it, so hiding,
/// hovering or pinning the low acted on the high (seen live: "pin" copied the day's high).
#[test]
fn a_periods_high_and_low_are_different_lines() {
    let bar = |t: i64, price: f64| Found {
        origin: None,
        shape: FoundShape::Horizontal { price },
        touches: vec![Touch {
            t_ms: t as f64,
            price,
        }],
        strength: 1.0,
        first_ms: DAY0 as f64,
        last_ms: (DAY0 + 3_600_000) as f64,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
    };
    let high = bar(DAY0 + 600_000, 2.0);
    let low = bar(DAY0 + 1_200_000, 1.0);
    assert_ne!(LineKey::of(0, &high), LineKey::of(0, &low));
    // The next run nudges a level's price (one more touch moves its mean): still the same line.
    let mut rerun = low.clone();
    rerun.shape = FoundShape::Horizontal { price: 1.01 };
    assert_eq!(LineKey::of(0, &low), LineKey::of(0, &rerun));
}

/// Breakage (#12a/b): the price gate removed (every market sync re-selects, a pan included), or a
/// crossing that leaves the published set in place — the side and its `BySign` colour stay stale.
#[test]
fn the_price_pass_skips_an_unchanged_price_and_replaces_on_a_crossing() {
    let mut set = Rc::new(published(&[95.0, 105.0, 112.0], Vec::new(), 100.0));
    let first = set.clone();
    // First sight of a price: evaluated; nothing drawn moved, so the set is kept.
    assert_eq!(price_step(&mut set, 100.0, 0.0), PriceStep::Same);
    assert!(Rc::ptr_eq(&set, &first));
    // The same price again: not even evaluated.
    assert_eq!(price_step(&mut set, 100.0, 0.0), PriceStep::Skipped);
    // Across 105: replaced, and 105 is now below the price.
    assert_eq!(price_step(&mut set, 106.0, 0.0), PriceStep::Replaced);
    assert!(!Rc::ptr_eq(&set, &first));
    let at_105 = set.rows[0].kept.iter().find(|k| k.ix == 1).unwrap();
    assert!(!at_105.above);
    // And the replacement remembers its price: the next sync at 106 does nothing.
    assert_eq!(price_step(&mut set, 106.0, 0.0), PriceStep::Skipped);
}

/// The raw bytes of a buffer of GPU instances, for comparing two of them exactly.
fn bytes<T: bytemuck::Pod>(v: &[T]) -> Vec<u8> {
    bytemuck::cast_slice(v).to_vec()
}

/// Breakage (#12c): a partial redraw that does not cut back to the recorded base (the old lines
/// stay under the new ones) or that appends something a full rebuild would not.
#[test]
fn redrawing_the_segment_equals_a_full_rebuild() {
    use moon_chart::layers::{MARKER_SHAPE_CROSS, MarkerInstance, ZoneInstance};
    let theme = ChartTheme::default();
    // What the layers in front of the auto segment put there: one order zone, one marker.
    let prefix = |scratch: &mut crate::chartdx::UdScratch| {
        scratch.zones.push(ZoneInstance {
            price0: 1.0,
            price1: 2.0,
            t0_rel: 0.0,
            t1_rel: 1.0,
            color: [1.0; 4],
        });
        scratch.markers.push(MarkerInstance::at_price(
            0.0,
            1.0,
            3.0,
            1.0,
            MARKER_SHAPE_CROSS,
            [1.0; 4],
        ));
    };
    let full = |set: &Rc<AutoPaneFigures>| {
        let mut scratch = crate::chartdx::UdScratch::default();
        prefix(&mut scratch);
        let base = SegmentBase {
            zones: scratch.zones.len(),
            hlines: scratch.hlines.len(),
            segs: scratch.segs.len(),
            markers: scratch.markers.len(),
        };
        let mut drawn = AutoPaneDrawn::default();
        resegment(&mut scratch, base, Some(set), &theme, 1.0, 0.0, &mut drawn);
        (scratch, base)
    };
    let mut band = level_row(&[95.0, 105.0]);
    band.style.zone = Some(0.2);
    // A 0.5 % band, as the detector would report it with each level.
    for f in &mut Arc::make_mut(&mut band.outcome).found {
        f.tolerance_frac = 0.005;
    }
    band.kept = select_kept(
        &band.outcome,
        &rules(&band, &[]),
        100.0,
        0.0,
        &mut Vec::new(),
    );
    let before = Rc::new(AutoPaneFigures::new(vec![band.clone()], Vec::new()));
    let after = Rc::new(
        before
            .reselect(106.0, 0.0)
            .expect("the crossing moved a line"),
    );

    let (mut partial, base) = full(&before);
    let mut drawn = AutoPaneDrawn::default();
    resegment(
        &mut partial,
        base,
        Some(&after),
        &theme,
        1.0,
        0.0,
        &mut drawn,
    );
    let (fresh, _) = full(&after);
    assert_eq!(bytes(&partial.zones), bytes(&fresh.zones));
    assert_eq!(bytes(&partial.hlines), bytes(&fresh.hlines));
    assert_eq!(bytes(&partial.segs), bytes(&fresh.segs));
    assert_eq!(bytes(&partial.markers), bytes(&fresh.markers));
    // And the front layers are untouched.
    assert_eq!(partial.zones[0].price1, 2.0);
    assert!(partial.zones.len() > base.zones);
}

/// Breakage (#2): the caption price cached by tick multiples. With a 0.25 tick the price prints
/// two decimals, so 100.0 and 100.1 — one tick multiple — print differently and the caption went
/// stale; 100.001 and 100.004 print the same and must not rebuild.
#[test]
fn a_caption_price_is_quantized_at_its_printed_precision() {
    let tick = Some(0.25);
    assert_ne!(
        price_quantum(Some(100.0), tick),
        price_quantum(Some(100.1), tick)
    );
    assert_eq!(
        price_quantum(Some(100.001), tick),
        price_quantum(Some(100.004), tick)
    );
    assert_eq!(
        moon_chart::axes::format_price(100.001, tick),
        moon_chart::axes::format_price(100.004, tick)
    );
}

/// Breakage (#10): two trend lines from one shared pivot keyed alike — hiding, pinning or hovering
/// one acted on the other.
#[test]
fn two_trend_lines_from_one_pivot_are_different_lines() {
    let line = |t1: f64, p1: f64| Found {
        origin: None,
        shape: FoundShape::Sloped {
            t0_ms: 0.0,
            p0: 100.0,
            t1_ms: t1,
            p1,
        },
        touches: vec![
            Touch {
                t_ms: 0.0,
                price: 100.0,
            },
            Touch {
                t_ms: t1,
                price: p1,
            },
        ],
        strength: 1.0,
        first_ms: 0.0,
        last_ms: t1,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: None,
    };
    assert_ne!(
        LineKey::of(0, &line(3_600_000.0, 101.0)),
        LineKey::of(0, &line(7_200_000.0, 103.0))
    );
}

/// A row of `prices` on `tf`, numbered `row_ix`, keeping up to five lines per side nearest first —
/// for the cross-row passes, where several rows have to meet.
fn tf_row(row_ix: usize, tf: TfId, prices: &[f64]) -> AutoRowDrawn {
    let mut row = level_row(prices);
    row.row_ix = row_ix;
    row.take.count = 5;
    let mut outcome = (*row.outcome).clone();
    outcome.key.tf = tf;
    row.outcome = Arc::new(outcome);
    row
}

/// The confluence marks of every kept line of `set`, row by row, ascending by line price (the
/// kept order is the take's ranking, which these tests do not judge).
fn confluences(set: &AutoPaneFigures) -> Vec<Vec<(f64, bool)>> {
    set.rows
        .iter()
        .map(|r| {
            let mut lines: Vec<(f64, bool)> = r
                .kept
                .iter()
                .map(|k| {
                    let price = r.outcome.found[k.ix].shape.price_at(0.0).unwrap();
                    (price, k.events.contains(AutoEvent::Confluence))
                })
                .collect();
            lines.sort_by(|a, b| a.0.total_cmp(&b.0));
            lines
        })
        .collect()
}

/// Confluence is judged across the pane's rows on every price pass: a 1h line at 100 and a 4h line
/// at 100.2 (0.2 % apart, inside the default 0.3 %) both get the mark; a second 1h row's line at
/// 100.1 is not a confluence with the first 1h row (same timeframe) but is with the 4h one; a 4h
/// line at 110 has nothing near. A row that does not mark confluence shows none and still gives
/// one to the others. The publication's own pass (`select_all`) agrees with the price pass.
///
/// Breakage: confluence never inserted (the per-line events pass never sets it), judged within one
/// row, the mark filter ignored, an unmarking row dropped as a partner, or the price pass comparing
/// before the cross-row marks are in.
#[test]
fn lines_of_two_timeframes_close_together_are_marked_as_confluence() {
    let rows = vec![
        tf_row(0, TfId::H1, &[100.0]),
        tf_row(1, TfId::H4, &[100.2, 110.0]),
        tf_row(2, TfId::H1, &[100.1]),
    ];
    let set = AutoPaneFigures::new(rows, Vec::new());
    let next = set
        .reselect(105.0, 0.0)
        .expect("the first selection draws lines");
    assert_eq!(
        confluences(&next),
        vec![
            vec![(100.0, true)],
            vec![(100.2, true), (110.0, false)],
            vec![(100.1, true)],
        ]
    );
    // The same answer again replaces nothing: the marks are part of what is compared.
    assert!(next.reselect(105.0, 0.0).is_none());

    // The 4h row turns the mark off: it shows none, the 1h row still sees it as a partner.
    let mut rows = vec![
        tf_row(0, TfId::H1, &[100.0]),
        tf_row(1, TfId::H4, &[100.2, 110.0]),
    ];
    rows[1].marks.confluence = false;
    let mut published = AutoPaneFigures::new(rows, Vec::new());
    published.select_all(&[105.0; 2], 0.0);
    assert_eq!(
        confluences(&published),
        vec![vec![(100.0, true)], vec![(100.2, false), (110.0, false)]]
    );
}

/// Breakage (B3): a line keyed by its anchors alone — every round level of a row (no touches, no
/// anchor), the POC, VAH and VAL of one day (one span, no touches), and a Monday's day open and
/// week open (one bar) each shared a key, so "Hide this line" hid all of them. The key must still
/// follow a line across runs: the same round price recomputed, a day's POC that moved as the day
/// grew.
#[test]
fn lines_sharing_their_anchors_are_told_apart_by_tag_and_round_price() {
    use moon_chart::auto_figures::LineTag;
    let line = |price: f64, first_ms: f64, touch: Option<f64>, tag: Option<LineTag>| Found {
        origin: None,
        shape: FoundShape::Horizontal { price },
        touches: touch
            .map(|t| vec![Touch { t_ms: t, price }])
            .unwrap_or_default(),
        strength: 1.0,
        first_ms,
        last_ms: first_ms,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag,
    };
    // Round prices: no touches, no anchor, no tag.
    let a = line(1.0, 0.0, None, None);
    let b = line(1.5, 0.0, None, None);
    assert_ne!(LineKey::of(0, &a), LineKey::of(0, &b));
    // The same level computed from another order of magnitude, a last digit off: the same line.
    let a_again = line(1.0 + f64::EPSILON, 0.0, None, None);
    assert_eq!(LineKey::of(0, &a), LineKey::of(0, &a_again));

    // One day's profile: one span, no touches, three tags.
    let day = DAY0 as f64;
    let poc = line(100.0, day, None, Some(LineTag::Poc));
    let vah = line(101.0, day, None, Some(LineTag::Vah));
    let val = line(99.0, day, None, Some(LineTag::Val));
    assert_ne!(LineKey::of(0, &poc), LineKey::of(0, &vah));
    assert_ne!(LineKey::of(0, &poc), LineKey::of(0, &val));
    assert_ne!(LineKey::of(0, &vah), LineKey::of(0, &val));
    // The day grew and its POC moved: still the line the reader hid.
    let poc_later = line(100.4, day, None, Some(LineTag::Poc));
    assert_eq!(LineKey::of(0, &poc), LineKey::of(0, &poc_later));

    // A Monday: the day's open and the week's open are the same bar.
    let day_open = line(100.0, day, Some(day), Some(LineTag::DayOpen));
    let week_open = line(100.0, day, Some(day), Some(LineTag::WeekOpen));
    assert_ne!(LineKey::of(0, &day_open), LineKey::of(0, &week_open));
}

/// A previous day's POC is retagged naked when price leaves it and plain when price returns; the
/// line the reader hid is the same line either way, and stays hidden. Another day's POC is not.
///
/// Breakage: the raw tag in the key (F4) — the hidden POC reappeared on the run after price moved
/// off it.
#[test]
fn a_hidden_poc_stays_hidden_when_it_turns_naked() {
    use moon_chart::auto_figures::LineTag;
    let poc = |first_ms: f64, tag| Found {
        origin: None,
        shape: FoundShape::Horizontal { price: 100.0 },
        touches: Vec::new(),
        strength: 1.0,
        first_ms,
        last_ms: first_ms + 86_000_000.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.0,
        tag: Some(tag),
    };
    let yesterday = (DAY0 - 86_400_000) as f64;
    let plain = poc(yesterday, LineTag::Poc);
    let naked = poc(yesterday, LineTag::NakedPoc);
    assert_eq!(LineKey::of(0, &plain), LineKey::of(0, &naked));
    assert_ne!(
        LineKey::of(0, &naked),
        LineKey::of(0, &poc(yesterday - 86_400_000.0, LineTag::NakedPoc))
    );
    // Through the selection: hidden while plain, still hidden once naked.
    let mut row = level_row(&[100.0]);
    Arc::make_mut(&mut row.outcome).found = vec![naked];
    let hidden = [LineKey::of(0, &plain)];
    let kept = select_kept(
        &row.outcome,
        &rules(&row, &hidden),
        99.0,
        0.0,
        &mut Vec::new(),
    );
    assert!(kept.is_empty(), "the hidden POC came back as a naked one");
}

/// Breakage (B1, B2): the events read `events::bars_read` bars — the role-flip lookback, the
/// false-break window and two more — but a row kept only `window + 6`, so a role flip was almost
/// always reported as a bounce; and the tail was cut from the DETECTOR's bars, so a row of round
/// prices (one bar needed) had one bar of history and no bar events at all. The tail comes from
/// everything fetched, the cache read reaches back for it, and "not enough history" still measures
/// the detector's own need.
#[test]
fn the_events_tail_is_independent_of_what_the_detector_needs() {
    use moon_chart::auto_figures::events;
    use moon_core::config::auto_figures::RoundLevelsParams;
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    let wanted = events::bars_read(BrokenPolicy::default());
    let round = |tf: TfId| RowKey {
        detector: DetectorParams::RoundLevels(RoundLevelsParams::default()),
        tf,
        last_closed_ms: last_closed_open_ms(now, tf),
        chart_ready: true,
    };
    // Twelve hours of 5-minute chart: the chart series for the 5-minute rows.
    let chart: Arc<[ChartCandle]> = series(DAY0, 5 * MIN, 144).into();
    let job = PaneJob {
        rows: vec![(0, round(TfId::M5)), (1, key(TfId::M5, now, 10))],
        chart_tf_ms: 5 * MIN,
        chart: chart.clone(),
        cache: None,
        now_ms: now,
        price_tick: None,
        diag: None,
    };
    let out = run(&job, None);
    assert_eq!(out[0].1.bars, 1, "round prices read one bar");
    assert_eq!(out[1].1.bars, 10);
    for (_, o) in &out {
        assert!(
            o.recent.len() >= wanted,
            "{} recent bars, the events read {wanted}",
            o.recent.len()
        );
        assert_eq!(
            o.recent.last(),
            chart.last(),
            "the tail ends on the newest bar"
        );
        assert!(!o.short_history(), "the events tail is not missing history");
    }

    // An hour row of round prices on that chart: its detector needs one hour bar, and the twelve
    // hours of the chart hold it — so the chart, resampled, with or without a cache. The events get
    // the twelve hours the chart has: best-effort, fewer than they could read, and no reason to
    // leave the chart for the cache.
    let m5_cache = || FakeCache {
        m5: Some(series(DAY0 - 2 * 86_400_000, 5 * MIN, 2 * 288 + 144)),
        reads: RefCell::new(Vec::new()),
    };
    let job = PaneJob {
        rows: vec![(0, round(TfId::H1))],
        chart_tf_ms: 5 * MIN,
        chart,
        cache: None,
        now_ms: now,
        price_tick: None,
        diag: None,
    };
    let cache = m5_cache();
    for with_cache in [true, false] {
        let out = run(&job, with_cache.then_some(&cache as &dyn BarCache));
        let o = &out[0].1;
        assert_eq!(o.source, BarSource::ChartResampled, "cache: {with_cache}");
        assert_eq!(o.bars, 1, "cache: {with_cache}");
        assert_eq!(o.recent.len(), 12, "every closed hour of the chart");
        assert!(!o.short_history());
    }
    assert!(cache.reads.borrow().is_empty(), "the cache is never read");

    // A chart only an hour deep cannot hold even that one closed hour: the cache, and its read
    // reaches back far enough for the events.
    let job = PaneJob {
        chart: series(DAY0 + 11 * 3_600_000, 5 * MIN, 12).into(),
        ..job
    };
    let cache = m5_cache();
    let out = run(&job, Some(&cache));
    assert_eq!(out[0].1.source, BarSource::Cache5m);
    assert!(out[0].1.recent.len() >= wanted);
    let last_closed = last_closed_open_ms(now, TfId::H1);
    let reads = cache.reads.borrow();
    assert!(reads[0].1 <= last_closed - (wanted as i64 - 1) * 3_600_000);
}

/// A key of `detector` on `tf` at `now`.
fn key_of(detector: DetectorParams, tf: TfId, now_ms: f64) -> RowKey {
    RowKey {
        detector,
        tf,
        last_closed_ms: last_closed_open_ms(now_ms, tf),
        chart_ready: true,
    }
}

/// The stand's "Period levels" row on M5 with the week open: 2017 bars needed, a 647-bar chart of
/// the same timeframe, and a cache that is a week deep but an hour behind. The row reads the cache
/// and the chart's tail after it — the whole week, ending on the chart's newest bar.
///
/// Breakage: the equal-timeframe short-cut to the chart series (647 bars: the week open is never
/// found while the chart is open), or a cache path that drops the chart's fresher tail.
#[test]
fn a_row_on_the_charts_timeframe_reads_the_cache_when_the_series_is_too_short() {
    use moon_core::config::auto_figures::PeriodLevelParams;
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    let row = key_of(
        DetectorParams::PeriodLevel(PeriodLevelParams::default()),
        TfId::M5,
        now,
    );
    assert_eq!(row.detector.bars_needed(TfId::M5), 2017);
    let chart: Arc<[ChartCandle]> =
        series(DAY0 + 12 * 3_600_000 - 647 * 5 * MIN, 5 * MIN, 647).into();
    // Eight days of rows up to 10:55 — the recorder has not written the last hour yet.
    let cache = FakeCache {
        m5: Some(series(DAY0 - 8 * 86_400_000, 5 * MIN, 8 * 288 + 132)),
        reads: RefCell::new(Vec::new()),
    };
    let job = PaneJob {
        rows: vec![(0, row)],
        chart_tf_ms: 5 * MIN,
        chart: chart.clone(),
        cache: None,
        now_ms: now,
        price_tick: None,
        diag: None,
    };
    let out = run(&job, Some(&cache));
    let o = &out[0].1;
    assert_eq!(o.source, BarSource::Cache5m);
    assert_eq!(o.bars, 2017);
    assert!(!o.short_history());
    assert_eq!(
        o.recent.last(),
        chart.last(),
        "the chart's tail is appended"
    );
    // On a market with no cache the chart is all there is: it is used, and the row says it is short.
    let out = run(&job, None);
    let o = &out[0].1;
    assert_eq!(o.source, BarSource::ChartSeries);
    assert_eq!(o.bars, 647);
    assert!(o.short_history());
}

/// "Not enough history" from 90 % of the detector's need for a row that samples its bars (§13.3):
/// the 4h levels row of the scenes' set reads 68 of its 72 bars and has its answer; 64 of 72
/// (88.9 %) is short, 65 (90.3 %) is not. A calendar row needs every bar of its period: one bar
/// short of a week's 2017 is short, at 95 % as well.
///
/// Breakage: the old `bars < needed` for every row (every hole in the cache read as missing
/// history), the threshold measured against the events tail, or the 90 % applied to calendar rows
/// too (a week open lost to a hole drawn as nothing, with no note).
#[test]
fn sampled_rows_have_their_history_at_ninety_percent_calendar_rows_at_all_of_it() {
    use moon_core::config::auto_figures::{
        PeriodExtremeParams, PeriodLevelParams, VolumeProfileParams,
    };
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    let mut o = without_cache(key(TfId::H4, now, 72), BarSource::Cache5m, 72);
    for (bars, short) in [(72, false), (68, false), (65, false), (64, true), (0, true)] {
        o.bars = bars;
        assert_eq!(o.short_history(), short, "{bars} of 72");
    }
    let mut one = without_cache(key(TfId::H4, now, 72), BarSource::Cache5m, 1);
    one.bars = 1;
    assert!(!one.short_history());
    one.bars = 0;
    assert!(one.short_history());

    for detector in [
        DetectorParams::PeriodLevel(PeriodLevelParams::default()),
        DetectorParams::PeriodExtreme(PeriodExtremeParams::default()),
        DetectorParams::VolumeProfile(VolumeProfileParams::default()),
    ] {
        let mut o = without_cache(key_of(detector, TfId::M5, now), BarSource::Cache5m, 2017);
        for (bars, short) in [(2017, false), (2016, true), (1917, true)] {
            o.bars = bars;
            assert_eq!(o.short_history(), short, "{detector:?}: {bars} of 2017");
        }
    }
}

/// The caption names what a tagged line is beside its row ("POC", "Week open"), and leaves out
/// what a line does not have: no touch count for a line nothing touched, no age for a line with no
/// moment of birth.
///
/// Breakage: the tag never printed (a day's POC and VAH captioned alike), "touches: 0" printed as
/// a fact on a profile line, or a round price aged from the epoch.
#[test]
fn a_caption_names_the_tag_and_skips_what_a_line_lacks() {
    use super::caption::{CaptionValues, caption_text, tag_key};
    use moon_chart::auto_figures::LineTag;
    crate::test_locale::warm();
    let title = RowTitle::Named("Profile".into());
    let values = |tag, touches, first_ms| CaptionValues {
        title: &title,
        tag,
        line_now: Some(100.0),
        price: Some(100.0),
        touches,
        tf: TfId::H1,
        first_ms,
        events: Default::default(),
        now_ms: (DAY0 + 3_600_000) as f64,
        tick: None,
    };
    let poc = rust_i18n::t!(tag_key(LineTag::Poc)).to_string();
    assert_eq!(
        caption_text(&[CaptionPart::RowName], &values(Some(LineTag::Poc), 0, 1.0)),
        format!("Profile · {poc}")
    );
    assert_eq!(
        caption_text(&[CaptionPart::RowName], &values(None, 0, 1.0)),
        "Profile"
    );
    assert_eq!(
        caption_text(&[CaptionPart::Touches], &values(None, 0, 1.0)),
        ""
    );
    assert!(!caption_text(&[CaptionPart::Touches], &values(None, 3, 1.0)).is_empty());
    assert_eq!(caption_text(&[CaptionPart::Age], &values(None, 0, 0.0)), "");
    assert!(!caption_text(&[CaptionPart::Age], &values(None, 0, DAY0 as f64)).is_empty());
}

/// Unnamed period-extreme lines name their own period and side, while named rows keep the row name
/// plus tag; merged captions use the same per-line name.
///
/// Breakage: restoring the combined row title on each line ("Day high and low · High"), or building
/// a merged caption from a different name than the standalone one.
#[test]
fn unnamed_period_extremes_name_each_line_and_merged_caption() {
    use super::caption::{CaptionValues, caption_halves, caption_text, line_title, merged_caption};
    use moon_chart::auto_figures::LineTag;
    use moon_core::config::auto_figures::{
        AutoFigurePreset, AutoFigureRow, DetectorKind, ExtremePeriod, PeriodExtremeParams,
    };

    let en = crate::test_locale::force("en");
    let row_for = |period| {
        AutoFigureRow::new(DetectorParams::PeriodExtreme(PeriodExtremeParams {
            period,
            ..PeriodExtremeParams::default()
        }))
    };
    let cases = [
        (ExtremePeriod::Day, "Day high", "Day low"),
        (
            ExtremePeriod::PrevDay,
            "Yesterday's high",
            "Yesterday's low",
        ),
        (ExtremePeriod::Week, "Week high", "Week low"),
        (
            ExtremePeriod::PrevWeek,
            "Last week's high",
            "Last week's low",
        ),
        (ExtremePeriod::Last4h, "4h high", "4h low"),
        (ExtremePeriod::Last12h, "12h high", "12h low"),
        (ExtremePeriod::Last24h, "24h high", "24h low"),
        (ExtremePeriod::Last48h, "48h high", "48h low"),
    ];
    for (period, high, low) in cases {
        let title = RowTitle::of(&row_for(period), DetectorKind::PeriodExtreme);
        assert_eq!(line_title(&title, Some(LineTag::High)), high);
        assert_eq!(line_title(&title, Some(LineTag::Low)), low);
    }

    let mut day = row_for(ExtremePeriod::Day);
    day.preset = Some(AutoFigurePreset::Extremes24h);
    let title = RowTitle::of(&day, DetectorKind::PeriodExtreme);
    assert_eq!(title.text(), "Day high and low");
    let high = CaptionValues {
        title: &title,
        tag: Some(LineTag::High),
        line_now: Some(0.2959),
        price: Some(0.25),
        touches: 0,
        tf: TfId::H1,
        first_ms: 0.0,
        events: Default::default(),
        now_ms: 0.0,
        tick: None,
    };
    assert_eq!(
        caption_text(&[CaptionPart::RowName, CaptionPart::Price], &high),
        "Day high · 0.2959"
    );
    let (name, rest) = caption_halves(&[CaptionPart::RowName, CaptionPart::Price], &high);
    let mut merged = String::new();
    assert!(merged_caption(
        &mut merged,
        name.as_deref().into_iter(),
        Some(&rest),
    ));
    assert_eq!(merged, "Day high · 0.2959");

    let mut named = row_for(ExtremePeriod::Day);
    named.name = "My levels".to_owned();
    let named_title = RowTitle::of(&named, DetectorKind::PeriodExtreme);
    assert_eq!(
        line_title(&named_title, Some(LineTag::High)),
        "My levels · High"
    );
    drop(en);

    let _ru = crate::test_locale::force("ru");
    let title = RowTitle::of(&day, DetectorKind::PeriodExtreme);
    let low = CaptionValues {
        title: &title,
        tag: Some(LineTag::Low),
        line_now: Some(0.2005),
        ..high
    };
    assert_eq!(
        caption_text(&[CaptionPart::RowName, CaptionPart::Price], &low),
        "Минимум дня · 0.2005"
    );
    assert_eq!(line_title(&title, Some(LineTag::High)), "Максимум дня");
    assert_eq!(
        line_title(&named_title, Some(LineTag::High)),
        "My levels · Максимум"
    );
}

/// The caption's rebuild key follows the age only where the age is printed: a line with no moment
/// of birth (a round price) keeps its key as the clock moves, an anchored line's key moves with
/// its age bucket.
///
/// Breakage: the age bucket hashed for every line (F7) — a round price's caption, which prints no
/// age, was rebuilt at every bucket of the time since the epoch.
#[test]
fn an_unprinted_age_does_not_move_the_caption_key() {
    use std::hash::Hasher as _;

    use super::caption::{CaptionValues, caption_quantum};
    let title = RowTitle::Named("Round".into());
    let key_at = |first_ms: f64, now_ms: i64| {
        let values = CaptionValues {
            title: &title,
            tag: None,
            line_now: Some(100.0),
            price: Some(100.0),
            touches: 0,
            tf: TfId::H1,
            first_ms,
            events: Default::default(),
            now_ms: now_ms as f64,
            tick: None,
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        caption_quantum(&[CaptionPart::RowName, CaptionPart::Age], &values, &mut h);
        h.finish()
    };
    // A day apart: the time since the epoch crosses a day bucket, so a hashed epoch age would move.
    let (now, day_later) = (DAY0 + 3_600_000, DAY0 + 86_400_000 + 3_600_000);
    assert_eq!(key_at(0.0, now), key_at(0.0, day_later));
    assert_ne!(key_at(DAY0 as f64, now), key_at(DAY0 as f64, day_later));
}

/// A two-line formation of `tag`: an upper side from `(upper_t0, 110)` and a lower one from
/// `(lower_t0, 100)`, both flat to 10 h, with `extra` touches after the sides' own.
fn formation(
    tag: moon_chart::auto_figures::LineTag,
    upper_t0: f64,
    lower_t0: f64,
    extra: usize,
) -> Found {
    use moon_chart::auto_figures::Seg;
    let end = 10.0 * 3_600_000.0;
    let side = |t0_ms: f64, p: f64| Seg {
        t0_ms,
        p0: p,
        t1_ms: end,
        p1: p,
    };
    let mut touches = vec![
        Touch {
            t_ms: upper_t0,
            price: 110.0,
        },
        Touch {
            t_ms: lower_t0,
            price: 100.0,
        },
    ];
    touches.extend((0..extra).map(|k| Touch {
        t_ms: end + k as f64 * 60_000.0,
        price: 110.0,
    }));
    Found {
        origin: None,
        shape: FoundShape::Pair {
            upper: side(upper_t0, 110.0),
            lower: side(lower_t0, 100.0),
        },
        first_ms: upper_t0.min(lower_t0),
        last_ms: end,
        touches,
        strength: 4.0,
        broken: None,
        touch_volume: 0.0,
        tolerance_frac: 0.002,
        tag: Some(tag),
    }
}

/// A formation keeps its key while it grows — one more touch, a flag outgrowing its cap into the
/// channel it is — and two formations with different side starts, or a double top beside a
/// triangle on the same bars, are different lines.
///
/// Breakage: the pair keyed by its merged touch list (a hidden formation reappears on the next
/// swing), by its raw tag (a hidden flag reappears as a channel), or keyed like a level (two
/// formations of one row collide).
#[test]
fn a_formation_keeps_its_key_while_it_grows() {
    use moon_chart::auto_figures::LineTag;
    let base = formation(LineTag::Flag, 3_600_000.0, 7_200_000.0, 0);
    let grown = formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 2);
    assert_eq!(LineKey::of(0, &base), LineKey::of(0, &grown));
    let other = formation(LineTag::Channel, 3_600_000.0, 5_400_000.0, 0);
    assert_ne!(LineKey::of(0, &base), LineKey::of(0, &other));
    let double = formation(LineTag::DoubleTop, 3_600_000.0, 7_200_000.0, 0);
    assert_ne!(LineKey::of(0, &base), LineKey::of(0, &double));
}

/// A formation is kept, coloured, marked and captioned by its side NEARER the price, is hidden
/// whole by one key, and pins as exactly two figures.
///
/// By hand: a 100..110 range (a channel of two flat sides, drawn as two levels) against a price of
/// 101 — the lower side is 1 away, the upper 9: kept by the lower side, below price (support), and
/// its drawn record prices the lower side, 100. Pinning gives two figures, one per side, at 110 and
/// 100. Hidden by its key, the row keeps nothing.
///
/// Breakage: the side read as the upper one wherever a reference price exists (the caption and the
/// event mark on the far side, `BySign` colouring it as resistance), pinning the drawn figures of
/// one side only, or a pair needing two keys to hide.
#[test]
fn a_formation_is_kept_drawn_and_pinned_by_its_nearer_side() {
    use moon_chart::auto_figures::{LineTag, PairSide};
    use moon_core::figures::FigureKind;
    let mut row = level_row(&[]);
    Arc::make_mut(&mut row.outcome).found =
        vec![formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 0)];
    let at = 9.0 * 3_600_000.0;
    row.ref_ms = at;
    row.kept = select_kept(&row.outcome, &rules(&row, &[]), 101.0, at, &mut Vec::new());
    assert_eq!(row.kept.len(), 1);
    assert_eq!(row.kept[0].side, Some(PairSide::Lower));
    assert!(!row.kept[0].above);

    let set = Rc::new(AutoPaneFigures::new(vec![row.clone()], Vec::new()));
    let mut drawn = AutoPaneDrawn::default();
    let mut scratch = crate::chartdx::UdScratch::default();
    let mut no_labels = Vec::new();
    append_auto_geometry(
        Some(&set),
        &ChartTheme::default(),
        1.0,
        0.0,
        &mut FigureBuffers {
            zones: &mut scratch.zones,
            hlines: &mut scratch.hlines,
            segs: &mut scratch.segs,
            markers: &mut scratch.markers,
            labels: &mut no_labels,
        },
        &mut drawn,
    );
    let rec = &drawn.lines[0];
    assert_eq!(rec.side, Some(PairSide::Lower));
    assert_eq!(rec.shape.side_price_at(at, rec.side), Some(100.0));

    let pinned: Vec<f64> = drawn
        .line_figures(rec.key)
        .iter()
        .map(|f| match &f.kind {
            FigureKind::HorizontalRay(r) => r.origin.price,
            FigureKind::HLine(h) => h.price,
            FigureKind::Segment(s) => s.a.price,
            k => panic!("{k:?}"),
        })
        .collect();
    assert_eq!(pinned, [110.0, 100.0]);

    let hidden = [rec.key];
    assert!(
        select_kept(
            &row.outcome,
            &rules(&row, &hidden),
            101.0,
            at,
            &mut Vec::new()
        )
        .is_empty()
    );
}

/// Draw `set` into fresh buffers, returning what the pane recorded and the event marks appended.
fn draw_set(set: &Rc<AutoPaneFigures>) -> (AutoPaneDrawn, Vec<moon_chart::layers::MarkerInstance>) {
    let (drawn, scratch) = draw_into_scratch(set);
    (drawn, scratch.markers)
}

/// Draw `set` into fresh buffers, returning what the pane recorded and every buffer it filled.
fn draw_into_scratch(set: &Rc<AutoPaneFigures>) -> (AutoPaneDrawn, crate::chartdx::UdScratch) {
    let mut drawn = AutoPaneDrawn::default();
    let mut scratch = crate::chartdx::UdScratch::default();
    let mut no_labels = Vec::new();
    append_auto_geometry(
        Some(set),
        &ChartTheme::default(),
        1.0,
        0.0,
        &mut FigureBuffers {
            zones: &mut scratch.zones,
            hlines: &mut scratch.hlines,
            segs: &mut scratch.segs,
            markers: &mut scratch.markers,
            labels: &mut no_labels,
        },
        &mut drawn,
    );
    (drawn, scratch)
}

/// The alpha of every line instance a draw of `set` put into the GPU buffers, levels then
/// segments.
fn drawn_alphas(set: &Rc<AutoPaneFigures>) -> Vec<f32> {
    let (_, scratch) = draw_into_scratch(set);
    let levels = scratch.hlines.iter().map(|l| l.color[3]);
    levels
        .chain(scratch.segs.iter().map(|s| s.color[3]))
        .collect()
}

/// A formation kept by its LOWER side is read at that side by every consumer of the kept side:
/// its event mark, its caption's printed price and its confluence with another timeframe's line.
///
/// By hand: the 100..110 range of `formation` against a price of 101 is kept by its lower side
/// (1 away; the upper is 9). Its event mark sits at 100, its `Price` caption prints 100, and a 4h
/// level at 100.2 — 0.2 % from 100, inside the default 0.3 % — is a confluence with it; the upper
/// side, 110, would be none of these.
///
/// Breakage: any of `event_mark_at`, `caption_values` or `mark_confluence` reading the pair
/// through `price_at` (its upper side) instead of `side_price_at(.., kept.side)`.
#[test]
fn every_reader_of_a_kept_formation_reads_the_side_it_was_kept_by() {
    use moon_chart::auto_figures::{LineTag, PairSide};
    let at = 9.0 * 3_600_000.0;
    let mut row = level_row(&[]);
    row.take.count = 5;
    Arc::make_mut(&mut row.outcome).found =
        vec![formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 0)];
    row.ref_ms = at;
    row.caption = LineCaption {
        parts: vec![CaptionPart::Price],
        place: CaptionPlace::RightEnd,
        show: CaptionShow::Always,
    };
    row.kept = select_kept(&row.outcome, &rules(&row, &[]), 101.0, at, &mut Vec::new());
    assert_eq!(row.kept.len(), 1);
    assert_eq!(row.kept[0].side, Some(PairSide::Lower));
    // An event to mark, so the mark is drawn.
    row.kept[0].events.insert(AutoEvent::Approach);

    let (mut drawn, markers) = draw_set(&Rc::new(AutoPaneFigures::new(
        vec![row.clone()],
        Vec::new(),
    )));
    assert_eq!(markers.last().map(|m| m.price), Some(100.0));
    assert!(refresh_auto_captions(&mut drawn, Some(101.0), at, None));
    assert_eq!(
        drawn.lines[0].text.as_deref(),
        Some(moon_chart::axes::format_price(100.0, None).as_str())
    );

    let set = AutoPaneFigures::new(vec![row, tf_row(1, TfId::H4, &[100.2])], Vec::new());
    let next = set.reselect(101.0, at).expect("the confluence is new");
    assert!(next.rows[0].kept[0].events.contains(AutoEvent::Confluence));
    assert!(next.rows[1].kept[0].events.contains(AutoEvent::Confluence));
}

/// A converging formation's caption is placed and priced on its DRAWN side: the side ends at the
/// apex, so the line's record spans to the apex and its `Price` caption prints the price there.
///
/// By hand: the top falls 110 → 106 over 1..3 h (−2/h), the bottom rises 90 → 92 over 2..4 h
/// (+1/h); they meet at 8 h at 96. Drawn at a reference of 20 h, each side ends at 8 h, and the
/// caption read at 20 h prints 96 — where an extrapolated side would print 72 (the top) or 108
/// (the bottom), far outside the triangle, at the plot edge.
///
/// Breakage: the span taken from the row's `extend` alone (`RightEdge` → to the plot edge), or the
/// printed price read at "now" past the drawn end (review finding F1).
#[test]
fn a_triangles_caption_is_placed_and_priced_on_its_drawn_side() {
    use moon_chart::auto_figures::{LineTag, Seg};
    let h = 3_600_000.0;
    let (upper, lower) = (
        Seg {
            t0_ms: h,
            p0: 110.0,
            t1_ms: 3.0 * h,
            p1: 106.0,
        },
        Seg {
            t0_ms: 2.0 * h,
            p0: 90.0,
            t1_ms: 4.0 * h,
            p1: 92.0,
        },
    );
    let mut found = formation(LineTag::Triangle, h, 2.0 * h, 0);
    found.shape = FoundShape::Pair { upper, lower };
    found.last_ms = 4.0 * h;
    let now = 20.0 * h;
    let mut row = level_row(&[]);
    row.take.count = 5;
    Arc::make_mut(&mut row.outcome).found = vec![found];
    row.ref_ms = now;
    row.caption = LineCaption {
        parts: vec![CaptionPart::Price],
        place: CaptionPlace::RightEnd,
        show: CaptionShow::Always,
    };
    row.kept = select_kept(&row.outcome, &rules(&row, &[]), 95.0, now, &mut Vec::new());
    assert_eq!(row.kept.len(), 1);

    let (mut drawn, _) = draw_set(&Rc::new(AutoPaneFigures::new(vec![row], Vec::new())));
    let (_, end) = drawn.lines[0].span;
    assert!((end - 8.0 * h).abs() < 1.0, "{end}");
    assert!(refresh_auto_captions(&mut drawn, Some(95.0), now, None));
    assert_eq!(
        drawn.lines[0].text.as_deref(),
        Some(moon_chart::axes::format_price(96.0, None).as_str())
    );
}

/// The tags of every row's kept lines, row by row, in kept order.
fn kept_tags(set: &AutoPaneFigures) -> Vec<Vec<Option<moon_chart::auto_figures::LineTag>>> {
    set.rows
        .iter()
        .map(|r| r.kept.iter().map(|k| r.outcome.found[k.ix].tag).collect())
        .collect()
}

/// One formation that a "Channels" row and a "Flags" row both keep is drawn ONCE, as the flag —
/// although the channel row comes first — while a formation with other sides stays drawn beside
/// it; hiding the drawn flag hides the channel reading of it too (and nothing takes its place);
/// and the publication's pass (`select_all`) draws what the price pass draws.
///
/// By hand: both rows hold a pair from (1 h, 110) over (2 h, 100), the channel row's grown by two
/// touches (the same formation, `a_formation_keeps_its_key_while_it_grows`); against 101 at 9 h
/// each row alone would keep it by its lower side. A channel from (1 h, 110) over (1.5 h, 100) has
/// another lower start: another formation.
///
/// Breakage: the rows' readings drawn side by side (two captions on one pair of lines), the
/// channel winning by its row order, twins told apart by their tags or merged touch lists, or a
/// hidden flag letting its channel twin take its place.
#[test]
fn one_formation_two_rows_find_is_drawn_once_under_the_flags_name() {
    use moon_chart::auto_figures::LineTag;
    let row = |row_ix: usize, found: Vec<Found>| {
        let mut r = tf_row(row_ix, TfId::H1, &[]);
        Arc::make_mut(&mut r.outcome).found = found;
        r
    };
    let flag = formation(LineTag::Flag, 3_600_000.0, 7_200_000.0, 0);
    let channel = formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 2);
    let other = formation(LineTag::Channel, 3_600_000.0, 5_400_000.0, 0);
    let at = 9.0 * 3_600_000.0;

    let rows = vec![row(0, vec![channel.clone()]), row(1, vec![flag.clone()])];
    let drawn = AutoPaneFigures::new(rows.clone(), Vec::new())
        .reselect(101.0, at)
        .expect("the first selection draws lines");
    assert_eq!(kept_tags(&drawn), [vec![], vec![Some(LineTag::Flag)]]);

    // The publication's own pass agrees with the price pass.
    let mut published = AutoPaneFigures::new(rows, Vec::new());
    published.select_all(&[101.0; 2], at);
    assert_eq!(kept_tags(&published), kept_tags(&drawn));

    // Other sides: another formation, drawn as well.
    let both = AutoPaneFigures::new(
        vec![row(0, vec![other]), row(1, vec![flag.clone()])],
        Vec::new(),
    )
    .reselect(101.0, at)
    .expect("the first selection draws lines");
    assert_eq!(
        kept_tags(&both),
        [vec![Some(LineTag::Channel)], vec![Some(LineTag::Flag)]]
    );

    // Hiding the drawn flag hides the formation, not only the flag row's reading of it.
    let hidden = vec![LineKey::of(1, &flag)];
    let mut set = AutoPaneFigures::new(
        vec![row(0, vec![channel]), row(1, vec![flag])],
        hidden.clone(),
    );
    set.select_all(&[101.0; 2], at);
    assert_eq!(kept_tags(&set), [vec![], vec![]]);
}

/// The row that loses a kept twin takes its next line in the slot, once, and the drawn formation
/// shows the events its twin's row marks.
///
/// By hand: a "Channels" row keeping one line per side, nearest first, finds the flag's channel
/// reading (sides 110 / 100) and a lower channel (sides 99 / 90, shifted by 11); against 101 the
/// nearest below is the twin's 100 (1 away), then the other channel's 99 (2 away). The "Flags" row
/// finds the flag and marks nothing but bounces; the channel row marks approaches at 2 %. So the
/// channel row draws the LOWER channel — the twin's slot goes to its next line, it does not stay
/// empty — the flag row draws the flag, and the flag shows an approach (101 is 1 % from 100, inside
/// the channel row's 2 %), which its own row does not mark. The same holds after a price pass.
///
/// Breakage: the lost twin's slot left empty (the channel row keeps only the twin, which is then
/// removed, and draws nothing below), or the twin's marks dropped with it (the flag shows no
/// approach).
#[test]
fn a_twin_leaves_its_rows_count_and_hands_its_marks_to_the_flag() {
    use moon_chart::auto_figures::LineTag;
    let row = |row_ix: usize, found: Vec<Found>, marks: EventMarks| {
        let mut r = tf_row(row_ix, TfId::H1, &[]);
        Arc::make_mut(&mut r.outcome).found = found;
        r.take.count = 1;
        r.marks = marks;
        r
    };
    let shifted = |f: Found, by: f64| {
        let FoundShape::Pair {
            mut upper,
            mut lower,
        } = f.shape
        else {
            unreachable!("a formation is a pair");
        };
        upper.p0 -= by;
        upper.p1 -= by;
        lower.p0 -= by;
        lower.p1 -= by;
        Found {
            origin: None,
            shape: FoundShape::Pair { upper, lower },
            ..f
        }
    };
    let quiet = EventMarks {
        approach: false,
        squeeze: false,
        breakout: false,
        false_break: false,
        bounce: true,
        role_flip: false,
        stop_hunt: false,
        confluence: false,
        ..EventMarks::default()
    };
    let approaching = EventMarks {
        approach: true,
        approach_pct: 2.0,
        bounce: false,
        ..quiet
    };
    let flag = formation(LineTag::Flag, 3_600_000.0, 7_200_000.0, 0);
    let twin = formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 1);
    let lower = shifted(
        formation(LineTag::Channel, 3_000_000.0, 6_000_000.0, 0),
        11.0,
    );
    let at = 9.0 * 3_600_000.0;
    let mut set = AutoPaneFigures::new(
        vec![
            row(0, vec![twin, lower], approaching),
            row(1, vec![flag], quiet),
        ],
        Vec::new(),
    );
    set.select_all(&[101.0; 2], at);
    let kept_ix = |set: &AutoPaneFigures| -> Vec<Vec<usize>> {
        set.rows
            .iter()
            .map(|r| r.kept.iter().map(|k| k.ix).collect())
            .collect()
    };
    assert_eq!(kept_ix(&set), [vec![1], vec![0]]);
    assert!(set.rows[1].kept[0].events.contains(AutoEvent::Approach));
    assert!(!set.rows[1].marks.approach);

    let moved = set.reselect(101.5, at).unwrap_or(set);
    assert_eq!(kept_ix(&moved), [vec![1], vec![0]]);
    assert!(moved.rows[1].kept[0].events.contains(AutoEvent::Approach));
}

/// A cache with the core's ring kind beside kind 5: `ring` answers the ring kind, `None` a read
/// that did not happen.
struct RingCache {
    m5: Vec<ChartCandle>,
    ring: Option<Vec<ChartCandle>>,
}

impl BarCache for RingCache {
    fn read(&self, kind_min: u32, _from_ms: i64, _to_ms: i64) -> Option<Vec<ChartCandle>> {
        match kind_min {
            5 => Some(self.m5.clone()),
            moon_core::market::kline_cache::RING_5M_KIND => self.ring.clone(),
            _ => Some(Vec::new()),
        }
    }
}

/// Rows of the ring kind reach the row's bars with an unknown open and close, and a real candle
/// (kind 5) replaces them in its bucket — whatever either looks like: of a day and a half of ring
/// rows the last hour is real, and one of those real candles has the ring's own shape (it opened
/// at its high and closed at its low), so the newest H1 bar closes at that real low and the older
/// ones have no close. The reference price is the real close.
///
/// Breakage: the ring's placeholder `close == low` read as a close (the older H1 bars closing at
/// their lows), the mark taken from the shape (the real falling candle losing its close), or a
/// ring row shadowing a real one.
#[test]
fn ring_kind_rows_have_no_close_and_real_rows_keep_theirs() {
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    let from = DAY0 - 86_400_000;
    let candle = |i: i64, open: f32, high: f32, low: f32, close: f32| ChartCandle {
        t_open_ms: (from + i * 5 * MIN) as f64,
        open,
        high,
        low,
        close,
        volume: 1.0,
        quote_volume: 1.0,
    };
    let n = 288 + 144;
    let ring: Vec<ChartCandle> = (0..n)
        .map(|i| candle(i, 102.0, 102.0, 98.0, 98.0))
        .collect();
    // The last hour before 12:00 is real: opens at 99, closes at 101 — but its last candle fell
    // from its high to its low.
    let mut m5: Vec<ChartCandle> = (n - 12..n)
        .map(|i| candle(i, 99.0, 102.0, 98.0, 101.0))
        .collect();
    *m5.last_mut().expect("real rows") = candle(n - 1, 102.0, 102.0, 98.0, 98.0);
    let cache = RingCache {
        m5,
        ring: Some(ring),
    };
    let job = PaneJob {
        rows: vec![(0, key(TfId::H1, now, 30))],
        chart_tf_ms: 5 * MIN,
        chart: Vec::new().into(),
        cache: None,
        now_ms: now,
        price_tick: None,
        diag: None,
    };
    let out = run(&job, Some(&cache));
    let o = &out[0].1;
    assert_eq!(o.source, BarSource::Cache5m);
    assert!(!o.read_failed);
    let last = o.recent.last().expect("bars");
    assert_eq!((last.open, last.close), (99.0, 98.0));
    let older = &o.recent[o.recent.len() - 2];
    assert!(older.close.is_nan() && older.open.is_nan(), "{older:?}");
    assert_eq!((older.high, older.low), (102.0, 98.0));
    assert_eq!(o.last_close, Some(98.0));
}

/// A ring-kind read that did not happen fails the row like a kind-5 read that did not — it is
/// retried, rather than trusted on half its history.
///
/// Breakage: the missing ring read taken as "no ring rows", an answer short of the hours the ring
/// covers, never recomputed until the next bar closes.
#[test]
fn a_ring_read_that_did_not_happen_is_a_failure() {
    let now = (DAY0 + 12 * 3_600_000 + 10_000) as f64;
    let cache = RingCache {
        m5: series(DAY0 - 86_400_000, 5 * MIN, 288 + 144),
        ring: None,
    };
    let job = PaneJob {
        rows: vec![(0, key(TfId::H1, now, 30))],
        chart_tf_ms: 5 * MIN,
        chart: Vec::new().into(),
        cache: None,
        now_ms: now,
        price_tick: None,
        diag: None,
    };
    let out = run(&job, Some(&cache));
    assert_eq!(out[0].1.source, BarSource::Cache5m);
    assert!(out[0].1.read_failed);
    assert_eq!(out[0].1.bars, 0);
}

/// A formation the flag row does NOT keep stays with the channel row that does, under the
/// channel's name: against 101 the pair (110 over 100) is kept by its lower side, so a flag row
/// taking only lines above price — or none at all — keeps nothing, and the channel row draws it.
///
/// Breakage: twins judged over the FOUND lines — the flag row's unkept reading takes the formation
/// away from the channel row, and the chart draws neither.
#[test]
fn a_twin_the_flag_row_does_not_keep_stays_with_the_channel_row() {
    use moon_chart::auto_figures::LineTag;
    let row = |row_ix: usize, found: Vec<Found>| {
        let mut r = tf_row(row_ix, TfId::H1, &[]);
        Arc::make_mut(&mut r.outcome).found = found;
        r
    };
    let flag = formation(LineTag::Flag, 3_600_000.0, 7_200_000.0, 0);
    let channel = formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 2);
    let at = 9.0 * 3_600_000.0;
    let mut above_only = row(1, vec![flag.clone()]);
    above_only.take.side = TakeSide::Above;
    let mut none = row(1, vec![flag]);
    none.take.count = 0;
    for flags in [above_only, none] {
        let mut set = AutoPaneFigures::new(vec![row(0, vec![channel.clone()]), flags], Vec::new());
        set.select_all(&[101.0; 2], at);
        assert_eq!(kept_tags(&set), [vec![Some(LineTag::Channel)], vec![]]);
        assert!(set.reselect(101.0, at).is_none(), "the price pass agrees");
    }
}

/// The set's one opacity reaches everything it draws: each line instance's alpha in the GPU
/// buffers and the alpha its caption prints at — the colour itself stays the row's — and a new set
/// draws opaque until the publisher sets it.
///
/// By hand: at 0.5 a figure's alpha byte is 255 · 0.5 = 127.5 → 128, so every instance of the
/// faded draw is 128 / 255 of the same instance drawn opaque.
///
/// Breakage: the opacity copied into the set but not applied by the geometry, or applied to the
/// lines and not handed to the captions (a faded line under a loud name).
#[test]
fn the_sets_opacity_fades_every_line_and_its_caption() {
    let opaque = Rc::new(published(&[99.0, 101.0], Vec::new(), 100.0));
    assert_eq!(opaque.opacity, 1.0);
    let full = drawn_alphas(&opaque);
    assert!(!full.is_empty());

    let mut faded = published(&[99.0, 101.0], Vec::new(), 100.0);
    faded.opacity = 0.5;
    let faded = Rc::new(faded);
    let (drawn, _) = draw_set(&faded);
    assert!(!drawn.lines.is_empty());
    for line in &drawn.lines {
        assert_eq!(line.opacity, 0.5);
    }
    let alphas = drawn_alphas(&faded);
    assert_eq!(alphas.len(), full.len());
    for (f, o) in alphas.iter().zip(&full) {
        assert!((f / o - 128.0 / 255.0).abs() < 1e-4, "{f} of {o}");
    }
}

/// A pinned line is the reader's own figure: at full opacity whatever the block's, in the line's
/// own colour. The block is drawn at 60 % (an alpha byte of 255 · 0.6 = 153 on its figures); what
/// "Pin as figure" copies — a level, and both sides of a formation — carries 255.
///
/// Breakage: a level pinned as a copy of its drawn figures (the block's fade baked into the user's
/// figure for good), or only a formation's sides rebuilt at full opacity.
#[test]
fn a_pinned_line_is_at_full_opacity_whatever_the_blocks() {
    use moon_chart::auto_figures::LineTag;
    let mut levels = level_row(&[99.0, 101.0]);
    levels.kept = select_kept(
        &levels.outcome,
        &rules(&levels, &[]),
        100.0,
        0.0,
        &mut Vec::new(),
    );
    let mut pair = level_row(&[]);
    pair.row_ix = 1;
    Arc::make_mut(&mut pair.outcome).found =
        vec![formation(LineTag::Channel, 3_600_000.0, 7_200_000.0, 0)];
    let at = 9.0 * 3_600_000.0;
    pair.ref_ms = at;
    pair.kept = select_kept(
        &pair.outcome,
        &rules(&pair, &[]),
        101.0,
        at,
        &mut Vec::new(),
    );
    let mut set = AutoPaneFigures::new(vec![levels, pair], Vec::new());
    set.opacity = 0.6;
    let set = Rc::new(set);
    let (drawn, _) = draw_set(&set);
    assert!(drawn.lines.iter().all(|l| l.opacity == 0.6));
    assert!(drawn.lines.iter().any(|l| l.tag == Some(LineTag::Channel)));
    assert!(drawn.lines.iter().any(|l| l.tag != Some(LineTag::Channel)));
    for line in &drawn.lines {
        let figs = drawn.line_figures(line.key);
        assert!(!figs.is_empty(), "{:?}", line.tag);
        let rgb = [
            (line.color >> 16) as u8,
            (line.color >> 8) as u8,
            line.color as u8,
        ];
        for f in &figs {
            assert_eq!(f.color, [rgb[0], rgb[1], rgb[2], 255], "{:?}", line.tag);
        }
    }
}

/// The caption's halves are built with it: the row's name, and the caption without it — what the
/// text pass joins when another row's line shares the price — and no name for a caption that
/// prints none.
///
/// Breakage: the halves left empty (a shared price would print the lead's caption alone, the other
/// row's name lost), or the name kept in the rest (printed twice when joined).
#[test]
fn a_captions_name_and_rest_are_built_with_it() {
    let set = Rc::new(published(&[101.0], Vec::new(), 100.0));
    let (mut drawn, _) = draw_set(&set);
    assert!(refresh_auto_captions(&mut drawn, Some(100.0), 0.0, None));
    let price = moon_chart::axes::format_price(101.0, None);
    let line = &drawn.lines[0];
    assert_eq!(line.name.as_deref(), Some("Levels"));
    assert_eq!(line.rest.as_deref(), Some(price.as_str()));
    assert_eq!(
        line.text.as_deref(),
        Some(format!("Levels · {price}").as_str())
    );

    let mut bare = level_row(&[101.0]);
    bare.caption.parts = vec![CaptionPart::Price];
    bare.kept = select_kept(
        &bare.outcome,
        &rules(&bare, &[]),
        100.0,
        0.0,
        &mut Vec::new(),
    );
    let (mut drawn, _) = draw_set(&Rc::new(AutoPaneFigures::new(vec![bare], Vec::new())));
    assert!(refresh_auto_captions(&mut drawn, Some(100.0), 0.0, None));
    assert_eq!(drawn.lines[0].name, None);
    assert_eq!(drawn.lines[0].rest.as_deref(), Some(price.as_str()));
}
