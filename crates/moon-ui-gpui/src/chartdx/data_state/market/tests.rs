//! Regression tests for the chart-history floor calculation.

use moon_core::market::CandleViewCfg;
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_TF_CHOICES_MIN, CANDLE_ZONE_MAX,
};

use super::{chart_history_floor_ms, hide_start_rel};

/// `market.rs:chart_history_floor_ms` must keep every supported candle timeframe within the
/// 120-to-1500-bar request band; swapping the clamps or lowering the floor would silently request
/// too much history or make a chart appear empty near the current time.
#[test]
fn chart_history_floor_stays_finite_and_inside_the_bar_band() {
    let mut five_minute_bars = None;
    let mut thirty_minute_bars = None;

    for tf_min in CANDLE_TF_CHOICES_MIN {
        let cfg = CandleViewCfg {
            tf_min,
            mode: CANDLE_MODE_FILLED,
            ..CandleViewCfg::default()
        };
        let floor_ms = chart_history_floor_ms(cfg) as f64;
        let bars = floor_ms / cfg.tf_ms() as f64;

        assert!(floor_ms.is_finite());
        assert!(floor_ms >= 0.0);
        assert!((120.0..=1500.0).contains(&bars));

        if tf_min == 5 {
            five_minute_bars = Some(bars);
        }
        if tf_min == 30 {
            thirty_minute_bars = Some(bars);
        }
    }

    assert!(
        five_minute_bars.expect("the supported set includes five minutes")
            > thirty_minute_bars.expect("the supported set includes thirty minutes")
    );
}

/// `market.rs:chart_history_floor_ms` must return zero for CANDLE_MODE_OFF; dropping that early
/// return would rebuild and upload candle history even though the user selected a pure tick chart.
#[test]
fn chart_history_floor_is_zero_when_candles_are_off() {
    let cfg = CandleViewCfg {
        mode: CANDLE_MODE_OFF,
        ..CandleViewCfg::default()
    };

    assert_eq!(chart_history_floor_ms(cfg), 0.0);
}

/// A caption measuring around the pointer orders NO read while the pointer is off the pane.
///
/// Breakage: falling back to the live edge would answer a different question under a heading that
/// says "cursor", and the reader would have no way to tell which one they are looking at.
#[test]
fn a_measuring_period_is_dropped_without_a_pointer() {
    use moon_core::config::{LabelSpan, LabelWindow, SpanAnchor, VolumeSpanKey};
    use moon_core::market::{VolumeAt, VolumeSpan};

    let live = VolumeSpanKey {
        span: LabelSpan::Window,
        window: LabelWindow::M1,
        anchor: SpanAnchor::Now,
        liquidations: false,
    };
    let measured = VolumeSpanKey {
        anchor: SpanAnchor::Cursor,
        ..live
    };

    let without = super::resolve_span_keys(&[live, measured], None);
    assert_eq!(
        without,
        vec![(VolumeSpan::Millis(60_000), VolumeAt::Now)],
        "only the live-edge period survives with no pointer"
    );

    let with = super::resolve_span_keys(&[live, measured], Some(1_700_000_000_000));
    assert_eq!(
        with.len(),
        2,
        "both periods are read once the pointer lands"
    );
    assert!(with.contains(&(VolumeSpan::Millis(60_000), VolumeAt::Now)));
    assert!(with.contains(&(
        VolumeSpan::Millis(60_000),
        VolumeAt::Around(1_700_000_000_000)
    )));
}

/// Two captions over one period are ONE read, however many figures they print.
#[test]
fn one_period_is_resolved_once() {
    use moon_core::config::{LabelSpan, LabelWindow, SpanAnchor, VolumeSpanKey};

    let key = VolumeSpanKey {
        span: LabelSpan::Window,
        window: LabelWindow::M5,
        anchor: SpanAnchor::Now,
        liquidations: true,
    };
    assert_eq!(super::resolve_span_keys(&[key, key], None).len(), 1);
}

