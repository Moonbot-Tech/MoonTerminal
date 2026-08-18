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
