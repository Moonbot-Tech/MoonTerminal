//! Regression tests for the chart-history floor calculation.

use moon_core::market::CandleViewCfg;
use moon_core::market::candles::{CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_TF_CHOICES_MIN};

use super::chart_history_floor_ms;

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
    assert_eq!(with.len(), 2, "both periods are read once the pointer lands");
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
    assert!(!held.iter().any(|(key, _)| *key == old_point), "the stale point is gone");
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