/// The pointer's refresh may replace only what the pointer owns.
///
/// Breakage: the two anchors are refreshed on different clocks — the market's and the mouse's — so
/// replacing the set wholesale on a mouse move would blank every live-edge figure until the next
/// market revision, which on a quiet coin is a visible hole.
#[test]
fn a_pointer_refresh_leaves_the_live_edge_entries_alone() {
    use moon_core::market::{VolumeAt, VolumeSpan};

    let live = (VolumeSpan::Millis(60_000), VolumeAt::Now);
    let old_point = (VolumeSpan::Millis(60_000), VolumeAt::Around(1_000));
    let new_point = (VolumeSpan::Millis(60_000), VolumeAt::Around(2_000));

    let mut held = vec![(live, 1u32), (old_point, 2u32)];
    super::merge_readouts(&mut held, vec![(new_point, 3u32)]);

    assert!(held.contains(&(live, 1)), "the live-edge reading survives");
    assert!(
        !held.iter().any(|(key, _)| *key == old_point),
        "the stale point is gone"
    );
    assert!(held.contains(&(new_point, 3)), "the fresh point is in");
}

/// The pointer leaving drops what it was measuring, rather than freezing it.
#[test]
fn a_pointer_leaving_clears_only_its_own_entries() {
    use moon_core::market::{VolumeAt, VolumeSpan};

    let live = (VolumeSpan::Millis(60_000), VolumeAt::Now);
    let point = (VolumeSpan::Millis(60_000), VolumeAt::Around(1_000));
    let mut held = vec![(live, 1u32), (point, 2u32)];

    super::merge_readouts(&mut held, Vec::new());

    assert_eq!(held, vec![(live, 1)]);
}

/// Width and zoom invalidate the fit even with no pan; subpixel follow motion stays cheap.
#[test]
fn price_fit_cache_tracks_viewport_and_accumulates_subpixel_motion() {
    let cached = Some((1000.0, 60000.0, 0.01));
    assert!(!super::price_fit_window_changed(
        cached,
        (1050.0, 60000.0, 0.01)
    ));
    assert!(super::price_fit_window_changed(
        cached,
        (1100.0, 60000.0, 0.01)
    ));
    assert!(super::price_fit_window_changed(
        cached,
        (1000.0, 30000.0, 0.01)
    ));
    assert!(super::price_fit_window_changed(
        cached,
        (1000.0, 60000.0, 0.02)
    ));
    assert!(super::price_fit_window_changed(
        None,
        (1000.0, 60000.0, 0.01)
    ));
}

/// Measured on 2026-09-13 (Main pane, 5m, bucket open at 524454 rel): with `hide_candles = 3` the
/// boundary was `-52298` — the first trade's timestamp, 23 s past the open of the third-from-last
/// bucket at `-75546` — so that bucket kept its candle and two were hidden instead of three.
#[test]
fn hide_zone_boundary_snaps_the_first_trade_to_its_bucket_open() {
    let tf_ms = 300_000;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let bucket_open_rel =
        (moon_core::market::candles::bucket_open_ms(now_ms, tf_ms) - epoch_ms) as f32;
    let hide_open_rel = bucket_open_rel - 2.0 * tf_ms as f32;
    // First resident trade 23 s into the third-from-last bucket.
    let first_trade_rel = hide_open_rel + 23_000.0;
    let boundary = hide_start_rel(3, now_ms, tf_ms, epoch_ms, first_trade_rel);
    assert_eq!(
        boundary, hide_open_rel,
        "the bucket holding the first trade must be hidden too"
    );
}

/// The zone never reaches left of the first resident trade: with trades starting in the last
/// bucket only, `hide_candles = 3` hides that one bucket, not three.
#[test]
fn hide_zone_boundary_is_clamped_at_the_first_trade_bucket() {
    let tf_ms = 300_000;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let bucket_open_rel =
        (moon_core::market::candles::bucket_open_ms(now_ms, tf_ms) - epoch_ms) as f32;
    let first_trade_rel = bucket_open_rel + 90_000.0;
    let boundary = hide_start_rel(3, now_ms, tf_ms, epoch_ms, first_trade_rel);
    assert_eq!(boundary, bucket_open_rel);
}

