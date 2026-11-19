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
use super::select::select_kept;
use super::{
    AutoPaneFigures, AutoRowDrawn, LineKey, PriceStep, RowTitle, native_closed_candles, price_step,
    resegment, resolve_rgb,
};
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
        ref_ms: 0.0,
    }
}

/// A set of one level row selected against `price`, the way the panel publishes it.
fn published(levels: &[f64], hidden: Vec<LineKey>, price: f64) -> AutoPaneFigures {
    let mut row = level_row(levels);
    row.kept = select_kept(
        &row.outcome,
        &row.rules(&hidden),
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
    row.kept = select_kept(&row.outcome, &row.rules(&[]), 100.0, 0.0, &mut Vec::new());
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
    band.kept = select_kept(&band.outcome, &band.rules(&[]), 100.0, 0.0, &mut Vec::new());
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
/// one to the others. The publication's own pass (`mark_confluence`) agrees with the price pass.
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
    for row in &mut rows {
        row.kept = select_kept(&row.outcome, &row.rules(&[]), 105.0, 0.0, &mut Vec::new());
    }
    let mut published = AutoPaneFigures::new(rows, Vec::new());
    published.mark_confluence(0.0);
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

    // An hour row of round prices on that chart: twelve hours cannot hold the hour bars the events
    // read, so it goes to the cache, and the read reaches back far enough for them.
    let cache = FakeCache {
        m5: Some(series(DAY0 - 2 * 86_400_000, 5 * MIN, 2 * 288 + 144)),
        reads: RefCell::new(Vec::new()),
    };
    let job = PaneJob {
        rows: vec![(0, round(TfId::H1))],
        chart_tf_ms: 5 * MIN,
        chart,
        cache: None,
        now_ms: now,
    };
    let out = run(&job, Some(&cache));
    assert_eq!(out[0].1.source, BarSource::Cache5m);
    assert!(out[0].1.recent.len() >= wanted);
    let last_closed = last_closed_open_ms(now, TfId::H1);
    let reads = cache.reads.borrow();
    assert!(reads[0].1 <= last_closed - (wanted as i64 - 1) * 3_600_000);
}

/// "Not enough history" from 90 % of the detector's need (§13.3): the 4h levels row of the scenes'
/// set reads 68 of its 72 bars and has its answer; 64 of 72 (88.9 %) is short, 65 (90.3 %) is not.
///
/// Breakage: the old `bars < needed` (every hole in the cache read as missing history), or the
/// threshold measured against the events tail.
#[test]
fn a_row_has_its_history_from_ninety_percent_of_what_it_needs() {
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
