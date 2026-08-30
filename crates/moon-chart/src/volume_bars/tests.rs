use super::*;
use moon_core::market::candles::{CandleSeries, candle_intersects_window};

/// Builds one volume-only candle for a deterministic visibility scenario.
fn candle(t_open_ms: f64, volume: f32) -> ChartCandle {
    ChartCandle {
        t_open_ms,
        volume,
        ..ChartCandle::default()
    }
}

/// `volume_bars.rs:visible_volume_stats` — changing the half-open visibility predicate or
/// averaging all samples would scale the band and its labels from off-screen candles.
#[test]
fn visible_stats_follow_the_shared_half_open_window_over_only_visible_buckets() {
    let samples = vec![
        VolumeSample {
            t_open_ms: 0.0,
            tf_ms: 10.0,
            volume: 100.0,
        },
        VolumeSample {
            t_open_ms: 5.0,
            tf_ms: 10.0,
            volume: 10.0,
        },
        VolumeSample {
            t_open_ms: 20.0,
            tf_ms: 10.0,
            volume: 30.0,
        },
    ];

    let stats = visible_volume_stats(&samples, 10.0, 20.0).expect("two buckets intersect");
    assert_eq!(stats.max, 30.0);
    assert_eq!(stats.avg, 20.0);
    assert_eq!(stats.count, 2);

    assert!(!candle_intersects_window(0.0, 10.0, 10.0, 20.0));
    assert!(candle_intersects_window(5.0, 10.0, 10.0, 20.0));
    assert!(candle_intersects_window(20.0, 10.0, 10.0, 20.0));

    let base = [candle(0.0, 100.0), candle(5.0, 10.0), candle(20.0, 30.0)];
    let mut series = CandleSeries::default();
    series.rebuild(10, &base, 10, &[]);
    assert_eq!(
        visible_volume_stats(&samples, 10.0, 20.0).is_some(),
        series.price_range(10.0, 20.0).is_some(),
        "volume scaling and the price range must include the same half-open candle set"
    );
}

/// `volume_bars.rs:collect_samples` — replacing each candle's own timeframe with the series
/// timeframe would omit coarse history-tail buckets that are visibly intersecting the chart.
#[test]
fn collected_samples_keep_each_candles_timeframe_for_visibility() {
    let mut samples = Vec::new();
    collect_samples(
        &[candle(0.0, 4.0), candle(50.0, 8.0)],
        &[0.0, 100.0],
        10.0,
        &mut samples,
    );

    assert_eq!(samples[0].tf_ms, 10.0);
    assert_eq!(samples[1].tf_ms, 100.0);
    let stats = visible_volume_stats(&samples, 120.0, 130.0).expect("coarse bucket intersects");
    assert_eq!(
        stats,
        VolumeStats {
            max: 8.0,
            avg: 8.0,
            count: 1
        }
    );
}

/// `volume_bars.rs:visible_volume_stats` — returning a zero maximum for empty or zero buckets
/// would let the chart upload an invalid reciprocal and produce a broken volume band.
#[test]
fn visible_stats_refuse_empty_and_zero_volume_windows() {
    assert_eq!(visible_volume_stats(&[], 0.0, 10.0), None);
    let zeros = [VolumeSample {
        t_open_ms: 0.0,
        tf_ms: 10.0,
        volume: 0.0,
    }];
    assert_eq!(visible_volume_stats(&zeros, 0.0, 10.0), None);
}

/// `volume_bars.rs:clamp_band_fraction` — returning `frac` unchanged (dropping the clamp)
/// lets a hand-edited `theme.toml` push the band past half the plot, so the footer swallows
/// the price action it is supposed to annotate. With the pixel ceiling gone this clamp is
/// the only remaining bound on band height.
#[test]
fn clamp_band_fraction_holds_a_hand_edited_height_inside_the_named_footer_range() {
    // Boundary pair: at each named limit the fraction is unchanged.
    assert_eq!(clamp_band_fraction(VOLUME_HEIGHT_MIN), VOLUME_HEIGHT_MIN);
    assert_eq!(clamp_band_fraction(VOLUME_HEIGHT_MAX), VOLUME_HEIGHT_MAX);
    // Boundary pair: one step past each limit is pulled back.
    assert_eq!(
        clamp_band_fraction(VOLUME_HEIGHT_MIN - 0.001),
        VOLUME_HEIGHT_MIN
    );
    assert_eq!(
        clamp_band_fraction(VOLUME_HEIGHT_MAX + 0.001),
        VOLUME_HEIGHT_MAX
    );
    // A value strictly inside is unchanged — the clamp is not a snap-to-bound.
    let inside = (VOLUME_HEIGHT_MIN + VOLUME_HEIGHT_MAX) * 0.5;
    assert_eq!(clamp_band_fraction(inside), inside);
    // Non-finite input must not leak a NaN/Inf height into the shader.
    assert_eq!(clamp_band_fraction(f32::NAN), VOLUME_HEIGHT_MIN);
    assert_eq!(clamp_band_fraction(f32::INFINITY), VOLUME_HEIGHT_MIN);
    assert_eq!(clamp_band_fraction(f32::NEG_INFINITY), VOLUME_HEIGHT_MIN);
}

/// `volume_bars.rs:quantize_inv_max` — removing relative quantization or its positive-input
/// guard would either rebake on ordinary live ticks or send a zero/NaN scale to the shader.
#[test]
fn quantized_inverse_max_is_positive_monotone_and_stable_for_small_relative_changes() {
    let low = quantize_inv_max(0.50);
    let nearby = quantize_inv_max(0.5001);
    let high = quantize_inv_max(0.75);

    assert!(low.is_finite() && low > 0.0);
    assert!(high.is_finite() && high > 0.0);
    assert!(low <= high);
    assert_eq!(low, nearby);
}