#[test]
fn hide_zone_is_off_without_a_count_or_without_resident_trades() {
    let tf_ms = 300_000;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    assert_eq!(hide_start_rel(0, now_ms, tf_ms, epoch_ms, 0.0), f32::MAX);
    assert_eq!(
        hide_start_rel(3, now_ms, tf_ms, epoch_ms, f32::NAN),
        f32::MAX
    );
}

/// Hide Max on the chart path must use the oldest trade's bucket, not a candle count back
/// from now. Treating the sentinel as a huge `hide_candles` would blank the whole series.
#[test]
fn hide_max_follows_the_oldest_trade_bucket_and_stays_off_without_one() {
    let tf_ms = 300_000_i64;
    let epoch_ms = 1_700_000_000_000.0;
    let now_ms = epoch_ms + 637_474.0;
    let oldest_rel = 410_000.0;
    let abs = epoch_ms + oldest_rel as f64;
    let expected = ((abs / tf_ms as f64).floor() * tf_ms as f64 - epoch_ms) as f32;
    assert_eq!(
        hide_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, oldest_rel),
        expected
    );
    assert_eq!(
        hide_start_rel(CANDLE_ZONE_MAX, now_ms, tf_ms, epoch_ms, f32::NAN),
        f32::MAX
    );
}

/// `data_state/market.rs:book_instances_stale` + `orderbook.rs:plan_book_bake`: keying staleness
/// on the exact window edges (or its derived width) rebuilds the book instances and rebakes the
/// glass on every frame of a live follow; leaving the emitted window must still rebuild at once,
/// and a backend without a margin keeps the exact old `(rev, lo, hi)` rule.
#[cfg(windows)]
#[test]
fn live_follow_inside_the_book_margin_keeps_instances_and_bitmap() {
    use crate::chartdx::orderbook::{BookBakeKey, book_v_margin_px, plan_book_bake};

    use super::book_instances_stale;

    let epoch = 1.7e12;
    let mut view = moon_chart::view::ChartView::new(epoch);
    let area = moon_chart::view::Rect {
        x: 0.0,
        y: 0.0,
        w: 1000.0,
        h: 600.0,
    };
    let mut now = epoch + 16.0;
    view.update_y(now, area.h, Some((50.0, 150.0)), Some(100.0));
    let gpu = |v: &moon_chart::view::ChartView| {
        crate::chartdx::view::view_gpu(
            v,
            area,
            [area.w, area.h],
            1.0,
            crate::chartdx::view::ViewStyle::default(),
        )
    };
    let window = |v: &moon_chart::view::ChartView| {
        let half = v.render_range.max(1e-9) * 0.5;
        (v.render_center - half, v.render_center + half)
    };
    let v_margin = book_v_margin_px(area.h);
    let g0 = gpu(&view);
    let margin_price = v_margin / g0.price_to_px;
    let rev = 7u64;
    let (lo0, hi0) = window(&view);
    let mut emit = (lo0 - margin_price, hi0 + margin_price);
    let mut last_range = view.render_range;
    let mut last_lo_hi = (lo0, hi0);
    let key = BookBakeKey {
        tex_h_total: area.h as u32 + 2 * v_margin as u32,
        v_margin,
        bake_p0: g0.view_price0 - v_margin / g0.price_to_px,
        price_to_px: g0.price_to_px,
        baked: true,
    };

    let (mut old_rebuilds, mut new_rebuilds, mut bakes) = (0, 0, 0);
    for i in 1..=120 {
        now += 16.0;
        let last = 100.0 + 25.0 * i as f32 / 120.0;
        view.update_y(now, area.h, Some((last - 50.0, last + 50.0)), Some(last));
        let (lo, hi) = window(&view);
        if (lo, hi) != last_lo_hi {
            old_rebuilds += 1;
        }
        if book_instances_stale(
            rev,
            emit,
            last_range,
            last_lo_hi,
            rev,
            view.render_range,
            lo,
            hi,
            margin_price,
        ) {
            new_rebuilds += 1;
            emit = (lo - margin_price, hi + margin_price);
            last_range = view.render_range;
        }
        last_lo_hi = (lo, hi);
        if plan_book_bake(&key, &gpu(&view), false, false, false).bake {
            bakes += 1;
        }
    }
    println!("[G1] book rebuilds per 120 follow syncs: old={old_rebuilds} new={new_rebuilds}");
    assert!(
        old_rebuilds > 0,
        "the drive must actually move the book window"
    );
    assert_eq!(
        new_rebuilds, 0,
        "book instances rebuilt inside the emitted window"
    );
    assert_eq!(bakes, 0, "book glass rebaked inside its margin");

    // Leaving the emitted window rebuilds, and that rebuild bakes immediately.
    let (lo, hi) = (emit.0 - 1.0, emit.0 - 1.0 + view.render_range);
    assert!(book_instances_stale(
        rev,
        emit,
        last_range,
        last_lo_hi,
        rev,
        view.render_range,
        lo,
        hi,
        margin_price
    ));
    let window_rebuilt = !(lo >= emit.0) || !(hi <= emit.1);
    let plan = plan_book_bake(&key, &gpu(&view), false, false, window_rebuilt);
    assert!(
        plan.bake && plan.immediate,
        "leaving the window must bake at once"
    );

    // No margin (Metal/wgpu): exactly the old (rev, lo, hi) inequality.
    let table = [
        (7u64, (1.0f32, 2.0f32)),
        (8, (1.0, 2.0)),
        (7, (1.0, 2.5)),
        (7, (0.5, 2.0)),
        (7, (1.5, 1.8)),
    ];
    for (r, (l, h)) in table {
        let stale = book_instances_stale(7, (1.0, 2.0), 1.0, (1.0, 2.0), r, 1.0, l, h, 0.0);
        assert_eq!(
            stale,
            r != 7 || (l, h) != (1.0, 2.0),
            "rev {r} lo {l} hi {h}"
        );
    }
}

