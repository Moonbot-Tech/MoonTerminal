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