/// `market.rs:volume_sample_timeframe_bound` seeding the patch fold at `0.0` drops a wide row
/// the patch does not touch.
///
/// The user-visible consequence is the volume band clipping a coarse candle that is still
/// retained. A wider tail is the other direction and must raise the bound.
#[test]
fn patch_bound_keeps_wide_row_and_wider_tail() {
    use super::{CandleApply, volume_sample_timeframe_bound};

    let mut samples = Vec::with_capacity(8);
    for i in 0..8 {
        samples.push(moon_chart::VolumeSample {
            t_open_ms: i as f64,
            tf_ms: if i == 1 { 3_600_000.0 } else { 60_000.0 },
            quote_volume: 1.0,
        });
    }
    let previous = 3_600_000.0;
    let suffix_max = samples[4..]
        .iter()
        .map(|sample| sample.tf_ms)
        .fold(0.0, f64::max);
    assert!(
        suffix_max < previous,
        "fixture suffix is narrower than row 1"
    );
    assert_eq!(
        volume_sample_timeframe_bound(&samples, &CandleApply::Patch(4), previous),
        previous.max(suffix_max),
        "a patch must keep the older wide row"
    );

    samples[7].tf_ms = 7_200_000.0;
    let raised_suffix = samples[4..]
        .iter()
        .map(|sample| sample.tf_ms)
        .fold(0.0, f64::max);
    assert!(raised_suffix > previous, "fixture tail is a new maximum");
    assert_eq!(
        volume_sample_timeframe_bound(&samples, &CandleApply::Patch(4), previous),
        previous.max(raised_suffix),
        "a wider tail must raise the bound"
    );
}
